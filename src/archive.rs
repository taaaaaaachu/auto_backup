use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::manifest::{Manifest, Part, SkippedFile};
use crate::progress::{NoProgress, ProgressReporter};

pub fn build_exclude_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid exclude pattern {pattern}"))?);
    }
    builder.build().context("failed to build exclude matcher")
}

pub fn create_tar_zst_with_progress(
    source: &Path,
    output: &Path,
    compression_level: i32,
    excludes: &GlobSet,
    skip_unreadable_files: bool,
    progress: &mut impl ProgressReporter,
) -> Result<Vec<SkippedFile>> {
    let total_entries = count_archive_entries(source, excludes, skip_unreadable_files)?;
    progress.start("archive entries", Some(total_entries));

    let file = File::create(output)
        .with_context(|| format!("failed to create archive {}", output.display()))?;
    let encoder = zstd::Encoder::new(BufWriter::new(file), compression_level)
        .context("failed to create zstd encoder")?;
    let mut tar = tar::Builder::new(encoder);
    let mut skipped_files = Vec::new();

    for entry in WalkDir::new(source).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if skip_unreadable_files => {
                skipped_files.push(SkippedFile {
                    path: error
                        .path()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| source.to_path_buf()),
                    reason: error.to_string(),
                });
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("failed to walk {}", source.display()))
            }
        };
        let path = entry.path();
        if path == source {
            continue;
        }

        let relative = path
            .strip_prefix(source)
            .with_context(|| format!("failed to make path relative: {}", path.display()))?;

        if excludes.is_match(relative) {
            continue;
        }

        if entry.file_type().is_dir() {
            tar.append_dir(relative, path)
                .with_context(|| format!("failed to append directory {}", path.display()))?;
            progress.advance(1);
        } else if entry.file_type().is_file() {
            if let Err(error) = tar.append_path_with_name(path, relative) {
                if skip_unreadable_files {
                    skipped_files.push(SkippedFile {
                        path: path.to_path_buf(),
                        reason: error.to_string(),
                    });
                    progress.advance(1);
                    continue;
                }

                return Err(error)
                    .with_context(|| format!("failed to append file {}", path.display()));
            }
            progress.advance(1);
        }
    }

    let encoder = tar.into_inner().context("failed to finish tar stream")?;
    encoder.finish().context("failed to finish zstd stream")?;
    progress.finish();
    Ok(skipped_files)
}

pub fn split_archive_with_progress(
    archive_path: &Path,
    base_name: &str,
    max_part_size: u64,
    created_at: String,
    source_path: PathBuf,
    progress: &mut impl ProgressReporter,
) -> Result<(Manifest, Vec<PathBuf>)> {
    let full_sha256 = sha256_file_with_progress(archive_path, progress)?;
    let full_size = archive_path.metadata()?.len();
    let parent = archive_path
        .parent()
        .context("archive path must have a parent directory")?;

    progress.start("split archive bytes", Some(full_size));
    if full_size <= max_part_size {
        let file_name = format!("{base_name}.tar.zst");
        let final_path = parent.join(&file_name);
        std::fs::rename(archive_path, &final_path).with_context(|| {
            format!(
                "failed to move archive {} to {}",
                archive_path.display(),
                final_path.display()
            )
        })?;
        let part = Part {
            file_name,
            sha256: full_sha256.clone(),
            size: full_size,
        };
        let manifest = Manifest {
            version: 1,
            created_at,
            source_path,
            archive_name: base_name.to_string(),
            full_sha256,
            full_size,
            parts: vec![part],
            skipped_files: Vec::new(),
        };
        progress.advance(full_size);
        progress.finish();
        return Ok((manifest, vec![final_path]));
    }

    let mut input = BufReader::new(File::open(archive_path)?);
    let mut parts = Vec::new();
    let mut paths = Vec::new();
    let mut index = 1usize;

    loop {
        let file_name = format!("{base_name}.part{index:03}");
        let part_path = parent.join(&file_name);
        let mut output = BufWriter::new(File::create(&part_path)?);
        let mut remaining = max_part_size;
        let mut written = 0u64;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];

        while remaining > 0 {
            let read_len = buffer.len().min(remaining as usize);
            let bytes_read = input.read(&mut buffer[..read_len])?;
            if bytes_read == 0 {
                break;
            }
            output.write_all(&buffer[..bytes_read])?;
            hasher.update(&buffer[..bytes_read]);
            written += bytes_read as u64;
            remaining -= bytes_read as u64;
            progress.advance(bytes_read as u64);
        }

        output.flush()?;
        if written == 0 {
            std::fs::remove_file(&part_path).ok();
            break;
        }

        parts.push(Part {
            file_name,
            sha256: hex::encode(hasher.finalize()),
            size: written,
        });
        paths.push(part_path);
        index += 1;
    }

    std::fs::remove_file(archive_path).ok();
    let manifest = Manifest {
        version: 1,
        created_at,
        source_path,
        archive_name: base_name.to_string(),
        full_sha256,
        full_size,
        parts,
        skipped_files: Vec::new(),
    };
    progress.finish();
    Ok((manifest, paths))
}

pub fn sha256_file_with_progress(
    path: &Path,
    progress: &mut impl ProgressReporter,
) -> Result<String> {
    let total_size = path
        .metadata()
        .with_context(|| format!("failed to read metadata for checksum {}", path.display()))?
        .len();
    progress.start("checksum bytes", Some(total_size));
    let mut file = BufReader::new(
        File::open(path)
            .with_context(|| format!("failed to open file for checksum {}", path.display()))?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
        progress.advance(bytes_read as u64);
    }

    progress.finish();
    Ok(hex::encode(hasher.finalize()))
}

pub fn verify_file(path: &Path, expected_sha256: &str, expected_size: u64) -> Result<()> {
    verify_file_with_progress(path, expected_sha256, expected_size, &mut NoProgress)
}

pub fn verify_file_with_progress(
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
    progress: &mut impl ProgressReporter,
) -> Result<()> {
    let metadata = path
        .metadata()
        .with_context(|| format!("missing copied file {}", path.display()))?;
    if metadata.len() != expected_size {
        bail!(
            "size mismatch for {}: expected {}, got {}",
            path.display(),
            expected_size,
            metadata.len()
        );
    }

    let actual = sha256_file_with_progress(path, progress)?;
    if actual != expected_sha256 {
        bail!("checksum mismatch for {}", path.display());
    }
    Ok(())
}

pub fn combine_parts_with_progress(
    manifest: &Manifest,
    manifest_dir: &Path,
    output: &Path,
    progress: &mut impl ProgressReporter,
) -> Result<()> {
    let mut writer = BufWriter::new(File::create(output)?);
    let mut full_hasher = Sha256::new();
    let mut total_size = 0u64;

    progress.start("combine bytes", Some(manifest.full_size));
    for part in &manifest.parts {
        let part_path = manifest_dir.join(&part.file_name);
        verify_file(&part_path, &part.sha256, part.size)?;

        let mut reader = BufReader::new(File::open(&part_path)?);
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            writer.write_all(&buffer[..bytes_read])?;
            full_hasher.update(&buffer[..bytes_read]);
            total_size += bytes_read as u64;
            progress.advance(bytes_read as u64);
        }
    }

    writer.flush()?;
    if total_size != manifest.full_size {
        bail!(
            "combined archive size mismatch: expected {}, got {}",
            manifest.full_size,
            total_size
        );
    }
    if hex::encode(full_hasher.finalize()) != manifest.full_sha256 {
        bail!("combined archive checksum mismatch");
    }
    progress.finish();
    Ok(())
}

pub fn restore_tar_zst_with_progress(
    archive_path: &Path,
    output_dir: &Path,
    progress: &mut impl ProgressReporter,
) -> Result<()> {
    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create restore directory {}",
            output_dir.display()
        )
    })?;
    let total_entries = ensure_no_existing_entries(archive_path, output_dir)?;
    progress.start("restore entries", Some(total_entries));

    let file = File::open(archive_path)?;
    let decoder =
        zstd::Decoder::new(BufReader::new(file)).context("failed to create zstd decoder")?;
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .context("failed to read archive entries")?
    {
        let mut entry = entry?;
        entry
            .unpack_in(output_dir)
            .with_context(|| format!("failed to unpack archive to {}", output_dir.display()))?;
        progress.advance(1);
    }
    progress.finish();
    Ok(())
}

fn ensure_no_existing_entries(archive_path: &Path, output_dir: &Path) -> Result<u64> {
    let file = File::open(archive_path)?;
    let decoder =
        zstd::Decoder::new(BufReader::new(file)).context("failed to create zstd decoder")?;
    let mut archive = tar::Archive::new(decoder);
    let mut count = 0u64;
    for entry in archive
        .entries()
        .context("failed to read archive entries")?
    {
        let entry = entry?;
        let entry_path = entry.path()?.into_owned();
        if entry_path.is_absolute()
            || entry_path
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            bail!("archive contains unsafe path {}", entry_path.display());
        }

        let target = output_dir.join(&entry_path);
        if target.exists() {
            bail!("restore target already exists: {}", target.display());
        }
        count += 1;
    }

    Ok(count)
}

fn count_archive_entries(
    source: &Path,
    excludes: &GlobSet,
    skip_unreadable_files: bool,
) -> Result<u64> {
    let mut count = 0u64;
    for entry in WalkDir::new(source).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if skip_unreadable_files => {
                let _ = error;
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("failed to walk {}", source.display()))
            }
        };
        let path = entry.path();
        if path == source {
            continue;
        }
        let relative = path
            .strip_prefix(source)
            .with_context(|| format!("failed to make path relative: {}", path.display()))?;
        if excludes.is_match(relative) {
            continue;
        }
        if entry.file_type().is_dir() || entry.file_type().is_file() {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[derive(Default)]
    struct RecordingProgress {
        starts: Vec<(String, Option<u64>)>,
        advanced_by: u64,
        finishes: usize,
    }

    impl ProgressReporter for RecordingProgress {
        fn start(&mut self, label: &str, total: Option<u64>) {
            self.starts.push((label.to_string(), total));
        }

        fn advance(&mut self, amount: u64) {
            self.advanced_by += amount;
        }

        fn finish(&mut self) {
            self.finishes += 1;
        }
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "auto_backup_{name}_{}_{}",
                std::process::id(),
                suffix
            ));
            std::fs::create_dir_all(&path).expect("failed to create test directory");
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.path).ok();
        }
    }

    #[test]
    fn create_tar_zst_reports_entry_progress() {
        let dir = TestDir::new("archive_progress");
        let source = dir.path.join("source");
        std::fs::create_dir(&source).expect("failed to create source");
        std::fs::write(source.join("hello.txt"), "hello").expect("failed to write source file");
        let output = dir.path.join("backup.tar.zst");
        let excludes = build_exclude_set(&[]).expect("failed to build excludes");
        let mut progress = RecordingProgress::default();

        create_tar_zst_with_progress(&source, &output, 1, &excludes, false, &mut progress)
            .expect("failed to create archive");

        assert_eq!(
            progress.starts,
            vec![("archive entries".to_string(), Some(1))]
        );
        assert_eq!(progress.advanced_by, 1);
        assert_eq!(progress.finishes, 1);
        assert!(output.exists());
    }

    #[test]
    fn split_archive_reports_byte_progress() {
        let dir = TestDir::new("split_progress");
        let archive = dir.path.join("source.tmp.tar.zst");
        let archive_bytes = vec![42u8; 10];
        std::fs::write(&archive, &archive_bytes).expect("failed to write archive");
        let mut progress = RecordingProgress::default();

        let (_manifest, parts) = split_archive_with_progress(
            &archive,
            "source",
            4,
            "2026-06-14T00:00:00Z".to_string(),
            dir.path.join("source"),
            &mut progress,
        )
        .expect("failed to split archive");

        assert_eq!(
            progress.starts,
            vec![
                ("checksum bytes".to_string(), Some(10)),
                ("split archive bytes".to_string(), Some(10)),
            ]
        );
        assert_eq!(progress.advanced_by, 20);
        assert_eq!(progress.finishes, 2);
        assert_eq!(parts.len(), 3);
    }
}
