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
    Localization,
}

/// The semantic kind used for declarations and calls in Thoth helper files.
pub const THOTH_FUNCTION_KIND: &str = "ThothFunction";

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
}

/// A semantic lookup target that remains valid across module compositions.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum SymbolTarget {
    Named { kind: Option<String>, name: String },
    Uuid(Uuid),
}

/// A semantic symbol use extracted from a BG3 source file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub target: SymbolTarget,
    pub range: TextRange,
    pub context: String,
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
