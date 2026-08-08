#!/usr/bin/env python3
"""Compare selected rnqcc aggregate layouts against the host C compiler."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    from smoke_utils import env_timeout, is_positive_finite, run_with_timeout
except ModuleNotFoundError as err:
    if err.name != "smoke_utils":
        raise
    from scripts.smoke_utils import env_timeout, is_positive_finite, run_with_timeout


ROOT = Path(__file__).resolve().parents[1]
RNQCC = Path(os.environ.get("RNQCC", ROOT / "target" / "debug" / "rnqcc"))
CC = os.environ.get("CC", "cc")
DEFAULT_TIMEOUT = env_timeout("LAYOUT_ORACLE_TIMEOUT", "30.0")


CASES: list[tuple[str, str]] = [
    (
        "natural",
        """
        struct S { char c; int i; long l; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, i) +
                    __builtin_offsetof(struct S, l)) & 255;
        }
        """,
    ),
    (
        "packed",
        """
        struct __attribute__((packed)) S { char c; int i; long l; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, i) +
                    __builtin_offsetof(struct S, l)) & 255;
        }
        """,
    ),
    (
        "field-packed",
        """
        struct S { char c; int i __attribute__((packed)); char tail; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, i) +
                    __builtin_offsetof(struct S, tail)) & 255;
        }
        """,
    ),
    (
        "aggregate-aligned",
        """
        struct __attribute__((aligned(16))) S { char c; int i; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, i)) & 255;
        }
        """,
    ),
    (
        "packed-aligned",
        """
        struct __attribute__((packed, aligned(4))) S { char c; int i; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, i)) & 255;
        }
        """,
    ),
    (
        "nested",
        """
        struct Inner { char c; int i; };
        struct __attribute__((packed)) Outer { char tag; struct Inner inner; char tail; };
        int main(void) {
            return (sizeof(struct Outer) + _Alignof(struct Outer) +
                    __builtin_offsetof(struct Outer, inner) +
                    __builtin_offsetof(struct Outer, tail)) & 255;
        }
        """,
    ),
    (
        "union-packed-aligned",
        """
        union __attribute__((packed, aligned(8))) U { char c; int i; long l; };
        int main(void) {
            return (sizeof(union U) + _Alignof(union U)) & 255;
        }
        """,
    ),
    (
        "bitfields-with-tail",
        """
        struct S { unsigned a:3; unsigned b:5; unsigned c:9; char tail; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, tail)) & 255;
        }
        """,
    ),
    (
        "nested-union-array",
        """
        union U { char c; double d; };
        struct S { char tag; union U slots[2]; int tail; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, slots) +
                    __builtin_offsetof(struct S, tail)) & 255;
        }
        """,
    ),
    (
        "long-double-field",
        """
        struct S { char c; long double ld; char tail; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, ld) +
                    __builtin_offsetof(struct S, tail)) & 255;
        }
        """,
    ),
    (
        "transparent-union-layout",
        """
        union __attribute__((transparent_union)) U { int i; long l; };
        struct S { char c; union U u; char tail; };
        int main(void) {
            return (sizeof(struct S) + _Alignof(struct S) +
                    __builtin_offsetof(struct S, u) +
                    __builtin_offsetof(struct S, tail)) & 255;
        }
        """,
    ),
]


def run(cmd: list[str], timeout: float = DEFAULT_TIMEOUT) -> subprocess.CompletedProcess[str]:
    return run_with_timeout(cmd, timeout=timeout)


def build_rnqcc() -> None:
    if RNQCC.is_file():
        return
    if RNQCC.exists():
        raise SystemExit(f"RNQCC path is not a file: {RNQCC}")
    try:
        result = run(["cargo", "build", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")])
    except OSError as exc:
        raise SystemExit(f"could not build rnqcc: {exc}") from exc
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    if not RNQCC.is_file():
        raise SystemExit(f"RNQCC not found after build: {RNQCC}")


def compile_and_run(compiler: list[str], source: Path, output: Path) -> int:
    result = run([*compiler, str(source), "-o", str(output)])
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    result = run([str(output)])
    if result.returncode == 124 and "timed out after" in result.stderr:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    return result.returncode


def main() -> int:
    if not is_positive_finite(DEFAULT_TIMEOUT):
        print("LAYOUT_ORACLE_TIMEOUT must be positive", file=sys.stderr)
        return 1
    build_rnqcc()
    with tempfile.TemporaryDirectory(prefix="rnqcc-layout-oracle.") as tmp:
        tmpdir = Path(tmp)
        for name, body in CASES:
            source = tmpdir / f"{name}.c"
            source.write_text(body, encoding="utf-8")
            host_status = compile_and_run([CC], source, tmpdir / f"{name}.host")
            rnqcc_status = compile_and_run([str(RNQCC)], source, tmpdir / f"{name}.rnqcc")
            if host_status != rnqcc_status:
                print(
                    f"{name}: layout mismatch, host returned {host_status}, "
                    f"rnqcc returned {rnqcc_status}",
                    file=sys.stderr,
                )
                return 1
    print("layout oracle passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
