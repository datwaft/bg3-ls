use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::package::PackageReader;

const MAX_THOTH_ENTRY_SIZE: usize = 4 * 1024 * 1024;
const MAX_THOTH_CATALOG_SIZE: usize = 64 * 1024 * 1024;

/// Aggregate coverage of packaged Thoth sources in one installed Data folder.
///
/// This inventory is research evidence only. It does not select modules or
/// contribute symbols to workspace resolution.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PackagedThothInventory {
    pub package_files: u64,
    pub rejected_packages: u64,
    pub thoth_entries: u64,
    pub parsed_sources: u64,
    pub rejected_sources: u64,
    pub declared_source_bytes: u64,
    pub functions: u64,
    pub classes: u64,
    pub aliases: u64,
    pub fields: u64,
    pub function_annotations: u64,
    pub duplicate_functions: u64,
    pub equal_priority_function_conflicts: u64,
    pub modules: BTreeMap<String, PackagedThothModuleInventory>,
}

/// Aggregate packaged Thoth coverage for one module.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PackagedThothModuleInventory {
    pub entries: u64,
    pub parsed_sources: u64,
    pub rejected_sources: u64,
    pub declared_source_bytes: u64,
    pub functions: u64,
    pub classes: u64,
    pub aliases: u64,
    pub fields: u64,
    pub function_annotations: u64,
}

/// One Thoth source stored in a BG3 package.
///
/// A packaged source is a virtual document. Its package and entry paths are
/// provenance only; they are not a filesystem path that an editor can open.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PackagedThothSource {
    module: String,
    package: PathBuf,
    entry: String,
    priority: u8,
    text: Arc<str>,
}

impl PackagedThothSource {
    /// Creates one packaged Thoth source after validating its virtual entry.
    pub fn new(
        module: impl Into<String>,
        package: impl Into<PathBuf>,
        entry: impl Into<String>,
        priority: u8,
        text: impl Into<Arc<str>>,
    ) -> Result<Self, Error> {
        let module = module.into();
        let package = package.into();
        let entry = entry.into();
        validate_module(&module)?;
        validate_entry(&entry, &module)?;
        if package.as_os_str().is_empty() {
            return Err(Error::Package(
                "packaged Thoth source has no package path".into(),
            ));
        }
        Ok(Self {
            module,
            package,
            entry,
            priority,
            text: text.into(),
        })
    }

    /// Creates one packaged Thoth source from a UTF-8 package entry.
    pub fn from_bytes(
        module: impl Into<String>,
        package: impl Into<PathBuf>,
        entry: impl Into<String>,
        priority: u8,
        bytes: &[u8],
    ) -> Result<Self, Error> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| Error::Package(format!("Thoth source is not UTF-8: {error}")))?;
        Self::new(module, package, entry, priority, Arc::<str>::from(text))
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

    /// Returns the immutable UTF-8 source text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the immutable UTF-8 bytes of the source text.
    pub fn bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }
}

/// The result of resolving one packaged Thoth entry.
///
/// Equal-priority candidates remain ambiguous. A caller must not choose by
/// package filename or filesystem enumeration order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackagedThothResolution<'a> {
    Missing,
    Unique(&'a PackagedThothSource),
    Ambiguous(&'a [PackagedThothSource]),
}

/// Immutable configured packaged Thoth sources grouped by virtual entry.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedThothCatalog {
    sources: BTreeMap<(String, String), Vec<PackagedThothSource>>,
}

impl PackagedThothCatalog {
    /// Builds a catalog and sorts candidates from highest to lowest priority.
    pub fn from_sources(
        sources: impl IntoIterator<Item = PackagedThothSource>,
    ) -> Result<Self, Error> {
        let mut catalog = Self::default();
        for source in sources {
            validate_module(source.module())?;
            validate_entry(source.entry(), source.module())?;
            catalog
                .sources
                .entry((source.module.clone(), source.entry.clone()))
                .or_default()
                .push(source);
        }
        for candidates in catalog.sources.values_mut() {
            candidates.sort_by(|left, right| {
                right
                    .priority
                    .cmp(&left.priority)
                    .then_with(|| left.package.cmp(&right.package))
                    .then_with(|| left.text.cmp(&right.text))
            });
        }
        Ok(catalog)
    }

    /// Returns all candidates for one exact module and virtual entry.
    ///
    /// Candidates are ordered by descending package priority. Equal-priority
    /// candidates are all retained.
    pub fn sources_for(&self, module: &str, entry: &str) -> &[PackagedThothSource] {
        self.sources
            .get(&(module.to_owned(), entry.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Resolves one exact entry without collapsing equal-priority candidates.
    pub fn resolve(&self, module: &str, entry: &str) -> PackagedThothResolution<'_> {
        let candidates = self.sources_for(module, entry);
        let Some(first) = candidates.first() else {
            return PackagedThothResolution::Missing;
        };
        let top_priority = first.priority;
        let top_count = candidates
            .iter()
            .take_while(|candidate| candidate.priority == top_priority)
            .count();
        if top_count == 1 {
            PackagedThothResolution::Unique(first)
        } else {
            PackagedThothResolution::Ambiguous(&candidates[..top_count])
        }
    }

    /// Returns the number of virtual source candidates, including ambiguities.
    pub fn len(&self) -> usize {
        self.sources.values().map(Vec::len).sum()
    }

    /// Tests whether no packaged Thoth sources were indexed.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Iterates over all candidates in deterministic module and entry order.
    pub fn sources(&self) -> impl Iterator<Item = &PackagedThothSource> {
        self.sources
            .values()
            .flat_map(|candidates| candidates.iter())
    }
}

/// Finds the narrow set of top-level packages that can contain configured
/// base-module Thoth sources.
///
/// This deliberately does not scan arbitrary package files. A base module is
/// represented by its exact module package file, while patch packages use the
/// top-level Patch*.pak naming convention. Returned paths are existing files
/// in deterministic order.
pub fn packaged_thoth_package_candidates(
    game_data: &Path,
    base_modules: &[String],
) -> Result<Vec<PathBuf>, Error> {
    let mut candidates = BTreeSet::new();
    for module in base_modules {
        validate_module(module)?;
        let path = game_data.join(format!("{module}.pak"));
        if path.is_file() {
            candidates.insert(path);
        }
    }
    for entry in fs::read_dir(game_data)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("Patch") && name.ends_with(".pak") {
            candidates.insert(path);
        }
    }
    Ok(candidates.into_iter().collect())
}

/// Inventories every supported Thoth entry in direct `.pak` files below one
/// installed Data directory.
///
/// Unlike [`read_packaged_thoth_catalog`], this function deliberately ignores
/// configured module selection. Its result is aggregate-only evidence for
/// deciding which installed source families can later become semantic inputs.
/// An unreadable package or source increments a rejection count and does not
/// hide unrelated valid entries.
pub fn inventory_packaged_thoth(game_data: &Path) -> Result<PackagedThothInventory, Error> {
    let mut packages = Vec::new();
    for entry in fs::read_dir(game_data)? {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|extension| extension == "pak") {
            packages.push(path);
        }
    }
    packages.sort();

    let mut inventory = PackagedThothInventory::default();
    let mut function_candidates: BTreeMap<(String, String), Vec<u8>> = BTreeMap::new();
    for package in packages {
        inventory.package_files += 1;
        let reader = match PackageReader::open(&package) {
            Ok(reader) => reader,
            Err(_) => {
                inventory.rejected_packages += 1;
                continue;
            }
        };
        let priority = reader.header().priority;
        for entry in reader.all_thoth_entries() {
            let Some(module) = thoth_module_from_entry(entry.name()) else {
                continue;
            };
            let module_inventory = inventory.modules.entry(module.into()).or_default();
            inventory.thoth_entries += 1;
            module_inventory.entries += 1;
            let declared_bytes = u64::try_from(entry.uncompressed_size())
                .map_err(|_| Error::Package("Thoth entry size does not fit u64".into()))?;
            inventory.declared_source_bytes = inventory
                .declared_source_bytes
                .checked_add(declared_bytes)
                .ok_or_else(|| Error::Package("Thoth inventory byte count overflowed".into()))?;
            module_inventory.declared_source_bytes = module_inventory
                .declared_source_bytes
                .checked_add(declared_bytes)
                .ok_or_else(|| Error::Package("Thoth inventory byte count overflowed".into()))?;
            if entry.uncompressed_size() > MAX_THOTH_ENTRY_SIZE {
                inventory.rejected_sources += 1;
                module_inventory.rejected_sources += 1;
                continue;
            }
            let bytes = match reader.read_entry(entry, MAX_THOTH_ENTRY_SIZE) {
                Ok(bytes) => bytes,
                Err(_) => {
                    inventory.rejected_sources += 1;
                    module_inventory.rejected_sources += 1;
                    continue;
                }
            };
            let source = match PackagedThothSource::from_bytes(
                module,
                &package,
                entry.name(),
                priority,
                &bytes,
            ) {
                Ok(source) => source,
                Err(_) => {
                    inventory.rejected_sources += 1;
                    module_inventory.rejected_sources += 1;
                    continue;
                }
            };
            let facts = match crate::parser::parse_thoth_file(source.text()) {
                Ok(facts) => facts,
                Err(_) => {
                    inventory.rejected_sources += 1;
                    module_inventory.rejected_sources += 1;
                    continue;
                }
            };
            inventory.parsed_sources += 1;
            module_inventory.parsed_sources += 1;
            let functions = u64::try_from(facts.declarations.len())
                .map_err(|_| Error::Package("Thoth declaration count does not fit u64".into()))?;
            let classes = u64::try_from(facts.annotations.classes.len())
                .map_err(|_| Error::Package("Thoth class count does not fit u64".into()))?;
            let aliases = u64::try_from(facts.annotations.aliases.len())
                .map_err(|_| Error::Package("Thoth alias count does not fit u64".into()))?;
            let fields = facts
                .annotations
                .classes
                .iter()
                .try_fold(0_u64, |total, class| {
                    total
                        .checked_add(u64::try_from(class.fields.len()).map_err(|_| {
                            Error::Package("Thoth field count does not fit u64".into())
                        })?)
                        .ok_or_else(|| Error::Package("Thoth field count overflowed".into()))
                })?;
            let function_annotations =
                u64::try_from(facts.annotations.functions.len()).map_err(|_| {
                    Error::Package("Thoth function annotation count does not fit u64".into())
                })?;
            for count in [
                (
                    &mut inventory.functions,
                    &mut module_inventory.functions,
                    functions,
                ),
                (
                    &mut inventory.classes,
                    &mut module_inventory.classes,
                    classes,
                ),
                (
                    &mut inventory.aliases,
                    &mut module_inventory.aliases,
                    aliases,
                ),
                (&mut inventory.fields, &mut module_inventory.fields, fields),
                (
                    &mut inventory.function_annotations,
                    &mut module_inventory.function_annotations,
                    function_annotations,
                ),
            ] {
                *count.0 = count
                    .0
                    .checked_add(count.2)
                    .ok_or_else(|| Error::Package("Thoth inventory count overflowed".into()))?;
                *count.1 = count
                    .1
                    .checked_add(count.2)
                    .ok_or_else(|| Error::Package("Thoth inventory count overflowed".into()))?;
            }
            for declaration in facts.declarations {
                function_candidates
                    .entry((module.into(), declaration.name))
                    .or_default()
                    .push(priority);
            }
        }
    }
    for priorities in function_candidates.into_values() {
        if priorities.len() > 1 {
            inventory.duplicate_functions += 1;
        }
        if let Some(highest) = priorities.iter().max()
            && priorities
                .iter()
                .filter(|priority| *priority == highest)
                .nth(1)
                .is_some()
        {
            inventory.equal_priority_function_conflicts += 1;
        }
    }
    Ok(inventory)
}

/// Reads configured base-module Thoth sources from the selected packages.
///
/// Every matching package entry is retained. Resolution is performed later by
/// PackagedThothCatalog::resolve, which selects a strictly higher priority
/// and preserves equal-priority ambiguity. Package paths and virtual entry
/// names remain provenance; no package entry is extracted as a filesystem
/// document.
pub fn read_packaged_thoth_catalog(
    game_data: &Path,
    base_modules: &[String],
) -> Result<PackagedThothCatalog, Error> {
    let packages = packaged_thoth_package_candidates(game_data, base_modules)?;
    let modules: BTreeSet<_> = base_modules.iter().map(String::as_str).collect();
    let mut sources = Vec::new();
    let mut total_size = 0_usize;
    for package in packages {
        let reader = PackageReader::open(&package)?;
        let priority = reader.header().priority;
        for module in modules.iter().copied() {
            let entries = reader.thoth_entries(module)?;
            for entry in entries {
                let entry_size = entry.uncompressed_size().max(entry.size_on_disk());
                if entry_size > MAX_THOTH_ENTRY_SIZE {
                    return Err(Error::Package(format!(
                        "Thoth entry {} exceeds the {MAX_THOTH_ENTRY_SIZE} byte limit",
                        entry.name()
                    )));
                }
                total_size = total_size
                    .checked_add(entry_size)
                    .ok_or_else(|| Error::Package("total Thoth catalog size overflowed".into()))?;
                if total_size > MAX_THOTH_CATALOG_SIZE {
                    return Err(Error::Package(
                        "Thoth catalog exceeds its aggregate byte limit".into(),
                    ));
                }
                let bytes = reader.read_entry(entry, MAX_THOTH_ENTRY_SIZE)?;
                let module_from_entry = thoth_module_from_entry(entry.name()).ok_or_else(|| {
                    Error::Package(format!(
                        "package reader returned an invalid Thoth entry {}",
                        entry.name()
                    ))
                })?;
                sources.push(PackagedThothSource::from_bytes(
                    module_from_entry,
                    package.clone(),
                    entry.name(),
                    priority,
                    &bytes,
                )?);
            }
        }
    }
    PackagedThothCatalog::from_sources(sources)
}

/// Extracts the module name from a supported packaged Thoth entry path.
pub fn thoth_module_from_entry(entry: &str) -> Option<&str> {
    let mut parts = entry.split('/');
    if parts.next() != Some("Mods") {
        return None;
    }
    let module = parts.next()?;
    if parts.next() != Some("Scripts") || parts.next() != Some("thoth") {
        return None;
    }
    let remainder = parts.collect::<Vec<_>>();
    if remainder.is_empty() || !entry.ends_with(".khn") {
        return None;
    }
    (!module.is_empty()
        && remainder
            .iter()
            .all(|part| !part.is_empty() && *part != "." && *part != ".." && !part.contains('\\')))
    .then_some(module)
}

fn validate_module(module: &str) -> Result<(), Error> {
    if module.is_empty()
        || module == "."
        || module == ".."
        || module.contains('/')
        || module.contains('\\')
        || module.chars().any(|character| character.is_control())
    {
        return Err(Error::Package(format!(
            "invalid Thoth module name `{module}`"
        )));
    }
    Ok(())
}

fn validate_entry(entry: &str, module: &str) -> Result<(), Error> {
    if thoth_module_from_entry(entry) != Some(module) {
        return Err(Error::Package(format!(
            "package entry `{entry}` is not a Thoth source for module `{module}`"
        )));
    }
    if entry
        .bytes()
        .any(|byte| byte == 0 || byte.is_ascii_control())
    {
        return Err(Error::Package(format!(
            "package entry `{entry}` contains a control character"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(package: &str, priority: u8, text: &str) -> PackagedThothSource {
        PackagedThothSource::new(
            "Example",
            package,
            "Mods/Example/Scripts/thoth/helpers/WeaponMastery.khn",
            priority,
            text,
        )
        .expect("valid synthetic source")
    }

    #[test]
    fn catalog_selects_highest_priority() {
        let catalog = PackagedThothCatalog::from_sources([
            source("base.pak", 0, "base"),
            source("patch.pak", 1, "patch"),
        ])
        .expect("valid catalog");

        assert_eq!(catalog.len(), 2);
        assert!(matches!(
            catalog.resolve("Example", "Mods/Example/Scripts/thoth/helpers/WeaponMastery.khn"),
            PackagedThothResolution::Unique(source) if source.text() == "patch"
        ));
    }

    #[test]
    fn catalog_preserves_equal_priority_ambiguity() {
        let catalog = PackagedThothCatalog::from_sources([
            source("second.pak", 1, "second"),
            source("first.pak", 1, "first"),
            source("base.pak", 0, "base"),
        ])
        .expect("valid catalog");

        let candidates = catalog.sources_for(
            "Example",
            "Mods/Example/Scripts/thoth/helpers/WeaponMastery.khn",
        );
        assert_eq!(candidates.len(), 3);
        assert!(matches!(
            catalog.resolve("Example", "Mods/Example/Scripts/thoth/helpers/WeaponMastery.khn"),
            PackagedThothResolution::Ambiguous(sources)
                if sources.len() == 2 && sources[0].priority() == 1
        ));
    }

    #[test]
    fn only_supported_thoth_entries_are_accepted() {
        assert_eq!(
            thoth_module_from_entry("Mods/Example/Scripts/thoth/helpers/file.khn"),
            Some("Example")
        );
        assert_eq!(
            thoth_module_from_entry("Mods/Example/Scripts/other/file.khn"),
            None
        );
        assert!(
            PackagedThothSource::new(
                "Example",
                "base.pak",
                "Mods/Other/Scripts/thoth/file.khn",
                0,
                "source",
            )
            .is_err()
        );
    }

    #[test]
    fn bytes_must_be_utf8() {
        assert!(
            PackagedThothSource::from_bytes(
                "Example",
                "base.pak",
                "Mods/Example/Scripts/thoth/file.khn",
                0,
                &[0xff],
            )
            .is_err()
        );
    }

    fn synthetic_package(entries: &[(&str, &[u8])], priority: u8) -> Vec<u8> {
        const HEADER_SIZE: usize = 40;
        const ENTRY_SIZE: usize = 272;
        let mut stored_entries = Vec::new();
        let mut raw_entries = Vec::new();
        let mut offset = HEADER_SIZE;
        for (name, contents) in entries {
            let mut entry = vec![0_u8; ENTRY_SIZE];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            entry[256..260].copy_from_slice(
                &u32::try_from(offset)
                    .expect("synthetic offset")
                    .to_le_bytes(),
            );
            entry[264..268].copy_from_slice(
                &u32::try_from(contents.len())
                    .expect("synthetic size")
                    .to_le_bytes(),
            );
            offset += contents.len();
            stored_entries.push(contents.to_vec());
            raw_entries.extend_from_slice(&entry);
        }
        let compressed_list = lz4_flex::block::compress(&raw_entries);
        let mut file_list = Vec::with_capacity(8 + compressed_list.len());
        file_list.extend_from_slice(
            &u32::try_from(entries.len())
                .expect("synthetic count")
                .to_le_bytes(),
        );
        file_list.extend_from_slice(
            &u32::try_from(compressed_list.len())
                .expect("synthetic list size")
                .to_le_bytes(),
        );
        file_list.extend_from_slice(&compressed_list);
        let file_list_offset = offset;
        let mut package = Vec::new();
        package.extend_from_slice(b"LSPK");
        package.extend_from_slice(&18_u32.to_le_bytes());
        package.extend_from_slice(
            &u64::try_from(file_list_offset)
                .expect("synthetic list offset")
                .to_le_bytes(),
        );
        package.extend_from_slice(
            &u32::try_from(file_list.len())
                .expect("synthetic list size")
                .to_le_bytes(),
        );
        package.push(0);
        package.push(priority);
        package.extend_from_slice(&[0_u8; 16]);
        package.extend_from_slice(&1_u16.to_le_bytes());
        for contents in stored_entries {
            package.extend_from_slice(&contents);
        }
        package.extend_from_slice(&file_list);
        package
    }

    #[test]
    fn package_candidates_are_narrow_and_deterministic() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(directory.path().join("Example.pak"), []).expect("base package");
        fs::write(directory.path().join("Patch2.pak"), []).expect("patch package");
        fs::write(directory.path().join("Other.pak"), []).expect("unrelated package");
        fs::create_dir(directory.path().join("nested")).expect("nested directory");
        fs::write(directory.path().join("nested/Patch3.pak"), []).expect("nested package");

        let modules = vec!["Example".to_owned(), "Example".to_owned()];
        let candidates =
            packaged_thoth_package_candidates(directory.path(), &modules).expect("candidates");
        assert_eq!(
            candidates,
            vec![
                directory.path().join("Example.pak"),
                directory.path().join("Patch2.pak")
            ]
        );
    }

    #[test]
    fn package_catalog_reads_configured_entries_and_keeps_provenance() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let entry = "Mods/Example/Scripts/thoth/helpers/WeaponMastery.khn";
        fs::write(
            directory.path().join("Example.pak"),
            synthetic_package(&[(entry, b"base")], 0),
        )
        .expect("base package");
        fs::write(
            directory.path().join("Patch1.pak"),
            synthetic_package(
                &[
                    (entry, b"patch"),
                    ("Mods/Other/Scripts/thoth/helpers/ignored.khn", b"ignored"),
                ],
                1,
            ),
        )
        .expect("patch package");
        fs::write(
            directory.path().join("Other.pak"),
            synthetic_package(&[(entry, b"unrelated")], 99),
        )
        .expect("unrelated package");

        let modules = vec![String::from("Example")];
        let catalog =
            read_packaged_thoth_catalog(directory.path(), &modules).expect("package catalog");
        assert_eq!(catalog.len(), 2);
        assert!(matches!(
            catalog.resolve("Example", entry),
            PackagedThothResolution::Unique(source)
                if source.text() == "patch"
                    && source.package() == directory.path().join("Patch1.pak")
        ));
    }

    #[test]
    fn inventory_scans_all_modules_and_preserves_aggregate_completeness() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            directory.path().join("Base.pak"),
            synthetic_package(
                &[
                    (
                        "Mods/Shared/Scripts/thoth/helpers/alpha.khn",
                        b"function Alpha() end\n",
                    ),
                    (
                        "Mods/Other/Scripts/thoth/helpers/beta.khn",
                        b"function Beta() end\n",
                    ),
                    ("Mods/Other/Scripts/thoth/helpers/invalid.khn", b"\xff"),
                    ("Mods/Other/Scripts/other/ignored.khn", b"ignored"),
                ],
                0,
            ),
        )
        .expect("base package");
        fs::write(
            directory.path().join("Patch1.pak"),
            synthetic_package(
                &[(
                    "Mods/Shared/Scripts/thoth/helpers/alpha.khn",
                    b"---@class Result\n---@field Value string\n---@alias ResultAlias Result\n---@return Result\nfunction Alpha() end\n",
                )],
                1,
            ),
        )
        .expect("first patch");
        fs::write(
            directory.path().join("Patch2.pak"),
            synthetic_package(
                &[(
                    "Mods/Shared/Scripts/thoth/helpers/alpha.khn",
                    b"function Alpha() end\n",
                )],
                1,
            ),
        )
        .expect("second patch");
        fs::write(directory.path().join("Broken.pak"), b"not an LSPK package")
            .expect("broken package");
        fs::write(directory.path().join("readme.txt"), b"ignored").expect("non-package file");

        let inventory = inventory_packaged_thoth(directory.path()).expect("inventory");
        assert_eq!(inventory.package_files, 4);
        assert_eq!(inventory.rejected_packages, 1);
        assert_eq!(inventory.thoth_entries, 5);
        assert_eq!(inventory.parsed_sources, 4);
        assert_eq!(inventory.rejected_sources, 1);
        assert_eq!(inventory.functions, 4);
        assert_eq!(inventory.classes, 1);
        assert_eq!(inventory.aliases, 1);
        assert_eq!(inventory.fields, 1);
        assert_eq!(inventory.function_annotations, 1);
        assert_eq!(inventory.duplicate_functions, 1);
        assert_eq!(inventory.equal_priority_function_conflicts, 1);
        assert_eq!(inventory.modules["Shared"].entries, 3);
        assert_eq!(inventory.modules["Shared"].parsed_sources, 3);
        assert_eq!(inventory.modules["Other"].entries, 2);
        assert_eq!(inventory.modules["Other"].parsed_sources, 1);
        assert_eq!(inventory.modules["Other"].rejected_sources, 1);
    }
}
