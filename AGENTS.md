# Repository Instructions

These instructions apply to all work in `bg3-ls`. Keep this project useful,
fast, and conservative about diagnostics. Prefer a small correct change to a
large speculative change.

## Product Direction

`bg3-ls` is a standalone Rust language server for loose Baldur's Gate 3 mod
data. It must keep editor integration standard. Users configure it through LSP
initialization options or `bg3-ls.json`. They must not need a Neovim plugin.

Maintain these crate boundaries:

- `bg3-index` owns discovery, schemas, parsers, caches, and domain records.
- `bg3-ide` owns editor-neutral language operations and internal result types.
- `bg3-ls` owns LSP types, configuration, the CLI, progress, watchers, and
  process coordination.

Do not put LSP protocol types in `bg3-index` or `bg3-ide`. Do not add a database
when exact immutable indexes are sufficient. Keep published workspace
snapshots immutable and keep queries available during index refreshes.

Use `tree-sitter-bg3` as the syntax contract for legacy Stats and Stats-value
expressions. Change the grammar only when it cannot represent valid syntax.
Put grammar defects and grammar changes in the `tree-sitter-bg3` repository.

## Correctness Policy

Prefer no diagnostic to an incorrect diagnostic. Installed base resources can
exist only in packed files, so an absent loose declaration does not always mean
that a reference is invalid.

Add a diagnostic only when syntax, schema metadata, or a curated function
contract proves the problem. Do not guess generic expression types, required
fields, function arity, or unavailable packed resources.

Keep load-order behavior explicit:

1. Base-module precedence increases in configured order.
2. Dependency precedence increases in configured order.
3. The current project has the highest precedence.

Resolution examines the layers in reverse order. It must preserve same-rank
ambiguity and must not expose unconfigured modules.

## Source and Test Data

Use synthetic fixtures in the repository. Never commit installed BG3 data,
unpacked mods, `.pak` contents, user paths, or copyrighted localization text.

You can use installed data for a local smoke test. Do not include that data in
a patch, test snapshot, log, issue, or pull request. Reduce each regression to
a minimal synthetic fixture before commit.

## Implementation Process

Before a change:

1. Read the applicable crate and its tests.
2. Reproduce a reported defect when reproduction is possible.
3. Classify the problem as a server defect, grammar defect, or mod defect.
4. Confirm the public behavior and the acceptance criteria.
5. Check the current Jujutsu status before edits.

Use concrete, top-down Rust. Add a helper or named type only when it has a clear
design purpose. Examples include a stable domain concept, fragile behavior,
complex nested code, or an independently testable operation. Fail clearly on
unknown input. Do not use a silent fallback.

Keep comments focused on constraints and non-obvious decisions. Do not narrate
the code. Add Rust documentation to stable public concepts and interfaces.

After a change:

1. Add or update tests for observable behavior.
2. Run the applicable verification commands.
3. Inspect the complete diff.
4. Update user documentation when behavior or configuration changes.
5. Decide whether the change requires a release version update.

## Required Verification

Run these commands for each Rust change:

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Run `make test-lsp` when a change affects protocol behavior, synchronization,
progress, configuration, overlays, or process lifecycle. Run `cargo deny check`
when dependencies or license policy change and the command is available.

Use `bg3-ls check` on an affected real mod only as a local smoke test. A smoke
test does not replace a synthetic regression test.

For index, cache, or parser performance changes, run the full-data benchmark
with five cold trials and five warm trials. Compare p50, p95, cache hit rate,
peak memory, and cache size with `PERFORMANCE.md`.

## Issue Policy

Create a GitHub issue before substantial work when one of these conditions is
true:

- A user reports a reproducible server defect.
- The product needs a new command, configuration field, LSP capability, or data
  source.
- A change needs a design decision or more than one pull request.
- A discovered problem is valid but outside the active issue.
- A performance regression needs measurement and follow-up work.

Do not create a separate issue for a small correction that is necessary to
complete the active issue. Do not create a `bg3-ls` issue for a confirmed mod
defect or grammar defect.

A useful bug issue contains:

- the affected version or commit.
- the expected and actual behavior.
- a minimal synthetic example or safe reproduction procedure.
- the diagnostic code or LSP method when applicable.
- the probable component, if known.
- clear acceptance criteria.

Use concise technical English. Preserve uncertainty when the cause is not yet
known. Do not include private paths or installed game data.

## Jujutsu Commit Process

Use Jujutsu for all version-control writes. Jujutsu and Git share this
repository so that `gh` can create issues and pull requests.

Start new work from current `main`:

```sh
jj git fetch
jj new main
jj status
```

Make one coherent commit for one logical change. Include its tests and required
documentation in the same commit. Do not mix cleanup or unrelated refactors
with a feature or fix.

Use an active Conventional Commit title:

```text
feat: add JSON workspace configuration
fix: interpret legacy schema discriminators
docs: define the release process
test: cover cache corruption recovery
perf: reduce warm index construction time
refactor: isolate schema selection
chore: prepare v0.4.0
```

Keep the title concise. Use a commit body when the reason, compatibility limit,
or safety constraint is not clear from the diff. Describe the problem, the
change, and important limitations. Report only the verification that you ran.

Describe the current commit without an interactive editor:

```sh
jj describe -m "fix: describe the change"
```

Inspect `jj status`, `jj diff`, and `jj log` before publication. Do not use Git
commands to mutate commits, branches, or the worktree.

## Pull Request Process

Create a pull request for every change that will enter `main`. Do not push a
change directly to `main`. Create one focused pull request for each issue. A
pull request can contain more than one commit only when the commits form a
clear review sequence.

A small documentation, CI, or maintenance pull request does not need an issue.
Omit `Closes #123` when no issue exists. Explain why the repository needs the
change in the pull request body.

Use a `codex/` bookmark name with the issue number and a short topic:

```sh
jj bookmark create codex/issue-123-short-topic -r @
jj git push --bookmark codex/issue-123-short-topic
```

Use the commit title as the pull request title when possible. Use this pull
request body structure:

```markdown
Closes #123

## Problem

State the user-visible problem and its effect.

## Change

- State the important behavior changes.
- State intentional limits or false-positive protections.

## Verification

- List the tests and checks that ran.
- List a local smoke test only when it supplied useful evidence.
```

Use ASD-STE100 Issue 9 structural rules as guidance for issue, commit, and pull
request text. The text does not need controlled-dictionary verification. Keep
commands, identifiers, paths, diagnostic codes, and protocol method names
exact.

Wait for required CI checks. Do not merge a pull request with failed checks,
unresolved conflicts, or known acceptance failures. When the task grants merge
authority, use a squash merge and delete the remote bookmark. Otherwise, leave
the pull request ready for review.

After a merge:

```sh
jj git fetch
jj new main
jj status
```

Confirm that `main` contains the squash merge. Confirm that `jj status` reports
no changes. Run post-merge verification when several dependent pull requests
form one release batch.

## Version and Release Process

Every user-visible batch must finish with an explicit release decision. Do not
merge a feature batch and silently leave the old package version.

While the project is below `1.0.0`, use these version rules:

- Increase the patch version for compatible bug fixes, documentation fixes,
  and internal corrections with no new public behavior.
- Increase the minor version for a new CLI command, configuration option, LSP
  capability, indexed data source, or other compatible feature.
- Increase the minor version and document migration steps for an incompatible
  change. Do not make an incompatible change without an approved design issue.

For example, a bug fix after `0.3.0` can become `0.3.1`. A batch that adds JSON
configuration and a diagnostic CLI should become `0.4.0`.

Use a dedicated release pull request after a multi-PR batch. Use
`chore: prepare vX.Y.Z` as its title. The release pull request must:

1. Update `[workspace.package].version` in `Cargo.toml`.
2. Update `Cargo.lock` through Cargo.
3. Update versioned documentation and installation examples.
4. Summarize user-visible changes and migration requirements.
5. Run the complete verification suite.
6. Confirm that the pinned `tree-sitter-bg3` tag is correct.

After the release pull request merges, create an annotated `vX.Y.Z` tag from
the merge on `main`. Push the tag and create related GitHub release notes.
Never tag a pull request commit. Do not reuse or move a published version tag.

Release artifacts and `bg3-ls --version` must report the same version. Keep the
Cargo lockfile in each release commit.

## Documentation and Compatibility

Update the README and JSON schema for every public configuration change. Keep
inline LSP options at the highest precedence, JSON configuration second, and
built-in defaults last.

Do not require a Neovim-specific plugin. Keep Neovim nightly and Neovim 0.12+
examples current. Preserve standard LSP behavior for other editors.

Document intentional omissions. Do not imply support for packed sources,
binary resources, native Windows, or automatic dependency discovery until the
implementation and tests provide that support.

## Scope Control

Do not add rename, format, semantic tokens, code actions, packed-file
extraction, or new persistence technology as incidental work. Open a design
issue when one of these features becomes necessary.

Do not push unrelated bookmarks or rewrite published history. Do not publish
crates, tags, or releases unless the active task includes that authority.
