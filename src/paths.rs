use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

pub fn unique_source_names(paths: &[PathBuf]) -> Vec<String> {
    let components: Vec<Vec<String>> = paths
        .iter()
        .map(|path| reversed_components(path.as_path()))
        .collect();
    let mut depths = vec![1usize; paths.len()];

    loop {
        let names: Vec<String> = components
            .iter()
            .zip(depths.iter())
            .map(|(parts, depth)| {
                sanitize(
                    &parts
                        .iter()
                        .take(*depth)
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("_"),
                )
            })
            .collect();

        let mut counts = HashMap::<String, usize>::new();
        for name in &names {
            *counts.entry(name.clone()).or_default() += 1;
        }

        let mut changed = false;
        for (index, name) in names.iter().enumerate() {
            if counts[name] > 1 && depths[index] < components[index].len() {
                depths[index] += 1;
                changed = true;
            }
        }

        if !changed {
            return names
                .into_iter()
                .enumerate()
                .map(|(index, name)| {
                    if counts[&name] == 1 {
                        name
                    } else {
                        format!("{}_{}", name, short_hash(&paths[index]))
                    }
                })
                .collect();
        }
    }
}

/// Windows のパス比較用のキー。大文字小文字と区切り文字(`/` と `\`)の違いを無視する。
pub fn path_key(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// `path` が `base` の中(`base` 自身は含まない)にあれば、`base` からの相対パスを返す。
pub fn relative_to(base: &Path, path: &Path) -> Option<PathBuf> {
    let base_key = path_key(base);
    let key = path_key(path);
    if key.len() <= base_key.len() || key[..base_key.len()] != base_key[..] {
        return None;
    }
    Some(path.components().skip(base_key.len()).collect())
}

pub fn sanitize(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "root".to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

fn reversed_components(path: &Path) -> Vec<String> {
    let mut parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            Component::Prefix(value) => Some(value.as_os_str().to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    parts.reverse();
    if parts.is_empty() {
        parts.push("root".to_string());
    }
    parts
}

fn short_hash(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_to_ignores_case_and_separators() {
        assert_eq!(
            relative_to(
                Path::new("C:/Dev/Project"),
                Path::new(r"c:\dev\project\Target\debug")
            ),
            Some(PathBuf::from("Target/debug"))
        );
        assert_eq!(
            relative_to(Path::new("C:/dev/project"), Path::new("C:/dev/project2/x")),
            None
        );
        assert_eq!(
            relative_to(Path::new("C:/dev/project"), Path::new("C:/dev/project")),
            None
        );
    }
}
