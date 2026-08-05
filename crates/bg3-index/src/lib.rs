//! BG3 source extraction, schema metadata, module indexes, and persistent caches.

mod cache;
mod discovery;
mod domain;
mod module;
mod parser;
mod schema;
mod xml;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use cache::{CacheStats, CacheStore};
pub use discovery::{discover_module, path_is_within, resolve_path};
pub use domain::{
    Definition, LineMap, ObservedFunction, ParsedFile, Position, Reference, SourceFile,
    SourceIssue, SourceKind, SymbolTarget, TextRange,
};
pub use module::{DefinitionRecord, ModuleIndex, ReferenceRecord};
pub use parser::{canonical_kind, is_uuid, parse_source, schema_for_toolkit, schemas_for_plain};
pub use schema::{SchemaCatalog, SchemaDefinition, SchemaField};

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
