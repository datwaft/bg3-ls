use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::domain::{SourceFile, SourceKind};
use crate::{Error, ModuleRole, ModuleSpec};

/// Discovers all supported loose sources owned by one configured module.
pub fn discover_module(
    module: &ModuleSpec,
    game_data: &Path,
    language: &str,
    include_global_localization: bool,
) -> Result<Vec<SourceFile>, Error> {
    let mut roots = Vec::new();
    match module.role {
        ModuleRole::Base => {
            roots.push((module.root.clone(), DiscoveryMode::StatsOnly));
            roots.push((
                game_data.join("Public").join(&module.name),
                DiscoveryMode::LsxOnly,
            ));
            roots.push((
                game_data.join("Mods").join(&module.name),
                DiscoveryMode::LsxOnly,
            ));
            if include_global_localization {
                roots.push((
                    game_data.join("Localization").join(language),
                    DiscoveryMode::LocalizationOnly,
                ));
            }
        }
        ModuleRole::Dependency | ModuleRole::Project => {
            roots.push((module.root.clone(), DiscoveryMode::LooseModule));
        }
    }

    let mut files = Vec::new();
    for (root, mode) in roots {
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(root).follow_links(false) {
            let entry = entry?;
            if entry.file_type().is_file() {
                let path = entry.into_path();
                if let Some(kind) = classify(&path, mode, language) {
                    files.push(SourceFile { path, kind });
                }
            }
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    Ok(files)
}

#[derive(Clone, Copy, Debug)]
enum DiscoveryMode {
    StatsOnly,
    LsxOnly,
    LocalizationOnly,
    LooseModule,
}

/// Classifies a source path under one known discovery root.
fn classify(path: &Path, mode: DiscoveryMode, language: &str) -> Option<SourceKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let normalized = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    match mode {
        DiscoveryMode::StatsOnly => classify_stats(&normalized, &extension),
        DiscoveryMode::LsxOnly => (extension == "lsx").then_some(SourceKind::Lsx),
        DiscoveryMode::LocalizationOnly => (extension == "xml").then_some(SourceKind::Localization),
        DiscoveryMode::LooseModule => {
            if let Some(kind) = classify_stats(&normalized, &extension) {
                return Some(kind);
            }
            if extension == "lsx"
                && (normalized.contains("/public/") || normalized.contains("/mods/"))
            {
                return Some(SourceKind::Lsx);
            }
            let localization = format!("/localization/{}/", language.to_ascii_lowercase());
            if extension == "xml" && normalized.contains(&localization) {
                return Some(SourceKind::Localization);
            }
            None
        }
    }
}

/// Classifies Toolkit and legacy Stats source formats.
fn classify_stats(normalized: &str, extension: &str) -> Option<SourceKind> {
    match extension {
        "stats" if normalized.contains("/stats/") => Some(SourceKind::ToolkitStats),
        "tbl" => Some(SourceKind::Table),
        "txt" if normalized.contains("/stats/generated/data/") => Some(SourceKind::PlainStats),
        _ => None,
    }
}

/// Returns whether `path` is contained by `root` after lexical normalization.
pub fn path_is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

/// Resolves a relative dependency path against a workspace root.
pub fn resolve_path(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}
