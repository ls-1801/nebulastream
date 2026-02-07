#!/usr/bin/env bash
set -euo pipefail

# Quality gate script - runs all checks, only outputs on error.
# Usage: ./quality_gate.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

FAILED=0

run_step() {
    local name="$1"
    shift
    local output
    if output=$("$@" 2>&1); then
        printf "${GREEN}✓${NC} %s\n" "$name"
    else
        printf "${RED}✗${NC} %s\n" "$name"
        echo "$output"
        FAILED=1
    fi
}

run_systest() {
    local name="$1"
    local test_file="$2"
    run_step "systest: $name" ./devc timeout 60 \
        /workspace/nes/build/nes-systests/systest/systest \
        --sequential \
        -t "/workspace/nes/nes-systests/$test_file" \
        -- \
        --worker.default_query_execution.execution_mode=INTERPRETER \
        --worker.default_query_execution.join_strategy=NESTED_LOOP_JOIN
}

# --- Rust checks ---
run_step "cargo check" cargo check --manifest-path nes-adaptive-engine/Cargo.toml
run_step "cargo clippy" cargo clippy --manifest-path nes-adaptive-engine/Cargo.toml -- -D warnings
run_step "cargo fmt" cargo fmt --manifest-path nes-adaptive-engine/Cargo.toml --check

# --- Standalone QueryEngineTest ---
run_step "cmake configure" cmake --preset debug -S nes-adaptive-engine/cpp/
run_step "cmake build" cmake --build nes-adaptive-engine/build-ae
run_step "QueryEngineTest (standalone)" ctest --test-dir nes-adaptive-engine/build-ae --output-on-failure

# --- NES build + systest ---
run_step "NES build (systest target)" ./devc cmake --build /workspace/nes/build --target systest -j8

# Run NES QueryEngineTest
run_step "QueryEngineTest (NES)" ./devc timeout 120 ctest --test-dir /workspace/nes/build -R QueryEngineTest --output-on-failure --timeout 30

# Run individual systest files (each in its own engine instance)
run_systest "OneTuple" "tuples/OneTuple.test"
run_systest "FiveTuples" "tuples/FiveTuples.test"
run_systest "SourceWithoutTuples" "tuples/SourceWithoutTuples.test"
run_systest "TuplesDontFitIntoOneBuffer" "tuples/TuplesDontFitIntoOneBuffer.test"
run_systest "TupleLargerThanBuffer" "tuples/TupleLargerThanBuffer.test"

# Run full systest (all non-large queries in one engine instance)
run_step "systest (full)" ./devc timeout 60 \
    /workspace/nes/build/nes-systests/systest/systest \
    --exclude-groups large \
    -- \
    --worker.default_query_execution.execution_mode=INTERPRETER \
    --worker.default_query_execution.join_strategy=NESTED_LOOP_JOIN

if [ "$FAILED" -ne 0 ]; then
    printf "\n${RED}Quality gate FAILED${NC}\n"
    exit 1
else
    printf "\n${GREEN}Quality gate PASSED${NC}\n"
fi
