//! The SDK's few AWS calls without the AWS SDK (decision 0003): credentials
//! as a port, Signature Version 4, HTTPS behind a port, and the clients of
//! the services the agent and the `qsdk` CLI call.
//!
//! Each piece is behind a port ([`credentials::CredentialSource`],
//! [`http::Transport`]) so it is tested with fakes ([`fake`]): no test calls
//! a real account.

pub mod client;
pub mod credentials;
pub mod ecr;
pub mod fake;
pub mod http;
pub mod kms;
pub mod rolesanywhere;
pub mod sigv4;
