use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub created_at: String,
    pub source_path: PathBuf,
    pub archive_name: String,
    pub full_sha256: String,
    pub full_size: u64,
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_files: Vec<SkippedFile>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Part {
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SkippedFile {
    pub path: PathBuf,
    pub reason: String,
}

impl Manifest {
    pub fn write_to(&self, path: &PathBuf) -> Result<()> {
        let text = toml::to_string_pretty(self).context("failed to serialize manifest")?;
        std::fs::write(path, text)
            .with_context(|| format!("failed to write manifest {}", path.display()))
    }

    pub fn read_from(path: &PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read manifest {}", path.display()))?;
        toml::from_str(&text)
            .with_context(|| format!("failed to parse manifest {}", path.display()))
    }
}
