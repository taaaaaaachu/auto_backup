use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Local;

use crate::archive;
use crate::config::Config;
use crate::manifest::Manifest;
use crate::paths::unique_source_names;

pub fn backup(config_path: &PathBuf) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut log = Logger::new(&config.log_path())?;
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

    for (source, source_name) in config.source.paths.iter().zip(names.iter()) {
        if !source.is_dir() {
            bail!("source path is not a directory: {}", source.display());
        }

        let base_name = format!("{timestamp}_{source_name}");
        log.info(&format!("creating archive for {}", source.display()))?;
        let raw_archive = work_dir.join(format!("{base_name}.tmp.tar.zst"));
        archive::create_tar_zst(
            source,
            &raw_archive,
            config.archive.compression_level,
            &excludes,
        )?;

        let (manifest, mut local_files) = archive::split_archive(
            &raw_archive,
            &base_name,
            config.max_part_size_bytes(),
            timestamp.clone(),
            source.clone(),
        )?;
        let manifest_path = work_dir.join(format!("{base_name}.manifest.toml"));
        manifest.write_to(&manifest_path)?;
        local_files.push(manifest_path);

        copy_verify_and_cleanup(&config, &manifest, &local_files, &mut log)?;
    }

    log.info("backup finished")?;
    Ok(())
}

pub fn restore(config_path: &PathBuf, input: &PathBuf, output: &PathBuf) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut log = Logger::new(&config.log_path())?;
    log.info("restore started")?;

    let work_dir = config.work_dir();
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("failed to create work directory {}", work_dir.display()))?;

    let input = input
        .canonicalize()
        .with_context(|| format!("restore input not found: {}", input.display()))?;

    let (archive_to_restore, remove_after_restore) =
        if input.extension().and_then(|v| v.to_str()) == Some("toml") {
            let manifest = Manifest::read_from(&input)?;
            let manifest_dir = input.parent().context("manifest has no parent directory")?;
            let combined = work_dir.join(format!("{}.restore.tar.zst", manifest.archive_name));
            archive::combine_parts(&manifest, manifest_dir, &combined)?;
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
            archive::combine_parts(&manifest, manifest_dir, &combined)?;
            (combined, true)
        };

    archive::restore_tar_zst(&archive_to_restore, output)?;
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

        let copy_result = std::fs::copy(local_path, &destination_path).with_context(|| {
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
            archive::verify_file(&copied_path, &part.sha256, part.size)?;
        }
        let manifest_name = format!("{}.manifest.toml", manifest.archive_name);
        let copied_manifest = config.destination.path.join(&manifest_name);
        let local_manifest = local_files
            .iter()
            .find(|path| path.file_name().and_then(|v| v.to_str()) == Some(manifest_name.as_str()))
            .context("local manifest was not staged")?;
        let expected_manifest_sha = archive::sha256_file(local_manifest)?;
        let expected_manifest_size = local_manifest.metadata()?.len();
        archive::verify_file(
            &copied_manifest,
            &expected_manifest_sha,
            expected_manifest_size,
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
        let timestamp = Local::now().to_rfc3339();
        writeln!(self.file, "[{timestamp}] INFO {message}")?;
        println!("{message}");
        Ok(())
    }
}
