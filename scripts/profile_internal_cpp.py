#!/usr/bin/env python3
"""Profile rnqcc's internal preprocessor on selected GCC torture tests."""

from __future__ import annotations

import argparse
import os
import sys
import tempfile
import time
from pathlib import Path

try:
    from gcc_torture_smoke import (
        DEFAULT_RNQCC,
        resolve_suite,
        rnqcc_options_for_test,
        rnqcc_target_for_test,
    )
except ModuleNotFoundError as err:
    if err.name != "gcc_torture_smoke":
        raise
    from scripts.gcc_torture_smoke import (
        DEFAULT_RNQCC,
        resolve_suite,
        rnqcc_options_for_test,
        rnqcc_target_for_test,
    )

try:
    from smoke_utils import positive_finite_float, run_with_timeout
except ModuleNotFoundError as err:
    if err.name != "smoke_utils":
        raise
    from scripts.smoke_utils import positive_finite_float, run_with_timeout


DEFAULT_TESTS = [
    "compile/pr110386-2.c",
    "compile/20001226-1.c",
    "compile/limits-caselabels.c",
    "compile/limits-externdecl.c",
]
STATS_PREFIX = "rnqcc internal-cpp stats: "


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", type=Path, help="gcc.c-torture suite path")
    parser.add_argument("--rnqcc", type=Path, default=DEFAULT_RNQCC)
    parser.add_argument("--timeout", type=positive_finite_float, default=240.0)
    parser.add_argument(
        "--test",
        action="append",
        dest="tests",
        help="Relative gcc.c-torture test path; may be repeated",
    )
    return parser.parse_args()


def require_file(path: Path, description: str) -> None:
    if not path.is_file():
        raise SystemExit(f"{description} not found: {path}")


def stats_line(stderr: str) -> str:
    for line in stderr.splitlines():
        if line.startswith(STATS_PREFIX):
            return line[len(STATS_PREFIX) :]
    return "stats unavailable"


def profile_test(rnqcc: Path, suite: Path, rel: str, timeout: float, tmpdir: Path) -> int:
    src = suite / rel
    require_file(src, "GCC torture source")
    out = tmpdir / f"{rel.replace('/', '-')}.out"

    common = [str(rnqcc), "--Wno-missing-return", "--internal-cpp"]
    common.extend(rnqcc_options_for_test(src))
    if target := rnqcc_target_for_test(src):
        common.extend(["--target", target])
    if target:
        cmd = [*common, "-S", str(src), "-o", str(out)]
    else:
        cmd = [*common, "-c", str(src), "-o", str(out)]

    env = os.environ.copy()
    env["RNQCC_INTERNAL_CPP_STATS"] = "1"
    start = time.perf_counter()
    result = run_with_timeout(cmd, env=env, timeout=timeout)
    elapsed = time.perf_counter() - start
    if result.returncode == 124:
        print(f"{rel}\tTIMEOUT\t{elapsed:.3f}s\t{stats_line(result.stderr)}")
        return 1

    status = "PASS" if result.returncode == 0 else f"FAIL({result.returncode})"
    print(f"{rel}\t{status}\t{elapsed:.3f}s\t{stats_line(result.stderr)}")
    if result.returncode != 0:
        if result.stderr.strip():
            print(result.stderr.strip(), file=sys.stderr)
        return 1
    return 0


def main() -> int:
    args = parse_args()
    suite = resolve_suite(args.suite)
    require_file(args.rnqcc, "rnqcc binary")
    tests = args.tests or DEFAULT_TESTS

    failures = 0
    print("test\tstatus\telapsed\tstats")
    with tempfile.TemporaryDirectory(prefix="rnqcc-internal-cpp-profile.") as tmp:
        tmpdir = Path(tmp)
        for rel in tests:
            failures += profile_test(args.rnqcc, suite, rel, args.timeout, tmpdir)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
