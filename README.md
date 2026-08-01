# rnqcc

A not-quite-C compiler targeting x86-64 and AArch64 assembly.

## Targets

Supported backend targets:

- `x86_64-macos` (`osx`, `macos`, and `x86_64-apple-darwin` are accepted aliases)
- `x86_64-linux` (`linux` and `x86_64-unknown-linux-gnu` are accepted aliases)
- `aarch64-macos` (`arm64-macos` and `aarch64-apple-darwin` are accepted aliases)
- `aarch64-linux` (`arm64-linux` and `aarch64-unknown-linux-gnu` are accepted aliases)

The frontend and TACKY IR are target-independent. Target-specific lowering and
emission live under `src/backend/x86_64` and `src/backend/aarch64`.

Cross-target assembly output is supported with `-S`. Object files and executables are
assembled/linked through the host C compiler driver, so they must target the host OS.
The driver accepts C, preprocessed C, assembly, object, static-library, and
shared-library inputs, so simple build-system link lines can mix rnqcc-compiled
sources with existing `.o`, `.a`, `.so`, and `.dylib` artifacts.

## Support Matrix

| Area | x86_64 SysV/macOS | AArch64 Linux/macOS |
| --- | --- | --- |
| `int`, `long`, signed/unsigned chars | yes | yes |
| pointers, arrays, pointer arithmetic | yes | yes |
| globals, statics, string literals | yes | yes |
| `float`/`double` arithmetic/comparison/conversion | yes | yes |
| `long double` ABI storage/calls | x87 extended | binary128 on Linux, `double` width on macOS |
| direct calls and indirect calls | yes | yes |
| stack arguments | yes | yes |
| struct copies and member access | yes | yes |
| small aggregate args/returns | yes | yes |
| large aggregate stack args/hidden returns | yes | yes |
| cross-target assembly with `-S` | yes | yes |
| object/executable output | host target only | host target only |

Known simplifications:

- `long double` arithmetic is lowered for the target ABI but still uses `f64`
  literal/static-initializer values internally today. `_Float*` names are accepted
  as compatibility aliases over the existing `float`/`double` representation.
- The external C driver handles preprocessing, assembly, and linking.
- Direct calls to variadic prototypes and indirect calls through variadic
  function pointers are supported.
- AArch64 uses register allocation for non-aliased scalar temporaries while
  retaining conservative stack placement for aggregates and address-taken values.

## Usage

```sh
cargo run -- tests/return_42.c
cargo run -- -E tests/return_42.c
cargo run -- /tmp/return_42.i
cargo run -- -S tests/return_42.c
cargo run -- -c tests/return_42.c
cargo run -- -o /tmp/return_42 tests/return_42.c
cargo run -- --target x86_64-macos tests/return_42.c
cargo run -- --cc clang tests/return_42.c
cargo run -- --print-targets
```

Useful development flags:

```sh
cargo run -- --stage lex tests/return_42.c
cargo run -- --stage parse tests/return_42.c
cargo run -- --stage tacky tests/return_42.c
cargo run -- --stage codegen tests/return_42.c
cargo run -- --keep-temps tests/return_42.c
```

`rnqcc` uses `gcc` as the external compiler driver by default for preprocessing,
assembly, and linking. Override it with `--cc` or the `CC` environment variable.
Inputs may be `.c`, `.h`, already-preprocessed `.i`, assembly `.s`/`.S`, object
`.o`/`.obj`, static library `.a`, or shared library `.so`/`.dylib` files.

For Make-style builds, `rnqcc` accepts common compiler-driver spellings such as
`-I`, `-D`, `-U`, `-MMD`, `-MP`, `-MF`, `-MT`, `-std=...`, `-O2`, `-g`,
`-pthread`, `-L`, `-l`, `-Wl,...`, `-Xlinker`, `-Xassembler`, `-F`,
`-framework`, `--sysroot`, response files, and sanitizer/linker runtime flags.
Unsupported frontend options are generally forwarded to preprocessing when that
is the least surprising behavior.

## Tests

The shell runner expects the Writing a C Compiler test suite next to this repo by default:

```sh
./run_tests.sh
```

Override locations and tool paths with environment variables:

```sh
TESTDIR=/path/to/writing-a-c-compiler-tests/tests COMPILER=target/debug/rnqcc ./run_tests.sh
REF_CC=clang ./run_tests.sh
```

The local real-project smoke runner exercises multi-file builds, dependency
side effects, static-library linking, existing object/library inputs, and nested
response files:

```sh
sh scripts/real_project_smoke.sh
```

Additional deterministic smoke harnesses live under `scripts/`:

```sh
python3 scripts/fuzz_smoke.py --seed 31337 --cases 12 --target x86_64-linux
python3 scripts/fuzz_smoke.py --seed 4242 --cases 6 --target x86_64-linux --compare-runtime
python3 scripts/layout_oracle.py
python3 scripts/gcc_torture_smoke.py --mode compile --limit 100
```

The GCC torture runner supports expected-failure and expected-skip fixtures
under `tests/fixtures/` so compiler frontiers are ratcheted instead of silently
drifting.
