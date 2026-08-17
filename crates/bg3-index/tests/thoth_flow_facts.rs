use bg3_index::{
    ThothBinaryOperator, ThothControlFlowFact, ThothExpressionFact, ThothExpressionKind,
    ThothIfBranchKind, ThothScopeId, parse_thoth_file,
};

fn fact<'a>(facts: &'a bg3_index::ThothFile, text: &str) -> &'a ThothExpressionFact {
    facts
        .expression_facts
        .iter()
        .find(|fact| fact.text == text)
        .unwrap_or_else(|| panic!("missing expression fact {text:?}"))
}

fn condition_fact<'a>(
    facts: &'a bg3_index::ThothFile,
    control: &ThothControlFlowFact,
    branch: usize,
) -> Option<&'a ThothExpressionFact> {
    let range = control.branches.get(branch)?.condition?;
    facts
        .expression_facts
        .iter()
        .find(|fact| fact.range == range)
}

fn branch_scope_parent(
    facts: &bg3_index::ThothFile,
    control: &ThothControlFlowFact,
    branch: usize,
) -> Option<ThothScopeId> {
    let scope = control.branches.get(branch)?.scope?;
    facts
        .scopes
        .iter()
        .find(|candidate| candidate.id == scope)
        .and_then(|scope| scope.parent)
}

#[test]
fn preserves_structured_operators_and_reversed_nil_operands() {
    let text = "function Operators(value, other)\n  local equal = value == nil\n  local reversed = nil ~= value\n  local compound = value ~= nil and other\n  local unary = not value\n  return equal, reversed, compound, unary\nend\n";
    let facts = parse_thoth_file(text).expect("synthetic Thoth source");

    let equal = fact(&facts, "value == nil");
    let ThothExpressionKind::Binary {
        operator: ThothBinaryOperator::Equal,
        left,
        right,
    } = &equal.kind
    else {
        panic!("direct equality must retain its operands")
    };
    assert_eq!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.range == *left)
            .map(|fact| fact.text.as_str()),
        Some("value")
    );
    assert_eq!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.range == *right)
            .map(|fact| fact.text.as_str()),
        Some("nil")
    );

    let reversed = fact(&facts, "nil ~= value");
    let ThothExpressionKind::Binary {
        operator: ThothBinaryOperator::NotEqual,
        left,
        right,
    } = &reversed.kind
    else {
        panic!("reversed nil comparison must retain its operands")
    };
    assert_eq!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.range == *left)
            .map(|fact| fact.text.as_str()),
        Some("nil")
    );
    assert_eq!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.range == *right)
            .map(|fact| fact.text.as_str()),
        Some("value")
    );

    let compound = fact(&facts, "value ~= nil and other");
    assert!(matches!(
        compound.kind,
        ThothExpressionKind::Binary {
            operator: ThothBinaryOperator::And,
            ..
        }
    ));
    assert!(matches!(
        fact(&facts, "not value").kind,
        ThothExpressionKind::Unary { .. }
    ));
}

#[test]
fn links_nested_elseif_and_else_branches_to_their_scopes() {
    let text = "function Branches(value, other)\n  if value == nil then\n    if other ~= nil then\n      return value\n    end\n  elseif nil ~= value then\n    return value\n  else\n    return nil\n  end\nend\n";
    let facts = parse_thoth_file(text).expect("synthetic Thoth source");

    let outer = facts
        .control_flow
        .iter()
        .find(|control| control.branches.len() == 3)
        .expect("outer if control-flow fact");
    assert_eq!(outer.branches[0].kind, ThothIfBranchKind::Consequence);
    assert_eq!(outer.branches[1].kind, ThothIfBranchKind::ElseIf);
    assert_eq!(outer.branches[2].kind, ThothIfBranchKind::Else);
    assert!(matches!(
        condition_fact(&facts, outer, 0).map(|fact| &fact.kind),
        Some(ThothExpressionKind::Binary {
            operator: ThothBinaryOperator::Equal,
            ..
        })
    ));
    assert!(matches!(
        condition_fact(&facts, outer, 1).map(|fact| &fact.kind),
        Some(ThothExpressionKind::Binary {
            operator: ThothBinaryOperator::NotEqual,
            ..
        })
    ));
    assert!(condition_fact(&facts, outer, 2).is_none());
    for (index, branch) in outer.branches.iter().enumerate() {
        assert!(branch.scope.is_some(), "non-empty branch needs a scope");
        assert_eq!(
            branch_scope_parent(&facts, outer, index),
            Some(outer.statement.scope)
        );
    }

    let nested = facts
        .control_flow
        .iter()
        .find(|control| control.branches.len() == 1)
        .expect("nested if control-flow fact");
    assert_eq!(nested.branches[0].kind, ThothIfBranchKind::Consequence);
    assert!(matches!(
        condition_fact(&facts, nested, 0).map(|fact| &fact.kind),
        Some(ThothExpressionKind::Binary {
            operator: ThothBinaryOperator::NotEqual,
            ..
        })
    ));
    assert_eq!(
        branch_scope_parent(&facts, nested, 0),
        Some(nested.statement.scope)
    );
    assert_ne!(nested.statement.scope, outer.statement.scope);
}

#[test]
fn records_bare_and_valued_returns_with_statement_identity() {
    let text = "function Bare(value)\n  if value == nil then\n    return\n  end\nend\nfunction Valued(value)\n  return value\nend\n";
    let facts = parse_thoth_file(text).expect("synthetic Thoth source");

    assert_eq!(facts.returns.len(), 2);
    let bare = facts
        .returns
        .iter()
        .find(|return_fact| return_fact.expressions.is_empty())
        .expect("bare return");
    let valued = facts
        .returns
        .iter()
        .find(|return_fact| !return_fact.expressions.is_empty())
        .expect("valued return");
    assert!(
        bare.statement.is_some(),
        "bare return needs statement identity"
    );
    assert!(
        valued.statement.is_some(),
        "valued return needs statement identity"
    );
    assert_ne!(bare.statement, valued.statement);
    assert_eq!(valued.expressions[0].text, "value");
    assert_eq!(
        facts
            .expression_facts
            .iter()
            .find(|fact| fact.range == valued.expressions[0].range)
            .map(|fact| fact.text.as_str()),
        Some("value")
    );
}

#[test]
fn parse_is_deterministic_and_does_not_promote_truthiness_or_compound_conditions() {
    let text = "function Guards(value, other)\n  if value then\n    return value\n  end\n  if value ~= nil and other then\n    return value\n  end\nend\n";
    let first = parse_thoth_file(text).expect("synthetic Thoth source");
    let second = parse_thoth_file(text).expect("synthetic Thoth source");
    assert_eq!(first, second);

    let controls = first
        .control_flow
        .iter()
        .map(|control| {
            control
                .branches
                .iter()
                .find_map(|branch| branch.condition)
                .expect("condition range")
        })
        .collect::<Vec<_>>();
    assert_eq!(controls.len(), 2);
    let conditions = controls
        .iter()
        .map(|range| {
            first
                .expression_facts
                .iter()
                .find(|fact| fact.range == *range)
                .expect("condition fact")
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        conditions[0].kind,
        ThothExpressionKind::Identifier
    ));
    assert!(matches!(
        conditions[1].kind,
        ThothExpressionKind::Binary {
            operator: ThothBinaryOperator::And,
            ..
        }
    ));
    assert!(conditions.iter().all(|condition| {
        !matches!(
            condition.kind,
            ThothExpressionKind::Binary {
                operator: ThothBinaryOperator::Equal | ThothBinaryOperator::NotEqual,
                ..
            }
        )
    }));
}
