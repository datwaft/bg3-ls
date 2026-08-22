//! Packaged legacy Stats entries from configured base-module packages.
//!
//! The game stores its runtime Stats sources only inside `.pak` archives. This
//! module reads the `Stats/Generated/Data/*.txt` entries of configured base
//! modules from their module package and top-level patch packages, parses
//! them with the loose Stats parser against a virtual path, and exposes them
//! through an immutable catalog with package priority and ambiguity rules.
//!
//! Package paths and entry names remain provenance only. No package entry is
//! extracted as a filesystem document.

use std::collections::BTreeMap;

use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Error;
use crate::domain::{Definition, ParsedFile, SourceFile, SourceKind, SymbolTarget};
use crate::package::PackageReader;
use crate::parser::{canonical_kind, parse_source};
use crate::schema::SchemaCatalog;
use crate::thoth::packaged_thoth_package_candidates;

const MAX_STATS_ENTRY_SIZE: usize = 16 * 1024 * 1024;
const MAX_STATS_CATALOG_SIZE: usize = 128 * 1024 * 1024;

/// One parsed Stats source stored in a BG3 package.
///
/// A packaged source is a virtual document. Its package path and entry name
/// are provenance; they are not a filesystem location that an editor can
/// open. The virtual source path equals the package-relative entry name so
/// schema inference behaves exactly like loose `Public/<module>/Stats/`
/// discovery.
#[derive(Clone, Debug)]
pub struct PackagedStatsSource {
    module: String,
    package: PathBuf,
    entry: String,
    priority: u8,
    parsed: Arc<ParsedFile>,
}

impl PackagedStatsSource {
    /// Creates one packaged Stats source from already-parsed content.
    pub fn new(
        module: impl Into<String>,
        package: impl Into<PathBuf>,
        entry: impl Into<String>,
        priority: u8,
        parsed: ParsedFile,
    ) -> Result<Self, Error> {
        let module = module.into();
        validate_module(&module)?;
        let package = package.into();
        if package.as_os_str().is_empty() {
            return Err(Error::Package(
                "packaged Stats source has no package path".into(),
            ));
        }
        let entry = entry.into();
        if stats_module_from_entry(&entry) != Some(module.as_str()) {
            return Err(Error::Package(format!(
                "package entry `{entry}` is not a Stats source for module `{module}`"
            )));
        }
        Ok(Self {
            module,
            package,
            entry,
            priority,
            parsed: Arc::new(parsed),
        })
    }

    /// Returns the configured module that owns this source.
    pub fn module(&self) -> &str {
        &self.module
    }

    /// Returns the package used as provenance for this source.
    pub fn package(&self) -> &Path {
        &self.package
    }

    /// Returns the package-relative virtual entry path.
    pub fn entry(&self) -> &str {
        &self.entry
    }

    /// Returns the package priority used for packed-source resolution.
    pub fn priority(&self) -> u8 {
        self.priority
    }

    /// Returns the immutable parsed record of this source.
    pub fn parsed(&self) -> &ParsedFile {
        &self.parsed
    }
}

impl Serialize for PackagedStatsSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            &self.module,
            &self.package,
            &self.entry,
            self.priority,
            &self.parsed,
        )
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PackagedStatsSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (module, package, entry, priority, parsed): (String, PathBuf, String, u8, ParsedFile) =
            Deserialize::deserialize(deserializer)?;
        Self::new(module, package, entry, priority, parsed).map_err(serde::de::Error::custom)
    }
}

/// One declaration inside a packaged Stats source.
#[derive(Clone, Debug)]
pub struct PackagedStatsDefinition {
    source: Arc<PackagedStatsSource>,
    definition_index: usize,
}

impl PackagedStatsDefinition {
    /// Borrows the declaration from the owning immutable parsed file.
    pub fn definition(&self) -> &Definition {
        &self.source.parsed().definitions[self.definition_index]
    }

    /// Returns the packaged source that contains this declaration.
    pub fn source(&self) -> &PackagedStatsSource {
        &self.source
    }
}

/// The result of resolving one packaged Stats declaration.
///
/// Equal-priority candidates remain ambiguous. A caller must not choose by
/// package filename or file-list order.
#[derive(Clone, Debug)]
pub enum PackagedStatsResolution<'a> {
    Missing,
    Unique(&'a PackagedStatsDefinition),
    Ambiguous(&'a [PackagedStatsDefinition]),
}

/// Immutable packaged Stats declarations grouped by semantic target key.
///
/// The cacheable form serializes the source list once; candidate maps are
/// rebuilt on load so shared parsed records are never duplicated per
/// declaration.
#[derive(Clone, Debug, Default)]
pub struct PackagedStatsCatalog {
    sources: Vec<Arc<PackagedStatsSource>>,
    by_kind_name: BTreeMap<(String, String), Vec<PackagedStatsDefinition>>,
    by_name: BTreeMap<String, Vec<PackagedStatsDefinition>>,
    by_uuid: BTreeMap<Uuid, Vec<PackagedStatsDefinition>>,
}

impl PackagedStatsCatalog {
    /// Builds a catalog from complete, validated packaged Stats sources.
    ///
    /// Every declaration of every source is indexed under its kind-name pair,
    /// bare name, and UUID keys. Candidates are ordered by descending
    /// package priority and then by deterministic provenance order.
    pub fn from_sources(
        sources: impl IntoIterator<Item = PackagedStatsSource>,
    ) -> Result<Self, Error> {
        let mut sources: Vec<_> = sources.into_iter().collect();
        for source in &sources {
            validate_module(source.module())?;
            if stats_module_from_entry(source.entry()) != Some(source.module()) {
                return Err(Error::Package(format!(
                    "package entry `{}` is not a Stats source for module `{}`",
                    source.entry(),
                    source.module()
                )));
            }
        }
        sources.sort_by(|left, right| {
            left.module
                .cmp(&right.module)
                .then_with(|| left.package.cmp(&right.package))
                .then_with(|| left.entry.cmp(&right.entry))
        });
        let mut catalog = Self::default();
        for source in sources {
            let shared = Arc::new(source);
            for definition_index in 0..shared.parsed().definitions.len() {
                let candidate = PackagedStatsDefinition {
                    source: Arc::clone(&shared),
                    definition_index,
                };
                let definition = candidate.definition();
                if !definition.name.is_empty() && definition.name != "<anonymous>" {
                    push_candidate(
                        &mut catalog.by_kind_name,
                        (
                            canonical_kind(&definition.kind).to_owned(),
                            definition.name.clone(),
                        ),
                        candidate.clone(),
                    );
                    push_candidate(
                        &mut catalog.by_name,
                        definition.name.clone(),
                        candidate.clone(),
                    );
                }
                if let Some(uuid) = definition.uuid {
                    push_candidate(&mut catalog.by_uuid, uuid, candidate);
                }
            }
            catalog.sources.push(shared);
        }
        Ok(catalog)
    }

    /// Resolves one typed reference without collapsing equal priorities.
    pub fn resolve_kind_name(&self, kind: &str, name: &str) -> PackagedStatsResolution<'_> {
        resolve_candidates(
            self.by_kind_name
                .get(&(canonical_kind(kind).to_owned(), name.to_owned())),
        )
    }

    /// Resolves one untyped or alias reference without collapsing priorities.
    pub fn resolve_name(&self, name: &str) -> PackagedStatsResolution<'_> {
        resolve_candidates(self.by_name.get(name))
    }

    /// Resolves one UUID reference without collapsing equal priorities.
    pub fn resolve_uuid(&self, uuid: Uuid) -> PackagedStatsResolution<'_> {
        resolve_candidates(self.by_uuid.get(&uuid))
    }

    /// Returns every visible declaration that matches one semantic target.
    ///
    /// Targets without packaged evidence, such as tooltips and Osiris
    /// symbols, produce no candidates.
    pub fn candidates_for(&self, target: &SymbolTarget) -> Vec<&PackagedStatsDefinition> {
        match target {
            SymbolTarget::Named {
                kind: Some(kind),
                name,
            } => self.list(self.resolve_kind_name(kind, name)),
            SymbolTarget::Named { kind: None, name } => self.list(self.resolve_name(name)),
            SymbolTarget::Uuid(uuid) => self.list(self.resolve_uuid(*uuid)),
            _ => Vec::new(),
        }
    }

    fn list<'a>(
        &self,
        resolution: PackagedStatsResolution<'a>,
    ) -> Vec<&'a PackagedStatsDefinition> {
        match resolution {
            PackagedStatsResolution::Unique(candidate) => vec![candidate],
            PackagedStatsResolution::Ambiguous(candidates) => candidates.iter().collect(),
            PackagedStatsResolution::Missing => Vec::new(),
        }
    }

    /// Returns every indexed declaration of one canonical kind in name order.
    pub fn definitions_of_kind<'a>(
        &'a self,
        kind: &str,
    ) -> impl Iterator<Item = &'a PackagedStatsDefinition> + 'a {
        let kind = canonical_kind(kind).to_owned();
        self.by_kind_name
            .range((kind.clone(), String::new())..)
            .take_while(move |((candidate_kind, _), _)| candidate_kind == &kind)
            .flat_map(|(_, candidates)| candidates.iter())
    }

    /// Returns the number of indexed declarations across all sources.
    pub fn len(&self) -> usize {
        self.sources
            .iter()
            .map(|source| source.parsed().definitions.len())
            .sum()
    }

    /// Tests whether no packaged Stats declarations were indexed.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Iterates over all packaged sources in deterministic order.
    pub fn sources(&self) -> impl Iterator<Item = &PackagedStatsSource> {
        self.sources.iter().map(|source| source.as_ref())
    }
}

// The candidate maps hold shared references into the source list, so the
// cacheable form serializes only the sources once and rebuilds the maps on
// load instead of duplicating one parsed record per declaration.
impl Serialize for PackagedStatsCatalog {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.sources.iter())
    }
}

impl<'de> Deserialize<'de> for PackagedStatsCatalog {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let sources: Vec<PackagedStatsSource> = Vec::deserialize(deserializer)?;
        Self::from_sources(sources).map_err(serde::de::Error::custom)
    }
}

fn resolve_candidates(
    candidates: Option<&Vec<PackagedStatsDefinition>>,
) -> PackagedStatsResolution<'_> {
    let Some(candidates) = candidates else {
        return PackagedStatsResolution::Missing;
    };
    let Some(first) = candidates.first() else {
        return PackagedStatsResolution::Missing;
    };
    let top_priority = first.source.priority();
    let top_count = candidates
        .iter()
        .take_while(|candidate| candidate.source.priority() == top_priority)
        .count();
    if top_count == 1 {
        PackagedStatsResolution::Unique(first)
    } else {
        PackagedStatsResolution::Ambiguous(&candidates[..top_count])
    }
}

fn push_candidate<K: Ord>(
    map: &mut BTreeMap<K, Vec<PackagedStatsDefinition>>,
    key: K,
    candidate: PackagedStatsDefinition,
) {
    let candidates = map.entry(key).or_default();
    let position = candidates
        .binary_search_by(|existing| candidate_order(existing, &candidate))
        .unwrap_or_else(|position| position);
    candidates.insert(position, candidate);
}

fn candidate_order(
    left: &PackagedStatsDefinition,
    right: &PackagedStatsDefinition,
) -> std::cmp::Ordering {
    left.source()
        .priority()
        .cmp(&right.source().priority())
        .reverse()
        .then_with(|| left.source().package().cmp(right.source().package()))
        .then_with(|| left.source().entry().cmp(right.source().entry()))
        .then_with(|| left.definition_index.cmp(&right.definition_index))
}

/// Extracts the configured module name from a supported packaged Stats path.
///
/// Supported entries use the same relative shape as loose discovery:
/// `Public/<module>/Stats/Generated/Data/<file>.txt` or
/// `Mods/<module>/Stats/Generated/Data/<file>.txt`.
pub fn stats_module_from_entry(entry: &str) -> Option<&str> {
    let mut parts = entry.split('/');
    let root = parts.next()?;
    if root != "Public" && root != "Mods" {
        return None;
    }
    let module = parts.next()?;
    if module.is_empty() || module == "." || module == ".." {
        return None;
    }
    if parts.next()? != "Stats" || parts.next()? != "Generated" || parts.next()? != "Data" {
        return None;
    }
    let file = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let stem = file.strip_suffix(".txt")?;
    (!stem.is_empty()).then_some(module)
}

/// Reads packaged Stats entries for configured base modules.
///
/// Every matching package entry is retained. Resolution happens later through
/// [`PackagedStatsCatalog`] methods, which select a strictly higher priority
/// and preserve equal-priority ambiguity. Package paths and entry names stay
/// provenance; no package content becomes a filesystem document.
pub fn read_packaged_stats_catalog(
    game_data: &Path,
    base_modules: &[String],
    schema: &SchemaCatalog,
    language: &str,
) -> Result<PackagedStatsCatalog, Error> {
    let packages = packaged_thoth_package_candidates(game_data, base_modules)?;
    read_packaged_stats_catalog_from_packages(&packages, base_modules, schema, language)
}

/// Reads packaged Stats entries from one explicit package candidate set.
///
/// The caller supplies the complete, deterministic candidate set discovered
/// for the current configuration so cache identities can validate all
/// package inputs. Entry parsing runs in parallel because one cold catalog
/// parse otherwise adds close to a second to every first build.
pub fn read_packaged_stats_catalog_from_packages(
    packages: &[PathBuf],
    base_modules: &[String],
    schema: &SchemaCatalog,
    language: &str,
) -> Result<PackagedStatsCatalog, Error> {
    let mut modules = BTreeMap::new();
    for module in base_modules {
        validate_module(module)?;
        modules.insert(module.to_ascii_lowercase(), module.as_str());
    }
    let mut groups = Vec::new();
    let mut total_size = 0_usize;
    for package in packages {
        let reader = PackageReader::open(package)?;
        let priority = reader.header().priority;
        let mut entries = Vec::new();
        for entry in reader.entries() {
            let Some(raw_module) = stats_module_from_entry(entry.name()) else {
                continue;
            };
            if !modules.contains_key(&raw_module.to_ascii_lowercase()) {
                continue;
            }
            let entry_size = entry.uncompressed_size().max(entry.size_on_disk());
            if entry_size > MAX_STATS_ENTRY_SIZE {
                return Err(Error::Package(format!(
                    "Stats entry {} exceeds the {MAX_STATS_ENTRY_SIZE} byte limit",
                    entry.name()
                )));
            }
            total_size = total_size
                .checked_add(entry_size)
                .ok_or_else(|| Error::Package("total Stats catalog size overflowed".into()))?;
            if total_size > MAX_STATS_CATALOG_SIZE {
                return Err(Error::Package(
                    "Stats catalog exceeds its aggregate byte limit".into(),
                ));
            }
            entries.push(entry.name().to_owned());
        }
        if !entries.is_empty() {
            groups.push((package.clone(), priority, entries));
        }
    }

    let parsed_groups: Vec<Result<Vec<PackagedStatsSource>, Error>> = groups
        .par_iter()
        .map(|(package, priority, entries)| {
            let reader = PackageReader::open(package)?;
            let by_name: std::collections::HashMap<&str, _> = reader
                .entries()
                .iter()
                .map(|entry| (entry.name(), entry))
                .collect();
            entries
                .iter()
                .map(|name| {
                    let entry = *by_name.get(name.as_str()).ok_or_else(|| {
                        Error::Package(format!(
                            "package {} lost its Stats entry {name} while it was read",
                            package.display()
                        ))
                    })?;
                    let bytes = reader.read_entry(entry, MAX_STATS_ENTRY_SIZE)?;
                    let text = std::str::from_utf8(&bytes).map_err(|error| {
                        Error::Package(format!("Stats source is not UTF-8: {error}"))
                    })?;
                    let parsed = parse_source(
                        SourceFile {
                            path: PathBuf::from(entry.name()),
                            kind: SourceKind::PlainStats,
                        },
                        text,
                        schema,
                        language,
                    )?;
                    let module = modules
                        .get(
                            &stats_module_from_entry(entry.name())
                                .expect("checked entry shape")
                                .to_ascii_lowercase(),
                        )
                        .expect("checked module");
                    PackagedStatsSource::new(
                        (*module).to_owned(),
                        package.clone(),
                        entry.name(),
                        *priority,
                        parsed,
                    )
                })
                .collect()
        })
        .collect();

    let mut sources = Vec::new();
    for group in parsed_groups {
        sources.extend(group?);
    }
    PackagedStatsCatalog::from_sources(sources)
}

fn validate_module(module: &str) -> Result<(), Error> {
    if module.is_empty()
        || module == "."
        || module == ".."
        || module.contains('/')
        || module.contains('\\')
        || module.chars().any(char::is_control)
    {
        return Err(Error::Package(format!(
            "invalid packaged Stats module name `{module}`"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER_SIZE: usize = 40;
    const ENTRY_SIZE: usize = 272;

    fn synthetic_package(entries: &[(&str, &[u8])], priority: u8) -> Vec<u8> {
        let mut stored_entries = Vec::new();
        let mut raw_entries = Vec::new();
        let mut offset = HEADER_SIZE;
        for (name, contents) in entries {
            let stored = lz4_flex::block::compress(contents);
            let mut entry = vec![0_u8; ENTRY_SIZE];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            entry[256..260].copy_from_slice(
                &u32::try_from(offset)
                    .expect("synthetic offset")
                    .to_le_bytes(),
            );
            entry[263] = 2;
            entry[264..268].copy_from_slice(
                &u32::try_from(stored.len())
                    .expect("stored size")
                    .to_le_bytes(),
            );
            entry[268..272].copy_from_slice(
                &u32::try_from(contents.len())
                    .expect("decoded size")
                    .to_le_bytes(),
            );
            offset += stored.len();
            stored_entries.push(stored);
            raw_entries.extend_from_slice(&entry);
        }
        let compressed_list = lz4_flex::block::compress(&raw_entries);
        let mut file_list = Vec::with_capacity(8 + compressed_list.len());
        file_list.extend_from_slice(&u32::try_from(entries.len()).expect("count").to_le_bytes());
        file_list.extend_from_slice(
            &u32::try_from(compressed_list.len())
                .expect("list size")
                .to_le_bytes(),
        );
        file_list.extend_from_slice(&compressed_list);
        let file_list_offset = offset;
        let mut package = Vec::with_capacity(file_list_offset + file_list.len());
        package.extend_from_slice(b"LSPK");
        package.extend_from_slice(&18_u32.to_le_bytes());
        package.extend_from_slice(&u64::try_from(file_list_offset).unwrap().to_le_bytes());
        package.extend_from_slice(&u32::try_from(file_list.len()).unwrap().to_le_bytes());
        package.push(0);
        package.push(priority);
        package.extend_from_slice(&[0_u8; 16]);
        package.extend_from_slice(&1_u16.to_le_bytes());
        for stored in stored_entries {
            package.extend_from_slice(&stored);
        }
        package.extend_from_slice(&file_list);
        package
    }

    fn spell_text(name: &str, uuid: Option<&str>) -> String {
        let mut text = format!("new entry \"{name}\"\ntype \"SpellData\"\n");
        if let Some(uuid) = uuid {
            text.push_str(&format!("data \"UUID\" \"{uuid}\"\n"));
        }
        text.push_str("data \"UseCosts\" \"ActionPoint:1\"\n");
        text
    }

    fn write_package(
        directory: &Path,
        name: &str,
        entries: &[(&str, &[u8])],
        priority: u8,
    ) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, synthetic_package(entries, priority)).expect("write package");
        path
    }

    #[test]
    fn only_generated_data_txt_shapes_are_supported() {
        assert_eq!(
            stats_module_from_entry("Public/Shared/Stats/Generated/Data/Spell_Example.txt"),
            Some("Shared")
        );
        assert_eq!(
            stats_module_from_entry("Mods/Example/Stats/Generated/Data/Passive.txt"),
            Some("Example")
        );
        assert_eq!(
            stats_module_from_entry("Public/Shared/Stats/Other.txt"),
            None
        );
        assert_eq!(
            stats_module_from_entry("Public/Shared/Stats/Generated/Data/Spell_Example.lsx"),
            None
        );
        assert_eq!(
            stats_module_from_entry("Public/Shared/Stats/Generated/Data/Nested/Deep.txt"),
            None
        );
        assert_eq!(
            stats_module_from_entry("Localization/Shared/Stats/Generated/Data/A.txt"),
            None
        );
    }

    #[test]
    fn catalog_selects_strictly_higher_priority_and_preserves_ties() {
        let source = |package: &str, priority: u8| {
            PackagedStatsSource::new(
                "Shared",
                package,
                "Public/Shared/Stats/Generated/Data/Spell_A.txt",
                priority,
                parse_source(
                    SourceFile {
                        path: "Public/Shared/Stats/Generated/Data/Spell_A.txt".into(),
                        kind: SourceKind::PlainStats,
                    },
                    &spell_text("SPELL_A", None),
                    &SchemaCatalog::default(),
                    "English",
                )
                .expect("parse"),
            )
            .expect("source")
        };

        let unique =
            PackagedStatsCatalog::from_sources([source("base.pak", 0), source("patch.pak", 2)])
                .expect("catalog");
        assert!(matches!(
            unique.resolve_kind_name("SpellData", "SPELL_A"),
            PackagedStatsResolution::Unique(candidate) if candidate.source().priority() == 2
        ));

        let ambiguous = PackagedStatsCatalog::from_sources([
            source("tie.pak", 2),
            source("base.pak", 0),
            source("patch.pak", 2),
        ])
        .expect("catalog");
        match ambiguous.resolve_kind_name("SpellData", "SPELL_A") {
            PackagedStatsResolution::Ambiguous(candidates) => {
                assert_eq!(candidates.len(), 2);
                assert!(
                    candidates
                        .iter()
                        .all(|candidate| candidate.source().priority() == 2)
                );
            }
            _ => panic!("expected ambiguity"),
        }
    }

    #[test]
    fn missing_targets_resolve_to_missing() {
        let catalog = PackagedStatsCatalog::default();
        assert!(matches!(
            catalog.resolve_kind_name("SpellData", "SPELL_MISSING"),
            PackagedStatsResolution::Missing
        ));
        assert!(matches!(
            catalog.resolve_name("SPELL_MISSING"),
            PackagedStatsResolution::Missing
        ));
        assert!(matches!(
            catalog.resolve_uuid(Uuid::nil()),
            PackagedStatsResolution::Missing
        ));
    }

    #[test]
    fn indexes_kind_name_name_and_uuid_keys() {
        let uuid = "11111111-2222-4333-8444-555555555555";
        let text = spell_text("SPELL_INDEXED", Some(uuid));
        let source = PackagedStatsSource::new(
            "Shared",
            "base.pak",
            "Public/Shared/Stats/Generated/Data/Spell_Index.txt",
            0,
            parse_source(
                SourceFile {
                    path: "Public/Shared/Stats/Generated/Data/Spell_Index.txt".into(),
                    kind: SourceKind::PlainStats,
                },
                &text,
                &SchemaCatalog::default(),
                "English",
            )
            .expect("parse"),
        )
        .expect("source");
        let catalog = PackagedStatsCatalog::from_sources([source]).expect("catalog");
        assert_eq!(catalog.len(), 1);

        let typed = catalog.resolve_kind_name("SpellData", "SPELL_INDEXED");
        let untyped = catalog.resolve_name("SPELL_INDEXED");
        let parsed_uuid = Uuid::parse_str(uuid).expect("synthetic uuid");
        let by_uuid = catalog.resolve_uuid(parsed_uuid);

        for resolution in [typed, untyped, by_uuid] {
            let PackagedStatsResolution::Unique(candidate) = resolution else {
                panic!("expected unique resolution");
            };
            assert_eq!(candidate.definition().name, "SPELL_INDEXED");
            assert_eq!(candidate.definition().kind, "SpellData");
            assert_eq!(
                candidate
                    .definition()
                    .fields
                    .get("UseCosts")
                    .map(String::as_str),
                Some("ActionPoint:1")
            );
        }
    }

    #[test]
    fn rejects_entries_that_do_not_match_their_module() {
        assert!(
            PackagedStatsSource::new(
                "Shared",
                "base.pak",
                "Public/Other/Stats/Generated/Data/Spell_A.txt",
                0,
                parse_source(
                    SourceFile {
                        path: "Public/Other/Stats/Generated/Data/Spell_A.txt".into(),
                        kind: SourceKind::PlainStats,
                    },
                    &spell_text("SPELL_A", None),
                    &SchemaCatalog::default(),
                    "English",
                )
                .expect("parse"),
            )
            .is_err()
        );
    }

    #[test]
    fn reads_configured_modules_and_skips_unconfigured_ones() {
        let directory = tempfile::tempdir().expect("tempdir");
        let package = write_package(
            directory.path(),
            "Shared.pak",
            &[
                (
                    "Public/Shared/Stats/Generated/Data/Spell_Configured.txt",
                    spell_text("SPELL_CONFIGURED", None).as_bytes(),
                ),
                (
                    "Public/Unlisted/Stats/Generated/Data/Spell_Unlisted.txt",
                    b"new entry \"SPELL_UNLISTED\"\ntype \"SpellData\"\n",
                ),
                ("Public/Shared/Docs/Readme.md", b"skip"),
            ],
            0,
        );
        let candidates = vec![package];
        let catalog = read_packaged_stats_catalog_from_packages(
            &candidates,
            &["Shared".to_owned()],
            &SchemaCatalog::default(),
            "English",
        )
        .expect("catalog");

        assert!(matches!(
            catalog.resolve_kind_name("SpellData", "SPELL_CONFIGURED"),
            PackagedStatsResolution::Unique(_)
        ));
        assert!(matches!(
            catalog.resolve_name("SPELL_UNLISTED"),
            PackagedStatsResolution::Missing
        ));
    }

    #[test]
    fn non_utf8_entries_fail_loudly() {
        let directory = tempfile::tempdir().expect("tempdir");
        let package = write_package(
            directory.path(),
            "Shared.pak",
            &[(
                "Public/Shared/Stats/Generated/Data/Spell_Bad.txt",
                &[0xff, 0xfe],
            )],
            0,
        );
        let error = read_packaged_stats_catalog_from_packages(
            &[package],
            &["Shared".to_owned()],
            &SchemaCatalog::default(),
            "English",
        )
        .expect_err("non-UTF-8 must fail");
        assert!(error.to_string().contains("not UTF-8"));
    }

    #[test]
    fn serialization_round_trip_preserves_resolution() {
        let text = spell_text("SPELL_CACHED", None);
        let source = PackagedStatsSource::new(
            "Shared",
            "base.pak",
            "Public/Shared/Stats/Generated/Data/Spell_Cached.txt",
            3,
            parse_source(
                SourceFile {
                    path: "Public/Shared/Stats/Generated/Data/Spell_Cached.txt".into(),
                    kind: SourceKind::PlainStats,
                },
                &text,
                &SchemaCatalog::default(),
                "English",
            )
            .expect("parse"),
        )
        .expect("source");
        let catalog = PackagedStatsCatalog::from_sources([source]).expect("catalog");
        let bytes = postcard::to_stdvec(&catalog).expect("serialize");
        let restored: PackagedStatsCatalog = postcard::from_bytes(&bytes).expect("deserialize");
        assert!(matches!(
            restored.resolve_kind_name("SpellData", "SPELL_CACHED"),
            PackagedStatsResolution::Unique(candidate) if candidate.source().priority() == 3
        ));
    }
}
