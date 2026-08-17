//! BG3 source extraction, schema metadata, module indexes, and persistent caches.

mod annotation;
mod cache;
mod catalog;
mod discovery;
mod domain;
mod localization;
mod module;
mod package;
mod parser;
mod schema;
mod thoth;
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
    FUNCTIONS, FunctionForm, FunctionSpec, ParameterSpec, field_kind, function_spec,
    is_lsx_value_field,
};
pub use discovery::{
    discover_module, module_watch_roots, path_is_within, resolve_path, source_kind_for_document,
};
pub use domain::{
    Definition, LineMap, OSIRIS_DATABASE_KIND, OSIRIS_GOAL_KIND, OSIRIS_PROCEDURE_KIND,
    OSIRIS_QUERY_KIND, ObservedFunction, OsirisArgument, OsirisCallRole, OsirisDatabaseOccurrence,
    OsirisFile, OsirisTypeEvidence, ParsedFile, Position, Reference, SourceFile, SourceIssue,
    SourceKind, SymbolTarget, THOTH_FUNCTION_KIND, TextRange, ThothAssignment, ThothCall,
    ThothDeclaration, ThothDeclarationOwner, ThothExpression, ThothExpressionFact,
    ThothExpressionKind, ThothFile, ThothLexicalScope, ThothLiteralKind, ThothMemberAccess,
    ThothMemberAccessKind, ThothMemberSegment, ThothParameter, ThothReturn, ThothScopeId,
    ThothStatementId,
};
pub use localization::{
    LocalizationCatalog, LocalizedText, read_base_localization_package, read_localization_package,
};
pub use module::{DefinitionRecord, ModuleIndex, ReferenceRecord};
pub use package::{PackageEntry, PackageHeader, PackageReader};
pub use parser::{
    canonical_kind, is_uuid, parse_source, parse_thoth_file, schema_for_toolkit, schemas_for_plain,
};
pub use schema::{SchemaCatalog, SchemaDefinition, SchemaField, is_schema_discriminator};
pub use thoth::{
    PackagedThothCatalog, PackagedThothResolution, PackagedThothSource,
    packaged_thoth_package_candidates, read_packaged_thoth_catalog, thoth_module_from_entry,
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
