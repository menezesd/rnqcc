# rnqcc feature matrix

## Frontend

- Lexer and parser return structured `Result` errors rather than panicking.
- Diagnostics are rendered through phase-specific parse, resolve, and TACKY
  diagnostics.
- Focused unit tests cover lexing, parsing errors, resolver warnings, TACKY
  lowering, backend codegen, and emitter behavior.

## Preprocessor

The default mode still delegates preprocessing to the configured external C
driver. `--internal-cpp` is intended for self-contained sources and local
fixtures.

The internal preprocessor supports:

- object-like macros
- function-like macros
- variadic macros with `__VA_ARGS__`
- `__VA_OPT__(...)`
- GNU comma elision for empty `, ##__VA_ARGS__`
- stringification with `#`
- token pasting with `##`
- recursive macro expansion with simple self-recursion protection
- `__FILE__`, `__LINE__`, standard C feature macros, and target-specific
  architecture/OS macros
- stateful/source macros including `__COUNTER__`, `__BASE_FILE__`,
  `__INCLUDE_LEVEL__`, `__DATE__`, and `__TIME__`
- `#undef`
- quoted local `#include`
- `<...>` includes through `-I`, `CPATH`, `C_INCLUDE_PATH`, and common system
  include directories
- `-iquote`, `-isystem`, `-idirafter`, and `-nostdinc` include controls
- command-line macro `-D` and `-U`
- macro-expanded include operands
- `#include_next`
- `#if`, `#ifdef`, `#ifndef`, `#elif`, `#else`, and `#endif`
- `#elifdef` and `#elifndef`
- `defined NAME` and `defined(NAME)`
- `__has_include(...)` and `__has_include_next(...)`
- `__has_builtin(...)`, `__has_attribute(...)`, `__has_c_attribute(...)`,
  `__has_declspec_attribute(...)`, `__has_feature(...)`,
  `__has_extension(...)`, `__has_warning(...)`, and `__is_identifier(...)`,
  including common GNU/Clang
  attribute probes such as `nonnull`, `warn_unused_result`, `returns_nonnull`,
  `noinline`, `pure`, `const`, `malloc`, `cold`, `hot`, `weak`, `used`,
  `section`, `gnu_inline`, `alloc_size`, `alloc_align`, `format_arg`,
  `unavailable`, accepted double-underscore aliases such as `align`,
  `__align__`, and `__deprecated__`, plus `alias`, `mode`, `vector_size`,
  `transparent_union`,
  `no_instrument_function`, and `scalar_storage_order`, standard C attribute
  probes such as `nodiscard`, `maybe_unused`, `reproducible`, and
  `unsequenced`, scoped C attribute probes such
  as `gnu::unused`, `__gnu__::__unused__`, `__gcc__::__unused__`,
  `gcc::unused`, `clang::fallthrough`, and `__clang__::__fallthrough__`,
  declspec probes such as `align` and `deprecated`,
  feature probes for implemented C features such as variadic macros, `_Generic`,
  `_Generic` controlling type operands, supported `_BitInt` widths, and
  compatibility probes for
  attribute messages and no-op nullability annotations; feature and declspec
  probes also accept double-underscore aliases such as `__c_static_assert__`
  and `__dllexport__`, while `__is_identifier` rejects parser-reserved
  extension type names,
  plus supported GNU builtin probes including byte-swap, bit-count, checked
  arithmetic, floating
  classification/infinity, address introspection, stdarg, atomic, vector,
  allocation, formatted-output, floating math aliases, fortified libc, and
  string/memory helpers; accepted warning probes include active diagnostics and
  compatibility switches such as `-Wextra` and `-Wpedantic`
- integer `#if` expressions using unary, arithmetic, shift, comparison,
  equality, bitwise, logical, conditional, and parenthesized operators, including
  decimal, octal, hexadecimal, binary, C23 digit separators, character constants, and common integer
  suffixes
- escaped newline splicing
- `#line` and GCC-style line marker directives
- line and block comment removal
- `#error`
- `#warning`
- ignored unknown `#pragma` directives
- `#pragma once`, `_Pragma("once")`, `#pragma GCC system_header`,
  `#pragma clang system_header`, `#pragma GCC poison`, and
  common `#pragma pack` push/pop alignments for subsequent struct/union
  definitions

Known internal preprocessor gaps:

- exact host compiler predefined macro parity
- compiler-specific system header extensions beyond `#include_next`
- macro expansion corner cases around disabled/re-enabled macro identifiers
- full source-map propagation from preprocessing tokens into later diagnostics

## Targets

- `x86_64-linux`
- `x86_64-macos`
- `aarch64-linux`
- `aarch64-macos`

Cross-target assembly is supported. Cross-target object files and executables
are intentionally rejected unless the host toolchain can assemble and link for
that target.

## Verification

- `scripts/ci.sh` runs formatting, `cargo check`, Clippy, the Rust and CLI test
  suites, deterministic fuzzing, layout and real-project corpus checks,
  cross-target assembly smoke tests, the local real-project runner, and the
  optional external Writing a C Compiler suite when it is available.
- `scripts/real_project_smoke.sh` exercises project-shaped local builds with
  multiple translation units, dependency files, static libraries, and nested
  response files.
- `scripts/fuzz_smoke.py` generates deterministic preprocessed C programs and
  drives all public compiler stages.
- CLI tests include target-specific assembly checks, warning behavior,
  optimization equivalence, internal preprocessor behavior, and runtime smoke
  cases where the host can run the generated executable.
