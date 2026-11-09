use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Local;

use crate::archive;
use crate::config::{Config, RestoreConfig};
use crate::manifest::Manifest;
use crate::paths::unique_source_names;
use crate::progress::{ConsoleProgress, ProgressReporter};
use crate::update;
use crate::vss::VssSession;

pub fn backup(config_path: &PathBuf) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut log = Logger::new(&config.log_path())?;
    let mut progress = ConsoleProgress::new();
    log.info("backup started")?;
    if config.behavior.check_for_updates {
        match update::newer_version() {
            Ok(Some(version)) => log.info(&format!(
                "new version {version} is available. run `auto_backup update` to update"
            ))?,
            Ok(None) => {}
            Err(err) => log.warn(&format!("update check failed: {err:#}"))?,
        }
    }

    std::fs::create_dir_all(&config.destination.path).with_context(|| {
        format!(
            "failed to create destination {}",
            config.destination.path.display()
        )
    })?;
    let work_dir = WorkDir::create(&config.work_dir())?;

    let names = unique_source_names(&config.source.paths);
    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();

    // スナップショット作成は数十秒かかることがあるので、待っている間も進捗スレッドに表示させる
    progress.start("creating VSS snapshot", None);
    let (vss_session, vss_logs) = VssSession::new(&config.source.paths);
    progress.finish();
    for line in &vss_logs {
        log.info(line)?;
    }

    for (source, source_name) in config.source.paths.iter().zip(names.iter()) {
        if !source.is_dir() {
            bail!("source path is not a directory: {}", source.display());
        }

        let excludes =
            archive::Excludes::new(&config.exclude.patterns, &config.exclude.paths, source)?;
        let source_read = vss_session.resolve(source);
        log.info(&format!(
            "{} for {}",
            if source_read.used_vss {
                "VSS snapshot used"
            } else {
                "direct read (no VSS)"
            },
            source.display()
        ))?;

        let base_name = format!("{timestamp}_{source_name}");
        log.info(&format!("creating archive for {}", source.display()))?;
        let raw_archive = work_dir.path.join(format!("{base_name}.tmp.tar.zst"));
        let skipped_files = archive::create_tar_zst_with_progress(
            &source_read.read_path,
            &raw_archive,
            config.archive.compression_level,
            &excludes,
            config.behavior.skip_unreadable_files,
            &mut progress,
        )?;

        let (mut manifest, mut local_files) = archive::split_archive_with_progress(
            &raw_archive,
            &base_name,
            config.max_part_size_bytes(),
            timestamp.clone(),
            source.clone(),
            &mut progress,
        )?;
        manifest.skipped_files = skipped_files;
        for skipped_file in &manifest.skipped_files {
            log.warn(&format!(
                "skipped unreadable file {}: {}",
                skipped_file.path.display(),
                skipped_file.reason
            ))?;
        }
        let manifest_path = work_dir.path.join(format!("{base_name}.manifest.toml"));
        manifest.write_to(&manifest_path)?;
        local_files.push(manifest_path);

        copy_verify_and_cleanup(&config, &manifest, &local_files, &mut log, &mut progress)?;
    }

    log.info("backup finished")?;
    Ok(())
}

pub fn restore(config_path: Option<&PathBuf>, input: &Path, output: &Path) -> Result<()> {
    let config = RestoreConfig::load(config_path)?;
    let mut log = Logger::new(&config.log_path())?;
    let mut progress = ConsoleProgress::new();
    log.info("restore started")?;

    let input = input
        .canonicalize()
        .with_context(|| format!("restore input not found: {}", input.display()))?;

    // 結合ファイルと作業フォルダは、成功・失敗どちらでも Drop で片付く
    let combined;
    let archive_to_restore: &Path = if input.extension().and_then(|v| v.to_str()) == Some("toml") {
        combined = combine_in_work_dir(&config.work_dir(), &input, &mut progress)?;
        &combined.path
    } else if input
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|name| name.ends_with(".tar.zst"))
    {
        verify_single_archive_if_manifest_exists(&input)?;
        &input
    } else {
        let manifest_path = detect_manifest_for_part(&input)?;
        combined = combine_in_work_dir(&config.work_dir(), &manifest_path, &mut progress)?;
        &combined.path
    };

    archive::restore_tar_zst_with_progress(archive_to_restore, output, &mut progress)?;
    log.info(&format!("restore finished to {}", output.display()))?;
    Ok(())
}

/// 作業フォルダ。実行中に新しく作った場合だけ、Drop 時に作った最上位まで空のフォルダを消す。
/// 中身が残っていれば(他の実行のファイル、失敗時の作業ファイルなど)消さずに残す。
struct WorkDir {
    path: PathBuf,
    created_top: Option<PathBuf>,
}

impl WorkDir {
    fn create(path: &Path) -> Result<Self> {
        let created_top = path
            .ancestors()
            .take_while(|dir| !dir.as_os_str().is_empty() && !dir.exists())
            .last()
            .map(Path::to_path_buf);
        std::fs::create_dir_all(path)
            .with_context(|| format!("failed to create work directory {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            created_top,
        })
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let Some(top) = &self.created_top else {
            return;
        };
        for dir in self.path.ancestors() {
            // remove_dir は空でなければ失敗するので、他人のファイルには触れない
            if std::fs::remove_dir(dir).is_err() || dir == top {
                break;
            }
        }
    }
}

/// 作業フォルダ内に結合した Archive。Drop でファイルを消し、続けて作業フォルダを片付ける。
struct CombinedArchive {
    path: PathBuf,
    _work_dir: WorkDir,
}

impl Drop for CombinedArchive {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

fn combine_in_work_dir(
    work_dir: &Path,
    manifest_path: &PathBuf,
    progress: &mut impl ProgressReporter,
) -> Result<CombinedArchive> {
    let manifest = Manifest::read_from(manifest_path)?;
    let manifest_dir = manifest_path
        .parent()
        .context("manifest has no parent directory")?;
    let work_dir = WorkDir::create(work_dir)?;
    let combined = CombinedArchive {
        path: work_dir
            .path
            .join(format!("{}.restore.tar.zst", manifest.archive_name)),
        _work_dir: work_dir,
    };
    archive::combine_parts_with_progress(&manifest, manifest_dir, &combined.path, progress)?;
    Ok(combined)
}

fn copy_verify_and_cleanup(
    config: &Config,
    manifest: &Manifest,
    local_files: &[PathBuf],
    log: &mut Logger,
    progress: &mut impl ProgressReporter,
) -> Result<()> {
    let mut copied_files = Vec::new();
    for local_path in local_files {
        let file_name = local_path
            .file_name()
            .context("local archive file has no file name")?;
        let destination_path = config.destination.path.join(file_name);
        if destination_path.exists() {
            cleanup_files(&copied_files);
            bail!(
                "destination file already exists: {}",
                destination_path.display()
            );
        }

        let copy_result = copy_file_with_progress(local_path, &destination_path, progress)
            .with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    local_path.display(),
                    destination_path.display()
                )
            });
        if let Err(error) = copy_result {
            cleanup_files(&copied_files);
            return Err(error);
        }
        copied_files.push(destination_path);
    }

    let verify_result: Result<()> = (|| {
        if !config.behavior.verify_after_copy {
            return Ok(());
        }

        for part in &manifest.parts {
            let copied_path = config.destination.path.join(&part.file_name);
            archive::verify_file_with_progress(&copied_path, &part.sha256, part.size, progress)?;
        }
        let manifest_name = format!("{}.manifest.toml", manifest.archive_name);
        let copied_manifest = config.destination.path.join(&manifest_name);
        let local_manifest = local_files
            .iter()
            .find(|path| path.file_name().and_then(|v| v.to_str()) == Some(manifest_name.as_str()))
            .context("local manifest was not staged")?;
        let expected_manifest_sha = archive::sha256_file_with_progress(local_manifest, progress)?;
        let expected_manifest_size = local_manifest.metadata()?.len();
        archive::verify_file_with_progress(
            &copied_manifest,
            &expected_manifest_sha,
            expected_manifest_size,
            progress,
        )?;
        Ok(())
    })();

    if let Err(error) = verify_result {
        cleanup_files(&copied_files);
        return Err(error);
    }

    if config.behavior.delete_local_archive_after_copy {
        for local_path in local_files {
            std::fs::remove_file(local_path).with_context(|| {
                format!(
                    "failed to remove local staging file {}",
                    local_path.display()
                )
            })?;
        }
    }

    log.info(&format!(
        "copied and verified {} part(s) for {}",
        manifest.parts.len(),
        manifest.archive_name
    ))?;
    Ok(())
}

fn copy_file_with_progress(
    source: &Path,
    destination: &Path,
    progress: &mut impl ProgressReporter,
) -> Result<u64> {
    let total_size = source
        .metadata()
        .with_context(|| format!("failed to read metadata for {}", source.display()))?
        .len();
    progress.start("copy bytes", Some(total_size));

    let mut reader = BufReader::new(std::fs::File::open(source)?);
    let mut writer = BufWriter::new(std::fs::File::create(destination)?);
    let mut copied = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        writer.write_all(&buffer[..bytes_read])?;
        copied += bytes_read as u64;
        progress.advance(bytes_read as u64);
    }

    writer.flush()?;
    progress.finish();
    Ok(copied)
}

fn cleanup_files(paths: &[PathBuf]) {
    for path in paths {
        std::fs::remove_file(path).ok();
    }
}

fn detect_manifest_for_part(part_path: &Path) -> Result<PathBuf> {
    let file_name = part_path
        .file_name()
        .and_then(|v| v.to_str())
        .context("input part has no valid file name")?;
    let base = file_name
        .split(".part")
        .next()
        .context("input does not look like a split part")?;
    let manifest = part_path
        .parent()
        .context("input part has no parent directory")?
        .join(format!("{base}.manifest.toml"));
    if !manifest.exists() {
        bail!(
            "could not find manifest for split part. expected {}",
            manifest.display()
        );
    }
    Ok(manifest)
}

fn verify_single_archive_if_manifest_exists(archive_path: &Path) -> Result<()> {
    let file_name = archive_path
        .file_name()
        .and_then(|v| v.to_str())
        .context("archive has no valid file name")?;
    let base = file_name
        .strip_suffix(".tar.zst")
        .context("archive does not end with .tar.zst")?;
    let manifest_path = archive_path
        .parent()
        .context("archive has no parent directory")?
        .join(format!("{base}.manifest.toml"));

    if manifest_path.exists() {
        let manifest = Manifest::read_from(&manifest_path)?;
        if manifest.parts.len() == 1 {
            let part = &manifest.parts[0];
            archive::verify_file(archive_path, &part.sha256, part.size)?;
        }
    }

    Ok(())
}

struct Logger {
    file: std::fs::File,
}

impl Logger {
    fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("failed to open log {}", path.display()))?;
        Ok(Self { file })
    }

    fn info(&mut self, message: &str) -> Result<()> {
        self.write("INFO", message)
    }

    fn warn(&mut self, message: &str) -> Result<()> {
        self.write("WARN", message)
    }

    fn write(&mut self, level: &str, message: &str) -> Result<()> {
        let timestamp = Local::now().to_rfc3339();
        writeln!(self.file, "[{timestamp}] {level} {message}")?;
        println!("{message}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "auto_backup_app_{name}_{}_{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).expect("failed to create test root");
        root
    }

    #[test]
    fn work_dir_removes_created_ancestors_up_to_existing_one() {
        let root = temp_root("nested");
        let path = root.join("a").join("b").join("work");
        drop(WorkDir::create(&path).expect("create"));
        assert!(!root.join("a").exists());
        assert!(root.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn work_dir_keeps_preexisting_directory() {
        let root = temp_root("existing");
        drop(WorkDir::create(&root).expect("create"));
        assert!(root.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn work_dir_keeps_non_empty_directory() {
        let root = temp_root("non_empty");
        let path = root.join("a").join("work");
        let work_dir = WorkDir::create(&path).expect("create");
        std::fs::write(path.join("other.tmp"), b"x").expect("write");
        drop(work_dir);
        assert!(path.join("other.tmp").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn combined_archive_cleans_up_file_and_work_dir_on_failure() {
        let root = temp_root("combine_fail");
        let work = root.join("work");
        // Part が存在しないので、結合ファイルを作った後で失敗する
        let manifest_path = root.join("x.manifest.toml");
        Manifest {
            version: 1,
            created_at: String::new(),
            source_path: PathBuf::new(),
            archive_name: "x".to_string(),
            full_sha256: String::new(),
            full_size: 1,
            parts: vec![crate::manifest::Part {
                file_name: "x.part001".to_string(),
                sha256: String::new(),
                size: 1,
            }],
            skipped_files: Vec::new(),
        }
        .write_to(&manifest_path)
        .expect("write manifest");
        let result = combine_in_work_dir(&work, &manifest_path, &mut crate::progress::NoProgress);
        assert!(result.is_err());
        assert!(!work.exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
