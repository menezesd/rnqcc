#!/usr/bin/env sh
set -eu

# Validation should not leave interpreter bytecode in the source tree.
export PYTHONDONTWRITEBYTECODE=1

cargo fmt -- --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/fuzz_smoke.py --seed 31337 --cases 12 --rnqcc target/debug/rnqcc --target x86_64-linux --target aarch64-linux --rnqcc-arg=--optimize
RNQCC_AARCH64_REGALLOC=1 python3 scripts/fuzz_smoke.py --seed 31337 --cases 6 --rnqcc target/debug/rnqcc --target aarch64-linux --rnqcc-arg=--optimize
for opt in --licm --cse --inline-functions --ipcp; do
    python3 scripts/fuzz_smoke.py --seed 31337 --cases 6 --rnqcc target/debug/rnqcc --target x86_64-linux --rnqcc-arg="$opt"
done
python3 scripts/fuzz_smoke.py --seed 4242 --cases 6 --rnqcc target/debug/rnqcc --target x86_64-linux --compare-runtime
python3 scripts/layout_oracle.py
python3 scripts/real_project_corpus.py
python3 scripts/real_project_corpus.py --rnqcc-arg=--optimize
for opt in --licm --cse --inline-functions --ipcp; do
    python3 scripts/real_project_corpus.py --rnqcc-arg="$opt"
done
bash -n run_tests.sh

cargo build --locked
sh scripts/real_project_smoke.sh
sh scripts/cmake_smoke.sh
if git ls-files '*.i' | grep -q .; then
    git ls-files -z '*.i' |
        xargs -0 -n 1 ./target/debug/rnqcc --target aarch64-linux -S \
            -o /tmp/rnqcc-aarch64-ci.s
fi
find tests -maxdepth 1 -name '*.c' -type f -exec \
    ./target/debug/rnqcc --target aarch64-linux -S -o /tmp/rnqcc-aarch64-ci.s {} \;

if [ -d "${TESTDIR:-../writing-a-c-compiler-tests/tests}" ]; then
    ./run_tests.sh 1 2 3 4 5 6 7 8 9 10
fi
