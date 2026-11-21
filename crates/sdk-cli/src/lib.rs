//! The `quality-sdk` CLI (decision 0001): enrol an installation
//! ([`enrol`]), sign its enrolment with the installations' CA, revoke it, and publish a platform version to the signed index.
//!
//! Nothing here holds a long-lived secret: the installation's key never
//! leaves its server, and the CA's key is read only where it is kept.

pub mod args;
pub mod enrol;
pub mod pki;
