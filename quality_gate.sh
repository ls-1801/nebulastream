#!/usr/bin/env bash

# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at

#    https://www.apache.org/licenses/LICENSE-2.0

# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

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
    run_step "systest: $name" timeout 60 \
        "$SCRIPT_DIR/build-debug/nes-systests/systest/systest" \
        --sequential \
        -t "$SCRIPT_DIR/nes-systests/$test_file" \
        -- \
        --worker.default_query_execution.execution_mode=INTERPRETER \
        --worker.default_query_execution.join_strategy=NESTED_LOOP_JOIN
}

# --- Rust checks ---
run_step "cargo check" cargo check --manifest-path nes-adaptive-engine/Cargo.toml
run_step "cargo clippy" cargo clippy --manifest-path nes-adaptive-engine/Cargo.toml -- -D warnings
run_step "cargo fmt" cargo fmt --manifest-path nes-adaptive-engine/Cargo.toml --check
run_step "cargo test" cargo test --manifest-path nes-adaptive-engine/Cargo.toml

# --- NES build + tests ---
run_step "NES configure" cmake --preset debug
run_step "NES build" cmake --build --preset debug --target systest --target QueryEngineTest -j$(nproc)
run_step "clang-format" cmake --build --preset debug --target format -j$(nproc)

# Run NES QueryEngineTest
run_step "QueryEngineTest (NES)" timeout 120 ctest --test-dir "$SCRIPT_DIR/build-debug" -R QueryEngineTest --output-on-failure --timeout 30

# Run individual systest files (each in its own engine instance)
run_systest "OneTuple" "tuples/OneTuple.test"
run_systest "FiveTuples" "tuples/FiveTuples.test"
run_systest "SourceWithoutTuples" "tuples/SourceWithoutTuples.test"
run_systest "TuplesDontFitIntoOneBuffer" "tuples/TuplesDontFitIntoOneBuffer.test"
run_systest "TupleLargerThanBuffer" "tuples/TupleLargerThanBuffer.test"

# Run full systest (all non-large queries in one engine instance)
run_step "systest (full)" timeout 60 \
    "$SCRIPT_DIR/build-debug/nes-systests/systest/systest" \
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
