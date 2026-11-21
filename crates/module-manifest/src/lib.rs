//! A module's manifest, `module.yaml` (decision 0002).
//!
//! A module declares what it provides and requires as **capabilities**
//! (a name, a version and the protocols it promises), how its health is
//! checked beyond its Compose healthchecks, and what its data is, so a
//! backup covers what is critical. [`Manifest::parse`] reads and validates
//! one; [`unmet`] says whether installed modules meet a module's
//! requirements, and [`lost_on_removal`] which modules would lose a
//! required capability if one were removed.
//!
//! Pure: it reads text and answers values. Reading `module.yaml`, backing
//! up and checking health belong to the agent.

mod checks;
mod error;
mod model;
mod validate;

pub use checks::{lost_on_removal, unmet, Loss, Unmet, Why};
pub use error::ManifestError;
pub use model::{
    Check, ConsumerHold, Data, Database, Health, Key, Manifest, ProvidedCapability, ProviderHold,
    Provides, RequiredCapability, Requires, Volume, CHECK_TIMEOUT_SECS, MAX_CHECK_TIMEOUT_SECS,
};
