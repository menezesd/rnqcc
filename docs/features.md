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
  `__has_extension(...)`, and `__has_warning(...)`
- integer `#if` expressions using unary, arithmetic, shift, comparison,
  equality, bitwise, logical, conditional, and parenthesized operators, including
  decimal, octal, hexadecimal, binary, character constants, and common integer
  suffixes
- escaped newline splicing
- `#line` and GCC-style line marker directives
- line and block comment removal
- `#error`
- `#warning`
- ignored unknown `#pragma` directives
- `#pragma once`, `_Pragma("once")`, `#pragma GCC system_header`,
  `#pragma clang system_header`, and `#pragma GCC poison`

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

- `scripts/ci.sh` runs `cargo check`, unit tests, CLI tests, a build, and the
  local real-project smoke runner.
- `scripts/real_project_smoke.sh` exercises project-shaped local builds with
  multiple translation units, dependency files, static libraries, and nested
  response files.
- `scripts/fuzz_smoke.py` generates deterministic preprocessed C programs and
  drives all public compiler stages.
- CLI tests include target-specific assembly checks, warning behavior,
  optimization equivalence, internal preprocessor behavior, and runtime smoke
  cases where the host can run the generated executable.
