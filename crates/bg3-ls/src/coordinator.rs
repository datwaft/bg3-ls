use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwapOption;
use bg3_ide::WorkspaceSnapshot;
use bg3_index::{
    CacheStats, CacheStore, LocalizationCatalog, ModuleIndex, ModuleRole, PackagedThothCatalog,
    THOTH_FUNCTION_KIND, TooltipCatalog, base_tooltip_package_path, discover_module,
    module_watch_roots, packaged_thoth_package_candidates, read_packaged_thoth_catalog,
};
use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{Mutex, mpsc, watch};
use tower_lsp_server::Client;
use tower_lsp_server::ls_types::{MessageType, ProgressToken, request};

use crate::Error;
use crate::config::ResolvedConfig;

/// Observable lifecycle state for initial requests and status reporting.
#[derive(Clone, Debug)]
pub enum BuildState {
    Idle,
    Building,
    Ready(IndexInfo),
    Failed(String),
}

/// Counts and timing data for the active immutable index generation.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct IndexInfo {
    pub generation: u64,
    pub modules: usize,
    pub documents: usize,
    pub definitions: usize,
    pub references: usize,
    pub schemas: usize,
    pub enumerations: usize,
    pub resources: usize,
    pub localizations: usize,
    pub tooltips: usize,
    pub packaged_thoth_sources: usize,
    pub functions: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub elapsed_milliseconds: u128,
}

/// Serializes rebuild decisions and publishes complete immutable snapshots.
pub struct Coordinator {
    pub snapshot: ArcSwapOption<WorkspaceSnapshot>,
    state_tx: watch::Sender<BuildState>,
    build_guard: Mutex<()>,
    generation: AtomicU64,
    watcher: Mutex<Option<RecommendedWatcher>>,
    cache_dir: Option<PathBuf>,
}

impl Coordinator {
    /// Creates an idle coordinator with no published workspace.
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        let (state_tx, _) = watch::channel(BuildState::Idle);
        Self {
            snapshot: ArcSwapOption::empty(),
            state_tx,
            build_guard: Mutex::new(()),
            generation: AtomicU64::new(0),
            watcher: Mutex::new(None),
            cache_dir,
        }
    }

    /// Returns a receiver that tracks build readiness and failures.
    pub fn subscribe(&self) -> watch::Receiver<BuildState> {
        self.state_tx.subscribe()
    }

    /// Waits for the first complete snapshot without blocking the async runtime.
    pub async fn wait_snapshot(&self) -> Result<Arc<WorkspaceSnapshot>, Error> {
        if let Some(snapshot) = self.snapshot.load_full() {
            return Ok(snapshot);
        }
        let mut receiver = self.subscribe();
        loop {
            match receiver.borrow().clone() {
                BuildState::Failed(message) => return Err(Error::Index(message)),
                BuildState::Ready(_) => {
                    if let Some(snapshot) = self.snapshot.load_full() {
                        return Ok(snapshot);
                    }
                }
                BuildState::Idle | BuildState::Building => {}
            }
            receiver
                .changed()
                .await
                .map_err(|_| Error::Index("index coordinator stopped".into()))?;
        }
    }

    /// Returns status information for the current or most recent build.
    pub fn state(&self) -> BuildState {
        self.state_tx.borrow().clone()
    }

    /// Builds and atomically publishes the configured workspace.
    pub async fn rebuild(&self, config: Arc<ResolvedConfig>, client: &Client) -> bool {
        self.rebuild_scoped(config, client, None).await
    }

    /// Rebuilds only configured modules whose watched source paths changed.
    async fn rebuild_affected(
        &self,
        config: Arc<ResolvedConfig>,
        client: &Client,
        affected: HashSet<String>,
    ) -> bool {
        self.rebuild_scoped(config, client, Some(affected)).await
    }

    /// Serializes a full or scoped build and keeps the last snapshot queryable.
    async fn rebuild_scoped(
        &self,
        config: Arc<ResolvedConfig>,
        client: &Client,
        affected: Option<HashSet<String>>,
    ) -> bool {
        let _guard = self.build_guard.lock().await;
        self.state_tx.send_replace(BuildState::Building);
        let token = ProgressToken::String(format!(
            "bg3-index-{}",
            self.generation.load(Ordering::Relaxed) + 1
        ));
        let progress = if client
            .send_request::<request::WorkDoneProgressCreate>(
                tower_lsp_server::ls_types::WorkDoneProgressCreateParams {
                    token: token.clone(),
                },
            )
            .await
            .is_ok()
        {
            Some(
                client
                    .progress(token, "Indexing BG3 data")
                    .with_message("Loading schema metadata")
                    .with_percentage(0)
                    .begin()
                    .await,
            )
        } else {
            None
        };
        let started = std::time::Instant::now();
        let result = self
            .build(config.clone(), client, progress.as_ref(), affected)
            .await;
        match result {
            Ok((snapshot, mut info)) => {
                info.elapsed_milliseconds = started.elapsed().as_millis();
                self.snapshot.store(Some(Arc::new(snapshot)));
                self.state_tx.send_replace(BuildState::Ready(info.clone()));
                if let Some(progress) = progress {
                    progress.finish_with_message("BG3 index is ready").await;
                }
                client
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "BG3 index generation {} is ready: {} files, {} definitions, {} ms",
                            info.generation,
                            info.documents,
                            info.definitions,
                            info.elapsed_milliseconds
                        ),
                    )
                    .await;
                let cache_dir = self.cache_dir.clone();
                let gc_client = client.clone();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        let cache = if let Some(path) = cache_dir {
                            CacheStore::new(path)?
                        } else {
                            CacheStore::xdg()?
                        };
                        cache.garbage_collect()
                    })
                    .await;
                    if let Err(error) = result {
                        gc_client
                            .log_message(
                                MessageType::WARNING,
                                format!("BG3 cache cleanup task failed: {error}"),
                            )
                            .await;
                    } else if let Ok(Err(error)) = result {
                        gc_client
                            .log_message(
                                MessageType::WARNING,
                                format!("BG3 cache cleanup failed: {error}"),
                            )
                            .await;
                    }
                });
            }
            Err(error) => {
                let message = error.to_string();
                self.state_tx
                    .send_replace(BuildState::Failed(message.clone()));
                if let Some(progress) = progress {
                    progress.finish_with_message("BG3 indexing failed").await;
                }
                client.show_message(MessageType::ERROR, message).await;
            }
        }
        true
    }

    /// Builds schemas and module layers while reporting coarse stable phases.
    async fn build(
        &self,
        config: Arc<ResolvedConfig>,
        client: &Client,
        progress: Option<
            &tower_lsp_server::OngoingProgress<
                tower_lsp_server::Bounded,
                tower_lsp_server::NotCancellable,
            >,
        >,
        affected: Option<HashSet<String>>,
    ) -> Result<(WorkspaceSnapshot, IndexInfo), Error> {
        let cache = if let Some(path) = &self.cache_dir {
            CacheStore::new(path.clone())?
        } else {
            CacheStore::xdg()?
        };
        let mut cache_stats = CacheStats::default();
        let previous = self.snapshot.load_full();
        // Package decoding is independent from schema and module parsing. Run
        // it in parallel so packed tooltip text does not delay warm startup.
        let localization_task = if affected.is_none() {
            let localization_cache = cache.clone();
            let game_data = config.game_data.clone();
            let language = config.language.clone();
            Some(tokio::task::spawn_blocking(move || {
                localization_cache.load_base_localization(&game_data, &language)
            }))
        } else {
            None
        };
        let tooltip_task = if affected.is_none() {
            let tooltip_cache = cache.clone();
            let game_data = config.game_data.clone();
            Some(tokio::task::spawn_blocking(move || {
                tooltip_cache.load_base_tooltips(&game_data)
            }))
        } else {
            None
        };
        let thoth_task = {
            let thoth_cache = cache.clone();
            let game_data = config.game_data.clone();
            let base_modules: Vec<_> = config
                .modules
                .iter()
                .filter(|module| module.role == ModuleRole::Base)
                .map(|module| module.name.clone())
                .collect();
            tokio::task::spawn_blocking(move || {
                let candidates = packaged_thoth_package_candidates(&game_data, &base_modules)?;
                thoth_cache.load_packaged_thoth(&base_modules, &candidates, || {
                    read_packaged_thoth_catalog(&game_data, &base_modules)
                })
            })
        };
        let schema = if affected.is_some() {
            previous
                .as_ref()
                .map(|workspace| Arc::clone(&workspace.schema))
                .ok_or_else(|| Error::Index("a scoped build has no previous snapshot".into()))?
        } else {
            let schema_config = Arc::clone(&config);
            let schema_cache = cache.clone();
            let (schema, _) = tokio::task::spawn_blocking(move || {
                schema_cache.load_schema(&schema_config.game_data)
            })
            .await
            .map_err(|error| Error::Index(format!("schema task failed: {error}")))??;
            Arc::new(schema)
        };
        if let Some(progress) = progress {
            progress
                .report_with_message("Loaded schema metadata", 10)
                .await;
        }

        let base_localization = if affected.is_some() {
            previous
                .as_ref()
                .map(|workspace| workspace.base_localization())
                .ok_or_else(|| Error::Index("a scoped build has no previous snapshot".into()))?
        } else {
            if let Some(progress) = progress {
                progress
                    .report_with_message("Finishing base localization", 15)
                    .await;
            }
            match localization_task
                .expect("a full build starts a localization task")
                .await
            {
                Ok(Ok(Some((catalog, hit)))) => {
                    if hit {
                        cache_stats.hits += 1;
                    } else {
                        cache_stats.misses += 1;
                    }
                    Arc::new(catalog)
                }
                Ok(Ok(None)) => Arc::new(LocalizationCatalog::new(config.language.clone())),
                Ok(Err(error)) => {
                    client
                        .log_message(
                            MessageType::WARNING,
                            format!(
                                "BG3 base localization is unavailable; hover previews will use loose text only: {error}"
                            ),
                        )
                        .await;
                    Arc::new(LocalizationCatalog::new(config.language.clone()))
                }
                Err(error) => {
                    client
                        .log_message(
                            MessageType::WARNING,
                            format!(
                                "BG3 base-localization task failed; hover previews will use loose text only: {error}"
                            ),
                        )
                        .await;
                    Arc::new(LocalizationCatalog::new(config.language.clone()))
                }
            }
        };
        let tooltips = if affected.is_some() {
            previous
                .as_ref()
                .map(|workspace| workspace.tooltips())
                .ok_or_else(|| Error::Index("a scoped build has no previous snapshot".into()))?
        } else {
            match tooltip_task
                .expect("a full build starts a tooltip task")
                .await
            {
                Ok(Ok(Some((catalog, hit)))) => {
                    if hit {
                        cache_stats.hits += 1;
                    } else {
                        cache_stats.misses += 1;
                    }
                    Arc::new(catalog)
                }
                Ok(Ok(None)) => Arc::new(TooltipCatalog::default()),
                Ok(Err(error)) => {
                    client
                        .log_message(
                            MessageType::WARNING,
                            format!(
                                "BG3 tooltip glossary is unavailable; localization tag hover will use typed loose resources only: {error}"
                            ),
                        )
                        .await;
                    Arc::new(TooltipCatalog::default())
                }
                Err(error) => {
                    client
                        .log_message(
                            MessageType::WARNING,
                            format!(
                                "BG3 tooltip-glossary task failed; localization tag hover will use typed loose resources only: {error}"
                            ),
                        )
                        .await;
                    Arc::new(TooltipCatalog::default())
                }
            }
        };
        let packaged_thoth = match thoth_task.await {
            Ok(Ok((catalog, hit))) => {
                if hit {
                    cache_stats.hits += 1;
                } else {
                    cache_stats.misses += 1;
                }
                Arc::new(catalog)
            }
            Ok(Err(error)) => {
                client
                    .log_message(
                        MessageType::WARNING,
                        format!(
                            "BG3 packaged Thoth sources are unavailable; keeping the previous catalog when possible: {error}"
                        ),
                    )
                    .await;
                previous
                    .as_ref()
                    .map(|workspace| workspace.packaged_thoth())
                    .unwrap_or_else(|| Arc::new(PackagedThothCatalog::default()))
            }
            Err(error) => {
                client
                    .log_message(
                        MessageType::WARNING,
                        format!(
                            "BG3 packaged-Thoth task failed; keeping the previous catalog when possible: {error}"
                        ),
                    )
                    .await;
                previous
                    .as_ref()
                    .map(|workspace| workspace.packaged_thoth())
                    .unwrap_or_else(|| Arc::new(PackagedThothCatalog::default()))
            }
        };

        let mut layers = Vec::new();
        let module_count = config.modules.len();
        for (index, module) in config.modules.iter().cloned().enumerate() {
            if affected
                .as_ref()
                .is_some_and(|affected| !affected.contains(&module.name))
                && let Some(layer) = previous.as_ref().and_then(|workspace| {
                    workspace.layers.iter().find(|layer| layer.spec == module)
                })
            {
                layers.push(Arc::clone(layer));
                if let Some(progress) = progress {
                    let percentage =
                        10 + u32::try_from(80 * (index + 1) / module_count.max(1)).unwrap_or(80);
                    progress
                        .report_with_message(
                            format!("Reused unchanged module {}", config.modules[index].name),
                            percentage,
                        )
                        .await;
                }
                continue;
            }
            if let Some(progress) = progress {
                let percentage = 10 + u32::try_from(80 * index / module_count.max(1)).unwrap_or(10);
                progress
                    .report_with_message(
                        format!("Discovering {} and loading its cache", module.name),
                        percentage,
                    )
                    .await;
            }
            let game_data = config.game_data.clone();
            let language = config.language.clone();
            let schema_for_module = Arc::clone(&schema);
            let cache_for_module = cache.clone();
            let include_localization = index == 0 && module.role == ModuleRole::Base;
            let result = tokio::task::spawn_blocking(move || {
                let files = discover_module(&module, &game_data, &language, include_localization)?;
                let (parsed, stats) = cache_for_module.build_module(
                    &module,
                    &files,
                    &schema_for_module,
                    &language,
                )?;
                Ok::<_, Error>((Arc::new(ModuleIndex::new(module, parsed)), stats))
            })
            .await
            .map_err(|error| Error::Index(format!("module task failed: {error}")))?;
            let (layer, stats) = match result {
                Ok(result) => result,
                Err(error) => {
                    if let Some(layer) = previous.as_ref().and_then(|workspace| {
                        workspace
                            .layers
                            .iter()
                            .find(|layer| layer.spec.name == config.modules[index].name)
                    }) {
                        client
                            .log_message(
                                MessageType::WARNING,
                                format!(
                                    "BG3 module `{}` rebuild failed; keeping generation {}: {error}",
                                    config.modules[index].name,
                                    previous.as_ref().map_or(0, |workspace| workspace.generation)
                                ),
                            )
                            .await;
                        layers.push(Arc::clone(layer));
                        continue;
                    }
                    return Err(error);
                }
            };
            cache_stats.hits += stats.hits;
            cache_stats.misses += stats.misses;
            layers.push(layer);
            if let Some(progress) = progress {
                let percentage =
                    10 + u32::try_from(80 * (index + 1) / module_count.max(1)).unwrap_or(80);
                progress
                    .report_with_message(
                        format!(
                            "Parsed and built module {} ({}/{})",
                            config.modules[index].name,
                            index + 1,
                            module_count
                        ),
                        percentage,
                    )
                    .await;
            }
        }

        if let Some(progress) = progress {
            progress
                .report_with_message("Publishing the complete workspace", 95)
                .await;
        }

        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let incomplete_kinds = config.incomplete_kinds();
        let workspace = WorkspaceSnapshot::new(
            schema,
            layers,
            generation,
            config.max_workspace_symbols,
            config.max_completion_items,
        )
        .with_base_localization(base_localization)
        .with_packaged_thoth(packaged_thoth)
        .with_tooltips(tooltips)
        .with_incomplete_kinds(incomplete_kinds);
        let info = index_info(&workspace, cache_stats);
        Ok((workspace, info))
    }

    /// Starts one recursive watcher and coalesces external changes before rebuilding.
    pub async fn start_watcher(
        self: &Arc<Self>,
        config: Arc<ResolvedConfig>,
        client: Client,
    ) -> Result<(), Error> {
        let mut active = self.watcher.lock().await;
        if active.is_some() {
            return Ok(());
        }
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let _ = sender.send(event);
            },
            NotifyConfig::default(),
        )?;
        let mut watched = HashSet::new();
        for module in &config.modules {
            for root in module_watch_roots(module, &config.game_data) {
                if watched.insert(root.clone()) {
                    watcher.watch(&root, RecursiveMode::Recursive)?;
                }
            }
        }
        if watched.insert(config.game_data.clone()) {
            watcher.watch(&config.game_data, RecursiveMode::NonRecursive)?;
        }
        for relative in ["Editor/Config/Stats", "Editor/Config/UuidObjects"] {
            watcher.watch(&config.game_data.join(relative), RecursiveMode::Recursive)?;
        }
        let localization_root = config.game_data.join("Localization");
        if localization_root.is_dir() {
            watcher.watch(&localization_root, RecursiveMode::Recursive)?;
        }
        let tooltip_package = base_tooltip_package_path(&config.game_data);
        if tooltip_package.is_file() {
            watcher.watch(&tooltip_package, RecursiveMode::NonRecursive)?;
        }
        *active = Some(watcher);
        drop(active);

        let coordinator = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                let mut paths = match event {
                    Ok(event) => event.paths,
                    Err(error) => {
                        client
                            .log_message(
                                MessageType::WARNING,
                                format!("BG3 watcher error: {error}"),
                            )
                            .await;
                        continue;
                    }
                };
                tokio::time::sleep(Duration::from_millis(250)).await;
                while let Ok(event) = receiver.try_recv() {
                    match event {
                        Ok(event) => paths.extend(event.paths),
                        Err(error) => {
                            client
                                .log_message(
                                    MessageType::WARNING,
                                    format!("BG3 watcher error: {error}"),
                                )
                                .await;
                        }
                    }
                }

                if paths
                    .iter()
                    .any(|path| is_packaged_thoth_package(path, &config.game_data, &config.modules))
                {
                    coordinator
                        .rebuild_affected(Arc::clone(&config), &client, HashSet::new())
                        .await;
                    continue;
                }
                let full_rebuild_required = paths.iter().any(|path| {
                    path.starts_with(config.game_data.join("Editor/Config/Stats"))
                        || path.starts_with(config.game_data.join("Editor/Config/UuidObjects"))
                        || path.starts_with(config.game_data.join("Localization"))
                        || path == &base_tooltip_package_path(&config.game_data)
                });
                if full_rebuild_required {
                    coordinator.rebuild(Arc::clone(&config), &client).await;
                    continue;
                }
                let affected: HashSet<_> = config
                    .modules
                    .iter()
                    .filter(|module| {
                        module_watch_roots(module, &config.game_data)
                            .iter()
                            .any(|root| paths.iter().any(|path| path.starts_with(root)))
                    })
                    .map(|module| module.name.clone())
                    .collect();
                if !affected.is_empty() {
                    coordinator
                        .rebuild_affected(Arc::clone(&config), &client, affected)
                        .await;
                }
            }
        });
        Ok(())
    }
}

/// Tests whether a top-level package can contribute configured base Thoth sources.
fn is_packaged_thoth_package(
    path: &Path,
    game_data: &Path,
    modules: &[bg3_index::ModuleSpec],
) -> bool {
    if path.parent() != Some(game_data) {
        return false;
    }
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if file_name.starts_with("Patch") && file_name.ends_with(".pak") {
        return true;
    }
    modules.iter().any(|module| {
        module.role == ModuleRole::Base && file_name == format!("{}.pak", module.name)
    })
}

/// Computes user-facing counts from one published workspace generation.
fn index_info(workspace: &WorkspaceSnapshot, cache: CacheStats) -> IndexInfo {
    let mut info = IndexInfo {
        generation: workspace.generation,
        modules: workspace.layers.len(),
        schemas: workspace.schema.by_id.len(),
        enumerations: workspace.schema.enumerations.len(),
        localizations: workspace.base_localization_count(),
        tooltips: workspace.tooltip_count(),
        packaged_thoth_sources: workspace.packaged_thoth_count(),
        cache_hits: cache.hits,
        cache_misses: cache.misses,
        ..IndexInfo::default()
    };
    for layer in &workspace.layers {
        info.documents += layer.files.len();
        info.definitions += layer.definitions.len();
        info.references += layer.references.len();
        info.functions += layer.functions.len();
        for definition in &layer.definitions {
            if definition.definition().kind == THOTH_FUNCTION_KIND {
                info.functions += 1;
            }
            if definition.definition().kind == "Localization" {
                info.localizations += 1;
            } else if definition.definition().uuid.is_some() {
                info.resources += 1;
            }
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_thoth_events_accept_selected_top_level_packages_only() {
        let game_data = Path::new("/game/Data");
        let modules = vec![
            bg3_index::ModuleSpec {
                name: "Shared".into(),
                root: game_data.join("Editor/Mods/Shared"),
                role: ModuleRole::Base,
            },
            bg3_index::ModuleSpec {
                name: "Dependency".into(),
                root: PathBuf::from("/mods/Dependency"),
                role: ModuleRole::Dependency,
            },
        ];

        assert!(is_packaged_thoth_package(
            &game_data.join("Shared.pak"),
            game_data,
            &modules
        ));
        assert!(is_packaged_thoth_package(
            &game_data.join("Patch3.pak"),
            game_data,
            &modules
        ));
        assert!(is_packaged_thoth_package(
            &game_data.join("Patch.pak"),
            game_data,
            &modules
        ));

        assert!(!is_packaged_thoth_package(
            &game_data.join("Dependency.pak"),
            game_data,
            &modules
        ));
        assert!(!is_packaged_thoth_package(
            &game_data.join("Other.pak"),
            game_data,
            &modules
        ));
        assert!(!is_packaged_thoth_package(
            &game_data.join("nested/Patch4.pak"),
            game_data,
            &modules
        ));
        assert!(!is_packaged_thoth_package(
            &game_data.join("Shared.PAK"),
            game_data,
            &modules
        ));
    }
}
