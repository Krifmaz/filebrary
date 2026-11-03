//! Indexes every supported file under a directory into a temporary content
//! index, then runs one query. Used to measure indexing speed and peak
//! memory:
//!
//! ```sh
//! cargo run --release -p filebrary-fulltext --example index_dir -- <dir> <query>
//! /usr/bin/time -v target/release/examples/index_dir <dir> <query>
//! ```

use std::path::Path;
use std::time::Instant;

use filebrary_fulltext::{ContentIndex, FulltextConfig, detect_kind, extract_text};

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            walk(root, &p, out);
        } else if ft.is_file()
            && let Ok(rel) = p.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().expect("usage: index_dir <dir> <query>"));
    let query = args.next().unwrap_or_else(|| "the".into());
    let cfg = FulltextConfig::default();
    let tmp = std::env::temp_dir().join(format!("filebrary-ft-{}", std::process::id()));
    let index = ContentIndex::open(&tmp, cfg.clone()).expect("open index");
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    let start = Instant::now();
    let (mut n, mut bytes_total) = (0usize, 0u64);
    for rel in &files {
        if detect_kind(rel).is_none() {
            continue;
        }
        let Ok(bytes) = std::fs::read(root.join(rel)) else {
            continue;
        };
        if bytes.len() as u64 > cfg.max_file_bytes() {
            continue;
        }
        bytes_total += bytes.len() as u64;
        let text = extract_text(rel, &bytes, cfg.max_chars)
            .ok()
            .flatten()
            .unwrap_or_default();
        index
            .upsert("s", rel, bytes.len() as u64, 0, &text)
            .expect("upsert");
        n += 1;
        if n % cfg.commit_every_docs == 0 {
            index.commit().expect("commit");
        }
    }
    index.commit().expect("commit");
    println!(
        "indexed {n} files ({:.1} MiB) in {:.2?}",
        bytes_total as f64 / 1048576.0,
        start.elapsed()
    );
    let t = Instant::now();
    let hits = index.search(&query, &["s"], 10, 0).expect("search");
    println!(
        "query {query:?}: {} hits in {:.2?}",
        hits.len(),
        t.elapsed()
    );
    for h in hits.iter().take(3) {
        println!("  {} ({:.2})", h.path, h.score);
    }
    drop(index);
    let _ = std::fs::remove_dir_all(&tmp);
}
