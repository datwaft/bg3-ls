use std::path::{Path, PathBuf};
use std::sync::Arc;

use bg3_ide::{OverlayDocument, OverlaySet, WorkspaceSnapshot};
use bg3_index::{
    ModuleIndex, ModuleRole, ModuleSpec, Position, SchemaCatalog, SourceFile, SourceKind,
    TextRange, parse_source,
};

fn parse_overlay(
    path: &Path,
    text: &str,
    kind: SourceKind,
    schema: &SchemaCatalog,
) -> Arc<bg3_index::ParsedFile> {
    Arc::new(
        parse_source(
            SourceFile {
                path: path.to_owned(),
                kind,
            },
            text,
            schema,
            "English",
        )
        .expect("synthetic source"),
    )
}

fn workspace(schema: SchemaCatalog) -> WorkspaceSnapshot {
    WorkspaceSnapshot::new(
        Arc::new(schema),
        vec![Arc::new(ModuleIndex::new(
            ModuleSpec {
                name: "MyMod".into(),
                root: PathBuf::from("/synthetic/MyMod"),
                role: ModuleRole::Project,
            },
            Vec::new(),
        ))],
        1,
        200,
        200,
    )
}

fn workspace_with_thoth_files(
    schema: SchemaCatalog,
    files: Vec<(&str, &str)>,
) -> WorkspaceSnapshot {
    let schema = Arc::new(schema);
    let parsed = files
        .into_iter()
        .map(|(path, text)| {
            parse_source(
                SourceFile {
                    path: PathBuf::from(path),
                    kind: SourceKind::Thoth,
                },
                text,
                &schema,
                "English",
            )
            .expect("synthetic Thoth source")
        })
        .collect();
    WorkspaceSnapshot::new(
        schema,
        vec![Arc::new(ModuleIndex::new(
            ModuleSpec {
                name: "MyMod".into(),
                root: PathBuf::from("/synthetic/MyMod"),
                role: ModuleRole::Project,
            },
            parsed,
        ))],
        1,
        200,
        200,
    )
}

fn overlays(
    workspace: &WorkspaceSnapshot,
    path: &Path,
    text: &str,
    kind: SourceKind,
) -> OverlaySet {
    let mut overlays = OverlaySet::default();
    overlays.insert(
        path.to_owned(),
        OverlayDocument {
            module: "MyMod".into(),
            version: 1,
            text: text.into(),
            parsed: parse_overlay(path, text, kind, &workspace.schema),
        },
    );
    overlays
}

fn at(text: &str, needle: &str) -> Position {
    text.lines()
        .enumerate()
        .find_map(|(line, source)| {
            source.find(needle).map(|character| Position {
                line: u32::try_from(line).unwrap(),
                character: u32::try_from(character).unwrap(),
            })
        })
        .expect("synthetic token")
}

#[test]
fn canonical_thoth_annotation_hover_has_stable_order_and_exact_range() {
    let workspace = workspace(SchemaCatalog::default());

    let thoth_path = PathBuf::from("/synthetic/MyMod/Scripts/Helper.khn");
    let thoth_text = "--- A *literal* [note].\n---@param value number\n---@return boolean\nfunction Helper(value)\n  return true\nend\n";
    let thoth_overlays = overlays(&workspace, &thoth_path, thoth_text, SourceKind::Thoth);
    let hover = workspace
        .hover(&thoth_path, at(thoth_text, "Helper"), &thoth_overlays)
        .expect("annotated hover");
    assert_eq!(
        hover.markdown,
        "**Thoth function** `Helper`\n\nSignature: `Helper(value: number): boolean`\n\nA \\*literal\\* \\[note\\].\n\nExplicit Thoth annotation.\n\nReturns: `boolean`\n\nModule: `MyMod`\n\nSource: `/synthetic/MyMod/Scripts/Helper.khn`"
    );
    assert_eq!(
        hover.range,
        Some(TextRange {
            start: Position {
                line: 3,
                character: 9
            },
            end: Position {
                line: 3,
                character: 15
            },
        })
    );
}

#[test]
fn canonical_stats_hovers_have_stable_markdown_contracts() {
    let workspace = workspace(SchemaCatalog::default());

    let stats_path = PathBuf::from("/synthetic/MyMod/Stats/Test.txt");
    let stats_text = "new entry \"TEST\"\ntype \"SpellData\"\ndata \"SpellProperties\" \"GROUND:ExecuteWeaponFunctors(MainHand);RemoveStatus(SELF,Y)\"";
    let stats_overlays = overlays(&workspace, &stats_path, stats_text, SourceKind::PlainStats);

    let property = workspace
        .language_hover(
            &stats_path,
            at(stats_text, "SpellProperties"),
            &stats_overlays,
        )
        .expect("Stats property hover");
    assert_eq!(
        property.markdown,
        "**Stats property** `SpellProperties`\n\nFunctors that run when the spell resolves, grouped by execution position prefixes.\n\n```bg3_stats_value\nGROUND:ExecuteWeaponFunctors(MainHand)\nRemoveStatus(SELF,Y)\n```"
    );
    let enum_hover = workspace
        .language_hover(&stats_path, at(stats_text, "MainHand"), &stats_overlays)
        .expect("Stats enum hover");
    assert_eq!(
        enum_hover.markdown,
        "**Enum value** `MainHand`\n\nParameter: `eHandSlot`\n\nFunction: `ExecuteWeaponFunctors`\n\nWeapon slot selector for weapon functors."
    );
}

#[test]
fn canonical_ambiguous_thoth_hover_does_not_invent_signature() {
    let ambiguous_workspace = workspace_with_thoth_files(
        SchemaCatalog::default(),
        vec![
            (
                "/synthetic/MyMod/Scripts/A.khn",
                "function Ambiguous(value) end\n",
            ),
            (
                "/synthetic/MyMod/Scripts/B.khn",
                "function Ambiguous(value, other) end\n",
            ),
        ],
    );
    let ambiguous_path = PathBuf::from("/synthetic/MyMod/Stats/Ambiguous.txt");
    let ambiguous_text =
        "new entry \"TEST\"\ntype \"PassiveData\"\ndata \"Boosts\" \"Ambiguous(1)\"";
    let ambiguous_overlays = overlays(
        &ambiguous_workspace,
        &ambiguous_path,
        ambiguous_text,
        SourceKind::PlainStats,
    );
    let hover = ambiguous_workspace
        .language_hover(
            &ambiguous_path,
            at(ambiguous_text, "Ambiguous"),
            &ambiguous_overlays,
        )
        .expect("ambiguous Thoth hover");
    assert_eq!(
        hover.markdown,
        "**Thoth function** `Ambiguous`\n\nModule: `MyMod`\n\nDeclarations: `2`\n\nSame-rank Thoth declarations are ambiguous. The signature is not verified."
    );
    assert!(!hover.markdown.contains("Signature:"));
}
