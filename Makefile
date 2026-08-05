.PHONY: build check test test-lsp

build:
	cargo build --workspace

check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

test-lsp: build
	BG3_LS_ROOT="$(CURDIR)" \
	BG3_LS_TREE_SITTER="$(abspath ../tree-sitter-bg3)" \
	BG3_LS_TEST_CACHE="$$(mktemp -d)" \
	nvim --headless -u test/minimal_init.lua -l test/navigation_spec.lua
