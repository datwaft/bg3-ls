use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bg3_index::{
    ModuleIndex, ModuleRole, PackagedThothFact, PackagedThothResolution, TextRange,
    ThothAliasAnnotation, ThothClassAnnotation, ThothExpressionKind, ThothFile,
    ThothFunctionAnnotation, ThothFunctionContract, ThothStatementId, TypeExpression,
};

use crate::{OverlaySet, WorkspaceSnapshot};

/// The source of explicit Thoth type evidence.
///
/// Packaged entries are virtual sources. They retain package and entry
/// provenance but never expose a fabricated editor path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThothTypeSource {
    Loose {
        module: String,
        path: PathBuf,
        range: TextRange,
    },
    Packaged {
        module: String,
        package: PathBuf,
        entry: String,
        range: TextRange,
    },
}

/// One uniquely effective explicit Thoth alias.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedThothAlias {
    pub name: String,
    pub ty: TypeExpression,
    pub name_range: TextRange,
    pub type_range: TextRange,
    pub source: ThothTypeSource,
}

/// One field from a uniquely effective explicit Thoth class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedThothField {
    pub name: String,
    pub ty: TypeExpression,
    pub range: TextRange,
    pub name_range: TextRange,
    pub type_range: TextRange,
}

/// One uniquely effective explicit Thoth class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedThothClass {
    pub name: String,
    pub name_range: TextRange,
    pub fields: Vec<ResolvedThothField>,
    pub source: ThothTypeSource,
}

/// One uniquely effective annotated Thoth helper.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedThothFunction {
    pub name: String,
    pub name_range: TextRange,
    pub contracts: Vec<ThothFunctionContract>,
    pub source: ThothTypeSource,
}

/// One direct `@type` binding with its syntax-proven declaration identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedThothVariable {
    pub target: String,
    pub ty: TypeExpression,
    pub target_range: TextRange,
    pub type_range: TextRange,
    pub source: ThothTypeSource,
    pub statement: ThothStatementId,
    pub local: bool,
    pub global: bool,
}

#[derive(Clone)]
enum NominalCandidate {
    Alias(ThothAliasAnnotation, ThothTypeSource),
    Class(ThothClassAnnotation, ThothTypeSource),
}

enum NominalResolution {
    Missing,
    Unique(Box<NominalCandidate>),
    Ambiguous,
}

impl WorkspaceSnapshot {
    /// Resolves an explicit type expression without inferring program behavior.
    ///
    /// Missing names, same-rank ambiguity, and alias cycles become `Unknown`.
    pub fn resolve_thoth_type(&self, ty: &TypeExpression, overlays: &OverlaySet) -> TypeExpression {
        self.resolve_thoth_type_inner(ty, overlays, &mut BTreeSet::new())
    }

    /// Resolves one uniquely effective explicit alias.
    pub fn resolve_thoth_alias(
        &self,
        name: &str,
        overlays: &OverlaySet,
    ) -> Option<ResolvedThothAlias> {
        let NominalResolution::Unique(candidate) = self.resolve_nominal(name, overlays) else {
            return None;
        };
        let NominalCandidate::Alias(alias, source) = *candidate else {
            return None;
        };
        Some(ResolvedThothAlias {
            name: alias.name,
            ty: self.resolve_thoth_type(&alias.ty, overlays),
            name_range: alias.name_range,
            type_range: alias.type_range,
            source,
        })
    }

    /// Resolves one uniquely effective explicit class and its field types.
    pub fn resolve_thoth_class(
        &self,
        name: &str,
        overlays: &OverlaySet,
    ) -> Option<ResolvedThothClass> {
        let NominalResolution::Unique(candidate) = self.resolve_nominal(name, overlays) else {
            return None;
        };
        let NominalCandidate::Class(class, source) = *candidate else {
            return None;
        };
        Some(ResolvedThothClass {
            name: class.name,
            name_range: class.name_range,
            fields: class
                .fields
                .into_iter()
                .map(|field| ResolvedThothField {
                    name: field.name,
                    ty: self.resolve_thoth_type(&field.ty, overlays),
                    range: field.range,
                    name_range: field.name_range,
                    type_range: field.type_range,
                })
                .collect(),
            source,
        })
    }

    /// Resolves the contracts on one uniquely effective annotated helper.
    ///
    /// A higher-precedence unannotated declaration masks lower annotations.
    pub fn resolve_thoth_function(
        &self,
        name: &str,
        overlays: &OverlaySet,
    ) -> Option<ResolvedThothFunction> {
        for layer in self.layers.iter().rev() {
            let loose = self.loose_function_candidates(layer, name, overlays);
            if !loose.is_empty() {
                let (annotation, source) = exactly_one(loose)??;
                return self.resolved_function(name, annotation, source, overlays);
            }
            if layer.spec.role != ModuleRole::Base {
                continue;
            }
            let records = self.effective_packaged_records(&layer.spec.name)?;
            let packaged = records
                .into_iter()
                .flat_map(|record| {
                    record
                        .facts()
                        .declarations
                        .iter()
                        .filter(move |declaration| declaration.name == name)
                        .map(move |declaration| {
                            let annotation =
                                function_annotation(record.facts(), name, declaration.name_range);
                            let source = annotation
                                .as_ref()
                                .map(|annotation| packaged_source(record, annotation.range));
                            annotation.zip(source)
                        })
                })
                .collect::<Vec<_>>();
            if !packaged.is_empty() {
                let (annotation, source) = exactly_one(packaged)??;
                return self.resolved_function(name, annotation, source, overlays);
            }
        }
        None
    }

    /// Resolves one direct `@type` binding by its exact declaration range.
    ///
    /// This does not search same-name uses or perform flow analysis.
    pub fn resolve_thoth_variable(
        &self,
        path: &Path,
        target_range: TextRange,
        overlays: &OverlaySet,
    ) -> Option<ResolvedThothVariable> {
        let (module, file) = self.file(path, overlays)?;
        if !self.layers.iter().any(|layer| layer.spec.name == module) {
            return None;
        }
        let thoth = file.thoth.as_ref()?;
        let annotation = exactly_one(
            thoth
                .annotations
                .variables
                .iter()
                .filter(|annotation| annotation.target_range == target_range),
        )?;
        let fact = exactly_one(
            thoth
                .expression_facts
                .iter()
                .filter(|fact| fact.range == target_range),
        )?;
        if !matches!(fact.kind, ThothExpressionKind::Identifier) {
            return None;
        }
        let assignment = exactly_one(thoth.assignments.iter().filter(|assignment| {
            assignment.targets.len() == 1 && assignment.targets[0].range == target_range
        }))?;
        Some(ResolvedThothVariable {
            target: annotation.target.clone(),
            ty: self.resolve_thoth_type(&annotation.ty, overlays),
            target_range: annotation.target_range,
            type_range: annotation.type_range,
            source: ThothTypeSource::Loose {
                module: module.to_owned(),
                path: path.to_owned(),
                range: annotation.range,
            },
            statement: fact.statement,
            local: assignment.local,
            global: assignment.global,
        })
    }

    fn resolve_thoth_type_inner(
        &self,
        ty: &TypeExpression,
        overlays: &OverlaySet,
        visited: &mut BTreeSet<String>,
    ) -> TypeExpression {
        match ty {
            TypeExpression::Unknown => TypeExpression::Unknown,
            TypeExpression::Nil => TypeExpression::Nil,
            TypeExpression::Primitive(primitive) => TypeExpression::Primitive(*primitive),
            TypeExpression::Name(name) => {
                if !visited.insert(name.clone()) {
                    return TypeExpression::Unknown;
                }
                let resolved = match self.resolve_nominal(name, overlays) {
                    NominalResolution::Unique(candidate) => match *candidate {
                        NominalCandidate::Class(class, _) => TypeExpression::Name(class.name),
                        NominalCandidate::Alias(alias, _) => {
                            self.resolve_thoth_type_inner(&alias.ty, overlays, visited)
                        }
                    },
                    NominalResolution::Missing | NominalResolution::Ambiguous => {
                        TypeExpression::Unknown
                    }
                };
                visited.remove(name);
                resolved
            }
            TypeExpression::Union(members) => {
                let members = members
                    .iter()
                    .map(|member| self.resolve_thoth_type_inner(member, overlays, visited))
                    .collect::<Vec<_>>();
                TypeExpression::union(members)
            }
            TypeExpression::Array(element) => TypeExpression::Array(Box::new(
                self.resolve_thoth_type_inner(element, overlays, visited),
            )),
            TypeExpression::Function {
                parameters,
                returns,
            } => TypeExpression::Function {
                parameters: parameters
                    .iter()
                    .cloned()
                    .map(|mut parameter| {
                        parameter.ty =
                            self.resolve_thoth_type_inner(&parameter.ty, overlays, visited);
                        parameter
                    })
                    .collect(),
                returns: returns
                    .iter()
                    .map(|ty| self.resolve_thoth_type_inner(ty, overlays, visited))
                    .collect(),
            },
        }
    }

    fn resolve_nominal(&self, name: &str, overlays: &OverlaySet) -> NominalResolution {
        for layer in self.layers.iter().rev() {
            let loose = self.loose_nominal_candidates(layer, name, overlays);
            if !loose.is_empty() {
                return match exactly_one(loose) {
                    Some(candidate) => NominalResolution::Unique(Box::new(candidate)),
                    None => NominalResolution::Ambiguous,
                };
            }
            if layer.spec.role != ModuleRole::Base {
                continue;
            }
            let Some(records) = self.effective_packaged_records(&layer.spec.name) else {
                return NominalResolution::Ambiguous;
            };
            let mut packaged = Vec::new();
            for record in records {
                for alias in record
                    .facts()
                    .annotations
                    .aliases
                    .iter()
                    .filter(|alias| alias.name == name)
                {
                    packaged.push(NominalCandidate::Alias(
                        alias.clone(),
                        packaged_source(record, alias.range),
                    ));
                }
                for class in record
                    .facts()
                    .annotations
                    .classes
                    .iter()
                    .filter(|class| class.name == name)
                {
                    packaged.push(NominalCandidate::Class(
                        class.clone(),
                        packaged_source(record, class.range),
                    ));
                }
            }
            if !packaged.is_empty() {
                return match exactly_one(packaged) {
                    Some(candidate) => NominalResolution::Unique(Box::new(candidate)),
                    None => NominalResolution::Ambiguous,
                };
            }
        }
        NominalResolution::Missing
    }

    fn loose_nominal_candidates(
        &self,
        layer: &ModuleIndex,
        name: &str,
        overlays: &OverlaySet,
    ) -> Vec<NominalCandidate> {
        let mut candidates = Vec::new();
        for (path, file) in visible_thoth_files(layer, overlays) {
            for alias in file
                .annotations
                .aliases
                .iter()
                .filter(|alias| alias.name == name)
            {
                candidates.push(NominalCandidate::Alias(
                    alias.clone(),
                    loose_source(layer, path, alias.range),
                ));
            }
            for class in file
                .annotations
                .classes
                .iter()
                .filter(|class| class.name == name)
            {
                candidates.push(NominalCandidate::Class(
                    class.clone(),
                    loose_source(layer, path, class.range),
                ));
            }
        }
        candidates
    }

    fn loose_function_candidates(
        &self,
        layer: &ModuleIndex,
        name: &str,
        overlays: &OverlaySet,
    ) -> Vec<Option<(ThothFunctionAnnotation, ThothTypeSource)>> {
        let mut candidates = Vec::new();
        for (path, file) in visible_thoth_files(layer, overlays) {
            for declaration in file
                .declarations
                .iter()
                .filter(|declaration| declaration.name == name)
            {
                candidates.push(function_annotation(file, name, declaration.name_range).map(
                    |annotation| {
                        let source = loose_source(layer, path, annotation.range);
                        (annotation, source)
                    },
                ));
            }
        }
        candidates
    }

    fn effective_packaged_records<'a>(
        &'a self,
        module: &str,
    ) -> Option<Vec<&'a PackagedThothFact<ThothFile>>> {
        // Type resolution requires a complete effective view of the module.
        // One ambiguous or rejected top-priority entry can contain a competing
        // declaration, so it suppresses packaged typed metadata for this rank.
        let catalog = self.packaged_thoth.as_ref();
        let facts = self.packaged_thoth_facts.as_ref();
        let entries = catalog
            .sources()
            .filter(|source| source.module() == module)
            .map(|source| source.entry().to_owned())
            .collect::<BTreeSet<_>>();
        let mut records = Vec::new();
        for entry in entries {
            let source = match catalog.resolve(module, &entry) {
                PackagedThothResolution::Unique(source) => source,
                PackagedThothResolution::Missing | PackagedThothResolution::Ambiguous(_) => {
                    return None;
                }
            };
            let record = facts.iter().find(|record| record.source() == source)?;
            records.push(record);
        }
        Some(records)
    }

    fn resolved_function(
        &self,
        name: &str,
        annotation: ThothFunctionAnnotation,
        source: ThothTypeSource,
        overlays: &OverlaySet,
    ) -> Option<ResolvedThothFunction> {
        let name_range = annotation.name_range?;
        let contracts = annotation
            .contracts
            .into_iter()
            .map(|mut contract| {
                for parameter in &mut contract.parameters {
                    parameter.ty = self.resolve_thoth_type(&parameter.ty, overlays);
                }
                for return_value in &mut contract.returns {
                    return_value.ty = self.resolve_thoth_type(&return_value.ty, overlays);
                }
                contract
            })
            .collect();
        Some(ResolvedThothFunction {
            name: name.to_owned(),
            name_range,
            contracts,
            source,
        })
    }
}

fn visible_thoth_files<'a>(
    layer: &'a ModuleIndex,
    overlays: &'a OverlaySet,
) -> Vec<(&'a Path, &'a ThothFile)> {
    let mut files = Vec::new();
    for (path, overlay) in overlays.for_module(&layer.spec.name) {
        if let Some(thoth) = overlay.parsed.thoth.as_ref() {
            files.push((path.as_path(), thoth));
        }
    }
    for (path, file) in &layer.files {
        if overlays.contains(path) {
            continue;
        }
        if let Some(thoth) = file.thoth.as_ref() {
            files.push((path.as_path(), thoth));
        }
    }
    files.sort_by(|left, right| left.0.cmp(right.0));
    files
}

fn function_annotation(
    file: &ThothFile,
    name: &str,
    name_range: TextRange,
) -> Option<ThothFunctionAnnotation> {
    file.annotations
        .functions
        .iter()
        .find(|annotation| {
            annotation.name.as_deref() == Some(name) && annotation.name_range == Some(name_range)
        })
        .cloned()
}

fn loose_source(layer: &ModuleIndex, path: &Path, range: TextRange) -> ThothTypeSource {
    ThothTypeSource::Loose {
        module: layer.spec.name.clone(),
        path: path.to_owned(),
        range,
    }
}

fn packaged_source(record: &PackagedThothFact<ThothFile>, range: TextRange) -> ThothTypeSource {
    ThothTypeSource::Packaged {
        module: record.source().module().to_owned(),
        package: record.source().package().to_owned(),
        entry: record.source().entry().to_owned(),
        range,
    }
}

fn exactly_one<T>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut values = values.into_iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}
