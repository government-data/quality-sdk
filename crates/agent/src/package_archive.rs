//! A module's package as a file: a gzipped tar read and written in the
//! agent's own process (no `tar` command). Only regular files and
//! directories with plain relative paths are taken from a package; a link,
//! a device, an absolute path or a `..` refuses the whole package.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use tar::{Archive, Builder, EntryType, Header};

/// A package's member as a plain relative path, or why not.
fn member_path(path: &Path) -> Result<Option<PathBuf>, String> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(p) => out.push(p),
            _ => {
                return Err(format!(
                    "the package has a path it may not have: {}",
                    path.display()
                ))
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

fn open(package: &Path) -> Result<Archive<GzDecoder<File>>, String> {
    let f = File::open(package).map_err(|e| format!("{}: {e}", package.display()))?;
    Ok(Archive::new(GzDecoder::new(f)))
}

/// Every regular file of a package, checked; or why it is refused.
pub fn listing(package: &Path) -> Result<Vec<String>, String> {
    let mut files = BTreeSet::new();
    let fail = |e: std::io::Error| format!("{}: {e}", package.display());
    for entry in open(package)?.entries().map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let path = entry.path().map_err(fail)?.into_owned();
        let Some(rel) = member_path(&path)? else {
            continue;
        };
        match entry.header().entry_type() {
            EntryType::Regular => {
                files.insert(rel.to_string_lossy().into_owned());
            }
            EntryType::Directory => {}
            other => {
                return Err(format!(
                    "the package has {} that is not a file or a directory ({other:?})",
                    path.display()
                ))
            }
        }
    }
    if files.is_empty() {
        return Err(format!("{} holds no file", package.display()));
    }
    Ok(files.into_iter().collect())
}

/// Extract `package` (already listed, so checked) into the new `dir`.
pub fn extract(package: &Path, dir: &Path) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", package.display());
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in open(package)?.entries().map_err(fail)? {
        let mut entry = entry.map_err(fail)?;
        let path = entry.path().map_err(fail)?.into_owned();
        let Some(rel) = member_path(&path)? else {
            continue;
        };
        let to = dir.join(&rel);
        if entry.header().entry_type() == EntryType::Directory {
            std::fs::create_dir_all(&to).map_err(|e| format!("{}: {e}", to.display()))?;
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        // Executable or not, readable by all, writable by the owner only.
        let mode = if entry.header().mode().map_err(fail)? & 0o111 != 0 {
            0o755
        } else {
            0o644
        };
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(fail)?;
        write_file(&to, &bytes, mode)?;
    }
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)?;
        // Whatever the umask: the mode is the package's, not the agent's.
        f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(mode))?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    write().map_err(|e| format!("{}: {e}", path.display()))
}

/// Every regular file under `dir`, relative to it.
pub fn files_under(dir: &Path) -> Result<Vec<String>, String> {
    fn walk(root: &Path, rel: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(root.join(rel))? {
            let entry = entry?;
            let rel = rel.join(entry.file_name());
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(root, &rel, out)?;
            } else if kind.is_file() {
                out.push(rel.to_string_lossy().into_owned());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out).map_err(|e| format!("{}: {e}", dir.display()))?;
    out.sort();
    Ok(out)
}

/// Write `dir`'s regular files as a package at `to`: to a temporary file
/// written to disk, then renamed. Returns the files.
pub fn pack(dir: &Path, to: &Path) -> Result<Vec<String>, String> {
    let files = files_under(dir)?;
    let partial = to.with_extension("gz.partial");
    let fail = |e: std::io::Error| format!("{}: {e}", partial.display());
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    let _ = std::fs::remove_file(&partial);
    let mut tar = Builder::new(GzEncoder::new(
        File::create(&partial).map_err(fail)?,
        flate2::Compression::default(),
    ));
    for rel in &files {
        let mut header = Header::new_gnu();
        let bytes = std::fs::read(dir.join(rel)).map_err(fail)?;
        let meta = std::fs::metadata(dir.join(rel)).map_err(fail)?;
        header.set_size(bytes.len() as u64);
        header.set_mode(std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777);
        header.set_cksum();
        tar.append_data(&mut header, rel, bytes.as_slice())
            .map_err(fail)?;
    }
    let file = tar.into_inner().map_err(fail)?.finish().map_err(fail)?;
    file.sync_all().map_err(fail)?;
    std::fs::rename(&partial, to).map_err(fail)?;
    Ok(files)
}

#[cfg(test)]
#[path = "package_archive_tests.rs"]
mod tests;
