.PHONY: setup build test test-integration test-conformance test-all fmt clippy check audit coverage sync-protos sync-protos-local check-protos

SPEC_PROTO_DIR := ../multiagentcoordinationprotocol/schemas/proto
PROTO_FILES := macp/v1/envelope.proto macp/v1/core.proto macp/modes/decision/v1/decision.proto macp/modes/proposal/v1/proposal.proto macp/modes/task/v1/task.proto macp/modes/handoff/v1/handoff.proto macp/modes/quorum/v1/quorum.proto

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

test-integration:
	cargo test --test '*'

test-conformance:
	cargo test conformance

test-all: fmt clippy test test-integration test-conformance

coverage:
	cargo tarpaulin --all-targets --out html

audit:
	cargo audit

check: fmt clippy test

## Pull latest proto files from BSR
sync-protos:
	buf export buf.build/multiagentcoordinationprotocol/macp -o proto
	@echo "Done. Run 'git diff proto/' to review changes."

## Sync from local sibling checkout (for development before BSR publish)
sync-protos-local:
	@if [ ! -d "$(SPEC_PROTO_DIR)" ]; then \
		echo "Error: Spec repo not found at $(SPEC_PROTO_DIR)"; \
		echo "Use 'make sync-protos' to sync from BSR instead."; \
		exit 1; \
	fi
	@for f in $(PROTO_FILES); do \
		mkdir -p proto/$$(dirname $$f); \
		cp "$(SPEC_PROTO_DIR)/$$f" "proto/$$f"; \
		echo "  Copied $$f"; \
	done
	@echo "Done. Run 'git diff proto/' to review changes."

## Check if local protos match BSR
check-protos:
	@TMPDIR=$$(mktemp -d); \
	buf export buf.build/multiagentcoordinationprotocol/macp -o "$$TMPDIR"; \
	DRIFT=0; \
	for f in $(PROTO_FILES); do \
		if ! diff -q "$$TMPDIR/$$f" "proto/$$f" > /dev/null 2>&1; then \
			echo "DRIFT: $$f"; \
			DRIFT=1; \
		fi; \
	done; \
	rm -rf "$$TMPDIR"; \
	if [ "$$DRIFT" -eq 0 ]; then \
		echo "All proto files match BSR."; \
	else \
		echo "Run 'make sync-protos' to update."; \
		exit 1; \
	fi
