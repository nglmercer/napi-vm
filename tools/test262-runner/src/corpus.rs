//! The Test262 checkout: revision checks, test selection, and worker requests.

use crate::metadata::{Metadata, Variant};
use crate::options::Options;
use serde_json::{Value as Json, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MANIFEST: &str = env!("CARGO_MANIFEST_DIR");

/// Repository root, so every default path works from any directory.
pub fn repository() -> PathBuf {
    Path::new(MANIFEST).join("../..")
}

pub fn default_corpus() -> PathBuf {
    repository().join(".napi-vm/test262")
}

pub fn default_engine() -> PathBuf {
    repository().join("target/release/napi-vm-test262")
}

pub fn catalog_path() -> PathBuf {
    Path::new(MANIFEST).join("../test262/groups.json")
}

pub fn lock_path() -> PathBuf {
    Path::new(MANIFEST).join("../test262/corpus.lock.json")
}

/// Source text exactly as stored: `Function.prototype.toString` observes it, so
/// line endings are never normalized.
pub fn read_source(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

pub struct Corpus {
    root: PathBuf,
    harness_dir: PathBuf,
    harness: Mutex<HashMap<String, Arc<str>>>,
}

impl Corpus {
    /// Open a checkout. With `expected`, the revision must match and the tree
    /// must be clean, so every result is reproducible.
    pub fn open(path: &Path, expected: Option<&str>) -> Result<(Self, String), String> {
        let root = path
            .canonicalize()
            .map_err(|error| format!("corpus {}: {error}", path.display()))?;
        let revision = git(&root, &["rev-parse", "HEAD"])?;
        if let Some(expected) = expected {
            if revision != expected {
                return Err(format!(
                    "corpus revision mismatch: {revision}, expected {expected}"
                ));
            }
            if !git(&root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
                return Err("corpus has modified or untracked files".into());
            }
        }
        let harness_dir = root
            .join("harness")
            .canonicalize()
            .map_err(|error| format!("corpus harness: {error}"))?;
        let corpus = Corpus {
            root: root.clone(),
            harness_dir,
            harness: Mutex::new(HashMap::new()),
        };
        Ok((corpus, revision))
    }

    /// Open the corpus named by `--corpus` (or the default), checking the pinned
    /// revision from `--revision` or the lock file when `strict` is set.
    pub fn from_options(options: &Options, strict: bool) -> Result<(Self, String), String> {
        let path = options
            .one("corpus")
            .map(PathBuf::from)
            .unwrap_or_else(default_corpus);
        let expected = if strict {
            Some(match options.one("revision") {
                Some(revision) => revision.to_string(),
                None => locked_revision()?,
            })
        } else {
            None
        };
        Self::open(&path, expected.as_deref())
    }

    pub fn test_dir(&self) -> PathBuf {
        self.root.join("test")
    }

    /// Test name relative to `test/`, with forward slashes.
    pub fn name(&self, test: &Path) -> String {
        test.strip_prefix(self.test_dir())
            .unwrap_or(test)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Resolve a test name (relative to `test/`) to its file.
    pub fn file(&self, name: &str) -> Result<PathBuf, String> {
        let path = self.test_dir().join(name);
        if path.is_file() {
            Ok(path)
        } else {
            Err(format!("test does not exist: {name}"))
        }
    }

    /// Union of explicit paths and named groups; no selection means everything.
    /// `_FIXTURE.js` helper files are never executed as tests.
    pub fn select(&self, paths: &[String], groups: &[String]) -> Result<Vec<PathBuf>, String> {
        let test_dir = self.test_dir();
        let mut selections: Vec<(String, Option<regex::Regex>)> =
            paths.iter().map(|path| (path.clone(), None)).collect();
        if !groups.is_empty() {
            let catalog = read_json(&catalog_path())?;
            for group in groups {
                let entry = catalog
                    .get(group)
                    .ok_or_else(|| format!("unknown focused group: {group}"))?;
                let pattern = match entry.get("pattern").and_then(Json::as_str) {
                    Some(pattern) => Some(regex::Regex::new(pattern).map_err(|e| e.to_string())?),
                    None => None,
                };
                for path in entry["paths"].as_array().into_iter().flatten() {
                    let path = path.as_str().unwrap_or_default().to_string();
                    selections.push((path, pattern.clone()));
                }
            }
        }
        if selections.is_empty() {
            selections.push((".".into(), None));
        }
        let mut tests = BTreeSet::new();
        for (selection, pattern) in selections {
            let selected = test_dir
                .join(&selection)
                .canonicalize()
                .map_err(|_| format!("selection does not exist: {selection}"))?;
            if !selected.starts_with(&test_dir) {
                return Err(format!("selection escapes test directory: {selection}"));
            }
            let mut files = Vec::new();
            collect_js(&selected, &mut files)?;
            for file in files {
                let name = self.name(&file);
                if pattern
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(&name))
                {
                    tests.insert(file);
                }
            }
        }
        Ok(tests
            .into_iter()
            .filter(|test| !test.to_string_lossy().ends_with("_FIXTURE.js"))
            .collect())
    }

    /// Harness files are read once per corpus, not once per variant.
    fn harness(&self, name: &str) -> Result<Arc<str>, String> {
        if let Some(text) = self.harness.lock().expect("harness cache").get(name) {
            return Ok(text.clone());
        }
        let canonical = self
            .harness_dir
            .join(name)
            .canonicalize()
            .map_err(|error| format!("harness {name}: {error}"))?;
        if !canonical.starts_with(&self.harness_dir) {
            return Err(format!("harness include escapes corpus: {name}"));
        }
        let text: Arc<str> = read_source(&canonical)?.into();
        self.harness
            .lock()
            .expect("harness cache")
            .insert(name.to_string(), text.clone());
        Ok(text)
    }

    /// The worker request for one variant.
    pub fn request(
        &self,
        id: &str,
        source: &str,
        metadata: &Metadata,
        variant: Variant,
    ) -> Result<Json, String> {
        let mut names: Vec<String> = if metadata.has_flag("raw") {
            Vec::new()
        } else {
            vec!["sta.js".into(), "assert.js".into()]
        };
        for include in &metadata.includes {
            if !names.contains(include) {
                names.push(include.clone());
            }
        }
        // The worker provides $DONE itself; doneprintHandle.js needs print.
        names.retain(|name| name != "doneprintHandle.js");
        let mut harness = String::new();
        for (index, name) in names.iter().enumerate() {
            if index > 0 {
                harness.push('\n');
            }
            harness.push_str(&self.harness(name)?);
        }
        let source = if variant == Variant::Strict {
            format!("\"use strict\";\n{source}")
        } else {
            source.to_string()
        };
        Ok(json!({
            "source": source,
            "harness": harness,
            "module": variant == Variant::Module,
            "asynchronous": metadata.has_flag("async"),
            "can_block": !metadata.has_flag("CanBlockIsFalse"),
            "modules": {},
            "id": id,
            "corpus_root": self.test_dir().to_string_lossy(),
        }))
    }
}

/// Clone the pinned corpus into `--corpus` (default `.napi-vm/test262`) at the
/// revision in `corpus.lock.json`. A checkout already at that revision and
/// clean is left untouched.
pub fn fetch(options: &Options) -> Result<(), String> {
    let lock = read_json(&lock_path())?;
    let repository = lock["repository"]
        .as_str()
        .ok_or("corpus.lock.json has no repository")?;
    let revision = lock["revision"]
        .as_str()
        .ok_or("corpus.lock.json has no revision")?;
    let path = options
        .one("corpus")
        .map(PathBuf::from)
        .unwrap_or_else(default_corpus);
    if let Ok((_, current)) = Corpus::open(&path, Some(revision)) {
        println!("corpus already at {current}");
        return Ok(());
    }
    std::fs::create_dir_all(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !path.join(".git").exists() {
        git(
            &path.canonicalize().map_err(|e| e.to_string())?,
            &["init", "-q"],
        )?;
    }
    let root = path.canonicalize().map_err(|error| error.to_string())?;
    git(&root, &["fetch", "-q", "--depth=1", repository, revision])?;
    git(&root, &["checkout", "-q", "--detach", "FETCH_HEAD"])?;
    let (_, checked_out) = Corpus::open(&root, Some(revision))?;
    println!("corpus fetched at {checked_out}");
    Ok(())
}

fn locked_revision() -> Result<String, String> {
    let lock = read_json(&lock_path())?;
    lock["revision"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "corpus.lock.json has no revision".into())
}

fn read_json(path: &Path) -> Result<Json, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

fn git(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn collect_js(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let entries =
        std::fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))?;
    for entry in entries {
        let child = entry.map_err(|error| error.to_string())?.path();
        if child.is_dir() {
            collect_js(&child, out)?;
        } else if child.extension().is_some_and(|extension| extension == "js") {
            out.push(child);
        }
    }
    Ok(())
}
