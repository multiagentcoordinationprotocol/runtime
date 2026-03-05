.PHONY: setup build test fmt clippy check

## First-time setup: configure git hooks
setup:
	git config core.hooksPath .githooks
	@echo "Git hooks configured."

build:
	cargo build

test:
	cargo test

fmt:
	cargo fmt --all

clippy:
	cargo clippy --all-targets -- -D warnings

check: fmt clippy test
