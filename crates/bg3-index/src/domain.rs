use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A zero-based UTF-8 source position.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A half-open source range.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
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
    /// A database fact removal (`NOT DB_...`) in INIT/EXIT or THEN.
    Remove,
}

/// The provenance of one Osiris argument type observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OsirisEvidenceOrigin {
    /// An explicit source cast, a typed rule variable, or a literal.
    Explicit,
    /// Derived from a curated engine event signature at a rule head.
    Engine,
}

/// One exact source-backed type observation for an Osiris argument.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisTypeEvidence {
    pub type_name: String,
    pub source_range: TextRange,
    pub origin: OsirisEvidenceOrigin,
}

/// One argument supplied to an Osiris database call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisArgument {
    pub range: TextRange,
    pub evidence: Option<OsirisTypeEvidence>,
}

/// One read, write, or removal of an implicit Osiris user database.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisDatabaseOccurrence {
    pub name: String,
    pub arity: u16,
    pub range: TextRange,
    pub selection_range: TextRange,
    pub role: OsirisCallRole,
    pub arguments: Vec<OsirisArgument>,
}

/// The database column that introduced an Osiris variable in a positive
/// database condition.
///
/// The column is zero-based to match the parser's argument indexing. This is
/// source metadata only; the effective column type is resolved from visible
/// database writes by the IDE.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisDatabaseBinding {
    pub name: String,
    pub arity: u16,
    pub column: u16,
}

/// One source occurrence of a rule-local Osiris variable.
///
/// Unlike the grouped [`OsirisVariableFact`], this record preserves the
/// binding state at the occurrence. A variable can be used before a later DB
/// or query output binds it, so one rule-wide binding range is not sufficient
/// for navigation or hover.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisVariableOccurrence {
    pub range: TextRange,
    /// The producer visible at this occurrence, including the occurrence that
    /// established the value. `None` means no producer is proven yet.
    #[serde(default)]
    pub binding_range: Option<TextRange>,
    /// The DB column that supplied the value, when the visible producer is a
    /// positive DB condition.
    #[serde(default)]
    pub database_binding: Option<OsirisDatabaseBinding>,
    /// Type evidence available at this source position. Evidence from later
    /// casts is intentionally not copied backwards into earlier occurrences.
    #[serde(default)]
    pub evidence: Option<OsirisTypeEvidence>,
}

/// One rule-local Osiris variable grouped by name.
///
/// The rule range keeps equal variable names in different rules separate.
/// `binding_range` is populated only when syntax and the known Osiris rule
/// shape prove that one occurrence introduces the value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisVariableFact {
    pub rule_range: TextRange,
    pub name: String,
    pub occurrences: Vec<TextRange>,
    pub binding_range: Option<TextRange>,
    /// The positive DB condition that introduced this variable, when known.
    #[serde(default)]
    pub database_binding: Option<OsirisDatabaseBinding>,
    pub evidence: Option<OsirisTypeEvidence>,
    /// Source-ordered occurrence facts. Empty for caches written before the
    /// occurrence-level model was introduced; callers must fall back to the
    /// legacy grouped fields in that case.
    #[serde(default)]
    pub occurrence_facts: Vec<OsirisVariableOccurrence>,
}

/// Osiris-specific facts retained with one cacheable goal record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OsirisFile {
    pub goal: String,
    pub occurrences: Vec<OsirisDatabaseOccurrence>,
    #[serde(default)]
    pub variables: Vec<OsirisVariableFact>,
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
    #[serde(default)]
    pub expression_facts: Vec<ThothExpressionFact>,
    #[serde(default)]
    pub scopes: Vec<ThothLexicalScope>,
    #[serde(default)]
    pub control_flow: Vec<ThothControlFlowFact>,
    /// Exact expressions used as `if`, `elseif`, or `while` conditions.
    ///
    /// The parser records ranges separately from branch facts so consumers
    /// can validate loop conditions without treating loop bodies as `if`
    /// branches for flow narrowing.
    #[serde(default)]
    pub condition_ranges: Vec<TextRange>,
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
    /// The executable statement that owns this return, when parsed by the
    /// current extractor. The option keeps older cached records readable.
    #[serde(default)]
    pub statement: Option<ThothStatementId>,
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

/// The literal classes that can be established directly from Thoth syntax.
///
/// This records syntax evidence only. It does not retain a parsed value or
/// infer a wider semantic type.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ThothLiteralKind {
    Nil,
    Boolean,
    Number,
    String,
}

/// The syntax class of one Thoth expression fact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ThothExpressionKind {
    /// A literal whose class is known from its syntax.
    Literal(ThothLiteralKind),
    /// A single identifier expression.
    Identifier,
    /// A function-call expression.
    FunctionCall,
    /// A parenthesized expression and the exact range of its inner value.
    Parenthesized { expression: TextRange },
    /// A unary expression and the exact range of its operand.
    Unary {
        operator: ThothUnaryOperator,
        operand: TextRange,
    },
    /// A binary expression and the exact ranges of both operands.
    Binary {
        operator: ThothBinaryOperator,
        left: TextRange,
        right: TextRange,
    },
    /// A member chain, including the root, with one range per segment.
    MemberAccess(Vec<ThothMemberSegment>),
    /// An expression form that the fact extractor does not classify.
    Unknown,
}

/// The complete set of unary operators accepted by the Thoth grammar.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ThothUnaryOperator {
    Not,
    Length,
    Negate,
    BitNot,
}

/// The complete set of binary operators accepted by the Thoth grammar.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ThothBinaryOperator {
    Or,
    And,
    Less,
    LessOrEqual,
    Equal,
    NotEqual,
    GreaterOrEqual,
    Greater,
    BitOr,
    BitXor,
    BitAnd,
    ShiftLeft,
    ShiftRight,
    Concatenate,
    Add,
    Subtract,
    Multiply,
    Divide,
    FloorDivide,
    Modulo,
    Power,
}

/// The syntax form that produced one member-access segment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ThothMemberAccessKind {
    /// The root expression of a member-access chain.
    Root,
    /// A named field selected with dot syntax.
    Dot,
    /// A named method selected with colon syntax.
    Method,
    /// A key selected with bracket syntax.
    Bracket,
}

/// One source span in a member-access chain.
///
/// Segments include the root expression. For example, `GetObject().Member`
/// contains `GetObject()` and `Member`, each with its own exact source range.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothMemberSegment {
    pub text: String,
    pub range: TextRange,
    pub access: ThothMemberAccessKind,
}

/// A stable identity for a lexical Thoth scope.
///
/// The identity is source-backed and does not contain a filesystem path. A
/// parser assigns `Function` to a function declaration and `Block` to a
/// nested lexical block. `File` identifies top-level statements.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ThothScopeId {
    File,
    Function { range: TextRange },
    Block { range: TextRange },
}

/// A stable identity for a statement within one lexical scope.
///
/// `order` is the zero-based source order within `scope`; it is intentionally
/// not a global counter so independently cached facts can be compared and
/// merged without a source path.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ThothStatementId {
    pub scope: ThothScopeId,
    pub order: u32,
}

/// The role of one branch in a Thoth `if` statement.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ThothIfBranchKind {
    Consequence,
    ElseIf,
    Else,
}

/// One branch body and its optional condition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothIfBranch {
    pub kind: ThothIfBranchKind,
    pub condition: Option<TextRange>,
    pub scope: Option<ThothScopeId>,
}

/// A control-flow fact currently describing one `if` statement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothControlFlowFact {
    pub statement: ThothStatementId,
    pub branches: Vec<ThothIfBranch>,
}

/// One lexical scope and its enclosing scope, if any.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothLexicalScope {
    pub id: ThothScopeId,
    pub parent: Option<ThothScopeId>,
}

/// A cacheable, syntax-classified Thoth expression.
///
/// This model is intentionally separate from [`ThothExpression`], whose
/// source text is retained for existing callers. It adds only the stable
/// scope and statement identity required by later flow analysis; it does not
/// resolve names or infer expression types.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothExpressionFact {
    pub range: TextRange,
    pub text: String,
    pub kind: ThothExpressionKind,
    pub statement: ThothStatementId,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> TextRange {
        TextRange {
            start: Position {
                line: 0,
                character: start,
            },
            end: Position {
                line: 0,
                character: end,
            },
        }
    }

    #[test]
    fn expression_fact_preserves_syntax_class_and_member_ranges() {
        let statement = ThothStatementId {
            scope: ThothScopeId::Function {
                range: range(0, 20),
            },
            order: 3,
        };
        let fact = ThothExpressionFact {
            range: range(4, 20),
            text: "Namespace.Member".into(),
            kind: ThothExpressionKind::MemberAccess(vec![
                ThothMemberSegment {
                    text: "Namespace".into(),
                    range: range(4, 13),
                    access: ThothMemberAccessKind::Root,
                },
                ThothMemberSegment {
                    text: "Member".into(),
                    range: range(14, 20),
                    access: ThothMemberAccessKind::Dot,
                },
            ]),
            statement,
        };

        assert_eq!(fact.statement.scope, statement.scope);
        assert_eq!(fact.statement.order, 3);
        assert_eq!(
            fact.kind,
            ThothExpressionKind::MemberAccess(vec![
                ThothMemberSegment {
                    text: "Namespace".into(),
                    range: range(4, 13),
                    access: ThothMemberAccessKind::Root,
                },
                ThothMemberSegment {
                    text: "Member".into(),
                    range: range(14, 20),
                    access: ThothMemberAccessKind::Dot,
                },
            ])
        );
    }

    #[test]
    fn complete_thoth_file_with_expression_facts_is_postcard_cacheable() {
        let fact = ThothExpressionFact {
            range: range(0, 4),
            text: "true".into(),
            kind: ThothExpressionKind::Literal(ThothLiteralKind::Boolean),
            statement: ThothStatementId {
                scope: ThothScopeId::File,
                order: 0,
            },
        };
        let file = ThothFile {
            expression_facts: vec![fact],
            scopes: vec![ThothLexicalScope {
                id: ThothScopeId::File,
                parent: None,
            }],
            control_flow: vec![ThothControlFlowFact {
                statement: ThothStatementId {
                    scope: ThothScopeId::File,
                    order: 1,
                },
                branches: vec![ThothIfBranch {
                    kind: ThothIfBranchKind::Consequence,
                    condition: None,
                    scope: Some(ThothScopeId::Block {
                        range: range(5, 10),
                    }),
                }],
            }],
            condition_ranges: Vec::new(),
            ..ThothFile::default()
        };

        let encoded = postcard::to_stdvec(&file).expect("encode Thoth file");
        let decoded: ThothFile = postcard::from_bytes(&encoded).expect("decode Thoth file");
        assert_eq!(decoded, file);
    }
}
