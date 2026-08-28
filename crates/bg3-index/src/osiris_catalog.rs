//! Parsing and rendering for the engine's generated Osiris contract catalog.
//!
//! The game writes `story_header.div` as a plain-text declaration file.  This
//! module deliberately treats it as build input only: the language server can
//! ship the generated contract data without requiring a game installation.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[rustfmt::skip]
mod generated_osiris_catalog;

pub use generated_osiris_catalog::{
    OSIRIS_CATALOG_GENERATOR_VERSION as GENERATED_OSIRIS_CATALOG_GENERATOR_VERSION,
    OSIRIS_CATALOG_SOURCE_HASH, OSIRIS_CATALOG_SOURCE_VERSION, OSIRIS_CONTRACTS,
};

/// Version of the checked-in generated contract format.
pub const OSIRIS_CATALOG_GENERATOR_VERSION: &str = "bg3-ls-osiris-catalog-v1";

/// The generic GUID value type used by the BG3 Osiris compiler.
pub const OSIRIS_GUIDSTRING_TYPE: &str = "GUIDSTRING";

/// BG3 GUID aliases verified for the generated contract catalog's source
/// build. Each name has GUIDSTRING as its intrinsic type, but two different
/// specialized aliases are not interchangeable. Keep this list tied to the
/// versioned catalog until alias metadata is emitted by the catalog
/// generator; unknown names must remain unresolved.
pub const OSIRIS_GUID_ALIASES: &[&str] = &[
    "ANIMATION",
    "CHARACTER",
    "CHARACTERROOT",
    "DIALOGRESOURCE",
    "DIFFICULTYCLASS",
    "DISTURBANCEPROPERTY",
    "DLC",
    "EFFECTRESOURCE",
    "FACTION",
    "FLAG",
    "GOLDREWARD",
    "ITEM",
    "ITEMROOT",
    "LEVELTEMPLATE",
    "PLATFORM",
    "ROOT",
    "RULESETMODIFIER",
    "SHAPESHIFTRULE",
    "SPLINE",
    "TAG",
    "TIMELINERESOURCE",
    "TRIGGER",
    "TUTORIALEVENT",
    "UNIFIEDTUTORIAL",
    "VOICEBARKRESOURCE",
];

const OSIRIS_INTRINSIC_TYPES: &[&str] = &["INTEGER", "INTEGER64", "REAL", "STRING"];

/// Returns whether two proven Osiris type names are compatible for a
/// database argument.
///
/// `Some(true)` means that the names are equal or that one is the generic
/// `GUIDSTRING` type and the other is a verified BG3 GUID alias. `Some(false)`
/// means that both names are verified, distinct specialized GUID aliases.
/// `None` means that the relationship is not known and callers must stay
/// silent rather than guessing. The same rule applies to non-GUID types: an
/// exact spelling is compatible, while different or unknown spellings are not
/// proven compatible.
pub fn osiris_type_compatibility(left: &str, right: &str) -> Option<bool> {
    if left == right {
        return Some(true);
    }

    let left_is_guid_alias = left == OSIRIS_GUIDSTRING_TYPE || OSIRIS_GUID_ALIASES.contains(&left);
    let right_is_guid_alias =
        right == OSIRIS_GUIDSTRING_TYPE || OSIRIS_GUID_ALIASES.contains(&right);
    if left_is_guid_alias && right_is_guid_alias {
        if left == OSIRIS_GUIDSTRING_TYPE || right == OSIRIS_GUIDSTRING_TYPE {
            Some(true)
        } else {
            Some(false)
        }
    } else if is_known_osiris_type(left) && is_known_osiris_type(right) {
        Some(false)
    } else {
        None
    }
}

fn is_known_osiris_type(type_name: &str) -> bool {
    OSIRIS_INTRINSIC_TYPES.contains(&type_name)
        || type_name == OSIRIS_GUIDSTRING_TYPE
        || OSIRIS_GUID_ALIASES.contains(&type_name)
        || OSIRIS_CONTRACTS.iter().any(|contract| {
            contract
                .parameters
                .iter()
                .any(|parameter| parameter.type_name == type_name)
        })
}

/// The direction of one Osiris query parameter.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum OsirisParameterDirection {
    In,
    /// Project-level compatibility for synthetic contracts. Canonical BG3
    /// headers expose only `[in]` and `[out]`; parser binding remains
    /// conservative when no generated contract attests this direction.
    InOut,
    Out,
}

/// The kind of a declaration in `story_header.div`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum OsirisContractKind {
    Call,
    Event,
    Query,
    Syscall,
    Sysquery,
}

impl OsirisContractKind {
    fn from_keyword(keyword: &str) -> Option<Self> {
        Some(match keyword {
            "call" => Self::Call,
            "event" => Self::Event,
            "query" => Self::Query,
            "syscall" => Self::Syscall,
            "sysquery" => Self::Sysquery,
            _ => return None,
        })
    }

    const fn rank(self) -> u8 {
        match self {
            Self::Call => 0,
            Self::Event => 1,
            Self::Query => 2,
            Self::Syscall => 3,
            Self::Sysquery => 4,
        }
    }
}

/// One parameter from an engine Osiris declaration.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct OsirisParameter {
    pub direction: OsirisParameterDirection,
    pub type_name: String,
    pub name: String,
}

/// One engine Osiris call, event, or query contract.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct OsirisContract {
    pub kind: OsirisContractKind,
    pub name: String,
    pub parameters: Vec<OsirisParameter>,
}

impl OsirisContract {
    /// Returns the contract arity as the index's stable integer width.
    pub fn arity(&self) -> u16 {
        self.parameters.len().try_into().unwrap_or(u16::MAX)
    }
}

/// Provenance attached to one generated contract catalog.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisCatalogMetadata {
    /// Exact game build identifier read from the game installation.
    pub source_version: String,
    /// Digest of the source header, normally SHA-256 encoded as lowercase hex.
    pub source_hash: String,
    /// Version of the generator's output format.
    pub generator_version: String,
}

/// Parsed, sorted engine contracts and their source provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisCatalog {
    pub metadata: OsirisCatalogMetadata,
    pub contracts: Vec<OsirisContract>,
}

/// Parses a header and returns the complete deterministic generated Rust
/// module. `game_version` must be the exact build identifier read from the
/// game installation; it is kept as provenance and is never inferred from
/// the header's internal numeric IDs.
pub fn generate_osiris_catalog(source: &str, game_version: &str) -> Result<String, crate::Error> {
    if game_version.trim().is_empty() {
        return Err(crate::Error::Config(
            "the Osiris catalog source version must not be empty".into(),
        ));
    }
    let source_hash = Sha256::digest(source.as_bytes());
    let source_hash = source_hash
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let catalog = parse_story_header(
        source,
        OsirisCatalogMetadata {
            source_version: game_version.trim().to_owned(),
            source_hash,
            generator_version: OSIRIS_CATALOG_GENERATOR_VERSION.into(),
        },
    )?;
    if catalog.contracts.is_empty() {
        return Err(crate::Error::Parse(
            "story_header.div contains no engine declarations".into(),
        ));
    }
    Ok(render_osiris_catalog(&catalog))
}

/// Parses one generated `story_header.div` source.
pub fn parse_story_header(
    source: &str,
    metadata: OsirisCatalogMetadata,
) -> Result<OsirisCatalog, crate::Error> {
    let mut contracts = Vec::new();
    for (line_number, line) in source.lines().enumerate() {
        let line = line.trim();
        let Some(split) = line.find(char::is_whitespace) else {
            if OsirisContractKind::from_keyword(line).is_some() {
                return Err(parse_error(
                    line_number,
                    "an Osiris declaration has no name",
                ));
            }
            continue;
        };
        let (keyword, declaration) = line.split_at(split);
        let Some(kind) = OsirisContractKind::from_keyword(keyword) else {
            continue;
        };
        let declaration = declaration.trim_start();
        let (name, rest) = parse_identifier(declaration)
            .ok_or_else(|| parse_error(line_number, "an Osiris declaration has no valid name"))?;
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('(') else {
            return Err(parse_error(
                line_number,
                "an Osiris declaration has no parameter list",
            ));
        };
        let Some(close) = find_parameter_list_end(rest) else {
            return Err(parse_error(
                line_number,
                "an Osiris declaration has an unterminated parameter list",
            ));
        };
        let parameter_text = &rest[..close];
        let parameters = parse_parameters(parameter_text, kind).map_err(|message| {
            parse_error(
                line_number,
                format!("invalid parameters for {name}: {message}"),
            )
        })?;
        if parameters.len() > u16::MAX as usize {
            return Err(parse_error(
                line_number,
                format!("{name} has more than 65,535 parameters"),
            ));
        }
        contracts.push(OsirisContract {
            kind,
            name: name.to_owned(),
            parameters,
        });
    }

    contracts.sort_by(contract_order);
    validate_contracts(&contracts)?;
    contracts.dedup();
    Ok(OsirisCatalog {
        metadata,
        contracts,
    })
}

/// Renders a catalog as deterministic Rust source suitable for `include!` or
/// a checked-in generated module.
pub fn render_osiris_catalog(catalog: &OsirisCatalog) -> String {
    let mut contracts = catalog.contracts.clone();
    contracts.sort_by(contract_order);
    let mut output = String::from(concat!(
        "// @generated by bg3-ls; do not edit.\n",
        "use super::{OsirisContractKind, OsirisContractSpec, OsirisParameterDirection, OsirisParameterSpec};\n\n",
    ));
    output.push_str(&format!(
        "pub const OSIRIS_CATALOG_SOURCE_VERSION: &str = {:?};\n",
        catalog.metadata.source_version
    ));
    output.push_str(&format!(
        "pub const OSIRIS_CATALOG_SOURCE_HASH: &str = {:?};\n",
        catalog.metadata.source_hash
    ));
    output.push_str(&format!(
        "pub const OSIRIS_CATALOG_GENERATOR_VERSION: &str = {:?};\n\n",
        catalog.metadata.generator_version
    ));
    output.push_str("pub const OSIRIS_CONTRACTS: &[OsirisContractSpec] = &[\n");
    for (index, contract) in contracts.iter().enumerate() {
        output.push_str(&format!(
            "    OsirisContractSpec {{ kind: {}, name: {:?}, parameters: OSIRIS_PARAMETERS_{index} }},\n",
            rust_contract_kind(contract.kind),
            contract.name,
        ));
    }
    output.push_str("];\n\n");
    for (index, contract) in contracts.iter().enumerate() {
        output.push_str(&format!(
            "const OSIRIS_PARAMETERS_{index}: &[OsirisParameterSpec] = &[\n"
        ));
        for parameter in &contract.parameters {
            output.push_str(&format!(
                "    OsirisParameterSpec {{ direction: {}, type_name: {:?}, name: {:?} }},\n",
                rust_parameter_direction(parameter.direction),
                parameter.type_name,
                parameter.name,
            ));
        }
        output.push_str("];\n");
    }
    output
}

/// A static-friendly parameter contract used by generated catalogs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OsirisParameterSpec {
    pub direction: OsirisParameterDirection,
    pub type_name: &'static str,
    pub name: &'static str,
}

/// A static-friendly engine contract used by generated catalogs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OsirisContractSpec {
    pub kind: OsirisContractKind,
    pub name: &'static str,
    pub parameters: &'static [OsirisParameterSpec],
}

/// Looks up a generated static contract by exact name and arity.
pub fn osiris_contract<'a>(
    contracts: &'a [OsirisContractSpec],
    name: &str,
    arity: u16,
) -> Option<&'a OsirisContractSpec> {
    // Generated catalogs are sorted by name. Find the name range first so a
    // lookup does not scan the complete catalog, then reject an ambiguous
    // name/arity pair instead of selecting an arbitrary declaration kind.
    let start = contracts.partition_point(|contract| contract.name < name);
    let named = &contracts[start..];
    let end = named.partition_point(|contract| contract.name == name);
    let mut matches = named[..end]
        .iter()
        .filter(|contract| contract.parameters.len() as u16 == arity);
    let contract = matches.next()?;
    matches.next().is_none().then_some(contract)
}

/// Looks up an engine event in the checked-in generated catalog.
pub fn osiris_event_contract(name: &str, arity: u16) -> Option<&'static OsirisContractSpec> {
    osiris_contract(OSIRIS_CONTRACTS, name, arity)
        .filter(|contract| contract.kind == OsirisContractKind::Event)
}

fn parse_parameters(text: &str, kind: OsirisContractKind) -> Result<Vec<OsirisParameter>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    text.split(',')
        .map(|raw| {
            let mut parameter = raw.trim();
            let direction = if let Some(rest) = parameter.strip_prefix("[inout]") {
                parameter = rest.trim_start();
                OsirisParameterDirection::InOut
            } else if let Some(rest) = parameter.strip_prefix("[in]") {
                parameter = rest.trim_start();
                OsirisParameterDirection::In
            } else if let Some(rest) = parameter.strip_prefix("[out]") {
                parameter = rest.trim_start();
                OsirisParameterDirection::Out
            } else {
                if matches!(
                    kind,
                    OsirisContractKind::Query | OsirisContractKind::Sysquery
                ) {
                    return Err("queries require [in] or [out] directions".into());
                }
                OsirisParameterDirection::In
            };
            let Some(rest) = parameter.strip_prefix('(') else {
                return Err("parameter has no type".into());
            };
            let Some(type_end) = rest.find(')') else {
                return Err("parameter type is unterminated".into());
            };
            let type_name = rest[..type_end].trim();
            if !is_identifier(type_name) {
                return Err("parameter type is not an identifier".into());
            }
            let name = rest[type_end + 1..].trim();
            if !is_identifier(name) {
                return Err("parameter name is not an identifier".into());
            }
            Ok(OsirisParameter {
                direction,
                type_name: type_name.to_owned(),
                name: name.to_owned(),
            })
        })
        .collect()
}

fn find_parameter_list_end(value: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (index, character) in value.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_identifier(value: &str) -> Option<(&str, &str)> {
    let end = value
        .char_indices()
        .find(|(_, character)| !character.is_ascii_alphanumeric() && *character != '_')
        .map_or(value.len(), |(index, _)| index);
    let identifier = &value[..end];
    is_identifier(identifier).then_some((identifier, &value[end..]))
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|character| {
        (character.is_ascii_alphabetic() || character == '_')
            && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

fn contract_order(left: &OsirisContract, right: &OsirisContract) -> Ordering {
    left.name
        .cmp(&right.name)
        .then_with(|| left.kind.rank().cmp(&right.kind.rank()))
        .then_with(|| left.parameters.cmp(&right.parameters))
}

fn rust_contract_kind(kind: OsirisContractKind) -> &'static str {
    match kind {
        OsirisContractKind::Call => "OsirisContractKind::Call",
        OsirisContractKind::Event => "OsirisContractKind::Event",
        OsirisContractKind::Query => "OsirisContractKind::Query",
        OsirisContractKind::Syscall => "OsirisContractKind::Syscall",
        OsirisContractKind::Sysquery => "OsirisContractKind::Sysquery",
    }
}

fn rust_parameter_direction(direction: OsirisParameterDirection) -> &'static str {
    match direction {
        OsirisParameterDirection::In => "OsirisParameterDirection::In",
        OsirisParameterDirection::InOut => "OsirisParameterDirection::InOut",
        OsirisParameterDirection::Out => "OsirisParameterDirection::Out",
    }
}

fn validate_contracts(contracts: &[OsirisContract]) -> Result<(), crate::Error> {
    for pair in contracts.windows(2) {
        let [left, right] = pair else {
            unreachable!("a two-item window always has two contracts")
        };
        if left.kind == right.kind
            && left.name == right.name
            && left.parameters.len() == right.parameters.len()
            && left.parameters != right.parameters
        {
            return Err(crate::Error::Parse(format!(
                "story_header.div contains conflicting {} declarations for {} with arity {}",
                contract_kind_name(left.kind),
                left.name,
                left.parameters.len()
            )));
        }
    }
    Ok(())
}

fn contract_kind_name(kind: OsirisContractKind) -> &'static str {
    match kind {
        OsirisContractKind::Call => "call",
        OsirisContractKind::Event => "event",
        OsirisContractKind::Query => "query",
        OsirisContractKind::Syscall => "syscall",
        OsirisContractKind::Sysquery => "sysquery",
    }
}

fn parse_error(line: usize, message: impl Into<String>) -> crate::Error {
    crate::Error::Parse(format!(
        "story_header.div line {}: {}",
        line + 1,
        message.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> OsirisCatalogMetadata {
        OsirisCatalogMetadata {
            source_version: "4.1.1.7398727".into(),
            source_hash: "abc123".into(),
            generator_version: "osiris-catalog-v1".into(),
        }
    }

    #[test]
    fn parses_calls_events_and_directional_queries() {
        let catalog = parse_story_header(
            "// comment\nevent CastedSpell((GUIDSTRING)_Caster, (STRING)_Spell, (INTEGER)_ID) (3,0,1,1)\nquery IntegerSum([in](INTEGER)_A, [in](INTEGER)_B, [out](INTEGER)_Sum) (2,0,2,1)\ncall AutoSave() (1,0,3,1)\n",
            metadata(),
        )
        .expect("valid header");
        assert_eq!(catalog.contracts.len(), 3);
        let query = catalog
            .contracts
            .iter()
            .find(|contract| contract.name == "IntegerSum")
            .expect("query");
        assert_eq!(query.kind, OsirisContractKind::Query);
        assert_eq!(query.parameters[2].direction, OsirisParameterDirection::Out);
        assert_eq!(query.parameters[2].type_name, "INTEGER");
    }

    #[test]
    fn parses_inout_query_parameters() {
        let catalog =
            parse_story_header("query InOut([inout](STRING)_Value) (2,0,1,1)\n", metadata())
                .expect("valid header");
        assert_eq!(
            catalog.contracts[0].parameters[0].direction,
            OsirisParameterDirection::InOut
        );
        assert!(render_osiris_catalog(&catalog).contains("OsirisParameterDirection::InOut"));
    }

    #[test]
    fn sorts_and_deduplicates_contracts() {
        let catalog = parse_story_header(
            "call Z((STRING)_Value) (1,0,2,1)\ncall A() (1,0,1,1)\ncall Z((STRING)_Value) (1,0,2,1)\n",
            metadata(),
        )
        .expect("valid header");
        assert_eq!(
            catalog
                .contracts
                .iter()
                .map(|contract| contract.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "Z"]
        );
    }

    #[test]
    fn rejects_query_without_parameter_direction() {
        let error = parse_story_header("query Broken((INTEGER)_Value) (2,0,0,1)\n", metadata())
            .expect_err("missing direction");
        assert!(error.to_string().contains("require [in] or [out]"));
    }

    #[test]
    fn rejects_conflicting_same_arity_declarations() {
        let error = parse_story_header(
            "query Value([out](INTEGER)_Value) (2,0,1,1)\nquery Value([out](REAL)_Value) (2,0,2,1)\n",
            metadata(),
        )
        .expect_err("conflicting declaration");
        assert!(
            error
                .to_string()
                .contains("conflicting query declarations for Value with arity 1")
        );
    }

    #[test]
    fn rendering_is_deterministic_and_records_provenance() {
        let catalog = parse_story_header("call Z() (1,0,2,1)\ncall A() (1,0,1,1)\n", metadata())
            .expect("valid header");
        let rendered = render_osiris_catalog(&catalog);
        assert_eq!(rendered, render_osiris_catalog(&catalog));
        assert!(rendered.contains("OSIRIS_CATALOG_SOURCE_VERSION"));
        assert!(rendered.contains("4.1.1.7398727"));
        assert!(!rendered.contains("parameters: &OSIRIS_PARAMETERS_"));
        assert!(rendered.find("name: \"A\"").unwrap() < rendered.find("name: \"Z\"").unwrap());
    }

    #[test]
    fn generation_hashes_exact_source_bytes() {
        let rendered =
            generate_osiris_catalog("call A() (1,0,1,1)\n", "4.1.1.7398727").expect("valid header");
        assert!(rendered.contains(
            "OSIRIS_CATALOG_SOURCE_HASH: &str = \"cf019c0ad1b368d3496dc89b873c9919263ae385d9c57b0c5b8cd703e9f18e5c\""
        ));
        let hash_start = rendered
            .find("OSIRIS_CATALOG_SOURCE_HASH: &str = \"")
            .expect("hash metadata")
            + "OSIRIS_CATALOG_SOURCE_HASH: &str = \"".len();
        let hash_end = rendered[hash_start..]
            .find('"')
            .map(|offset| hash_start + offset)
            .expect("hash terminator");
        assert_eq!(hash_end - hash_start, 64);
        assert!(
            rendered[hash_start..hash_end]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn contract_lookup_uses_name_range_and_rejects_ambiguous_arity() {
        const NO_PARAMETERS: &[OsirisParameterSpec] = &[];
        const ONE_PARAMETER: &[OsirisParameterSpec] = &[OsirisParameterSpec {
            direction: OsirisParameterDirection::In,
            type_name: "GUIDSTRING",
            name: "_Value",
        }];
        let contracts = [
            OsirisContractSpec {
                kind: OsirisContractKind::Call,
                name: "Alpha",
                parameters: NO_PARAMETERS,
            },
            OsirisContractSpec {
                kind: OsirisContractKind::Event,
                name: "CastedSpell",
                parameters: ONE_PARAMETER,
            },
            OsirisContractSpec {
                kind: OsirisContractKind::Query,
                name: "CastedSpell",
                parameters: ONE_PARAMETER,
            },
            OsirisContractSpec {
                kind: OsirisContractKind::Call,
                name: "Omega",
                parameters: NO_PARAMETERS,
            },
        ];

        assert_eq!(
            osiris_contract(&contracts, "Alpha", 0).unwrap().name,
            "Alpha"
        );
        assert!(osiris_contract(&contracts, "Missing", 0).is_none());
        assert!(osiris_contract(&contracts, "CastedSpell", 0).is_none());
        assert!(osiris_contract(&contracts, "CastedSpell", 1).is_none());
        assert!(osiris_contract(&contracts, "Omega", 0).is_some());
    }

    #[test]
    fn contract_lookup_returns_unique_name_and_arity_match() {
        const NO_PARAMETERS: &[OsirisParameterSpec] = &[];
        const ONE_PARAMETER: &[OsirisParameterSpec] = &[OsirisParameterSpec {
            direction: OsirisParameterDirection::Out,
            type_name: "REAL",
            name: "_Value",
        }];
        let contracts = [
            OsirisContractSpec {
                kind: OsirisContractKind::Call,
                name: "Value",
                parameters: NO_PARAMETERS,
            },
            OsirisContractSpec {
                kind: OsirisContractKind::Call,
                name: "Value",
                parameters: ONE_PARAMETER,
            },
        ];

        assert_eq!(
            osiris_contract(&contracts, "Value", 1).unwrap().parameters[0].type_name,
            "REAL"
        );
    }
}
