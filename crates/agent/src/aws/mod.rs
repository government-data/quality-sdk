//! The AWS adapter (decisions 0001 step 5, 0003 step 6): credentials through
//! IAM Roles Anywhere, signed HTTPS calls to S3 and ECR, the signed index,
//! the packages, the images and the reports.
//!
//! Each piece is behind a port ([`credentials::CredentialSource`],
//! [`http::Transport`]) so it is tested with fakes: no test calls a real
//! account.

pub mod client;
pub mod credentials;
pub mod ecr;
pub mod http;
pub mod index;
pub mod packages;
pub mod sigv4;
