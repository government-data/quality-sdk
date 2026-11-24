//! A module's route on the web entry point, how it is signed in to, and its
//! entry in the core's apps menu (decision 0002, the amendment to steps 3,
//! 7 and 9).
//!
//! ```yaml
//! route:
//!   path: /docs          # one segment, not one the core uses
//!   service: web         # a service of the module's compose
//!   port: 8080
//! auth: forward          # forward | oidc
//! menu:
//!   label: { en: User guide, es: Guía de usuario }
//!   path: { en: /docs/, es: /docs/es/ }      # default: route.path + "/"
//! ```

use serde::{Deserialize, Serialize};

use crate::ManifestError;

/// The path segments the core serves itself: no module may take them.
pub const RESERVED_ROUTE_SEGMENTS: &[&str] = &[
    "api",
    "auth",
    "login",
    "apps",
    "internal",
    "modules",
    "assets",
    ".well-known",
];

/// The longest menu label.
const MAX_LABEL: usize = 80;
/// The longest path.
const MAX_PATH: usize = 200;

/// Where the module is served to people: Traefik's `websecure` routes
/// `path` and everything under it to `service` on `port`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// `/` and one segment (`/docs`).
    pub path: String,
    pub service: String,
    pub port: u16,
}

/// How a person gets through the module's route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    /// The core checks the session before Traefik passes each request (a
    /// static site that does not sign in itself).
    Forward,
    /// The module signs in with the core's OIDC itself.
    Oidc,
}

/// A text in each language the web speaks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InLanguages {
    pub en: String,
    pub es: String,
}

/// The module's entry in the core's apps menu.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Menu {
    pub label: InLanguages,
    /// The page it opens, under the route; `route.path` + `/` without it.
    #[serde(default)]
    pub path: Option<InLanguages>,
    /// The action a person must hold to see it (decision 0009): it hides
    /// the link only, it is not access control.
    #[serde(default)]
    pub requires: Option<String>,
}

impl Menu {
    /// The page it opens in each language.
    pub fn paths(&self, route: &Route) -> InLanguages {
        self.path.clone().unwrap_or_else(|| InLanguages {
            en: format!("{}/", route.path),
            es: format!("{}/", route.path),
        })
    }
}

fn invalid(why: String) -> ManifestError {
    ManifestError::Invalid(why)
}

/// `route`, `auth` and `menu` together: `auth` and `menu` need `route`.
pub(crate) fn validate(
    route: Option<&Route>,
    auth: Option<Auth>,
    menu: Option<&Menu>,
) -> Result<(), ManifestError> {
    let Some(route) = route else {
        if auth.is_some() || menu.is_some() {
            return Err(invalid("auth and menu need a route".into()));
        }
        return Ok(());
    };
    validate_route(route)?;
    if let Some(menu) = menu {
        validate_menu(menu, route)?;
    }
    Ok(())
}

fn validate_route(route: &Route) -> Result<(), ManifestError> {
    let segment = route.path.strip_prefix('/').unwrap_or("");
    let ok = !segment.is_empty()
        && route.path.len() <= MAX_PATH
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        return Err(invalid(format!(
            "route.path {:?}: '/' and one segment of lowercase letters, digits or '-'",
            route.path
        )));
    }
    if RESERVED_ROUTE_SEGMENTS.contains(&segment) {
        return Err(invalid(format!(
            "route.path {:?} is the core's",
            route.path
        )));
    }
    crate::validate::check_service("route.service", &route.service)?;
    if route.port == 0 {
        return Err(invalid("route.port: 1 to 65535".into()));
    }
    Ok(())
}

fn validate_menu(menu: &Menu, route: &Route) -> Result<(), ManifestError> {
    for label in [&menu.label.en, &menu.label.es] {
        let l = label.trim();
        if l.is_empty() || l.chars().count() > MAX_LABEL || l.chars().any(char::is_control) {
            return Err(invalid(format!(
                "menu.label {label:?}: 1 to {MAX_LABEL} printable characters"
            )));
        }
    }
    let paths = menu.paths(route);
    for path in [&paths.en, &paths.es] {
        if !under(path, &route.path) {
            return Err(invalid(format!(
                "menu.path {path:?} is not under the route {:?}",
                route.path
            )));
        }
    }
    Ok(())
}

/// The route itself or a path under it, with no query, fragment, `..`, `.`
/// or empty segment (only the last may be empty: a trailing `/`).
pub(crate) fn under(path: &str, route: &str) -> bool {
    let rest = match path.strip_prefix(route) {
        Some("") => return true,
        Some(rest) => rest,
        None => return false,
    };
    let Some(rest) = rest.strip_prefix('/') else {
        return false;
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let last = segments.len() - 1;
    path.len() <= MAX_PATH
        && segments
            .iter()
            .enumerate()
            .all(|(i, s)| (i == last || !s.is_empty()) && *s != "." && *s != "..")
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'))
}
