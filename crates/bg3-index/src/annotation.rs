//! Parsing and source-backed storage for LuaCATS-like Thoth annotations.
//!
//! This module deliberately parses only the type expression.  A later Thoth
//! comment parser can attach the result to a tag without losing the exact
//! offset at which a description starts.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::domain::TextRange;

/// A primitive type name understood by the annotation parser.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum PrimitiveType {
    Any,
    Boolean,
    Function,
    Integer,
    Number,
    String,
    Table,
    Thread,
    Userdata,
    Void,
}

impl PrimitiveType {
    fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "any" => Self::Any,
            "boolean" | "bool" => Self::Boolean,
            "function" => Self::Function,
            "integer" | "int" => Self::Integer,
            "number" | "float" => Self::Number,
            "string" => Self::String,
            "table" => Self::Table,
            "thread" => Self::Thread,
            "userdata" => Self::Userdata,
            "void" => Self::Void,
            _ => return None,
        })
    }
}

impl fmt::Display for PrimitiveType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Any => "any",
            Self::Boolean => "boolean",
            Self::Function => "function",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::String => "string",
            Self::Table => "table",
            Self::Thread => "thread",
            Self::Userdata => "userdata",
            Self::Void => "void",
        })
    }
}

/// A normalized explicit type expression.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum TypeExpression {
    /// No type evidence was supplied.
    #[default]
    Unknown,
    /// The Lua `nil` type.
    Nil,
    /// A built-in primitive type.
    Primitive(PrimitiveType),
    /// A user-defined, possibly dotted, type name.
    Name(String),
    /// A normalized union. Nested unions and duplicate members are removed.
    Union(Vec<TypeExpression>),
    /// An array whose element type is explicit.
    Array(Box<TypeExpression>),
    /// A function type with optional parameter names and one or more returns.
    Function {
        parameters: Vec<FunctionParameterType>,
        returns: Vec<TypeExpression>,
    },
}

impl TypeExpression {
    /// Creates a normalized union, flattening nested unions and removing
    /// duplicate members while preserving their first-seen order.
    pub fn union(types: impl IntoIterator<Item = Self>) -> Self {
        let mut members = Vec::new();
        for ty in types {
            match ty {
                Self::Union(nested) => {
                    for member in nested {
                        push_unique(&mut members, member);
                    }
                }
                member => push_unique(&mut members, member),
            }
        }
        match members.as_slice() {
            [] => Self::Unknown,
            [member] => member.clone(),
            _ => Self::Union(members),
        }
    }

    /// Returns the canonical form of this type.
    pub fn normalized(self) -> Self {
        match self {
            Self::Union(types) => Self::union(types.into_iter().map(Self::normalized)),
            Self::Array(element) => Self::Array(Box::new(element.normalized())),
            Self::Function {
                parameters,
                returns,
            } => Self::Function {
                parameters: parameters
                    .into_iter()
                    .map(FunctionParameterType::normalized)
                    .collect(),
                returns: returns.into_iter().map(Self::normalized).collect(),
            },
            other => other,
        }
    }

    /// Returns true when this type is the unknown type.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl fmt::Display for TypeExpression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => formatter.write_str("unknown"),
            Self::Nil => formatter.write_str("nil"),
            Self::Primitive(primitive) => primitive.fmt(formatter),
            Self::Name(name) => formatter.write_str(name),
            Self::Union(types) => {
                for (index, ty) in types.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str("|")?;
                    }
                    ty.fmt(formatter)?;
                }
                Ok(())
            }
            Self::Array(element) => {
                if matches!(element.as_ref(), Self::Union(_)) {
                    write!(formatter, "({})[]", element)
                } else {
                    write!(formatter, "{}[]", element)
                }
            }
            Self::Function {
                parameters,
                returns,
            } => {
                formatter.write_str("fun(")?;
                for (index, parameter) in parameters.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(", ")?;
                    }
                    parameter.fmt(formatter)?;
                }
                formatter.write_str("): ")?;
                for (index, ty) in returns.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(", ")?;
                    }
                    ty.fmt(formatter)?;
                }
                Ok(())
            }
        }
    }
}

fn push_unique(types: &mut Vec<TypeExpression>, candidate: TypeExpression) {
    if !types.contains(&candidate) {
        types.push(candidate);
    }
}

/// A parameter in a function type or annotation contract.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct FunctionParameterType {
    pub name: Option<String>,
    pub ty: TypeExpression,
    pub variadic: bool,
}

impl FunctionParameterType {
    fn normalized(self) -> Self {
        Self {
            name: self.name,
            ty: self.ty.normalized(),
            variadic: self.variadic,
        }
    }
}

impl fmt::Display for FunctionParameterType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.variadic, &self.name, &self.ty) {
            (true, Some(name), ty) => write!(formatter, "...{}: {}", name, ty),
            (true, None, TypeExpression::Unknown) => formatter.write_str("..."),
            (true, None, ty) => write!(formatter, "...: {}", ty),
            (false, Some(name), ty) => write!(formatter, "{}: {}", name, ty),
            (false, None, ty) => ty.fmt(formatter),
        }
    }
}

/// A successful prefix parse and the number of bytes consumed from the input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedTypeExpression {
    pub ty: TypeExpression,
    pub consumed: usize,
}

impl ParsedTypeExpression {
    /// Returns the parsed normalized type.
    pub fn ty(&self) -> &TypeExpression {
        &self.ty
    }

    /// Returns the byte offset immediately after the type expression.
    pub fn consumed(&self) -> usize {
        self.consumed
    }
}

/// A relative error while parsing a type expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeParseError {
    pub message: String,
    pub offset: usize,
}

impl fmt::Display for TypeParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for TypeParseError {}

/// Parses the first type expression in `input`.
///
/// Leading whitespace is consumed. Parsing stops before trailing whitespace
/// and description text, so a tag parser can continue at `consumed`.
pub fn parse_type_expression(input: &str) -> Result<ParsedTypeExpression, TypeParseError> {
    let mut parser = TypeParser::new(input);
    parser.skip_whitespace();
    let ty = parser.parse_union()?;
    Ok(ParsedTypeExpression {
        ty: ty.normalized(),
        consumed: parser.position,
    })
}

struct TypeParser<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> TypeParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.input[self.position..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.position += character.len_utf8();
        Some(character)
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, TypeParseError> {
        Err(TypeParseError {
            message: message.into(),
            offset: self.position,
        })
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn parse_union(&mut self) -> Result<TypeExpression, TypeParseError> {
        let mut types = vec![self.parse_postfix()?];
        loop {
            let separator_start = self.position;
            self.skip_whitespace();
            if !self.eat('|') {
                self.position = separator_start;
                break;
            }
            self.skip_whitespace();
            if self.peek().is_none() {
                return self.error("expected a type after `|`");
            }
            types.push(self.parse_postfix()?);
        }
        Ok(TypeExpression::union(types))
    }

    fn parse_postfix(&mut self) -> Result<TypeExpression, TypeParseError> {
        let mut ty = self.parse_atom()?;
        loop {
            if self.rest().starts_with("[]") {
                self.position += 2;
                ty = TypeExpression::Array(Box::new(ty));
            } else if self.eat('?') {
                ty = TypeExpression::union([ty, TypeExpression::Nil]);
            } else {
                break;
            }
        }
        Ok(ty)
    }

    fn parse_atom(&mut self) -> Result<TypeExpression, TypeParseError> {
        self.skip_whitespace();
        if self.eat('(') {
            let ty = self.parse_union()?;
            self.skip_whitespace();
            if !self.eat(')') {
                return self.error("expected `)`");
            }
            return Ok(ty);
        }
        if self.rest().starts_with("...") {
            return self.error("variadic marker is only valid in function parameters");
        }
        let name = self.parse_name()?;
        if name == "fun" && self.peek() == Some('(') {
            return self.parse_function();
        }
        if name.eq_ignore_ascii_case("array") && self.eat('<') {
            let element = self.parse_union()?;
            self.skip_whitespace();
            if !self.eat('>') {
                return self.error("expected `>`");
            }
            return Ok(TypeExpression::Array(Box::new(element)));
        }
        if name.eq_ignore_ascii_case("nil") {
            return Ok(TypeExpression::Nil);
        }
        if name.eq_ignore_ascii_case("unknown") {
            return Ok(TypeExpression::Unknown);
        }
        if let Some(primitive) = PrimitiveType::parse(&name) {
            return Ok(TypeExpression::Primitive(primitive));
        }
        Ok(TypeExpression::Name(name))
    }

    fn parse_function(&mut self) -> Result<TypeExpression, TypeParseError> {
        self.bump();
        let mut parameters = Vec::new();
        loop {
            self.skip_whitespace();
            if self.eat(')') {
                break;
            }
            let variadic = self.rest().starts_with("...");
            if variadic {
                self.position += 3;
            }
            self.skip_whitespace();
            let (name, ty) = if variadic && (self.peek() == Some(')') || self.peek() == Some(',')) {
                (None, TypeExpression::Unknown)
            } else if variadic && self.eat(':') {
                self.skip_whitespace();
                (None, self.parse_union()?)
            } else {
                let first = self.parse_name()?;
                let optional_name = self.eat('?');
                self.skip_whitespace();
                if self.eat(':') {
                    self.skip_whitespace();
                    let ty = self.parse_union()?;
                    (
                        Some(first),
                        if optional_name {
                            TypeExpression::union([ty, TypeExpression::Nil])
                        } else {
                            ty
                        },
                    )
                } else if optional_name {
                    (
                        None,
                        TypeExpression::union([name_to_type(first), TypeExpression::Nil]),
                    )
                } else {
                    (None, name_to_type(first))
                }
            };
            parameters.push(FunctionParameterType { name, ty, variadic });
            self.skip_whitespace();
            if self.eat(')') {
                break;
            }
            if !self.eat(',') {
                return self.error("expected `,` or `)` in function parameters");
            }
        }
        self.skip_whitespace();
        if !self.eat(':') {
            return self.error("expected `:` before function return type");
        }
        self.skip_whitespace();
        let mut returns = vec![self.parse_union()?];
        loop {
            let separator_start = self.position;
            self.skip_whitespace();
            if !self.eat(',') {
                self.position = separator_start;
                break;
            }
            self.skip_whitespace();
            returns.push(self.parse_union()?);
        }
        Ok(TypeExpression::Function {
            parameters,
            returns,
        })
    }

    fn parse_name(&mut self) -> Result<String, TypeParseError> {
        self.skip_whitespace();
        let start = self.position;
        loop {
            if !self.peek().is_some_and(is_name_start) {
                return self.error(if self.position == start {
                    "expected a type name"
                } else {
                    "expected a name segment after `.`"
                });
            }
            self.bump();
            while self.peek().is_some_and(is_name_continue) {
                self.bump();
            }
            if self.peek() == Some('-') {
                return self.error("hyphens are not valid in type names");
            }
            if !self.eat('.') {
                break;
            }
        }
        Ok(self.input[start..self.position].to_owned())
    }
}

fn is_name_start(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_'
}

fn is_name_continue(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

fn name_to_type(name: String) -> TypeExpression {
    if name.eq_ignore_ascii_case("nil") {
        TypeExpression::Nil
    } else if name.eq_ignore_ascii_case("unknown") {
        TypeExpression::Unknown
    } else if let Some(primitive) = PrimitiveType::parse(&name) {
        TypeExpression::Primitive(primitive)
    } else {
        TypeExpression::Name(name)
    }
}

/// Source-backed class field annotation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothFieldAnnotation {
    pub name: String,
    pub ty: TypeExpression,
    pub range: TextRange,
    pub name_range: TextRange,
    pub type_range: TextRange,
}

/// Source-backed class annotation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothClassAnnotation {
    pub name: String,
    pub range: TextRange,
    pub name_range: TextRange,
    pub fields: Vec<ThothFieldAnnotation>,
}

/// Source-backed type alias annotation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothAliasAnnotation {
    pub name: String,
    pub ty: TypeExpression,
    pub range: TextRange,
    pub name_range: TextRange,
    pub type_range: TextRange,
}

/// A parameter in a source-backed function contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothParameterAnnotation {
    pub name: String,
    pub ty: TypeExpression,
    pub range: TextRange,
    pub name_range: TextRange,
    pub type_range: TextRange,
    pub variadic: bool,
}

/// A return entry in a source-backed function contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothReturnAnnotation {
    pub ty: TypeExpression,
    pub range: TextRange,
    pub type_range: TextRange,
}

/// A complete function signature supplied by an annotation tag.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothFunctionContract {
    pub parameters: Vec<ThothParameterAnnotation>,
    pub returns: Vec<ThothReturnAnnotation>,
    pub range: TextRange,
}

/// Source-backed function annotation, including optional overload contracts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothFunctionAnnotation {
    pub name: Option<String>,
    pub range: TextRange,
    pub name_range: Option<TextRange>,
    pub contracts: Vec<ThothFunctionContract>,
}

/// A variable target and its explicit type evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothVariableAnnotation {
    pub target: String,
    pub ty: TypeExpression,
    pub range: TextRange,
    pub target_range: TextRange,
    pub type_range: TextRange,
}

/// All annotation records attached to one Thoth source file.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThothAnnotations {
    pub classes: Vec<ThothClassAnnotation>,
    pub aliases: Vec<ThothAliasAnnotation>,
    pub functions: Vec<ThothFunctionAnnotation>,
    pub variables: Vec<ThothVariableAnnotation>,
}
