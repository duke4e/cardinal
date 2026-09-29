//! Times `icon_of_path_ns` for the entries of a directory.
//!
//! Run:
//!   cargo run --release -p fs-icon --example bench_icon -- <dir>

use std::time::Instant;

fn main() {
    let dir = std::env::args().nth(1).expect("directory");
    let paths: Vec<String> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .flatten()
        .take(60)
        .map(|e| e.path().display().to_string())
        .collect();
    for pass in 0..2 {
        let start = Instant::now();
        let bytes: usize = paths
            .iter()
            .filter_map(|p| fs_icon::icon_of_path_ns(p))
            .map(|d| d.len())
            .sum();
        println!(
            "pass {pass}: {} icons in {:?} ({:?}/icon), {} KB PNG",
            paths.len(),
            start.elapsed(),
            start.elapsed() / paths.len().max(1) as u32,
            bytes / 1024
        );
    }
}
