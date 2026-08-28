//! Editor-neutral language operations over immutable BG3 module indexes.

mod diagnostics;
mod language;
mod thoth;
mod thoth_flow;
mod thoth_members;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_index::{
    Definition, LocalizationCatalog, ModuleIndex, ModuleRole, ModuleSpec, OSIRIS_CONTRACTS,
    OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND, OSIRIS_PROCEDURE_KIND, OSIRIS_QUERY_KIND,
    OsirisCallRole, OsirisContractKind, OsirisDatabaseBinding, OsirisParameterDirection,
    OsirisVariableFact, OsirisVariableOccurrence, PackagedOsirisIndex, PackagedOsirisResolution,
    PackagedStatsCatalog, PackagedThothApiIndex, PackagedThothCatalog, PackagedThothFacts,
    ParsedFile, Position, Reference, SchemaCatalog, SourceKind, SymbolTarget,
    THOTH_FACTS_EXTRACTOR_VERSION, THOTH_FUNCTION_KIND, TextRange, ThothExpressionKind, ThothFile,
    TooltipCatalog, canonical_kind, osiris_contract, osiris_type_compatibility,
    parse_packaged_thoth_facts,
};

pub use diagnostics::{Diagnostic, DiagnosticSeverity};
pub use language::{CompletionItem, CompletionKind, CompletionList, SignatureHelp};
pub use thoth::{
    ResolvedThothAlias, ResolvedThothClass, ResolvedThothField, ResolvedThothFunction,
    ResolvedThothVariable, ThothTypeSource,
};

/// Editor-neutral hover content and the exact source span that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverResult {
    pub markdown: String,
    pub range: Option<TextRange>,
}

struct OsirisVariableMatch<'a> {
    variable: &'a OsirisVariableFact,
    occurrence_range: TextRange,
}

/// One proven type observation for a database column in a visible Story
/// occurrence. This is kept separate from runtime row counts: reads and
/// removals can contribute compile-time type evidence without storing rows.
#[derive(Clone, Debug)]
pub(crate) struct OsirisDatabaseTypeObservation {
    pub(crate) path: PathBuf,
    pub(crate) range: TextRange,
    pub(crate) type_name: String,
}

/// The schema state for one database column after Story-order folding.
#[derive(Clone, Debug, Default)]
pub(crate) struct OsirisDatabaseColumnSchema {
    pub(crate) established: Option<OsirisDatabaseTypeObservation>,
    pub(crate) conflicts: Vec<OsirisDatabaseTypeObservation>,
    pub(crate) ambiguous: bool,
    /// All distinct source observations in Story order. The established
    /// observation is not enough to compare GUID aliases because generic
    /// `GUIDSTRING` is compatible with each specialized alias while those
    /// aliases are not compatible with each other.
    observations: Vec<OsirisDatabaseTypeObservation>,
}

/// One implicit database schema assembled from all visible loose Story goals.
///
/// Goal names define the Story order. A duplicate goal name is an equal-order
/// group, so incompatible evidence in that group is ambiguous rather than
/// resolved by filesystem path or module precedence.
#[derive(Clone, Debug)]
pub(crate) struct OsirisDatabaseSchema {
    pub(crate) name: String,
    pub(crate) arity: u16,
    pub(crate) columns: Vec<OsirisDatabaseColumnSchema>,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) removals: usize,
    pub(crate) contributors: BTreeSet<(usize, String, String, PathBuf)>,
}

struct VisibleOsirisSource<'a> {
    module: &'a str,
    rank: usize,
    path: PathBuf,
    file: &'a ParsedFile,
}

#[derive(Clone)]
struct OrderedDatabaseOccurrence {
    name: String,
    arity: u16,
    goal: String,
    module: String,
    rank: usize,
    path: PathBuf,
    role: OsirisCallRole,
    argument_ranges: Vec<TextRange>,
    arguments: Vec<Option<OsirisDatabaseTypeObservation>>,
    propagation_sources: Vec<Option<OsirisDatabaseColumnKey>>,
}

type OsirisDatabaseColumnKey = (String, u16, usize);

impl OsirisVariableMatch<'_> {
    fn occurrence(&self) -> Option<&OsirisVariableOccurrence> {
        self.variable
            .occurrence_facts
            .iter()
            .find(|occurrence| occurrence.range == self.occurrence_range)
    }

    fn binding_range(&self) -> Option<TextRange> {
        match self.occurrence() {
            Some(occurrence) => occurrence.binding_range,
            None => self.variable.binding_range,
        }
    }

    fn database_binding(&self) -> Option<&OsirisDatabaseBinding> {
        match self.occurrence() {
            Some(occurrence) => occurrence.database_binding.as_ref(),
            None => self.variable.database_binding.as_ref(),
        }
    }

    fn evidence(&self) -> Option<&bg3_index::OsirisTypeEvidence> {
        match self.occurrence() {
            Some(occurrence) => occurrence.evidence.as_ref(),
            None => self.variable.evidence.as_ref(),
        }
    }
}

impl std::ops::Deref for HoverResult {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.markdown
    }
}

impl std::fmt::Display for HoverResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.markdown)
    }
}

/// Shared presentation builder for the stable hover information hierarchy.
pub(crate) struct HoverMarkup {
    markdown: String,
}

impl HoverMarkup {
    pub(crate) fn new(kind: &str, name: &str) -> Self {
        Self {
            markdown: format!(
                "**{}** {}",
                escape_markdown_text(kind),
                markdown_inline_code(name)
            ),
        }
    }

    pub(crate) fn fact(mut self, label: &str, value: &str) -> Self {
        self.markdown
            .push_str(&format!("\n\n{label}: {}", bounded_inline_code(value)));
        self
    }

    /// Appends trusted Markdown supplied by a curated catalog or renderer.
    pub(crate) fn markdown(mut self, markdown: &str) -> Self {
        if !markdown.is_empty() {
            self.markdown.push_str("\n\n");
            self.markdown.push_str(markdown);
        }
        self
    }

    /// Appends external prose as escaped, bounded plain text.
    pub(crate) fn prose(mut self, prose: &str) -> Self {
        if !prose.is_empty() {
            self.markdown.push_str("\n\n");
            self.markdown.push_str(&bounded_markdown_text(prose));
        }
        self
    }

    pub(crate) fn finish(self) -> String {
        self.markdown
    }
}

/// A definition result with the module that contributes its precedence.
#[derive(Clone, Debug)]
pub struct ResolvedDefinition {
    pub module: String,
    pub rank: usize,
    pub path: PathBuf,
    pub definition: Definition,
    pub ambiguous: bool,
    /// The package-relative entry name when this declaration originates from
    /// a base-module package. Packaged origins have no editable location.
    pub packaged_entry: Option<String>,
}

/// A source location returned by editor-neutral analysis operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub range: TextRange,
}

/// A top-level symbol with its module label.
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub module: String,
    pub location: SourceLocation,
}

/// A full open-document overlay that replaces one disk record.
#[derive(Clone, Debug)]
pub struct OverlayDocument {
    pub module: String,
    pub version: i32,
    pub text: String,
    pub parsed: Arc<ParsedFile>,
}

/// Open document records kept separate from immutable disk module indexes.
#[derive(Clone, Debug, Default)]
pub struct OverlaySet {
    pub(crate) documents: BTreeMap<PathBuf, OverlayDocument>,
}

impl OverlaySet {
    /// Replaces the overlay for one open source path.
    pub fn insert(&mut self, path: PathBuf, document: OverlayDocument) {
        self.documents.insert(path, document);
    }

    /// Removes an overlay so queries use the disk record again.
    pub fn remove(&mut self, path: &Path) {
        self.documents.remove(path);
    }

    /// Returns the current overlay for one path.
    pub fn get(&self, path: &Path) -> Option<&OverlayDocument> {
        self.documents.get(path)
    }

    /// Lists open paths and versions for asynchronous diagnostic refreshes.
    pub fn versions(&self) -> Vec<(PathBuf, i32)> {
        self.documents
            .iter()
            .map(|(path, document)| (path.clone(), document.version))
            .collect()
    }

    /// Tests whether a disk record is suppressed by an open document.
    fn contains(&self, path: &Path) -> bool {
        self.documents.contains_key(path)
    }

    /// Returns overlays owned by one module.
    fn for_module<'a>(
        &'a self,
        module: &'a str,
    ) -> impl Iterator<Item = (&'a PathBuf, &'a OverlayDocument)> {
        self.documents
            .iter()
            .filter(move |(_, document)| document.module == module)
    }
}

/// An immutable, generation-numbered composition of visible module layers.
#[derive(Clone, Debug)]
pub struct WorkspaceSnapshot {
    pub schema: Arc<SchemaCatalog>,
    pub layers: Vec<Arc<ModuleIndex>>,
    pub generation: u64,
    pub max_workspace_symbols: usize,
    pub max_completion_items: usize,
    base_localization: Arc<LocalizationCatalog>,
    packaged_thoth: Arc<PackagedThothCatalog>,
    packaged_thoth_facts: Arc<PackagedThothFacts<ThothFile>>,
    packaged_stats: Arc<PackagedStatsCatalog>,
    packaged_thoth_api: Arc<PackagedThothApiIndex>,
    packaged_osiris: Arc<PackagedOsirisIndex>,
    tooltips: Arc<TooltipCatalog>,
    incomplete_kinds: BTreeSet<String>,
}

fn empty_packaged_thoth_facts() -> PackagedThothFacts<ThothFile> {
    parse_packaged_thoth_facts(
        &PackagedThothCatalog::default(),
        THOTH_FACTS_EXTRACTOR_VERSION,
        |_| Ok(ThothFile::default()),
    )
    .expect("an empty packaged Thoth catalog must parse")
}

impl WorkspaceSnapshot {
    /// Creates a workspace whose layers are ordered from lowest to highest precedence.
    pub fn new(
        schema: Arc<SchemaCatalog>,
        layers: Vec<Arc<ModuleIndex>>,
        generation: u64,
        max_workspace_symbols: usize,
        max_completion_items: usize,
    ) -> Self {
        Self {
            schema,
            layers,
            generation,
            max_workspace_symbols,
            max_completion_items,
            base_localization: Arc::new(LocalizationCatalog::default()),
            packaged_thoth: Arc::new(PackagedThothCatalog::default()),
            packaged_thoth_facts: Arc::new(empty_packaged_thoth_facts()),
            packaged_stats: Arc::new(PackagedStatsCatalog::default()),
            packaged_thoth_api: Arc::new(PackagedThothApiIndex::default()),
            packaged_osiris: Arc::new(PackagedOsirisIndex::default()),
            tooltips: Arc::new(TooltipCatalog::default()),
            incomplete_kinds: BTreeSet::new(),
        }
    }

    /// Adds packed base text that has no navigable source location.
    pub fn with_base_localization(mut self, catalog: Arc<LocalizationCatalog>) -> Self {
        self.base_localization = catalog;
        self
    }

    /// Returns the number of packed base localization handles.
    pub fn base_localization_count(&self) -> usize {
        self.base_localization.len()
    }

    /// Shares the immutable packed catalog with a scoped workspace rebuild.
    pub fn base_localization(&self) -> Arc<LocalizationCatalog> {
        Arc::clone(&self.base_localization)
    }

    /// Adds installed Thoth sources read from configured base-game packages.
    pub fn with_packaged_thoth(mut self, catalog: Arc<PackagedThothCatalog>) -> Self {
        self.packaged_thoth = catalog;
        self.packaged_thoth_api = Arc::new(PackagedThothApiIndex::from_catalog_and_facts(
            &self.packaged_thoth,
            &self.packaged_thoth_facts,
        ));
        self
    }

    /// Returns the number of packaged Thoth source candidates.
    pub fn packaged_thoth_count(&self) -> usize {
        self.packaged_thoth.len()
    }

    /// Shares the immutable packaged Thoth catalog with a workspace rebuild.
    pub fn packaged_thoth(&self) -> Arc<PackagedThothCatalog> {
        Arc::clone(&self.packaged_thoth)
    }

    /// Adds parsed facts extracted from installed Thoth package entries.
    pub fn with_packaged_thoth_facts(mut self, facts: Arc<PackagedThothFacts<ThothFile>>) -> Self {
        self.packaged_thoth_facts = facts;
        self.packaged_thoth_api = Arc::new(PackagedThothApiIndex::from_catalog_and_facts(
            &self.packaged_thoth,
            &self.packaged_thoth_facts,
        ));
        self
    }

    /// Shares immutable parsed facts extracted from installed Thoth packages.
    pub fn packaged_thoth_facts(&self) -> Arc<PackagedThothFacts<ThothFile>> {
        Arc::clone(&self.packaged_thoth_facts)
    }

    /// Adds immutable Stats declarations read from configured base-module
    /// packages.
    pub fn with_packaged_stats(mut self, catalog: Arc<PackagedStatsCatalog>) -> Self {
        self.packaged_stats = catalog;
        self
    }

    /// Returns the number of indexed packaged Stats declarations.
    pub fn packaged_stats_count(&self) -> usize {
        self.packaged_stats.len()
    }

    /// Shares the immutable packaged Stats catalog with a scoped rebuild.
    pub fn packaged_stats(&self) -> Arc<PackagedStatsCatalog> {
        Arc::clone(&self.packaged_stats)
    }

    /// Returns the number of installed package entries with parsed Thoth facts.
    pub fn packaged_thoth_facts_count(&self) -> usize {
        self.packaged_thoth_facts.len()
    }

    /// Adds parsed facts extracted from installed Osiris goal package entries.
    pub fn with_packaged_osiris(mut self, index: Arc<PackagedOsirisIndex>) -> Self {
        self.packaged_osiris = index;
        self
    }

    /// Shares the immutable packaged Osiris callable index.
    pub fn packaged_osiris(&self) -> Arc<PackagedOsirisIndex> {
        Arc::clone(&self.packaged_osiris)
    }

    /// Shares immutable source-backed API contracts for configured packages.
    pub fn packaged_thoth_api(&self) -> Arc<PackagedThothApiIndex> {
        Arc::clone(&self.packaged_thoth_api)
    }

    /// Adds the static game tooltip glossary that has no navigable source location.
    pub fn with_tooltips(mut self, catalog: Arc<TooltipCatalog>) -> Self {
        self.tooltips = catalog;
        self
    }

    /// Returns the number of static game tooltip keys.
    pub fn tooltip_count(&self) -> usize {
        self.tooltips.len()
    }

    /// Shares the immutable static tooltip glossary with a scoped workspace rebuild.
    pub fn tooltips(&self) -> Arc<TooltipCatalog> {
        Arc::clone(&self.tooltips)
    }

    /// Marks symbol kinds whose visible loose sources omit packed data.
    ///
    /// Resolution still uses every indexed declaration. This flag only stops
    /// diagnostics that would claim that an absent declaration does not exist.
    pub fn with_incomplete_kinds(
        mut self,
        kinds: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.incomplete_kinds
            .extend(kinds.into_iter().map(Into::into));
        self
    }

    /// Tests whether the index can prove that a declaration kind is absent.
    pub fn has_complete_kind(&self, kind: &str) -> bool {
        !self.incomplete_kinds.contains(kind)
    }

    /// Returns the most specific configured module that contains a source path.
    pub fn module_for_path(&self, path: &Path) -> Option<&ModuleSpec> {
        if let Some(layer) = self.layers.iter().find(|layer| layer.file(path).is_some()) {
            return Some(&layer.spec);
        }
        self.layers
            .iter()
            .filter(|layer| path.starts_with(&layer.spec.root))
            .max_by_key(|layer| layer.spec.root.as_os_str().len())
            .map(|layer| &layer.spec)
    }

    /// Resolves every visible declaration from highest to lowest precedence.
    ///
    /// Packaged base-module declarations join the rank of their module, so a
    /// loose declaration in the same base module stays a same-rank ambiguity
    /// and every dependency or project override still wins.
    pub fn resolve(&self, target: &SymbolTarget, overlays: &OverlaySet) -> Vec<ResolvedDefinition> {
        let mut resolved = Vec::new();
        let packaged = self.packaged_stats.candidates_for(target);
        for (rank, layer) in self.layers.iter().enumerate().rev() {
            let mut at_rank = Vec::new();
            for (path, overlay) in overlays.for_module(&layer.spec.name) {
                for definition in &overlay.parsed.definitions {
                    if definition_matches(definition, target) {
                        at_rank.push(ResolvedDefinition {
                            module: layer.spec.name.clone(),
                            rank,
                            path: path.clone(),
                            definition: definition.clone(),
                            ambiguous: false,
                            packaged_entry: None,
                        });
                    }
                }
            }
            for record in layer.resolve(target) {
                if !overlays.contains(record.path.as_ref()) {
                    at_rank.push(ResolvedDefinition {
                        module: layer.spec.name.clone(),
                        rank,
                        path: record.path.as_ref().clone(),
                        definition: record.definition().clone(),
                        ambiguous: false,
                        packaged_entry: None,
                    });
                }
            }
            for candidate in packaged.iter() {
                if candidate.source().module() != layer.spec.name {
                    continue;
                }
                at_rank.push(ResolvedDefinition {
                    module: layer.spec.name.clone(),
                    rank,
                    path: candidate.source().package().to_path_buf(),
                    definition: candidate.definition().clone(),
                    ambiguous: false,
                    packaged_entry: Some(candidate.source().entry().to_owned()),
                });
            }
            at_rank.sort_by(|left, right| {
                left.path.cmp(&right.path).then_with(|| {
                    left.definition
                        .selection_range
                        .start
                        .line
                        .cmp(&right.definition.selection_range.start.line)
                })
            });
            if at_rank.len() > 1 && !matches!(target, SymbolTarget::OsirisDatabase { .. }) {
                for definition in &mut at_rank {
                    definition.ambiguous = true;
                }
            }
            resolved.extend(at_rank);
        }
        resolved
    }

    /// Returns definitions for the symbol under one source position.
    pub fn definitions_at(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Vec<ResolvedDefinition> {
        self.target_at(path, position, overlays)
            .map_or_else(Vec::new, |target| self.resolve(&target, overlays))
    }

    /// Returns navigable locations for the symbol under one source position.
    ///
    /// Packaged Thoth members and packaged Stats declarations are virtual
    /// evidence and therefore return no location instead of a fabricated
    /// archive-entry URI.
    pub fn definition_locations_at(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Vec<SourceLocation> {
        if let Some(variable) = self.osiris_variable_at(path, position, overlays) {
            return variable
                .binding_range()
                .into_iter()
                .map(|range| SourceLocation {
                    path: path.to_owned(),
                    range,
                })
                .collect();
        }
        if let Some(locations) = self.thoth_member_definition_locations(path, position, overlays) {
            return locations;
        }
        self.definitions_at(path, position, overlays)
            .into_iter()
            .filter(|definition| definition.packaged_entry.is_none())
            .map(|definition| SourceLocation {
                path: definition.path,
                range: definition.definition.selection_range,
            })
            .collect()
    }

    /// Returns a rich Markdown description for the symbol under one position.
    pub fn hover(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<HoverResult> {
        self.hover_markdown(path, position, overlays)
            .map(|markdown| HoverResult {
                markdown,
                range: self.hover_range_at(path, position, overlays),
            })
    }

    /// Returns language-specific hover when normal symbol resolution is silent.
    pub fn language_hover(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<HoverResult> {
        self.language_hover_markdown(path, position, overlays)
            .map(|markdown| HoverResult {
                markdown,
                range: self.hover_range_at(path, position, overlays),
            })
    }

    fn hover_markdown(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<String> {
        if let Some(variable) = self.osiris_variable_at(path, position, overlays) {
            return Some(self.osiris_variable_hover(variable, overlays));
        }
        let target = self.target_at(path, position, overlays)?;
        if let SymbolTarget::Named {
            kind: Some(kind),
            name,
        } = &target
            && kind == "Localization"
        {
            return self.localization_hover(name, overlays);
        }
        if let Some(field) = self.field_at(path, position, overlays) {
            return Some(field);
        }
        if let SymbolTarget::Tooltip { name } = &target {
            return self.tooltip_tag_hover(name, overlays);
        }
        if let SymbolTarget::OsirisDatabase { .. } = &target {
            return self.osiris_database_hover(&target, overlays);
        }
        let definitions = self.resolve(&target, overlays);
        if definitions.is_empty()
            && let SymbolTarget::OsirisCallable { name, arity } = &target
        {
            if let Some(hover) = self.packaged_osiris_callable_hover(name, *arity) {
                return Some(hover);
            }
            if let Some(hover) = self.generated_osiris_callable_hover(name, *arity) {
                return Some(hover);
            }
            return Some(
                HoverMarkup::new("Osiris callable", &format!("{name}/{arity}"))
                    .fact("Arity", &arity.to_string())
                    .markdown(
                        "No configured loose declaration is visible. Callable kind and parameter types are unknown; the symbol can come from packed or unconfigured Story sources.",
                    )
                    .finish(),
            );
        }
        let effective = definitions.first()?;
        if effective.definition.kind == THOTH_FUNCTION_KIND
            && let Some(hover) =
                self.annotated_thoth_hover(&effective.definition.name, &definitions, overlays)
        {
            return Some(hover);
        }
        let heading = match effective.definition.kind.as_str() {
            THOTH_FUNCTION_KIND => "Thoth function",
            OSIRIS_GOAL_KIND => "Osiris goal",
            OSIRIS_PROCEDURE_KIND => "Osiris procedure",
            OSIRIS_QUERY_KIND => "Osiris query",
            _ => &effective.definition.kind,
        };
        let mut markdown =
            HoverMarkup::new(heading, &effective.definition.name).fact("Module", &effective.module);
        if let Some(block) = self.stats_source_block(effective, overlays) {
            markdown = markdown.markdown(block.trim_start());
        } else {
            if let Some(uuid) = effective.definition.uuid {
                markdown = markdown.fact("UUID", &uuid.to_string());
            }
            if let Some(parent) = &effective.definition.parent {
                markdown = markdown.fact("Parent", parent);
            }
            if matches!(
                effective.definition.kind.as_str(),
                THOTH_FUNCTION_KIND | OSIRIS_PROCEDURE_KIND | OSIRIS_QUERY_KIND
            ) {
                let parameters = effective
                    .definition
                    .fields
                    .get("Parameters")
                    .map_or("", String::as_str);
                let signature = format!("{}({parameters})", effective.definition.name);
                markdown = markdown.fact("Signature", &signature);
            }
            let signature_kind = matches!(
                effective.definition.kind.as_str(),
                THOTH_FUNCTION_KIND | OSIRIS_PROCEDURE_KIND | OSIRIS_QUERY_KIND
            );
            let fields: Vec<_> = effective
                .definition
                .fields
                .iter()
                .filter(|(key, _)| !(signature_kind && *key == "Parameters"))
                .collect();
            let mut field_markdown = String::new();
            for (key, value) in fields.iter().take(MAX_HOVER_LIST_ENTRIES) {
                field_markdown.push_str(&format!(
                    "\n\n- **{}:** {}",
                    escape_markdown_text(key),
                    field_value_markdown(value)
                ));
            }
            append_omitted_entries(
                &mut field_markdown,
                fields.len().saturating_sub(MAX_HOVER_LIST_ENTRIES),
            );
            markdown = markdown.markdown(field_markdown.trim_start());
        }
        markdown = markdown.fact("Source", &display_path(&effective.path));
        if let Some(entry) = &effective.packaged_entry {
            markdown = markdown.fact("Package entry", entry);
        }
        if definitions.len() > 1 {
            let mut overrides = String::from("**Override chain**\n");
            for definition in definitions.iter().take(MAX_HOVER_LIST_ENTRIES) {
                let ambiguity = if definition.ambiguous {
                    " — same-rank ambiguity"
                } else {
                    ""
                };
                overrides.push_str(&format!(
                    "\n- {} — {}{}",
                    markdown_inline_code(&definition.module),
                    markdown_inline_code(&display_path(&definition.path)),
                    ambiguity
                ));
            }
            append_omitted_entries(
                &mut overrides,
                definitions.len().saturating_sub(MAX_HOVER_LIST_ENTRIES),
            );
            markdown = markdown.markdown(&overrides);
        }
        if let Some(preview) = self.tooltip_preview(&definitions, overlays) {
            markdown = markdown.markdown(preview.trim_start());
        }
        Some(markdown.finish())
    }

    /// Renders a callable from the versioned engine contract catalog.
    fn generated_osiris_callable_hover(&self, name: &str, arity: u16) -> Option<String> {
        let contract = osiris_contract(OSIRIS_CONTRACTS, name, arity)?;
        let mut markdown = HoverMarkup::new(
            osiris_contract_kind_label(contract.kind),
            &format!("{name}/{arity}"),
        )
        .markdown(&osiris_contract_signature_markdown(name, contract));
        if let Some(description) = bg3_index::osiris_callable_description(name, arity) {
            markdown = markdown.prose(description);
        }
        markdown = markdown.markdown(&format!(
            "**Catalog:** BG3 build `{}`",
            bg3_index::OSIRIS_CATALOG_SOURCE_VERSION
        ));
        Some(markdown.finish())
    }

    /// Finds the smallest syntax-backed span that contains one hover position.
    fn hover_range_at(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<TextRange> {
        let (_, file) = self.file(path, overlays)?;
        let mut candidates = file
            .references
            .iter()
            .map(|reference| reference.range)
            .chain(
                file.definitions
                    .iter()
                    .map(|definition| definition.selection_range),
            )
            .chain(
                file.definitions
                    .iter()
                    .flat_map(|definition| definition.field_ranges.values().copied()),
            )
            .collect::<Vec<_>>();
        if let Some(thoth) = &file.thoth {
            candidates.extend(
                thoth
                    .declarations
                    .iter()
                    .map(|declaration| declaration.name_range),
            );
            candidates.extend(thoth.calls.iter().map(|call| call.name_range));
            candidates.extend(thoth.member_accesses.iter().map(|access| access.range));
            for fact in &thoth.expression_facts {
                candidates.push(fact.range);
                if let ThothExpressionKind::MemberAccess(segments) = &fact.kind {
                    candidates.extend(segments.iter().skip(1).map(|segment| segment.range));
                }
            }
            candidates.extend(thoth.annotations.classes.iter().flat_map(|class| {
                std::iter::once(class.name_range)
                    .chain(class.fields.iter().map(|field| field.name_range))
            }));
            candidates.extend(
                thoth
                    .annotations
                    .aliases
                    .iter()
                    .map(|alias| alias.name_range),
            );
            candidates.extend(
                thoth
                    .annotations
                    .functions
                    .iter()
                    .filter_map(|function| function.name_range),
            );
            candidates.extend(
                thoth
                    .annotations
                    .variables
                    .iter()
                    .map(|variable| variable.target_range),
            );
        }
        if let Some(osiris) = &file.osiris {
            candidates.extend(
                osiris
                    .variables
                    .iter()
                    .flat_map(|variable| variable.occurrences.iter().copied()),
            );
        }
        let semantic = candidates
            .into_iter()
            .filter(|range| range_contains(*range, position))
            .min_by_key(hover_range_size);
        semantic.or_else(|| {
            let text = overlays.get(path)?.text.as_str();
            lexical_hover_range(text, position)
        })
    }

    /// Renders the configured-language value for one exact localization handle.
    fn localization_hover(&self, handle: &str, overlays: &OverlaySet) -> Option<String> {
        let target = SymbolTarget::Named {
            kind: Some("Localization".into()),
            name: handle.into(),
        };
        let text = self.localized_value(handle, overlays)?;
        let mut markdown = HoverMarkup::new("Localization", handle);
        if let Some(definition) = self.resolve(&target, overlays).first() {
            if let Some(language) = definition.definition.fields.get("Language") {
                markdown = markdown.fact("Language", language);
            }
            if let Some(version) = definition.definition.fields.get("Version") {
                markdown = markdown.fact("Version", version);
            }
            markdown = markdown
                .fact("Module", &definition.module)
                .fact("Source", &display_path(&definition.path));
        } else if let Some(packed) = self.base_localization.get(handle) {
            markdown = markdown
                .fact("Language", self.base_localization.language())
                .fact("Version", &packed.version.to_string())
                .fact("Source", "packed base localization");
        }
        Some(markdown.markdown(&render_localized_text(&text)).finish())
    }

    /// Describes one database from all visible loose occurrence evidence.
    fn osiris_database_hover(
        &self,
        target: &SymbolTarget,
        overlays: &OverlaySet,
    ) -> Option<String> {
        let SymbolTarget::OsirisDatabase { name, arity } = target else {
            return None;
        };
        let schema = self
            .osiris_database_schemas(overlays)
            .remove(&(name.clone(), *arity))?;
        let parameters = osiris_database_parameter_types(&schema);
        let database_name = format!("{name}/{arity}");
        let signature = format!("{}({})", name, parameters.join(", "));
        let mut markdown = HoverMarkup::new("Osiris database", &database_name)
            .fact("Signature", &signature)
            .fact("Writes", &schema.writes.to_string())
            .fact("Reads", &schema.reads.to_string())
            .fact("Removals", &schema.removals.to_string());
        if schema.writes == 0 {
            markdown = markdown.markdown(
                "No write is visible in configured loose sources. The database can come from packed or unconfigured Story sources.",
            );
        }
        if !schema.contributors.is_empty() {
            let mut goals = String::from("**Contributing goals**");
            let mut contributors: Vec<_> = schema.contributors.into_iter().collect();
            contributors.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
            let omitted = contributors.len().saturating_sub(MAX_HOVER_LIST_ENTRIES);
            for (_, module, goal, path) in contributors.into_iter().take(MAX_HOVER_LIST_ENTRIES) {
                goals.push_str(&format!(
                    "\n\n- {} — module {} — {}",
                    markdown_inline_code(&goal),
                    markdown_inline_code(&module),
                    markdown_inline_code(&display_path(&path))
                ));
            }
            append_omitted_entries(&mut goals, omitted);
            markdown = markdown.markdown(&goals);
        }
        Some(markdown.finish())
    }

    /// Renders installed declaration evidence for a loose-callable miss.
    ///
    /// Base-module goals are virtual package entries, so the hover reports
    /// module provenance and authored parameter aliases without inventing a
    /// source path. Same-priority disagreements stay untyped.
    fn packaged_osiris_callable_hover(&self, name: &str, arity: u16) -> Option<String> {
        let mut ambiguous = false;
        for layer in self
            .layers
            .iter()
            .rev()
            .filter(|layer| layer.spec.role == ModuleRole::Base)
        {
            match self.packaged_osiris.resolve(&layer.spec.name, name, arity) {
                PackagedOsirisResolution::Missing => continue,
                PackagedOsirisResolution::Ambiguous(_) => {
                    ambiguous = true;
                    break;
                }
                PackagedOsirisResolution::Unique(candidate) => {
                    let callable = candidate.callable();
                    let heading = if callable.kind == OSIRIS_PROCEDURE_KIND {
                        "Installed Osiris procedure"
                    } else {
                        "Installed Osiris query"
                    };
                    return Some(
                        HoverMarkup::new(heading, &format!("{name}/{arity}"))
                            .fact("Module", &layer.spec.name)
                            .fact("Signature", &format!("{}({})", name, callable.parameters.join(", ")))
                            .markdown(
                                "Declared in an installed package goal. Parameter aliases come from the authored declaration; the entry stays virtual and has no file location.",
                            )
                            .finish(),
                    );
                }
            }
        }
        ambiguous.then(|| {
            HoverMarkup::new("Installed Osiris callable", &format!("{name}/{arity}"))
                .fact("Arity", &arity.to_string())
                .markdown(
                    "Same-priority installed package declarations disagree on this callable. Parameter types stay untyped.",
                )
                .finish()
        })
    }

    /// Builds a static localized preview from effective inherited tooltip fields.
    fn tooltip_preview(
        &self,
        definitions: &[ResolvedDefinition],
        overlays: &OverlaySet,
    ) -> Option<String> {
        let effective = definitions.first()?;
        let mut visited = BTreeSet::new();
        let fields = self.inherited_fields(effective, definitions, 0, overlays, &mut visited);
        let display_name = fields
            .get("DisplayName")
            .filter(|value| !value.is_empty())
            .and_then(|value| self.localized_value(value, overlays));
        let description = fields
            .get("Description")
            .filter(|value| !value.is_empty())
            .and_then(|value| self.localized_value(value, overlays));
        let parameters = fields
            .get("DescriptionParams")
            .filter(|value| !value.is_empty());
        if display_name.is_none() && description.is_none() && parameters.is_none() {
            return None;
        }

        let mut preview = "---\n\n### Game text preview".to_owned();
        if let Some(display_name) = display_name {
            preview.push_str(&format!(
                "\n\n**Title**\n\n**{}**",
                render_localized_text(&display_name)
            ));
        }
        if let Some(description) = description {
            preview.push_str(&format!(
                "\n\n**Description**\n\n{}",
                render_localized_text(&description)
            ));
        }
        if let Some(parameters) = parameters {
            preview.push_str(&format!(
                "\n\nDescription parameters: {}",
                bounded_inline_code(parameters)
            ));
        }
        preview.push_str("\n\n*Static preview. Game logic and UI formatting are not evaluated.*");
        Some(preview)
    }

    /// Renders one static game glossary entry without evaluating UI data bindings.
    fn tooltip_tag_hover(&self, name: &str, overlays: &OverlaySet) -> Option<String> {
        let tooltip = self.tooltips.get(name)?;
        let title = tooltip
            .title
            .as_deref()
            .and_then(|handle| self.localized_value(handle, overlays));
        let description = tooltip
            .description
            .as_deref()
            .and_then(|handle| self.localized_value(handle, overlays));
        if title.is_none() && description.is_none() {
            return None;
        }

        let mut markdown = HoverMarkup::new("Game tooltip", name);
        if let Some(title) = title {
            markdown = markdown.markdown(&format!(
                "\n\n**Title**\n\n**{}**",
                render_localized_text(&title)
            ));
        }
        if let Some(description) = description {
            markdown = markdown.markdown(&format!(
                "\n\n**Description**\n\n{}",
                render_localized_text(&description)
            ));
        }
        markdown = markdown.markdown(
            "\n\n*Static game text. Runtime values and UI formatting are not evaluated.*",
        );
        Some(markdown.finish())
    }

    /// Resolves `using` recursively and lets each child field replace its parent value.
    fn inherited_fields(
        &self,
        current: &ResolvedDefinition,
        chain: &[ResolvedDefinition],
        chain_index: usize,
        overlays: &OverlaySet,
        visited: &mut BTreeSet<(usize, PathBuf, u32, u32)>,
    ) -> BTreeMap<String, String> {
        let identity = (
            current.rank,
            current.path.clone(),
            current.definition.selection_range.start.line,
            current.definition.selection_range.start.character,
        );
        if !visited.insert(identity) {
            return BTreeMap::new();
        }

        let mut fields = BTreeMap::new();
        if let Some(parent) = &current.definition.parent {
            if parent == &current.definition.name {
                if let Some((next_index, next)) = chain
                    .iter()
                    .enumerate()
                    .skip(chain_index + 1)
                    .find(|(_, candidate)| candidate.rank < current.rank)
                {
                    fields = self.inherited_fields(next, chain, next_index, overlays, visited);
                }
            } else {
                let parent_chain = self.resolve(
                    &SymbolTarget::Named {
                        kind: Some(current.definition.kind.clone()),
                        name: parent.clone(),
                    },
                    overlays,
                );
                if let Some(parent_definition) = parent_chain.first() {
                    fields = self.inherited_fields(
                        parent_definition,
                        &parent_chain,
                        0,
                        overlays,
                        visited,
                    );
                }
            }
        }
        fields.extend(current.definition.fields.clone());
        fields
    }

    /// Resolves a translated-string handle from loose layers before packed base text.
    fn localized_value(&self, value: &str, overlays: &OverlaySet) -> Option<String> {
        let handle = value.split_once(';').map_or(value, |(handle, _)| handle);
        let loose = self.resolve(
            &SymbolTarget::Named {
                kind: Some("Localization".into()),
                name: handle.into(),
            },
            overlays,
        );
        loose
            .first()
            .and_then(|definition| definition.definition.fields.get("Text"))
            .cloned()
            .or_else(|| {
                self.base_localization
                    .get(handle)
                    .map(|value| value.text.to_owned())
            })
    }

    /// Renders one legacy Stats declaration as its reconstructed source block.
    ///
    /// The block keeps the original field order and resolves localized handles
    /// into comment lines, so editors with the `tree-sitter-bg3` queries
    /// highlight it exactly like a Stats file. Only `new entry` declarations
    /// use this shape; named blocks and other source formats keep the field
    /// list.
    fn stats_source_block(
        &self,
        effective: &ResolvedDefinition,
        overlays: &OverlaySet,
    ) -> Option<String> {
        let packaged = effective.packaged_entry.is_some();
        if !packaged {
            let (_, file) = self.file(&effective.path, overlays)?;
            if file.source.kind != SourceKind::PlainStats {
                return None;
            }
        }
        if NAMED_BLOCK_KINDS.contains(&effective.definition.kind.as_str()) {
            return None;
        }
        let mut ordered: Vec<_> = effective.definition.fields.iter().collect();
        ordered.sort_by(|(left, _), (right, _)| {
            let position = |key: &String| {
                effective
                    .definition
                    .field_ranges
                    .get(key)
                    .map(|range| (range.start.line, range.start.character))
            };
            match (position(left), position(right)) {
                (Some(left_position), Some(right_position)) => left_position.cmp(&right_position),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left.cmp(right),
            }
        });
        let mut lines = vec![format!("new entry \"{}\"", effective.definition.name)];
        if effective.definition.kind != "StatEntry" {
            lines.push(format!("type \"{}\"", effective.definition.kind));
        }
        if let Some(parent) = &effective.definition.parent {
            lines.push(format!("using \"{parent}\""));
        }
        let mut hidden = 0usize;
        let omitted_fields = ordered.len().saturating_sub(MAX_HOVER_SOURCE_FIELDS);
        for (key, value) in ordered.into_iter().take(MAX_HOVER_SOURCE_FIELDS) {
            if matches!(key.as_str(), "DisplayName" | "Description") {
                for comment in self.localization_comment_lines(value, overlays) {
                    lines.push(format!("// {comment}"));
                }
            }
            if PRESENTATION_ONLY_FIELDS.contains(&key.as_str()) {
                hidden += 1;
                continue;
            }
            lines.push(format!("data \"{key}\" \"{}\"", clamp_stats_value(value)));
        }
        if hidden > 0 {
            lines.push(format!("// … {hidden} hidden presentation fields"));
        }
        if omitted_fields > 0 {
            lines.push(format!("// … {omitted_fields} additional fields omitted"));
        }
        let fence_length = lines
            .iter()
            .map(|line| longest_backtick_run(line))
            .chain([2])
            .max()
            .unwrap_or(2)
            + 1;
        let fence = "`".repeat(fence_length);
        let mut block = String::from("\n\n");
        block.push_str(&fence);
        block.push_str("bg3_stats\n");
        for line in &lines {
            block.push_str(line);
            block.push('\n');
        }
        block.push_str(&fence);
        Some(block)
    }

    /// Resolves one translated-string field value into single-line comments.
    fn localization_comment_lines(&self, value: &str, overlays: &OverlaySet) -> Vec<String> {
        let Some(text) = self.localized_value(value, overlays) else {
            return Vec::new();
        };
        render_localized_text(&text)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Finds references to the symbol under one position across visible modules.
    pub fn references_at(
        &self,
        path: &Path,
        position: Position,
        include_declaration: bool,
        overlays: &OverlaySet,
    ) -> Vec<SourceLocation> {
        if let Some(variable) = self.osiris_variable_at(path, position, overlays) {
            return variable
                .variable
                .occurrences
                .iter()
                .filter(|range| {
                    include_declaration || Some(**range) != variable.variable.binding_range
                })
                .map(|range| SourceLocation {
                    path: path.to_owned(),
                    range: *range,
                })
                .collect();
        }
        let Some(target) = self.target_at(path, position, overlays) else {
            return Vec::new();
        };
        let mut locations = Vec::new();
        for layer in &self.layers {
            for (overlay_path, overlay) in overlays.for_module(&layer.spec.name) {
                for reference in &overlay.parsed.references {
                    if reference.target == target {
                        locations.push(SourceLocation {
                            path: overlay_path.clone(),
                            range: reference.range,
                        });
                    }
                }
            }
            for record in layer.references_to(&target) {
                if !overlays.contains(record.path.as_ref()) {
                    locations.push(SourceLocation {
                        path: record.path.as_ref().clone(),
                        range: record.reference().range,
                    });
                }
            }
        }
        if include_declaration {
            locations.extend(
                self.resolve(&target, overlays)
                    .into_iter()
                    .filter(|definition| definition.packaged_entry.is_none())
                    .map(|definition| SourceLocation {
                        path: definition.path,
                        range: definition.definition.selection_range,
                    }),
            );
        }
        locations.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.range.start.line.cmp(&right.range.start.line))
                .then(left.range.start.character.cmp(&right.range.start.character))
        });
        locations.dedup();
        locations
    }

    /// Lists top-level declarations in one disk or open document.
    pub fn document_symbols(&self, path: &Path, overlays: &OverlaySet) -> Vec<Symbol> {
        let Some((module, file)) = self.file(path, overlays) else {
            return Vec::new();
        };
        file.definitions
            .iter()
            .map(|definition| Symbol {
                name: definition.name.clone(),
                kind: definition.kind.clone(),
                module: module.to_owned(),
                location: SourceLocation {
                    path: path.to_path_buf(),
                    range: definition.range,
                },
            })
            .collect()
    }

    /// Searches every visible declaration and preserves shadowed overrides.
    pub fn workspace_symbols(&self, query: &str, overlays: &OverlaySet) -> Vec<Symbol> {
        let query = query.to_ascii_lowercase();
        let mut symbols = Vec::new();
        let mut suppressed = BTreeSet::new();
        for (path, overlay) in &overlays.documents {
            suppressed.insert(path.clone());
            for definition in &overlay.parsed.definitions {
                if definition.name.to_ascii_lowercase().contains(&query) {
                    symbols.push(Symbol {
                        name: definition.name.clone(),
                        kind: definition.kind.clone(),
                        module: overlay.module.clone(),
                        location: SourceLocation {
                            path: path.clone(),
                            range: definition.selection_range,
                        },
                    });
                }
            }
        }
        for layer in self.layers.iter().rev() {
            for record in &layer.definitions {
                if !suppressed.contains(record.path.as_ref())
                    && record
                        .definition()
                        .name
                        .to_ascii_lowercase()
                        .contains(&query)
                {
                    symbols.push(Symbol {
                        name: record.definition().name.clone(),
                        kind: record.definition().kind.clone(),
                        module: layer.spec.name.clone(),
                        location: SourceLocation {
                            path: record.path.as_ref().clone(),
                            range: record.definition().selection_range,
                        },
                    });
                }
            }
        }
        symbols.truncate(self.max_workspace_symbols);
        symbols
    }

    /// Finds the semantic target under one source position.
    fn target_at(
        &self,
        path: &Path,
        position: Position,
        overlays: &OverlaySet,
    ) -> Option<SymbolTarget> {
        let (_, file) = self.file(path, overlays)?;
        if let Some(reference) = file
            .references
            .iter()
            .find(|reference| range_contains(reference.range, position))
        {
            return Some(reference.target.clone());
        }
        file.definitions
            .iter()
            .find(|definition| range_contains(definition.selection_range, position))
            .map(definition_target)
    }

    /// Returns the rule-local Osiris variable fact under one source position.
    fn osiris_variable_at<'a>(
        &'a self,
        path: &Path,
        position: Position,
        overlays: &'a OverlaySet,
    ) -> Option<OsirisVariableMatch<'a>> {
        let (_, file) = self.file(path, overlays)?;
        let osiris = file.osiris.as_ref()?;
        osiris.variables.iter().find_map(|variable| {
            variable
                .occurrence_facts
                .iter()
                .find(|occurrence| range_contains(occurrence.range, position))
                .map_or_else(
                    || {
                        variable
                            .occurrences
                            .iter()
                            .copied()
                            .find(|range| range_contains(*range, position))
                            .map(|occurrence_range| OsirisVariableMatch {
                                variable,
                                occurrence_range,
                            })
                    },
                    |occurrence| {
                        Some(OsirisVariableMatch {
                            variable,
                            occurrence_range: occurrence.range,
                        })
                    },
                )
        })
    }

    /// Renders one rule-local Osiris variable without claiming a declaration.
    fn osiris_variable_hover(
        &self,
        variable: OsirisVariableMatch<'_>,
        overlays: &OverlaySet,
    ) -> String {
        let mut markdown = HoverMarkup::new("Osiris variable", &variable.variable.name);
        // A cast at this occurrence is stronger than the inferred type of a
        // database column. Use the DB type only when no occurrence evidence
        // is available for this bound variable.
        let type_name = variable
            .evidence()
            .as_ref()
            .map(|evidence| evidence.type_name.clone())
            .or_else(|| {
                variable
                    .database_binding()
                    .and_then(|binding| self.osiris_database_binding_type(binding, overlays))
            });
        if let Some(type_name) = type_name {
            markdown = markdown.fact("Type", &type_name);
        }
        markdown.finish()
    }

    /// Resolves one DB-bound variable type from the shared database schema.
    ///
    /// A positive DB read can introduce a variable from a matching row. Its
    /// later engine-query uses are constraints, not type-producing evidence.
    /// Missing or conflicting writes therefore remain unknown rather than
    /// falling back to an input contract's expected type. The propagation
    /// pass separately restricts DB-derived roots to writes; reads and
    /// removals can still provide the established schema shown here.
    fn osiris_database_binding_type(
        &self,
        binding: &OsirisDatabaseBinding,
        overlays: &OverlaySet,
    ) -> Option<String> {
        let schema = self
            .osiris_database_schemas(overlays)
            .remove(&(binding.name.clone(), binding.arity))?;
        let column = schema.columns.get(usize::from(binding.column))?;
        if column.ambiguous || !column.conflicts.is_empty() {
            return None;
        }
        column
            .established
            .as_ref()
            .map(|observation| observation.type_name.clone())
    }

    /// Returns schema field documentation when the position is on a field name.
    fn field_at(&self, path: &Path, position: Position, overlays: &OverlaySet) -> Option<String> {
        let (_, file) = self.file(path, overlays)?;
        for definition in &file.definitions {
            for (name, range) in &definition.field_ranges {
                if range_contains(*range, position) {
                    let candidates = if let Some(schema_id) = &definition.schema_id {
                        self.schema.by_id.get(schema_id).into_iter().collect()
                    } else {
                        self.schema
                            .infer_legacy(path, Some(&definition.kind), &definition.fields)
                    };
                    for schema in candidates {
                        if let Some(field) = schema.field(name) {
                            let mut markdown = HoverMarkup::new("Field", name);
                            if let Some(field_type) = &field.field_type {
                                markdown = markdown.fact("Type", field_type);
                            }
                            if let Some(description) = &field.description {
                                markdown = markdown.prose(description);
                            }
                            return Some(markdown.finish());
                        }
                    }
                }
            }
        }
        None
    }

    /// Returns the active disk or overlay file and its owning module.
    fn file<'a>(
        &'a self,
        path: &Path,
        overlays: &'a OverlaySet,
    ) -> Option<(&'a str, &'a ParsedFile)> {
        if let Some(overlay) = overlays.get(path) {
            return Some((&overlay.module, &overlay.parsed));
        }
        self.layers.iter().find_map(|layer| {
            layer
                .file(path)
                .map(|file| (layer.spec.name.as_str(), file.as_ref()))
        })
    }

    /// Folds visible loose database occurrences in Story goal order.
    ///
    /// The module layers describe symbol precedence, not Osiris evaluation
    /// order. Open documents replace their disk records before this fold.
    pub(crate) fn osiris_database_schemas(
        &self,
        overlays: &OverlaySet,
    ) -> BTreeMap<(String, u16), OsirisDatabaseSchema> {
        let mut sources = Vec::new();
        for (rank, layer) in self.layers.iter().enumerate() {
            for (path, overlay) in overlays.for_module(&layer.spec.name) {
                if overlay.parsed.source.kind == SourceKind::Osiris {
                    sources.push(VisibleOsirisSource {
                        module: &layer.spec.name,
                        rank,
                        path: path.clone(),
                        file: &overlay.parsed,
                    });
                }
            }
            for (path, file) in &layer.files {
                if overlays.contains(path) || file.source.kind != SourceKind::Osiris {
                    continue;
                }
                sources.push(VisibleOsirisSource {
                    module: &layer.spec.name,
                    rank,
                    path: path.clone(),
                    file,
                });
            }
        }
        sources.sort_by(|left, right| {
            let left_goal = left
                .file
                .osiris
                .as_ref()
                .map_or("", |osiris| osiris.goal.as_str());
            let right_goal = right
                .file
                .osiris
                .as_ref()
                .map_or("", |osiris| osiris.goal.as_str());
            left_goal
                .cmp(right_goal)
                .then_with(|| left.path.cmp(&right.path))
        });

        let mut goal_source_counts = BTreeMap::<String, usize>::new();
        for source in &sources {
            if let Some(osiris) = &source.file.osiris {
                *goal_source_counts.entry(osiris.goal.clone()).or_default() += 1;
            }
        }

        // Build the immutable dependency graph once. A graph/SCC pass keeps
        // conflicts from oscillating between known and unknown states and
        // recomputes the complete closure when overlays change the source
        // set. Runtime bindings remain source-ordered parser facts; this pass
        // only supplies compile-time types to already-valid DB writes.
        let base_occurrences = ordered_osiris_database_occurrences(&sources);
        let available = propagatable_osiris_database_types(&base_occurrences, &goal_source_counts);
        let occurrences = apply_osiris_database_propagation(&base_occurrences, &available);
        fold_osiris_database_schemas(&occurrences, &goal_source_counts)
    }
}

/// Builds database occurrences from the effective source set in global Story
/// order. Only write arguments without intrinsic evidence receive a derived
/// type from a rule-local variable.
fn ordered_osiris_database_occurrences(
    sources: &[VisibleOsirisSource<'_>],
) -> Vec<OrderedDatabaseOccurrence> {
    let mut records = Vec::new();
    for source in sources {
        let Some(osiris) = &source.file.osiris else {
            continue;
        };
        for occurrence in &osiris.occurrences {
            let mut arguments = Vec::with_capacity(occurrence.arguments.len());
            let mut propagation_sources = Vec::with_capacity(occurrence.arguments.len());
            let argument_ranges = occurrence
                .arguments
                .iter()
                .map(|argument| argument.range)
                .collect::<Vec<_>>();
            for argument in &occurrence.arguments {
                let intrinsic =
                    argument
                        .evidence
                        .as_ref()
                        .map(|evidence| OsirisDatabaseTypeObservation {
                            path: source.path.clone(),
                            range: argument.range,
                            type_name: evidence.type_name.clone(),
                        });
                let (argument, propagation_source) = if occurrence.role != OsirisCallRole::Write {
                    (intrinsic, None)
                } else {
                    let variable = osiris_variable_occurrence_at(osiris, argument.range);
                    match variable {
                        None => (intrinsic, None),
                        Some(variable) if variable.binding_range.is_none() => {
                            // A cast or literal-like appearance around an
                            // unbound action variable cannot establish a DB
                            // schema. Keep only arguments with no local
                            // variable, such as typed literals.
                            (None, None)
                        }
                        Some(variable)
                            if argument.range == variable.range
                                && variable.database_binding.is_some() =>
                        {
                            // This is a plain DB-bound local. Explicit type
                            // evidence established later in the rule is a
                            // valid local type and takes precedence over the
                            // DB edge. Without it, the source column supplies
                            // the type through the dependency graph.
                            let binding = variable.database_binding.as_ref().unwrap();
                            if intrinsic.is_some() {
                                (intrinsic, None)
                            } else {
                                (
                                    None,
                                    Some((
                                        binding.name.clone(),
                                        binding.arity,
                                        usize::from(binding.column),
                                    )),
                                )
                            }
                        }
                        Some(_) => {
                            // A previously valid event/query local, or an
                            // explicitly cast action argument, is direct
                            // evidence. It does not create a DB dependency.
                            (intrinsic, None)
                        }
                    }
                };
                arguments.push(argument);
                propagation_sources.push(propagation_source);
            }
            records.push(OrderedDatabaseOccurrence {
                name: occurrence.name.clone(),
                arity: occurrence.arity,
                goal: osiris.goal.clone(),
                module: source.module.to_owned(),
                rank: source.rank,
                path: source.path.clone(),
                role: occurrence.role,
                argument_ranges,
                arguments,
                propagation_sources,
            });
        }
    }
    records
}

fn osiris_variable_occurrence_at(
    osiris: &bg3_index::OsirisFile,
    argument_range: TextRange,
) -> Option<&OsirisVariableOccurrence> {
    osiris
        .variables
        .iter()
        .flat_map(|variable| &variable.occurrence_facts)
        .find(|occurrence| range_covers(argument_range, occurrence.range))
}

fn fold_osiris_database_schemas(
    occurrences: &[OrderedDatabaseOccurrence],
    goal_source_counts: &BTreeMap<String, usize>,
) -> BTreeMap<(String, u16), OsirisDatabaseSchema> {
    let mut schemas = BTreeMap::<(String, u16), OsirisDatabaseSchema>::new();
    let mut by_database = BTreeMap::<(String, u16), Vec<&OrderedDatabaseOccurrence>>::new();
    for occurrence in occurrences {
        let key = (occurrence.name.clone(), occurrence.arity);
        let schema = schemas
            .entry(key.clone())
            .or_insert_with(|| OsirisDatabaseSchema {
                name: key.0.clone(),
                arity: key.1,
                columns: (0..usize::from(key.1))
                    .map(|_| OsirisDatabaseColumnSchema::default())
                    .collect(),
                reads: 0,
                writes: 0,
                removals: 0,
                contributors: BTreeSet::new(),
            });
        match occurrence.role {
            OsirisCallRole::Read => schema.reads += 1,
            OsirisCallRole::Write => schema.writes += 1,
            OsirisCallRole::Remove => schema.removals += 1,
        }
        schema.contributors.insert((
            occurrence.rank,
            occurrence.module.clone(),
            occurrence.goal.clone(),
            occurrence.path.clone(),
        ));
        by_database.entry(key).or_default().push(occurrence);
    }

    for (key, records) in by_database {
        let Some(schema) = schemas.get_mut(&key) else {
            continue;
        };
        fold_osiris_database_records(schema, &records, goal_source_counts);
    }
    schemas
}

fn fold_osiris_database_records(
    schema: &mut OsirisDatabaseSchema,
    records: &[&OrderedDatabaseOccurrence],
    goal_source_counts: &BTreeMap<String, usize>,
) {
    let mut start = 0;
    while start < records.len() {
        let goal = &records[start].goal;
        let mut end = start + 1;
        while end < records.len() && records[end].goal == *goal {
            end += 1;
        }
        if goal_source_counts.get(goal).copied().unwrap_or_default() > 1 {
            // Equal-name goals have no language-defined ordering. A
            // disagreement in the group is therefore ambiguous.
            for column in 0..usize::from(schema.arity) {
                let mut known = Vec::<OsirisDatabaseTypeObservation>::new();
                for record in &records[start..end] {
                    if let Some(Some(observation)) = record.arguments.get(column) {
                        add_osiris_type_observation(&mut known, observation.clone());
                    }
                }
                let Some(column_schema) = schema.columns.get_mut(column) else {
                    continue;
                };
                if !known.is_empty() && osiris_type_observations_resolve(&known).is_none() {
                    column_schema.established = None;
                    column_schema.conflicts.clear();
                    column_schema.ambiguous = true;
                } else if !column_schema.ambiguous {
                    for observation in known {
                        fold_osiris_database_type(column_schema, observation);
                    }
                }
            }
        } else {
            for record in &records[start..end] {
                // Every proven argument can contribute compile-time signature
                // evidence, including reads and removals.
                for (column, observation) in record.arguments.iter().enumerate() {
                    if let Some(observation) = observation
                        && let Some(column_schema) = schema.columns.get_mut(column)
                    {
                        fold_osiris_database_type(column_schema, observation.clone());
                    }
                }
            }
        }
        start = end;
    }
}

fn propagatable_osiris_database_types(
    occurrences: &[OrderedDatabaseOccurrence],
    goal_source_counts: &BTreeMap<String, usize>,
) -> BTreeMap<OsirisDatabaseColumnKey, Option<String>> {
    let mut states = BTreeMap::<OsirisDatabaseColumnKey, Vec<String>>::new();
    let mut ambiguous = BTreeSet::<OsirisDatabaseColumnKey>::new();
    let mut outgoing =
        BTreeMap::<OsirisDatabaseColumnKey, BTreeSet<OsirisDatabaseColumnKey>>::new();
    let mut direct_by_goal = BTreeMap::<(OsirisDatabaseColumnKey, String), Vec<String>>::new();

    for occurrence in occurrences {
        for column in 0..usize::from(occurrence.arity) {
            states
                .entry((occurrence.name.clone(), occurrence.arity, column))
                .or_default();
        }
        if occurrence.role != OsirisCallRole::Write {
            continue;
        }
        for column in 0..usize::from(occurrence.arity) {
            let target = (occurrence.name.clone(), occurrence.arity, column);
            if let Some(Some(observation)) = occurrence.arguments.get(column) {
                add_osiris_type_candidate(
                    states.entry(target.clone()).or_default(),
                    observation.type_name.clone(),
                );
                add_osiris_type_candidate(
                    direct_by_goal
                        .entry((target.clone(), occurrence.goal.clone()))
                        .or_default(),
                    observation.type_name.clone(),
                );
            }
            if let Some(Some(source)) = occurrence.propagation_sources.get(column) {
                outgoing
                    .entry(source.clone())
                    .or_default()
                    .insert(target.clone());
                states.entry(source.clone()).or_default();
            }
        }
    }

    // Duplicate visible sources with the same goal name have no language
    // ordering. Only incompatible direct write evidence is ambiguous; equal
    // evidence remains usable.
    for ((column, goal), types) in direct_by_goal {
        if goal_source_counts.get(&goal).copied().unwrap_or_default() > 1
            && osiris_type_candidates_resolve(&types).is_none()
        {
            ambiguous.insert(column);
        }
    }

    // Condense the dependency graph into strongly connected components. A
    // component receives direct roots and singleton outputs from predecessor
    // components. This gives rooted cycles a type, leaves unrooted cycles
    // unknown, and prevents a conflicting component from contaminating an
    // independently rooted downstream database.
    let keys = states.keys().cloned().collect::<Vec<_>>();
    let mut reverse = BTreeMap::<OsirisDatabaseColumnKey, BTreeSet<OsirisDatabaseColumnKey>>::new();
    for (source, targets) in &outgoing {
        for target in targets {
            reverse
                .entry(target.clone())
                .or_default()
                .insert(source.clone());
        }
    }
    let mut visited = BTreeSet::new();
    let mut order = Vec::new();
    for key in &keys {
        osiris_database_dfs_order(key, &outgoing, &mut visited, &mut order);
    }
    let mut component_of = BTreeMap::<OsirisDatabaseColumnKey, usize>::new();
    let mut components = Vec::<Vec<OsirisDatabaseColumnKey>>::new();
    for key in order.into_iter().rev() {
        if component_of.contains_key(&key) {
            continue;
        }
        let index = components.len();
        let mut component = Vec::new();
        osiris_database_dfs_component(&key, &reverse, &mut component_of, index, &mut component);
        components.push(component);
    }

    let mut predecessors = vec![BTreeSet::<usize>::new(); components.len()];
    let mut successors = vec![BTreeSet::<usize>::new(); components.len()];
    for (source, targets) in &outgoing {
        let source_component = component_of[source];
        for target in targets {
            let target_component = component_of[target];
            if source_component != target_component {
                predecessors[target_component].insert(source_component);
                successors[source_component].insert(target_component);
            }
        }
    }
    let mut indegree = predecessors.iter().map(BTreeSet::len).collect::<Vec<_>>();
    let mut ready = (0..components.len())
        .filter(|component| indegree[*component] == 0)
        .collect::<BTreeSet<_>>();
    let mut component_order = Vec::with_capacity(components.len());
    while let Some(component) = ready.pop_first() {
        component_order.push(component);
        for successor in &successors[component] {
            indegree[*successor] -= 1;
            if indegree[*successor] == 0 {
                ready.insert(*successor);
            }
        }
    }

    let mut component_types = vec![None::<String>; components.len()];
    for component in component_order {
        let mut candidates = Vec::new();
        let mut blocked = false;
        for key in &components[component] {
            for type_name in &states[key] {
                add_osiris_type_candidate(&mut candidates, type_name.clone());
            }
            blocked |= ambiguous.contains(key);
        }
        for predecessor in &predecessors[component] {
            if let Some(type_name) = &component_types[*predecessor] {
                add_osiris_type_candidate(&mut candidates, type_name.clone());
            }
        }
        if !blocked {
            component_types[component] = osiris_type_candidates_resolve(&candidates);
        }
    }

    states
        .into_keys()
        .map(|key| {
            let component = component_of[&key];
            (key, component_types[component].clone())
        })
        .collect()
}

fn osiris_database_dfs_order(
    key: &OsirisDatabaseColumnKey,
    outgoing: &BTreeMap<OsirisDatabaseColumnKey, BTreeSet<OsirisDatabaseColumnKey>>,
    visited: &mut BTreeSet<OsirisDatabaseColumnKey>,
    order: &mut Vec<OsirisDatabaseColumnKey>,
) {
    if !visited.insert(key.clone()) {
        return;
    }
    if let Some(targets) = outgoing.get(key) {
        for target in targets {
            osiris_database_dfs_order(target, outgoing, visited, order);
        }
    }
    order.push(key.clone());
}

fn osiris_database_dfs_component(
    key: &OsirisDatabaseColumnKey,
    reverse: &BTreeMap<OsirisDatabaseColumnKey, BTreeSet<OsirisDatabaseColumnKey>>,
    component_of: &mut BTreeMap<OsirisDatabaseColumnKey, usize>,
    component: usize,
    members: &mut Vec<OsirisDatabaseColumnKey>,
) {
    if component_of.contains_key(key) {
        return;
    }
    component_of.insert(key.clone(), component);
    members.push(key.clone());
    if let Some(sources) = reverse.get(key) {
        for source in sources {
            osiris_database_dfs_component(source, reverse, component_of, component, members);
        }
    }
}

fn apply_osiris_database_propagation(
    occurrences: &[OrderedDatabaseOccurrence],
    available: &BTreeMap<OsirisDatabaseColumnKey, Option<String>>,
) -> Vec<OrderedDatabaseOccurrence> {
    occurrences
        .iter()
        .cloned()
        .map(|mut occurrence| {
            if occurrence.role == OsirisCallRole::Write {
                for column in 0..usize::from(occurrence.arity) {
                    if occurrence.arguments[column].is_some() {
                        continue;
                    }
                    let Some(Some(source)) = occurrence.propagation_sources.get(column) else {
                        continue;
                    };
                    let Some(Some(type_name)) = available.get(source) else {
                        continue;
                    };
                    occurrence.arguments[column] = Some(OsirisDatabaseTypeObservation {
                        path: occurrence.path.clone(),
                        range: occurrence.argument_ranges[column],
                        type_name: type_name.clone(),
                    });
                }
            }
            occurrence
        })
        .collect()
}

fn range_covers(outer: TextRange, inner: TextRange) -> bool {
    (outer.start.line, outer.start.character) <= (inner.start.line, inner.start.character)
        && (inner.end.line, inner.end.character) <= (outer.end.line, outer.end.character)
}

/// Retains each distinct candidate. Compatibility is not transitive for the
/// GUID family: `GUIDSTRING` matches both `CHARACTER` and `ITEM`, while those
/// specialized aliases do not match each other. The complete candidate set is
/// therefore required before a type can be selected.
fn add_osiris_type_candidate(candidates: &mut Vec<String>, type_name: String) {
    if candidates.iter().any(|candidate| candidate == &type_name) {
        return;
    }
    candidates.push(type_name);
}

/// Retains one source location for each distinct type in an equal-order goal
/// group. A later schema fold still records every retained type so ordered
/// observations can report the first proven pairwise mismatch.
fn add_osiris_type_observation(
    observations: &mut Vec<OsirisDatabaseTypeObservation>,
    observation: OsirisDatabaseTypeObservation,
) {
    if observations
        .iter()
        .any(|existing| existing.type_name == observation.type_name)
    {
        return;
    }
    observations.push(observation);
}

/// Returns the first candidate when every distinct candidate is pairwise
/// proven compatible. Unknown relationships and proven mismatches both stay
/// unresolved so propagation cannot make an unsafe choice.
fn osiris_type_candidates_resolve(candidates: &[String]) -> Option<String> {
    for (index, left) in candidates.iter().enumerate() {
        for right in &candidates[index + 1..] {
            if osiris_type_compatibility(left, right) != Some(true) {
                return None;
            }
        }
    }
    candidates.first().cloned()
}

fn osiris_type_observations_resolve(
    observations: &[OsirisDatabaseTypeObservation],
) -> Option<String> {
    let types = observations
        .iter()
        .map(|observation| observation.type_name.clone())
        .collect::<Vec<_>>();
    osiris_type_candidates_resolve(&types)
}

fn fold_osiris_database_type(
    column: &mut OsirisDatabaseColumnSchema,
    observation: OsirisDatabaseTypeObservation,
) {
    if column.ambiguous {
        return;
    }
    if column.established.is_none() {
        column.established = Some(observation.clone());
    } else if column.observations.iter().any(|existing| {
        osiris_type_compatibility(&existing.type_name, &observation.type_name) != Some(true)
    }) {
        column.conflicts.push(observation.clone());
    }
    column.observations.push(observation);
}

pub(crate) fn osiris_database_parameter_types(schema: &OsirisDatabaseSchema) -> Vec<String> {
    schema
        .columns
        .iter()
        .map(|column| {
            if column.ambiguous {
                "conflicting".into()
            } else {
                column.established.as_ref().map_or_else(
                    || "unknown".into(),
                    |observation| observation.type_name.clone(),
                )
            }
        })
        .collect()
}

/// Tests whether one position is inside a half-open source range.
pub fn range_contains(range: TextRange, position: Position) -> bool {
    let after_start =
        (position.line, position.character) >= (range.start.line, range.start.character);
    let before_end = (position.line, position.character) < (range.end.line, range.end.character);
    after_start && before_end
}

/// Orders source ranges without overflowing on multiline spans.
fn hover_range_size(range: &TextRange) -> (u32, u32, u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range.end.character.saturating_sub(range.start.character),
        range.start.line,
        range.start.character,
    )
}

/// Tests a declaration against a semantic target.
fn definition_matches(definition: &Definition, target: &SymbolTarget) -> bool {
    match target {
        SymbolTarget::Named {
            kind: Some(kind),
            name,
        } => {
            (canonical_kind(&definition.kind) == canonical_kind(kind) && definition.name == *name)
                || definition.aliases.contains(name)
        }
        SymbolTarget::Named { kind: None, name } => {
            definition.name == *name || definition.aliases.contains(name)
        }
        SymbolTarget::Tooltip { .. } => false,
        SymbolTarget::Uuid(uuid) => definition.uuid == Some(*uuid),
        SymbolTarget::OsirisGoal { name } => {
            definition.kind == OSIRIS_GOAL_KIND && definition.name == *name
        }
        SymbolTarget::OsirisCallable { name, arity } => {
            matches!(
                definition.kind.as_str(),
                OSIRIS_PROCEDURE_KIND | OSIRIS_QUERY_KIND
            ) && definition.name == *name
                && definition.arity == Some(*arity)
        }
        SymbolTarget::OsirisDatabase { name, arity } => {
            definition.kind == OSIRIS_DATABASE_KIND
                && definition.name == *name
                && definition.arity == Some(*arity)
        }
    }
}

/// Returns the semantic target represented by one declaration.
pub fn definition_target(definition: &Definition) -> SymbolTarget {
    match (definition.kind.as_str(), definition.arity) {
        (OSIRIS_GOAL_KIND, _) => {
            return SymbolTarget::OsirisGoal {
                name: definition.name.clone(),
            };
        }
        (OSIRIS_PROCEDURE_KIND | OSIRIS_QUERY_KIND, Some(arity)) => {
            return SymbolTarget::OsirisCallable {
                name: definition.name.clone(),
                arity,
            };
        }
        (OSIRIS_DATABASE_KIND, Some(arity)) => {
            return SymbolTarget::OsirisDatabase {
                name: definition.name.clone(),
                arity,
            };
        }
        _ => {}
    }
    SymbolTarget::Named {
        kind: Some(definition.kind.clone()),
        name: definition.name.clone(),
    }
}

/// Returns whether two references identify the same semantic target.
pub fn references_same_target(left: &Reference, right: &Reference) -> bool {
    left.target == right.target
}

/// Converts common Larian presentation tags to safe readable Markdown text.
fn render_localized_text(source: &str) -> String {
    let mut plain = String::with_capacity(source.len());
    let mut remaining = source;
    while let Some(open) = remaining.find('<') {
        plain.push_str(&remaining[..open]);
        let Some(close) = remaining[open..].find('>') else {
            plain.push_str(&remaining[open..]);
            remaining = "";
            break;
        };
        let tag = remaining[open + 1..open + close]
            .trim()
            .to_ascii_lowercase();
        if tag.starts_with("br") || tag.starts_with("/p") {
            plain.push('\n');
        }
        remaining = &remaining[open + close + 1..];
    }
    plain.push_str(remaining);

    let unescaped = plain
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'");
    bounded_markdown_text(&unescaped)
}

/// Escapes external plain text without interpreting it as hover Markdown.
fn escape_markdown_text(source: &str) -> String {
    let mut markdown = String::with_capacity(source.len());
    let mut line_start = true;
    let mut line_digits_only = true;
    let mut line_has_character = false;
    for character in source.chars() {
        let ordered_list_marker = character == '.' && line_digits_only && line_has_character;
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '#' | '<' | '>'
        ) || (line_start && matches!(character, '-' | '+'))
            || ordered_list_marker
        {
            markdown.push('\\');
        }
        markdown.push(character);
        if character == '\n' {
            line_start = true;
            line_digits_only = true;
            line_has_character = false;
        } else {
            line_start = false;
            line_has_character = true;
            if !character.is_ascii_digit() {
                line_digits_only = false;
            }
        }
    }
    markdown
}

fn osiris_contract_kind_label(kind: OsirisContractKind) -> &'static str {
    match kind {
        OsirisContractKind::Call => "Osiris engine call",
        OsirisContractKind::Event => "Osiris engine event",
        OsirisContractKind::Query => "Osiris engine query",
        OsirisContractKind::Syscall => "Osiris engine syscall",
        OsirisContractKind::Sysquery => "Osiris engine sysquery",
    }
}

fn osiris_contract_signature_markdown(
    name: &str,
    contract: &bg3_index::OsirisContractSpec,
) -> String {
    let parameters = contract
        .parameters
        .iter()
        .map(|parameter| {
            format!(
                "{} {} {}",
                osiris_parameter_direction(parameter.direction),
                parameter.type_name,
                parameter.name
            )
        })
        .collect::<Vec<_>>();
    let compact = format!("{name}({})", parameters.join(", "));
    let signature = if compact.chars().count() <= 80 {
        compact
    } else {
        let mut expanded = format!("{name}(\n");
        for (index, parameter) in parameters.iter().enumerate() {
            let comma = if index + 1 == parameters.len() {
                ""
            } else {
                ","
            };
            expanded.push_str(&format!("    {parameter}{comma}\n"));
        }
        expanded.push(')');
        expanded
    };
    format!("```bg3_osiris\n{signature}\n```")
}

fn osiris_parameter_direction(direction: OsirisParameterDirection) -> &'static str {
    match direction {
        OsirisParameterDirection::In => "[in]",
        OsirisParameterDirection::InOut => "[inout]",
        OsirisParameterDirection::Out => "[out]",
    }
}

/// Escapes external text and bounds its contribution to one hover.
fn bounded_markdown_text(source: &str) -> String {
    let total = source.chars().count();
    let rendered: String = source.chars().take(MAX_RENDERED_FIELD_CHARACTERS).collect();
    let mut markdown = escape_markdown_text(&rendered);
    if total > MAX_RENDERED_FIELD_CHARACTERS {
        markdown.push_str(&format!(
            "… *({} more characters)*",
            total - MAX_RENDERED_FIELD_CHARACTERS
        ));
    }
    markdown
}

/// Returns the length of the longest backtick run in one source string.
fn longest_backtick_run(source: &str) -> usize {
    source
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0)
}

/// Wraps raw source in a Markdown fence that cannot collide with its backticks.
pub(crate) fn markdown_inline_code(source: &str) -> String {
    let fence = "`".repeat(longest_backtick_run(source) + 1);
    let padding = if source.starts_with(['`', ' ']) || source.ends_with(['`', ' ']) {
        " "
    } else {
        ""
    };
    format!("{fence}{padding}{source}{padding}{fence}")
}

/// Renders one external value as bounded inline code with an omission note.
fn bounded_inline_code(source: &str) -> String {
    let total = source.chars().count();
    if total <= MAX_RENDERED_FIELD_CHARACTERS {
        return markdown_inline_code(source);
    }
    let rendered: String = source.chars().take(MAX_RENDERED_FIELD_CHARACTERS).collect();
    format!(
        "{}… *({} more characters)*",
        markdown_inline_code(&rendered),
        total - MAX_RENDERED_FIELD_CHARACTERS
    )
}

/// Maximum rendered characters before one stored field value is elided.
const MAX_RENDERED_FIELD_CHARACTERS: usize = 160;

/// Maximum repeated records shown in one hover section.
pub(crate) const MAX_HOVER_LIST_ENTRIES: usize = 12;

/// Maximum fields reconstructed into one Stats source hover block.
const MAX_HOVER_SOURCE_FIELDS: usize = 64;

fn append_omitted_entries(markdown: &mut String, omitted: usize) {
    if omitted > 0 {
        markdown.push_str(&format!("\n\n- … {omitted} additional entries omitted"));
    }
}

/// Legacy Stats declarations whose header shape is not `new entry`.
const NAMED_BLOCK_KINDS: [&str; 5] = [
    "TreasureTable",
    "Equipment",
    "SpellSet",
    "ItemGroup",
    "NameGroup",
];

/// Stats fields that describe presentation, not inspectable behavior.
const PRESENTATION_ONLY_FIELDS: [&str; 8] = [
    "CastEffect",
    "DualWieldingSpellAnimation",
    "HitAnimationType",
    "PreviewCursor",
    "Sheathing",
    "SpellAnimation",
    "SpellAnimationIntentType",
    "SpellSoundMagnitude",
];

/// Renders one stored field value and elides values that would dominate hover.
///
/// The complete value stays available through definition navigation, so the
/// elision note only marks how much of the stored value is hidden.
fn field_value_markdown(value: &str) -> String {
    let total = value.chars().count();
    if total <= MAX_RENDERED_FIELD_CHARACTERS {
        return markdown_inline_code(value);
    }
    let rendered: String = value.chars().take(MAX_RENDERED_FIELD_CHARACTERS).collect();
    format!(
        "{}… *({} more characters)*",
        markdown_inline_code(&rendered),
        total - MAX_RENDERED_FIELD_CHARACTERS
    )
}

/// Cuts one stored Stats value after its last complete top-level statement.
///
/// Elision appends the Unicode ellipsis character, which `tree-sitter-bg3`
/// 0.5.0 accepts as a placeholder, so the fragment stays valid Stats-value
/// syntax for editor highlighting. A value without a depth-zero `;` cut point
/// renders only the ellipsis instead of broken syntax.
fn clamp_stats_value(value: &str) -> String {
    if value.chars().count() <= MAX_RENDERED_FIELD_CHARACTERS {
        return value.to_owned();
    }
    let mut depth = 0usize;
    let mut cut = None;
    for (rendered, (end, character)) in value.char_indices().enumerate() {
        if rendered >= MAX_RENDERED_FIELD_CHARACTERS {
            break;
        }
        match character {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => cut = Some(end + character.len_utf8()),
            _ => {}
        }
    }
    match cut {
        Some(cut) => format!("{}…", &value[..cut]),
        None => "…".to_owned(),
    }
}

/// Renders one path for human-readable hover markdown.
///
/// Hover text replaces the home directory prefix with `~` so long absolute
/// paths stay readable. Navigation targets keep full paths.
fn display_path(path: &Path) -> String {
    abbreviate_home(path, std::env::home_dir().as_deref())
}

/// Returns the identifier-like token under a UTF-8 source position.
fn lexical_hover_range(source: &str, position: Position) -> Option<TextRange> {
    let line = source
        .split('\n')
        .nth(usize::try_from(position.line).ok()?)?;
    let cursor = usize::try_from(position.character).ok()?.min(line.len());
    let is_token = |character: char| character.is_alphanumeric() || matches!(character, '_' | '-');
    let start = line[..cursor]
        .char_indices()
        .rev()
        .find(|(_, character)| !is_token(*character))
        .map_or(0, |(index, character)| index + character.len_utf8());
    let end = line[cursor..]
        .char_indices()
        .find(|(_, character)| !is_token(*character))
        .map_or(line.len(), |(index, _)| cursor + index);
    (start < end).then_some(TextRange {
        start: Position {
            line: position.line,
            character: u32::try_from(start).ok()?,
        },
        end: Position {
            line: position.line,
            character: u32::try_from(end).ok()?,
        },
    })
}

/// Replaces an exact home-directory prefix with `~`.
///
/// A path must start a segment boundary inside the home directory to count;
/// unrelated absolute and relative paths render unchanged.
fn abbreviate_home(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.display().to_string();
    };
    let Some(rest) = path.strip_prefix(home).ok() else {
        return path.display().to_string();
    };
    if rest.as_os_str().is_empty() {
        "~".to_owned()
    } else {
        format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bg3_index::PackagedThothSource;

    #[test]
    fn packaged_facts_are_replaced_without_mutating_an_existing_snapshot() {
        let source = PackagedThothSource::new(
            "Example",
            "/synthetic/Example.pak",
            "Mods/Example/Scripts/thoth/helpers/base.khn",
            0,
            "function Base() end",
        )
        .expect("synthetic package source");
        let catalog = PackagedThothCatalog::from_sources([source]).expect("synthetic catalog");
        let facts = parse_packaged_thoth_facts(&catalog, "test-v1", |_| {
            Ok::<_, bg3_index::Error>(ThothFile::default())
        })
        .expect("synthetic facts");
        let workspace =
            WorkspaceSnapshot::new(Arc::new(SchemaCatalog::default()), Vec::new(), 1, 100, 100)
                .with_packaged_thoth_facts(Arc::new(facts));
        let old_facts = workspace.packaged_thoth_facts();

        let replacement = empty_packaged_thoth_facts();
        let next = workspace.with_packaged_thoth_facts(Arc::new(replacement));

        assert_eq!(old_facts.len(), 1);
        assert_eq!(next.packaged_thoth_facts_count(), 0);
        assert!(!Arc::ptr_eq(&old_facts, &next.packaged_thoth_facts()));
    }

    #[test]
    fn abbreviates_only_exact_home_directory_prefixes() {
        let home = Path::new("/Users/td");
        let render = |path: &str| abbreviate_home(Path::new(path), Some(home));

        assert_eq!(
            render("/Users/td/Mods/X/Stats/a.txt"),
            "~/Mods/X/Stats/a.txt"
        );
        assert_eq!(render("/Users/td"), "~");
        assert_eq!(render("/Users/tdx/Mods"), "/Users/tdx/Mods");
        assert_eq!(render("/synthetic/MyMod/a.khn"), "/synthetic/MyMod/a.khn");
        assert_eq!(render("relative/a.txt"), "relative/a.txt");
        assert_eq!(
            abbreviate_home(Path::new("/Users/td/a.txt"), None),
            "/Users/td/a.txt"
        );
    }

    #[test]
    fn inline_code_fences_are_longer_than_embedded_backticks() {
        assert_eq!(markdown_inline_code("plain"), "`plain`");
        assert_eq!(markdown_inline_code("`quoted`"), "`` `quoted` ``");
        assert_eq!(markdown_inline_code("``"), "``` `` ```");
    }

    #[test]
    fn external_hover_text_escapes_markdown_controls() {
        assert_eq!(
            escape_markdown_text("*bold* [link](https://example.test) <tag>"),
            "\\*bold\\* \\[link\\]\\(https://example.test\\) \\<tag\\>"
        );
    }

    #[test]
    fn hover_markup_separates_trusted_markdown_from_external_prose() {
        let trusted = HoverMarkup::new("Kind", "name")
            .markdown("**trusted** `syntax`")
            .finish();
        let external = HoverMarkup::new("Kind", "name")
            .prose("**external** `syntax`")
            .finish();

        assert!(trusted.contains("**trusted** `syntax`"));
        assert!(external.contains("\\*\\*external\\*\\* \\`syntax\\`"));
        assert!(!external.contains("**external** `syntax`"));
    }

    #[test]
    fn localized_hover_text_is_bounded() {
        let source = format!("{} tail", "x".repeat(MAX_RENDERED_FIELD_CHARACTERS + 10));
        let rendered = render_localized_text(&source);

        assert!(rendered.starts_with(&"x".repeat(MAX_RENDERED_FIELD_CHARACTERS)));
        assert!(rendered.ends_with("… *(15 more characters)*"));
    }

    #[test]
    fn field_values_are_bounded_without_breaking_inline_code() {
        let source = format!("{} `tail`", "x".repeat(MAX_RENDERED_FIELD_CHARACTERS + 10));
        let rendered = field_value_markdown(&source);

        assert!(rendered.starts_with('`'));
        assert!(rendered.contains("… *(17 more characters)*"));
        assert!(rendered.ends_with("*"));
    }

    #[test]
    fn range_contains_uses_half_open_source_ranges() {
        let range = TextRange {
            start: Position {
                line: 2,
                character: 4,
            },
            end: Position {
                line: 2,
                character: 9,
            },
        };

        assert!(range_contains(
            range,
            Position {
                line: 2,
                character: 4
            }
        ));
        assert!(range_contains(
            range,
            Position {
                line: 2,
                character: 8
            }
        ));
        assert!(!range_contains(
            range,
            Position {
                line: 2,
                character: 9
            }
        ));
        assert!(!range_contains(
            range,
            Position {
                line: 2,
                character: 3
            }
        ));
    }

    #[test]
    fn hover_range_size_handles_multiline_ranges_without_underflow() {
        let range = TextRange {
            start: Position {
                line: 3,
                character: 40,
            },
            end: Position {
                line: 4,
                character: 5,
            },
        };

        assert_eq!(hover_range_size(&range), (1, 0, 3, 40));
    }
}
