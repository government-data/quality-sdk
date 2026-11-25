//! `secrets.generate[].kind` (decision 0012 step 2): random bytes by
//! default, or a private key that `bytes` does not apply to; a written
//! `kind` needs the agent that reads it.

use module_manifest::{
    Manifest, ManifestError, SecretContent, SecretKind, SECRET_BYTES, SECRET_KIND_AGENT,
};

fn one(entry: &str) -> Result<Manifest, ManifestError> {
    Manifest::parse(&format!(
        "name: m\nversion: 1.0.0\nsecrets: {{ generate: [{entry}] }}\n"
    ))
}

fn refused(entry: &str, why: &str) {
    match one(entry) {
        Err(ManifestError::Invalid(w)) => assert!(w.contains(why), "{why:?} not in {w:?}"),
        other => panic!("expected Invalid with {why:?}, got {other:?}"),
    }
}

#[test]
fn left_out_the_kind_is_random_bytes() {
    let m = one("{ name: k }").unwrap();
    let g = &m.secrets.generate[0];
    assert_eq!(g.kind, None);
    assert_eq!(g.kind(), SecretKind::Random);
    assert_eq!(
        g.content(),
        SecretContent::Random {
            bytes: SECRET_BYTES
        }
    );
    let m = one("{ name: k, kind: random, bytes: 48 }").unwrap();
    assert_eq!(
        m.secrets.generate[0].content(),
        SecretContent::Random { bytes: 48 }
    );
}

#[test]
fn a_key_kind_is_a_key_and_takes_no_bytes() {
    let m = one("{ name: k, kind: ed25519 }").unwrap();
    assert_eq!(m.secrets.generate[0].content(), SecretContent::Ed25519);
    assert!(m.secrets.generate[0].critical);
    let m = one("{ name: k, kind: p256, critical: false }").unwrap();
    assert_eq!(m.secrets.generate[0].content(), SecretContent::P256);
    refused("{ name: k, kind: ed25519, bytes: 32 }", "kind random only");
    refused("{ name: k, kind: p256, bytes: 32 }", "kind random only");
    refused("{ name: k, kind: random, bytes: 8 }", "16 to 64");
}

#[test]
fn an_unknown_kind_is_malformed() {
    for kind in ["rsa", "Ed25519", "P256", "''"] {
        let entry = format!("{{ name: k, kind: {kind} }}");
        assert!(
            matches!(one(&entry), Err(ManifestError::Malformed(_))),
            "{kind}"
        );
    }
}

#[test]
fn a_written_kind_needs_the_agent_that_reads_it() {
    for kind in ["random", "ed25519", "p256"] {
        let m = one(&format!("{{ name: k, kind: {kind} }}")).unwrap();
        assert_eq!(
            m.agent_needed().unwrap().to_string(),
            SECRET_KIND_AGENT,
            "{kind}"
        );
    }
    let m = one("{ name: k }").unwrap();
    assert_eq!(
        m.agent_needed().unwrap().to_string(),
        module_manifest::MODULE_ADD_AGENT
    );
}
