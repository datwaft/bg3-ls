use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use bg3_index::{ModuleRole, ModuleSpec};
use serde::Deserialize;
use serde_json::Value;
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

/// One partial configuration source before precedence and defaults are applied.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigLayer {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    game_data: Option<PathBuf>,
    base_modules: Option<Vec<String>>,
    project: Option<ProjectLayer>,
    localization: Option<LocalizationLayer>,
    max_workspace_symbols: Option<usize>,
    max_completion_items: Option<usize>,
}

/// Partial project values that permit field-level configuration overrides.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectLayer {
    name: Option<String>,
    dependencies: Option<Vec<DependencyOptions>>,
    diagnostics: Option<DiagnosticLayer>,
}

/// Partial localization values from one configuration source.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalizationLayer {
    language: Option<String>,
}

/// Partial diagnostic values from one configuration source.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticLayer {
    unresolved_references: Option<Value>,
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
    /// Loads workspace JSON and applies higher-priority inline LSP options.
    pub fn load(inline: Option<Value>, root_uri: &str) -> Result<Self, Error> {
        let workspace_root = workspace_root(root_uri)?;
        let config_path = workspace_root.join("bg3-ls.json");
        let json = if config_path.is_file() {
            Some(read_layer(&config_path)?)
        } else {
            None
        };
        let inline = inline
            .map(|value| parse_layer(value, "initializationOptions"))
            .transpose()?;
        let options =
            ConfigLayer::merge(json.unwrap_or_default(), inline.unwrap_or_default()).complete()?;
        Self::resolve_at_root(options, workspace_root)
    }

    /// Validates complete options directly for focused configuration tests.
    #[cfg(test)]
    fn resolve(options: InitOptions, root_uri: &str) -> Result<Self, Error> {
        Self::resolve_at_root(options, workspace_root(root_uri)?)
    }

    /// Validates a complete option set against one canonical workspace root.
    fn resolve_at_root(options: InitOptions, workspace_root: PathBuf) -> Result<Self, Error> {
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

impl ConfigLayer {
    /// Applies one higher-priority layer without replacing unrelated fields.
    fn merge(lower: Self, higher: Self) -> Self {
        Self {
            schema: higher.schema.or(lower.schema),
            game_data: higher.game_data.or(lower.game_data),
            base_modules: higher.base_modules.or(lower.base_modules),
            project: merge_project(lower.project, higher.project),
            localization: merge_localization(lower.localization, higher.localization),
            max_workspace_symbols: higher.max_workspace_symbols.or(lower.max_workspace_symbols),
            max_completion_items: higher.max_completion_items.or(lower.max_completion_items),
        }
    }

    /// Applies defaults and requires values that no default can supply.
    fn complete(self) -> Result<InitOptions, Error> {
        let project = self.project.unwrap_or_default();
        let diagnostics = project.diagnostics.unwrap_or_default();
        Ok(InitOptions {
            game_data: self
                .game_data
                .ok_or_else(|| Error::Config("game_data is required".into()))?,
            base_modules: self
                .base_modules
                .ok_or_else(|| Error::Config("base_modules is required".into()))?,
            project: ProjectOptions {
                name: project
                    .name
                    .ok_or_else(|| Error::Config("project.name is required".into()))?,
                dependencies: project.dependencies.unwrap_or_default(),
                diagnostics: DiagnosticOptions {
                    unresolved_references: diagnostics
                        .unresolved_references
                        .unwrap_or_else(default_unresolved),
                },
            },
            localization: LocalizationOptions {
                language: self
                    .localization
                    .and_then(|localization| localization.language)
                    .unwrap_or_else(default_language),
            },
            max_workspace_symbols: self.max_workspace_symbols.unwrap_or_else(default_limit),
            max_completion_items: self.max_completion_items.unwrap_or_else(default_limit),
        })
    }
}

/// Merges nested project fields while higher-priority lists replace lower lists.
fn merge_project(
    lower: Option<ProjectLayer>,
    higher: Option<ProjectLayer>,
) -> Option<ProjectLayer> {
    match (lower, higher) {
        (None, None) => None,
        (Some(layer), None) | (None, Some(layer)) => Some(layer),
        (Some(lower), Some(higher)) => Some(ProjectLayer {
            name: higher.name.or(lower.name),
            dependencies: higher.dependencies.or(lower.dependencies),
            diagnostics: merge_diagnostics(lower.diagnostics, higher.diagnostics),
        }),
    }
}

/// Merges localization objects by field.
fn merge_localization(
    lower: Option<LocalizationLayer>,
    higher: Option<LocalizationLayer>,
) -> Option<LocalizationLayer> {
    match (lower, higher) {
        (None, None) => None,
        (Some(layer), None) | (None, Some(layer)) => Some(layer),
        (Some(lower), Some(higher)) => Some(LocalizationLayer {
            language: higher.language.or(lower.language),
        }),
    }
}

/// Merges diagnostic objects by field.
fn merge_diagnostics(
    lower: Option<DiagnosticLayer>,
    higher: Option<DiagnosticLayer>,
) -> Option<DiagnosticLayer> {
    match (lower, higher) {
        (None, None) => None,
        (Some(layer), None) | (None, Some(layer)) => Some(layer),
        (Some(lower), Some(higher)) => Some(DiagnosticLayer {
            unresolved_references: higher.unresolved_references.or(lower.unresolved_references),
        }),
    }
}

/// Reads and parses the workspace JSON configuration with a source label.
fn read_layer(path: &Path) -> Result<ConfigLayer, Error> {
    let source = fs::read_to_string(path).map_err(|error| {
        Error::Config(format!(
            "cannot read configuration {}: {error}",
            path.display()
        ))
    })?;
    let value: Value = serde_json::from_str(&source).map_err(|error| {
        Error::Config(format!(
            "configuration {} is not valid JSON: {error}",
            path.display()
        ))
    })?;
    parse_layer(value, &path.display().to_string())
}

/// Rejects null deletion semantics and parses one partial configuration layer.
fn parse_layer(value: Value, source: &str) -> Result<ConfigLayer, Error> {
    reject_nulls(&value, source)?;
    serde_json::from_value(value)
        .map_err(|error| Error::Config(format!("configuration {source} is not valid: {error}")))
}

/// Rejects null at any depth because merge deletion has no defined behavior.
fn reject_nulls(value: &Value, path: &str) -> Result<(), Error> {
    match value {
        Value::Null => Err(Error::Config(format!(
            "configuration value `{path}` is null"
        ))),
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                reject_nulls(value, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for (name, value) in values {
                reject_nulls(value, &format!("{path}.{name}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Converts an LSP root URI to one canonical workspace directory.
fn workspace_root(root_uri: &str) -> Result<PathBuf, Error> {
    let root_url = Url::parse(root_uri)
        .map_err(|error| Error::Config(format!("workspace root URI is invalid: {error}")))?;
    let root = root_url
        .to_file_path()
        .map_err(|()| Error::Config("workspace root must be a file URI".into()))?;
    canonical_directory(&root, "workspace root")
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
    use std::fs;
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

    #[test]
    fn loads_json_configuration_with_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let fixture_root = fixtures();
        fs::write(
            root.join("bg3-ls.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "$schema": "https://example.invalid/bg3-ls.schema.json",
                "game_data": fixture_root.join("game"),
                "base_modules": ["Shared"],
                "project": { "name": "JsonProject" }
            }))
            .unwrap(),
        )
        .unwrap();
        let uri = Url::from_directory_path(root).unwrap();

        let config = ResolvedConfig::load(None, uri.as_str()).unwrap();

        assert_eq!(config.modules.last().unwrap().name, "JsonProject");
        assert_eq!(config.language, "English");
        assert_eq!(config.max_workspace_symbols, 200);
        assert_eq!(config.unresolved_references.as_deref(), Some("warning"));
    }

    #[test]
    fn merges_inline_options_above_json_by_field() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let fixture_root = fixtures();
        fs::write(
            root.join("bg3-ls.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "game_data": fixture_root.join("game"),
                "base_modules": ["Shared"],
                "project": {
                    "name": "JsonProject",
                    "dependencies": [{
                        "name": "Dependency",
                        "path": fixture_root.join("dependency")
                    }],
                    "diagnostics": { "unresolved_references": "warning" }
                },
                "localization": { "language": "German" },
                "max_workspace_symbols": 111,
                "max_completion_items": 222
            }))
            .unwrap(),
        )
        .unwrap();
        let inline = serde_json::json!({
            "project": {
                "name": "InlineProject",
                "dependencies": [],
                "diagnostics": { "unresolved_references": "hint" }
            },
            "localization": { "language": "English" },
            "max_completion_items": 50
        });
        let uri = Url::from_directory_path(root).unwrap();

        let config = ResolvedConfig::load(Some(inline), uri.as_str()).unwrap();

        assert_eq!(config.modules.len(), 2);
        assert_eq!(config.modules.last().unwrap().name, "InlineProject");
        assert_eq!(config.language, "English");
        assert_eq!(config.max_workspace_symbols, 111);
        assert_eq!(config.max_completion_items, 50);
        assert_eq!(config.unresolved_references.as_deref(), Some("hint"));
    }

    #[test]
    fn rejects_unknown_json_keys_and_null_overrides() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let uri = Url::from_directory_path(root).unwrap();
        fs::write(root.join("bg3-ls.json"), r#"{"unexpected":true}"#).unwrap();
        assert!(ResolvedConfig::load(None, uri.as_str()).is_err());

        fs::write(root.join("bg3-ls.json"), r#"{"game_data":null}"#).unwrap();
        let error = ResolvedConfig::load(None, uri.as_str()).unwrap_err();
        assert!(error.to_string().contains("null"));
    }

    #[test]
    fn ships_a_valid_json_schema() {
        let schema = include_str!("../../../schemas/bg3-ls.schema.json");
        let value: serde_json::Value = serde_json::from_str(schema).unwrap();
        assert_eq!(
            value["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert_eq!(value["additionalProperties"], false);
    }
}
