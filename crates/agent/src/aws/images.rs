//! Pulling a platform version's images by digest (decision 0003 gives
//! `pull_images` to the AWS adapter). Every image must be in the
//! installation's ECR registry, pinned by digest: third-party images are
//! mirrored there (decision 0002), so none is pulled from anywhere else.
//! The pull itself goes through a port; the real one runs `docker` with the
//! registry's temporary credential in a configuration removed afterwards.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use updater::engine::Offer;

use super::ecr::RegistryAuth;

/// Pulls images, logged in to their registry.
pub trait ImagePuller {
    fn pull(&self, auth: &RegistryAuth, images: &[String]) -> Result<(), String>;
}

/// Every image of `offer`, once, after checking each is in the offer's
/// registry and pinned by digest.
pub fn pull_all(offer: &Offer, puller: &dyn ImagePuller) -> Result<(), String> {
    let (host, user, password) = offer
        .registry
        .clone()
        .ok_or("no registry credential came with the offer")?;
    let auth = RegistryAuth {
        host,
        user,
        password,
    };
    let mut images = BTreeSet::new();
    for (module, m) in &offer.modules {
        for (name, image) in &m.images {
            let pinned = image
                .split_once("@sha256:")
                .is_some_and(|(_, d)| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()));
            let ours = image
                .strip_prefix(&auth.host)
                .is_some_and(|rest| rest.starts_with('/'));
            if !pinned || !ours {
                return Err(format!(
                    "{module}/{name}: {image} is not pinned in the installation's registry"
                ));
            }
            images.insert(image.clone());
        }
    }
    let images: Vec<String> = images.into_iter().collect();
    puller.pull(&auth, &images)
}

/// `docker login` to the registry in a configuration of its own, then
/// `docker pull` of each image by digest; the configuration, which holds
/// the credential, is removed whatever happens.
#[derive(Debug, Clone)]
pub struct Docker {
    pub docker: PathBuf,
    /// Where the temporary configuration is made.
    pub scratch: PathBuf,
}

impl Default for Docker {
    fn default() -> Self {
        Self {
            docker: "docker".into(),
            scratch: std::env::temp_dir(),
        }
    }
}

/// A directory removed when dropped.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ImagePuller for Docker {
    fn pull(&self, auth: &RegistryAuth, images: &[String]) -> Result<(), String> {
        let dir = Scratch(make_private_dir(&self.scratch)?);
        let mut login = Command::new(&self.docker)
            .arg("--config")
            .arg(&dir.0)
            .args([
                "login",
                "--username",
                &auth.user,
                "--password-stdin",
                &auth.host,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run {}: {e}", self.docker.display()))?;
        if let Some(mut stdin) = login.stdin.take() {
            stdin
                .write_all(auth.password.as_bytes())
                .map_err(|e| format!("docker login: {e}"))?;
        }
        let out = login
            .wait_with_output()
            .map_err(|e| format!("docker login: {e}"))?;
        if !out.status.success() {
            return Err(format!("docker login {}: {}", auth.host, said(&out.stderr)));
        }
        for image in images {
            let out = Command::new(&self.docker)
                .arg("--config")
                .arg(&dir.0)
                .args(["pull", "--quiet", image])
                .output()
                .map_err(|e| format!("could not run {}: {e}", self.docker.display()))?;
            if !out.status.success() {
                return Err(format!("docker pull {image}: {}", said(&out.stderr)));
            }
        }
        Ok(())
    }
}

fn said(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .trim()
        .chars()
        .take(300)
        .collect()
}

/// A new directory only its owner may read.
fn make_private_dir(under: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let dir = under.join(format!("quality-data-pull-{}-{nanos}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
#[path = "images_tests.rs"]
mod tests;
