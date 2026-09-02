use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    OSIRIS_PROCEDURE_KIND, OSIRIS_QUERY_KIND, PackagedThothCatalog, PackagedThothFacts,
    PackagedThothResolution, PackagedThothSource, ParsedFile, SchemaCatalog, SourceKind,
};

/// Invalidates cached packaged Osiris goal facts when their shape changes.
pub const OSIRIS_FACTS_EXTRACTOR_VERSION: &str = "bg3-ls-osiris-facts-v9";

/// Parses one complete, syntax-valid packaged Osiris goal into cacheable facts.
///
/// The virtual entry path provides the goal name; no filesystem access
/// happens because the source already carries its decoded text. Loose source
/// parsing remains recovery-friendly, but package facts must not expose a
/// partial goal or a standalone callable signature as an indexable record.
pub fn parse_osiris_goal_source(source: &PackagedThothSource) -> Result<ParsedFile, crate::Error> {
    let parsed = crate::parse_source(
        crate::SourceFile {
            path: PathBuf::from(source.entry()),
            kind: SourceKind::Osiris,
        },
        source.text(),
        &SchemaCatalog::default(),
        "English",
    )?;
    if !parsed.issues.is_empty() {
        return Err(crate::Error::Parse(
            "the packaged Osiris goal contains syntax errors".into(),
        ));
    }
    if parsed.osiris.is_none() {
        return Err(crate::Error::Parse(
            "the packaged Osiris source is not a complete goal".into(),
        ));
    }
    Ok(parsed)
}

/// The callable role that packaged source evidence proves.
///
/// A same-name and same-arity disagreement can still prove that every
/// candidate is a procedure or query. A disagreement between those roles is
/// intentionally represented as `Unknown`; callers must not place it in a
/// statement position by guessing one declaration kind.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
pub enum PackagedOsirisCallableRole {
    Procedure,
    Query,
    #[default]
    Unknown,
}

/// One declared packaged Osiris procedure or query.
///
/// Parameters keep the authored declaration order. Each entry is the stored
/// source label, which prefixes an alias such as `CHARACTER _Actor` when the
/// declaration casts the parameter and keeps the plain variable name when it
/// does not.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedOsirisCallable {
    pub kind: String,
    /// The role remains explicit even when parameter declarations are
    /// ambiguous. `Unknown` means that the candidates cannot be safely
    /// placed as either a procedure or query.
    #[serde(default)]
    pub role: PackagedOsirisCallableRole,
    pub parameters: Vec<String>,
    pub goal: String,
}

/// One source-backed candidate for a packaged Osiris callable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedOsirisCandidate {
    source: PackagedThothSource,
    callable: PackagedOsirisCallable,
}

impl PackagedOsirisCandidate {
    /// Returns package and virtual-entry provenance for this candidate.
    pub fn source(&self) -> &PackagedThothSource {
        &self.source
    }

    /// Returns the declared callable.
    pub fn callable(&self) -> &PackagedOsirisCallable {
        &self.callable
    }
}

/// The conservative result of resolving one packaged Osiris callable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackagedOsirisResolution<'a> {
    Missing,
    Unique(&'a PackagedOsirisCandidate),
    Ambiguous(&'a [PackagedOsirisCandidate]),
}

/// Immutable packaged Osiris callables grouped by module, name, and arity.
///
/// This is source evidence only. It does not select configured workspace
/// modules or contribute diagnostics.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedOsirisIndex {
    candidates: BTreeMap<(String, String, u16), Vec<PackagedOsirisCandidate>>,
    effective: BTreeMap<(String, String, u16), Vec<PackagedOsirisCandidate>>,
}

impl PackagedOsirisIndex {
    /// Projects parsed goal records into a deterministic callable index.
    ///
    /// The catalog suppresses facts from a lower-priority source when a
    /// higher candidate for the same virtual entry is rejected or tied, so
    /// the effective view never treats incomplete package facts as API data.
    pub fn from_catalog_and_facts(
        catalog: &PackagedThothCatalog,
        facts: &PackagedThothFacts<ParsedFile>,
    ) -> Self {
        let mut index = Self::default();
        for record in facts.records() {
            if record.facts().source.kind != SourceKind::Osiris {
                continue;
            }
            let source = record.source();
            let is_effective = matches!(
                catalog.resolve(source.module(), source.entry()),
                PackagedThothResolution::Unique(effective) if effective == source
            );
            let Some(osiris) = record.facts().osiris.as_ref() else {
                continue;
            };
            for definition in &record.facts().definitions {
                if definition.kind != OSIRIS_PROCEDURE_KIND && definition.kind != OSIRIS_QUERY_KIND
                {
                    continue;
                }
                let Some(arity) = definition.arity else {
                    continue;
                };
                let callable = PackagedOsirisCallable {
                    kind: definition.kind.clone(),
                    role: packaged_osiris_callable_role(&definition.kind),
                    parameters: stored_osiris_parameters(definition),
                    goal: osiris.goal.clone(),
                };
                index.add_candidate(source, definition.name.clone(), arity, callable.clone());
                if is_effective {
                    index.add_effective(source, definition.name.clone(), arity, callable);
                }
            }
        }
        sort_candidates(&mut index.candidates);
        sort_candidates(&mut index.effective);
        index
    }

    /// Resolves one exact callable, collapsing agreeing same-priority candidates.
    ///
    /// Goals may repeat one procedure rule with the same signature. Such
    /// repetitions agree, so they resolve uniquely; only genuinely differing
    /// same-priority declarations stay ambiguous.
    pub fn resolve(&self, module: &str, name: &str, arity: u16) -> PackagedOsirisResolution<'_> {
        let candidates = self
            .effective
            .get(&(module.to_owned(), name.to_owned(), arity))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let Some(first) = candidates.first() else {
            return PackagedOsirisResolution::Missing;
        };
        let top_count = candidates
            .iter()
            .take_while(|candidate| candidate.source.priority() == first.source.priority())
            .count();
        let top = &candidates[..top_count];
        let agrees = top.iter().all(|candidate| {
            candidate.callable().parameters == first.callable().parameters
                && candidate.callable().role == first.callable().role
        });
        if agrees {
            PackagedOsirisResolution::Unique(first)
        } else {
            PackagedOsirisResolution::Ambiguous(top)
        }
    }

    /// Returns the effective arities declared for one callable in a module.
    pub fn effective_arities(&self, module: &str, name: &str) -> Vec<u16> {
        self.effective
            .keys()
            .filter(|(candidate_module, candidate_name, _)| {
                candidate_module == module && candidate_name == name
            })
            .map(|(_, _, arity)| *arity)
            .collect()
    }

    /// Returns effective callables in one module matching a prefix.
    ///
    /// Ambiguous same-priority declarations are returned with empty
    /// parameters. Their role is preserved when all candidates agree on
    /// procedure versus query; mixed-role ambiguity is returned as
    /// [`PackagedOsirisCallableRole::Unknown`] and must not be placed by
    /// callers as either kind.
    pub fn completions_for_module(
        &self,
        module: &str,
        prefix: &str,
    ) -> Vec<(String, u16, PackagedOsirisCallable)> {
        let mut completions = Vec::new();
        for (candidate_module, name, arity) in self.effective.keys() {
            if candidate_module != module
                || !name
                    .get(..prefix.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
            {
                continue;
            }
            match self.resolve(module, name, *arity) {
                PackagedOsirisResolution::Missing => continue,
                PackagedOsirisResolution::Ambiguous(candidates) => {
                    let role = candidates
                        .first()
                        .map(|candidate| candidate.callable().role)
                        .filter(|role| {
                            candidates
                                .iter()
                                .all(|candidate| candidate.callable().role == *role)
                        })
                        .unwrap_or_default();
                    let kind = match role {
                        PackagedOsirisCallableRole::Procedure => OSIRIS_PROCEDURE_KIND,
                        PackagedOsirisCallableRole::Query => OSIRIS_QUERY_KIND,
                        PackagedOsirisCallableRole::Unknown => "",
                    };
                    completions.push((
                        name.clone(),
                        *arity,
                        PackagedOsirisCallable {
                            kind: kind.to_owned(),
                            role,
                            parameters: Vec::new(),
                            goal: String::new(),
                        },
                    ));
                }
                PackagedOsirisResolution::Unique(candidate) => {
                    completions.push((name.clone(), *arity, candidate.callable().clone()))
                }
            }
        }
        completions
    }

    /// Returns the count of indexed candidates, including duplicates.
    pub fn len(&self) -> usize {
        self.candidates.values().map(Vec::len).sum()
    }

    /// Tests whether no packaged Osiris callables were indexed.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn add_candidate(
        &mut self,
        source: &PackagedThothSource,
        name: String,
        arity: u16,
        callable: PackagedOsirisCallable,
    ) {
        Self::add(&mut self.candidates, source, name, arity, callable);
    }

    fn add_effective(
        &mut self,
        source: &PackagedThothSource,
        name: String,
        arity: u16,
        callable: PackagedOsirisCallable,
    ) {
        Self::add(&mut self.effective, source, name, arity, callable);
    }

    fn add(
        candidates: &mut BTreeMap<(String, String, u16), Vec<PackagedOsirisCandidate>>,
        source: &PackagedThothSource,
        name: String,
        arity: u16,
        callable: PackagedOsirisCallable,
    ) {
        candidates
            .entry((source.module().to_owned(), name, arity))
            .or_default()
            .push(PackagedOsirisCandidate {
                source: source.clone(),
                callable,
            });
    }
}

fn packaged_osiris_callable_role(kind: &str) -> PackagedOsirisCallableRole {
    match kind {
        OSIRIS_PROCEDURE_KIND => PackagedOsirisCallableRole::Procedure,
        OSIRIS_QUERY_KIND => PackagedOsirisCallableRole::Query,
        _ => PackagedOsirisCallableRole::Unknown,
    }
}

fn stored_osiris_parameters(definition: &crate::Definition) -> Vec<String> {
    definition
        .fields
        .get("Parameters")
        .map(|parameters| {
            parameters
                .split(',')
                .map(str::trim)
                .filter(|parameter| !parameter.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn sort_candidates(callables: &mut BTreeMap<(String, String, u16), Vec<PackagedOsirisCandidate>>) {
    for candidates in callables.values_mut() {
        candidates.sort_by(|left, right| {
            right
                .source
                .priority()
                .cmp(&left.source.priority())
                .then_with(|| left.source.cmp(&right.source))
        });
    }
}
