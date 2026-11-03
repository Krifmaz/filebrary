//! Health checks.
//!
//! * `/healthz` (and `/health`, `/api/health`) is the liveness probe: it
//!   answers as long as the process serves HTTP. The Docker `HEALTHCHECK`
//!   uses it through `filebrary healthcheck`.
//! * `/readyz` is the readiness probe: it also checks that the database
//!   answers and that every source's root can be read, each within two
//!   seconds. It returns 503 when any check fails, so a load balancer or
//!   Kubernetes stops sending traffic to an instance whose disk vanished.
//!
//! Neither reveals paths or error details; failures are logged.

use std::{collections::BTreeMap, time::Duration};

use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{state::AppState, storage::VPath};

/// How long one readiness check may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// Outcome of one readiness check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    /// The component answered.
    Ok,
    /// The component failed or timed out.
    Fail,
}

/// Body of `/readyz`.
#[derive(Debug, Serialize, ToSchema)]
pub struct Readiness {
    /// `ok` when every check passed, otherwise `fail`.
    pub status: Check,
    /// Server version.
    pub version: &'static str,
    /// Result per component: `database` and `source:<name>` for each source.
    pub checks: BTreeMap<String, Check>,
}

/// `GET /readyz`: readiness probe.
#[utoipa::path(
    get,
    path = "/readyz",
    tag = "operations",
    responses(
        (status = 200, description = "Every component is ready", body = Readiness),
        (status = 503, description = "At least one component failed", body = Readiness),
    )
)]
pub async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Readiness>) {
    let mut checks = BTreeMap::new();
    let db = tokio::time::timeout(
        CHECK_TIMEOUT,
        sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(&state.db),
    )
    .await;
    checks.insert(
        "database".to_owned(),
        outcome("database", db.map_err(|_| "timed out")),
    );
    for (name, src) in &state.sources {
        let r = tokio::time::timeout(CHECK_TIMEOUT, src.storage.stat(&VPath::root())).await;
        let key = format!("source:{name}");
        let c = outcome(&key, r.map_err(|_| "timed out"));
        checks.insert(key, c);
    }
    let ok = checks.values().all(|c| *c == Check::Ok);
    let status = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(Readiness {
            status: if ok { Check::Ok } else { Check::Fail },
            version: env!("CARGO_PKG_VERSION"),
            checks,
        }),
    )
}

fn outcome<T, E: std::fmt::Display>(what: &str, r: Result<Result<T, E>, &str>) -> Check {
    match r {
        Ok(Ok(_)) => Check::Ok,
        Ok(Err(e)) => {
            tracing::warn!(check = what, error = %e, "readiness check failed");
            Check::Fail
        }
        Err(e) => {
            tracing::warn!(check = what, error = e, "readiness check failed");
            Check::Fail
        }
    }
}
