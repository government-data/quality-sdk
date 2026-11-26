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
//! Its machine roles (`topology`, decision 0013): the services each role
//! runs on a joined machine and how those machines are updated.
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
mod topology;
mod validate;

pub use checks::{lost_on_removal, unmet, Loss, Unmet, Why};
pub use error::ManifestError;
pub use model::{
    Backup, Check, ConsumerHold, Data, Database, Drain, Health, Key, Manifest, ProvidedCapability,
    ProviderHold, Provides, RequiredCapability, Requires, Volume, CHECK_TIMEOUT_SECS,
    MAX_CHECK_TIMEOUT_SECS,
};
pub use oidc::{
    Oidc, OidcMachine, OidcPeople, RoleClaim, TokenAlg, MAX_MACHINE_AUDIENCES, MAX_OIDC_PATHS,
    MAX_ROLE_VALUES, RESERVED_CLAIMS,
};
pub use permissions::{Action, Permissions, Policy};
pub use route::{Auth, InLanguages, Menu, Route, RESERVED_ROUTE_SEGMENTS};
pub use secrets::{
    database_prefix, GeneratedSecret, SecretContent, SecretKind, Secrets, DATABASE_PASSWORD,
    ISSUED_SECRETS, MAX_CREATED_DATABASES, OIDC_MACHINE_SECRET, OIDC_PEOPLE_SECRET, SECRET_BYTES,
};
pub use topology::{Role, Strategy, Topology, MAX_ROLES, MAX_ROLE_SERVICES, RESERVED_ROLES};
pub use validate::RESERVED_MODULE_NAMES;

/// The first agent version that reads what a module gets at `module add`
/// (decision 0009); an older agent refuses those keys.
pub const MODULE_ADD_AGENT: &str = "0.2.0";

/// The agent version a written `data.volumes[].backup` needs (decision
/// 0002): 0.3.0 first read the key, `dump` or `replica`, and refused every
/// update of a volume with no replica; this one dumps it and goes ahead
/// (decision 0012), so `qsdk publish` warns before such a package reaches
/// an agent that would refuse.
pub const REPLICA_BACKUP_AGENT: &str = "0.5.0";

/// The first agent version that reads `secrets.generate[].kind` (decision
/// 0012 step 2): an older agent refuses the key.
pub const SECRET_KIND_AGENT: &str = "0.4.0";

/// The first agent version that tells the core a module's `topology`
/// roles with their images and files (decision 0013): 0.7.0 told the
/// images only, and an older one tells none, so the role's machines would
/// get nothing or not all they run.
pub const ROLES_AGENT: &str = "0.8.0";

/// The first agent version that reads `health.oneshot` and takes a one-shot
/// service's container that exited with code 0 as healthy (decision 0002,
/// amended 2026-11-25): an older agent refuses the key.
pub const ONESHOT_AGENT: &str = "0.9.0";

/// The first agent version that reads `oidc.machine.audiences` and sends
/// the module's `requires` with its machine client (decision 0009, rulings
/// 30 and 47): an older agent refuses the key.
pub const MACHINE_AUDIENCES_AGENT: &str = "0.10.0";
