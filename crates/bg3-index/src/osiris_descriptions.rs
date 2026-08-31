//! Curated, human-readable descriptions for engine Osiris callables.
//!
//! `story_header.div` describes callable shape, but it does not document
//! behavior. Keep behavioral text in a separate reviewed overlay so generated
//! contract data remains reproducible and the overlay can grow independently.
//!
//! Descriptions are concise paraphrases reviewed against the official BG3
//! Modding documentation at `https://docs.baldursgate3.game/`. Do not infer
//! descriptions from callable names or apply one across unverified overloads.

use crate::osiris_catalog::{
    OSIRIS_CONTRACTS, OsirisContractKind, OsirisContractSpec, osiris_contract,
    osiris_contract_by_kind,
};

/// Version of the curated Osiris description record format.
pub const OSIRIS_DESCRIPTION_CATALOG_VERSION: &str = "bg3-ls-osiris-descriptions-v1";

/// One reviewed behavioral description and its source provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OsirisDescriptionRecord {
    pub kind: OsirisContractKind,
    pub name: &'static str,
    pub arity: u16,
    pub description: &'static str,
    pub source_url: &'static str,
    /// MediaWiki revision ID used for the review.
    pub source_revision: u32,
    /// ISO 8601 calendar date of the latest prose update and review.
    pub reviewed_on: &'static str,
}

/// Curated descriptions sorted by name, kind, and arity.
pub const OSIRIS_DESCRIPTION_RECORDS: &[OsirisDescriptionRecord] = &[
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Call,
        name: "AddPassive",
        arity: 2,
        description: "Adds the named passive to the specified entity.",
        source_url: "https://docs.baldursgate3.game/index.php?title=AddPassive&oldid=3067",
        source_revision: 3067,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Call,
        name: "ApplyStatus",
        arity: 5,
        description: "Applies the named status to an object for the requested duration, with force behavior and source attribution.",
        source_url: "https://docs.baldursgate3.game/index.php?title=ApplyStatus&oldid=2246",
        source_revision: 2246,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Event,
        name: "CastedSpell",
        arity: 5,
        description: "Event raised after a spell is cast, with the caster, spell, spell type, spell element, and story action ID.",
        source_url: "https://docs.baldursgate3.game/index.php?title=CastedSpell&oldid=2269",
        source_revision: 2269,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Query,
        name: "Exists",
        arity: 2,
        description: "Tests whether the specified object exists.",
        source_url: "https://docs.baldursgate3.game/index.php?title=Exists&oldid=1951",
        source_revision: 1951,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Query,
        name: "GetActionResourceValuePersonal",
        arity: 4,
        description: "Returns a character's value for the named action resource at the requested resource level.",
        source_url: "https://docs.baldursgate3.game/index.php?title=GetActionResourceValuePersonal&oldid=3198",
        source_revision: 3198,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Query,
        name: "GetDistanceTo",
        arity: 3,
        description: "Returns the distance between the two specified objects.",
        source_url: "https://docs.baldursgate3.game/index.php?title=GetDistanceTo&oldid=1650",
        source_revision: 1650,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Query,
        name: "HasPassive",
        arity: 3,
        description: "Reports whether the specified entity has the named passive (0 for false, 1 for true).",
        source_url: "https://docs.baldursgate3.game/index.php?title=HasPassive&oldid=3073",
        source_revision: 3073,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Call,
        name: "RemovePassive",
        arity: 2,
        description: "Removes the named passive from the specified entity.",
        source_url: "https://docs.baldursgate3.game/index.php?title=RemovePassive&oldid=3069",
        source_revision: 3069,
        reviewed_on: "2026-08-31",
    },
    OsirisDescriptionRecord {
        kind: OsirisContractKind::Event,
        name: "UsingSpell",
        arity: 5,
        description: "Event raised when a character begins using a spell, with the caster, spell, spell type, spell element, and story action ID.",
        source_url: "https://docs.baldursgate3.game/index.php?title=UsingSpell&oldid=2279",
        source_revision: 2279,
        reviewed_on: "2026-08-31",
    },
];

/// Returns a description with exact callable kind, name, and arity identity.
pub fn osiris_callable_description_for_kind(
    kind: OsirisContractKind,
    name: &str,
    arity: u16,
) -> Option<&'static str> {
    OSIRIS_DESCRIPTION_RECORDS
        .binary_search_by(|record| {
            record
                .name
                .cmp(name)
                .then_with(|| record.kind.cmp(&kind))
                .then_with(|| record.arity.cmp(&arity))
        })
        .ok()
        .map(|index| OSIRIS_DESCRIPTION_RECORDS[index].description)
}

/// Returns a description for the unique generated contract with this name and arity.
///
/// New callers should use [`osiris_callable_description_for_kind`] so a record
/// cannot cross callable roles. This wrapper preserves the original public API
/// and remains conservative if the generated catalog contains a cross-kind
/// name and arity collision.
pub fn osiris_callable_description(name: &str, arity: u16) -> Option<&'static str> {
    let contract = osiris_contract(OSIRIS_CONTRACTS, name, arity)?;
    osiris_callable_description_for_kind(contract.kind, name, arity)
}

/// Validates curated records against exact generated callable identities.
pub fn validate_osiris_descriptions(
    contracts: &[OsirisContractSpec],
    records: &[OsirisDescriptionRecord],
) -> Result<(), crate::Error> {
    for pair in records.windows(2) {
        let [left, right] = pair else {
            unreachable!("a two-item window always has two records")
        };
        let order = left
            .name
            .cmp(right.name)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.arity.cmp(&right.arity));
        if !order.is_lt() {
            return Err(crate::Error::Config(format!(
                "Osiris description records are duplicated or out of order at {} {:?}/{}",
                right.name, right.kind, right.arity
            )));
        }
    }

    for record in records {
        if record.name.trim().is_empty() || record.description.trim().is_empty() {
            return Err(description_error(
                record,
                "name and description must not be empty",
            ));
        }
        let expected_source_url = format!(
            "https://docs.baldursgate3.game/index.php?title={}&oldid={}",
            record.name, record.source_revision
        );
        if record.source_revision == 0 || record.source_url != expected_source_url {
            return Err(description_error(
                record,
                "source URL and revision must identify official BG3 Modding documentation",
            ));
        }
        if !is_iso_date(record.reviewed_on) {
            return Err(description_error(record, "review date must use YYYY-MM-DD"));
        }
        if osiris_contract_by_kind(contracts, record.kind, record.name, record.arity).is_none() {
            return Err(description_error(
                record,
                "key does not match exactly one generated contract",
            ));
        }
    }
    Ok(())
}

fn description_error(record: &OsirisDescriptionRecord, message: &str) -> crate::Error {
    crate::Error::Config(format!(
        "Osiris description {} {:?}/{}: {message}",
        record.name, record.kind, record.arity
    ))
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
    use crate::OSIRIS_CONTRACTS;

    #[test]
    fn checked_in_description_records_are_valid() {
        validate_osiris_descriptions(OSIRIS_CONTRACTS, OSIRIS_DESCRIPTION_RECORDS)
            .expect("valid checked-in descriptions");
    }

    #[test]
    fn returns_descriptions_for_exact_curated_callables() {
        for (kind, name, arity, expected) in [
            (OsirisContractKind::Call, "AddPassive", 2, "named passive"),
            (OsirisContractKind::Call, "ApplyStatus", 5, "force behavior"),
            (OsirisContractKind::Event, "CastedSpell", 5, "spell is cast"),
            (OsirisContractKind::Query, "Exists", 2, "exists"),
            (
                OsirisContractKind::Query,
                "GetActionResourceValuePersonal",
                4,
                "action resource",
            ),
            (OsirisContractKind::Query, "GetDistanceTo", 3, "distance"),
            (OsirisContractKind::Query, "HasPassive", 3, "named passive"),
            (
                OsirisContractKind::Call,
                "RemovePassive",
                2,
                "named passive",
            ),
            (OsirisContractKind::Event, "UsingSpell", 5, "begins using"),
        ] {
            assert!(
                osiris_callable_description_for_kind(kind, name, arity)
                    .expect("description")
                    .contains(expected),
                "{name}"
            );
            assert!(osiris_callable_description(name, arity).is_some(), "{name}");
        }
    }

    #[test]
    fn exact_lookup_rejects_wrong_kind_arity_case_and_unknown_names() {
        assert_eq!(
            osiris_callable_description_for_kind(OsirisContractKind::Call, "HasPassive", 3),
            None
        );
        assert_eq!(
            osiris_callable_description_for_kind(OsirisContractKind::Query, "HasPassive", 2),
            None
        );
        assert_eq!(
            osiris_callable_description_for_kind(OsirisContractKind::Query, "hasPassive", 3),
            None
        );
        assert_eq!(
            osiris_callable_description_for_kind(OsirisContractKind::Call, "AutoSave", 0),
            None
        );
    }

    #[test]
    fn validation_rejects_stale_wrong_and_duplicate_identities() {
        let mut stale = OSIRIS_DESCRIPTION_RECORDS[0];
        stale.name = "Missing";
        stale.source_url = "https://docs.baldursgate3.game/index.php?title=Missing&oldid=3067";
        let error =
            validate_osiris_descriptions(OSIRIS_CONTRACTS, &[stale]).expect_err("stale key");
        assert!(error.to_string().contains("does not match exactly one"));

        let mut wrong_kind = OSIRIS_DESCRIPTION_RECORDS[0];
        wrong_kind.kind = OsirisContractKind::Query;
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[wrong_kind]).is_err());

        let mut wrong_arity = OSIRIS_DESCRIPTION_RECORDS[0];
        wrong_arity.arity = 3;
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[wrong_arity]).is_err());

        let duplicate = [OSIRIS_DESCRIPTION_RECORDS[0], OSIRIS_DESCRIPTION_RECORDS[0]];
        let error =
            validate_osiris_descriptions(OSIRIS_CONTRACTS, &duplicate).expect_err("duplicate key");
        assert!(error.to_string().contains("duplicated or out of order"));

        let contract =
            *osiris_contract_by_kind(OSIRIS_CONTRACTS, OsirisContractKind::Call, "AddPassive", 2)
                .expect("generated AddPassive contract");
        let duplicate_contracts = [contract, contract];
        assert!(
            validate_osiris_descriptions(&duplicate_contracts, &OSIRIS_DESCRIPTION_RECORDS[..1])
                .is_err()
        );
    }

    #[test]
    fn validation_rejects_missing_provenance_and_review_metadata() {
        let mut missing_source = OSIRIS_DESCRIPTION_RECORDS[0];
        missing_source.source_url = "";
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[missing_source]).is_err());

        let mut missing_revision = OSIRIS_DESCRIPTION_RECORDS[0];
        missing_revision.source_revision = 0;
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[missing_revision]).is_err());

        let mut mismatched_source = OSIRIS_DESCRIPTION_RECORDS[0];
        mismatched_source.source_url =
            "https://docs.baldursgate3.game/index.php?title=ApplyStatus&oldid=3067";
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[mismatched_source]).is_err());

        let mut mismatched_revision = OSIRIS_DESCRIPTION_RECORDS[0];
        mismatched_revision.source_revision = 1;
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[mismatched_revision]).is_err());

        let mut invalid_review_date = OSIRIS_DESCRIPTION_RECORDS[0];
        invalid_review_date.reviewed_on = "2026/08/31";
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[invalid_review_date]).is_err());

        invalid_review_date.reviewed_on = "2026-02-30";
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[invalid_review_date]).is_err());

        let mut leap_day = OSIRIS_DESCRIPTION_RECORDS[0];
        leap_day.reviewed_on = "2024-02-29";
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[leap_day]).is_ok());
        leap_day.reviewed_on = "2025-02-29";
        assert!(validate_osiris_descriptions(OSIRIS_CONTRACTS, &[leap_day]).is_err());
    }
}
