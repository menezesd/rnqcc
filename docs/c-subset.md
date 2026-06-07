# rnqcc C subset

`rnqcc` is a small C compiler, not a complete hosted C implementation. This file
tracks the implemented language and ABI surface so tests and backend work have a
single target.

## Supported

- integer scalars and decimal/hex/octal/GNU binary integer literals: `char`, `short`,
  `int`, `long`, signed and unsigned variants; `long long` suffixes are
  accepted as aliases for the 64-bit `long` model
- decimal and hexadecimal floating literals for `float`/`double` expressions
- `_Bool`, pointers, arrays, inferred first dimensions for initialized arrays,
  string literals with common C byte escapes, and pointer arithmetic
- `_Alignas` / `alignas` on objects and struct members; `_Alignof` / `alignof`
  and GNU `__alignof__` for type names and expressions
- local variables, globals, static storage, and static local variables; static
  pointer initializers may reference named objects, arrays, functions, and
  string literals
- `if`, `while`, `do`, `for`, `break`, `continue`, `goto`, labels, and `switch`
- C11 `_Generic` selections resolved by the frontend for ordinary scalar,
  pointer, array-decayed, function-decayed, and aggregate expression types
- structs and unions, including copies, member access, nested aggregates,
  anonymous aggregate members, aggregate definitions with declarators, and
  aggregate arguments/returns; trailing flexible array members are supported in
  struct layout
- C99 compound literals for automatic objects and static aggregate/scalar
  initializers
- integer bit-fields that fit within their declared storage unit, including
  signed reads/writes, zero-width alignment fields, and host-checked mixed
  storage-unit layout cases
- function prototypes, direct calls, indirect calls, function designator decay, and
  direct calls to variadic prototypes
- `double` arithmetic, comparisons, conversions, arrays, arguments, and returns
- preprocessing through either the configured external C driver or the
  self-contained `--internal-cpp` preprocessor for local fixtures
- assembly and linking through the configured external C driver, including
  mixed source/object/static-library/shared-library link lines

## Target Backends

- `x86_64-linux`
- `x86_64-macos`
- `aarch64-linux`
- `aarch64-macos`

Cross-target assembly output is supported with `-S`. Object and executable output
goes through the host C compiler driver, so the target must match the host OS for
assembly/linking.

## Accepted With Simplifications

- `float` has distinct 4-byte layout and single-precision arithmetic/conversion
  lowering. `long double` and `_Float*` type names are still accepted as
  compatibility aliases for the compiler's `double` representation.
- `_Atomic(...)`, `_Atomic` qualifiers, and `static_assert` / `_Static_assert`
  are accepted for hosted-header compatibility. `_Thread_local`/`__thread`
  storage is tracked distinctly and lowers to ELF local-exec TLS sections and
  access sequences on Linux targets, and to Mach-O TLV sections/descriptors and
  access sequences on Darwin targets.
- Common GNU/Clang compatibility constructs are accepted where they can be
  folded or ignored safely: `__builtin_expect`, `__builtin_choose_expr` with
  integer constant conditions, `__builtin_types_compatible_p` for scalar,
  struct/union, pointer, function, and fixed/unsized array type names,
  `__builtin_offsetof` for struct/union member designators,
  `__builtin_unreachable`, `__builtin_trap`, `__attribute__`, `__declspec`,
  simple `asm(...)` annotations, and C++-style `[[...]]` attributes.
- GNU statement expressions of the form `({ ... expr; })` are supported for
  scalar and aggregate-side-effect macros; the value is the final expression
  statement, or `void` when the block does not end in an expression statement.
- `_Noreturn`, C23/C++-style `[[noreturn]]`, GNU
  `__attribute__((noreturn))`, and MSVC `__declspec(noreturn)` are tracked on
  direct function declarations for missing-return and unreachable-statement
  analysis.
- GNU `__attribute__((aligned(n)))` and MSVC `__declspec(align(n))` alignment
  annotations are honored for object declarations and struct/union members when
  `n` is an integer constant expression.
- GNU `__attribute__((packed))` / `__attribute__((__packed__))` is honored on
  whole struct/union definitions and individual members for layout, including
  dense member placement, aggregate alignment 1, non-recursive nested struct
  layout, and zero-width bit-field alignment barriers. GNU
  `__attribute__((aligned(n)))` on whole struct/union definitions raises the
  aggregate alignment and size padding, including in combination with `packed`.
- Additional GNU builtin compatibility includes `__builtin_constant_p`,
  `__builtin_expect_with_probability`, `__builtin_assume_aligned`,
  `__builtin_prefetch`, `__builtin_bswap32`, `__builtin_bswap64`,
  `__builtin_object_size`, `__builtin_dynamic_object_size`, and libc-style
  aliases such as `__builtin_memcpy`, `__builtin_memmove`,
  `__builtin_memset`, `__builtin_memcmp`, `__builtin_strlen`, and
  `__builtin_strcmp`, plus common standard lookup helpers such as
  `__builtin_memchr`, `__builtin_strchr`, and `__builtin_strstr`.
  Common fortified libc builtins such as
  `__builtin___memcpy_chk`, `__builtin___memset_chk`, and string `_chk`
  variants lower to the corresponding libc operation.
- GNU `typeof` / `__typeof__` and C23-style `typeof_unqual` /
  `__typeof_unqual__` are accepted for type-name and expression operands; rnqcc
  does not model C qualifiers, so the unqualified form shares the same internal
  representation as `typeof`.
- C23 `_BitInt(N)` is accepted only for widths that map exactly to existing
  storage and lowering paths: signed/unsigned 32, 64, and 128 bits. Other
  widths, duplicate `_BitInt` specifiers, and combinations with another
  arithmetic type specifier are rejected instead of being approximated with a
  wider standard integer type.
- Common `__atomic_*_fetch`, `__atomic_fetch_*`, `__sync_*_and_fetch`,
  `__sync_fetch_and_*`, `__atomic_load_n`, `__atomic_store_n`,
  `__atomic_exchange_n`,
  `__atomic_compare_exchange_n`, `__sync_bool_compare_and_swap`, and
  `__sync_val_compare_and_swap` builtins have backend ordering support for
  scalar objects: load/store forms lower to ordinary accesses preceded by a
  full memory fence, while integer fetch read-modify-write forms, including
  GCC's `nand` variants, lower to
  locked x86-64 instructions or AArch64 acquire/release exclusive loops and
  preserve the requested old-value vs new-value result convention.
  Integer and pointer exchange lowers to x86-64 `xchg` or an AArch64
  acquire/release exclusive loop; compare-exchange updates `*expected` on
  failure, and legacy sync compare-and-swap supports both bool and old-value
  return forms.
  `__atomic_thread_fence`, `__atomic_signal_fence`, and `__sync_synchronize`
  emit full backend memory fences.
- GCC-style builtin floating names are parsed as compatibility aliases, not as
  separate IEEE formats.
- C99 `_Complex` / GNU `__complex__` type specifiers are parsed as compatibility
  modifiers over the existing `float`/`double` representation for header
  declarations; complex arithmetic and ABI-accurate complex object layout are
  not implemented.
- `__builtin_va_list` and `__gnuc_va_list` are recognized as pointer-like typedefs
  so common preprocessed headers can be parsed farther.
  `stdarg.h` exposes `va_start`, `va_end`, `va_copy`, and `va_arg` compatibility
  macros for compile probes; receiving variadic arguments inside rnqcc-compiled
  functions is still not ABI-complete.
- The internal preprocessor handles local and `-I` includes,
  object/function/variadic macros, stringification, token pasting,
  `__FILE__`, `__LINE__`, stateful/source builtins, common predefined ABI macros,
  `#undef`, conditional directives including `#elifdef`/`#elifndef`,
  `defined`, `__has_include`, `__has_attribute` / `__has_warning` probes for
  common GNU/Clang compatibility attributes and warnings, richer integer `#if` expressions, comments,
  continued lines, `#line`, GCC line markers, `#error`, `#warning`, ignored
  pragmas, `#pragma once`, and common `#pragma pack` push/pop alignments
  for subsequent struct/union definitions. Angle includes search user include paths,
  include-related environment variables, and common system include directories;
  macro-expanded includes, `#include_next`, virtual compatibility headers
  including `alloca.h`, `assert.h`, `arpa/inet.h`, `ctype.h`, `dirent.h`, `dlfcn.h`,
  `errno.h`, `fcntl.h`, `fnmatch.h`, `getopt.h`, `glob.h`, `grp.h`, `ifaddrs.h`,
  `iso646.h`, `libgen.h`, `limits.h`, `linux/limits.h`, `locale.h`, `malloc.h`, `math.h`, `memory.h`, `net/if.h`, `netdb.h`, `netinet/in.h`,
  `netinet/ip.h`, `netinet/tcp.h`, `netinet/udp.h`, `paths.h`, `poll.h`, `pthread.h`, `pwd.h`, `regex.h`, `resolv.h`, `setjmp.h`, `signal.h`,
  `stdalign.h`, `stdatomic.h`,
  `stdnoreturn.h`, `stdio.h`, `stdlib.h`, `string.h`, `strings.h`,
  `sysexits.h`, `sys/errno.h`, `sys/file.h`, `sys/ioctl.h`, `sys/mman.h`, `sys/param.h`, `sys/poll.h`, `sys/resource.h`, `sys/select.h`,
  `sys/socket.h`, `sys/stat.h`, `sys/sysmacros.h`, `sys/time.h`, `sys/types.h`, `sys/uio.h`,
  `sys/un.h`, `sys/utsname.h`, `sys/wait.h`, `syslog.h`, `termios.h`, `time.h`, `unistd.h`, `utime.h`,
  `wchar.h`, `wctype.h`, and common C library typedef/macro headers, `-D`, `-U`,
  `-iquote`, `-isystem`,
  `-idirafter`, and `-nostdinc` are supported.

## Unsupported Or Incomplete

- Full source spans are not carried through every diagnostic yet.
- The internal preprocessor intentionally does not exactly mirror every hosted
  compiler predefined macro, compiler-specific header directive, or macro
  expansion corner case yet.
- Full arbitrary-width `_BitInt(N)` needs a real bit-precise integer type in the
  rich type representation, usual arithmetic conversions that preserve precision
  and signedness, constant folding with unsigned modulo behavior at result
  widths, cast/assignment truncation and sign extension, TACKY value metadata,
  static initializer handling, and backend lowering for non-native widths.
- AArch64 code generation favors correctness over register allocation quality and
  currently emits stack-heavy assembly.

## Real Project Compatibility Work

The checked-in `scripts/real_project_smoke.sh` runner is the compatibility
ratchet for project-shaped builds. It covers separate compilation, `-MMD -MP`
dependency files, existing object/static-library inputs, nested response files,
and paths containing spaces. New failures from real C projects should be reduced
into fixtures under `tests/fixtures/smoke` or configure-style probes under
`tests/fixtures/real_project` before broadening the compiler.
`scripts/layout_oracle.py` additionally compares selected aggregate layout
checks against the host C compiler so packing/alignment changes are
regression-tested against the platform ABI.
`scripts/real_project_corpus.py` compiles a manifest of representative
translation units with `--internal-cpp` and supports expected-failure entries so
external project reductions can be tracked as a compatibility ratchet.
`scripts/gcc_torture_smoke.py` exercises deterministic GCC C torture slices.
Known failures and skips are tracked separately in
`tests/fixtures/gcc_torture_expected_failures.txt` and
`tests/fixtures/gcc_torture_expected_skips.txt`; both fixtures should shrink as
the compiler and internal preprocessor improve.
