//! Performance benchmark for the Files data layer, so a large project's tree stays cheap. Measures the pure
//! CPU path (`build_tree` over a synthetic flat list) and the on-disk walk (`list` over a generated tree).

use std::hint::black_box;
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};
use files::{build_tree, list, FileEntry};

/// A synthetic flat entry list: `dirs` top folders, each with `dirs` subfolders, each with `files` files.
fn synthetic(dirs: usize, files: usize) -> Vec<FileEntry> {
    let mut out = Vec::new();
    for a in 0..dirs {
        out.push(FileEntry {
            repo: String::new(),
            path: format!("d{a}"),
            is_dir: true,
            size: 0,
        });
        for b in 0..dirs {
            out.push(FileEntry {
                repo: String::new(),
                path: format!("d{a}/s{b}"),
                is_dir: true,
                size: 0,
            });
            for c in 0..files {
                out.push(FileEntry {
                    repo: String::new(),
                    path: format!("d{a}/s{b}/f{c}.rs"),
                    is_dir: false,
                    size: 100,
                });
            }
        }
    }
    out
}

fn write_tree(root: &Path, dirs: usize, files: usize) {
    for a in 0..dirs {
        for b in 0..dirs {
            let d = root.join(format!("d{a}/s{b}"));
            std::fs::create_dir_all(&d).unwrap();
            for c in 0..files {
                std::fs::write(d.join(format!("f{c}.rs")), b"fn main() {}").unwrap();
            }
        }
    }
}

fn benches(c: &mut Criterion) {
    // ~10k files (20 x 20 x 25) — a large monorepo scale.
    let entries = synthetic(20, 25);
    c.bench_function("build_tree_10k", |b| {
        b.iter(|| black_box(build_tree(black_box(&entries))))
    });

    // On-disk walk of ~4k files (14 x 14 x 20), generated once.
    let root = std::env::temp_dir().join(format!("pom-files-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write_tree(&root, 14, 20);
    c.bench_function("list_4k", |b| b.iter(|| black_box(list(black_box(&root)))));
    let _ = std::fs::remove_dir_all(&root);
}

criterion_group!(g, benches);
criterion_main!(g);
