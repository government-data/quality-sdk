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

/// Writes `archive` to a command's standard output file, answers each
/// command by the first of `answers` its line contains (`Ok("")`
/// otherwise), and keeps what ran.
#[derive(Default)]
struct Cp {
    archive: Vec<u8>,
    answers: RefCell<Vec<(&'static str, Result<String, String>)>>,
    ran: RefCell<Vec<Cmd>>,
}

impl Runner for Rc<Cp> {
    fn run(&self, cmd: &Cmd) -> Result<String, String> {
        self.ran.borrow_mut().push(cmd.clone());
        if let Some(out) = &cmd.stdout {
            std::fs::write(out, &self.archive).unwrap();
        }
        let line = cmd.line();
        let answers = self.answers.borrow();
        let rule = answers.iter().find(|(text, _)| line.contains(text));
        rule.map_or(Ok(String::new()), |(_, answer)| answer.clone())
    }
}

/// What `docker inspect` says of the stopped `api` container.
const INSPECT: &str = r#"[{"Id":"c0ffee","Image":"sha256:1ma6e",
    "Mounts":[{"Type":"volume","Destination":"/var/lib/dolt"}]}]"#;

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
        answers: RefCell::new(vec![
            (" ps --all --quiet api", Ok("c0ffee\n".into())),
            ("docker inspect c0ffee", Ok(INSPECT.into())),
        ]),
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
    let copy = ran
        .iter()
        .position(|c| c.line().contains(" cp - "))
        .unwrap();
    let line = ran[copy].line();
    assert!(line.ends_with(".env cp - api:/var/lib"), "{line}");
    assert_eq!(
        ran[copy].stdin.as_deref(),
        Some(
            dir.join("modules/quality-data/volumes/catalog.tar")
                .as_path()
        )
    );
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn a_volume_is_emptied_through_its_container_s_mounts_before_its_archive_goes_in() {
    let r = root("empty");
    let dir = backup_dir(&r);
    let (data, cp) = installed(&r, archive());
    data.back_up_into(&dir, &["quality".into()]).unwrap();
    cp.ran.borrow_mut().clear();
    data.restore(&dir).unwrap();
    let lines: Vec<String> = cp.ran.borrow().iter().map(Cmd::line).collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(
        lines[0].ends_with(".env ps --all --quiet api"),
        "{}",
        lines[0]
    );
    assert_eq!(lines[1], "docker inspect c0ffee");
    assert_eq!(
        lines[2],
        "docker run --rm --network none --user 0:0 --volumes-from c0ffee \
         --entrypoint find sha256:1ma6e /var/lib/dolt -mindepth 1 -delete"
    );
    assert!(lines[3].ends_with(".env cp - api:/var/lib"), "{}", lines[3]);
    std::fs::remove_dir_all(r).unwrap();
}

#[test]
fn a_volume_that_cannot_be_emptied_is_not_restored_over() {
    for (rule, answer, says) in [
        (
            " ps --all --quiet api",
            Ok(String::new()),
            "api has no container",
        ),
        (
            "docker inspect c0ffee",
            Ok(r#"[{"Image":"sha256:1ma6e","Mounts":[]}]"#.to_string()),
            "/var/lib/dolt is not a volume of api's container",
        ),
        (
            "docker inspect c0ffee",
            Ok("[]".to_string()),
            "names no image",
        ),
        (
            "--entrypoint find",
            Err("find: not found".to_string()),
            "emptying it: find",
        ),
    ] {
        let r = root("not-emptied");
        let dir = backup_dir(&r);
        let (data, cp) = installed(&r, archive());
        data.back_up_into(&dir, &["quality".into()]).unwrap();
        cp.answers.borrow_mut().insert(0, (rule, answer));
        cp.ran.borrow_mut().clear();
        let err = data.restore(&dir).unwrap_err();
        assert!(
            err.contains("restoring quality-data /var/lib/dolt"),
            "{err}"
        );
        assert!(err.contains(says), "{err}");
        let copied = cp.ran.borrow().iter().any(|c| c.line().contains(" cp - "));
        assert!(!copied, "{says}: nothing is copied in");
        std::fs::remove_dir_all(r).unwrap();
    }
}
