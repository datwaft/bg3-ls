use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A zero-based UTF-8 source position.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A half-open source range.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: Position,
    pub end: Position,
}

/// A source format supported by the indexer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum SourceKind {
    PlainStats,
    ToolkitStats,
    Table,
    Lsx,
    Thoth,
    Osiris,
    Localization,
}

/// The semantic kind used for declarations and calls in Thoth helper files.
pub const THOTH_FUNCTION_KIND: &str = "ThothFunction";

/// The semantic kind used for one loose Osiris goal file.
pub const OSIRIS_GOAL_KIND: &str = "OsirisGoal";

/// The semantic kind used for an implicit Osiris user database.
pub const OSIRIS_DATABASE_KIND: &str = "OsirisDatabase";

/// The semantic kind used for an Osiris procedure declaration.
pub const OSIRIS_PROCEDURE_KIND: &str = "OsirisProcedure";

/// The semantic kind used for an Osiris query declaration.
pub const OSIRIS_QUERY_KIND: &str = "OsirisQuery";

/// Identifies one source file without assigning it a load-order rank.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: PathBuf,
    pub kind: SourceKind,
}

/// A symbol declaration extracted from one BG3 source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub kind: String,
    pub name: String,
    pub range: TextRange,
    pub selection_range: TextRange,
    pub fields: BTreeMap<String, String>,
    pub field_ranges: BTreeMap<String, TextRange>,
    pub aliases: Vec<String>,
    pub uuid: Option<Uuid>,
    pub parent: Option<String>,
    pub schema_id: Option<String>,
    pub arity: Option<u16>,
}

/// A semantic lookup target that remains valid across module compositions.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum SymbolTarget {
    Named { kind: Option<String>, name: String },
    Tooltip { name: String },
    Uuid(Uuid),
    OsirisGoal { name: String },
    OsirisCallable { name: String, arity: u16 },
    OsirisDatabase { name: String, arity: u16 },
}

/// A semantic symbol use extracted from a BG3 source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub target: SymbolTarget,
    pub range: TextRange,
    pub context: String,
}

/// The semantic role of one Osiris database occurrence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OsirisCallRole {
    Read,
    Write,
}

/// One exact source-backed type observation for an Osiris argument.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisTypeEvidence {
    pub type_name: String,
    pub source_range: TextRange,
}

/// One argument supplied to an Osiris database call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisArgument {
    pub range: TextRange,
    pub evidence: Option<OsirisTypeEvidence>,
}

/// One read or write of an implicit Osiris user database.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisDatabaseOccurrence {
    pub name: String,
    pub arity: u16,
    pub range: TextRange,
    pub selection_range: TextRange,
    pub role: OsirisCallRole,
    pub arguments: Vec<OsirisArgument>,
}

/// Osiris-specific facts retained with one cacheable goal record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisFile {
    pub goal: String,
    pub occurrences: Vec<OsirisDatabaseOccurrence>,
}

/// Cacheable semantic facts extracted from one Thoth source file.
///
/// Declarations are stored separately from observations. In particular, a
/// member access or call is not treated as a declaration of the referenced
/// name. Types are intentionally absent until syntax or an explicit contract
/// supplies them.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothFile {
    pub declarations: Vec<ThothDeclaration>,
    pub returns: Vec<ThothReturn>,
    pub calls: Vec<ThothCall>,
    pub assignments: Vec<ThothAssignment>,
    pub member_accesses: Vec<ThothMemberAccess>,
    pub annotations: crate::annotation::ThothAnnotations,
}

/// Identifies the containing Thoth function for an expression observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothDeclarationOwner {
    pub name: String,
    pub range: TextRange,
}

/// A function declaration observed in a Thoth source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothDeclaration {
    pub name: String,
    pub range: TextRange,
    pub name_range: TextRange,
    pub parameters: Vec<ThothParameter>,
}

/// One declared Thoth function parameter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothParameter {
    pub name: String,
    pub range: TextRange,
    pub variadic: bool,
}

/// One return statement and its exact expression spans.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothReturn {
    pub range: TextRange,
    pub expressions: Vec<ThothExpression>,
    pub owner: Option<ThothDeclarationOwner>,
}

/// One function call observation, including the exact observed arity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothCall {
    pub name: String,
    pub name_range: TextRange,
    pub range: TextRange,
    pub arguments: Vec<ThothExpression>,
    pub arity: u16,
    pub owner: Option<ThothDeclarationOwner>,
}

/// One assignment or local/global declaration observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothAssignment {
    pub range: TextRange,
    pub local: bool,
    pub global: bool,
    pub targets: Vec<ThothExpression>,
    pub values: Vec<ThothExpression>,
    pub owner: Option<ThothDeclarationOwner>,
}

/// One complete member-access chain such as `Namespace.Member.Value`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothMemberAccess {
    pub range: TextRange,
    pub text: String,
    pub root: String,
    pub members: Vec<String>,
    pub owner: Option<ThothDeclarationOwner>,
}

/// A source-backed expression observation. Its type remains unknown unless a
/// later analysis layer proves one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothExpression {
    pub range: TextRange,
    pub text: String,
}

/// Aggregate information about a function observed in indexed expressions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ObservedFunction {
    pub name: String,
    pub count: u32,
    pub min_arity: u16,
    pub max_arity: u16,
}

/// A recoverable problem found while one source file is parsed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceIssue {
    pub code: String,
    pub message: String,
    pub range: TextRange,
}

/// The complete, cacheable semantic record for one source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedFile {
    pub source: SourceFile,
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
    pub observed_functions: Vec<ObservedFunction>,
    pub issues: Vec<SourceIssue>,
    pub osiris: Option<OsirisFile>,
    pub thoth: Option<ThothFile>,
}

/// Converts byte offsets to line and UTF-8 column positions.
#[derive(Debug)]
pub struct LineMap {
    starts: Vec<usize>,
}

impl LineMap {
    /// Builds a line map for one complete source document.
    pub fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(offset, byte)| (byte == b'\n').then_some(offset + 1)),
        );
        Self { starts }
    }

    /// Converts a clamped byte offset to a source position.
    pub fn position(&self, offset: usize) -> Position {
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            character: u32::try_from(offset - self.starts[line]).unwrap_or(u32::MAX),
        }
    }

    /// Converts a half-open byte span to a source range.
    pub fn range(&self, start: usize, end: usize) -> TextRange {
        TextRange {
            start: self.position(start),
            end: self.position(end),
        }
    }
}
