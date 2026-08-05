mod config;
mod coordinator;
mod server;

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use bg3_ide::{OverlaySet, WorkspaceSnapshot, definition_target};
use bg3_index::{CacheStats, CacheStore, ModuleIndex, ModuleRole, ModuleSpec, discover_module};
use clap::{Parser, Subcommand};
use serde::Serialize;
use thiserror::Error;
use tower_lsp_server::{LspService, Server};
use tracing_subscriber::EnvFilter;

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
}

/// Runs cache maintenance or serves LSP messages over stdio by default.
#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("BG3_LS_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        return run_command(command, cli.cache_dir);
    }

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let cache_dir = cli.cache_dir;
    let (service, socket) = LspService::new(move |client| Backend::new(client, cache_dir.clone()));
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(())
}

/// Executes one non-LSP command and writes only human-readable stdout.
fn run_command(command: Command, cache_dir: Option<PathBuf>) -> Result<(), Error> {
    match command {
        Command::Cache {
            command: CacheCommand::Path,
        } => println!("{}", open_cache(cache_dir)?.root().display()),
        Command::Cache {
            command: CacheCommand::Info,
        } => {
            let cache = open_cache(cache_dir)?;
            let (files, bytes) = cache.info()?;
            println!("{} files, {} bytes", files, bytes);
        }
        Command::Cache {
            command: CacheCommand::Clear,
        } => {
            let cache = open_cache(cache_dir)?;
            cache.clear()?;
            println!("cleared {}", cache.root().display());
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
        }
    }
    Ok(())
}

/// Opens the explicit cache override or the normal XDG cache.
fn open_cache(cache_dir: Option<PathBuf>) -> Result<CacheStore, Error> {
    Ok(if let Some(path) = cache_dir {
        CacheStore::new(path)?
    } else {
        CacheStore::xdg()?
    })
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    trials: usize,
    data_revision: String,
    grammar_version: String,
    parser_abi: usize,
    documents: usize,
    definitions: usize,
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
    let total = warm_hits + warm_misses;
    Ok(BenchmarkReport {
        trials: options.trials,
        data_revision: workspace.schema.digest()?.to_hex().to_string(),
        grammar_version: tree_sitter_bg3::GRAMMAR_VERSION.into(),
        parser_abi: tree_sitter::Language::from(tree_sitter_bg3::BG3_STATS_LANGUAGE).abi_version(),
        documents,
        definitions,
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
    Ok((WorkspaceSnapshot::new(schema, layers, 1, 200, 200), totals))
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
