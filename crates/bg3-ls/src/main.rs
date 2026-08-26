mod config;
mod conversion;
mod coordinator;
mod lifecycle;
mod server;

use std::collections::{BTreeSet, HashSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use bg3_ide::{DiagnosticSeverity, OverlaySet, WorkspaceSnapshot, definition_target};
use bg3_index::{
    CacheStats, CacheStore, ModuleIndex, ModuleRole, ModuleSpec, OSIRIS_FACTS_EXTRACTOR_VERSION,
    PackagedOsirisIndex, SourceKind, THOTH_FACTS_EXTRACTOR_VERSION, discover_module,
    inventory_packaged_thoth, packaged_thoth_package_candidates, parse_osiris_goal_source,
    parse_thoth_file, read_packaged_osiris_catalog, read_packaged_stats_catalog_from_packages,
    read_packaged_thoth_catalog,
};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use thiserror::Error;
use tower_lsp_server::{LspService, Server};
use tracing_subscriber::EnvFilter;

use crate::config::ResolvedConfig;
use crate::server::Backend;

/// Standalone language intelligence for Baldur's Gate 3 Stats files.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// Overrides the XDG cache directory.
    #[arg(long, global = true)]
    cache_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Maintenance commands that do not start the LSP transport.
#[derive(Debug, Subcommand)]
enum Command {
    /// Inspects or removes disposable parsed-file caches.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Measures cold and warm full-data indexing with a dedicated disposable cache.
    Benchmark(BenchmarkOptions),
    /// Checks project Stats, Osiris, and Thoth files without an LSP client.
    Check(CheckOptions),
    /// Reports aggregate installed packaged-Thoth source coverage as JSON.
    Inventory(InventoryOptions),
    /// Converts one loose BG3 resource between binary LSF and textual LSX.
    Convert(conversion::Options),
    /// Generates or validates the checked-in Osiris engine contract catalog.
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

/// Operations on the generated Osiris engine contract catalog.
#[derive(Debug, Subcommand)]
enum CatalogCommand {
    /// Generates catalog Rust source from a game story header.
    Generate(CatalogOptions),
    /// Checks a generated catalog against the current game story header.
    Check(CatalogOptions),
}

/// Inputs shared by catalog generation and validation.
#[derive(Clone, Debug, clap::Args)]
struct CatalogOptions {
    /// Path to the source story_header.div declaration file.
    #[arg(long)]
    input: PathBuf,
    /// Path to the BG3 installation used to read its exact build version.
    #[arg(long)]
    game_root: PathBuf,
    /// Destination for generated Rust source.
    #[arg(long)]
    output: PathBuf,
}

/// Inputs for one standalone diagnostic pass.
#[derive(Clone, Debug, clap::Args)]
struct CheckOptions {
    /// Files or directories that limit diagnostic output.
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,
    /// Selects human-readable or stable JSON diagnostic output.
    #[arg(long, value_enum, default_value = "human")]
    format: CheckFormat,
    /// Selects the minimum severity that produces exit code 1.
    #[arg(long, value_enum, default_value = "error")]
    fail_on: FailOn,
}

/// Inputs for an aggregate-only installed packaged-Thoth inventory.
#[derive(Clone, Debug, clap::Args)]
struct InventoryOptions {
    /// Absolute BG3 Data directory to inspect without extracting package entries.
    #[arg(long)]
    game_data: PathBuf,
}

/// Output encodings supported by the diagnostic command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CheckFormat {
    Human,
    Json,
}

/// Diagnostic thresholds supported by the diagnostic command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum FailOn {
    Error,
    Warning,
    Information,
    Hint,
    Never,
}

/// Inputs required to reproduce a full-data indexing baseline.
#[derive(Clone, Debug, clap::Args)]
struct BenchmarkOptions {
    #[arg(long)]
    game_data: PathBuf,
    #[arg(long)]
    workspace_root: PathBuf,
    #[arg(long)]
    project_name: String,
    #[arg(long = "base-module", required = true)]
    base_modules: Vec<String>,
    #[arg(long = "dependency")]
    dependencies: Vec<BenchmarkDependency>,
    #[arg(long, default_value = "English")]
    language: String,
    #[arg(long, default_value_t = 5)]
    trials: usize,
}

/// A command-line dependency in the form `NAME=PATH`.
#[derive(Clone, Debug)]
struct BenchmarkDependency {
    name: String,
    path: PathBuf,
}

impl FromStr for BenchmarkDependency {
    type Err = String;

    /// Splits one dependency at the first equals sign so paths can contain spaces.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (name, path) = value
            .split_once('=')
            .ok_or_else(|| "dependencies must use NAME=PATH".to_owned())?;
        if name.trim().is_empty() || path.is_empty() {
            return Err("dependency names and paths must not be empty".into());
        }
        Ok(Self {
            name: name.into(),
            path: path.into(),
        })
    }
}

/// Supported cache maintenance operations.
#[derive(Debug, Subcommand)]
enum CacheCommand {
    /// Prints the active cache directory.
    Path,
    /// Prints cache file and byte counts.
    Info,
    /// Removes every cache object and manifest.
    Clear,
}

/// Failures at the binary, protocol, and asynchronous task boundaries.
#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("index error: {0}")]
    Index(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error(transparent)]
    IndexData(#[from] bg3_index::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Notify(#[from] notify::Error),
    #[error(transparent)]
    Conversion(#[from] conversion::Error),
}

/// Runs cache maintenance or serves LSP messages over stdio by default.
#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("BG3_LS_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        return match run_command(command, cli.cache_dir) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("bg3-ls: {error}");
                ExitCode::from(2)
            }
        };
    }

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let cache_dir = cli.cache_dir;
    let lifecycle = lifecycle::State::default();
    let (service, socket) = LspService::new(move |client| Backend::new(client, cache_dir.clone()));
    let service = lifecycle::Service::new(service, lifecycle.clone());
    Server::new(stdin, stdout, socket).serve(service).await;
    lifecycle.exit_code()
}

/// Executes one non-LSP command and writes only human-readable stdout.
fn run_command(command: Command, cache_dir: Option<PathBuf>) -> Result<ExitCode, Error> {
    match command {
        Command::Cache {
            command: CacheCommand::Path,
        } => {
            println!("{}", open_cache(cache_dir)?.root().display());
            Ok(ExitCode::SUCCESS)
        }
        Command::Cache {
            command: CacheCommand::Info,
        } => {
            let cache = open_cache(cache_dir)?;
            let (files, bytes) = cache.info()?;
            println!("{} files, {} bytes", files, bytes);
            Ok(ExitCode::SUCCESS)
        }
        Command::Cache {
            command: CacheCommand::Clear,
        } => {
            let cache = open_cache(cache_dir)?;
            cache.clear()?;
            println!("cleared {}", cache.root().display());
            Ok(ExitCode::SUCCESS)
        }
        Command::Benchmark(options) => {
            let cache_dir = cache_dir.ok_or_else(|| {
                Error::Config("benchmark requires an explicit --cache-dir".into())
            })?;
            let report = run_benchmark(options, CacheStore::new(cache_dir)?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .map_err(|error| Error::Protocol(error.to_string()))?
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Check(options) => run_check(options, open_cache(cache_dir)?),
        Command::Inventory(options) => {
            let game_data = fs::canonicalize(&options.game_data)?;
            if !game_data.is_dir() {
                return Err(Error::Config(
                    "inventory game_data must be a directory".into(),
                ));
            }
            let inventory = inventory_packaged_thoth(&game_data)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&inventory)
                    .map_err(|error| Error::Protocol(error.to_string()))?
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Convert(options) => {
            conversion::convert(&options)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Catalog { command } => run_catalog(command),
    }
}

fn run_catalog(command: CatalogCommand) -> Result<ExitCode, Error> {
    let (options, check) = match command {
        CatalogCommand::Generate(options) => (options, false),
        CatalogCommand::Check(options) => (options, true),
    };
    let source = fs::read_to_string(&options.input)?;
    let version = bg3_index::detect_game_build_version(&options.game_root)
        .map_err(|error| Error::Index(error.to_string()))?
        .version;
    let rendered = bg3_index::generate_osiris_catalog(&source, &version)
        .map_err(|error| Error::Index(error.to_string()))?;
    if check {
        let existing = fs::read_to_string(&options.output)?;
        if existing != rendered {
            return Err(Error::Config(format!(
                "catalog is out of date: {}",
                options.output.display()
            )));
        }
        println!("catalog is up to date: {}", options.output.display());
    } else {
        let parent = options.output.parent().unwrap_or_else(|| Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(rendered.as_bytes())?;
        temporary
            .persist(&options.output)
            .map_err(|error| Error::Io(error.error))?;
        println!("wrote catalog: {}", options.output.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// Opens the explicit cache override or the normal XDG cache.
fn open_cache(cache_dir: Option<PathBuf>) -> Result<CacheStore, Error> {
    Ok(if let Some(path) = cache_dir {
        CacheStore::new(path)?
    } else {
        CacheStore::xdg()?
    })
}

/// One stable JSON diagnostic record with zero-based source positions.
#[derive(Debug, Serialize)]
struct CheckDiagnostic {
    path: String,
    range: CheckRange,
    severity: &'static str,
    code: String,
    message: String,
}

/// One zero-based diagnostic range in JSON output.
#[derive(Debug, Serialize)]
struct CheckRange {
    start: CheckPosition,
    end: CheckPosition,
}

/// One zero-based line and character pair in JSON output.
#[derive(Debug, Serialize)]
struct CheckPosition {
    line: u32,
    character: u32,
}

/// Builds a workspace, computes diagnostics, and selects the process exit code.
fn run_check(options: CheckOptions, cache: CacheStore) -> Result<ExitCode, Error> {
    let current_directory = env::current_dir()?;
    let config = ResolvedConfig::discover(&current_directory)?;
    eprintln!("bg3-ls: building the workspace index");
    let (workspace, _) = build_workspace(
        &cache,
        &config.game_data,
        &config.modules,
        &config.language,
        config.max_workspace_symbols,
        config.max_completion_items,
        &config.incomplete_kinds(),
    )?;
    let project_root = config
        .modules
        .iter()
        .find(|module| module.role == ModuleRole::Project)
        .map(|module| module.root.as_path())
        .ok_or_else(|| Error::Config("the configuration has no project module".into()))?;
    let paths = select_check_paths(&workspace, project_root, &current_directory, &options.paths)?;
    let reference_severity = config
        .unresolved_references
        .as_deref()
        .map(configured_severity);
    let overlays = OverlaySet::default();
    let mut output = Vec::new();
    let mut failed = false;
    for path in paths {
        for diagnostic in workspace.diagnostics(&path, &overlays, reference_severity) {
            failed |= options.fail_on.matches(diagnostic.severity);
            output.push(CheckDiagnostic {
                path: display_path(project_root, &path),
                range: CheckRange {
                    start: CheckPosition {
                        line: diagnostic.range.start.line,
                        character: diagnostic.range.start.character,
                    },
                    end: CheckPosition {
                        line: diagnostic.range.end.line,
                        character: diagnostic.range.end.character,
                    },
                },
                severity: severity_name(diagnostic.severity),
                code: diagnostic.code,
                message: diagnostic.message,
            });
        }
    }
    match options.format {
        CheckFormat::Human => print_human_diagnostics(&output),
        CheckFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&output)
                .map_err(|error| Error::Protocol(error.to_string()))?
        ),
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Selects indexed diagnostic sources from defaults or explicit paths.
fn select_check_paths(
    workspace: &WorkspaceSnapshot,
    project_root: &Path,
    current_directory: &Path,
    requested: &[PathBuf],
) -> Result<Vec<PathBuf>, Error> {
    if requested.is_empty() {
        let project = workspace
            .layers
            .iter()
            .find(|layer| layer.spec.role == ModuleRole::Project)
            .ok_or_else(|| Error::Config("the workspace has no project index".into()))?;
        return Ok(project
            .files
            .iter()
            .filter(|(_, file)| supports_diagnostics(file.source.kind))
            .map(|(path, _)| path.clone())
            .collect());
    }

    let mut selected = BTreeSet::new();
    for requested_path in requested {
        let path = if requested_path.is_absolute() {
            requested_path.clone()
        } else {
            current_directory.join(requested_path)
        };
        let path = fs::canonicalize(&path).map_err(|error| {
            Error::Config(format!(
                "cannot resolve diagnostic path {}: {error}",
                path.display()
            ))
        })?;
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(&path).follow_links(false) {
                let entry = entry.map_err(|error| Error::Config(error.to_string()))?;
                if entry.file_type().is_file()
                    && is_indexed_diagnostic_source(workspace, entry.path())
                {
                    selected.insert(entry.into_path());
                }
            }
        } else if is_indexed_diagnostic_source(workspace, &path) {
            selected.insert(path);
        } else {
            return Err(Error::Config(format!(
                "diagnostic path is not an indexed Stats, Osiris, or Thoth file: {}",
                path.display()
            )));
        }
    }
    if selected.is_empty() {
        return Err(Error::Config(format!(
            "the selected paths contain no indexed Stats, Osiris, or Thoth files below {}",
            project_root.display()
        )));
    }
    Ok(selected.into_iter().collect())
}

/// Tests whether one path has a parsed diagnostic record in any visible layer.
fn is_indexed_diagnostic_source(workspace: &WorkspaceSnapshot, path: &Path) -> bool {
    workspace.layers.iter().any(|layer| {
        layer
            .file(path)
            .is_some_and(|file| supports_diagnostics(file.source.kind))
    })
}

/// Returns whether one source kind can produce proven diagnostics.
fn supports_diagnostics(kind: SourceKind) -> bool {
    matches!(
        kind,
        SourceKind::PlainStats | SourceKind::Osiris | SourceKind::Thoth
    )
}

/// Prints diagnostics in the common path, line, column form.
fn print_human_diagnostics(diagnostics: &[CheckDiagnostic]) {
    for diagnostic in diagnostics {
        println!(
            "{}:{}:{}: {} [{}] {}",
            diagnostic.path,
            diagnostic.range.start.line + 1,
            diagnostic.range.start.character + 1,
            diagnostic.severity,
            diagnostic.code,
            diagnostic.message
        );
    }
}

/// Returns a project-relative display path when one is available.
fn display_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Converts the validated configuration severity to the editor-neutral type.
fn configured_severity(value: &str) -> DiagnosticSeverity {
    match value {
        "error" => DiagnosticSeverity::Error,
        "warning" => DiagnosticSeverity::Warning,
        "information" => DiagnosticSeverity::Information,
        "hint" => DiagnosticSeverity::Hint,
        _ => unreachable!("configuration validation rejects unsupported severities"),
    }
}

/// Returns the stable lower-case diagnostic severity name.
const fn severity_name(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Information => "information",
        DiagnosticSeverity::Hint => "hint",
    }
}

impl FailOn {
    /// Tests whether one diagnostic meets this failure threshold.
    const fn matches(self, severity: DiagnosticSeverity) -> bool {
        match self {
            Self::Never => false,
            Self::Error => matches!(severity, DiagnosticSeverity::Error),
            Self::Warning => matches!(
                severity,
                DiagnosticSeverity::Error | DiagnosticSeverity::Warning
            ),
            Self::Information => !matches!(severity, DiagnosticSeverity::Hint),
            Self::Hint => true,
        }
    }
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    trials: usize,
    data_revision: String,
    grammar_version: String,
    parser_abi: usize,
    documents: usize,
    definitions: usize,
    packaged_thoth_sources: usize,
    packaged_thoth_bytes: usize,
    packaged_thoth_packages: usize,
    packaged_stats_declarations: usize,
    cold: MillisecondDistribution,
    warm: MillisecondDistribution,
    warm_cache_hit_rate: f64,
    peak_rss_bytes: u64,
    cache_files: u64,
    cache_bytes: u64,
    navigation: NanosecondDistribution,
}

#[derive(Debug, Serialize)]
struct MillisecondDistribution {
    p50: u128,
    p95: u128,
}

#[derive(Debug, Serialize)]
struct NanosecondDistribution {
    p50: u128,
    p95: u128,
}

/// Runs reproducible cold and warm trials against the same module composition.
fn run_benchmark(options: BenchmarkOptions, cache: CacheStore) -> Result<BenchmarkReport, Error> {
    if options.trials == 0 {
        return Err(Error::Config("benchmark trials must be positive".into()));
    }
    let game_data = fs::canonicalize(&options.game_data)?;
    let workspace_root = fs::canonicalize(&options.workspace_root)?;
    let modules = benchmark_modules(&options, &game_data, &workspace_root)?;
    let mut cold = Vec::with_capacity(options.trials);
    let mut warm = Vec::with_capacity(options.trials);
    let mut peak_rss_bytes = 0;
    let mut final_workspace = None;
    let mut warm_hits = 0;
    let mut warm_misses = 0;

    for _ in 0..options.trials {
        cache.clear()?;
        let started = Instant::now();
        let (workspace, _) =
            build_benchmark_workspace(&cache, &game_data, &modules, &options.language)?;
        cold.push(started.elapsed().as_millis());
        final_workspace = Some(workspace);
        peak_rss_bytes = peak_rss_bytes.max(resident_memory_bytes());
    }
    for _ in 0..options.trials {
        let started = Instant::now();
        let (workspace, stats) =
            build_benchmark_workspace(&cache, &game_data, &modules, &options.language)?;
        warm.push(started.elapsed().as_millis());
        warm_hits += stats.hits;
        warm_misses += stats.misses;
        final_workspace = Some(workspace);
        peak_rss_bytes = peak_rss_bytes.max(resident_memory_bytes());
    }

    let workspace = final_workspace.expect("positive benchmark trial count");
    let (cache_files, cache_bytes) = cache.info()?;
    let navigation = navigation_samples(&workspace)?;
    let documents = workspace.layers.iter().map(|layer| layer.files.len()).sum();
    let definitions = workspace
        .layers
        .iter()
        .map(|layer| layer.definitions.len())
        .sum();
    let packaged_thoth = workspace.packaged_thoth();
    let packaged_thoth_sources = packaged_thoth.len();
    let packaged_thoth_bytes = packaged_thoth
        .sources()
        .map(|source| source.bytes().len())
        .sum();
    let packaged_thoth_packages = packaged_thoth
        .sources()
        .map(|source| source.package())
        .collect::<HashSet<_>>()
        .len();
    let packaged_stats_declarations = workspace.packaged_stats_count();
    let total = warm_hits + warm_misses;
    Ok(BenchmarkReport {
        trials: options.trials,
        data_revision: workspace.schema.digest()?.to_hex().to_string(),
        grammar_version: tree_sitter_bg3::GRAMMAR_VERSION.into(),
        parser_abi: tree_sitter::Language::from(tree_sitter_bg3::BG3_STATS_LANGUAGE).abi_version(),
        documents,
        definitions,
        packaged_thoth_sources,
        packaged_thoth_bytes,
        packaged_thoth_packages,
        packaged_stats_declarations,
        cold: MillisecondDistribution {
            p50: percentile(&mut cold, 50),
            p95: percentile(&mut cold, 95),
        },
        warm: MillisecondDistribution {
            p50: percentile(&mut warm, 50),
            p95: percentile(&mut warm, 95),
        },
        warm_cache_hit_rate: if total == 0 {
            1.0
        } else {
            warm_hits as f64 / total as f64
        },
        peak_rss_bytes,
        cache_files,
        cache_bytes,
        navigation: NanosecondDistribution {
            p50: percentile(&mut navigation.clone(), 50),
            p95: percentile(&mut navigation.clone(), 95),
        },
    })
}

/// Resolves and validates benchmark modules in ascending precedence.
fn benchmark_modules(
    options: &BenchmarkOptions,
    game_data: &std::path::Path,
    workspace_root: &std::path::Path,
) -> Result<Vec<ModuleSpec>, Error> {
    let mut names = HashSet::new();
    let mut modules = Vec::new();
    for name in &options.base_modules {
        if !names.insert(name.clone()) {
            return Err(Error::Config(format!("duplicate module: {name}")));
        }
        modules.push(ModuleSpec {
            name: name.clone(),
            root: fs::canonicalize(game_data.join("Editor/Mods").join(name))?,
            role: ModuleRole::Base,
        });
    }
    for dependency in &options.dependencies {
        if !names.insert(dependency.name.clone()) {
            return Err(Error::Config(format!(
                "duplicate module: {}",
                dependency.name
            )));
        }
        let path = if dependency.path.is_absolute() {
            dependency.path.clone()
        } else {
            workspace_root.join(&dependency.path)
        };
        modules.push(ModuleSpec {
            name: dependency.name.clone(),
            root: fs::canonicalize(path)?,
            role: ModuleRole::Dependency,
        });
    }
    if !names.insert(options.project_name.clone()) {
        return Err(Error::Config(format!(
            "duplicate module: {}",
            options.project_name
        )));
    }
    modules.push(ModuleSpec {
        name: options.project_name.clone(),
        root: workspace_root.to_owned(),
        role: ModuleRole::Project,
    });
    Ok(modules)
}

/// Builds one immutable workspace and returns its aggregate cache counters.
fn build_benchmark_workspace(
    cache: &CacheStore,
    game_data: &std::path::Path,
    modules: &[ModuleSpec],
    language: &str,
) -> Result<(WorkspaceSnapshot, CacheStats), Error> {
    // Base localization does not depend on schemas or module records. Load it
    // concurrently so this benchmark matches the LSP coordinator's startup.
    let ((workspace, mut stats), localization) = std::thread::scope(|scope| {
        let localization = scope.spawn(|| cache.load_base_localization(game_data, language));
        let workspace = build_workspace(cache, game_data, modules, language, 200, 200, &[]);
        let localization = localization
            .join()
            .map_err(|_| Error::Index("base-localization benchmark task panicked".into()))?;
        Ok::<_, Error>((workspace?, localization?))
    })?;
    let workspace = if let Some((catalog, hit)) = localization {
        if hit {
            stats.hits += 1;
        } else {
            stats.misses += 1;
        }
        workspace.with_base_localization(Arc::new(catalog))
    } else {
        workspace
    };
    Ok((workspace, stats))
}

/// Builds one immutable workspace for non-LSP commands.
#[allow(clippy::too_many_arguments)]
fn build_workspace(
    cache: &CacheStore,
    game_data: &std::path::Path,
    modules: &[ModuleSpec],
    language: &str,
    max_workspace_symbols: usize,
    max_completion_items: usize,
    incomplete_kinds: &[&str],
) -> Result<(WorkspaceSnapshot, CacheStats), Error> {
    let (schema, _) = cache.load_schema(game_data)?;
    let schema = Arc::new(schema);
    let mut layers = Vec::new();
    let mut totals = CacheStats::default();
    for (index, module) in modules.iter().enumerate() {
        let sources = discover_module(
            module,
            game_data,
            language,
            index == 0 && module.role == ModuleRole::Base,
        )?;
        let (files, stats) = cache.build_module(module, &sources, &schema, language)?;
        totals.hits += stats.hits;
        totals.misses += stats.misses;
        layers.push(Arc::new(ModuleIndex::new(module.clone(), files)));
    }
    let base_modules: Vec<_> = modules
        .iter()
        .filter(|module| module.role == ModuleRole::Base)
        .map(|module| module.name.clone())
        .collect();
    let package_candidates = packaged_thoth_package_candidates(game_data, &base_modules)?;
    let (packaged_thoth, catalog_hit) =
        cache.load_packaged_thoth(&base_modules, &package_candidates, || {
            read_packaged_thoth_catalog(game_data, &base_modules)
        })?;
    if catalog_hit {
        totals.hits += 1;
    } else {
        totals.misses += 1;
    }
    let (packaged_thoth_facts, facts_hit) = cache.load_packaged_thoth_facts(
        &packaged_thoth,
        THOTH_FACTS_EXTRACTOR_VERSION,
        |source| parse_thoth_file(source.text()),
    )?;
    if facts_hit {
        totals.hits += 1;
    } else {
        totals.misses += 1;
    }
    let (packaged_stats_catalog, stats_hit) =
        cache.load_packaged_stats(&base_modules, &package_candidates, || {
            read_packaged_stats_catalog_from_packages(
                &package_candidates,
                &base_modules,
                &schema,
                language,
            )
        })?;
    if stats_hit {
        totals.hits += 1;
    } else {
        totals.misses += 1;
    }
    let packaged_osiris_catalog = read_packaged_osiris_catalog(game_data, &base_modules)?;
    let (packaged_osiris_facts, osiris_hit) = cache.load_packaged_thoth_facts(
        &packaged_osiris_catalog,
        OSIRIS_FACTS_EXTRACTOR_VERSION,
        parse_osiris_goal_source,
    )?;
    if osiris_hit {
        totals.hits += 1;
    } else {
        totals.misses += 1;
    }
    let packaged_osiris = PackagedOsirisIndex::from_catalog_and_facts(
        &packaged_osiris_catalog,
        &packaged_osiris_facts,
    );
    Ok((
        WorkspaceSnapshot::new(
            schema,
            layers,
            1,
            max_workspace_symbols,
            max_completion_items,
        )
        .with_packaged_thoth(Arc::new(packaged_thoth))
        .with_packaged_thoth_facts(Arc::new(packaged_thoth_facts))
        .with_packaged_osiris(Arc::new(packaged_osiris))
        .with_packaged_stats(Arc::new(packaged_stats_catalog))
        .with_incomplete_kinds(incomplete_kinds.iter().copied()),
        totals,
    ))
}

/// Measures exact semantic lookup latency against one indexed declaration.
fn navigation_samples(workspace: &WorkspaceSnapshot) -> Result<Vec<u128>, Error> {
    let definition = workspace
        .layers
        .iter()
        .rev()
        .find_map(|layer| layer.definitions.first())
        .ok_or_else(|| Error::Index("benchmark indexed no definitions".into()))?;
    let target = definition_target(definition.definition());
    let overlays = OverlaySet::default();
    let mut samples = Vec::with_capacity(1_000);
    for _ in 0..1_000 {
        let started = Instant::now();
        std::hint::black_box(workspace.resolve(&target, &overlays));
        samples.push(started.elapsed().as_nanos());
    }
    Ok(samples)
}

/// Returns an observed resident-set size in bytes on macOS and Linux.
fn resident_memory_bytes() -> u64 {
    let output = ProcessCommand::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output();
    output
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
        .saturating_mul(1024)
}

/// Returns a nearest-rank percentile from one non-empty measurement set.
fn percentile(values: &mut [u128], percentile: usize) -> u128 {
    values.sort_unstable();
    let index = (values.len() - 1) * percentile / 100;
    values[index]
}
