//! BG3 source extraction, schema metadata, module indexes, and persistent caches.

mod annotation;
mod cache;
mod catalog;
mod discovery;
mod domain;
mod game_version;
mod localization;
mod module;
mod osiris_api;
mod osiris_catalog;
mod osiris_descriptions;
mod package;
mod packaged_stats;
mod parser;
mod schema;
mod thoth;
mod thoth_api;
mod thoth_facts;
mod tooltip;
mod xml;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use annotation::{
    FunctionParameterType, ParsedTypeExpression, PrimitiveType, ThothAliasAnnotation,
    ThothAnnotations, ThothClassAnnotation, ThothFieldAnnotation, ThothFunctionAnnotation,
    ThothFunctionContract, ThothParameterAnnotation, ThothReturnAnnotation,
    ThothVariableAnnotation, TypeExpression, TypeParseError, parse_type_expression,
};
pub use cache::{CacheStats, CacheStore};
pub use catalog::{
    ContextMemberSpec, ContextPropertySpec, FUNCTIONS, FunctionForm, FunctionSpec,
    FunctorPrefixSpec, OsirisSignature, ParameterSpec, context_member, context_members,
    context_properties, context_property, context_side, enum_value, field_documentation,
    field_kind, function_spec, functor_prefix, functor_prefixes, is_lsx_value_field,
    member_enumeration, osiris_legacy_signatures, osiris_signature,
};
pub use discovery::{
    discover_module, module_watch_roots, path_is_within, resolve_path, source_kind_for_document,
};
pub use domain::{
    Definition, LineMap, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND, OSIRIS_PROCEDURE_KIND,
    OSIRIS_QUERY_KIND, ObservedFunction, OsirisArgument, OsirisCallRole, OsirisDatabaseBinding,
    OsirisDatabaseOccurrence, OsirisEvidenceOrigin, OsirisFile, OsirisTypeEvidence,
    OsirisVariableFact, OsirisVariableOccurrence, ParsedFile, Position, Reference, SourceFile,
    SourceIssue, SourceKind, SymbolTarget, THOTH_FUNCTION_KIND, TextRange, ThothAssignment,
    ThothBinaryOperator, ThothCall, ThothControlFlowFact, ThothDeclaration, ThothDeclarationOwner,
    ThothExpression, ThothExpressionFact, ThothExpressionKind, ThothFile, ThothIfBranch,
    ThothIfBranchKind, ThothLexicalScope, ThothLiteralKind, ThothMemberAccess,
    ThothMemberAccessKind, ThothMemberSegment, ThothParameter, ThothReturn, ThothScopeId,
    ThothStatementId, ThothUnaryOperator,
};
pub use game_version::{
    GameBuildVersion, GameBuildVersionError, GameBuildVersionSource, detect_game_build_version,
};
pub use localization::{
    LocalizationCatalog, LocalizedText, read_base_localization_package, read_localization_package,
    valid_language,
};
pub use module::{DefinitionRecord, ModuleIndex, ReferenceRecord};
pub use osiris_api::{
    OSIRIS_FACTS_EXTRACTOR_VERSION, PackagedOsirisCallable, PackagedOsirisCallableRole,
    PackagedOsirisCandidate, PackagedOsirisIndex, PackagedOsirisResolution,
    parse_osiris_goal_source,
};
pub use osiris_catalog::{
    GENERATED_OSIRIS_CATALOG_GENERATOR_VERSION, OSIRIS_CATALOG_GENERATOR_VERSION,
    OSIRIS_CATALOG_SOURCE_HASH, OSIRIS_CATALOG_SOURCE_VERSION, OSIRIS_CONTRACTS,
    OSIRIS_GUID_ALIASES, OSIRIS_GUIDSTRING_TYPE, OsirisCatalog, OsirisCatalogMetadata,
    OsirisContract, OsirisContractKind, OsirisContractSpec, OsirisParameter,
    OsirisParameterDirection, OsirisParameterSpec, generate_osiris_catalog, osiris_contract,
    osiris_contract_by_kind, osiris_event_contract, osiris_type_compatibility, parse_story_header,
    render_osiris_catalog,
};
pub use osiris_descriptions::{
    OSIRIS_DESCRIPTION_CATALOG_VERSION, OSIRIS_DESCRIPTION_RECORDS, OsirisDescriptionRecord,
    osiris_callable_description, osiris_callable_description_for_kind,
    validate_osiris_descriptions,
};
pub use package::{PackageEntry, PackageHeader, PackageReader};
pub use packaged_stats::{
    PackagedStatsCatalog, PackagedStatsDefinition, PackagedStatsResolution, PackagedStatsSource,
    read_packaged_stats_catalog, read_packaged_stats_catalog_from_packages,
    stats_module_from_entry,
};
pub use parser::{
    canonical_kind, is_structural_stats_value, is_uuid, parse_source, parse_thoth_file,
    schema_for_toolkit, schemas_for_plain,
};
pub use schema::{SchemaCatalog, SchemaDefinition, SchemaField, is_schema_discriminator};
pub use thoth::{
    PackagedThothCatalog, PackagedThothInventory, PackagedThothModuleInventory,
    PackagedThothPackageRejection, PackagedThothResolution, PackagedThothSource,
    PackagedThothSourceRejection, inventory_packaged_thoth, osiris_module_from_entry,
    packaged_thoth_package_candidates, read_packaged_osiris_catalog, read_packaged_thoth_catalog,
    thoth_module_from_entry,
};
pub use thoth_api::{
    PackagedThothApiCandidate, PackagedThothApiIndex, PackagedThothApiResolution,
    PackagedThothApiSymbol, PackagedThothApiSymbolKind,
};
pub use thoth_facts::{
    CachedThothFacts, PackagedThothFact, PackagedThothFacts, THOTH_FACTS_EXTRACTOR_VERSION,
    parse_packaged_thoth_facts,
};
pub use tooltip::{
    TooltipCatalog, TooltipText, base_tooltip_package_path, parse_tooltip_catalog,
    read_base_tooltip_catalog,
};

/// The role of a module in the configured BG3 load order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ModuleRole {
    Base,
    Dependency,
    Project,
}

/// A validated module root without a baked-in precedence rank.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ModuleSpec {
    pub name: String,
    pub root: PathBuf,
    pub role: ModuleRole,
}

/// Failures produced while BG3 data is validated, parsed, or cached.
#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("schema error: {0}")]
    Schema(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("package error: {0}")]
    Package(String),
    #[error("localization error: {0}")]
    Localization(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Xml(#[from] quick_xml::Error),
    #[error(transparent)]
    XmlAttribute(#[from] quick_xml::events::attributes::AttrError),
    #[error(transparent)]
    XmlEncoding(#[from] quick_xml::encoding::EncodingError),
    #[error(transparent)]
    Postcard(#[from] postcard::Error),
    #[error(transparent)]
    TreeSitterLanguage(#[from] tree_sitter::LanguageError),
    #[error(transparent)]
    TreeSitterUtf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    WalkDir(#[from] walkdir::Error),
    #[error(transparent)]
    RayonPool(#[from] rayon::ThreadPoolBuildError),
}
