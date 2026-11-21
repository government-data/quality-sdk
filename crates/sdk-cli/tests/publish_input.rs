//! The platform version file `publish` reads.

mod common;

use std::path::Path;

use common::dir;
use index_model::installation_hash;
use sdk_cli::publish::input::{parse, read};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn file(extra_module: &str) -> String {
    format!(
        r#"
version = "1.4.0"
upgrade_from = ">=1.3.0, <1.4.0"
notes = "What is new."

[rollout]
percent = 10
pilots = ["inst-0001"]

[modules.quality-data]
version = "1.4.0"
package = "qd.tar.gz"
requires = ["lake"]

[modules.quality-data.images]
api = "1.dkr.ecr.us-east-1.amazonaws.com/quality-data/quality-data/api@sha256:{DIGEST}"
{extra_module}
"#
    )
}

const LAKE: &str = r#"
[modules.lake-rustfs]
version = "0.9.0"
package = "packages/lake.tar.gz"
provides = ["lake"]

[modules.lake-rustfs.images]
rustfs = "1.dkr.ecr.us-east-1.amazonaws.com/quality-data/lake-rustfs/rustfs@sha256:0000000000000000000000000000000000000000000000000000000000000000"
"#;

fn with_packages(name: &str) -> std::path::PathBuf {
    let d = dir(name);
    std::fs::write(d.join("qd.tar.gz"), b"quality-data package").unwrap();
    std::fs::create_dir_all(d.join("packages")).unwrap();
    std::fs::write(d.join("packages/lake.tar.gz"), b"lake package").unwrap();
    d
}

#[test]
fn a_platform_version_is_read_with_its_packages_hashed_and_its_pilots_hidden() {
    let d = with_packages("input-ok");
    std::fs::write(d.join("1.4.0.toml"), file(LAKE)).unwrap();
    let r = read(&d.join("1.4.0.toml")).unwrap();
    let v = &r.version;
    assert_eq!(v.version.to_string(), "1.4.0");
    assert!(v.reversible && !v.urgent && !v.paused, "the defaults");
    assert_eq!(v.rollout.pilots, [installation_hash("inst-0001")]);
    assert_eq!(v.rollout.percent, 10);
    let qd = &v.modules["quality-data"];
    assert_eq!(qd.requires, ["lake"]);
    assert_eq!(qd.package.key, "packages/quality-data/1.4.0.tar.gz");
    assert_eq!(v.modules["lake-rustfs"].provides, ["lake"]);
    assert_eq!(
        v.modules["lake-rustfs"].package.key,
        "packages/lake-rustfs/0.9.0.tar.gz"
    );
    let lake = r
        .packages
        .iter()
        .find(|p| p.module == "lake-rustfs")
        .unwrap();
    assert_eq!(lake.bytes, b"lake package");
    assert_eq!(
        lake.sha256,
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(b"lake package"))
    );
    assert_eq!(lake.sha256, v.modules["lake-rustfs"].package.sha256);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn an_unknown_field_a_bad_pilot_a_missing_package_or_a_bad_name_is_refused() {
    let d = with_packages("input-refused");
    let unknown = file("").replace("notes =", "comment = \"x\"\nnotes =");
    assert!(parse(&unknown, &d).unwrap_err().contains("comment"));
    let pilot = file("").replace("\"inst-0001\"", "\"../x\"");
    assert!(parse(&pilot, &d).unwrap_err().contains("pilot"));
    let missing = file("").replace("qd.tar.gz", "none.tar.gz");
    assert!(parse(&missing, &d).unwrap_err().contains("none.tar.gz"));
    let name = file("").replace("[modules.quality-data", "[modules.\"../qd\"");
    assert!(parse(&name, &d).is_err());
    assert!(parse(&file(""), Path::new("/nonexistent")).is_err());
    std::fs::remove_dir_all(d).unwrap();
}
