#!/usr/bin/env sh
set -eu

cargo fmt -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
python3 scripts/fuzz_smoke.py --seed 31337 --cases 12 --rnqcc target/debug/rnqcc --target x86_64-linux --target aarch64-linux --rnqcc-arg=--optimize
python3 scripts/layout_oracle.py
python3 scripts/real_project_corpus.py
python3 scripts/real_project_corpus.py --rnqcc-arg=--optimize
bash -n run_tests.sh

cargo build
sh scripts/real_project_smoke.sh
for src in $(git ls-files '*.i') $(find tests -maxdepth 1 -name '*.c' -type f | sort); do
    echo "aarch64 smoke: $src"
    ./target/debug/rnqcc --target aarch64-linux -S "$src" -o /tmp/rnqcc-aarch64-ci.s
done

if [ -d "${TESTDIR:-../writing-a-c-compiler-tests/tests}" ]; then
    ./run_tests.sh 1 2 3 4 5 6 7 8 9 10
fi
