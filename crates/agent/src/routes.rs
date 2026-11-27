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
//!
//! A module that declares a drain or a removal check, or provides a
//! capability, also gets a router on the `internal` entry point (ruling
//! 95): `Host(<INTERNAL_DOMAIN>) && PathPrefix(/modules/<module>/)`, with
//! the core's `machines` TLS options (a client certificate of the CA
//! required), the prefix stripped and the verified certificate passed in
//! `X-Forwarded-Tls-Client-Cert` (`passTLSClientCert`), to the same
//! service as its route.

use std::path::Path;

use module_manifest::{Auth, Manifest, Route};

/// A module's route file's name.
pub fn file_name(module: &str) -> String {
    format!("module-{module}.yml")
}

/// Whether `m` is called by the core or by other machines on the internal
/// entry point: a drain, a removal check or a capability it provides.
pub fn called_inside(m: &Manifest) -> bool {
    m.drain.is_some() || m.removal.is_some() || !m.provides.capabilities.is_empty()
}

/// The route file of `module` serving `route`, with its internal router
/// when `inside`.
pub fn render(module: &str, route: &Route, auth: Option<Auth>, inside: bool) -> String {
    let name = format!("module-{module}");
    let path = &route.path;
    let middlewares = match auth {
        Some(Auth::Forward) => "[module-chain@file, module-forward-auth@file]",
        Some(Auth::Oidc) | None => "[module-chain@file]",
    };
    let server = format!("http://{module}-{}:{}", route.service, route.port);
    let (internal_router, internal_middlewares) = if inside {
        internal_parts(module, &name)
    } else {
        (String::new(), String::new())
    };
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
{internal_router}  services:
    {name}:
      loadBalancer:
        servers:
          - url: "{server}"
{internal_middlewares}"#
    )
}

/// The internal router and its middlewares (ruling 95).
fn internal_parts(module: &str, name: &str) -> (String, String) {
    let router = format!(
        r#"    {name}-internal:
      rule: "Host(`{{{{ env "INTERNAL_DOMAIN" | default "internal.localhost" }}}}`) && PathPrefix(`/modules/{module}/`)"
      entryPoints: [internal]
      tls:
        options: machines
      service: {name}
      middlewares: [{name}-internal-strip, {name}-internal-certificate]
"#
    );
    let middlewares = format!(
        r#"  middlewares:
    {name}-internal-strip:
      stripPrefix:
        prefixes: ["/modules/{module}"]
    {name}-internal-certificate:
      passTLSClientCert:
        pem: true
"#
    );
    (router, middlewares)
}

/// Write the route of every module of `installed` whose manifest has one
/// into `dir`, and remove every other module's route file there.
pub fn sync(dir: &Path, installed: &[(String, Option<Manifest>)]) -> Result<(), String> {
    let mut keep = Vec::new();
    for (module, manifest) in installed {
        let Some(m) = manifest else { continue };
        let Some(route) = &m.route else { continue };
        write(
            dir,
            module,
            &render(module, route, m.auth, called_inside(m)),
        )?;
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
