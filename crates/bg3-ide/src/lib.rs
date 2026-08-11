//! Editor-neutral language operations over immutable BG3 module indexes.

mod diagnostics;
mod language;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_index::{
    Definition, LocalizationCatalog, ModuleIndex, ModuleSpec, OSIRIS_DATABASE_KIND,
    OSIRIS_GOAL_KIND, OSIRIS_PROCEDURE_KIND, OSIRIS_QUERY_KIND, OsirisCallRole, ParsedFile,
    Position, Reference, SchemaCatalog, SymbolTarget, THOTH_FUNCTION_KIND, TextRange,
    TooltipCatalog, canonical_kind,
};

pub use diagnostics::{Diagnostic, DiagnosticSeverity};
pub use language::{CompletionItem, CompletionKind, CompletionList, SignatureHelp};

/// A definition result with the module that contributes its precedence.
#[derive(Clone, Debug)]
pub struct ResolvedDefinition {
    pub module: String,
    pub rank: usize,
    pub path: PathBuf,
    pub definition: Definition,
    pub ambiguous: bool,
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
    tooltips: Arc<TooltipCatalog>,
    incomplete_kinds: BTreeSet<String>,
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
    pub fn resolve(&self, target: &SymbolTarget, overlays: &OverlaySet) -> Vec<ResolvedDefinition> {
        let mut resolved = Vec::new();
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
                    });
                }
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

    /// Returns a rich Markdown description for the symbol under one position.
    pub fn hover(&self, path: &Path, position: Position, overlays: &OverlaySet) -> Option<String> {
        if let Some(field) = self.field_at(path, position, overlays) {
            return Some(field);
        }
        let target = self.target_at(path, position, overlays)?;
        if let SymbolTarget::Tooltip { name } = &target {
            return self.tooltip_tag_hover(name, overlays);
        }
        if let SymbolTarget::OsirisDatabase { .. } = &target {
            return self.osiris_database_hover(&target, overlays);
        }
        let definitions = self.resolve(&target, overlays);
        let effective = definitions.first()?;
        let heading = match effective.definition.kind.as_str() {
            THOTH_FUNCTION_KIND => "Thoth function",
            OSIRIS_GOAL_KIND => "Osiris goal",
            OSIRIS_PROCEDURE_KIND => "Osiris procedure",
            OSIRIS_QUERY_KIND => "Osiris query",
            _ => &effective.definition.kind,
        };
        let mut markdown = format!(
            "**{heading}** `{}`\n\nModule: `{}`",
            effective.definition.name, effective.module
        );
        if let Some(uuid) = effective.definition.uuid {
            markdown.push_str(&format!("\n\nUUID: `{uuid}`"));
        }
        if let Some(parent) = &effective.definition.parent {
            markdown.push_str(&format!("\n\nParent: `{parent}`"));
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
            markdown.push_str(&format!(
                "\n\nSignature: `{}({parameters})`",
                effective.definition.name
            ));
        }
        for key in ["DisplayName", "Description", "Text", "Boosts"] {
            if let Some(value) = effective.definition.fields.get(key) {
                markdown.push_str(&format!("\n\n- **{key}:** `{value}`"));
            }
        }
        markdown.push_str(&format!("\n\nSource: `{}`", effective.path.display()));
        if definitions.len() > 1 {
            markdown.push_str("\n\n**Override chain**\n");
            for definition in &definitions {
                let ambiguity = if definition.ambiguous {
                    " — same-rank ambiguity"
                } else {
                    ""
                };
                markdown.push_str(&format!(
                    "\n- `{}` — `{}`{}",
                    definition.module,
                    definition.path.display(),
                    ambiguity
                ));
            }
        }
        if let Some(preview) = self.tooltip_preview(&definitions, overlays) {
            markdown.push_str(&preview);
        }
        Some(markdown)
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
        let mut reads = 0;
        let mut writes = 0;
        let mut types = vec![BTreeSet::<String>::new(); usize::from(*arity)];
        let mut contributors = BTreeSet::new();
        for (rank, layer) in self.layers.iter().enumerate() {
            for (path, overlay) in overlays.for_module(&layer.spec.name) {
                collect_osiris_database_evidence(
                    &overlay.parsed,
                    name,
                    *arity,
                    rank,
                    &layer.spec.name,
                    path,
                    &mut reads,
                    &mut writes,
                    &mut types,
                    &mut contributors,
                );
            }
            for (path, file) in &layer.files {
                if overlays.contains(path) {
                    continue;
                }
                collect_osiris_database_evidence(
                    file,
                    name,
                    *arity,
                    rank,
                    &layer.spec.name,
                    path,
                    &mut reads,
                    &mut writes,
                    &mut types,
                    &mut contributors,
                );
            }
        }
        if reads + writes == 0 {
            return None;
        }
        let parameters: Vec<_> = types
            .into_iter()
            .map(|types| {
                if types.len() == 1 {
                    types.into_iter().next().unwrap_or_else(|| "unknown".into())
                } else if types.len() > 1 {
                    "conflicting".into()
                } else {
                    "unknown".into()
                }
            })
            .collect();
        let mut markdown = format!(
            "**Osiris database** `{name}/{arity}`\n\nSignature: `{}({})`\n\nWrites: {writes}\n\nReads: {reads}",
            name,
            parameters.join(", ")
        );
        if writes == 0 {
            markdown.push_str(
                "\n\nNo write is visible in configured loose sources. The database can come from packed or unconfigured Story sources.",
            );
        }
        if !contributors.is_empty() {
            markdown.push_str("\n\n**Contributing goals**");
            let mut contributors: Vec<_> = contributors.into_iter().collect();
            contributors.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
            for (_, module, goal, path) in contributors {
                markdown.push_str(&format!(
                    "\n\n- `{goal}` — module `{module}` — `{}`",
                    path.display()
                ));
            }
        }
        Some(markdown)
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

        let mut markdown = "\n\n---\n\n### Game text preview".to_owned();
        if let Some(display_name) = display_name {
            markdown.push_str(&format!("\n\n**{}**", render_localized_text(&display_name)));
        }
        if let Some(description) = description {
            markdown.push_str(&format!("\n\n{}", render_localized_text(&description)));
        }
        if let Some(parameters) = parameters {
            markdown.push_str(&format!(
                "\n\nDescription parameters: {}",
                markdown_inline_code(parameters)
            ));
        }
        markdown.push_str("\n\n*Static preview. Game logic and UI formatting are not evaluated.*");
        Some(markdown)
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

        let mut markdown = format!("**Game tooltip** `{name}`");
        if let Some(title) = title {
            markdown.push_str(&format!("\n\n**{}**", render_localized_text(&title)));
        }
        if let Some(description) = description {
            markdown.push_str(&format!("\n\n{}", render_localized_text(&description)));
        }
        markdown.push_str(
            "\n\n*Static game text. Runtime values and UI formatting are not evaluated.*",
        );
        Some(markdown)
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

    /// Finds references to the symbol under one position across visible modules.
    pub fn references_at(
        &self,
        path: &Path,
        position: Position,
        include_declaration: bool,
        overlays: &OverlaySet,
    ) -> Vec<SourceLocation> {
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
                            let mut markdown = format!("**Field** `{name}`");
                            if let Some(field_type) = &field.field_type {
                                markdown.push_str(&format!("\n\nType: `{field_type}`"));
                            }
                            if let Some(description) = &field.description {
                                markdown.push_str(&format!("\n\n{description}"));
                            }
                            return Some(markdown);
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
}

/// Accumulates exact database evidence from one disk or overlay goal record.
#[allow(clippy::too_many_arguments)]
fn collect_osiris_database_evidence(
    file: &ParsedFile,
    name: &str,
    arity: u16,
    rank: usize,
    module: &str,
    path: &Path,
    reads: &mut usize,
    writes: &mut usize,
    types: &mut [BTreeSet<String>],
    contributors: &mut BTreeSet<(usize, String, String, PathBuf)>,
) {
    let Some(osiris) = &file.osiris else {
        return;
    };
    let mut contributes = false;
    for occurrence in &osiris.occurrences {
        if occurrence.name != name || occurrence.arity != arity {
            continue;
        }
        contributes = true;
        match occurrence.role {
            OsirisCallRole::Read => *reads += 1,
            OsirisCallRole::Write => *writes += 1,
        }
        for (index, argument) in occurrence.arguments.iter().enumerate() {
            if let (Some(column), Some(evidence)) =
                (types.get_mut(index), argument.evidence.as_ref())
            {
                column.insert(evidence.type_name.clone());
            }
        }
    }
    if contributes {
        contributors.insert((rank, module.into(), osiris.goal.clone(), path.to_path_buf()));
    }
}

/// Tests whether one position is inside a half-open source range.
pub fn range_contains(range: TextRange, position: Position) -> bool {
    let after_start =
        (position.line, position.character) >= (range.start.line, range.start.character);
    let before_end = (position.line, position.character) <= (range.end.line, range.end.character);
    after_start && before_end
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
    let mut markdown = String::with_capacity(unescaped.len());
    for character in unescaped.chars() {
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '#' | '<' | '>'
        ) {
            markdown.push('\\');
        }
        markdown.push(character);
    }
    markdown
}

/// Wraps raw source in a Markdown fence that cannot collide with its backticks.
fn markdown_inline_code(source: &str) -> String {
    let longest_run = source
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    let padding = if source.starts_with(['`', ' ']) || source.ends_with(['`', ' ']) {
        " "
    } else {
        ""
    };
    format!("{fence}{padding}{source}{padding}{fence}")
}
