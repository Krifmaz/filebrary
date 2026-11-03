//! Measures how long a full index scan takes and how much memory it needs.
//!
//! ```sh
//! cargo run --release --example index_bench -- 200000
//! ```
//!
//! Creates `N` empty files (100 per folder, folders nested three deep) in a
//! temporary directory, scans them into a fresh on-disk database, rescans
//! (nothing changed), and prints the time, the peak resident memory (Linux:
//! `VmHWM`) and the database size. Results are in `docs/search.md`.

use std::time::Instant;

use filebrary::{
    AppState,
    config::{Config, SourceConfig, SourceKind},
    db,
    search::scan,
};

fn rss_kib(field: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with(field))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let n: usize = std::env::args()
        .nth(1)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(100_000);
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("files");
    let t = Instant::now();
    for i in 0..n {
        let dir = root.join(format!(
            "d{:03}/e{:03}/f{:04}",
            i / 1_000_000,
            (i / 10_000) % 100,
            (i / 100) % 100
        ));
        if i % 100 == 0 {
            std::fs::create_dir_all(&dir)?;
        }
        std::fs::write(dir.join(format!("file-{i:08}.txt")), b"")?;
    }
    eprintln!("created {n} files in {:.1?}", t.elapsed());

    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data)?;
    let config = Config {
        data_dir: data.clone(),
        sources: vec![SourceConfig {
            name: "files".into(),
            kind: SourceKind::Local,
            path: root.clone(),
        }],
        ..Config::default()
    };
    let pool = db::open(&data).await?;
    let state = AppState::new(config, pool, [0; 32])?;
    let root = std::fs::canonicalize(&root)?;
    let src = state.source("files").expect("source").clone();
    eprintln!("before scan: VmRSS {:?} KiB", rss_kib("VmRSS:"));

    let t = Instant::now();
    let stats = scan::full_scan(&state.search, &src, Some(&root)).await?;
    eprintln!("first scan: {stats:?} in {:.1?}", t.elapsed());
    eprintln!(
        "after first scan: VmRSS {:?} KiB, VmHWM {:?} KiB",
        rss_kib("VmRSS:"),
        rss_kib("VmHWM:")
    );

    let t = Instant::now();
    let stats = scan::full_scan(&state.search, &src, Some(&root)).await?;
    eprintln!("rescan: {stats:?} in {:.1?}", t.elapsed());
    eprintln!(
        "after rescan: VmRSS {:?} KiB, VmHWM {:?} KiB",
        rss_kib("VmRSS:"),
        rss_kib("VmHWM:")
    );

    let t = Instant::now();
    let user = filebrary::auth::User {
        id: 1,
        username: "a".into(),
        is_admin: true,
        grants: [(
            "files".to_owned(),
            filebrary::auth::Grant {
                perms: filebrary::auth::Perms::ALL,
                scope: filebrary::storage::VPath::root(),
            },
        )]
        .into(),
    };
    let f =
        filebrary::search::query::Filters::from_params(&filebrary::search::query::SearchParams {
            q: Some("0012345".into()),
            ..Default::default()
        })?;
    let (hits, _) = filebrary::search::query::run(&state.db, &user, &f, |_| true, false).await?;
    eprintln!(
        "substring query: {} hits in {:.1?}",
        hits.len(),
        t.elapsed()
    );

    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&state.db)
        .await?;
    let size = std::fs::metadata(data.join("filebrary.db"))?.len();
    eprintln!("database: {:.1} MiB", size as f64 / 1048576.0);
    Ok(())
}
