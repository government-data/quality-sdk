//! The files a machine role gets (decision 0013, ruling 6): the module's
//! `compose.yml` and every package file the role's services bind mount,
//! each by its path in the package, its SHA-256 and its bytes in base64.
//! A source outside the package (absolute, `~`, a `${…}` variable such as
//! the secrets' folder) is not a file of the role: a secret never travels.

use std::collections::BTreeMap;

use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The most bytes of distinct files a module's roles carry (256 KiB).
pub const MAX_MODULE_BYTES: usize = 256 * 1024;

/// A file as the core is told it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleFile {
    pub path: String,
    pub sha256: String,
    pub content_base64: String,
    /// Its bytes' length (not sent).
    #[serde(skip)]
    pub size: usize,
}

impl RoleFile {
    pub fn new(path: &str, bytes: &[u8]) -> Self {
        Self {
            path: path.into(),
            sha256: hex::encode(Sha256::digest(bytes)),
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            size: bytes.len(),
        }
    }
}

/// The package a module was put from.
pub trait Package {
    /// The regular file at `path` (relative to the package), or every
    /// regular file under the directory at `path`, by their paths in the
    /// package.
    fn files_under(&self, path: &str) -> Result<Vec<(String, Vec<u8>)>, String>;
}

/// The package files one service's `volumes` bind mount: a source
/// relative to the package (`./x`, `.`, `../x` refused), in the short
/// syntax (`SRC:DST[:MODE]`) or the long one (`type: bind`, `source`).
pub fn mounted(volumes: &[serde_json::Value]) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for v in volumes {
        let source = match v {
            serde_json::Value::String(short) => match short.split_once(':') {
                Some((source, _)) => source,
                None => continue,
            },
            serde_json::Value::Object(long)
                if long.get("type").and_then(|t| t.as_str()) == Some("bind") =>
            {
                match long.get("source").and_then(|s| s.as_str()) {
                    Some(source) => source,
                    None => continue,
                }
            }
            _ => continue,
        };
        if let Some(path) = in_package(source)? {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// `source` as a path in the package: `None` when it is not one (a named
/// volume, an absolute or `~` path, a variable); an error when it leaves
/// the package.
fn in_package(source: &str) -> Result<Option<String>, String> {
    if source != "." && !source.starts_with("./") && !source.starts_with("../") {
        return Ok(None);
    }
    let mut segments = Vec::new();
    for seg in source.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(format!("the mount {source:?} leaves the package")),
            s => segments.push(s),
        }
    }
    Ok(Some(segments.join("/")))
}

/// `compose.yml` and the files at `paths`, each once, sorted.
pub fn collect(
    compose: &str,
    paths: &[String],
    package: &dyn Package,
) -> Result<Vec<RoleFile>, String> {
    let mut files = BTreeMap::new();
    files.insert("compose.yml".to_string(), compose.as_bytes().to_vec());
    for path in paths {
        for (path, bytes) in package.files_under(path)? {
            files.insert(path, bytes);
        }
    }
    Ok(files
        .iter()
        .map(|(path, bytes)| RoleFile::new(path, bytes))
        .collect())
}

/// A package on disk: the module's `current/`.
#[derive(Debug, Clone)]
pub struct Dir(pub std::path::PathBuf);

impl Package for Dir {
    fn files_under(&self, path: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
        let mut found = Vec::new();
        walk(&self.0, path, &mut found)?;
        Ok(found)
    }
}

/// The regular file at `rel` under `root`, or every one under it; a link
/// is refused (it could point out of the package).
fn walk(
    root: &std::path::Path,
    rel: &str,
    found: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let full = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let what = |e: std::io::Error| format!("{}: {e}", full.display());
    let meta = std::fs::symlink_metadata(&full).map_err(what)?;
    if meta.is_file() {
        found.push((rel.to_string(), std::fs::read(&full).map_err(what)?));
        return Ok(());
    }
    if !meta.is_dir() {
        return Err(format!(
            "{}: not a regular file or a directory",
            full.display()
        ));
    }
    let mut names: Vec<String> = std::fs::read_dir(&full)
        .map_err(what)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()
        .map_err(what)?;
    names.sort();
    for name in names {
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        walk(root, &child, found)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "role_files_tests.rs"]
mod tests;
