//! The server's update card (kit-server-updates): Filebrary's newest release
//! on GitHub, compared with the running version.
//!
//! Filebrary ships as a Docker image, so the server does not replace itself:
//! `POST /api/update` answers 409 with the command that updates it, and the
//! card says the same in `how`. "Restart server" (`POST /api/restart`) works:
//! the process exits with code 75 and Docker's restart policy starts it again.
//!
//! The release list is asked for at most every [`CHECK_EVERY`] (or at once
//! with `?force=1`), only while the `update_check` setting is on.

use std::time::Duration;

use kit_server_core::{Account, err};
use kit_server_updates::{
    feed::Feed,
    server::{self, Channel},
};
use serde_json::{Value, json};

use crate::state::AppState;

/// How long a fetched release list is used before GitHub is asked again.
pub const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

/// How long one request to GitHub may take.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// How to update a Filebrary server, shown on the update card.
pub const HOW: &str =
    "Pull the new image and start it again: docker compose pull && docker compose up -d";

/// Where Filebrary's releases are.
pub fn feed() -> Feed {
    Feed::new("Krifmaz", "filebrary")
}

/// The last answer from GitHub.
#[derive(Debug, Default)]
pub struct UpdateCache(tokio::sync::Mutex<Cached>);

/// What [`UpdateCache`] holds.
#[derive(Debug, Default)]
struct Cached {
    /// The newest release, as `server::parse_release` keeps it.
    release: Option<Value>,
    /// When GitHub was last asked (unix seconds), successfully or not.
    checked_at: Option<f64>,
    /// Why the last check failed, in words.
    error: Option<String>,
}

/// Asks GitHub for the release list.
async fn fetch(feed: &Feed) -> Result<Value, String> {
    // The binary installs ring as rustls's provider at start; tests may not.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(format!("filebrary/{}", super::VERSION))
        .build()
        .map_err(|e| format!("the HTTP client can't start: {e}"))?;
    let resp = client
        .get(feed.releases_api())
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub can't be reached: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("GitHub answered {status}"));
    }
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("GitHub's answer was cut off: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("GitHub's answer is not JSON: {e}"))
}

/// Whether this build has a release number (`0.3.0`), not a development one.
fn is_dev() -> bool {
    !kit_server_updates::is_release_version(super::VERSION)
}

/// The update card: the kit's shared answer plus Filebrary's fields
/// (`enabled`, `channel`, `can_update`, `how`, `error`).
pub async fn status(st: &AppState, force: bool) -> Value {
    let feed = feed();
    let enabled = st.kit.settings.bool("update_check");
    let mut cached = st.kit.updates.0.lock().await;
    let now = kit_server_core::db::now();
    let stale = cached
        .checked_at
        .is_none_or(|t| now - t >= CHECK_EVERY.as_secs_f64());
    if enabled && (force || stale) {
        match fetch(&feed).await {
            Ok(list) => {
                cached.release = server::pick_newest(&list, Channel::Stable, now, &feed);
                cached.error = None;
            }
            Err(e) => {
                tracing::warn!(error = %e, "update check failed");
                cached.error = Some(e);
            }
        }
        cached.checked_at = Some(now);
    }
    let mut v = server::answer(cached.release.as_ref(), super::VERSION, is_dev(), &feed);
    if let Value::Object(o) = &mut v {
        o.insert("enabled".into(), json!(enabled));
        o.insert("channel".into(), json!(Channel::Stable.name()));
        o.insert("can_update".into(), json!(false));
        o.insert("how".into(), json!(HOW));
        o.insert("error".into(), json!(cached.error));
    }
    v
}

impl kit_server_updates::Updates for AppState {
    async fn update_status(&self, force: bool) -> kit_server_core::Result<Value> {
        Ok(status(self, force).await)
    }

    async fn update_now(
        &self,
        _who: &Account,
        _tag: Option<String>,
    ) -> kit_server_core::Result<Value> {
        Err(err(409, format!("This server can't update itself. {HOW}")))
    }

    fn restart(&self, who: &Account) {
        tracing::info!(user = %who.username, "restart requested");
        self.kit.restart.schedule(kit_server_updates::RESTART_DELAY);
    }
}
