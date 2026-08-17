use bg3_index::{FunctionParameterType, PrimitiveType, TypeExpression, parse_type_expression};

#[test]
fn normalizes_optional_and_duplicate_union_members() {
    let parsed = parse_type_expression("Entity? | Entity | nil description").expect("type");

    assert_eq!(
        parsed.ty,
        TypeExpression::union([TypeExpression::Name("Entity".into()), TypeExpression::Nil,])
    );
    assert_eq!(parsed.ty.to_string(), "Entity|nil");
    assert_eq!(parsed.consumed, "Entity? | Entity | nil".len());
}

#[test]
fn parses_primitives_dotted_names_and_arrays() {
    let parsed = parse_type_expression("array<Mod.Entity>[]").expect("type");

    assert_eq!(
        parsed.ty,
        TypeExpression::Array(Box::new(TypeExpression::Array(Box::new(
            TypeExpression::Name("Mod.Entity".into()),
        ))))
    );
    assert_eq!(parsed.ty.to_string(), "Mod.Entity[][]");
    assert_eq!(
        parse_type_expression("BOOLEAN").expect("primitive").ty,
        TypeExpression::Primitive(PrimitiveType::Boolean)
    );
}

#[test]
fn parses_function_types_and_multiple_returns() {
    let parsed =
        parse_type_expression("fun(name: string, ...: number): Entity, nil").expect("type");

    assert_eq!(
        parsed.ty,
        TypeExpression::Function {
            parameters: vec![
                FunctionParameterType {
                    name: Some("name".into()),
                    ty: TypeExpression::Primitive(PrimitiveType::String),
                    variadic: false,
                },
                FunctionParameterType {
                    name: None,
                    ty: TypeExpression::Primitive(PrimitiveType::Number),
                    variadic: true,
                },
            ],
            returns: vec![TypeExpression::Name("Entity".into()), TypeExpression::Nil,],
        }
    );
}

#[test]
fn displays_named_variadic_parameter_types() {
    let parsed = parse_type_expression("fun(...args: string): boolean").expect("type");

    assert_eq!(parsed.ty.to_string(), "fun(...args: string): boolean");
    let TypeExpression::Function { parameters, .. } = parsed.ty else {
        panic!("function type");
    };
    assert_eq!(parameters[0].name.as_deref(), Some("args"));
    assert!(parameters[0].variadic);
}

#[test]
fn normalizes_optional_function_parameter_names() {
    let parsed = parse_type_expression("fun(item?: Weapon): boolean").expect("type");

    let TypeExpression::Function { parameters, .. } = parsed.ty else {
        panic!("function type");
    };
    assert_eq!(parameters[0].name.as_deref(), Some("item"));
    assert_eq!(
        parameters[0].ty,
        TypeExpression::union([TypeExpression::Name("Weapon".into()), TypeExpression::Nil])
    );
}

#[test]
fn rejects_non_lua_type_name_segments() {
    for input in ["9Entity", "-Entity", "Entity-Type", "Mod.9Entity"] {
        parse_type_expression(input).expect_err(input);
    }

    assert_eq!(
        parse_type_expression("_Mod.Entity9")
            .expect("Lua identifier segments")
            .ty,
        TypeExpression::Name("_Mod.Entity9".into())
    );
}

#[test]
fn reports_relative_errors() {
    let error = parse_type_expression("Entity | ").expect_err("missing union member");
    assert_eq!(error.offset, "Entity | ".len());

    let error = parse_type_expression("array<Entity").expect_err("missing bracket");
    assert_eq!(error.offset, "array<Entity".len());
}
