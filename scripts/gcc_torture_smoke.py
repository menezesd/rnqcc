#!/usr/bin/env python3
"""Run a bounded GCC C torture smoke test against rnqcc.

This is intentionally not a DejaGnu replacement.  It gives us a repeatable
frontier against the GCC torture corpus by compiling a deterministic subset
and, for execute tests, checking that generated programs exit successfully.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RNQCC = ROOT / "target" / "debug" / "rnqcc"
DEFAULT_SUITE = Path("/tmp/rnqcc-gcc-torture/gcc/testsuite/gcc.c-torture")


def run(cmd: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
    )


def tests_for_mode(suite: Path, mode: str) -> list[Path]:
    subdir = suite / mode
    if not subdir.is_dir():
        raise SystemExit(f"{subdir}: not found")
    return sorted(subdir.glob("*.c"))


def short_failure(result: subprocess.CompletedProcess[str]) -> str:
    text = (result.stderr or result.stdout).strip()
    if not text:
        return f"exit status {result.returncode}"
    first = text.splitlines()[0]
    return first[:240]


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run a bounded rnqcc smoke pass over GCC C torture tests."
    )
    parser.add_argument("--rnqcc", default=str(DEFAULT_RNQCC))
    parser.add_argument("--suite", default=str(DEFAULT_SUITE))
    parser.add_argument(
        "--mode",
        choices=["execute", "compile"],
        default="execute",
        help="gcc.c-torture subdirectory to exercise",
    )
    parser.add_argument("--limit", type=int, default=50)
    parser.add_argument("--start", type=int, default=0)
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument(
        "--internal-cpp",
        action="store_true",
        help="use rnqcc's internal preprocessor instead of the host preprocessor",
    )
    args = parser.parse_args()

    rnqcc = Path(args.rnqcc)
    suite = Path(args.suite)
    if not rnqcc.exists():
        subprocess.run(["cargo", "build"], cwd=ROOT, check=True)

    tests = tests_for_mode(suite, args.mode)
    selected = tests[args.start : args.start + args.limit]
    if not selected:
        raise SystemExit("no tests selected")

    passed = 0
    failures: list[tuple[Path, str]] = []
    with tempfile.TemporaryDirectory(prefix="rnqcc-gcc-torture.") as tmp:
        tmpdir = Path(tmp)
        for idx, src in enumerate(selected, start=args.start):
            stem = f"{idx:04d}-{src.stem}"
            common = [str(rnqcc), "--Wno-missing-return"]
            if args.internal_cpp:
                common.append("--internal-cpp")

            if args.mode == "execute":
                exe = tmpdir / stem
                result = run([*common, str(src), "-o", str(exe)], args.timeout)
                if result.returncode == 0:
                    result = run([str(exe)], args.timeout)
                if result.returncode == 0:
                    passed += 1
                else:
                    failures.append((src, short_failure(result)))
            else:
                obj = tmpdir / f"{stem}.o"
                result = run([*common, "-c", str(src), "-o", str(obj)], args.timeout)
                if result.returncode == 0:
                    passed += 1
                else:
                    failures.append((src, short_failure(result)))

    print(
        f"gcc torture {args.mode}: {passed}/{len(selected)} passed "
        f"(start={args.start}, limit={args.limit})"
    )
    for src, reason in failures[:20]:
        rel = src.relative_to(suite)
        print(f"FAIL {rel}: {reason}")
    if len(failures) > 20:
        print(f"... {len(failures) - 20} more failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
