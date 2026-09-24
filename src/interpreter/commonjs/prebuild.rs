//! Node-API prebuild discovery and platform tag parsing.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct NodeApiPrebuildTarget {
    pub(super) platform: String,
    pub(super) architecture: String,
    pub(super) libc: Option<String>,
    pub(super) armv: Option<String>,
}

impl NodeApiPrebuildTarget {
    pub(super) fn current() -> Self {
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        }
        .to_string();
        let architecture = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "x86" => "ia32",
            "aarch64" => "arm64",
            "powerpc" => "ppc",
            "powerpc64" | "powerpc64le" => "ppc64",
            "loongarch64" => "loong64",
            other => other,
        }
        .to_string();
        let libc = if platform == "linux" {
            Some(
                std::env::var("LIBC")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        if cfg!(target_env = "musl") || Path::new("/etc/alpine-release").is_file() {
                            "musl".into()
                        } else {
                            "glibc".into()
                        }
                    }),
            )
        } else {
            None
        };
        let armv = std::env::var("ARM_VERSION")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| (architecture == "arm64").then(|| "8".into()));
        Self {
            platform,
            architecture,
            libc,
            armv,
        }
    }
}

#[derive(Debug)]
pub(super) struct PrebuildTuple {
    pub(super) name: String,
    pub(super) platform: String,
    pub(super) architectures: Vec<String>,
}

pub(super) fn node_gyp_build_prebuild_override_variable(package_name: &str) -> String {
    format!(
        "{}_PREBUILD",
        package_name.to_ascii_uppercase().replace('-', "_")
    )
}

pub(super) fn parse_prebuild_tuple(name: &str) -> Option<PrebuildTuple> {
    if name.split('-').count() != 2 {
        return None;
    }
    let (platform, architectures) = name.split_once('-')?;
    let architectures = architectures
        .split('+')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if platform.is_empty() || architectures.is_empty() || architectures.iter().any(String::is_empty)
    {
        return None;
    }
    Some(PrebuildTuple {
        name: name.to_string(),
        platform: platform.to_string(),
        architectures,
    })
}

#[derive(Default)]
pub(super) struct PrebuildTags {
    pub(super) file: String,
    pub(super) field_order: Vec<String>,
    pub(super) runtime: Option<String>,
    pub(super) napi: bool,
    pub(super) abi: Option<String>,
    pub(super) uv: Option<String>,
    pub(super) libc: Option<String>,
    pub(super) armv: Option<String>,
    pub(super) specificity: usize,
}

pub(super) fn parse_prebuild_tags(filename: &str) -> Option<PrebuildTags> {
    let stem = filename.strip_suffix(".node")?;
    let mut tags = PrebuildTags {
        file: filename.to_string(),
        ..PrebuildTags::default()
    };
    for tag in stem.split('.') {
        let field = match tag {
            "node" | "electron" | "node-webkit" => {
                tags.runtime = Some(tag.to_string());
                "runtime"
            }
            "napi" => {
                tags.napi = true;
                "napi"
            }
            "glibc" | "musl" => {
                tags.libc = Some(tag.to_string());
                "libc"
            }
            _ if tag.starts_with("abi") => {
                tags.abi = Some(tag[3..].to_string());
                "abi"
            }
            _ if tag.starts_with("uv") => {
                tags.uv = Some(tag[2..].to_string());
                "uv"
            }
            _ if tag.starts_with("armv") => {
                tags.armv = Some(tag[4..].to_string());
                "armv"
            }
            _ => continue,
        };
        if !tags.field_order.iter().any(|existing| existing == field) {
            tags.field_order.push(field.to_string());
        }
        tags.specificity += 1;
    }
    Some(tags)
}

pub(super) fn select_node_api_prebuild(
    package_root: &Path,
    target: &NodeApiPrebuildTarget,
    prebuilds_only: bool,
) -> Option<PathBuf> {
    if !prebuilds_only {
        for build_dir in ["build/Release", "build/Debug"] {
            let mut candidates = read_node_addon_files(&package_root.join(build_dir));
            if let Some(candidate) = candidates.drain(..).next() {
                return Some(candidate);
            }
        }
    }

    let prebuilds_root = package_root.join("prebuilds");
    let mut tuples = fs::read_dir(&prebuilds_root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            parse_prebuild_tuple(&entry.file_name().to_string_lossy())
        })
        .filter(|tuple| {
            tuple.platform == target.platform && tuple.architectures.contains(&target.architecture)
        })
        .collect::<Vec<_>>();
    tuples.sort_by(|left, right| {
        left.architectures
            .len()
            .cmp(&right.architectures.len())
            .then_with(|| left.name.cmp(&right.name))
    });
    let tuple = tuples.into_iter().next()?;
    let directory = prebuilds_root.join(tuple.name);

    let candidates = read_node_addon_files(&directory)
        .into_iter()
        .filter_map(|path| {
            let filename = path.file_name()?.to_str()?;
            let tags = parse_prebuild_tags(filename)?;
            if !tags.napi
                || tags.uv.as_deref().is_some_and(|uv| !uv.is_empty())
                || tags
                    .runtime
                    .as_deref()
                    .is_some_and(|runtime| runtime != "node")
                || tags
                    .libc
                    .as_deref()
                    .is_some_and(|libc| target.libc.as_deref() != Some(libc))
                || tags
                    .armv
                    .as_deref()
                    .is_some_and(|armv| target.armv.as_deref() != Some(armv))
            {
                return None;
            }
            Some((path, tags))
        })
        .collect::<Vec<_>>();
    let mut candidates = candidates;
    candidates.sort_by(|(left_path, left_tags), (right_path, right_tags)| {
        let left_runtime = usize::from(left_tags.runtime.as_deref() == Some("node"));
        let right_runtime = usize::from(right_tags.runtime.as_deref() == Some("node"));
        let left_abi = usize::from(left_tags.abi.as_deref().is_some_and(|abi| !abi.is_empty()));
        let right_abi = usize::from(right_tags.abi.as_deref().is_some_and(|abi| !abi.is_empty()));
        right_runtime
            .cmp(&left_runtime)
            .then_with(|| right_abi.cmp(&left_abi))
            .then_with(|| right_tags.specificity.cmp(&left_tags.specificity))
            .then_with(|| left_path.cmp(right_path))
    });
    candidates.into_iter().next().map(|(path, _)| path)
}

pub(super) fn read_node_addon_files(directory: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("node")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}
