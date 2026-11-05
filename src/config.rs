use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub source: SourceConfig,
    pub destination: DestinationConfig,
    #[serde(default)]
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
    /// 省略時は分割しない。
    pub max_part_size_mb: Option<u64>,
    #[serde(default = "default_compression_level")]
    pub compression_level: i32,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            max_part_size_mb: None,
            compression_level: default_compression_level(),
        }
    }
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
    #[serde(default)]
    pub skip_unreadable_files: bool,
    #[serde(default = "default_true")]
    pub check_for_updates: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            verify_after_copy: true,
            delete_local_archive_after_copy: true,
            skip_unreadable_files: false,
            check_for_updates: true,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct WorkConfig {
    pub path: Option<PathBuf>,
}

impl WorkConfig {
    fn dir(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("auto_backup"))
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct LogConfig {
    pub path: Option<PathBuf>,
}

impl LogConfig {
    fn file(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(|| PathBuf::from("auto_backup.log"))
    }
}

/// 復元で使う設定。`[work]`・`[log]` だけを読み、他のセクションは無視する。
#[derive(Debug, Default, Deserialize)]
pub struct RestoreConfig {
    #[serde(default)]
    pub work: WorkConfig,
    #[serde(default)]
    pub log: LogConfig,
}

impl RestoreConfig {
    /// `path` 省略時はすべて既定値で動く。
    pub fn load(path: Option<&PathBuf>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("failed to parse config {}", path.display()))
    }

    pub fn work_dir(&self) -> PathBuf {
        self.work.dir()
    }

    pub fn log_path(&self) -> PathBuf {
        self.log.file()
    }
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
            self.archive.max_part_size_mb != Some(0),
            "archive.max_part_size_mb must be greater than 0"
        );
        Ok(())
    }

    /// 分割サイズ。`max_part_size_mb` 省略時は上限なし(分割しない)。
    pub fn max_part_size_bytes(&self) -> u64 {
        self.archive
            .max_part_size_mb
            .map_or(u64::MAX, |mb| mb.saturating_mul(1024 * 1024))
    }

    pub fn work_dir(&self) -> PathBuf {
        self.work.dir()
    }

    pub fn log_path(&self) -> PathBuf {
        self.log.file()
    }
}

fn default_true() -> bool {
    true
}

fn default_compression_level() -> i32 {
    6
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    const MINIMAL: &str = r#"
[source]
paths = ["C:/data"]

[destination]
path = "D:/backups"
"#;

    #[test]
    fn archive_section_can_be_omitted() {
        let config = parse(MINIMAL).expect("minimal config should parse");
        assert_eq!(config.archive.compression_level, 6);
        assert!(config.behavior.check_for_updates);
        // 省略時は分割しない
        assert_eq!(config.max_part_size_bytes(), u64::MAX);
    }

    #[test]
    fn max_part_size_is_converted_to_bytes() {
        let config = parse(&format!(
            "{MINIMAL}
[archive]
max_part_size_mb = 500
"
        ))
        .expect("config should parse");
        assert_eq!(config.max_part_size_bytes(), 500 * 1024 * 1024);
    }

    #[test]
    fn zero_max_part_size_is_rejected() {
        assert!(parse(&format!(
            "{MINIMAL}
[archive]
max_part_size_mb = 0
"
        ))
        .is_err());
    }

    #[test]
    fn restore_config_defaults_without_file() {
        let config = RestoreConfig::load(None).expect("defaults");
        assert_eq!(config.work_dir(), std::env::temp_dir().join("auto_backup"));
        assert_eq!(config.log_path(), PathBuf::from("auto_backup.log"));
    }

    #[test]
    fn restore_config_reads_only_work_and_log() {
        let config: RestoreConfig = toml::from_str(
            r#"
[work]
path = "E:/work"

[log]
path = "E:/restore.log"
"#,
        )
        .expect("source/destination should not be required");
        assert_eq!(config.work_dir(), PathBuf::from("E:/work"));
        assert_eq!(config.log_path(), PathBuf::from("E:/restore.log"));

        // バックアップ用の config.toml をそのまま渡しても読める
        let config: RestoreConfig = toml::from_str(MINIMAL).expect("backup config should parse");
        assert_eq!(config.log_path(), PathBuf::from("auto_backup.log"));
    }

    #[test]
    fn update_check_can_be_disabled() {
        let config = parse(&format!(
            "{MINIMAL}
[behavior]
check_for_updates = false
"
        ))
        .expect("config should parse");
        assert!(!config.behavior.check_for_updates);
    }
}
