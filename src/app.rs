use std::fs::OpenOptions;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Local;

use crate::archive;
use crate::config::Config;
use crate::manifest::Manifest;
use crate::paths::unique_source_names;
use crate::progress::{ConsoleProgress, ProgressReporter};
use crate::vss::VssSession;

pub fn backup(config_path: &PathBuf) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut log = Logger::new(&config.log_path())?;
    let mut progress = ConsoleProgress::new();
    log.info("backup started")?;

    std::fs::create_dir_all(&config.destination.path).with_context(|| {
        format!(
            "failed to create destination {}",
            config.destination.path.display()
        )
    })?;
    let work_dir = config.work_dir();
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create work directory {}", work_dir.display()))?;

    let excludes = archive::build_exclude_set(&config.exclude.patterns)?;
    let names = unique_source_names(&config.source.paths);
    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();

    let (vss_session, vss_logs) = VssSession::new(&config.source.paths);
    for line in &vss_logs {
        log.info(line)?;
    }

    for (source, source_name) in config.source.paths.iter().zip(names.iter()) {
        if !source.is_dir() {
            bail!("source path is not a directory: {}", source.display());
        }

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
        let raw_archive = work_dir.join(format!("{base_name}.tmp.tar.zst"));
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
        let manifest_path = work_dir.join(format!("{base_name}.manifest.toml"));
        manifest.write_to(&manifest_path)?;
        local_files.push(manifest_path);

        copy_verify_and_cleanup(&config, &manifest, &local_files, &mut log, &mut progress)?;
    }

    log.info("backup finished")?;
    Ok(())
}

pub fn restore(config_path: &PathBuf, input: &PathBuf, output: &PathBuf) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut log = Logger::new(&config.log_path())?;
    let mut progress = ConsoleProgress::new();
    log.info("restore started")?;

    let work_dir = config.work_dir();
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create work directory {}", work_dir.display()))?;

    let input = input
        .canonicalize()
        .with_context(|| format!("restore input not found: {}", input.display()))?;

    let (archive_to_restore, remove_after_restore) = if input.extension().and_then(|v| v.to_str())
        == Some("toml")
    {
        let manifest = Manifest::read_from(&input)?;
        let manifest_dir = input.parent().context("manifest has no parent directory")?;
        let combined = work_dir.join(format!("{}.restore.tar.zst", manifest.archive_name));
        archive::combine_parts_with_progress(&manifest, manifest_dir, &combined, &mut progress)?;
        (combined, true)
    } else if input
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|name| name.ends_with(".tar.zst"))
    {
        verify_single_archive_if_manifest_exists(&input)?;
        (input, false)
    } else {
        let manifest_path = detect_manifest_for_part(&input)?;
        let manifest = Manifest::read_from(&manifest_path)?;
        let manifest_dir = manifest_path
            .parent()
            .context("manifest has no parent directory")?;
        let combined = work_dir.join(format!("{}.restore.tar.zst", manifest.archive_name));
        archive::combine_parts_with_progress(&manifest, manifest_dir, &combined, &mut progress)?;
        (combined, true)
    };

    archive::restore_tar_zst_with_progress(&archive_to_restore, output, &mut progress)?;
    if remove_after_restore {
        std::fs::remove_file(&archive_to_restore).ok();
    }
    log.info(&format!("restore finished to {}", output.display()))?;
    Ok(())
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
