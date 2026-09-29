//! Cross-checks the walker against `read_dir` + `lstat` for a directory tree.
//!
//! Run:
//!   cargo run --release -p fswalk --example verify_walk -- <root>

use fswalk::{Node, NodeFileType, NodeMetadata, WalkData, walk_it_without_root_chain};
use std::{fs, path::Path};

#[derive(Default, Debug)]
struct Report {
    nodes: usize,
    missing_metadata: usize,
    metadata_mismatch: usize,
    children_mismatch: usize,
}

fn check(node: &Node, path: &Path, report: &mut Report) {
    report.nodes += 1;
    let expected = fs::symlink_metadata(path).ok().map(NodeMetadata::from);
    match (node.metadata, expected) {
        (Some(actual), Some(expected)) => {
            let size_matters = !matches!(actual.r#type, NodeFileType::Dir);
            if actual.r#type != expected.r#type
                || (size_matters && actual.size != expected.size)
                || actual.ctime != expected.ctime
                || actual.mtime != expected.mtime
            {
                report.metadata_mismatch += 1;
                if report.metadata_mismatch <= 10 {
                    println!("metadata mismatch {path:?}: {actual:?} vs {expected:?}");
                }
            }
        }
        (None, Some(_)) => report.missing_metadata += 1,
        _ => {}
    }
    if node.metadata.map(|m| m.r#type) == Some(NodeFileType::Dir)
        && let Ok(entries) = fs::read_dir(path)
    {
        let mut expected: Vec<String> = entries
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        expected.sort();
        let actual: Vec<String> = node.children.iter().map(|c| c.name.to_string()).collect();
        if actual != expected {
            report.children_mismatch += 1;
            if report.children_mismatch <= 10 {
                println!(
                    "children mismatch {path:?}: {} vs {}",
                    actual.len(),
                    expected.len()
                );
            }
        }
    }
    for child in &node.children {
        check(child, &path.join(&*child.name), report);
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("root path");
    let root = Path::new(&root);
    let walk_data = WalkData::new(root, &[], &[], true, || false);
    let start = std::time::Instant::now();
    let node = walk_it_without_root_chain(&walk_data).expect("not cancelled");
    println!("walk: {:?}", start.elapsed());
    let mut report = Report::default();
    check(&node, root, &mut report);
    println!("{report:?}");
}
