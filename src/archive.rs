use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::manifest::{Manifest, Part, SkippedFile};

pub fn build_exclude_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid exclude pattern {pattern}"))?);
    }
    builder.build().context("failed to build exclude matcher")
}

pub fn create_tar_zst(
    source: &Path,
    output: &Path,
    compression_level: i32,
    excludes: &GlobSet,
    skip_unreadable_files: bool,
) -> Result<Vec<SkippedFile>> {
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
        } else if entry.file_type().is_file() {
            if let Err(error) = tar.append_path_with_name(path, relative) {
                if skip_unreadable_files {
                    skipped_files.push(SkippedFile {
                        path: path.to_path_buf(),
                        reason: error.to_string(),
                    });
                    continue;
                }

                return Err(error)
                    .with_context(|| format!("failed to append file {}", path.display()));
            }
        }
    }

    let encoder = tar.into_inner().context("failed to finish tar stream")?;
    encoder.finish().context("failed to finish zstd stream")?;
    Ok(skipped_files)
}

pub fn split_archive(
    archive_path: &Path,
    base_name: &str,
    max_part_size: u64,
    created_at: String,
    source_path: PathBuf,
) -> Result<(Manifest, Vec<PathBuf>)> {
    let full_sha256 = sha256_file(archive_path)?;
    let full_size = archive_path.metadata()?.len();
    let parent = archive_path
        .parent()
        .context("archive path must have a parent directory")?;

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
    Ok((manifest, paths))
}

pub fn sha256_file(path: &Path) -> Result<String> {
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
    }

    Ok(hex::encode(hasher.finalize()))
}

pub fn verify_file(path: &Path, expected_sha256: &str, expected_size: u64) -> Result<()> {
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

    let actual = sha256_file(path)?;
    if actual != expected_sha256 {
        bail!("checksum mismatch for {}", path.display());
    }
    Ok(())
}

pub fn combine_parts(manifest: &Manifest, manifest_dir: &Path, output: &Path) -> Result<()> {
    let mut writer = BufWriter::new(File::create(output)?);
    let mut full_hasher = Sha256::new();
    let mut total_size = 0u64;

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
    Ok(())
}

pub fn restore_tar_zst(archive_path: &Path, output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "failed to create restore directory {}",
            output_dir.display()
        )
    })?;
    ensure_no_existing_entries(archive_path, output_dir)?;

    let file = File::open(archive_path)?;
    let decoder =
        zstd::Decoder::new(BufReader::new(file)).context("failed to create zstd decoder")?;
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(output_dir)
        .with_context(|| format!("failed to unpack archive to {}", output_dir.display()))?;
    Ok(())
}

fn ensure_no_existing_entries(archive_path: &Path, output_dir: &Path) -> Result<()> {
    let file = File::open(archive_path)?;
    let decoder =
        zstd::Decoder::new(BufReader::new(file)).context("failed to create zstd decoder")?;
    let mut archive = tar::Archive::new(decoder);
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
    }

    Ok(())
}
