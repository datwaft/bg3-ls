# bg3-ls

> [!WARNING]
> This repository is 100% vibecoded. **Why?** because I needed some tooling to
> make some BG3 mods using Neovim, and I didn't want to spend my time on
> non-modding things.

`bg3-ls` is a standalone language server for Baldur's Gate 3 Stats, LSX, Thoth
helpers, and loose Osiris goals. It indexes loose Toolkit and mod data outside
the editor process. Neovim stays responsive while the server builds or
refreshes its index.

The server provides:

- ordered Go to Definition results for complete override chains;
- hover information for declarations, schema fields, Stats property names,
  enum values, functions, built-in Stats context properties, functor execution
  prefixes, resources, localization, and static game-text previews;
- references, document symbols, and workspace symbols;
- schema-aware completion with snippet support;
- verified signature help for curated Stats functions and declared Thoth helpers;
- conservative Thoth API evidence from configured loose and packaged sources;
- high-confidence syntax, schema, value, and typed-reference diagnostics;
- full-document overlays for unsaved files;
- recursive file watching and scoped module rebuilds;
- standard LSP work-done progress for clients such as fidget.nvim; and
- disposable XDG caches for fast warm starts.

The server uses [tree-sitter-bg3](https://github.com/datwaft/tree-sitter-bg3)
for legacy Stats syntax, embedded value expressions, Thoth helpers, and Osiris
goals. It streams XML with `quick-xml`; it does not require a Neovim XML
parser.

## Requirements

- Neovim nightly or Neovim 0.12+
- the `bg3_stats`, `bg3_lsx`, `bg3_localization`, `bg3_thoth`, and
  `bg3_osiris` filetypes from `tree-sitter-bg3` 0.5.2
- unpacked BG3 Toolkit data
- unpacked source directories for each mod dependency

The server reads three narrow package-backed data sources: the canonical LOCA
entry in `Localization/<Language>.pak`, the static `LSTag` glossary in
`Game.pak`, and Thoth helpers owned by configured base modules. It reads only
selected package entries and does not extract them. It does not provide general
`.pak` browsing or index arbitrary `.loca`, `.lsf`, or other binary resources.

## Install

Download a release archive for your platform and put `bg3-ls` on `PATH`.

For a source build, check out `bg3-ls` and the tagged grammar as sibling
directories. The colocated path keeps local grammar work testable, and release
CI checks out the exact `tree-sitter-bg3` tag:

```sh
git clone https://github.com/datwaft/bg3-ls
git clone --branch v0.5.2 https://github.com/datwaft/tree-sitter-bg3
cd bg3-ls
cargo install --path crates/bg3-ls --locked
```

Confirm that `bg3-ls` is on `PATH`:

```sh
bg3-ls --version
```

## Native LSF conversion

Convert a loose binary LSF resource to editable LSX without Wine, CrossOver,
or game data:

```sh
bg3-ls convert metadata.lsf metadata.lsx
```

Compile the edited LSX back to LSF:

```sh
bg3-ls convert metadata.lsx metadata.lsf --force
```

The command infers the direction from the extensions. It refuses to replace an
existing destination unless `--force` is present. Conversion writes a temporary
file beside the destination and publishes it only after the complete output is
written. A failed conversion does not change an existing destination.

Native LSF output is currently uncompressed. It remains valid BG3 data, but a
converted resource can be larger than its compressed source.

Version 0.19.0 describes Stats member expressions: hovering an enumeration
name or one of its values uses the Toolkit schema when it defines the
vocabulary and a curated catalog for `AttackType` and `DamageType`, which the
schema files omit. Hovering `context` and its members such as `context.Source`
uses a curated context-member catalog, `Target` documents the target-side
selector, and completion after `AttackType.`, `context.`, or `Target.`
offers exactly the matching members. Existing configuration remains
compatible, and no migration is required.

Version 0.18.0 curates `DealDamage` and `ExecuteWeaponFunctors` with typed
parameters, adds hover for curated parameter enum values such as `MainHand`
and the damage types, and completes exactly the documented domain inside
those argument positions. Expression parameters keep ordinary declaration
references. Existing configuration remains compatible, and no migration is
required.

Version 0.17.1 aligns with `tree-sitter-bg3` 0.5.2, whose Stats-value grammar
parses bracketed functor groups such as `CastOffhand[...]` and accepts
execution prefixes inside condition consequences. Bracketed statements keep
their references and join the expression preview. Existing configuration
remains compatible, and no migration is required.

Version 0.17.0 adds hover for legacy Stats property names: the quoted name of
a `data` clause renders its schema types, a curated description for
well-understood properties, and a fenced `bg3_stats_value` preview when the
value parses as structural expression syntax. Existing configuration remains
compatible, and no migration is required.

Version 0.16.1 aligns with `tree-sitter-bg3` 0.5.1, whose Stats-value grammar
parses functor execution-position prefixes as single statements. Prefixed
callees now highlight like other functions, and reference extraction no
longer treats prefix words such as `GROUND` as declaration references.
Existing configuration remains compatible, and no migration is required.

Version 0.16.0 adds curated hover and statement-start completion for functor
execution-position prefixes such as `GROUND:`, `TARGET:`, and the documented
`IF(condition):` conditional form. Curated vocabulary now sorts ahead of
observed evidence in capped completion lists. Existing configuration remains
compatible, and no migration is required.

Version 0.15.0 adds curated hover and completion for built-in Stats context
properties such as `MainMeleeWeapon`, `StrengthModifier`, and skill checks.
The catalog combines documented Stats expression keywords with weapon context
data attested in installed base modules. Identifiers outside the catalog stay
unreported because the engine vocabulary is not fully discoverable. Existing
configuration remains compatible, and no migration is required.

Version 0.14.0 indexes Stats declarations from configured base-module
packages, so references to base-game spells, statuses, passives, and items
resolve for hover, completion, and load-order-aware precedence without
manually unpacking packages. Packaged origins have no navigable source
location. Existing configuration remains compatible, and no migration is
required. This release uses `tree-sitter-bg3` 0.5.0.

Version 0.13.0 renders legacy Stats declarations in hover as highlighted
`bg3_stats` source blocks with resolved display-name and description comments,
hides presentation-only fields behind a count comment, elides values longer
than 160 characters with an ellipsis placeholder, and lists every stored field
for other formats. This release uses `tree-sitter-bg3` 0.5.0, whose Stats-value
grammar accepts the ellipsis placeholder. Existing configuration remains
compatible, and no migration is required.

Version 0.12.0 adds conservative packaged Thoth API inventory, coverage
classification, source-backed API indexing, and typed hover/signature support
for unique helpers with explicit annotations. It also diagnoses source-proven
`ConditionResult` values used with Lua `and` or `or`; use `&` or `|` for those
values. Unknown, unannotated, rejected, and ambiguous packaged symbols remain
untyped, and no migration is required. This release uses `tree-sitter-bg3`
0.4.3.

Version 0.11.0 adds conservative Thoth type flow for annotations, direct
assignments, unique helper return contracts, literals, supported operators,
schema enum values, and the built-in `ConditionResult` contract. It improves
Thoth completion, hover, and field definition navigation without adding
speculative semantic diagnostics. Unknown, ambiguous, and complex control
flow remains silent. Existing configuration remains compatible, and no
migration is required. This release uses `tree-sitter-bg3` 0.4.3.

Version 0.10.1 aligns source builds with `tree-sitter-bg3` 0.4.3. Existing
configuration remains compatible, and no migration is required.

Version 0.10.0 adds configured-language hover for LSX attributes with
`type="TranslatedString"`. Loose localization keeps normal module precedence
and replaces packed base text. Existing configuration remains compatible, and
no migration is required. This release continues to use `tree-sitter-bg3`
0.4.1.

Version 0.9.0 adds resolved hover for static game tooltip text and supported
typed `LSTag` resources in localization XML. Runtime-bound tooltip values stay
unresolved. Existing LSP configuration remains compatible, and this release
uses `tree-sitter-bg3` 0.4.1.

Version 0.8.0 added the native `bg3-ls convert` command for loose LSF and LSX
resources. Native LSF output is uncompressed, so compiled files can be larger
than their source resources.

## Configuration

Put shared workspace configuration in `bg3-ls.json` at the project root:

```json
{
  "$schema": "https://raw.githubusercontent.com/datwaft/bg3-ls/main/schemas/bg3-ls.schema.json",
  "game_data": "/path/to/Baldurs Gate 3/Data",
  "base_modules": ["Shared", "SharedDev", "Gustav", "GustavDev", "GustavX"],
  "project": {
    "name": "MyMod",
    "dependencies": [
      {
        "name": "Item and Spell Bug Fixes",
        "path": "../ItemAndSpellBugFixes"
      }
    ],
    "diagnostics": {
      "unresolved_references": "warning"
    }
  },
  "localization": {
    "language": "English"
  },
  "max_workspace_symbols": 200,
  "max_completion_items": 200
}
```

Inline LSP options override JSON fields. JSON fields override built-in defaults.
Nested objects merge by field, while an inline list replaces the complete JSON
list. The server rejects `null` values because configuration deletion has no
defined meaning.

Relative dependency paths resolve against the workspace root. `game_data` must
be absolute. The server rejects unknown keys, duplicate modules, missing roots,
and unsupported diagnostic severities.

## Neovim configuration

No Neovim integration plugin is required. Put the complete machine and project
configuration in the mod's trusted `.nvim.lua`:

```lua
local project_root = assert(vim.fs.root(vim.uv.cwd(), ".nvim.lua"))

vim.lsp.config("bg3", {
	cmd = { "bg3-ls" },
	filetypes = { "bg3_stats", "bg3_lsx", "bg3_localization", "bg3_thoth", "bg3_osiris" },
	workspace_required = true,

	-- Dependency files can be outside the project ancestor tree. Always return
	-- this root so those buffers reuse the same load-order-aware client.
	root_dir = function(_, on_dir)
		on_dir(project_root)
	end,

})

vim.lsp.enable("bg3")
```

Enable project configuration once in your normal Neovim configuration:

```lua
vim.o.exrc = true
```

Open the project and use `:trust` to approve its `.nvim.lua`. Relative
dependency paths resolve against `project_root`. Dependencies do not need their
own `.nvim.lua`.

`vim.lsp.config` can supply partial `init_options` when a project needs a local
override. Current inline-only configurations continue to work.

`unresolved_references` accepts `false`, `"error"`, `"warning"`,
`"information"`, or `"hint"`.

Standard LSP completion works with Blink's LSP source without extra setup.
Fidget displays the server's standard schema, discovery, parsing, module-build,
and publication progress.

Install `tree-sitter-bg3` as a Neovim plugin to detect `bg3_lsx`, `bg3_thoth`,
and `bg3_osiris` files. The plugin keeps XML as the outer LSX parser, injects
`bg3_stats_value` into selected `LSString` fields, parses `.khn` helpers with
the dedicated Thoth grammar, and detects Osiris only for
`Story/RawFiles/Goals/*.txt` paths.

## Load order

List base modules and dependencies from lowest to highest precedence. The
current project is always the highest layer:

```text
base_modules[1] ... base_modules[n]
dependencies[1] ... dependencies[n]
current project
```

Definition resolution examines that list in reverse. It returns the effective
project override first and the original base declaration last. It preserves all
same-module duplicates and labels them as ambiguous. Unconfigured modules are
not visible.

## Indexed sources

The server reads:

- `Editor/Config/Stats/StatObjectDefinitions.sod`;
- `Editor/Config/UuidObjects/TableDefinitions.sod`;
- Stats and UUID-object enumeration catalogs;
- Toolkit `.stats` and `.tbl` files;
- legacy `Public/*/Stats/Generated/Data/*.txt` files;
- Stats entries of configured base modules read from their module package and
  top-level patch packages;
- relevant loose `.lsx` resources below `Public` and `Mods`;
- loose `Mods/<module>/Scripts/thoth/**/*.khn` helpers;
- `Mods/<configured-module>/Scripts/thoth/**/*.khn` helpers selected from each
  configured base module package and top-level patch packages;
- loose `Mods/<module>/Story/RawFiles/Goals/*.txt` Osiris goals;
- loose localization XML for the configured language;
- the canonical configured-language LOCA catalog in the base-game localization
  package; and
- static tooltip-key localization handles from the canonical game UI glossary.

Declaration hover keeps the technical symbol information first. Legacy Stats
declarations render as a reconstructed `bg3_stats` code block in original
field order, so editors with the `tree-sitter-bg3` queries highlight it like a
Stats file. Resolved `DisplayName` and `Description` game text appears as
`//` comment lines above their fields, with one comment per localized line.
Presentation-only fields (`SpellAnimation`, `CastEffect`, and related sound,
sheathing, and cursor fields) are hidden behind a count comment. A value
longer than 160 characters is cut after its last complete top-level `;`
statement and ends with an ellipsis placeholder that `tree-sitter-bg3` 0.5.0
accepts; definition navigation always opens the complete source declaration.
Packaged base-module declarations render the same block and add a package
entry line, but they have no navigable location because their source lives
inside an archive. Other source formats list every stored field as Markdown
bullets. When an
effective Stats declaration has `DisplayName`, `Description`, or
`DescriptionParams`, hover adds a static game-text preview after a divider.
The preview resolves `using` inheritance and module overrides. Loose
localization uses normal module precedence and replaces packed base text. The
preview shows unresolved description parameters as source text because the
server does not run game logic.

Hover on a localization `LSTag` `Tooltip` value supports encoded
`&lt;...&gt;`, mixed `&lt;...>`, and literal `<...>` opening tags. Untyped
keys resolve static glossary title and description handles through the
configured language. Supported `Type` values resolve through the existing
load-order-aware Stats and resource indexes. Glossary entries that depend on
live UI or character state do not produce a preview.

Built-in Stats context properties such as `MainMeleeWeapon`,
`StrengthModifier`, or `Perception` resolve to curated hover with their kind
and meaning, and they appear in value completion next to declarations and
curated functions. The catalog combines the documented Stats expression
keywords, including ability and skill suffix forms such as
`DexteritySavingThrow` or `AthleticsModifier`, with weapon context data
attested inside installed base-module functor arguments. Identifiers outside
the catalog stay unreported because the engine vocabulary is not fully
discoverable.

Functor execution-position prefixes such as `GROUND:` or `TARGET:` resolve to
curated hover on the prefix word, and value completion offers them at functor
statement starts, after a top-level `;` or at the beginning of the value. The
catalog covers position selectors, the AI flags, the difficulty tiers, and the
documented `IF(condition):` conditional form, all attested inside installed
base-module statements. Prefixes compose, so hover describes each word
separately. Uncataloged prefixes stay unreported for the same reason as
context properties.

Hovering the quoted name of a legacy Stats `data` clause describes the
property: its name, the types reported by the effective schema chain, and a
curated description for well-understood properties such as `SpellProperties`
or `TargetConditions`. When the clause value parses as structural
`bg3_stats_value` syntax (functor calls, prefixes, conditions, dice, or
resource expressions), hover appends a fenced preview with one statement per
line. Plain values such as icons, handles, and markers get no preview.
Uncataloged property names show schema types only; nothing is invented.

Open Stats, LSX, Thoth, and Osiris files replace their disk records with
unsaved overlays. Closing a buffer restores its disk record. Thoth helper
declarations provide definition, hover, references, function completion,
declared-parameter signature help, document symbols, and workspace symbols.
The server also indexes conservative evidence from configured loose and
packaged Thoth sources: declared names and parameters, observed call names and
exact arity ranges, assignments, returns, and member-access chains. This
evidence can improve completion, hover, and signature help when the source
proves the fact. Generic access chains remain evidence only; the server does
not classify them as namespaces, enums, objects, or members without a declared
type. It keeps packaged helpers as immutable virtual sources and does not
expose their archive entries as editable filesystem locations or fake URIs.
Package priority selects a strictly higher-priority entry; equal-priority
candidates remain ambiguous. Loose dependency and project sources retain
their configured higher module precedence. Thoth parameter and return types
remain unknown unless an annotation or conservative type flow proves them.
Invalid Thoth syntax produces the stable `thoth-syntax-error` diagnostic code;
valid Thoth produces no syntax diagnostics. The server also warns for a
proven bare `ConditionResult` in an `if`, `elseif`, or `while` condition, for
`not` applied to one, and when two are combined with Lua `and` or `or` instead
of the supported `&` or `|` operators. Other semantic Thoth diagnostics remain
out of scope.

### Thoth annotations

Thoth helpers support a small, documented subset of
[LuaLS annotations](https://luals.github.io/wiki/annotations/). These tags are
line comments, so they are inert when the game loads the script:

```lua
---@class Weapon
---@field IsValid boolean

---@param weapon Weapon?
---@return ConditionResult
function IsWeaponCandidate(weapon) end
```

The supported tags are `---@class`, `---@field`, `---@alias`, `---@param`,
`---@return`, and `---@type`. Supported type syntax includes `boolean`,
`number`, `string`, `nil`, dotted names such as `Weapon.Properties`, unions
(`A|B`), nullable types (`Weapon?`), arrays (`Weapon[]`), and function-shaped
fields such as `fun(value: string): boolean`. A `---@param name? Type` tag
marks an optional parameter.

An annotation attaches only to the immediately following declaration or to
the immediately following contiguous annotation block. A blank line or an
ordinary comment breaks attachment. Malformed supported tags and type syntax
produce `thoth-annotation-error`; unknown names and unsupported annotation tags
are ignored. This is not full LuaLS compatibility.

Annotations provide explicit declaration, hover, signature, and typed member
evidence. Member completion and hover work for direct `---@type` bindings,
annotated parameters, annotated helper results, and proven field chains. Loose
fields navigate to their `---@field` name. Packaged fields remain virtual and
do not create fake locations. Unions expose only fields common to every known
non-`nil` member. Conservative type flow propagates explicit types through
direct assignments and uniquely resolved helper return contracts. Reachable
return statements contribute normalized unions, including incompatible return
types. Proven literals, schema-backed enum values, and supported primitive
unary and binary operators retain their known types. The built-in
`ConditionResult` constructor and its supported operators retain
`ConditionResult`.

Nil narrowing is limited to exact nil comparisons in a dominated branch or in
the remainder after a proven early exit. Unknown or ambiguous calls and
declarations, unsupported operators, and complex or uncertain control flow
remain unknown and produce no semantic diagnostics. The supported
`ConditionResult` warnings require exact type evidence; mixed Lua pass-through
expressions such as `flag and condition` remain silent.

Osiris navigation includes goals, parent edges, `PROC` and `QRY` declarations,
and user database occurrences. Procedure and query identity uses name and
arity in one callable namespace. User databases use a separate name-and-arity
namespace. A database has no source declaration, so definition returns the
first write in each contributing loose goal. References include all visible
reads and writes. Hover and signature help merge only explicit casts and
literal evidence from visible source. Unknown or conflicting columns remain
unknown. The server does not report missing Osiris symbols because engine and
packed declarations can be unavailable.

In supported LSX values, the server provides definition, hover, references,
function completion, typed symbol completion, and signature help. It does not
apply legacy Stats schema diagnostics to LSX documents.

Hover on an LSX attribute with `type="TranslatedString"` resolves its `handle`
through the configured-language localization sources. Loose localization keeps
normal module precedence and replaces packed base text.

Toolkit `.stats` and `.tbl` files remain readable navigation targets. The BG3
client attaches to localization XML for tooltip hover and references. It does
not attach to Toolkit Stats XML documents.

The watcher coalesces events for 250 ms. It rebuilds only affected modules and
publishes a complete immutable snapshot atomically. Queries continue to use the
previous snapshot during a refresh. A failed changed-module build keeps the
previous valid layer and reports a warning. A configured base or patch package
change refreshes the packaged Thoth catalog while reusing all unchanged module
layers.

## Commands and cache

Call server commands through the standard LSP API:

```lua
vim.lsp.buf.execute_command({ command = "bg3.reload", arguments = {} })
vim.lsp.buf.execute_command({ command = "bg3.indexInfo", arguments = {} })
```

The cache uses `$XDG_CACHE_HOME/bg3-ls`, or `~/.cache/bg3-ls` when the XDG
variable is not set. It stores versioned, checksummed Postcard objects and
module manifests. Corrupt or obsolete objects become cache misses. The server
removes incomplete temporary files on startup and unreferenced objects after 30
days.

```sh
bg3-ls cache path
bg3-ls cache info
bg3-ls cache clear
```

Run project diagnostics without an editor from any directory below a configured
workspace:

```sh
bg3-ls check
bg3-ls check Public/MyMod/Stats/Generated/Data/Passive.txt
bg3-ls check Mods/MyMod/Story/RawFiles/Goals/MyGoal.txt
bg3-ls check --format json --fail-on warning
```

The command finds the nearest `bg3-ls.json` in the current directory or its
ancestors. With no paths, it reports diagnostics for all legacy Stats files,
loose Thoth helpers, and loose Osiris goals in the project module. Thoth
diagnostics are syntax-only. Explicit files or directories limit diagnostic
output. The command still indexes visible dependencies and base modules for
resolution.

The stable syntax diagnostic codes are:

| Source | Code | Scope |
| --- | --- | --- |
| Legacy Stats | `syntax-error` | Stats syntax |
| Thoth | `thoth-syntax-error` | Thoth syntax only |
| Thoth | `thoth-annotation-error` | Supported annotation syntax |
| Osiris goals | `osiris-syntax-error` | Osiris goal syntax |

Human output uses one-based lines and columns. JSON output uses zero-based
`line` and `character` values that match LSP positions. `--fail-on` accepts
`error`, `warning`, `information`, `hint`, or `never`, and defaults to `error`.
Exit code 0 means no diagnostic met the threshold. Exit code 1 means at least
one diagnostic met it. Exit code 2 identifies a configuration, index, or file
analysis failure. Diagnostic data uses stdout, while progress uses stderr.

Use `--cache-dir PATH` before the subcommand to override the cache in tests.
Protocol traffic always uses stdout. Set `BG3_LS_LOG` to a tracing filter, such
as `bg3_ls=debug`, to write structured logs to stderr.

Inspect aggregate packaged-Thoth source coverage without extracting game data:

```sh
bg3-ls inventory --game-data "/path/to/Baldurs Gate 3/Data"
```

The command emits JSON counts for direct package files, package roots and
declared payload parts, matching Thoth entries, parseable and rejected
sources, declarations, annotations, duplicate functions, and module ownership.
It also separates unsupported package layouts and malformed packages from
entry-size, read, UTF-8, and syntax source rejections. It does not print
source text, change configured module resolution, or add discovered modules
to the LSP workspace.

## Benchmark

The benchmark requires a dedicated cache path because each cold trial clears
that path:

```sh
bg3-ls --cache-dir /tmp/bg3-ls-benchmark benchmark \
  --game-data "/path/to/Baldurs Gate 3/Data" \
  --workspace-root /path/to/MyMod \
  --project-name MyMod \
  --base-module Shared \
  --base-module SharedDev \
  --base-module Gustav \
  --base-module GustavDev \
  --base-module GustavX \
  --dependency "Dependency Name=../Dependency" \
  --trials 5
```

It emits JSON with cold and warm p50/p95 indexing times, warm cache hit rate,
observed resident memory, cache size, and exact navigation latency. See
[`PERFORMANCE.md`](PERFORMANCE.md) for the current machine baseline and allowed
regression limits.

## Development

```sh
make check
make test
make test-lsp
```

Tests use synthetic fixtures only. Do not add installed game or mod data to this
repository.

## Limitations

`bg3-ls` supports one active project per server process. It does not implement
rename, formatting, semantic tokens, code actions, arbitrary full-text search,
general packed-file extraction, dependency/mod package localization, binary
resource formats, automatic dependency discovery, Intel macOS, or native Windows releases.
LSX support uses a conservative field list. It does not provide an LSX schema,
field-name completion, LSX diagnostics, or XML entity transformation for
injected highlighting.
Most packed base resources are not visible, so diagnostics intentionally skip
generic expression identifiers, root-template UUID resolution, required
fields, and inferred function arity. Curated context properties power hover
and completion only; the server does not diagnose identifiers outside that
catalog because new engine keywords can appear between game patches. Static
tooltip previews do not evaluate `DescriptionParams`, live data bindings,
runtime values, gender variants, or BG3 UI rendering.

Thoth diagnostics cover syntax errors reported as `thoth-syntax-error` and
malformed supported annotations reported as `thoth-annotation-error`. Indexed
Thoth observations and annotations do not by themselves create semantic
diagnostics. General type propagation, nil narrowing, and semantic Thoth
diagnostics remain out of scope. Typed member features require explicit,
unambiguous annotations and stay silent for unknown receivers or fields.

Osiris support does not execute or compile Story, provide control-flow
analysis, load a complete engine API catalog, or infer aliases from
`story_header.div`. It does not diagnose database alias compatibility. A
[follow-up issue](https://github.com/datwaft/bg3-ls/issues/37) tracks that
check for configurations where installed or curated signatures prove the
types.

## License

MIT
