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
    LocalizationCatalog, base_localization_package_path, read_localization_package,
};
use crate::package::package_fingerprint;
use crate::parser::parse_source;
use crate::schema::SchemaCatalog;
use crate::thoth::PackagedThothCatalog;
use crate::tooltip::{TooltipCatalog, base_tooltip_package_path, read_base_tooltip_catalog};
use crate::{Error, ModuleSpec};

const CACHE_MAGIC: &[u8; 8] = b"BG3LSIDX";
const CACHE_VERSION: u32 = 2;
const EXTRACTOR_VERSION: &str = "bg3-ls-index-v3";
const LOCALIZATION_EXTRACTOR_VERSION: &str = "bg3-ls-localization-v1";
const TOOLTIP_EXTRACTOR_VERSION: &str = "bg3-ls-tooltips-v1";
const THOTH_EXTRACTOR_VERSION: &str = "bg3-ls-thoth-v1";
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
        identity.update(THOTH_EXTRACTOR_VERSION.as_bytes());
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
            .join("thoth")
            .join(format!("{}.cache", identity.finalize().to_hex()));

        if let Ok(cached) = self.read_envelope::<CachedPackagedThoth>(&cache_path)
            && cached.modules == modules
            && cached.packages == packages
            && cached.extractor_version == THOTH_EXTRACTOR_VERSION
        {
            return Ok((cached.catalog, true));
        }

        let catalog = load()?;
        self.write_envelope(
            &cache_path,
            &CachedPackagedThoth {
                modules,
                packages,
                extractor_version: THOTH_EXTRACTOR_VERSION.into(),
                catalog: catalog.clone(),
            },
        )?;
        Ok((catalog, false))
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
struct CachedPackagedThoth {
    modules: Vec<String>,
    packages: Vec<PackageManifest>,
    extractor_version: String,
    catalog: PackagedThothCatalog,
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
    hash.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
