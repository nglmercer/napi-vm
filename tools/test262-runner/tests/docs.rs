//! Every relative link in the repository's Markdown must resolve. Run with
//! `cargo test -p napi-vm-test262-runner`; no corpus or engine is needed.

use regex::Regex;
use std::path::{Path, PathBuf};

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn markdown_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | ".napi-vm"
            ) {
                markdown_files(&path, out);
            }
        } else if name.ends_with(".md") {
            out.push(path);
        }
    }
}

#[test]
fn relative_markdown_links_resolve() {
    let root = repository().canonicalize().expect("repository root");
    let mut files = Vec::new();
    for directory in ["docs", "tools", "crates"] {
        markdown_files(&root.join(directory), &mut files);
    }
    files.push(root.join("README.md"));
    let link = Regex::new(r"\]\(([^)\s]+)\)").expect("link pattern");
    let mut broken = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("readable markdown");
        for capture in link.captures_iter(&text) {
            let target = &capture[1];
            if target.contains("://") || target.starts_with("mailto:") || target.starts_with('#') {
                continue;
            }
            let path = target.split('#').next().unwrap_or_default();
            if path.is_empty() {
                continue;
            }
            let resolved = file.parent().expect("file has a parent").join(path);
            if !resolved.exists() {
                let relative = file.strip_prefix(&root).unwrap_or(file);
                broken.push(format!("{}: {target}", relative.display()));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "broken relative links:\n{}",
        broken.join("\n")
    );
}
