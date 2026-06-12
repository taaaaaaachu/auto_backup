use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub source: SourceConfig,
    pub destination: DestinationConfig,
    pub archive: ArchiveConfig,
    #[serde(default)]
    pub exclude: ExcludeConfig,
    #[serde(default)]
    pub behavior: BehaviorConfig,
    #[serde(default)]
    pub work: WorkConfig,
    #[serde(default)]
    pub log: LogConfig,
}

#[derive(Debug, Deserialize)]
pub struct SourceConfig {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct DestinationConfig {
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct ArchiveConfig {
    pub max_part_size_mb: u64,
    #[serde(default = "default_compression_level")]
    pub compression_level: i32,
}

#[derive(Debug, Default, Deserialize)]
pub struct ExcludeConfig {
    #[serde(default)]
    pub patterns: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct BehaviorConfig {
    #[serde(default = "default_true")]
    pub verify_after_copy: bool,
    #[serde(default = "default_true")]
    pub delete_local_archive_after_copy: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            verify_after_copy: true,
            delete_local_archive_after_copy: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WorkConfig {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
pub struct LogConfig {
    pub path: Option<PathBuf>,
}

impl Config {
    pub fn load(path: &PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("failed to parse config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.source.paths.is_empty(),
            "source.paths must not be empty"
        );
        anyhow::ensure!(
            self.archive.max_part_size_mb > 0,
            "archive.max_part_size_mb must be greater than 0"
        );
        Ok(())
    }

    pub fn max_part_size_bytes(&self) -> u64 {
        self.archive.max_part_size_mb * 1024 * 1024
    }

    pub fn work_dir(&self) -> PathBuf {
        self.work
            .path
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("auto_backup"))
    }

    pub fn log_path(&self) -> PathBuf {
        self.log
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from("auto_backup.log"))
    }
}

fn default_true() -> bool {
    true
}

fn default_compression_level() -> i32 {
    6
}
