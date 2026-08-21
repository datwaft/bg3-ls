use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    PackagedThothCatalog, PackagedThothFacts, PackagedThothResolution, PackagedThothSource,
    ThothAliasAnnotation, ThothClassAnnotation, ThothDeclaration, ThothFile,
    ThothFunctionAnnotation,
};

/// The source-backed kind of one packaged Thoth API symbol.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum PackagedThothApiSymbolKind {
    Function,
    Class,
    Alias,
}

/// One declared or explicitly annotated packaged Thoth API symbol.
///
/// Class candidates retain their explicit fields. Function candidates retain
/// their declaration even when no function contract is available. A function
/// declaration can be nested; this index does not establish runtime visibility.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PackagedThothApiSymbol {
    Function {
        declaration: ThothDeclaration,
        annotation: Option<ThothFunctionAnnotation>,
    },
    Class(ThothClassAnnotation),
    Alias(ThothAliasAnnotation),
}

impl PackagedThothApiSymbol {
    /// Returns the stable kind used for API-index queries.
    pub fn kind(&self) -> PackagedThothApiSymbolKind {
        match self {
            Self::Function { .. } => PackagedThothApiSymbolKind::Function,
            Self::Class(_) => PackagedThothApiSymbolKind::Class,
            Self::Alias(_) => PackagedThothApiSymbolKind::Alias,
        }
    }
}

/// One source-backed candidate for a packaged Thoth API symbol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedThothApiCandidate {
    source: PackagedThothSource,
    symbol: PackagedThothApiSymbol,
}

impl PackagedThothApiCandidate {
    /// Returns package and virtual-entry provenance for this candidate.
    pub fn source(&self) -> &PackagedThothSource {
        &self.source
    }

    /// Returns the declared or explicitly annotated API symbol.
    pub fn symbol(&self) -> &PackagedThothApiSymbol {
        &self.symbol
    }
}

/// The conservative result of resolving one packaged API symbol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackagedThothApiResolution<'a> {
    Missing,
    Unique(&'a PackagedThothApiCandidate),
    Ambiguous(&'a [PackagedThothApiCandidate]),
}

/// Immutable packaged Thoth API candidates grouped by module, kind, and name.
///
/// This is source evidence only. It does not select configured workspace
/// modules, resolve native runtime symbols, or contribute diagnostics.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedThothApiIndex {
    candidates:
        BTreeMap<(String, PackagedThothApiSymbolKind, String), Vec<PackagedThothApiCandidate>>,
    effective:
        BTreeMap<(String, PackagedThothApiSymbolKind, String), Vec<PackagedThothApiCandidate>>,
}

impl PackagedThothApiIndex {
    /// Projects parsed package facts into a deterministic API index.
    ///
    /// The catalog suppresses facts from a lower-priority source when a higher
    /// candidate for the same virtual entry is rejected or tied. This prevents
    /// the index from treating incomplete package facts as effective API data.
    /// All parseable candidates remain available for inspection.
    pub fn from_catalog_and_facts(
        catalog: &PackagedThothCatalog,
        facts: &PackagedThothFacts<ThothFile>,
    ) -> Self {
        let mut index = Self::default();
        for record in facts.records() {
            let source = record.source();
            let is_effective = matches!(
                catalog.resolve(source.module(), source.entry()),
                PackagedThothResolution::Unique(effective) if effective == source
            );
            let file = record.facts();
            for declaration in &file.declarations {
                let symbol = PackagedThothApiSymbol::Function {
                    declaration: declaration.clone(),
                    annotation: function_annotation(file, declaration),
                };
                index.add_candidate(source, declaration.name.clone(), symbol.clone());
                if is_effective {
                    index.add_effective(source, declaration.name.clone(), symbol);
                }
            }
            for class in &file.annotations.classes {
                let symbol = PackagedThothApiSymbol::Class(class.clone());
                index.add_candidate(source, class.name.clone(), symbol.clone());
                if is_effective {
                    index.add_effective(source, class.name.clone(), symbol);
                }
            }
            for alias in &file.annotations.aliases {
                let symbol = PackagedThothApiSymbol::Alias(alias.clone());
                index.add_candidate(source, alias.name.clone(), symbol.clone());
                if is_effective {
                    index.add_effective(source, alias.name.clone(), symbol);
                }
            }
        }
        sort_candidates(&mut index.candidates);
        sort_candidates(&mut index.effective);
        index
    }

    /// Returns all parseable candidates in descending priority order for one
    /// exact symbol, including lower-priority entry candidates.
    pub fn candidates_for(
        &self,
        module: &str,
        kind: PackagedThothApiSymbolKind,
        name: &str,
    ) -> &[PackagedThothApiCandidate] {
        self.candidates
            .get(&(module.to_owned(), kind, name.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Resolves one exact symbol without collapsing equal-priority candidates.
    pub fn resolve(
        &self,
        module: &str,
        kind: PackagedThothApiSymbolKind,
        name: &str,
    ) -> PackagedThothApiResolution<'_> {
        let candidates = self
            .effective
            .get(&(module.to_owned(), kind, name.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let Some(first) = candidates.first() else {
            return PackagedThothApiResolution::Missing;
        };
        let top_count = candidates
            .iter()
            .take_while(|candidate| candidate.source.priority() == first.source.priority())
            .count();
        if top_count == 1 {
            PackagedThothApiResolution::Unique(first)
        } else {
            PackagedThothApiResolution::Ambiguous(&candidates[..top_count])
        }
    }

    /// Returns the count of candidates, including duplicate declarations.
    pub fn len(&self) -> usize {
        self.candidates.values().map(Vec::len).sum()
    }

    /// Tests whether no packaged API candidates were indexed.
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
}

fn function_annotation(
    file: &ThothFile,
    declaration: &ThothDeclaration,
) -> Option<ThothFunctionAnnotation> {
    file.annotations
        .functions
        .iter()
        .find(|annotation| {
            annotation.name.as_deref() == Some(&declaration.name)
                && annotation.name_range == Some(declaration.name_range)
        })
        .cloned()
}

impl PackagedThothApiIndex {
    fn add_candidate(
        &mut self,
        source: &PackagedThothSource,
        name: String,
        symbol: PackagedThothApiSymbol,
    ) {
        Self::add(&mut self.candidates, source, name, symbol);
    }

    fn add_effective(
        &mut self,
        source: &PackagedThothSource,
        name: String,
        symbol: PackagedThothApiSymbol,
    ) {
        Self::add(&mut self.effective, source, name, symbol);
    }

    fn add(
        candidates: &mut BTreeMap<
            (String, PackagedThothApiSymbolKind, String),
            Vec<PackagedThothApiCandidate>,
        >,
        source: &PackagedThothSource,
        name: String,
        symbol: PackagedThothApiSymbol,
    ) {
        candidates
            .entry((source.module().to_owned(), symbol.kind(), name))
            .or_default()
            .push(PackagedThothApiCandidate {
                source: source.clone(),
                symbol,
            });
    }
}

fn sort_candidates(
    symbols: &mut BTreeMap<
        (String, PackagedThothApiSymbolKind, String),
        Vec<PackagedThothApiCandidate>,
    >,
) {
    for candidates in symbols.values_mut() {
        candidates.sort_by(|left, right| {
            right
                .source
                .priority()
                .cmp(&left.source.priority())
                .then_with(|| left.source.cmp(&right.source))
        });
    }
}
