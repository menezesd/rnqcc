#!/usr/bin/env python3
"""Compare selected rnqcc aggregate layouts against the host C compiler."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RNQCC = Path(os.environ.get("RNQCC", ROOT / "target" / "debug" / "rnqcc"))
CC = os.environ.get("CC", "cc")
DEFAULT_TIMEOUT = float(os.environ.get("LAYOUT_ORACLE_TIMEOUT", "30.0"))


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
]


def timeout_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value


def run(cmd: list[str], timeout: float = DEFAULT_TIMEOUT) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            cmd,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        return subprocess.CompletedProcess(
            cmd,
            124,
            stdout=timeout_text(exc.stdout),
            stderr=(timeout_text(exc.stderr) + f"\ntimed out after {timeout:.1f}s").lstrip(),
        )


def build_rnqcc() -> None:
    if RNQCC.exists():
        return
    result = run(["cargo", "build", "--manifest-path", str(ROOT / "Cargo.toml")])
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)


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
    if DEFAULT_TIMEOUT <= 0:
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
