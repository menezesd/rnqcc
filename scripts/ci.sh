#!/usr/bin/env sh
set -eu

cargo fmt -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
python3 scripts/layout_oracle.py
bash -n run_tests.sh

cargo build
sh scripts/real_project_smoke.sh
for src in ./*.i ./tests/*.c; do
    ./target/debug/rnqcc --target aarch64-linux -S "$src" -o /tmp/rnqcc-aarch64-ci.s
done

if [ -d "${TESTDIR:-../writing-a-c-compiler-tests/tests}" ]; then
    ./run_tests.sh 1 2 3 4 5 6 7 8 9 10
fi
