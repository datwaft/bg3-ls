//! Reviewed domains for string-valued Osiris arguments.
//!
//! The generated story header describes the syntax of an argument, but it
//! does not say whether a `STRING` is a resource name, an identifier, or free
//! text. This module keeps that semantic decision in a small, reviewed
//! overlay. Every record is tied to the complete callable identity and to the
//! parameter metadata emitted by the generated catalog.

use crate::osiris_catalog::{
    GENERATED_OSIRIS_CATALOG_GENERATOR_VERSION, OSIRIS_CATALOG_SOURCE_HASH,
    OSIRIS_CATALOG_SOURCE_VERSION, OSIRIS_CONTRACTS, OsirisContractKind, OsirisContractSpec,
    OsirisParameterDirection, osiris_contract_by_kind,
};

/// Version of the reviewed Osiris argument-domain record format.
pub const OSIRIS_ARGUMENT_DOMAIN_CATALOG_VERSION: &str = "bg3-ls-osiris-argument-domains-v1";

/// A known resource family that can be used as a `Named` target kind.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OsirisResourceDomain {
    ActionResource,
    Equipment,
    InterruptData,
    Localization,
    OsirisGoal,
    PassiveData,
    SpellData,
    SpellSet,
    StatusData,
    StatusGroup,
    TreasureTable,
    /// A reviewed family not yet represented by a dedicated enum variant.
    Custom(&'static str),
}

impl OsirisResourceDomain {
    /// Returns the schema kind used by the index's named-resource resolver.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ActionResource => "ActionResource",
            Self::Equipment => "Equipment",
            Self::InterruptData => "InterruptData",
            Self::Localization => "Localization",
            Self::OsirisGoal => "OsirisGoal",
            Self::PassiveData => "PassiveData",
            Self::SpellData => "SpellData",
            Self::SpellSet => "SpellSet",
            Self::StatusData => "StatusData",
            Self::StatusGroup => "StatusGroup",
            Self::TreasureTable => "TreasureTable",
            Self::Custom(value) => value,
        }
    }
}

/// The reviewed interpretation of one string-valued argument.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum OsirisArgumentDisposition {
    /// The literal names a resource in the specified index domain.
    Resource(OsirisResourceDomain),
    /// A runtime identifier such as a timer or callback name.
    RuntimeIdentifier,
    /// A finite value set, but not an indexed resource.
    Enumeration,
    /// An Osiris expression encoded as a string.
    Expression,
    /// User-facing or otherwise uninterpreted text.
    FreeText,
    /// The argument names a known resource, but its supporting index is not
    /// available yet. This never activates parser references.
    DeferredResource(OsirisResourceDomain),
    /// Review found insufficient evidence for a narrower semantic class.
    /// This explicit conservative disposition never activates parser
    /// references and stays separate from a known deferred resource.
    DeferredSemanticReview,
}

impl OsirisArgumentDisposition {
    /// Returns the resource kind when the disposition is navigable.
    pub const fn resource_domain(self) -> Option<&'static str> {
        match self {
            Self::Resource(domain) => Some(domain.as_str()),
            Self::RuntimeIdentifier
            | Self::Enumeration
            | Self::Expression
            | Self::FreeText
            | Self::DeferredResource(_)
            | Self::DeferredSemanticReview => None,
        }
    }

    /// Returns a reviewed domain whose index is not available yet.
    pub const fn deferred_resource_domain(self) -> Option<&'static str> {
        match self {
            Self::DeferredResource(domain) => Some(domain.as_str()),
            _ => None,
        }
    }
}

/// Provenance of the generated contract catalog used to review records.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OsirisCatalogProvenance {
    pub source_version: &'static str,
    pub source_hash: &'static str,
    pub generator_version: &'static str,
}

/// Provenance of the generated catalog checked into this build.
pub const CURRENT_OSIRIS_CATALOG_PROVENANCE: OsirisCatalogProvenance = OsirisCatalogProvenance {
    source_version: OSIRIS_CATALOG_SOURCE_VERSION,
    source_hash: OSIRIS_CATALOG_SOURCE_HASH,
    generator_version: GENERATED_OSIRIS_CATALOG_GENERATOR_VERSION,
};

/// One reviewed interpretation of an exact generated-contract argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OsirisArgumentDomainRecord {
    pub kind: OsirisContractKind,
    pub name: &'static str,
    pub arity: u16,
    pub index: usize,
    pub parameter_name: &'static str,
    pub parameter_type: &'static str,
    pub direction: OsirisParameterDirection,
    pub disposition: OsirisArgumentDisposition,
    /// Official documentation URL used for this review.
    pub source_url: Option<&'static str>,
    /// MediaWiki revision ID used for this review.
    pub source_revision: Option<u32>,
    /// ISO 8601 date of the review.
    pub reviewed_on: &'static str,
    pub catalog_source_version: &'static str,
    pub catalog_source_hash: &'static str,
    pub catalog_generator_version: &'static str,
}

impl OsirisArgumentDomainRecord {
    /// Builds a compact record with official evidence for an active or
    /// deferred resource.
    #[allow(clippy::too_many_arguments)]
    pub const fn resource(
        kind: OsirisContractKind,
        name: &'static str,
        arity: u16,
        index: usize,
        parameter_name: &'static str,
        parameter_type: &'static str,
        direction: OsirisParameterDirection,
        disposition: OsirisArgumentDisposition,
        source_url: &'static str,
        source_revision: u32,
        reviewed_on: &'static str,
        catalog_source_version: &'static str,
        catalog_source_hash: &'static str,
        catalog_generator_version: &'static str,
    ) -> Self {
        Self {
            kind,
            name,
            arity,
            index,
            parameter_name,
            parameter_type,
            direction,
            disposition,
            source_url: Some(source_url),
            source_revision: Some(source_revision),
            reviewed_on,
            catalog_source_version,
            catalog_source_hash,
            catalog_generator_version,
        }
    }

    /// Builds a compact catalog-only record for a negative or pending review.
    #[allow(clippy::too_many_arguments)]
    pub const fn catalog_only(
        kind: OsirisContractKind,
        name: &'static str,
        arity: u16,
        index: usize,
        parameter_name: &'static str,
        parameter_type: &'static str,
        direction: OsirisParameterDirection,
        disposition: OsirisArgumentDisposition,
        reviewed_on: &'static str,
        catalog_source_version: &'static str,
        catalog_source_hash: &'static str,
        catalog_generator_version: &'static str,
    ) -> Self {
        Self {
            kind,
            name,
            arity,
            index,
            parameter_name,
            parameter_type,
            direction,
            disposition,
            source_url: None,
            source_revision: None,
            reviewed_on,
            catalog_source_version,
            catalog_source_hash,
            catalog_generator_version,
        }
    }
}

/// Complete reviewed ledger currently shipped by the index.
pub use crate::osiris_domain_records::OSIRIS_ARGUMENT_DOMAIN_LEDGER as OSIRIS_ARGUMENT_DOMAIN_RECORDS;

/// Looks up a reviewed record by exact callable identity and argument index.
pub fn osiris_argument_domain_record(
    kind: OsirisContractKind,
    name: &str,
    arity: u16,
    index: usize,
) -> Option<&'static OsirisArgumentDomainRecord> {
    osiris_argument_domain_record_in(
        OSIRIS_CONTRACTS,
        OSIRIS_ARGUMENT_DOMAIN_RECORDS,
        kind,
        name,
        arity,
        index,
    )
}

/// Looks up a reviewed record in a supplied contract catalog and ledger.
///
/// This is the extension point for a generated or separately maintained
/// exhaustive ledger. The record and generated contract metadata must agree;
/// a matching tuple alone is not sufficient. `records` must be sorted by
/// callable name, kind, arity, and argument index, as enforced by the ledger
/// validators.
pub fn osiris_argument_domain_record_in<'a>(
    contracts: &[OsirisContractSpec],
    records: &'a [OsirisArgumentDomainRecord],
    kind: OsirisContractKind,
    name: &str,
    arity: u16,
    index: usize,
) -> Option<&'a OsirisArgumentDomainRecord> {
    let record = records
        .binary_search_by(|record| record_key(record).cmp(&(name, kind, arity, index)))
        .ok()
        .map(|index| &records[index])?;
    let contract = osiris_contract_by_kind(contracts, kind, name, arity)?;
    let parameter = contract.parameters.get(index)?;
    (parameter.name == record.parameter_name
        && parameter.type_name == record.parameter_type
        && parameter.direction == record.direction
        && record.parameter_type == "STRING"
        && matches!(
            record.direction,
            OsirisParameterDirection::In | OsirisParameterDirection::InOut
        ))
    .then_some(record)
}

/// Returns the reviewed interpretation of an exact argument.
pub fn osiris_argument_disposition(
    kind: OsirisContractKind,
    name: &str,
    arity: u16,
    index: usize,
) -> Option<OsirisArgumentDisposition> {
    osiris_argument_disposition_in(
        OSIRIS_CONTRACTS,
        OSIRIS_ARGUMENT_DOMAIN_RECORDS,
        kind,
        name,
        arity,
        index,
    )
}

/// Returns a disposition from a supplied contract catalog and ledger.
pub fn osiris_argument_disposition_in(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
    kind: OsirisContractKind,
    name: &str,
    arity: u16,
    index: usize,
) -> Option<OsirisArgumentDisposition> {
    let record = osiris_argument_domain_record_in(contracts, records, kind, name, arity, index)?;
    Some(record.disposition)
}

/// Returns a resource kind from a supplied contract catalog and ledger.
pub fn osiris_argument_domain_in(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
    kind: OsirisContractKind,
    name: &str,
    arity: u16,
    index: usize,
) -> Option<&'static str> {
    osiris_argument_disposition_in(contracts, records, kind, name, arity, index)
        .and_then(OsirisArgumentDisposition::resource_domain)
}

/// Validates the partial checked-in overlay against a generated catalog.
pub fn validate_osiris_argument_domains(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
) -> Result<(), crate::Error> {
    validate_records(contracts, records, CURRENT_OSIRIS_CATALOG_PROVENANCE, false)
}

/// Validates a complete ledger, including one disposition for every input
/// `STRING` parameter in the supplied generated catalog.
pub fn validate_osiris_argument_domains_complete(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
) -> Result<(), crate::Error> {
    validate_records(contracts, records, CURRENT_OSIRIS_CATALOG_PROVENANCE, true)
}

/// Validates records against an explicitly supplied generated-catalog build.
/// This is useful for synthetic catalogs and for offline catalog generation.
pub fn validate_osiris_argument_domains_against(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
    provenance: OsirisCatalogProvenance,
    require_complete: bool,
) -> Result<(), crate::Error> {
    validate_records(contracts, records, provenance, require_complete)
}

/// Deterministic review coverage for the input `STRING` ledger.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OsirisArgumentDomainCoverage {
    pub input_string: usize,
    pub active_resources: usize,
    pub deferred_resources: usize,
    pub deferred_semantic_review: usize,
    pub runtime_identifiers: usize,
    pub enumerations: usize,
    pub expressions: usize,
    pub free_text: usize,
    pub negative_dispositions: usize,
    pub unreviewed: usize,
}

/// Reports deterministic coverage without changing parser behavior.
///
/// The report validates supplied records first. `unreviewed` is the number of
/// input `STRING` positions in `contracts` with no exact ledger record.
pub fn osiris_argument_domain_coverage(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
) -> Result<OsirisArgumentDomainCoverage, crate::Error> {
    osiris_argument_domain_coverage_against(contracts, records, CURRENT_OSIRIS_CATALOG_PROVENANCE)
}

/// Reports coverage against an explicitly supplied generated-catalog build.
pub fn osiris_argument_domain_coverage_against(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
    provenance: OsirisCatalogProvenance,
) -> Result<OsirisArgumentDomainCoverage, crate::Error> {
    validate_records(contracts, records, provenance, false)?;
    let mut coverage = OsirisArgumentDomainCoverage {
        input_string: 0,
        active_resources: 0,
        deferred_resources: 0,
        deferred_semantic_review: 0,
        runtime_identifiers: 0,
        enumerations: 0,
        expressions: 0,
        free_text: 0,
        negative_dispositions: 0,
        unreviewed: 0,
    };
    for record in records {
        match record.disposition {
            OsirisArgumentDisposition::Resource(_) => coverage.active_resources += 1,
            OsirisArgumentDisposition::DeferredResource(_) => coverage.deferred_resources += 1,
            OsirisArgumentDisposition::DeferredSemanticReview => {
                coverage.deferred_semantic_review += 1
            }
            OsirisArgumentDisposition::RuntimeIdentifier => coverage.runtime_identifiers += 1,
            OsirisArgumentDisposition::Enumeration => coverage.enumerations += 1,
            OsirisArgumentDisposition::Expression => coverage.expressions += 1,
            OsirisArgumentDisposition::FreeText => coverage.free_text += 1,
        }
    }
    coverage.negative_dispositions = coverage.runtime_identifiers
        + coverage.enumerations
        + coverage.expressions
        + coverage.free_text;
    for contract in contracts {
        for (index, parameter) in contract.parameters.iter().enumerate() {
            if parameter.type_name != "STRING"
                || !matches!(
                    parameter.direction,
                    OsirisParameterDirection::In | OsirisParameterDirection::InOut
                )
            {
                continue;
            }
            coverage.input_string += 1;
            let key = (
                contract.name,
                contract.kind,
                contract.parameters.len() as u16,
                index,
            );
            if records
                .binary_search_by(|record| record_key(record).cmp(&key))
                .is_err()
            {
                coverage.unreviewed += 1;
            }
        }
    }
    Ok(coverage)
}

fn validate_records(
    contracts: &[OsirisContractSpec],
    records: &[OsirisArgumentDomainRecord],
    provenance: OsirisCatalogProvenance,
    require_complete: bool,
) -> Result<(), crate::Error> {
    if provenance.source_version.trim().is_empty()
        || provenance.generator_version.trim().is_empty()
        || !is_sha256(provenance.source_hash)
    {
        return Err(crate::Error::Config(
            "generated catalog provenance is invalid".into(),
        ));
    }

    for pair in records.windows(2) {
        let [left, right] = pair else { unreachable!() };
        if record_key(left) >= record_key(right) {
            return Err(domain_error(
                right,
                "records are duplicated or out of order",
            ));
        }
    }

    for record in records {
        if record.name.trim().is_empty() || record.parameter_name.trim().is_empty() {
            return Err(domain_error(
                record,
                "callable and parameter names must not be empty",
            ));
        }
        let requires_official_evidence = matches!(
            record.disposition,
            OsirisArgumentDisposition::Resource(_) | OsirisArgumentDisposition::DeferredResource(_)
        );
        match (record.source_url, record.source_revision) {
            (Some(source_url), Some(source_revision))
                if valid_official_source(source_url, source_revision) => {}
            (None, None) if !requires_official_evidence => {}
            _ => {
                return Err(domain_error(record, "source URL and revision are invalid"));
            }
        }
        if !is_iso_date(record.reviewed_on) {
            return Err(domain_error(record, "review date must use YYYY-MM-DD"));
        }
        if record.catalog_source_version != provenance.source_version
            || record.catalog_source_hash != provenance.source_hash
            || record.catalog_generator_version != provenance.generator_version
        {
            return Err(domain_error(
                record,
                "generated catalog provenance is stale",
            ));
        }
        if let OsirisArgumentDisposition::Resource(domain)
        | OsirisArgumentDisposition::DeferredResource(domain) = record.disposition
            && domain.as_str().trim().is_empty()
        {
            return Err(domain_error(record, "resource domain must not be empty"));
        }
        if !is_sha256(record.catalog_source_hash) {
            return Err(domain_error(
                record,
                "generated catalog hash must be SHA-256 hex",
            ));
        }

        let contract =
            osiris_contract_by_kind(contracts, record.kind, record.name, record.arity)
                .ok_or_else(|| domain_error(record, "key does not match exactly one contract"))?;
        let parameter = contract
            .parameters
            .get(record.index)
            .ok_or_else(|| domain_error(record, "argument index is outside the contract"))?;
        if parameter.name != record.parameter_name
            || parameter.type_name != record.parameter_type
            || parameter.direction != record.direction
        {
            return Err(domain_error(record, "parameter metadata is stale"));
        }
        if record.parameter_type != "STRING"
            || !matches!(
                record.direction,
                OsirisParameterDirection::In | OsirisParameterDirection::InOut
            )
        {
            return Err(domain_error(
                record,
                "record must describe an input STRING parameter",
            ));
        }
    }

    if require_complete {
        for contract in contracts {
            for (index, parameter) in contract.parameters.iter().enumerate() {
                if parameter.type_name != "STRING"
                    || !matches!(
                        parameter.direction,
                        OsirisParameterDirection::In | OsirisParameterDirection::InOut
                    )
                {
                    continue;
                }
                let key = (
                    contract.name,
                    contract.kind,
                    contract.parameters.len() as u16,
                    index,
                );
                if records
                    .binary_search_by(|record| record_key(record).cmp(&key))
                    .is_err()
                {
                    return Err(crate::Error::Config(format!(
                        "Osiris argument-domain ledger has an unreviewed input STRING at {} {:?}/{}/{}",
                        contract.name,
                        contract.kind,
                        contract.parameters.len(),
                        index
                    )));
                }
            }
        }
    }
    Ok(())
}

fn record_key(record: &OsirisArgumentDomainRecord) -> (&str, OsirisContractKind, u16, usize) {
    (record.name, record.kind, record.arity, record.index)
}

fn domain_error(record: &OsirisArgumentDomainRecord, message: &str) -> crate::Error {
    crate::Error::Config(format!(
        "Osiris argument-domain record {} {:?}/{}/{}: {message}",
        record.name, record.kind, record.arity, record.index
    ))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_official_source(source_url: &str, source_revision: u32) -> bool {
    if source_revision == 0 {
        return false;
    }
    let Some((base, query)) = source_url.split_once('?') else {
        return false;
    };
    if base != "https://docs.baldursgate3.game/index.php" {
        return false;
    }
    let mut has_title = false;
    let mut oldid = None;
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        match key {
            "title" => has_title = !value.is_empty(),
            "oldid" => oldid = Some(value),
            _ => {}
        }
    }
    let expected_revision = source_revision.to_string();
    has_title && oldid == Some(expected_revision.as_str())
}

fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return false;
    }
    let year =
        digit(bytes[0]) * 1000 + digit(bytes[1]) * 100 + digit(bytes[2]) * 10 + digit(bytes[3]);
    let month = digit(bytes[5]) * 10 + digit(bytes[6]);
    let day = digit(bytes[8]) * 10 + digit(bytes[9]);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => return false,
    };
    year != 0 && (1..=days_in_month).contains(&day)
}

fn digit(byte: u8) -> u16 {
    u16::from(byte - b'0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osiris_catalog::{OSIRIS_CONTRACTS, OsirisParameterSpec, osiris_argument_domain};

    fn status_record() -> OsirisArgumentDomainRecord {
        *osiris_argument_domain_record(OsirisContractKind::Call, "RemoveStatus", 3, 1)
            .expect("reviewed RemoveStatus domain")
    }

    #[test]
    fn checked_in_records_are_valid_and_exact() {
        validate_osiris_argument_domains(OSIRIS_CONTRACTS, OSIRIS_ARGUMENT_DOMAIN_RECORDS)
            .expect("valid checked-in records");
        assert_eq!(
            osiris_argument_domain(OsirisContractKind::Call, "RemoveStatus", 3, 1),
            Some("StatusData")
        );
    }

    #[test]
    fn validation_rejects_stale_metadata_and_provenance() {
        let mut stale = status_record();
        stale.parameter_name = "_Wrong";
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &[stale]).is_err());
        let mut stale = status_record();
        stale.parameter_type = "GUIDSTRING";
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &[stale]).is_err());
        let mut stale = status_record();
        stale.direction = OsirisParameterDirection::Out;
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &[stale]).is_err());
        let mut stale = status_record();
        stale.catalog_source_hash = "0";
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &[stale]).is_err());
        let mut stale = status_record();
        stale.reviewed_on = "2026-02-30";
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &[stale]).is_err());
        let mut stale = status_record();
        stale.source_url =
            Some("https://docs.baldursgate3.game/index.php?title=RemoveStatus&oldid=0");
        stale.source_revision = Some(0);
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &[stale]).is_err());
        let mut stale = status_record();
        stale.disposition =
            OsirisArgumentDisposition::DeferredResource(OsirisResourceDomain::Custom(""));
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &[stale]).is_err());
    }

    #[test]
    fn lookup_requires_the_complete_contract_identity() {
        assert!(
            osiris_argument_domain_record(OsirisContractKind::Event, "RemoveStatus", 3, 1)
                .is_none()
        );
        assert!(
            osiris_argument_domain_record(OsirisContractKind::Call, "RemoveStatus", 4, 1).is_none()
        );
        assert!(
            osiris_argument_domain_record(OsirisContractKind::Call, "RemoveStatus", 3, 3).is_none()
        );
        assert!(osiris_argument_domain_record(OsirisContractKind::Call, "Unknown", 3, 1).is_none());
    }

    #[test]
    fn complete_validation_rejects_unreviewed_input_strings() {
        const PARAMETERS: &[OsirisParameterSpec] = &[OsirisParameterSpec {
            direction: OsirisParameterDirection::In,
            type_name: "STRING",
            name: "_Value",
        }];
        const CONTRACTS: &[OsirisContractSpec] = &[OsirisContractSpec {
            kind: OsirisContractKind::Call,
            name: "Synthetic",
            parameters: PARAMETERS,
        }];
        assert!(validate_osiris_argument_domains_complete(CONTRACTS, &[]).is_err());
    }

    #[test]
    fn catalog_only_dispositions_and_coverage_are_conservative() {
        const PARAMETERS: &[OsirisParameterSpec] = &[
            OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "STRING",
                name: "_Runtime",
            },
            OsirisParameterSpec {
                direction: OsirisParameterDirection::In,
                type_name: "STRING",
                name: "_Resource",
            },
        ];
        const CONTRACTS: &[OsirisContractSpec] = &[OsirisContractSpec {
            kind: OsirisContractKind::Call,
            name: "Synthetic",
            parameters: PARAMETERS,
        }];
        const RECORDS: &[OsirisArgumentDomainRecord] = &[
            OsirisArgumentDomainRecord::catalog_only(
                OsirisContractKind::Call,
                "Synthetic",
                2,
                0,
                "_Runtime",
                "STRING",
                OsirisParameterDirection::In,
                OsirisArgumentDisposition::RuntimeIdentifier,
                "2026-09-01",
                "4.1.1.7398727",
                "4a2ca23f02f6b5b5eed91ed07e8290c16cadf5f28032e66a19c89d6c2697eaac",
                "bg3-ls-osiris-catalog-v1",
            ),
            OsirisArgumentDomainRecord::resource(
                OsirisContractKind::Call,
                "Synthetic",
                2,
                1,
                "_Resource",
                "STRING",
                OsirisParameterDirection::In,
                OsirisArgumentDisposition::DeferredResource(OsirisResourceDomain::StatusData),
                "https://docs.baldursgate3.game/index.php?title=Synthetic&oldid=1",
                1,
                "2026-09-01",
                "4.1.1.7398727",
                "4a2ca23f02f6b5b5eed91ed07e8290c16cadf5f28032e66a19c89d6c2697eaac",
                "bg3-ls-osiris-catalog-v1",
            ),
        ];
        let coverage = osiris_argument_domain_coverage_against(
            CONTRACTS,
            RECORDS,
            CURRENT_OSIRIS_CATALOG_PROVENANCE,
        )
        .expect("valid synthetic ledger");
        assert_eq!(coverage.input_string, 2);
        assert_eq!(coverage.deferred_resources, 1);
        assert_eq!(coverage.negative_dispositions, 1);
        assert_eq!(coverage.unreviewed, 0);
        assert_eq!(
            osiris_argument_disposition_in(
                CONTRACTS,
                RECORDS,
                OsirisContractKind::Call,
                "Synthetic",
                2,
                1,
            ),
            Some(OsirisArgumentDisposition::DeferredResource(
                OsirisResourceDomain::StatusData
            ))
        );
        assert_eq!(
            osiris_argument_domain_in(
                CONTRACTS,
                RECORDS,
                OsirisContractKind::Call,
                "Synthetic",
                2,
                1
            ),
            None
        );
    }

    #[test]
    fn record_order_and_duplicate_keys_are_checked() {
        let records = [
            OSIRIS_ARGUMENT_DOMAIN_RECORDS[1],
            OSIRIS_ARGUMENT_DOMAIN_RECORDS[0],
        ];
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &records).is_err());
        let status_record = status_record();
        let records = [status_record, status_record];
        assert!(validate_osiris_argument_domains(OSIRIS_CONTRACTS, &records).is_err());
    }
}
