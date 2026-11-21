//! The modules' data with a fake runner that writes the archive `docker
//! compose cp` would: nothing runs `docker`; what `cp` does was verified by
//! `tools/verify/module-data.sh`.

use std::cell::RefCell;
use std::os::unix::fs::PermissionsExt;
use std::rc::Rc;

use super::*;
use crate::command::Cmd;
use crate::paths::Paths;

fn root(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-data-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Writes `archive` to a command's standard output file, and keeps what
/// ran.
#[derive(Default)]
struct Cp {
    archive: Vec<u8>,
    ran: RefCell<Vec<Cmd>>,
}

impl Runner for Rc<Cp> {
    fn run(&self, cmd: &Cmd) -> Result<String, String> {
        self.ran.borrow_mut().push(cmd.clone());
        if let Some(out) = &cmd.stdout {
            std::fs::write(out, &self.archive).unwrap();
        }
        Ok(String::new())
    }
}

/// A tar of `dolt/` with one file, as `cp api:/var/lib/dolt -` gives it.
fn archive() -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(4);
    h.set_mode(0o644);
    h.set_cksum();
    b.append_data(&mut h, "dolt/catalog", &b"rows"[..]).unwrap();
    b.into_inner().unwrap()
}

const MANIFEST: &str = "name: quality-data\nversion: 1.3.0\ndata:\n  databases:\n    \
    - { name: quality }\n    - { name: scratch, critical: false }\n  volumes:\n    \
    - { name: catalog, service: api, path: /var/lib/dolt }\n  keys:\n    \
    - { path: secrets/a.key }\n    - { path: tmp/cache, critical: false }\n";

/// quality-data installed with `MANIFEST` and its key; the adapter with a
/// runner that writes `archive`.
fn installed(r: &Path, archive: Vec<u8>) -> (ModuleData, Rc<Cp>) {
    let module = r.join("opt/quality-data/quality-data");
    std::fs::create_dir_all(module.join("current")).unwrap();
    std::fs::create_dir_all(module.join("secrets")).unwrap();
    std::fs::write(module.join("current/module.yaml"), MANIFEST).unwrap();
    std::fs::write(module.join("secrets/a.key"), "key bytes").unwrap();
    let record = r.join("var/lib/quality-data/updater/installed.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    let json = r#"{"version":"1.3.0","modules":{},"order":["quality-data"]}"#;
    std::fs::write(&record, json).unwrap();
    let cp = Rc::new(Cp {
        archive,
        ..Cp::default()
    });
    let mut data = ModuleData::new(&Paths::under(r));
    data.runner = Box::new(cp.clone());
    (data, cp)
}

fn backup_dir(r: &Path) -> PathBuf {
    let dir = r.join("var/lib/quality-data/backups/20261121T120000000Z");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn without_a_record_or_a_manifest_there_is_nothing_to_back_up() {
    let r = root("none");
    let dir = backup_dir(&r);
    let data = ModuleData::new(&Paths::under(&r));
    assert_eq!(data.back_up_into(&dir, &[]).unwrap(), 0);
    assert_eq!(std::fs::read_to_string(dir.join(LIST)).unwrap(), "[]");
    data.restore(&r).unwrap();
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn critical_volumes_and_keys_are_backed_up_and_checked() {
    let r = root("back-up");
    let dir = backup_dir(&r);
    let (data, cp) = installed(&r, archive());
    assert_eq!(data.back_up_into(&dir, &["quality".into()]).unwrap(), 2);
    let ran = cp.ran.borrow();
    assert_eq!(ran.len(), 1);
    let line = ran[0].line();
    assert!(line.contains("--project-name quality-data"), "{line}");
    assert!(line.ends_with(".env cp api:/var/lib/dolt -"), "{line}");
    let tar = dir.join("modules/quality-data/volumes/catalog.tar");
    assert_eq!(ran[0].stdout.as_deref(), Some(tar.as_path()));
    let key = dir.join("modules/quality-data/keys/secrets/a.key");
    assert_eq!(std::fs::read_to_string(&key).unwrap(), "key bytes");
    assert_eq!(
        std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(!dir.join("modules/quality-data/keys/tmp").exists());
    let list: Vec<Piece> = serde_json::from_slice(&std::fs::read(dir.join(LIST)).unwrap()).unwrap();
    assert_eq!(
        list[1],
        Piece::Key {
            module: "quality-data".into(),
            path: "secrets/a.key".into(),
            file: "modules/quality-data/keys/secrets/a.key".into(),
        }
    );
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn a_piece_that_cannot_be_backed_up_whole_is_no_backup() {
    let r = root("bad");
    let dir = backup_dir(&r);
    let (data, _) = installed(&r, archive());
    let err = data.back_up_into(&dir, &["other".into()]).unwrap_err();
    assert_eq!(
        err,
        "the database quality of quality-data is not on the server"
    );
    let whole = archive();
    let (data, _) = installed(&r, whole[..1536].to_vec());
    let err = data.back_up_into(&dir, &["quality".into()]).unwrap_err();
    assert!(err.contains("end-of-archive marker"), "{err}");
    let (data, _) = installed(&r, vec![0; 1024]);
    let err = data.back_up_into(&dir, &["quality".into()]).unwrap_err();
    assert!(err.contains("holds nothing"), "{err}");
    let (data, _) = installed(&r, archive());
    std::fs::remove_file(r.join("opt/quality-data/quality-data/secrets/a.key")).unwrap();
    let err = data.back_up_into(&dir, &["quality".into()]).unwrap_err();
    assert!(err.contains("secrets/a.key"), "{err}");
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn a_restore_puts_back_each_key_and_copies_each_volume_s_archive_in() {
    let r = root("restore");
    let dir = backup_dir(&r);
    let (data, cp) = installed(&r, archive());
    data.back_up_into(&dir, &["quality".into()]).unwrap();
    let key = r.join("opt/quality-data/quality-data/secrets/a.key");
    std::fs::write(&key, "changed").unwrap();
    data.restore(&dir).unwrap();
    assert_eq!(std::fs::read_to_string(&key).unwrap(), "key bytes");
    let ran = cp.ran.borrow();
    let line = ran[1].line();
    assert!(line.ends_with(".env cp - api:/var/lib"), "{line}");
    assert_eq!(
        ran[1].stdin.as_deref(),
        Some(
            dir.join("modules/quality-data/volumes/catalog.tar")
                .as_path()
        )
    );
    std::fs::remove_dir_all(r).unwrap();
}
