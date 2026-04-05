#!/usr/bin/env bash
# Run MACP Tier 3 E2E tests with real LLM agents.
#
# Usage:
#   ./scripts/run-e2e.sh                    # Run all E2E tests
#   ./scripts/run-e2e.sh decision           # Run only the decision test
#   ./scripts/run-e2e.sh task               # Run only the task test
#   ./scripts/run-e2e.sh signals            # Run only the decision+signals test
#
# Prerequisites:
#   1. Set OPENAI_API_KEY in .env or export it in your shell
#   2. Rust toolchain + protoc installed

set -euo pipefail
cd "$(dirname "$0")/.."

# Load .env if present (without overriding existing env vars)
if [ -f .env ]; then
  set -a
  # shellcheck disable=SC1091
  source .env
  set +a
  echo "Loaded .env"
fi

# Check for API key
if [ -z "${OPENAI_API_KEY:-}" ]; then
  echo "ERROR: OPENAI_API_KEY is not set."
  echo ""
  echo "  Option 1: Add OPENAI_API_KEY=sk-... to .env"
  echo "  Option 2: export OPENAI_API_KEY='sk-...'"
  exit 1
fi

echo "OPENAI_API_KEY is set (${#OPENAI_API_KEY} chars)"
echo ""

# Build the runtime binary
echo "Building runtime..."
cargo build
echo ""

# Select test filter
FILTER=""
case "${1:-all}" in
  decision)  FILTER="real_llm_agents_coordinate_decision" ;;
  task)      FILTER="real_llm_agents_delegate_task" ;;
  signals)   FILTER="decision_with_signals_full_flow" ;;
  all)       FILTER="" ;;
  *)
    echo "Unknown test: $1"
    echo "Usage: $0 [decision|task|signals|all]"
    exit 1
    ;;
esac

echo "Running Tier 3 E2E tests..."
echo "═══════════════════════════════════════════════════════"
echo ""

cd integration_tests
export MACP_TEST_BINARY=../target/debug/macp-runtime
if [ -n "$FILTER" ]; then
  cargo test --test tier3 "$FILTER" -- --ignored --test-threads=1 --show-output
else
  cargo test --test tier3 -- --ignored --test-threads=1 --show-output
fi
