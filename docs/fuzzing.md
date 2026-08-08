# Fuzz Smoke Harness

`scripts/fuzz_smoke.py` is a deterministic, lightweight black-box harness for
rnqcc. It does not use external fuzzing tools and does not call compiler
internals. It generates small preprocessed C inputs from a seed, then invokes the
`rnqcc` command-line interface through the public stages:

- `--stage lex`
- `--stage parse`
- `--stage validate`
- `--stage tacky`
- `--stage codegen`
- `-S` assembly emission

Run it after building rnqcc:

```sh
cargo build --locked
python3 scripts/fuzz_smoke.py --seed 1234 --cases 16
```

Useful options:

```sh
python3 scripts/fuzz_smoke.py --seed 1234 --cases 1 --keep-successes
python3 scripts/fuzz_smoke.py --seed 1234 --cases 4 --target x86_64-linux
python3 scripts/fuzz_smoke.py --seed 1234 --cases 4 --target x86_64-linux --compare-runtime
python3 scripts/fuzz_smoke.py --seed 1234 --emit-only --work-dir /tmp/rnqcc-inputs
RNQCC=/path/to/rnqcc python3 scripts/fuzz_smoke.py --seed 1234
```

`--compare-runtime` builds each generated input with both rnqcc and the
reference C compiler, runs both executables, and compares exit statuses. The
reference compiler defaults to `cc` and can be overridden with `--cc` or `CC`.
Runtime comparison is host-target only, so pass a runnable host target such as
`x86_64-linux` on Linux or the default target on the host.

On failure, the harness reports the seed, case number, source path, failed
command, and captured output. Re-run the same seed and case count to reproduce
the generated input.
