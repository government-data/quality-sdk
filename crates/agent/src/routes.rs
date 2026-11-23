//! The installed modules' Traefik routes (decision 0002, the amendment to
//! steps 3, 7 and 9, point 2): one file per module with a `route` in its
//! manifest, `module-<module>.yml`, in the folder the core's Traefik
//! watches ([`crate::paths::Paths::routes_dir`]). A file put in a subfolder
//! after Traefik started is not read, so the files go in that folder
//! itself; each is written aside (`.tmp`, which Traefik does not read) and
//! renamed. [`sync`] writes the installed modules' and removes the files of
//! the modules that are no longer installed or have no route.
//!
//! The router names two middlewares of the core's package:
//! `module-chain@file` always, then `module-forward-auth@file` for
//! `auth: forward`. The host and the certificate resolver come from
//! Traefik's environment (`APP_DOMAIN`, `QD_TLS_MODE`), as the core's own
//! `apps.yml`.

use std::path::Path;

use module_manifest::{Auth, Manifest, Route};

/// A module's route file's name.
pub fn file_name(module: &str) -> String {
    format!("module-{module}.yml")
}

/// The route file of `module` serving `route`.
pub fn render(module: &str, route: &Route, auth: Option<Auth>) -> String {
    let name = format!("module-{module}");
    let path = &route.path;
    let middlewares = match auth {
        Some(Auth::Forward) => "[module-chain@file, module-forward-auth@file]",
        Some(Auth::Oidc) | None => "[module-chain@file]",
    };
    let server = format!("http://{module}-{}:{}", route.service, route.port);
    format!(
        r#"# Written by quality-data-agent from the module {module:?}'s manifest
# (module.yaml); rewritten after every start, removed with the module.
{{{{ $domain := env "APP_DOMAIN" | default "localhost" }}}}
{{{{ $resolver := "letsencrypt" }}}}{{{{ if eq (env "QD_TLS_MODE") "file" }}}}{{{{ $resolver = "" }}}}{{{{ end }}}}
http:
  routers:
    {name}:
      rule: "Host(`{{{{ $domain }}}}`) && (Path(`{path}`) || PathPrefix(`{path}/`))"
      entryPoints: [websecure]
      tls:
        certResolver: "{{{{ $resolver }}}}"
      priority: 50
      service: {name}
      middlewares: {middlewares}
  services:
    {name}:
      loadBalancer:
        servers:
          - url: "{server}"
"#
    )
}

/// Write the route of every module of `installed` whose manifest has one
/// into `dir`, and remove every other module's route file there.
pub fn sync(dir: &Path, installed: &[(String, Option<Manifest>)]) -> Result<(), String> {
    let mut keep = Vec::new();
    for (module, manifest) in installed {
        let Some(m) = manifest else { continue };
        let Some(route) = &m.route else { continue };
        write(dir, module, &render(module, route, m.auth))?;
        keep.push(file_name(module));
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // Nothing written and nowhere to look: no route to remove.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && keep.is_empty() => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("module-") && name.ends_with(".yml") && !keep.contains(&name) {
            std::fs::remove_file(entry.path())
                .map_err(|e| format!("{}: {e}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Remove `module`'s route file (none there: nothing to do).
pub fn remove(dir: &Path, module: &str) -> Result<(), String> {
    let path = dir.join(file_name(module));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write `text` as `module`'s route file, aside then renamed; unchanged,
/// it is left as it is (Traefik reloads on every write).
fn write(dir: &Path, module: &str, text: &str) -> Result<(), String> {
    let path = dir.join(file_name(module));
    if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
        return Ok(());
    }
    let aside = dir.join(format!(".{}.tmp", file_name(module)));
    std::fs::write(&aside, text)
        .and_then(|()| std::fs::rename(&aside, &path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
#[path = "routes_tests.rs"]
mod tests;
