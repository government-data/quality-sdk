//! Each installed module's manifest (decision 0002): `module.yaml` at the
//! root of its package, read where the package was put,
//! `/opt/quality-data/<module>/current/module.yaml`. The package is pinned
//! by its SHA-256 in the signed index, so its manifest is as trusted as the
//! index. A package without one (published before manifests) has none: its
//! health is the Compose floor and its data the databases alone.

use std::io::Read;
use std::path::Path;

use module_manifest::Manifest;

/// The manifest's name in a package.
pub const FILE: &str = "module.yaml";
/// The largest manifest read.
const MAX_BYTES: u64 = 1 << 20;

/// The manifest of `module` installed under `install`, if its package has
/// one; it must name that module.
pub fn read(install: &Path, module: &str) -> Result<Option<Manifest>, String> {
    let path = install.join(module).join("current").join(FILE);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut text = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() as u64 > MAX_BYTES {
        return Err(format!("{} is over {MAX_BYTES} bytes", path.display()));
    }
    let manifest = Manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if manifest.name != module {
        return Err(format!(
            "{} names the module {:?}, not {module:?}",
            path.display(),
            manifest.name
        ));
    }
    Ok(Some(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_s_manifest_is_read_if_it_has_one_and_names_its_module() {
        let r = std::env::temp_dir().join(format!("agent-manifests-{}", std::process::id()));
        let current = r.join("docs/current");
        std::fs::create_dir_all(&current).unwrap();
        assert_eq!(read(&r, "docs").unwrap(), None);
        std::fs::write(current.join(FILE), "name: docs\nversion: 1.0.0\n").unwrap();
        assert_eq!(read(&r, "docs").unwrap().unwrap().name, "docs");
        std::fs::write(current.join(FILE), "name: other\nversion: 1.0.0\n").unwrap();
        assert!(read(&r, "docs").unwrap_err().contains("not \"docs\""));
        std::fs::write(current.join(FILE), "name: docs\n").unwrap();
        assert!(read(&r, "docs").unwrap_err().contains("malformed"));
        std::fs::remove_dir_all(r).unwrap();
    }
}
