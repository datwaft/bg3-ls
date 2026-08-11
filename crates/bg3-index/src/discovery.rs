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
                DiscoveryMode::ModuleResources,
            ));
            roots.push((
                game_data.join("Mods").join(&module.name),
                DiscoveryMode::ModuleResources,
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

/// Returns the loose roots whose changes can affect one configured module.
pub fn module_watch_roots(module: &ModuleSpec, game_data: &Path) -> Vec<PathBuf> {
    let mut roots = match module.role {
        ModuleRole::Base => vec![
            module.root.clone(),
            game_data.join("Public").join(&module.name),
            game_data.join("Mods").join(&module.name),
        ],
        ModuleRole::Dependency | ModuleRole::Project => vec![module.root.clone()],
    };
    roots.retain(|root| root.is_dir());
    roots.sort();
    roots.dedup();
    roots
}

/// Classifies a loose document that can attach to the language server.
pub fn source_kind_for_document(path: &Path) -> Option<SourceKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let normalized = normalized_path(path);
    if is_osiris_goal(&normalized, &extension) && normalized.contains("/mods/") {
        return Some(SourceKind::Osiris);
    }
    if let Some(kind) = classify_stats(&normalized, &extension) {
        return Some(kind);
    }
    match extension.as_str() {
        "lsx" if normalized.contains("/public/") || normalized.contains("/mods/") => {
            Some(SourceKind::Lsx)
        }
        "khn" if normalized.contains("/mods/") && normalized.contains("/scripts/thoth/") => {
            Some(SourceKind::Thoth)
        }
        "xml" if normalized.contains("/localization/") => Some(SourceKind::Localization),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
enum DiscoveryMode {
    StatsOnly,
    ModuleResources,
    LocalizationOnly,
    LooseModule,
}

/// Classifies a source path under one known discovery root.
fn classify(path: &Path, mode: DiscoveryMode, language: &str) -> Option<SourceKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let normalized = normalized_path(path);
    match mode {
        DiscoveryMode::StatsOnly => classify_stats(&normalized, &extension),
        DiscoveryMode::ModuleResources => classify_module_resource(&normalized, &extension),
        DiscoveryMode::LocalizationOnly => (extension == "xml").then_some(SourceKind::Localization),
        DiscoveryMode::LooseModule => {
            if is_osiris_goal(&normalized, &extension) && normalized.contains("/mods/") {
                return Some(SourceKind::Osiris);
            }
            if let Some(kind) = classify_stats(&normalized, &extension) {
                return Some(kind);
            }
            if extension == "lsx"
                && (normalized.contains("/public/") || normalized.contains("/mods/"))
            {
                return Some(SourceKind::Lsx);
            }
            if extension == "khn"
                && normalized.contains("/mods/")
                && normalized.contains("/scripts/thoth/")
            {
                return Some(SourceKind::Thoth);
            }
            let localization = format!("/localization/{}/", language.to_ascii_lowercase());
            if extension == "xml" && normalized.contains(&localization) {
                return Some(SourceKind::Localization);
            }
            None
        }
    }
}

/// Classifies source formats that can occur below a base module resource root.
fn classify_module_resource(normalized: &str, extension: &str) -> Option<SourceKind> {
    match extension {
        "txt" if is_osiris_goal(normalized, extension) => Some(SourceKind::Osiris),
        "lsx" => Some(SourceKind::Lsx),
        "khn" if normalized.contains("/scripts/thoth/") => Some(SourceKind::Thoth),
        _ => None,
    }
}

/// Tests the exact `Story/RawFiles/Goals/*.txt` source shape.
fn is_osiris_goal(normalized: &str, extension: &str) -> bool {
    extension == "txt"
        && normalized
            .rsplit_once("/story/rawfiles/goals/")
            .is_some_and(|(_, file)| !file.is_empty() && !file.contains('/'))
}

/// Uses one stable separator and case for path-based source classification.
fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase()
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
