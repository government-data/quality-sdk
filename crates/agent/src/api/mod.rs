//! quality-data's API as the engine asks it: the `updater/v1` protocol
//! (decision 0003), served by quality-data on its `internal` entry point
//! with mTLS. The agent holds a client only, behind the [`Transport`] port:
//! the real one is HTTPS with the agent's certificate ([`tls`]), the tests use a local
//! fake server.
//!
//! | Call | `Host` |
//! | --- | --- |
//! | `PUT /updater/v1/state` | `save` (its failure is ignored) |
//! | `GET /updater/v1/orders` | `orders` (no answer: approval and no request) |
//! | `PUT /updater/v1/maintenance` | `maintenance` |
//! | `GET /updater/v1/drain?horizon_secs=N` | `drain` |
//! | `GET /updater/v1/version` | the version part of `healthy`; its `features` (decision 0009) |
//! | `PUT /updater/v1/modules` | which modules are installed (after a start, and when the agent starts) |
//! | `POST /updater/v1/request` | "Update now" for a person on the server (`update`, decision 0007) |

use std::time::Duration;

use aws_client::http::{Request, Transport};
use serde_json::{json, Value};
use updater::engine::Drain;
use updater::policy::Orders;
use updater::state::State;

use crate::host::{Api, Features, InstalledModule};

pub mod internal_files;
pub mod renew;
pub mod tls;

/// The protocol's prefix.
pub const PREFIX: &str = "/updater/v1";

/// What a call's error says when quality-data did not answer it: the
/// connection or its TLS failed (not an HTTP error answer).
const NO_ANSWER: &str = "quality-data does not answer";

/// Whether `err`, from a call of [`UpdaterApi`], is one quality-data did
/// not answer (no connection, or a TLS handshake that failed).
pub fn no_answer(err: &str) -> bool {
    err.contains(NO_ANSWER)
}

/// The `updater/v1` client.
pub struct UpdaterApi {
    base: String,
    transport: Box<dyn Transport>,
}

impl std::fmt::Debug for UpdaterApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdaterApi")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl UpdaterApi {
    /// `base`: quality-data's internal entry point (`https://host:port`,
    /// with the path its API is served under, `/api` behind Traefik).
    pub fn new(base: &str, transport: Box<dyn Transport>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            transport,
        }
    }

    /// The client the settings name, with the agent's certificates at
    /// `paths`.
    pub fn from_settings(
        settings: &crate::config::ApiSettings,
        paths: &crate::paths::Paths,
    ) -> Result<Self, String> {
        let files = internal_files::InternalFiles::new(paths);
        files.finish_pending()?;
        let transport = tls::Internal::new(files, settings.connect)?;
        Ok(Self::new(&settings.url, Box::new(transport)))
    }

    /// `POST /updater/v1/request`: "Update now" for a person on the server
    /// (decision 0007); the request's id. quality-data answers 409 while an
    /// update is under way. The body is an empty object, so the request
    /// has a length: quality-data reads none.
    pub fn request_update(&self) -> Result<String, String> {
        let v = self.call("POST", "/request", Some(&json!({})))?;
        v["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("POST {PREFIX}/request: no `id` in its answer"))
    }

    /// One call; its JSON answer (`null` when it has none).
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let mut request = Request::new(method, &format!("{}{PREFIX}{path}", self.base));
        request
            .headers
            .push(("accept".into(), "application/json".into()));
        if let Some(body) = body {
            request
                .headers
                .push(("content-type".into(), "application/json".into()));
            request.body = body.to_string().into_bytes();
        }
        let what = format!("{method} {PREFIX}{path}");
        let r = self
            .transport
            .send(request)
            .map_err(|e| format!("{what}: {NO_ANSWER}: {e}"))?;
        let answer: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
        if (200..300).contains(&r.status) {
            return Ok(answer);
        }
        let why = answer["error"].as_str().unwrap_or_default();
        let why: String = why.chars().take(300).collect();
        Err(format!("{what}: quality-data answered {} {why}", r.status)
            .trim_end()
            .to_string())
    }
}

impl Api for UpdaterApi {
    fn put_state(&self, state: &State) -> Result<(), String> {
        let body = serde_json::to_value(state).map_err(|e| e.to_string())?;
        self.call("PUT", "/state", Some(&body)).map(|_| ())
    }

    fn orders(&self) -> Orders {
        match self.call("GET", "/orders", None) {
            Ok(v) => serde_json::from_value(v).unwrap_or_else(|e| {
                tracing::warn!("orders that do not read (approval is expected): {e}");
                Orders::default()
            }),
            Err(e) => {
                tracing::debug!("no orders (approval is expected): {e}");
                Orders::default()
            }
        }
    }

    fn maintenance(&self, reason: Option<&str>, lasts: Duration) -> Result<(), String> {
        let body = match reason {
            Some(r) => json!({ "active": true, "reason": r, "secs": lasts.as_secs() }),
            None => json!({ "active": false }),
        };
        self.call("PUT", "/maintenance", Some(&body)).map(|_| ())
    }

    fn drain(&self, horizon: Duration) -> Result<Drain, String> {
        let path = format!("/drain?horizon_secs={}", horizon.as_secs());
        let v = self.call("GET", &path, None)?;
        let drained = v["drained"]
            .as_bool()
            .ok_or_else(|| format!("GET {PREFIX}/drain: no `drained` in its answer"))?;
        let n = |k: &str| v[k].as_u64().unwrap_or_default();
        Ok(Drain {
            drained,
            running: n("running"),
            deploying: n("deploying"),
            scheduled_soon: n("scheduled_soon"),
        })
    }

    fn version(&self) -> Result<String, String> {
        let v = self.call("GET", "/version", None)?;
        v["version"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("GET {PREFIX}/version: no `version` in its answer"))
    }

    fn put_modules(&self, modules: &[InstalledModule]) -> Result<(), String> {
        let body = json!({ "modules": modules });
        self.call("PUT", "/modules", Some(&body)).map(|_| ())
    }

    /// A core that predates `features` answers without it: none.
    fn features(&self) -> Result<Features, String> {
        let v = self.call("GET", "/version", None)?;
        let names = v["features"].as_array().map(|a| {
            a.iter()
                .filter_map(|f| f.as_str().map(str::to_string))
                .collect()
        });
        Ok(Features(names.unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests;
