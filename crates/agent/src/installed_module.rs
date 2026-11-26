//! What the agent tells quality-data of each installed module (`PUT
//! /updater/v1/modules`, decision 0003), and what the core understands of
//! it (`features`, decision 0009).

use std::collections::BTreeSet;

use module_manifest::{Manifest, Permissions};
use serde::Serialize;

/// What a core understands beyond the first `updater/v1` calls (decision
/// 0009, point 6); a core that names none has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Features(pub BTreeSet<String>);

impl Features {
    /// `PUT …/modules/{module}/clients` and `…/clients/disable`.
    pub const MODULE_CLIENTS: &'static str = "module-clients";
    /// `menu.requires` in `PUT /updater/v1/modules`, checked by the apps
    /// menu.
    pub const MENU_REQUIRES: &'static str = "menu-requires";
    /// `permissions` in `PUT /updater/v1/modules`.
    pub const MODULE_PERMISSIONS: &'static str = "module-permissions";
    /// `POST /updater/v1/audit`.
    pub const AGENT_AUDIT: &'static str = "agent-audit";
    /// `PUT /updater/v1/registry` (decision 0013).
    pub const REGISTRY_CREDENTIALS: &'static str = "registry-credentials";
    /// `events` in `PUT /updater/v1/modules` and the events routes
    /// (decision 0017).
    pub const MODULE_EVENTS: &'static str = "module-events";
    /// `PUT /updater/v1/modules/{module}/role-secrets` (decision 0013,
    /// ruling 44).
    pub const ROLE_SECRETS: &'static str = "role-secrets";
    /// `GET /updater/v1/removal/{module}` (decision 0015).
    pub const MODULE_REMOVAL: &'static str = "module-removal";

    pub fn has(&self, feature: &str) -> bool {
        self.0.contains(feature)
    }
}

/// An installed module as `PUT /updater/v1/modules` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledModule {
    pub name: String,
    pub version: String,
    /// Its manifest's `drain.path`, which the module serves on its route of
    /// the `internal` entry point (`null`: no work to drain).
    pub drain_path: Option<String>,
    /// Its manifest's `removal.path` (decision 0015), left out when none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removal_path: Option<String>,
    /// Its route's path on the web entry point (`null`: no route).
    pub route_path: Option<String>,
    /// Its entry in the core's apps menu (`null`: none).
    pub menu: Option<ModuleMenu>,
    /// Its actions and managed policies, to a core that has
    /// `module-permissions` only (absent otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Permissions>,
    /// The capabilities it provides, by name: how the core tells which
    /// module's data it replicates without naming the module (absent:
    /// none; an older core ignores it).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<String>,
    /// Its machine roles and the images each gets (decision 0013; absent:
    /// none; an older core ignores it). Set by the host, which reads the
    /// module's `compose.yml`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<crate::module_roles::ModuleRole>,
    /// The topics it provides, with their schemas, and consumes (decision
    /// 0017; absent: none; a core without `module-events` ignores it). Set
    /// by the host, which reads the schemas from the package.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<crate::module_events::ModuleEvents>,
}

/// A module's menu entry as `PUT /updater/v1/modules` names it: its label
/// and the page it opens, in English and Spanish (decision 0002).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModuleMenu {
    pub label: InLanguages,
    pub path: InLanguages,
    /// The action a person must hold to see it (absent: everyone).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires: Option<String>,
}

/// A text in each language the web speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InLanguages {
    pub en: String,
    pub es: String,
}

impl InstalledModule {
    /// `name` at `version`, with what its manifest declares that `features`
    /// let the core understand.
    pub fn new(
        name: &str,
        version: &str,
        manifest: Option<&Manifest>,
        features: &Features,
    ) -> Self {
        let route = manifest.and_then(|m| m.route.as_ref());
        // A menu shown only to who holds an action goes to a core that
        // checks it, or not at all: hidden from everyone, never shown to
        // everyone (decision 0009, point 6).
        let menu = manifest
            .and_then(|m| m.menu.as_ref())
            .filter(|menu| menu.requires.is_none() || features.has(Features::MENU_REQUIRES))
            .zip(route)
            .map(|(menu, route)| {
                let path = menu.paths(route);
                ModuleMenu {
                    label: InLanguages {
                        en: menu.label.en.clone(),
                        es: menu.label.es.clone(),
                    },
                    path: InLanguages {
                        en: path.en,
                        es: path.es,
                    },
                    requires: menu.requires.clone(),
                }
            });
        Self {
            name: name.into(),
            version: version.into(),
            drain_path: manifest
                .and_then(|m| m.drain.as_ref())
                .map(|d| d.path.clone()),
            removal_path: manifest
                .and_then(|m| m.removal.as_ref())
                .map(|r| r.path.clone()),
            route_path: route.map(|r| r.path.clone()),
            menu,
            permissions: manifest
                .and_then(|m| m.permissions.clone())
                .filter(|_| features.has(Features::MODULE_PERMISSIONS)),
            provides: manifest
                .map(|m| {
                    m.provides
                        .capabilities
                        .iter()
                        .map(|c| c.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            roles: Vec::new(),
            events: None,
        }
    }
}
