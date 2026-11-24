//! A module's manifest, `module.yaml` (decision 0002).
//!
//! A module declares what it provides and requires as **capabilities**
//! (a name, a version and the protocols it promises), how its health is
//! checked beyond its Compose healthchecks, and what its data is, so a
//! backup covers what is critical, and where the core asks what work it
//! runs before an update (`drain`), and where people reach it: its route,
//! how they sign in to it and its menu entry. [`Manifest::parse`] reads and validates
//! one; [`unmet`] says whether installed modules meet a module's
//! requirements, and [`lost_on_removal`] which modules would lose a
//! required capability if one were removed.
//!
//! What a module gets at `module add` (decision 0009): its OIDC clients,
//! its secret files,
//! the databases the agent creates, a menu entry that depends on an
//! action, and its own actions and managed policies.
//!
//! Pure: it reads text and answers values. Reading `module.yaml`, backing
//! up and checking health belong to the agent.

mod checks;
mod error;
mod model;
mod oidc;
mod permissions;
mod route;
mod secrets;
mod validate;

pub use checks::{lost_on_removal, unmet, Loss, Unmet, Why};
pub use error::ManifestError;
pub use model::{
    Check, ConsumerHold, Data, Database, Drain, Health, Key, Manifest, ProvidedCapability,
    ProviderHold, Provides, RequiredCapability, Requires, Volume, CHECK_TIMEOUT_SECS,
    MAX_CHECK_TIMEOUT_SECS,
};
pub use oidc::{
    Oidc, OidcMachine, OidcPeople, RoleClaim, TokenAlg, MAX_OIDC_PATHS, MAX_ROLE_VALUES,
    RESERVED_CLAIMS,
};
pub use permissions::{Action, Permissions, Policy};
pub use route::{Auth, InLanguages, Menu, Route, RESERVED_ROUTE_SEGMENTS};
pub use secrets::{
    database_prefix, GeneratedSecret, Secrets, DATABASE_PASSWORD, ISSUED_SECRETS,
    MAX_CREATED_DATABASES, OIDC_MACHINE_SECRET, OIDC_PEOPLE_SECRET, SECRET_BYTES,
};
pub use validate::RESERVED_MODULE_NAMES;
