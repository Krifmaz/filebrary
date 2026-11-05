//! The Krifmaz kit's server crates, wired to Filebrary.
//!
//! Filebrary keeps its own accounts, sessions and roles (`auth`, `identity`);
//! the kit only asks who is signed in through [`kit_server_core::Host`],
//! implemented here over [`crate::auth::AuthUser`]. Everything the kit stores
//! lives in its own `kit_*` tables in `<data_dir>/kit.db`, next to
//! `filebrary.db`, so the two never share a table or a lock.
//!
//! # Routes
//!
//! | Route | From |
//! |---|---|
//! | `GET /api/health`, `/healthz`, `/health` | kit-server-health (version; `status: "ok"` kept for old probes) |
//! | `/api/changelog`, `/api/changelog/new`, `/api/changelog/seen`, `/api/auth/prefs` | kit-server-changelog (What's new) |
//! | `/api/notices`, `/api/notices/check`, `/api/notices/{id}`, `/api/notices/{id}/check` | kit-server-notices ([`checks`]) |
//! | `/api/bug-reports/*`, `/api/crash-reports` | kit-server-bugreports |
//! | `/api/diagnostics`, `/api/diagnostics/{id}/test` | kit-server-diagnostics ([`checks`]) |
//! | `/api/settings`, `/api/settings/schema`, `/api/settings/secret/{key}` | kit-server-settings (`settings.toml`) |
//! | `GET /api/update`, `POST /api/update`, `POST /api/restart` | kit-server-updates ([`updates`]) |
//!
//! # Who may do what
//!
//! A Filebrary administrator is the kit's admin and owner (Filebrary has no
//! role above administrator). An API key counts as an admin only when the
//! account is one and the key has the `admin` scope, as for Filebrary's own
//! admin routes. Cookie sessions need the `X-CSRF-Token` header on unsafe
//! requests, as everywhere else.

pub mod checks;
pub mod updates;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::Context;
use axum::{Router, extract::FromRequestParts, http::request::Parts};
use kit_server_changelog::{Changelog, ChangelogHost};
use kit_server_core::{Account, Db, Host, PrefsStore, RowExt, SqliteDb, SqlitePrefs};
use kit_server_settings::{SettingsHost, SqliteSettings, Store, Table, Values};

use crate::{auth::AuthUser, config::Config, error::AppError, state::AppState};

/// The release notes built in from `docs/releases` by `build.rs`: `NOTES`.
mod changelog_gen {
    include!(concat!(env!("OUT_DIR"), "/changelog_gen.rs"));
}

/// The server's version: `APP_VERSION` at build time when the release
/// workflow sets it, else the crate's version.
pub const VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// The app's name as the kit shows it (What's new, bug reports).
pub const APP_NAME: &str = "Filebrary";

/// The kit's database file inside the data directory.
pub const DB_FILE: &str = "kit.db";

/// The settings table, read once from `settings.toml`.
pub fn settings_table() -> Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    *TABLE.get_or_init(|| {
        kit_server_settings::file::parse(include_str!("settings.toml"), &[])
            .unwrap_or_else(|e| panic!("kit/settings.toml: {e}"))
    })
}

/// The kit's part of the server state.
pub struct Kit {
    /// The kit's own database (`<data_dir>/kit.db`, `kit_*` tables only).
    pub db: SqliteDb,
    /// Each account's kit preferences (What's new: seen version, popup switch).
    pub prefs: SqlitePrefs,
    /// Bug reports and crash logs.
    pub reports: kit_server_bugreports::SqliteReports,
    /// The settings in effect: environment, then saved, then the defaults.
    pub settings: Store,
    /// What's new: the release notes built into this binary.
    pub changelog: Changelog,
    /// The notice banners ([`checks::notices`]), their last state kept.
    pub notices: kit_server_notices::Notices<AppState>,
    /// "Restart server": set off by `POST /api/restart`.
    pub restart: kit_server_updates::Restart,
    /// The last release seen on GitHub ([`updates`]).
    pub updates: updates::UpdateCache,
    /// The data directory (bug report attachments live below it).
    pub data_dir: PathBuf,
    /// Recent bug report and crash log submissions, per key, for the limits.
    limits: Mutex<HashMap<String, Vec<f64>>>,
}

impl std::fmt::Debug for Kit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kit")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl Kit {
    /// Opens the kit's database in the data directory and makes or updates
    /// its tables; settings are read from the environment.
    ///
    /// # Errors
    ///
    /// Fails when `kit.db` cannot be opened or its tables cannot be made.
    pub fn open(config: &Config) -> anyhow::Result<Self> {
        Self::open_with(&config.data_dir, |name| std::env::var(name).ok())
    }

    /// [`Kit::open`] on `data_dir` with the environment `env` gives (tests).
    ///
    /// # Errors
    ///
    /// As [`Kit::open`].
    pub fn open_with(
        data_dir: &Path,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        let path = data_dir.join(DB_FILE);
        let kit = |e: kit_server_core::Error| anyhow::anyhow!("{}", e.detail);
        let db = SqliteDb::open(&path)
            .map_err(kit)
            .with_context(|| format!("opening {}", path.display()))?;
        let tables = || format!("making the kit's tables in {}", path.display());
        let prefs = SqlitePrefs::new(db.clone())
            .map_err(kit)
            .with_context(tables)?;
        let reports = kit_server_bugreports::SqliteReports::new(db.clone())
            .map_err(kit)
            .with_context(tables)?;
        let saved = SqliteSettings::new(db.clone())
            .map_err(kit)
            .with_context(tables)?;
        let notices = checks::notices().keep_in(Arc::new(
            kit_server_notices::SqliteNotices::new(db.clone())
                .map_err(kit)
                .with_context(tables)?,
        ));
        let settings = Store::new(saved, settings_table(), env);
        // `Store::reload` is async only for stores on an async database; the
        // kit's SQLite store answers at once, so read the rows here.
        let rows = db
            .query("SELECT key, value FROM kit_settings", vec![])
            .map_err(kit)
            .context("reading the saved settings")?;
        let stored = rows
            .into_iter()
            .filter_map(|r| Some((r.s("key"), serde_json::from_str(&r.s("value")).ok()?)))
            .collect();
        settings.apply(&stored);
        Ok(Self {
            db,
            prefs,
            reports,
            settings,
            changelog: Changelog::new(APP_NAME, VERSION, changelog_gen::NOTES),
            notices,
            restart: kit_server_updates::Restart::default(),
            updates: updates::UpdateCache::default(),
            data_dir: data_dir.to_path_buf(),
            limits: Mutex::new(HashMap::new()),
        })
    }

    /// Whether `key` already made `limit` submissions in the last
    /// `window_secs` seconds; counts this one when it did not.
    fn limited(&self, key: &str, limit: usize, window_secs: f64) -> bool {
        let now = kit_server_core::db::now();
        let mut all = self.limits.lock().unwrap_or_else(|p| p.into_inner());
        // Forget keys that have been quiet for a whole window.
        all.retain(|_, times| times.last().is_some_and(|t| now - t < window_secs));
        let times = all.entry(key.to_owned()).or_default();
        times.retain(|t| now - t < window_secs);
        if times.len() >= limit {
            return true;
        }
        times.push(now);
        false
    }
}

/// The kit's account for a Filebrary user.
pub fn account(me: &AuthUser) -> Account {
    let admin = me.user.is_admin && me.has_scope("admin");
    Account {
        id: me.user.id,
        username: me.user.username.clone(),
        admin,
        owner: admin,
    }
}

/// Filebrary's error as the kit's `{"detail": …}` answer, with the same
/// status and words (internal details are logged, never sent).
pub fn kit_error(e: AppError) -> kit_server_core::Error {
    let detail = match &e {
        AppError::Internal(inner) => {
            tracing::error!(error = ?inner, "internal error");
            "internal error".to_owned()
        }
        other => other.to_string(),
    };
    let status = axum::response::IntoResponse::into_response(e).status();
    kit_server_core::Error::new(status.as_u16(), detail)
}

impl Host for AppState {
    async fn signed_in(&self, parts: &mut Parts) -> kit_server_core::Result<Account> {
        AuthUser::from_request_parts(parts, self)
            .await
            .map(|me| account(&me))
            .map_err(kit_error)
    }
}

impl ChangelogHost for AppState {
    fn changelog(&self) -> &Changelog {
        &self.kit.changelog
    }

    fn prefs(&self) -> &dyn PrefsStore {
        &self.kit.prefs
    }
}

impl SettingsHost for AppState {
    fn settings(&self) -> &dyn Values {
        &self.kit.settings
    }
}

impl kit_server_bugreports::BugReportsHost for AppState {
    fn reports(&self) -> &dyn kit_server_bugreports::BugReportStore {
        &self.kit.reports
    }

    fn data_dir(&self) -> PathBuf {
        self.kit.data_dir.clone()
    }

    fn app_name(&self) -> String {
        APP_NAME.to_owned()
    }

    fn app_version(&self) -> String {
        VERSION.to_owned()
    }

    fn limited(&self, key: &str, limit: usize, window_secs: f64) -> bool {
        self.kit.limited(key, limit, window_secs)
    }
}

impl kit_server_bugreports::crash::CrashHost for AppState {
    fn crashes(&self) -> &dyn kit_server_bugreports::CrashStore {
        &self.kit.reports
    }
}

impl kit_server_notices::NoticesHost for AppState {
    fn notices(&self) -> kit_server_notices::Notices<AppState> {
        self.kit.notices.clone()
    }
}

impl kit_server_diagnostics::Diagnostics for AppState {
    fn diagnostics(&self) -> kit_server_diagnostics::Registry<AppState> {
        checks::diagnostics()
    }
}

/// The health answer: the kit's `{ok, version, server, web}` plus
/// `status: "ok"`, which Filebrary's probes answered before. `web` is the
/// build of the Leptos UI (the hash in its script's name), so an open tab
/// sees a new build and reloads into it.
fn health() -> kit_server_health::Health {
    let mut h = kit_server_health::Health::new(VERSION, crate::web::next_build());
    h.extra.insert("status".into(), "ok".into());
    h
}

/// Every kit route, for the root router (paths already include `/api`).
pub fn router() -> Router<AppState> {
    let health = |path: &str| {
        kit_server_health::router_with(
            health(),
            kit_server_health::HealthPaths {
                health: path.to_owned(),
            },
        )
    };
    Router::new()
        .merge(health("/api/health"))
        .merge(health("/healthz"))
        .merge(health("/health"))
        .merge(kit_server_changelog::router())
        .merge(kit_server_changelog::prefs_router("/api/auth/prefs"))
        .merge(kit_server_notices::router())
        .merge(kit_server_bugreports::router())
        .merge(kit_server_bugreports::crash::router())
        .merge(kit_server_diagnostics::router())
        .merge(kit_server_settings::router())
        .merge(kit_server_updates::router())
}

/// Starts the kit's background work: the notices' last state is restored and
/// re-checked on their schedules, and a "Restart server" ends the process
/// with [`kit_server_updates::EXIT_RESTART`] so Docker's restart policy (or
/// the service manager) starts it again.
pub fn start(state: &AppState) {
    let st = state.clone();
    tokio::spawn(async move {
        if let Err(e) = st.kit.notices.restore().await {
            tracing::warn!(error = %e.detail, "restoring the notices failed");
        }
        kit_server_notices::spawn_rechecks(st);
    });
    let restart = state.kit.restart.clone();
    tokio::spawn(async move {
        restart.requested().await;
        tracing::info!("restarting on request");
        std::process::exit(kit_server_updates::EXIT_RESTART);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_table_parses() {
        let t = settings_table();
        assert!(t.def("update_check").is_some());
    }

    #[test]
    fn limits_count_per_key_and_window() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::open_with(dir.path(), |_| None).unwrap();
        assert!(!kit.limited("a", 2, 60.0));
        assert!(!kit.limited("a", 2, 60.0));
        assert!(kit.limited("a", 2, 60.0));
        assert!(!kit.limited("b", 2, 60.0));
    }

    #[test]
    fn saved_settings_are_read_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let kit = Kit::open_with(dir.path(), |_| None).unwrap();
        assert!(kit.settings.bool("update_check"));
        kit.db
            .execute(
                "INSERT INTO kit_settings (key, value, updated_at) VALUES ('update_check', 'false', 0)",
                vec![],
            )
            .unwrap();
        drop(kit);
        let kit = Kit::open_with(dir.path(), |_| None).unwrap();
        assert!(!kit.settings.bool("update_check"));
        let locked = Kit::open_with(dir.path(), |n| {
            (n == "FILEBRARY_UPDATE_CHECK").then(|| "true".to_owned())
        })
        .unwrap();
        assert!(locked.settings.bool("update_check"));
    }
}
