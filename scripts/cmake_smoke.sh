#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
FIXTURE="$ROOT/tests/fixtures/cmake_project"
COMPILER="${RNQCC:-$ROOT/target/debug/rnqcc}"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/rnqcc-cmake-smoke.XXXXXX")"

cleanup() {
    status=$?
    if [ "$status" -ne 0 ] && [ -n "${CI_ARTIFACT_DIR:-}" ]; then
        mkdir -p "$CI_ARTIFACT_DIR"
        cp -R "$WORKDIR" "$CI_ARTIFACT_DIR/cmake_smoke"
    fi
    rm -rf "$WORKDIR"
    exit "$status"
}
trap cleanup EXIT HUP INT TERM

if [ ! -x "$COMPILER" ]; then
    cargo build --locked --manifest-path "$ROOT/Cargo.toml"
fi
if [ ! -x "$COMPILER" ]; then
    echo "rnqcc compiler not found: $COMPILER" >&2
    exit 1
fi
if ! command -v cmake >/dev/null 2>&1; then
    echo "cmake is required for this smoke test" >&2
    exit 1
fi

cmake -S "$FIXTURE" -B "$WORKDIR/build" \
    -DCMAKE_C_COMPILER="$COMPILER" \
    -DCMAKE_C_COMPILER_FORCED=TRUE \
    -DCMAKE_C_COMPILER_WORKS=TRUE \
    -DCMAKE_C_ABI_COMPILED=TRUE \
    -DCMAKE_BUILD_TYPE=Release
cmake --build "$WORKDIR/build" --parallel 2
"$WORKDIR/build/cmake_smoke"
echo "cmake smoke passed"
