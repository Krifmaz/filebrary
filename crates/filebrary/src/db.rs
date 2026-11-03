//! SQLite database: connection pool, migrations and small helpers.
//!
//! The database lives at `<data_dir>/filebrary.db` and runs in WAL mode.
//! Migrations in `migrations/` are embedded in the binary and applied at start-up.

use std::{path::Path, str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

/// Embedded schema migrations.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Opens (creating if needed) the database in `data_dir` and runs migrations.
///
/// # Errors
///
/// Fails if the file cannot be opened or a migration fails.
pub async fn open(data_dir: &Path) -> anyhow::Result<SqlitePool> {
    let path = data_dir.join("filebrary.db");
    let url = format!("sqlite://{}", path.display());
    let opts = SqliteConnectOptions::from_str(&url)
        .with_context(|| format!("invalid database path {}", path.display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    connect(opts).await
}

/// Opens a private in-memory database (for tests).
///
/// # Errors
///
/// Fails if a migration fails.
pub async fn open_memory() -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
    // A single connection: every in-memory connection would be its own database.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

async fn connect(opts: SqliteConnectOptions) -> anyhow::Result<SqlitePool> {
    // A small pool keeps idle memory low; SQLite serialises writes anyway.
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .min_connections(1)
        .idle_timeout(Duration::from_secs(300))
        .connect_with(opts)
        .await
        .context("opening the database")?;
    MIGRATOR.run(&pool).await.context("running migrations")?;
    Ok(pool)
}

/// The current time as Unix seconds.
pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
