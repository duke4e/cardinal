//! End-to-end indexing benchmark: walk a root with Cardinal's default ignore
//! paths, then write and re-read the persistent cache.
//!
//! Run:
//!   cargo run --release -p search-cache --example bench_index -- [root] [cache_path]

use search_cache::{SearchCache, SearchOptions, WalkData};
use search_cancel::CancellationToken;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

static STOP: AtomicBool = AtomicBool::new(false);

/// Mirrors `DEFAULT_IGNORE_PATHS` in the frontend plus the system ignore path
/// the backend always appends.
fn default_ignore_paths() -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    [
        "/Volumes",
        "~/Library/CloudStorage",
        "~/Library/Biome",
        "~/Library/Caches",
        "~/Library/Logs",
        "~/Library/Metadata",
        "/Library/Caches",
        "/System/Library/Caches",
        "/private/var",
        "/private/tmp",
        "/System/Volumes/Data",
    ]
    .iter()
    .map(|p| PathBuf::from(p.replacen('~', &home, 1)))
    .chain(
        std::env::var("EXTRA_IGNORE")
            .unwrap_or_default()
            .split(':')
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
    )
    .collect()
}

fn main() {
    tracing_subscriber::fmt()
        .without_time()
        .with_target(false)
        .init();
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().unwrap_or_else(|| "/".into()));
    let cache_path = PathBuf::from(args.next().unwrap_or_else(|| {
        std::env::temp_dir()
            .join("bench_index.db")
            .display()
            .to_string()
    }));
    let ignore = default_ignore_paths();

    let walk_data = WalkData::new(&root, &ignore, &[], false, || false);
    let start = Instant::now();
    let cache = SearchCache::walk_fs_with_walk_data(&walk_data, &STOP).expect("not cancelled");
    let walk_time = start.elapsed();
    println!(
        "walk: {:?}, dirs: {}, files: {}, nodes: {}",
        walk_time,
        walk_data.num_dirs.load(Ordering::Relaxed),
        walk_data.num_files.load(Ordering::Relaxed),
        cache.get_total_files()
    );

    let start = Instant::now();
    cache.flush_to_file(&cache_path).expect("flush");
    let size = std::fs::metadata(&cache_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "write: {:?}, size: {:.1} MB",
        start.elapsed(),
        size as f64 / 1024.0 / 1024.0
    );

    let start = Instant::now();
    let mut cache =
        SearchCache::try_read_persistent_cache(&root, &cache_path, &ignore, &vec![], &STOP)
            .expect("read");
    println!(
        "read: {:?}, nodes: {}",
        start.elapsed(),
        cache.get_total_files()
    );

    for query in [
        "report",
        "*.rs",
        "index.js",
        "a",
        "/Users Library",
        "zzzz_no_match",
    ] {
        let start = Instant::now();
        let results = cache
            .search_with_options(query, SearchOptions::default(), CancellationToken::noop())
            .ok()
            .and_then(|outcome| outcome.nodes)
            .map_or(0, |nodes| nodes.len());
        println!("search {query:?}: {:?}, {results} results", start.elapsed());
    }
}
