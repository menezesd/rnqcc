#!/usr/bin/env python3
"""Run LLVM SingleSource/Regression/C smoke tests against rnqcc."""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RNQCC = ROOT / "target" / "debug" / "rnqcc"
DEFAULT_SUITE = Path("/tmp/rnqcc-llvm-test-suite/SingleSource/Regression/C")


def timeout_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value


def run(cmd: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        return subprocess.CompletedProcess(
            cmd,
            124,
            stdout=timeout_text(exc.stdout),
            stderr=(timeout_text(exc.stderr) + f"\ntimed out after {timeout:.1f}s").lstrip(),
        )


def short_failure(result: subprocess.CompletedProcess[str]) -> str:
    text = (result.stderr or result.stdout).strip()
    if not text:
        return f"exit status {result.returncode}"
    if result.returncode == 124:
        for line in reversed(text.splitlines()):
            if "timed out after" in line:
                return line[:240]
    return text.splitlines()[0][:240]


def expected_output(src: Path) -> str | None:
    reference = src.with_suffix(".reference_output")
    if reference.exists():
        return reference.read_text()
    return None


def observed_output(result: subprocess.CompletedProcess[str]) -> str:
    return f"{result.stdout or ''}exit {result.returncode}\n"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run rnqcc over LLVM SingleSource/Regression/C tests."
    )
    parser.add_argument("--rnqcc", default=str(DEFAULT_RNQCC))
    parser.add_argument("--suite", default=str(DEFAULT_SUITE))
    parser.add_argument("--limit", type=int, default=36)
    parser.add_argument("--start", type=int, default=0)
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--internal-cpp", action="store_true")
    args = parser.parse_args()

    if args.start < 0:
        raise SystemExit("--start must be non-negative")
    if args.limit <= 0:
        raise SystemExit("--limit must be positive")
    if args.timeout <= 0:
        raise SystemExit("--timeout must be positive")

    rnqcc = Path(args.rnqcc)
    suite = Path(args.suite)
    if not rnqcc.exists():
        subprocess.run(["cargo", "build"], cwd=ROOT, check=True)

    tests = sorted(suite.glob("*.c"))
    selected = tests[args.start : args.start + args.limit]
    if not selected:
        raise SystemExit("no tests selected")

    passed = 0
    failures: list[tuple[Path, str]] = []
    with tempfile.TemporaryDirectory(prefix="rnqcc-llvm-c-regression.") as tmp:
        tmpdir = Path(tmp)
        for idx, src in enumerate(selected, start=args.start):
            exe = tmpdir / f"{idx:04d}-{src.stem}"
            cmd = [str(rnqcc), "--Wno-missing-return"]
            if args.internal_cpp:
                cmd.append("--internal-cpp")
            result = run([*cmd, str(src), "-o", str(exe)], args.timeout)
            if result.returncode != 0:
                failures.append((src, short_failure(result)))
                continue

            result = run([str(exe)], args.timeout)
            expected = expected_output(src)
            if expected is not None and observed_output(result) != expected:
                failures.append((src, "stdout differed from reference output"))
                continue
            if expected is None and result.returncode != 0:
                failures.append((src, short_failure(result)))
                continue
            passed += 1

    print(
        f"llvm C regression: {passed}/{len(selected)} passed "
        f"(start={args.start}, limit={args.limit})"
    )
    for src, reason in failures[:20]:
        print(f"FAIL {src.name}: {reason}")
    if len(failures) > 20:
        print(f"... {len(failures) - 20} more failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
