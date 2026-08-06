# bg3-ls

> [!WARNING]
> This repository is 100% vibecoded. **Why?** because I needed some tooling to
> make some BG3 mods using Neovim, and I didn't want to spend my time on
> non-modding things.

`bg3-ls` is a standalone language server for Baldur's Gate 3 Stats files. It
indexes loose Toolkit and mod data outside the editor process. Neovim stays
responsive while the server builds or refreshes its index.

The server provides:

- ordered Go to Definition results for complete override chains;
- hover information for declarations, schema fields, enum values, functions,
  resources, and localization;
- references, document symbols, and workspace symbols;
- schema-aware completion with snippet support;
- verified signature help for curated Stats functions;
- high-confidence syntax, schema, value, and typed-reference diagnostics;
- full-document overlays for unsaved files;
- recursive file watching and scoped module rebuilds;
- standard LSP work-done progress for clients such as fidget.nvim; and
- disposable XDG caches for fast warm starts.

The server uses [tree-sitter-bg3](https://github.com/datwaft/tree-sitter-bg3)
for legacy Stats syntax and embedded value expressions. It streams XML with
`quick-xml`; it does not require a Neovim XML parser.

## Requirements

- Neovim nightly or Neovim 0.12+
- the `bg3_stats` filetype from `tree-sitter-bg3`
- unpacked BG3 Toolkit data
- unpacked source directories for each mod dependency

The server does not read `.pak`, `.loca`, `.lsf`, or other binary files.

## Install

Download a release archive for your platform and put `bg3-ls` on `PATH`.

For a source build, check out `bg3-ls` and the tagged grammar as sibling
directories. The colocated path keeps local grammar work testable, and release
CI checks out the exact `tree-sitter-bg3` tag:

```sh
git clone https://github.com/datwaft/bg3-ls
git clone --branch v0.1.0 https://github.com/datwaft/tree-sitter-bg3
cd bg3-ls
cargo install --path crates/bg3-ls --locked
```

Confirm that `bg3-ls` is on `PATH`:

```sh
bg3-ls --version
```

## Neovim configuration

No Neovim integration plugin is required. Put the complete machine and project
configuration in the mod's trusted `.nvim.lua`:

```lua
local project_root = assert(vim.fs.root(vim.uv.cwd(), ".nvim.lua"))

vim.lsp.config("bg3", {
	cmd = { "bg3-ls" },
	filetypes = { "bg3_stats" },
	workspace_required = true,

	-- Dependency files can be outside the project ancestor tree. Always return
	-- this root so those buffers reuse the same load-order-aware client.
	root_dir = function(_, on_dir)
		on_dir(project_root)
	end,

	init_options = {
		game_data = "/path/to/Baldurs Gate 3/Data",
		base_modules = {
			"Shared",
			"SharedDev",
			"Gustav",
			"GustavDev",
			"GustavX",
		},
		project = {
			name = "MyMod",
			dependencies = {
				{
					name = "Item and Spell Bug Fixes",
					path = "../ItemAndSpellBugFixes",
				},
			},
			diagnostics = {
				unresolved_references = "warning",
			},
		},
		localization = {
			language = "English",
		},
		max_workspace_symbols = 200,
		max_completion_items = 200,
	},
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

`unresolved_references` accepts `false`, `"error"`, `"warning"`,
`"information"`, or `"hint"`. All option tables reject unknown keys. The
server also rejects duplicate module names, relative `game_data` paths, missing
module roots, and missing schema catalogs during initialization.

Standard LSP completion works with Blink's LSP source without extra setup.
Fidget displays the server's standard schema, discovery, parsing, module-build,
and publication progress.

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
- relevant loose `.lsx` resources below `Public` and `Mods`; and
- loose localization XML for the configured language.

Open Stats files replace their disk records with unsaved overlays. Closing a
buffer restores its disk record. XML resources remain readable navigation
targets, but the BG3 client does not attach to them.

The watcher coalesces events for 250 ms. It rebuilds only affected modules and
publishes a complete immutable snapshot atomically. Queries continue to use the
previous snapshot during a refresh. A failed changed-module build keeps the
previous valid layer and reports a warning.

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

Use `--cache-dir PATH` before the subcommand to override the cache in tests.
Protocol traffic always uses stdout. Set `BG3_LS_LOG` to a tracing filter, such
as `bg3_ls=debug`, to write structured logs to stderr.

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
packed-file extraction, binary localization/resource formats, automatic
dependency discovery, or native Windows releases. Packed base resources are
not visible, so diagnostics intentionally skip generic expression identifiers,
root-template UUID resolution, required fields, and inferred function arity.

## License

MIT
