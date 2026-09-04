use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::domain::{ParsedFile, SourceFile};
use crate::localization::{
    LocalizationCatalog, base_localization_package_path, read_localization_package, valid_language,
};
use crate::package::package_fingerprint;
use crate::packaged_stats::PackagedStatsCatalog;
use crate::parser::parse_source;
use crate::schema::SchemaCatalog;
use crate::thoth::PackagedThothCatalog;
use crate::thoth_facts::{CachedThothFacts, PackagedThothFacts, parse_packaged_thoth_facts};
use crate::tooltip::{TooltipCatalog, base_tooltip_package_path, read_base_tooltip_catalog};
use crate::{Error, ModuleSpec};

const CACHE_MAGIC: &[u8; 8] = b"BG3LSIDX";
const CACHE_VERSION: u32 = 4;
const EXTRACTOR_VERSION: &str = "bg3-ls-index-v12";
const LOCALIZATION_EXTRACTOR_VERSION: &str = "bg3-ls-localization-v1";
const TOOLTIP_EXTRACTOR_VERSION: &str = "bg3-ls-tooltips-v1";
const THOTH_EXTRACTOR_VERSION: &str = "bg3-ls-thoth-v3";
const OSIRIS_CATALOG_EXTRACTOR_VERSION: &str = "bg3-ls-osiris-catalog-v1";
const STATS_EXTRACTOR_VERSION: &str = "bg3-ls-stats-v1";
const ABANDONED_OBJECT_AGE: Duration = Duration::from_hours(720);

/// Summary of cache use during one module build.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
}

/// A disposable content-addressed cache for parsed source records.
#[derive(Clone, Debug)]
pub struct CacheStore {
    root: PathBuf,
    pool: Arc<rayon::ThreadPool>,
}

impl CacheStore {
    /// Opens a cache at an explicit path and creates its directories.
    pub fn new(root: PathBuf) -> Result<Self, Error> {
        for child in [
            "objects",
            "manifests",
            "schemas",
            "localizations",
            "tooltips",
            "thoth",
            "osiris",
            "stats",
        ] {
            fs::create_dir_all(root.join(child))?;
        }
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(8);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("bg3-index-{index}"))
            .build()?;
        let store = Self {
            root,
            pool: Arc::new(pool),
        };
        store.remove_temporary_files()?;
        Ok(store)
    }

    /// Uses XDG_CACHE_HOME or the cross-platform XDG fallback under the home directory.
    pub fn xdg() -> Result<Self, Error> {
        let root = if let Some(value) = env::var_os("XDG_CACHE_HOME") {
            PathBuf::from(value).join("bg3-ls")
        } else {
            let home = env::var_os("HOME").ok_or_else(|| {
                Error::Config("HOME is not set and XDG_CACHE_HOME is missing".into())
            })?;
            PathBuf::from(home).join(".cache/bg3-ls")
        };
        Self::new(root)
    }

    /// Returns the active cache root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Loads a content-addressed schema catalog or parses the four required XML catalogs.
    pub fn load_schema(&self, game_data: &Path) -> Result<(SchemaCatalog, bool), Error> {
        let definitions = [
            "Editor/Config/Stats/StatObjectDefinitions.sod",
            "Editor/Config/UuidObjects/TableDefinitions.sod",
        ];
        let enumerations = [
            "Editor/Config/Stats/Enumerations.xml",
            "Editor/Config/UuidObjects/Enumerations.toe",
        ];
        let mut contents = Vec::new();
        let mut hash = blake3::Hasher::new();
        hash.update(EXTRACTOR_VERSION.as_bytes());
        for relative in definitions.into_iter().chain(enumerations) {
            let text = fs::read_to_string(game_data.join(relative))?;
            hash.update(relative.as_bytes());
            hash.update(text.as_bytes());
            contents.push((relative, text));
        }
        let path = self
            .root
            .join("schemas")
            .join(format!("{}.cache", hash.finalize().to_hex()));
        if let Ok(schema) = self.read_envelope(&path) {
            return Ok((schema, true));
        }

        let mut schema = SchemaCatalog::default();
        for (relative, text) in contents {
            if relative.ends_with(".sod") {
                schema.merge_definitions(&text)?;
            } else {
                schema.merge_enumerations(&text)?;
            }
        }
        self.write_envelope(&path, &schema)?;
        Ok((schema, false))
    }

    /// Loads the optional base language package and reuses its decoded catalog.
    pub fn load_base_localization(
        &self,
        game_data: &Path,
        language: &str,
    ) -> Result<Option<(LocalizationCatalog, bool)>, Error> {
        if !valid_language(language) {
            return Err(Error::Localization(format!(
                "the localization language {language:?} is not a safe catalog name"
            )));
        }
        let package = base_localization_package_path(game_data, language);
        if !package.is_file() {
            return Ok(None);
        }
        let metadata = fs::metadata(&package)?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        let mut key = blake3::Hasher::new();
        key.update(package.to_string_lossy().as_bytes());
        key.update(language.as_bytes());
        let cache_path = self
            .root
            .join("localizations")
            .join(format!("{}.cache", key.finalize().to_hex()));
        if let Ok(cached) = self.read_envelope::<CachedLocalization>(&cache_path)
            && cached.size == metadata.len()
            && cached.modified == modified
            && cached.extractor_version == LOCALIZATION_EXTRACTOR_VERSION
        {
            return Ok(Some((cached.catalog, true)));
        }

        let catalog = read_localization_package(&package, language)?;
        self.write_envelope(
            &cache_path,
            &CachedLocalization {
                size: metadata.len(),
                modified,
                extractor_version: LOCALIZATION_EXTRACTOR_VERSION.into(),
                catalog: catalog.clone(),
            },
        )?;
        Ok(Some((catalog, false)))
    }

    /// Loads the optional static game tooltip glossary and reuses its decoded catalog.
    pub fn load_base_tooltips(
        &self,
        game_data: &Path,
    ) -> Result<Option<(TooltipCatalog, bool)>, Error> {
        let package = base_tooltip_package_path(game_data);
        if !package.is_file() {
            return Ok(None);
        }
        let metadata = fs::metadata(&package)?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        let mut key = blake3::Hasher::new();
        key.update(package.to_string_lossy().as_bytes());
        let cache_path = self
            .root
            .join("tooltips")
            .join(format!("{}.cache", key.finalize().to_hex()));
        if let Ok(cached) = self.read_envelope::<CachedTooltips>(&cache_path)
            && cached.size == metadata.len()
            && cached.modified == modified
            && cached.extractor_version == TOOLTIP_EXTRACTOR_VERSION
        {
            return Ok(Some((cached.catalog, true)));
        }

        let Some(catalog) = read_base_tooltip_catalog(game_data)? else {
            return Ok(None);
        };
        self.write_envelope(
            &cache_path,
            &CachedTooltips {
                size: metadata.len(),
                modified,
                extractor_version: TOOLTIP_EXTRACTOR_VERSION.into(),
                catalog: catalog.clone(),
            },
        )?;
        Ok(Some((catalog, false)))
    }

    /// Loads the configured packaged Thoth catalog and reuses its decoded form.
    ///
    /// The caller supplies the complete, deterministic set of package
    /// candidates discovered for the current configuration. The callback is
    /// evaluated only on a cache miss, which keeps package parsing out of the
    /// cache layer while allowing the cache to validate all package inputs.
    pub fn load_packaged_thoth<F>(
        &self,
        base_modules: &[String],
        package_candidates: &[PathBuf],
        load: F,
    ) -> Result<(PackagedThothCatalog, bool), Error>
    where
        F: FnOnce() -> Result<PackagedThothCatalog, Error>,
    {
        self.load_packaged_catalog(
            base_modules,
            package_candidates,
            "thoth",
            THOTH_EXTRACTOR_VERSION,
            load,
        )
    }

    /// Loads the configured packaged Osiris catalog and reuses its decoded form.
    ///
    /// Osiris and Thoth catalogs use the same source representation, but they
    /// must have separate cache identities. Their package candidates overlap,
    /// and sharing a cache path can return a Thoth catalog where Osiris facts
    /// are expected.
    pub fn load_packaged_osiris<F>(
        &self,
        base_modules: &[String],
        package_candidates: &[PathBuf],
        load: F,
    ) -> Result<(PackagedThothCatalog, bool), Error>
    where
        F: FnOnce() -> Result<PackagedThothCatalog, Error>,
    {
        self.load_packaged_catalog(
            base_modules,
            package_candidates,
            "osiris",
            OSIRIS_CATALOG_EXTRACTOR_VERSION,
            load,
        )
    }

    fn load_packaged_catalog<F>(
        &self,
        base_modules: &[String],
        package_candidates: &[PathBuf],
        cache_namespace: &str,
        extractor_version: &str,
        load: F,
    ) -> Result<(PackagedThothCatalog, bool), Error>
    where
        F: FnOnce() -> Result<PackagedThothCatalog, Error>,
    {
        let mut modules = base_modules.to_vec();
        modules.sort();
        modules.dedup();

        let packages = package_candidates
            .iter()
            .map(|path| {
                let metadata = fs::metadata(path)?;
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |value| value.as_nanos());
                Ok(PackageManifest {
                    path: path.clone(),
                    size: metadata.len(),
                    modified,
                    fingerprint: package_fingerprint(path)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut packages = packages;
        packages.sort_by(|left, right| left.path.cmp(&right.path));
        packages.dedup_by(|left, right| left.path == right.path);

        let mut identity = blake3::Hasher::new();
        identity.update(extractor_version.as_bytes());
        for module in &modules {
            identity.update(module.as_bytes());
            identity.update(&[0]);
        }
        for package in &packages {
            identity.update(package.path.to_string_lossy().as_bytes());
            identity.update(&[0]);
        }
        let cache_path = self
            .root
            .join(cache_namespace)
            .join(format!("{}.cache", identity.finalize().to_hex()));

        if let Ok(cached) = self.read_envelope::<CachedPackagedCatalog>(&cache_path)
            && cached.modules == modules
            && cached.packages == packages
            && cached.extractor_version == extractor_version
        {
            return Ok((cached.catalog, true));
        }

        let catalog = load()?;
        self.write_envelope(
            &cache_path,
            &CachedPackagedCatalog {
                modules,
                packages,
                extractor_version: extractor_version.into(),
                catalog: catalog.clone(),
            },
        )?;
        Ok((catalog, false))
    }

    /// Loads the configured packaged Stats catalog and reuses its decoded form.
    ///
    /// The identity mirrors [`CacheStore::load_packaged_thoth`]: configured
    /// module names, every package manifest including its checksum fingerprint,
    /// and an extractor version. The callback runs only on a cache miss.
    pub fn load_packaged_stats<F>(
        &self,
        base_modules: &[String],
        package_candidates: &[PathBuf],
        load: F,
    ) -> Result<(PackagedStatsCatalog, bool), Error>
    where
        F: FnOnce() -> Result<PackagedStatsCatalog, Error>,
    {
        let mut modules = base_modules.to_vec();
        modules.sort();
        modules.dedup();

        let packages = package_candidates
            .iter()
            .map(|path| {
                let metadata = fs::metadata(path)?;
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |value| value.as_nanos());
                Ok(PackageManifest {
                    path: path.clone(),
                    size: metadata.len(),
                    modified,
                    fingerprint: package_fingerprint(path)?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut packages = packages;
        packages.sort_by(|left, right| left.path.cmp(&right.path));
        packages.dedup_by(|left, right| left.path == right.path);

        let mut identity = blake3::Hasher::new();
        identity.update(STATS_EXTRACTOR_VERSION.as_bytes());
        for module in &modules {
            identity.update(module.as_bytes());
            identity.update(&[0]);
        }
        for package in &packages {
            identity.update(package.path.to_string_lossy().as_bytes());
            identity.update(&[0]);
        }
        let cache_path = self
            .root
            .join("stats")
            .join(format!("{}.cache", identity.finalize().to_hex()));

        if let Ok(cached) = self.read_envelope::<CachedPackagedStats>(&cache_path)
            && cached.modules == modules
            && cached.packages == packages
            && cached.extractor_version == STATS_EXTRACTOR_VERSION
        {
            return Ok((cached.catalog, true));
        }

        let catalog = load()?;
        self.write_envelope(
            &cache_path,
            &CachedPackagedStats {
                modules,
                packages,
                extractor_version: STATS_EXTRACTOR_VERSION.into(),
                catalog: catalog.clone(),
            },
        )?;
        Ok((catalog, false))
    }

    /// Loads parsed, source-backed facts for every candidate in a packaged
    /// Thoth catalog.
    ///
    /// The catalog digest and extractor version form the cache identity. The
    /// parser callback runs only after a cache miss, and receives a
    /// `PackagedThothSource` rather than a fabricated filesystem path.
    pub fn load_packaged_thoth_facts<F, Parse>(
        &self,
        catalog: &PackagedThothCatalog,
        extractor_version: &str,
        parse: Parse,
    ) -> Result<(PackagedThothFacts<F>, bool), Error>
    where
        F: CachedThothFacts,
        Parse: Fn(&crate::PackagedThothSource) -> Result<F, Error>,
    {
        self.load_packaged_facts(
            "thoth",
            "facts",
            catalog,
            extractor_version,
            extractor_version,
            parse,
        )
    }

    /// Loads parsed facts for the packaged Osiris catalog.
    ///
    /// Osiris facts share the Thoth cache directory with a distinct filename
    /// prefix and include the reviewed argument-domain catalog plus the
    /// generated engine-catalog provenance in their identity. Changes to those
    /// contracts therefore invalidate only Osiris facts; packaged Thoth facts
    /// keep their existing cache identity.
    pub fn load_packaged_osiris_facts<F, Parse>(
        &self,
        catalog: &PackagedThothCatalog,
        extractor_version: &str,
        parse: Parse,
    ) -> Result<(PackagedThothFacts<F>, bool), Error>
    where
        F: CachedThothFacts,
        Parse: Fn(&crate::PackagedThothSource) -> Result<F, Error>,
    {
        let cache_identity = crate::osiris_facts_cache_identity(extractor_version);
        self.load_packaged_facts(
            "thoth",
            "osiris-facts",
            catalog,
            extractor_version,
            &cache_identity,
            parse,
        )
    }

    fn load_packaged_facts<F, Parse>(
        &self,
        cache_namespace: &str,
        filename_prefix: &str,
        catalog: &PackagedThothCatalog,
        extractor_version: &str,
        cache_identity: &str,
        parse: Parse,
    ) -> Result<(PackagedThothFacts<F>, bool), Error>
    where
        F: CachedThothFacts,
        Parse: Fn(&crate::PackagedThothSource) -> Result<F, Error>,
    {
        let catalog_bytes = postcard::to_stdvec(catalog)?;
        let mut identity = blake3::Hasher::new();
        identity.update(cache_identity.as_bytes());
        identity.update(&catalog_bytes);
        let digest = identity.finalize().to_hex().to_string();
        let cache_path = self
            .root
            .join(cache_namespace)
            .join(format!("{filename_prefix}-{digest}.cache"));

        if let Ok(cached) = self.read_envelope::<CachedPackagedThothFacts<F>>(&cache_path)
            && cached.extractor_version == cache_identity
            && cached.catalog_digest == digest
        {
            return Ok((cached.facts, true));
        }

        let facts = parse_packaged_thoth_facts(catalog, extractor_version, parse)?;
        self.write_envelope(
            &cache_path,
            &CachedPackagedThothFacts {
                extractor_version: cache_identity.to_owned(),
                catalog_digest: digest,
                facts: facts.clone(),
            },
        )?;
        Ok((facts, false))
    }

    /// Parses a module in parallel and reuses unchanged cached file records.
    pub fn build_module(
        &self,
        module: &ModuleSpec,
        sources: &[SourceFile],
        schema: &SchemaCatalog,
        language: &str,
    ) -> Result<(Vec<ParsedFile>, CacheStats), Error> {
        let manifest_path = self.manifest_path(module);
        let old_manifest = self.read_manifest(&manifest_path).unwrap_or_default();
        let schema_digest = schema.digest()?.to_hex().to_string();
        let mut fingerprints = HashMap::new();
        for source in sources {
            fingerprints
                .entry(source.kind)
                .or_insert_with(|| context_fingerprint(source.kind, &schema_digest, language));
        }

        let results: Vec<Result<(ParsedFile, FileManifest, bool), Error>> =
            self.pool.install(|| {
                sources
                    .par_iter()
                    .map(|source| {
                        let metadata = fs::metadata(&source.path)?;
                        let modified = metadata
                            .modified()
                            .ok()
                            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                            .map_or(0, |value| value.as_nanos());
                        let key = source.path.to_string_lossy().into_owned();
                        let fingerprint = fingerprints[&source.kind].clone();
                        if let Some(entry) = old_manifest.files.get(&key)
                            && entry.size == metadata.len()
                            && entry.modified == modified
                            && entry.fingerprint == fingerprint
                            && let Ok(parsed) = self.read_object(&entry.object)
                        {
                            return Ok((parsed, entry.clone(), true));
                        }

                        let text = fs::read_to_string(&source.path)?;
                        let content_hash = blake3::hash(text.as_bytes()).to_hex().to_string();
                        let object = object_key(source, &content_hash, &fingerprint);
                        let parsed = parse_source(source.clone(), &text, schema, language)?;
                        self.write_object(&object, &parsed)?;
                        Ok((
                            parsed,
                            FileManifest {
                                size: metadata.len(),
                                modified,
                                source_kind: source.kind,
                                content_hash,
                                fingerprint,
                                object,
                            },
                            false,
                        ))
                    })
                    .collect()
            });

        let mut parsed = Vec::with_capacity(results.len());
        let mut manifest = ModuleManifest {
            module: module.name.clone(),
            canonical_root: module.root.clone(),
            files: BTreeMap::new(),
        };
        let mut stats = CacheStats::default();
        for (source, result) in sources.iter().zip(results) {
            let (file, entry, hit) = result?;
            manifest
                .files
                .insert(source.path.to_string_lossy().into_owned(), entry);
            parsed.push(file);
            if hit {
                stats.hits += 1;
            } else {
                stats.misses += 1;
            }
        }
        self.write_envelope(&manifest_path, &manifest)?;
        Ok((parsed, stats))
    }

    /// Removes every disposable cache artifact.
    pub fn clear(&self) -> Result<(), Error> {
        if self.root.exists() {
            fs::remove_dir_all(&self.root)?;
        }
        for child in [
            "objects",
            "manifests",
            "schemas",
            "localizations",
            "tooltips",
            "thoth",
            "osiris",
            "stats",
        ] {
            fs::create_dir_all(self.root.join(child))?;
        }
        Ok(())
    }

    /// Returns the total number of cache files and bytes.
    pub fn info(&self) -> Result<(u64, u64), Error> {
        let mut files = 0;
        let mut bytes = 0;
        for entry in walkdir::WalkDir::new(&self.root) {
            let entry = entry?;
            if entry.file_type().is_file() {
                files += 1;
                bytes += entry.metadata()?.len();
            }
        }
        Ok((files, bytes))
    }

    /// Removes unreferenced file objects that have been abandoned for more than 30 days.
    pub fn garbage_collect(&self) -> Result<usize, Error> {
        let mut referenced = std::collections::HashSet::new();
        for entry in walkdir::WalkDir::new(self.root.join("manifests")) {
            let entry = entry?;
            if entry.file_type().is_file()
                && let Ok(manifest) = self.read_manifest(entry.path())
            {
                referenced.extend(manifest.files.into_values().map(|file| file.object));
            }
        }
        let now = SystemTime::now();
        let mut removed = 0;
        for entry in walkdir::WalkDir::new(self.root.join("objects")) {
            let entry = entry?;
            if !entry.file_type().is_file()
                || referenced.contains(&entry.file_name().to_string_lossy().into_owned())
            {
                continue;
            }
            let age = entry
                .metadata()?
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .unwrap_or_default();
            if age >= ABANDONED_OBJECT_AGE {
                fs::remove_file(entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Derives a stable manifest name from module identity and canonical root.
    fn manifest_path(&self, module: &ModuleSpec) -> PathBuf {
        let mut hash = blake3::Hasher::new();
        hash.update(module.name.as_bytes());
        hash.update(module.root.to_string_lossy().as_bytes());
        self.root
            .join("manifests")
            .join(format!("{}.cache", hash.finalize().to_hex()))
    }

    /// Reads a cached file object and validates its envelope.
    fn read_object(&self, object: &str) -> Result<ParsedFile, Error> {
        self.read_envelope(&self.root.join("objects").join(object))
    }

    /// Writes one content-addressed object if another process has not done so.
    fn write_object(&self, object: &str, parsed: &ParsedFile) -> Result<(), Error> {
        let path = self.root.join("objects").join(object);
        if path.is_file() {
            return Ok(());
        }
        self.write_envelope(&path, parsed)
    }

    /// Reads a module manifest and treats cache corruption as a miss.
    fn read_manifest(&self, path: &Path) -> Result<ModuleManifest, Error> {
        self.read_envelope(path)
    }

    /// Removes incomplete atomic-write files from earlier interrupted processes.
    fn remove_temporary_files(&self) -> Result<(), Error> {
        for entry in walkdir::WalkDir::new(&self.root) {
            let entry = entry?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension.to_string_lossy().starts_with("tmp-"))
            {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// Serializes a versioned and checksummed cache envelope atomically.
    fn write_envelope<T: Serialize>(&self, path: &Path, value: &T) -> Result<(), Error> {
        let payload = postcard::to_stdvec(value)?;
        let checksum = blake3::hash(&payload);
        let mut bytes = Vec::with_capacity(8 + 4 + 32 + payload.len());
        bytes.extend_from_slice(CACHE_MAGIC);
        bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
        bytes.extend_from_slice(checksum.as_bytes());
        bytes.extend_from_slice(&payload);

        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        // Cache entries are disposable and checksummed. A truncated buffered
        // write becomes a cache miss, while one fsync per source dominates a
        // cold full-data index.
        drop(file);
        fs::rename(temporary, path)?;
        Ok(())
    }

    /// Decodes one cache envelope after checking its magic, version, and payload hash.
    fn read_envelope<T: for<'de> Deserialize<'de>>(&self, path: &Path) -> Result<T, Error> {
        let bytes = fs::read(path)?;
        if bytes.len() < 44 || &bytes[..8] != CACHE_MAGIC {
            return Err(Error::Cache("cache magic is not valid".into()));
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().expect("fixed version bytes"));
        if version != CACHE_VERSION {
            return Err(Error::Cache(format!("unsupported cache version {version}")));
        }
        let expected: [u8; 32] = bytes[12..44].try_into().expect("fixed checksum bytes");
        let payload = &bytes[44..];
        if blake3::hash(payload).as_bytes() != &expected {
            return Err(Error::Cache("cache checksum does not match".into()));
        }
        Ok(postcard::from_bytes(payload)?)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ModuleManifest {
    module: String,
    canonical_root: PathBuf,
    files: BTreeMap<String, FileManifest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FileManifest {
    size: u64,
    modified: u128,
    source_kind: crate::SourceKind,
    content_hash: String,
    fingerprint: String,
    object: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedLocalization {
    size: u64,
    modified: u128,
    extractor_version: String,
    catalog: LocalizationCatalog,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedTooltips {
    size: u64,
    modified: u128,
    extractor_version: String,
    catalog: TooltipCatalog,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PackageManifest {
    path: PathBuf,
    size: u64,
    modified: u128,
    fingerprint: [u8; 16],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedPackagedCatalog {
    modules: Vec<String>,
    packages: Vec<PackageManifest>,
    extractor_version: String,
    catalog: PackagedThothCatalog,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedPackagedStats {
    modules: Vec<String>,
    packages: Vec<PackageManifest>,
    extractor_version: String,
    catalog: PackagedStatsCatalog,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedPackagedThothFacts<F> {
    extractor_version: String,
    catalog_digest: String,
    facts: PackagedThothFacts<F>,
}

/// Includes every semantic input that can change a cached parsed file.
fn object_key(source: &SourceFile, content_hash: &str, fingerprint: &str) -> String {
    let mut hash = blake3::Hasher::new();
    hash.update(source.path.to_string_lossy().as_bytes());
    hash.update(content_hash.as_bytes());
    hash.update(fingerprint.as_bytes());
    format!("{}.cache", hash.finalize().to_hex())
}

/// Fingerprints parser, schema, format, and localization inputs without reading source content.
fn context_fingerprint(kind: crate::SourceKind, schema: &str, language: &str) -> String {
    let osiris_identity =
        (kind == crate::SourceKind::Osiris).then(crate::osiris_catalog_cache_identity);
    context_fingerprint_with_osiris_identity(kind, schema, language, osiris_identity.as_deref())
}

/// Fingerprints parser inputs and an optional Osiris-only catalog identity.
///
/// Keeping the identity as an explicit input makes the source-kind boundary
/// testable: non-Osiris files must not acquire cache invalidations when the
/// Osiris contract catalog changes.
fn context_fingerprint_with_osiris_identity(
    kind: crate::SourceKind,
    schema: &str,
    language: &str,
    osiris_identity: Option<&str>,
) -> String {
    let stats_abi = tree_sitter::Language::from(tree_sitter_bg3::BG3_STATS_LANGUAGE).abi_version();
    let value_abi =
        tree_sitter::Language::from(tree_sitter_bg3::BG3_STATS_VALUE_LANGUAGE).abi_version();
    let thoth_abi = tree_sitter::Language::from(tree_sitter_bg3::BG3_THOTH_LANGUAGE).abi_version();
    let osiris_abi =
        tree_sitter::Language::from(tree_sitter_bg3::BG3_OSIRIS_LANGUAGE).abi_version();
    let mut hash = blake3::Hasher::new();
    hash.update(EXTRACTOR_VERSION.as_bytes());
    hash.update(env!("CARGO_PKG_VERSION").as_bytes());
    hash.update(tree_sitter_bg3::GRAMMAR_VERSION.as_bytes());
    hash.update(&stats_abi.to_le_bytes());
    hash.update(&value_abi.to_le_bytes());
    hash.update(&thoth_abi.to_le_bytes());
    hash.update(&osiris_abi.to_le_bytes());
    hash.update(format!("{kind:?}").as_bytes());
    hash.update(schema.as_bytes());
    hash.update(language.as_bytes());
    if kind == crate::SourceKind::Osiris
        && let Some(osiris_identity) = osiris_identity
    {
        hash.update(osiris_identity.as_bytes());
    }
    hash.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::SourceKind;
    use crate::packaged_stats::{PackagedStatsResolution, PackagedStatsSource};
    use crate::thoth::PackagedThothSource;
    use std::cell::Cell;
    use tempfile::tempdir;

    fn catalog(text: &str) -> PackagedThothCatalog {
        PackagedThothCatalog::from_sources([PackagedThothSource::new(
            "Example",
            "/synthetic/base.pak",
            "Mods/Example/Scripts/thoth/helpers/example.khn",
            0,
            text,
        )
        .expect("valid synthetic source")])
        .expect("valid synthetic catalog")
    }

    fn package_marker(checksum: u8) -> Vec<u8> {
        let mut package = Vec::new();
        package.extend_from_slice(b"LSPK");
        package.extend_from_slice(&18_u32.to_le_bytes());
        package.extend_from_slice(&40_u64.to_le_bytes());
        package.extend_from_slice(&8_u32.to_le_bytes());
        package.extend_from_slice(&[0, 0]);
        package.extend_from_slice(&[checksum; 16]);
        package.extend_from_slice(&1_u16.to_le_bytes());
        package.extend_from_slice(&[0_u8; 8]);
        package
    }

    #[test]
    fn packaged_thoth_cache_hits_and_invalidates_on_package_change() {
        let directory = tempdir().expect("temporary directory");
        let package = directory.path().join("base.pak");
        fs::write(&package, package_marker(1)).expect("write package marker");
        let cache = CacheStore::new(directory.path().join("cache")).expect("cache");
        let modules = vec!["Example".to_owned()];
        let candidates = vec![package.clone()];
        let calls = Cell::new(0);

        let (_, hit) = cache
            .load_packaged_thoth(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("first"))
            })
            .expect("first load");
        assert!(!hit);
        let (_, hit) = cache
            .load_packaged_thoth(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("unexpected"))
            })
            .expect("cached load");
        assert!(hit);
        assert_eq!(calls.get(), 1);

        fs::write(&package, package_marker(2)).expect("change package marker");
        let (_, hit) = cache
            .load_packaged_thoth(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("second"))
            })
            .expect("changed load");
        assert!(!hit);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn packaged_osiris_cache_is_distinct_from_thoth_cache() {
        let directory = tempdir().expect("temporary directory");
        let package = directory.path().join("base.pak");
        fs::write(&package, package_marker(1)).expect("write package marker");
        let cache = CacheStore::new(directory.path().join("cache")).expect("cache");
        let modules = vec!["Example".to_owned()];
        let candidates = vec![package.clone()];
        let calls = Cell::new(0);

        let (thoth, hit) = cache
            .load_packaged_thoth(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("thoth"))
            })
            .expect("Thoth catalog load");
        assert!(!hit);
        assert_eq!(
            thoth.sources().next().expect("Thoth source").text(),
            "thoth"
        );

        let (osiris, hit) = cache
            .load_packaged_osiris(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("osiris"))
            })
            .expect("Osiris catalog load");
        assert!(!hit);
        assert_eq!(
            osiris.sources().next().expect("Osiris source").text(),
            "osiris"
        );
        assert_eq!(calls.get(), 2);

        let (cached, hit) = cache
            .load_packaged_osiris(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("unexpected"))
            })
            .expect("cached Osiris catalog load");
        assert!(hit);
        assert_eq!(
            cached
                .sources()
                .next()
                .expect("cached Osiris source")
                .text(),
            "osiris"
        );
        assert_eq!(calls.get(), 2);

        fs::write(&package, package_marker(2)).expect("change package marker");
        let (changed, hit) = cache
            .load_packaged_osiris(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("changed"))
            })
            .expect("changed Osiris catalog load");
        assert!(!hit);
        assert_eq!(
            changed
                .sources()
                .next()
                .expect("changed Osiris source")
                .text(),
            "changed"
        );
        assert_eq!(calls.get(), 3);

        let cache_path = fs::read_dir(cache.root().join("osiris"))
            .expect("Osiris cache directory")
            .next()
            .expect("Osiris cache entry")
            .expect("read Osiris cache entry")
            .path();
        fs::write(cache_path, b"corrupt").expect("corrupt Osiris cache entry");
        let (rebuilt, hit) = cache
            .load_packaged_osiris(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(catalog("rebuilt"))
            })
            .expect("rebuilt Osiris catalog load");
        assert!(!hit);
        assert_eq!(
            rebuilt
                .sources()
                .next()
                .expect("rebuilt Osiris source")
                .text(),
            "rebuilt"
        );
        assert_eq!(calls.get(), 4);
        let (cached, hit) = cache
            .load_packaged_osiris(&modules, &candidates, || {
                panic!("a rebuilt Osiris catalog must be cached")
            })
            .expect("cached rebuilt Osiris catalog load");
        assert!(hit);
        assert_eq!(
            cached
                .sources()
                .next()
                .expect("cached rebuilt Osiris source")
                .text(),
            "rebuilt"
        );
    }

    #[test]
    fn packaged_catalog_cache_is_extractor_versioned() {
        let directory = tempdir().expect("temporary directory");
        let package = directory.path().join("base.pak");
        fs::write(&package, package_marker(1)).expect("write package marker");
        let cache = CacheStore::new(directory.path().join("cache")).expect("cache");
        let modules = vec!["Example".to_owned()];
        let candidates = vec![package];
        let calls = Cell::new(0);

        let (_, hit) = cache
            .load_packaged_catalog(&modules, &candidates, "osiris", "v1", || {
                calls.set(calls.get() + 1);
                Ok(catalog("v1"))
            })
            .expect("first versioned load");
        assert!(!hit);
        let (_, hit) = cache
            .load_packaged_catalog(&modules, &candidates, "osiris", "v1", || {
                calls.set(calls.get() + 1);
                Ok(catalog("unexpected"))
            })
            .expect("cached versioned load");
        assert!(hit);
        let (changed, hit) = cache
            .load_packaged_catalog(&modules, &candidates, "osiris", "v2", || {
                calls.set(calls.get() + 1);
                Ok(catalog("v2"))
            })
            .expect("changed extractor load");
        assert!(!hit);
        assert_eq!(
            changed.sources().next().expect("versioned source").text(),
            "v2"
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn packaged_stats_cache_hits_and_invalidates_on_package_change() {
        let directory = tempdir().expect("temporary directory");
        let package = directory.path().join("base.pak");
        fs::write(&package, package_marker(1)).expect("write package marker");
        let cache = CacheStore::new(directory.path().join("cache")).expect("cache");
        let modules = vec!["Shared".to_owned()];
        let candidates = vec![package.clone()];
        let calls = Cell::new(0);

        let load = |label: &str| {
            let source = PackagedStatsSource::new(
                "Shared",
                "base.pak",
                "Public/Shared/Stats/Generated/Data/Spell_Cache.txt",
                0,
                parse_source(
                    SourceFile {
                        path: "Public/Shared/Stats/Generated/Data/Spell_Cache.txt".into(),
                        kind: SourceKind::PlainStats,
                    },
                    &format!("new entry \"SPELL_{label}\"\ntype \"SpellData\"\n"),
                    &SchemaCatalog::default(),
                    "English",
                )
                .expect("synthetic parse"),
            )
            .expect("valid synthetic source");
            PackagedStatsCatalog::from_sources([source]).expect("valid synthetic catalog")
        };

        let (catalog, hit) = cache
            .load_packaged_stats(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(load("FIRST"))
            })
            .expect("first load");
        assert!(!hit);
        assert!(matches!(
            catalog.resolve_name("SPELL_FIRST"),
            PackagedStatsResolution::Unique(_)
        ));

        let (catalog, hit) = cache
            .load_packaged_stats(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok::<_, Error>(PackagedStatsCatalog::default())
            })
            .expect("cached load");
        assert!(hit);
        assert_eq!(calls.get(), 1);
        assert!(matches!(
            catalog.resolve_name("SPELL_FIRST"),
            PackagedStatsResolution::Unique(_)
        ));

        fs::write(&package, package_marker(2)).expect("change package marker");
        let (catalog, hit) = cache
            .load_packaged_stats(&modules, &candidates, || {
                calls.set(calls.get() + 1);
                Ok(load("SECOND"))
            })
            .expect("changed load");
        assert!(!hit);
        assert_eq!(calls.get(), 2);
        assert!(matches!(
            catalog.resolve_name("SPELL_SECOND"),
            PackagedStatsResolution::Unique(_)
        ));
    }

    #[test]
    fn packaged_thoth_facts_cache_is_content_and_extractor_versioned() {
        let directory = tempdir().expect("temporary directory");
        let cache = CacheStore::new(directory.path().join("cache")).expect("cache");
        let first_catalog = catalog("first");
        let calls = Cell::new(0);

        let (facts, hit) = cache
            .load_packaged_thoth_facts(&first_catalog, "facts-v1", |source| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>(source.text().to_owned())
            })
            .expect("first facts load");
        assert!(!hit);
        assert_eq!(facts.records()[0].facts(), "first");
        assert_eq!(calls.get(), 1);

        let (cached, hit) = cache
            .load_packaged_thoth_facts(&first_catalog, "facts-v1", |_| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>("unexpected".to_owned())
            })
            .expect("cached facts load");
        assert!(hit);
        assert_eq!(cached.records()[0].facts(), "first");
        assert_eq!(calls.get(), 1);

        let (reparsed, hit) = cache
            .load_packaged_thoth_facts(&catalog("second"), "facts-v1", |source| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>(source.text().to_owned())
            })
            .expect("changed facts load");
        assert!(!hit);
        assert_eq!(reparsed.records()[0].facts(), "second");
        assert_eq!(calls.get(), 2);

        let (_, hit) = cache
            .load_packaged_thoth_facts(&catalog("second"), "facts-v2", |_| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>("versioned".to_owned())
            })
            .expect("versioned facts load");
        assert!(!hit);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn packaged_thoth_facts_cache_persists_partial_parse_results() {
        let directory = tempdir().expect("temporary directory");
        let cache = CacheStore::new(directory.path().join("cache")).expect("cache");
        let catalog = PackagedThothCatalog::from_sources([
            PackagedThothSource::new(
                "Example",
                "/synthetic/base.pak",
                "Mods/Example/Scripts/thoth/helpers/valid.khn",
                0,
                "valid",
            )
            .expect("valid source"),
            PackagedThothSource::new(
                "Example",
                "/synthetic/base.pak",
                "Mods/Example/Scripts/thoth/helpers/broken.khn",
                0,
                "broken",
            )
            .expect("broken source still has valid package provenance"),
        ])
        .expect("valid catalog");

        let calls = Cell::new(0);
        let (facts, hit) = cache
            .load_packaged_thoth_facts(&catalog, "facts-v1", |source| {
                calls.set(calls.get() + 1);
                if source.text() == "broken" {
                    Err(Error::Parse("synthetic malformed source".into()))
                } else {
                    Ok::<_, Error>(source.text().to_owned())
                }
            })
            .expect("partial facts load");
        assert!(!hit);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts.rejected_count(), 1);
        assert_eq!(facts.relevant_rejected_count(), 1);
        assert_eq!(calls.get(), 2);

        let (cached, hit) = cache
            .load_packaged_thoth_facts(&catalog, "facts-v1", |_| -> Result<String, Error> {
                panic!("a cached partial result must not parse package entries")
            })
            .expect("cached partial facts load");
        assert!(hit);
        assert_eq!(cached.len(), 1);
        assert_eq!(cached.rejected_count(), 1);
        assert_eq!(cached.relevant_rejected_count(), 1);
    }

    #[test]
    fn osiris_context_identity_invalidates_only_osiris_fingerprints() {
        let identity = crate::osiris_catalog_cache_identity();
        let changed_identity = format!("{identity}\0changed");
        let osiris = context_fingerprint_with_osiris_identity(
            SourceKind::Osiris,
            "schema",
            "English",
            Some(&identity),
        );
        let changed_osiris = context_fingerprint_with_osiris_identity(
            SourceKind::Osiris,
            "schema",
            "English",
            Some(&changed_identity),
        );
        assert_ne!(osiris, changed_osiris);

        let thoth = context_fingerprint_with_osiris_identity(
            SourceKind::Thoth,
            "schema",
            "English",
            Some(&identity),
        );
        let changed_thoth = context_fingerprint_with_osiris_identity(
            SourceKind::Thoth,
            "schema",
            "English",
            Some(&changed_identity),
        );
        assert_eq!(thoth, changed_thoth);
    }

    #[test]
    fn packaged_osiris_facts_use_a_composite_identity_and_namespace() {
        let directory = tempdir().expect("temporary directory");
        let cache = CacheStore::new(directory.path().join("cache")).expect("cache");
        let catalog = catalog("same catalog");
        let calls = Cell::new(0);

        let (facts, hit) = cache
            .load_packaged_osiris_facts(&catalog, "facts-v1", |source| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>(source.text().to_owned())
            })
            .expect("first Osiris facts load");
        assert!(!hit);
        assert_eq!(facts.extractor_version(), "facts-v1");

        let (_, hit) = cache
            .load_packaged_osiris_facts(&catalog, "facts-v1", |_| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>("unexpected".to_owned())
            })
            .expect("cached Osiris facts load");
        assert!(hit);

        let (_, hit) = cache
            .load_packaged_osiris_facts(&catalog, "facts-v2", |source| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>(source.text().to_owned())
            })
            .expect("changed Osiris facts load");
        assert!(!hit);

        let (_, hit) = cache
            .load_packaged_thoth_facts(&catalog, "facts-v1", |_| {
                calls.set(calls.get() + 1);
                Ok::<_, Error>("thoth".to_owned())
            })
            .expect("separate Thoth facts load");
        assert!(!hit);
        assert_eq!(calls.get(), 3);
        let entries = fs::read_dir(cache.root().join("thoth"))
            .expect("packaged facts cache directory")
            .map(|entry| entry.expect("cache entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries
                .iter()
                .filter(|name| name.to_string_lossy().starts_with("facts-"))
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|name| name.to_string_lossy().starts_with("osiris-facts-"))
                .count(),
            2
        );
    }
}
