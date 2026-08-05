use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use bg3_index::{ModuleRole, ModuleSpec};
use serde::Deserialize;
use url::Url;

use crate::Error;

/// Initialization options accepted from `vim.lsp.config`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitOptions {
    pub game_data: PathBuf,
    pub base_modules: Vec<String>,
    pub project: ProjectOptions,
    #[serde(default)]
    pub localization: LocalizationOptions,
    #[serde(default = "default_limit")]
    pub max_workspace_symbols: usize,
    #[serde(default = "default_limit")]
    pub max_completion_items: usize,
}

/// Project-specific module and diagnostic configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectOptions {
    pub name: String,
    #[serde(default)]
    pub dependencies: Vec<DependencyOptions>,
    #[serde(default)]
    pub diagnostics: DiagnosticOptions,
}

/// One explicitly configured unpacked dependency root.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyOptions {
    pub name: String,
    pub path: PathBuf,
}

/// Localization source selection.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalizationOptions {
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for LocalizationOptions {
    fn default() -> Self {
        Self {
            language: default_language(),
        }
    }
}

/// Diagnostics that can be disabled or assigned an LSP severity.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticOptions {
    #[serde(default = "default_unresolved")]
    pub unresolved_references: serde_json::Value,
}

impl Default for DiagnosticOptions {
    fn default() -> Self {
        Self {
            unresolved_references: default_unresolved(),
        }
    }
}

/// Fully validated server configuration with canonical module roots.
#[derive(Clone, Debug)]
pub struct ResolvedConfig {
    pub game_data: PathBuf,
    pub modules: Vec<ModuleSpec>,
    pub language: String,
    pub max_workspace_symbols: usize,
    pub max_completion_items: usize,
    pub unresolved_references: Option<String>,
}

impl ResolvedConfig {
    /// Validates initialization options and resolves every configured path.
    pub fn resolve(options: InitOptions, root_uri: &str) -> Result<Self, Error> {
        let root_url = Url::parse(root_uri)
            .map_err(|error| Error::Config(format!("workspace root URI is invalid: {error}")))?;
        let workspace_root = root_url
            .to_file_path()
            .map_err(|()| Error::Config("workspace root must be a file URI".into()))?;
        let workspace_root = canonical_directory(&workspace_root, "workspace root")?;

        if !options.game_data.is_absolute() {
            return Err(Error::Config("game_data must be an absolute path".into()));
        }
        let game_data = canonical_directory(&options.game_data, "game_data")?;
        if options.base_modules.is_empty() {
            return Err(Error::Config("base_modules must not be empty".into()));
        }
        if options.project.name.trim().is_empty() {
            return Err(Error::Config("project.name must not be empty".into()));
        }
        if options.localization.language.trim().is_empty() {
            return Err(Error::Config(
                "localization.language must not be empty".into(),
            ));
        }
        if options.max_workspace_symbols == 0 || options.max_completion_items == 0 {
            return Err(Error::Config("result limits must be positive".into()));
        }

        for relative in [
            "Editor/Config/Stats/StatObjectDefinitions.sod",
            "Editor/Config/UuidObjects/TableDefinitions.sod",
            "Editor/Config/Stats/Enumerations.xml",
            "Editor/Config/UuidObjects/Enumerations.toe",
        ] {
            let path = game_data.join(relative);
            if !path.is_file() {
                return Err(Error::Config(format!(
                    "required schema catalog does not exist: {}",
                    path.display()
                )));
            }
        }

        let mut names = HashSet::new();
        let mut modules = Vec::new();
        for name in options.base_modules {
            validate_name(&name, "base module")?;
            add_name(&mut names, &name)?;
            let root = canonical_directory(
                &game_data.join("Editor/Mods").join(&name),
                &format!("base module `{name}`"),
            )?;
            modules.push(ModuleSpec {
                name,
                root,
                role: ModuleRole::Base,
            });
        }
        for dependency in options.project.dependencies {
            validate_name(&dependency.name, "dependency")?;
            add_name(&mut names, &dependency.name)?;
            let path = if dependency.path.is_absolute() {
                dependency.path
            } else {
                workspace_root.join(dependency.path)
            };
            let root = canonical_directory(&path, &format!("dependency `{}`", dependency.name))?;
            modules.push(ModuleSpec {
                name: dependency.name,
                root,
                role: ModuleRole::Dependency,
            });
        }
        add_name(&mut names, &options.project.name)?;
        modules.push(ModuleSpec {
            name: options.project.name,
            root: workspace_root.clone(),
            role: ModuleRole::Project,
        });

        let unresolved_references = validate_severity(
            &options.project.diagnostics.unresolved_references,
            "project.diagnostics.unresolved_references",
        )?;

        Ok(Self {
            game_data,
            modules,
            language: options.localization.language,
            max_workspace_symbols: options.max_workspace_symbols,
            max_completion_items: options.max_completion_items,
            unresolved_references,
        })
    }
}

/// Returns the default result cap used by the Lua prototype.
const fn default_limit() -> usize {
    200
}

/// Returns the default loose localization language.
fn default_language() -> String {
    "English".into()
}

/// Returns the default typed-reference diagnostic severity.
fn default_unresolved() -> serde_json::Value {
    serde_json::Value::String("warning".into())
}

/// Canonicalizes one required directory with an actionable label.
fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, Error> {
    if !path.is_dir() {
        return Err(Error::Config(format!(
            "{label} is not a directory: {}",
            path.display()
        )));
    }
    Ok(fs::canonicalize(path)?)
}

/// Rejects empty module names before they are used as cache identities.
fn validate_name(name: &str, label: &str) -> Result<(), Error> {
    if name.trim().is_empty() {
        return Err(Error::Config(format!("{label} name must not be empty")));
    }
    Ok(())
}

/// Adds a unique module name to the visible module set.
fn add_name(names: &mut HashSet<String>, name: &str) -> Result<(), Error> {
    if !names.insert(name.to_owned()) {
        return Err(Error::Config(format!("duplicate module: {name}")));
    }
    Ok(())
}

/// Validates a severity setting or the literal `false` disable switch.
fn validate_severity(value: &serde_json::Value, label: &str) -> Result<Option<String>, Error> {
    if value == &serde_json::Value::Bool(false) {
        return Ok(None);
    }
    let Some(value) = value.as_str() else {
        return Err(Error::Config(format!(
            "{label} must be false or a severity string"
        )));
    };
    if !["error", "warning", "information", "hint"].contains(&value) {
        return Err(Error::Config(format!(
            "{label} has invalid severity `{value}`"
        )));
    }
    Ok(Some(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{InitOptions, ResolvedConfig};
    use std::path::PathBuf;
    use url::Url;

    /// Returns the repository's synthetic fixture root.
    fn fixtures() -> PathBuf {
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures"))
            .unwrap()
    }

    /// Creates valid JSON options so each test can change one contract at a time.
    fn valid_options() -> serde_json::Value {
        let root = fixtures();
        serde_json::json!({
            "game_data": root.join("game"),
            "base_modules": ["Shared"],
            "project": {
                "name": "MyMod",
                "dependencies": [{
                    "name": "Dependency",
                    "path": "../dependency"
                }],
                "diagnostics": { "unresolved_references": "hint" }
            }
        })
    }

    #[test]
    fn resolves_relative_dependencies_and_defaults() {
        let root = fixtures().join("project");
        let uri = Url::from_directory_path(&root).unwrap();
        let options: InitOptions = serde_json::from_value(valid_options()).unwrap();
        let config = ResolvedConfig::resolve(options, uri.as_str()).unwrap();

        assert_eq!(config.modules[1].root, fixtures().join("dependency"));
        assert_eq!(config.language, "English");
        assert_eq!(config.max_completion_items, 200);
        assert_eq!(config.unresolved_references.as_deref(), Some("hint"));
    }

    #[test]
    fn rejects_unknown_options_duplicate_modules_and_invalid_severity() {
        let mut unknown = valid_options();
        unknown["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<InitOptions>(unknown).is_err());

        let root = fixtures().join("project");
        let uri = Url::from_directory_path(root).unwrap();
        let mut duplicate = valid_options();
        duplicate["project"]["name"] = serde_json::json!("Shared");
        let duplicate: InitOptions = serde_json::from_value(duplicate).unwrap();
        assert!(ResolvedConfig::resolve(duplicate, uri.as_str()).is_err());

        let mut severity = valid_options();
        severity["project"]["diagnostics"]["unresolved_references"] = serde_json::json!("notice");
        let severity: InitOptions = serde_json::from_value(severity).unwrap();
        assert!(ResolvedConfig::resolve(severity, uri.as_str()).is_err());
    }

    #[test]
    fn accepts_false_to_disable_unresolved_reference_diagnostics() {
        let root = fixtures().join("project");
        let uri = Url::from_directory_path(root).unwrap();
        let mut options = valid_options();
        options["project"]["diagnostics"]["unresolved_references"] = serde_json::json!(false);
        let options: InitOptions = serde_json::from_value(options).unwrap();
        let config = ResolvedConfig::resolve(options, uri.as_str()).unwrap();
        assert_eq!(config.unresolved_references, None);
    }
}
