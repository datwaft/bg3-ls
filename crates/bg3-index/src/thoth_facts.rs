use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::Error;
use crate::thoth::{PackagedThothCatalog, PackagedThothSource};

/// Invalidates cached packaged Thoth facts when their semantic shape changes.
pub const THOTH_FACTS_EXTRACTOR_VERSION: &str = "bg3-ls-thoth-facts-v5";

/// One parsed Thoth fact record with the package entry that produced it.
///
/// Package entries are virtual documents. Keeping the complete source beside
/// the fact lets later consumers report provenance without inventing a local
/// filesystem path or an editor URI.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedThothFact<F> {
    source: PackagedThothSource,
    facts: F,
}

impl<F> PackagedThothFact<F> {
    /// Creates a source-backed fact record.
    pub fn new(source: PackagedThothSource, facts: F) -> Self {
        Self { source, facts }
    }

    /// Returns the package-backed source represented by this record.
    pub fn source(&self) -> &PackagedThothSource {
        &self.source
    }

    /// Returns the parser result for this source.
    pub fn facts(&self) -> &F {
        &self.facts
    }

    /// Consumes the record and returns its parser result.
    pub fn into_facts(self) -> F {
        self.facts
    }
}

/// Immutable parsed records for all packaged Thoth candidates in a catalog.
///
/// Equal-priority candidates are retained. Consumers that need a resolved
/// declaration must apply the same priority and ambiguity rules as the source
/// catalog; this layer never chooses by package name or enumeration order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PackagedThothFacts<F> {
    extractor_version: String,
    records: Vec<PackagedThothFact<F>>,
    rejected: usize,
}

impl<F> PackagedThothFacts<F> {
    pub(crate) fn new(
        extractor_version: impl Into<String>,
        records: Vec<PackagedThothFact<F>>,
        rejected: usize,
    ) -> Self {
        Self {
            extractor_version: extractor_version.into(),
            records,
            rejected,
        }
    }

    /// Returns the extractor identity used to produce these records.
    pub fn extractor_version(&self) -> &str {
        &self.extractor_version
    }

    /// Returns all source-backed records in catalog order.
    pub fn records(&self) -> &[PackagedThothFact<F>] {
        &self.records
    }

    /// Returns the number of parsed package-entry candidates.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Tests whether no package-entry candidates produced facts.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns the number of package entries rejected by the parser.
    ///
    /// Rejected entries are intentionally not exposed as facts. The count is
    /// retained in the immutable cached result so callers can report that
    /// the catalog is incomplete without exposing package paths or source.
    pub fn rejected_count(&self) -> usize {
        self.rejected
    }

    /// Iterates over all source-backed records.
    pub fn iter(&self) -> impl Iterator<Item = &PackagedThothFact<F>> {
        self.records.iter()
    }
}

/// Parses every candidate in an immutable packaged Thoth catalog.
///
/// The callback is deliberately source-based rather than path-based. The
/// eventual Thoth fact model can therefore be introduced without making
/// package entries look like loose files to the rest of the index.
pub fn parse_packaged_thoth_facts<F, Parse>(
    catalog: &PackagedThothCatalog,
    extractor_version: impl Into<String>,
    parse: Parse,
) -> Result<PackagedThothFacts<F>, Error>
where
    Parse: Fn(&PackagedThothSource) -> Result<F, Error>,
{
    let extractor_version = extractor_version.into();
    let mut records = Vec::new();
    let mut rejected = 0;
    for source in catalog.sources() {
        match parse(source) {
            Ok(facts) => records.push(PackagedThothFact::new(source.clone(), facts)),
            Err(_) => rejected += 1,
        }
    }
    Ok(PackagedThothFacts::new(
        extractor_version,
        records,
        rejected,
    ))
}

/// Bounds required by the persistent fact cache.
pub trait CachedThothFacts: Clone + Serialize + DeserializeOwned {}

impl<T> CachedThothFacts for T where T: Clone + Serialize + DeserializeOwned {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thoth::PackagedThothSource;

    fn catalog() -> PackagedThothCatalog {
        PackagedThothCatalog::from_sources([
            PackagedThothSource::new(
                "Example",
                "/synthetic/base.pak",
                "Mods/Example/Scripts/thoth/helpers/base.khn",
                0,
                "base",
            )
            .expect("base source"),
            PackagedThothSource::new(
                "Example",
                "/synthetic/patch.pak",
                "Mods/Example/Scripts/thoth/helpers/base.khn",
                1,
                "patch",
            )
            .expect("patch source"),
        ])
        .expect("catalog")
    }

    #[test]
    fn parser_receives_provenance_without_a_filesystem_path() {
        let facts = parse_packaged_thoth_facts(&catalog(), "facts-v1", |source| {
            Ok::<_, Error>(format!("{}:{}", source.package().display(), source.entry()))
        })
        .expect("facts");

        assert_eq!(facts.extractor_version(), "facts-v1");
        assert_eq!(facts.len(), 2);
        assert_eq!(facts.rejected_count(), 0);
        assert_eq!(
            facts.records()[0].facts(),
            "/synthetic/patch.pak:Mods/Example/Scripts/thoth/helpers/base.khn"
        );
        assert_eq!(facts.records()[0].source().module(), "Example");
    }
}
