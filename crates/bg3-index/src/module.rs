use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use crate::domain::{
    Definition, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND, OSIRIS_PROCEDURE_KIND, OSIRIS_QUERY_KIND,
    ObservedFunction, ParsedFile, Reference,
};
use crate::parser::canonical_kind;
use crate::{ModuleSpec, SymbolTarget};

pub type DefinitionId = usize;

/// A declaration paired with its source path inside one module.
#[derive(Clone, Debug)]
pub struct DefinitionRecord {
    pub path: Arc<PathBuf>,
    file: Arc<ParsedFile>,
    definition_index: usize,
}

impl DefinitionRecord {
    /// Borrows the declaration from its owning immutable parsed file.
    pub fn definition(&self) -> &Definition {
        &self.file.definitions[self.definition_index]
    }
}

/// A reference paired with its source path inside one module.
#[derive(Clone, Debug)]
pub struct ReferenceRecord {
    pub path: Arc<PathBuf>,
    file: Arc<ParsedFile>,
    reference_index: usize,
}

impl ReferenceRecord {
    /// Borrows the reference from its owning immutable parsed file.
    pub fn reference(&self) -> &Reference {
        &self.file.references[self.reference_index]
    }
}

/// An immutable semantic index for one independently cached module.
#[derive(Clone, Debug)]
pub struct ModuleIndex {
    pub spec: ModuleSpec,
    pub files: BTreeMap<PathBuf, Arc<ParsedFile>>,
    pub definitions: Vec<DefinitionRecord>,
    pub references: Vec<ReferenceRecord>,
    pub functions: BTreeMap<String, ObservedFunction>,
    by_kind_name: HashMap<(String, String), Vec<DefinitionId>>,
    by_kind: HashMap<String, Vec<DefinitionId>>,
    by_name: HashMap<String, Vec<DefinitionId>>,
    by_uuid: HashMap<Uuid, Vec<DefinitionId>>,
    by_alias: HashMap<String, Vec<DefinitionId>>,
    by_osiris_goal: HashMap<String, Vec<DefinitionId>>,
    by_osiris_callable: HashMap<(String, u16), Vec<DefinitionId>>,
    by_osiris_database: HashMap<(String, u16), Vec<DefinitionId>>,
    references_by_target: HashMap<SymbolTarget, Vec<usize>>,
}

impl ModuleIndex {
    /// Builds deterministic lookup tables from independently parsed files.
    pub fn new(spec: ModuleSpec, parsed_files: Vec<ParsedFile>) -> Self {
        let mut files = BTreeMap::new();
        for parsed in parsed_files {
            files.insert(parsed.source.path.clone(), Arc::new(parsed));
        }

        let mut index = Self {
            spec,
            files,
            definitions: Vec::new(),
            references: Vec::new(),
            functions: BTreeMap::new(),
            by_kind_name: HashMap::new(),
            by_kind: HashMap::new(),
            by_name: HashMap::new(),
            by_uuid: HashMap::new(),
            by_alias: HashMap::new(),
            by_osiris_goal: HashMap::new(),
            by_osiris_callable: HashMap::new(),
            by_osiris_database: HashMap::new(),
            references_by_target: HashMap::new(),
        };
        index.rebuild();
        index
    }

    /// Rebuilds arenas and maps in stable path and declaration order.
    fn rebuild(&mut self) {
        let files: Vec<_> = self
            .files
            .iter()
            .map(|(path, file)| (path.clone(), Arc::clone(file)))
            .collect();
        for (path, file) in files {
            let path = Arc::new(path.clone());
            for (definition_index, definition) in file.definitions.iter().enumerate() {
                let id = self.definitions.len();
                self.definitions.push(DefinitionRecord {
                    path: Arc::clone(&path),
                    file: Arc::clone(&file),
                    definition_index,
                });
                self.by_kind_name
                    .entry((
                        canonical_kind(&definition.kind).to_owned(),
                        definition.name.clone(),
                    ))
                    .or_default()
                    .push(id);
                self.by_kind
                    .entry(canonical_kind(&definition.kind).to_owned())
                    .or_default()
                    .push(id);
                self.by_name
                    .entry(definition.name.clone())
                    .or_default()
                    .push(id);
                if let Some(uuid) = definition.uuid {
                    self.by_uuid.entry(uuid).or_default().push(id);
                }
                for alias in &definition.aliases {
                    self.by_alias.entry(alias.clone()).or_default().push(id);
                }
                match (definition.kind.as_str(), definition.arity) {
                    (OSIRIS_GOAL_KIND, _) => {
                        self.by_osiris_goal
                            .entry(definition.name.clone())
                            .or_default()
                            .push(id);
                    }
                    (OSIRIS_PROCEDURE_KIND | OSIRIS_QUERY_KIND, Some(arity)) => {
                        self.by_osiris_callable
                            .entry((definition.name.clone(), arity))
                            .or_default()
                            .push(id);
                    }
                    (OSIRIS_DATABASE_KIND, Some(arity)) => {
                        self.by_osiris_database
                            .entry((definition.name.clone(), arity))
                            .or_default()
                            .push(id);
                    }
                    _ => {}
                }
            }
            for (reference_index, reference) in file.references.iter().enumerate() {
                let id = self.references.len();
                self.references.push(ReferenceRecord {
                    path: Arc::clone(&path),
                    file: Arc::clone(&file),
                    reference_index,
                });
                self.references_by_target
                    .entry(reference.target.clone())
                    .or_default()
                    .push(id);
            }
            for function in &file.observed_functions {
                match self.functions.entry(function.name.clone()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(function.clone());
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        let aggregate = entry.get_mut();
                        aggregate.count += function.count;
                        aggregate.min_arity = aggregate.min_arity.min(function.min_arity);
                        aggregate.max_arity = aggregate.max_arity.max(function.max_arity);
                    }
                }
            }
        }
        for ids in self.by_kind.values_mut() {
            ids.sort_by(|left, right| {
                self.definitions[*left]
                    .definition()
                    .name
                    .cmp(&self.definitions[*right].definition().name)
            });
        }
    }

    /// Resolves a semantic target inside this module only.
    pub fn resolve(&self, target: &SymbolTarget) -> Vec<&DefinitionRecord> {
        let ids = match target {
            SymbolTarget::Named {
                kind: Some(kind),
                name,
            } => self
                .by_kind_name
                .get(&(canonical_kind(kind).to_owned(), name.clone()))
                .or_else(|| self.by_alias.get(name)),
            SymbolTarget::Named { kind: None, name } => {
                self.by_name.get(name).or_else(|| self.by_alias.get(name))
            }
            SymbolTarget::Tooltip { .. } => None,
            SymbolTarget::Uuid(uuid) => self.by_uuid.get(uuid),
            SymbolTarget::OsirisGoal { name } => self.by_osiris_goal.get(name),
            SymbolTarget::OsirisCallable { name, arity } => {
                self.by_osiris_callable.get(&(name.clone(), *arity))
            }
            SymbolTarget::OsirisDatabase { name, arity } => {
                self.by_osiris_database.get(&(name.clone(), *arity))
            }
        };
        ids.into_iter()
            .flatten()
            .map(|id| &self.definitions[*id])
            .collect()
    }

    /// Returns the parsed file for an absolute source path.
    pub fn file(&self, path: &Path) -> Option<&Arc<ParsedFile>> {
        self.files.get(path)
    }

    /// Returns declarations of one kind in deterministic name order.
    pub fn definitions_of_kind(&self, kind: &str) -> impl Iterator<Item = &DefinitionRecord> {
        self.by_kind
            .get(canonical_kind(kind))
            .into_iter()
            .flatten()
            .map(|id| &self.definitions[*id])
    }

    /// Finds all references whose semantic target resolves to the same key.
    pub fn references_to(&self, target: &SymbolTarget) -> Vec<&ReferenceRecord> {
        self.references_by_target
            .get(target)
            .into_iter()
            .flatten()
            .map(|id| &self.references[*id])
            .collect()
    }
}
