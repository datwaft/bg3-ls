use std::path::Path;

use bg3_index::{
    Definition, SchemaDefinition, SchemaField, SourceKind, SymbolTarget, TextRange,
    ThothBinaryOperator, ThothExpressionKind, ThothUnaryOperator, TypeExpression,
    is_schema_discriminator, osiris_type_compatibility,
};
use uuid::Uuid;

use crate::{OverlaySet, WorkspaceSnapshot};

/// The editor-neutral severity of one verified source problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A stable diagnostic that the protocol adapter can publish unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub message: String,
}

impl WorkspaceSnapshot {
    /// Computes only diagnostics whose expected type is established by syntax or schema data.
    pub fn diagnostics(
        &self,
        path: &Path,
        overlays: &OverlaySet,
        unresolved_references: Option<DiagnosticSeverity>,
    ) -> Vec<Diagnostic> {
        let Some((_, file)) = self.file(path, overlays) else {
            return Vec::new();
        };
        // The Stats catalogs do not describe LSX nodes. Do not report legacy
        // schema errors until the server has a verified LSX schema source.
        if matches!(file.source.kind, SourceKind::Lsx | SourceKind::Localization) {
            return Vec::new();
        }
        let mut diagnostics: Vec<_> = file
            .issues
            .iter()
            .map(|issue| Diagnostic {
                range: issue.range,
                severity: DiagnosticSeverity::Error,
                code: issue.code.clone(),
                message: issue.message.clone(),
            })
            .collect();
        // Thoth diagnostics use only exact, curated type evidence. Unknown,
        // ambiguous, and union-valued expressions remain silent.
        if matches!(file.source.kind, SourceKind::Osiris | SourceKind::Thoth) {
            if file.source.kind == SourceKind::Thoth {
                add_thoth_condition_diagnostics(self, path, file, overlays, &mut diagnostics);
            }
            if file.source.kind == SourceKind::Osiris {
                add_osiris_database_diagnostics(self, path, overlays, &mut diagnostics);
            }
            diagnostics.sort_by_key(|diagnostic| {
                (
                    diagnostic.range.start.line,
                    diagnostic.range.start.character,
                    diagnostic.code.clone(),
                )
            });
            return diagnostics;
        }

        for definition in &file.definitions {
            let schemas = schemas_for_definition(self, path, definition);
            if schemas.is_empty() {
                diagnostics.push(Diagnostic {
                    range: definition.selection_range,
                    severity: DiagnosticSeverity::Error,
                    code: "unknown-entry-type".into(),
                    message: format!("No Stats schema matches `{}`.", definition.kind),
                });
                continue;
            }
            if let Some((field, message)) =
                self.schema
                    .discriminator_error(path, Some(&definition.kind), &definition.fields)
            {
                diagnostics.push(Diagnostic {
                    range: definition
                        .field_ranges
                        .get(field)
                        .copied()
                        .unwrap_or(definition.range),
                    severity: DiagnosticSeverity::Error,
                    code: "invalid-schema-discriminator".into(),
                    message,
                });
            }
            validate_fields(self, definition, &schemas, &mut diagnostics);
        }

        for reference in &file.references {
            if reference.context == "using" {
                let Some(severity) = unresolved_references else {
                    continue;
                };
                let complete = match &reference.target {
                    SymbolTarget::Named {
                        kind: Some(kind), ..
                    } => self.has_complete_kind(kind),
                    _ => false,
                };
                if complete && self.resolve(&reference.target, overlays).is_empty() {
                    diagnostics.push(Diagnostic {
                        range: reference.range,
                        severity,
                        code: "missing-parent".into(),
                        message: format!(
                            "Parent `{}` is not visible.",
                            target_name(&reference.target)
                        ),
                    });
                }
                continue;
            }
            let Some(severity) = unresolved_references else {
                continue;
            };
            let SymbolTarget::Named {
                kind: Some(kind),
                name,
            } = &reference.target
            else {
                // UUIDs can identify packed root templates that the loose index cannot see.
                continue;
            };
            if reference.context != "using"
                && is_diagnosable_kind(kind)
                && self.has_complete_kind(kind)
                && self.resolve(&reference.target, overlays).is_empty()
            {
                diagnostics.push(Diagnostic {
                    range: reference.range,
                    severity,
                    code: "unresolved-reference".into(),
                    message: format!("No visible {kind} declaration matches `{name}`."),
                });
            }
        }
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.range.start.line,
                diagnostic.range.start.character,
                diagnostic.code.clone(),
            )
        });
        diagnostics
    }
}

/// Diagnoses user database columns whose occurrences use incompatible proven
/// GUID aliases.
///
/// Establishment follows the global Story goal order, not module precedence.
/// Compile-time evidence may come from any proven database occurrence, while
/// unknown evidence stays silent and runtime role counts remain separate.
fn add_osiris_database_diagnostics(
    workspace: &WorkspaceSnapshot,
    path: &Path,
    overlays: &OverlaySet,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for schema in workspace.osiris_database_schemas(overlays).into_values() {
        for (index, column) in schema.columns.into_iter().enumerate() {
            // A column without an established alias has nothing to violate.
            if column.established.is_none() {
                continue;
            }
            if column.ambiguous {
                continue;
            }
            for (observation_index, observation) in column.observations.iter().enumerate() {
                if observation.path != path {
                    continue;
                }
                // Repeated values of the same proven type do not introduce a
                // new mismatch. This also prevents a later repeat of an
                // established alias from being reported only because an
                // incompatible alias appeared between the two occurrences.
                if column.observations[..observation_index]
                    .iter()
                    .any(|prior| prior.type_name == observation.type_name)
                {
                    continue;
                }
                // The compiler permits generic GUIDSTRING values to match a
                // verified specialized alias. Different specialized aliases
                // remain a proven mismatch; unknown relationships stay
                // silent under the no-false-diagnostics policy.
                let Some(established) =
                    column.observations[..observation_index]
                        .iter()
                        .find(|prior| {
                            osiris_type_compatibility(&prior.type_name, &observation.type_name)
                                == Some(false)
                        })
                else {
                    continue;
                };
                diagnostics.push(Diagnostic {
                    range: observation.range,
                    severity: DiagnosticSeverity::Error,
                    code: "osiris-database-alias-mismatch".into(),
                    message: format!(
                        "Column {} of `{}/{}` is established as `{}`. This argument supplies `{}`. Add an explicit `({})` cast.",
                        index + 1,
                        schema.name,
                        schema.arity,
                        established.type_name,
                        observation.type_name,
                        established.type_name,
                    ),
                });
            }
        }
    }
}

fn add_thoth_condition_diagnostics(
    workspace: &WorkspaceSnapshot,
    path: &Path,
    file: &bg3_index::ParsedFile,
    overlays: &OverlaySet,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(thoth) = file.thoth.as_ref() else {
        return;
    };
    for range in &thoth.condition_ranges {
        let Some(fact) = thoth
            .expression_facts
            .iter()
            .find(|fact| fact.range == *range)
        else {
            continue;
        };
        if !is_condition_result_logical_combination(thoth, fact.range)
            && is_exact_condition_result(workspace.thoth_expression_type(path, *range, overlays))
        {
            diagnostics.push(Diagnostic {
                range: *range,
                severity: DiagnosticSeverity::Warning,
                code: "thoth-condition-result-condition".into(),
                message: "Use `.Result` when a `ConditionResult` is used as a boolean condition."
                    .into(),
            });
        }
    }

    for fact in &thoth.expression_facts {
        match &fact.kind {
            ThothExpressionKind::Unary {
                operator: ThothUnaryOperator::Not,
                operand,
            } if is_exact_condition_result(
                workspace.thoth_expression_type(path, *operand, overlays),
            ) =>
            {
                diagnostics.push(Diagnostic {
                    range: fact.range,
                    severity: DiagnosticSeverity::Warning,
                    code: "thoth-condition-result-boolean-operator".into(),
                    message: "Use `.Result` when `not` tests a `ConditionResult`.".into(),
                });
            }
            ThothExpressionKind::Binary {
                operator,
                left,
                right,
            } if matches!(operator, ThothBinaryOperator::And | ThothBinaryOperator::Or)
                && is_exact_condition_result(
                    workspace.thoth_expression_type(path, *left, overlays),
                )
                && is_exact_condition_result(
                    workspace.thoth_expression_type(path, *right, overlays),
                ) =>
            {
                diagnostics.push(Diagnostic {
                    range: fact.range,
                    severity: DiagnosticSeverity::Warning,
                    code: "thoth-condition-result-overloaded-operator".into(),
                    message: if *operator == ThothBinaryOperator::And {
                        "Use `&` to combine two `ConditionResult` values."
                    } else {
                        "Use `|` to combine two `ConditionResult` values."
                    }
                    .into(),
                });
            }
            _ => {}
        }
    }
}

fn is_condition_result_logical_combination(thoth: &bg3_index::ThothFile, range: TextRange) -> bool {
    let Some(fact) = thoth
        .expression_facts
        .iter()
        .find(|fact| fact.range == range)
    else {
        return false;
    };
    match &fact.kind {
        ThothExpressionKind::Binary {
            operator: ThothBinaryOperator::And | ThothBinaryOperator::Or,
            ..
        } => true,
        ThothExpressionKind::Parenthesized { expression } => {
            is_condition_result_logical_combination(thoth, *expression)
        }
        _ => false,
    }
}

fn is_exact_condition_result(ty: Option<TypeExpression>) -> bool {
    matches!(
        ty,
        Some(TypeExpression::Name(name)) if name == "ConditionResult"
    )
}

/// Validates fields against the union of all viable legacy schemas.
fn validate_fields(
    workspace: &WorkspaceSnapshot,
    definition: &Definition,
    schemas: &[&SchemaDefinition],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (name, value) in &definition.fields {
        if is_schema_discriminator(&definition.kind, name) {
            continue;
        }
        let range = definition
            .field_ranges
            .get(name)
            .copied()
            .unwrap_or(definition.range);
        let fields: Vec<_> = schemas
            .iter()
            .filter_map(|schema| schema.field(name))
            .collect();
        if fields.is_empty() {
            diagnostics.push(Diagnostic {
                range,
                severity: DiagnosticSeverity::Error,
                code: "unknown-field".into(),
                message: format!("Field `{name}` is not valid for `{}`.", definition.kind),
            });
            continue;
        }
        if fields
            .iter()
            .all(|field| field_value_error(workspace, field, value).is_some())
            && let Some((code, message)) = fields
                .iter()
                .find_map(|field| field_value_error(workspace, field, value))
        {
            diagnostics.push(Diagnostic {
                range,
                severity: DiagnosticSeverity::Error,
                code: code.into(),
                message,
            });
        }
    }
}

/// Returns a typed-value failure only when one schema field establishes the contract.
fn field_value_error(
    workspace: &WorkspaceSnapshot,
    field: &SchemaField,
    value: &str,
) -> Option<(&'static str, String)> {
    // An empty legacy value removes inherited data, independent of the field type.
    if value.is_empty() {
        return None;
    }
    if let Some(enumeration) = field.enumeration_type_name.as_ref()
        && let Some(values) = workspace.schema.enumerations.get(enumeration)
    {
        let invalid_member = if field
            .field_type
            .as_deref()
            .is_some_and(|field_type| field_type.eq_ignore_ascii_case("EnumerationList"))
        {
            let delimiter = field
                .delimiter
                .as_deref()
                .filter(|delimiter| !delimiter.is_empty())
                .unwrap_or(";");
            value.split(delimiter).map(str::trim).find(|member| {
                !member.is_empty() && !values.iter().any(|candidate| candidate == member)
            })
        } else {
            (!values.iter().any(|candidate| candidate == value)).then_some(value)
        };
        if let Some(invalid_member) = invalid_member {
            return Some((
                "invalid-enum",
                format!("`{invalid_member}` is not a member of enumeration `{enumeration}`."),
            ));
        }
    }
    let field_type = field.field_type.as_deref()?.to_ascii_lowercase();
    match field_type.as_str() {
        "boolean" | "bool"
            if !matches!(
                value.to_ascii_lowercase().as_str(),
                "true" | "false" | "yes" | "no" | "0" | "1"
            ) =>
        {
            Some((
                "invalid-boolean",
                format!("`{value}` is not a valid boolean."),
            ))
        }
        "integer" | "int" => validate_number::<i64>(field, value),
        "float" | "real" | "double" => validate_number::<f64>(field, value),
        "id" | "uuid" if Uuid::parse_str(value).is_err() => {
            Some(("invalid-uuid", format!("`{value}` is not a valid UUID.")))
        }
        "translatedstring" if !valid_translated_string(value) => Some((
            "invalid-translated-string",
            format!("`{value}` is not a valid translated-string handle."),
        )),
        _ => None,
    }
}

/// Parses a number and enforces optional inclusive schema bounds.
fn validate_number<T>(field: &SchemaField, value: &str) -> Option<(&'static str, String)>
where
    T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy,
{
    let Ok(number) = value.parse::<T>() else {
        return Some((
            "invalid-number",
            format!("`{value}` is not a valid number."),
        ));
    };
    if let Some(minimum) = field
        .min_value
        .as_ref()
        .and_then(|value| value.parse::<T>().ok())
        && number < minimum
    {
        return Some((
            "number-out-of-range",
            format!("`{value}` is less than the minimum value {minimum}."),
        ));
    }
    if let Some(maximum) = field
        .max_value
        .as_ref()
        .and_then(|value| value.parse::<T>().ok())
        && number > maximum
    {
        return Some((
            "number-out-of-range",
            format!("`{value}` is greater than the maximum value {maximum}."),
        ));
    }
    None
}

/// Accepts a BG3 localization handle with an optional numeric version suffix.
fn valid_translated_string(value: &str) -> bool {
    let (handle, version) = value
        .split_once(';')
        .map_or((value, None), |(handle, version)| (handle, Some(version)));
    let Some(body) = handle.strip_prefix('h') else {
        return false;
    };
    let groups = body.split('g').collect::<Vec<_>>();
    let valid_handle = body.len() == 36
        && (body.bytes().all(|byte| byte.is_ascii_hexdigit())
            || (groups.len() == 5
                && groups.iter().zip([8, 4, 4, 4, 12]).all(|(group, length)| {
                    group.len() == length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
                })));
    valid_handle
        && version.is_none_or(|version| {
            !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Selects exact Toolkit schemas or all viable legacy schema candidates.
fn schemas_for_definition<'a>(
    workspace: &'a WorkspaceSnapshot,
    path: &Path,
    definition: &Definition,
) -> Vec<&'a SchemaDefinition> {
    if let Some(schema_id) = &definition.schema_id {
        workspace.schema.by_id.get(schema_id).into_iter().collect()
    } else {
        workspace
            .schema
            .infer_legacy(path, Some(&definition.kind), &definition.fields)
    }
}

/// Limits unresolved-reference reports to contexts with complete loose-data coverage.
fn is_diagnosable_kind(kind: &str) -> bool {
    matches!(
        kind,
        "SpellData"
            | "StatusData"
            | "PassiveData"
            | "InterruptData"
            | "ActionResource"
            | "Localization"
    )
}

/// Formats one semantic target for a diagnostic message.
fn target_name(target: &SymbolTarget) -> String {
    match target {
        SymbolTarget::Named { name, .. } => name.clone(),
        SymbolTarget::Tooltip { name } => name.clone(),
        SymbolTarget::Uuid(uuid) => uuid.to_string(),
        SymbolTarget::OsirisGoal { name }
        | SymbolTarget::OsirisCallable { name, .. }
        | SymbolTarget::OsirisDatabase { name, .. } => name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::valid_translated_string;

    #[test]
    fn accepts_bg3_localization_handle_forms() {
        assert!(valid_translated_string(
            "h0150eda0gf427g466agaaaage5523351d9ac"
        ));
        assert!(valid_translated_string(
            "h0150EDA0gF427g466AgAAAAgE5523351D9AC;12"
        ));
        assert!(valid_translated_string(
            "h000000000000000000000000000000000001;2"
        ));
    }

    #[test]
    fn rejects_malformed_bg3_localization_handles() {
        assert!(!valid_translated_string(
            "h0150eda0-f427-466a-aaaa-e5523351d9ac"
        ));
        assert!(!valid_translated_string(
            "h0150eda0gf427g466agaaaage5523351d9a;1"
        ));
        assert!(!valid_translated_string(
            "h0150eda0gf427g466agaaaage5523351d9ac;v1"
        ));
    }
}
