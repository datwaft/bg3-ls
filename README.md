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
  prefixes, resources, localization, static game-text previews, and rule-local
  Osiris variables;
- references, document symbols, and workspace symbols;
- schema-aware completion with snippet support;
- verified signature help for Stats functions, Osiris engine callables, and
  declared Thoth helpers;
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

The LSP transport negotiates UTF-8 or UTF-16 positions with the client and
uses UTF-16 when the client does not advertise an encoding. It uses full-text
open, change, close, and save synchronization. Work-done progress is emitted
only for clients that advertise support, and command progress uses the token
provided by the client.

## Requirements

- Neovim nightly or Neovim 0.12+
- the `bg3_stats`, `bg3_lsx`, `bg3_localization`, `bg3_thoth`, and
  `bg3_osiris` filetypes from `tree-sitter-bg3` 0.7.2
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
git clone --branch v0.7.2 https://github.com/datwaft/tree-sitter-bg3
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

## Osiris catalog maintenance

Maintainers can generate or verify the versioned Osiris engine contract catalog
from a game installation and its `story_header.div`:

```sh
bg3-ls catalog generate \
  --input "/path/to/story_header.div" \
  --game-root "/path/to/Baldurs Gate 3" \
  --output crates/bg3-index/src/osiris_catalog/generated_osiris_catalog.rs

bg3-ls catalog check \
  --input "/path/to/story_header.div" \
  --game-root "/path/to/Baldurs Gate 3" \
  --output crates/bg3-index/src/osiris_catalog/generated_osiris_catalog.rs

bg3-ls catalog check-descriptions
```

Generation records the exact game build metadata and the input header hash in
the generated artifact. The normal language server uses only the checked-in
catalog; it does not require an installed game or a mod-local
`story_header.div`. Do not commit installed game files or extracted game data.

Curated engine descriptions remain separate from generated contracts. Add only
concise reviewed paraphrases from official BG3 Modding documentation. Each
record must include the exact callable kind, name, and arity, its official
source URL pinned to the recorded wiki revision ID, and the latest prose
update and review date. Verify the URL and revision manually, keep the records
sorted, then run `bg3-ls catalog check-descriptions`; the offline command
rejects duplicate, stale, or mismatched keys and incomplete provenance. Review
wiki input explicitly and do not copy pages into the repository. The language
server never fetches documentation at runtime.

Version 0.31.0 expands the verified Osiris description catalog with reviewed,
provenance-backed descriptions for additional engine callables. Hover,
completion, and signature help show prose only when the exact callable kind,
name, and arity are verified; undocumented callables remain signature-only.
`bg3-ls catalog check-descriptions` audits the catalog offline. Existing
configuration and caches remain compatible, runtime documentation access stays
offline, and no migration or cache reset is required. The release remains
compatible with `tree-sitter-bg3` 0.7.2.

Version 0.30.0 caches the packaged Osiris catalog for standalone `check` runs
and reports aggregate workspace cache hits and misses on stderr. Repeated
checks avoid unchanged package extraction while diagnostic stdout, ordering,
failure thresholds, and exit codes remain compatible. Existing caches rebuild
automatically when their inputs or versions change, so no cache clear or
configuration migration is required. The release remains compatible with
`tree-sitter-bg3` 0.7.2.

Version 0.29.2 restores the curated description for the `HasPassive` Osiris
engine query in hover, completion, and signature help. The release remains
compatible with `tree-sitter-bg3` 0.7.2 and existing configuration; no
migration is required.

Version 0.29.1 reuses Osiris database schema analysis across hover requests.
Immutable schemas are prepared once per workspace snapshot, and overlay-derived
schemas are cached until an open document changes. Installed engine queries
also retain exact curated descriptions in hover, completion, and signature
help. The release remains compatible with `tree-sitter-bg3` 0.7.2 and existing
configuration; no migration is required.

Version 0.29.0 completes the Osiris validation and editor-support batch.
Callable roles, event placement, and `NOT` usage are validated
conservatively. GUID-family mismatches, malformed packaged facts, and
incomplete roots are rejected when syntax or contract metadata proves the
problem. Multiline signature help now tracks the complete call context,
completion respects parameter roles, and document symbols use declaration
spans. The release uses `tree-sitter-bg3` 0.7.2, which rejects empty `THEN`
blocks, parses dotted enum constants, and distinguishes complete goal files
from standalone callable-signature sources. Existing configuration remains
compatible, and no migration is required.

Version 0.28.0 adds source-ordered Osiris database schema analysis. Database
types now use global Story goal order independently from module precedence, and
unique database types propagate through valid rule-local reads into later
writes. Hover, signature help, variable types, and diagnostics share the same
schema and propagation results. Missing, conflicting, ambiguous, or
overlay-incomplete evidence remains unknown. Open-file overlays replace their
disk records and recompute the propagation closure. Existing configuration
remains compatible, and no migration is required.

Version 0.27.3 fixes a false `osiris-database-alias-mismatch` diagnostic when a
database fact is read to match an existing row. Relational reads use the
established database schema without requiring an explicit cast. Existing
configuration remains compatible, and no migration is required.

Version 0.27.2 fixes Osiris signature context when typed casts use grouping
parentheses, and uses the released `bg3_osiris` grammar contract for generated
hover markup. The catalog provenance label is rendered as a heading. Existing
configuration remains compatible, and no migration is required.

Version 0.27.1 improves generated Osiris callable help. Long signatures render
with one parameter per line, and signature help refreshes after the opening
parenthesis and each comma throughout a call. Variable hover reports only the
proven type. Existing configuration remains compatible, and no migration is
required.

Version 0.27.0 adds generated Osiris callable help. Hover and signature help now
show verified engine callable signatures, parameter directions, and available
descriptions from the checked-in catalog without requiring game files at
runtime. Rule-local variable hover reports the inferred type without exposing
internal evidence details; go to definition and references remain available
for locating the binding. Existing configuration remains compatible, and no
migration is required.

Version 0.26.0 adds a checked-in, generated Osiris engine contract catalog and
Rust maintenance commands to regenerate or verify it from `story_header.div`
and the installed game's build metadata. The catalog supplies conservative
types and input/output directions for engine calls, events, and queries without
requiring game files at runtime. Existing configuration remains compatible, and
no migration is required.

Version 0.25.0 tracks rule-local Osiris variables across hover, go to
definition, and references. Proven event, database, and documented query
outputs provide bindings; unknown call directions, negated conditions, and
user-query arguments remain unresolved to avoid false positives. Variables
remain scoped to one rule, procedure, or query, and no configuration migration
is required. The Osiris facts cache rebuilds once after this release.

Version 0.24.2 fixes packaged Osiris indexing in LSP workspaces, restores
configured base-module precedence for packaged callable hover and signature
help, and avoids incorrect alias diagnostics when an engine event repeats a
variable with conflicting aliases. Packaged Osiris discovery now accepts only
direct goal files under `Goals/`, consistent with loose-source discovery. The
Osiris facts cache rebuilds once after this release because its curated event
catalog and inference behavior changed. Existing configuration remains
compatible, and no migration is required.

Version 0.24.1 aligns source builds with `tree-sitter-bg3` 0.5.3, which
improves localization markup highlighting in Neovim. The pinned parsers are
unchanged from 0.5.2, so language-server behavior is identical. Existing
configuration remains compatible, and no migration is required.

Version 0.24.0 indexes procedures and queries declared in installed
base-module goals the same way as packaged Thoth helpers. Hover, signature
help, and completion show their authored parameter aliases with module
provenance and no file location. Loose declarations outrank installed ones,
same-rank installed disagreements stay untyped, and open overlays replace
disk evidence live. Existing configuration remains compatible, and no
migration is required.

Version 0.23.0 expands the curated Osiris event catalog to cover every
installed engine event, transcribed from a machine-generated community
reference of the installed engine API. Uncast rule-head variables now inherit
aliases from any known event instead of only twelve common gameplay events,
so more proven `osiris-database-alias-mismatch` diagnostics appear where
columns receive different aliases. The original signatures are unchanged,
unknown events stay silent, and no configuration changes. Existing valid
configuration remains compatible, and no migration is required.

Version 0.22.0 diagnoses Osiris database alias mismatches: a user database
column established with one alias, such as `CHARACTER`, rejects a later
argument with a different alias, such as plain `GUIDSTRING`, through the new
stable `osiris-database-alias-mismatch` error. Uncast rule-head variables
inherit an alias from curated common gameplay event signatures such as `Died`
or `AddedTo`. Unknown events and unknown columns stay silent, and engine call
arguments stay unchecked so specific values remain valid for generic engine
parameters. The index cache rebuilds once after this release because cached
Osiris evidence gained provenance. Existing configuration remains compatible,
and no migration is required.

Version 0.21.0 unifies hover presentation across supported languages. Hovers
now use consistent headings, evidence wording, provenance, bounded disclosure,
exact symbol ranges, and client content-format negotiation. The server adds a
conservative description for unresolved Osiris callables when syntax proves the
name and arity, without guessing callable kind or parameter types. Existing
configuration remains compatible, and no migration is required.

Version 0.20.3 restores LSP 3.17 protocol conformance for position encoding,
process lifecycle, work-done progress, full-document synchronization, and
JSON-RPC error classification. The server now negotiates UTF-8 or UTF-16,
defaults to UTF-16 when the client does not advertise an encoding, ignores
stale document versions, and stops background protocol activity after
shutdown. Raw stdio regression tests cover these contracts. Existing
configuration remains compatible, and no migration is required.

Version 0.20.2 validates `localization.language` as one safe catalog name
before any package probe. The server rejects configuration values with path
separators, Windows-reserved filename characters, traversal components, NUL,
or control characters, so derived package paths always stay below
`Data/Localization`. Normal language names keep working. Existing valid
configuration remains compatible, and no migration is required.

Version 0.20.1 abbreviates the home directory prefix as `~` in hover source
paths, override chains, and Osiris contributing-goal lists so long absolute
paths stay readable. Existing configuration remains compatible, and no
migration is required.

Version 0.20.0 surfaces Thoth doc-comment prose: plain `---` comment lines in
an annotation block become the helper description, hover over legacy Stats
call sites and over the `.khn` definition shows this prose next to the
signature, signature help and completion documentation include it, and
`---@returns` is accepted as a spelling of `---@return`. A prose-only helper
keeps its declared parameter names instead of inventing types. The index
cache is rebuilt once after this release because cached annotations gained a
field. Existing configuration remains compatible, and no migration is
required.

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

`localization.language` must be one safe catalog name such as `English`. It
must not contain path separators, Windows-reserved filename characters,
traversal components, NUL, or control characters, so derived package paths
always stay below `Data/Localization`. The server rejects the configuration
when the value is unsafe.

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

### Hover behavior

Hover output uses a consistent information order where the evidence supports
it: kind and name first, then signature or type, documentation, contextual
facts, previews, and source provenance. Optional facts are omitted when the
index cannot prove them. Provenance identifies the contributing module and
source when available; packaged declarations also show their package entry.
Installed, observed, and same-priority ambiguous evidence keeps those labels
so that an observation is not presented as a verified declaration. Override
chains and static game-text previews are shown after the primary symbol
information. Preview text is static: unresolved description parameters remain
source text and the server does not run game logic.

The server returns the smallest syntax-backed range that contains the hovered
symbol, field, or reference. When no semantic span is available, it uses the
matching lexical word when one can be identified; otherwise the hover has no
range. Ranges use the position encoding negotiated during initialization
(UTF-8 or UTF-16, with UTF-16 as the default when the client advertises no
encoding).

Hover content follows the client's advertised content format. Clients that
support Markdown receive the structured Markdown form. Clients that request
plain text receive a readable plain-text rendering with Markdown decoration
and code fences removed.

Repeated provenance and list sections are bounded to keep editor popups
usable. They show at most 12 entries and add an explicit omission marker when
more entries exist. Reconstructed Stats blocks show at most 64 fields.
Individual stored field values remain capped at 160 characters where the
declaration renderer applies that limit. These limits do not imply that
omitted declarations are unavailable: definition navigation can still open
the complete loose source declaration.

Hover remains deliberately conservative. The server does not invent types,
required fields, return values, function arity, or unavailable packed-resource
declarations. It does not expose fabricated archive locations, evaluate
runtime-bound localization or tooltip values, or classify unknown Thoth member
chains without explicit type evidence. An absent loose declaration is not by
itself proof that a reference is invalid because base resources can exist only
in packages.

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

Osiris goals receive the stable `osiris-database-alias-mismatch` error when a
user database column receives two different aliases across the visible loose
goals of the workspace. Schema establishment follows alphabetical goal order
and source occurrence order within each goal, independently of module
precedence. The checker accepts a generic `GUIDSTRING` with any verified
specialized GUID alias, such as `CHARACTER` or `ITEM`, but rejects two
different specialized aliases, such as `CHARACTER` and `ITEM`, when both are
proven. Unknown aliases remain silent. Uncast variables inherit aliases from
the versioned generated engine contract catalog, which covers installed
events, calls, and queries; unknown contracts and unknown columns stay silent.
The server uses callable parameters as type evidence but does not diagnose
engine call arguments, so a specific `CHARACTER` value remains valid for an
engine parameter that accepts generic `GUIDSTRING`.

Known engine callables also receive conservative role checks. Events must be
the first trigger of an `IF` rule, queries and sysqueries must be conditions,
and calls and syscalls must be actions. `NOT` is diagnosed on actions when it
is applied to a known engine callable; database facts remain valid negated
conditions and removals. Unknown names remain silent because they may be
user-defined callables from another Story source; a `DB_`, `QRY_`, or `PROC_`
prefix alone does not establish a role.

Rule-local Osiris variables provide definition, reference, and hover navigation
when a rule head, positive user-database condition, or generated query
`[out]` parameter proves their binding. Bindings and types flow forward through
conditions in source order: a later database or query output does not resolve
an earlier use, and an already-bound output acts as a filter. Explicit casts
apply to their source occurrence. Unknown callable directions, negated
conditions, and action arguments remain silent.

Engine Osiris callables use the checked-in, generated contract catalog for
signatures, parameter directions, and parameter types. Hover and signature
help therefore work for calls such as `GetActionResourceValuePersonal` and
`CastedSpell` even when their declarations are packed or unavailable as loose
files. A versioned, provenance-backed overlay supplies reviewed official
descriptions only when callable kind, name, and arity match exactly. Other
catalogued callables still show their verified signature without invented
prose. Variable hover shows the proven type only; use go to definition or
references for binding locations. Hover formats long engine signatures with
one parameter per line. Signature help refreshes after commas and selects the
parameter at the cursor throughout the call.

Osiris callable completion follows the statement role: engine events are
offered for rule heads, queries for conditions, and calls for actions. User
procedures and queries follow the same action and condition split. Database
facts remain available in rule heads, conditions, and fact actions.

Procedures and queries declared in installed base-module goals are indexed
the same way as packaged Thoth helpers. Hover, signature help, and completion
show their authored parameter aliases with module provenance and no file
location. Loose declarations outrank installed ones, same-rank installed
disagreements stay untyped, and open overlays replace disk evidence live.

### Thoth annotations

Thoth helpers support a small, documented subset of
[LuaLS annotations](https://luals.github.io/wiki/annotations/). These tags are
line comments, so they are inert when the game loads the script:

```lua
--- Returns whether the entity can equip the candidate as a weapon.
---@class Weapon
---@field IsValid boolean

--- Used in condition contexts.
---@param weapon Weapon?
---@return ConditionResult
function IsWeaponCandidate(weapon) end
```

The supported tags are `---@class`, `---@field`, `---@alias`, `---@param`,
`---@return` (also spelled `---@returns`), and `---@type`. Supported type syntax includes `boolean`,
`number`, `string`, `nil`, dotted names such as `Weapon.Properties`, unions
(`A|B`), nullable types (`Weapon?`), arrays (`Weapon[]`), and function-shaped
fields such as `fun(value: string): boolean`. A `---@param name? Type` tag
marks an optional parameter.

Plain `---` doc-comment lines in the same block become the declaration
description. Hover and signature help show this prose next to the signature. A
prose-only helper keeps its declared parameter names instead of inventing
types.

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
| Osiris goals | `osiris-invalid-callable-role` | Known engine callable placement |
| Osiris goals | `osiris-invalid-negation` | `NOT` action applied to a known engine callable |

Human output uses one-based lines and columns. JSON output uses zero-based
`line` and `character` values that match LSP positions. `--fail-on` accepts
`error`, `warning`, `information`, `hint`, or `never`, and defaults to `error`.
Exit code 0 means no diagnostic met the threshold. Exit code 1 means at least
one diagnostic met it. Exit code 2 identifies a configuration, index, or file
analysis failure. Diagnostic data uses stdout, while progress uses stderr.
After indexing, `check` also reports aggregate workspace cache hits and misses
on stderr. Repeated runs reuse the packaged Osiris catalog and parsed facts;
changed package inputs and corrupt or obsolete cache objects rebuild as cache
misses. Diagnostic stdout and exit-code behavior do not change.

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

Maintainers create an annotated `vX.Y.Z` tag on `main` to start a release. The
release workflow accepts only numeric version components without leading
zeroes, then checks main ancestry and Cargo versions,
reruns CI at the tagged commit, builds the macOS ARM and Linux archives, checks
their checksums, and publishes them after every matrix job succeeds. Tags are
not released when the tagged commit is not on `main` or the package versions do
not match the tag.

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
analysis, or infer aliases from a mod-local `story_header.div`. Runtime engine
contracts and the verified GUID-family aliases are versioned with the
checked-in generated catalog; regenerating that catalog is a maintainer
operation that requires the game installation. Variable tracking remains
rule-local and conservative: it
does not infer bindings through unknown callable directions, negation, or
uncertain control flow. Database alias compatibility is diagnosed only when
the visible source or curated event signatures prove the relevant aliases.

## License

MIT
