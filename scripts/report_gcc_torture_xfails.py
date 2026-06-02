#!/usr/bin/env python3
"""Report stale and missing GCC torture expected failures from a CI artifact."""

from __future__ import annotations

import argparse
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_EXPECTED = ROOT / "tests" / "fixtures" / "gcc_torture_expected_failures.txt"


def load_expected(path: Path) -> dict[str, str]:
    expected: dict[str, str] = {}
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "|" not in line:
            raise SystemExit(f"{path}: expected '<test> | <reason>', got {raw!r}")
        test, reason = line.split("|", 1)
        expected[test.strip()] = reason.strip()
    return expected


def parse_failures(path: Path) -> tuple[dict[str, str], dict[str, str], dict[str, str]]:
    stale: dict[str, str] = {}
    xfail: dict[str, str] = {}
    fail: dict[str, str] = {}
    for raw in path.read_text().splitlines():
        if not raw.strip() or "\t" not in raw:
            continue
        test, status = raw.split("\t", 1)
        if status.startswith("STALE-XFAIL:"):
            stale[test] = status.removeprefix("STALE-XFAIL:").strip()
        elif status.startswith("XFAIL:"):
            xfail[test] = status.removeprefix("XFAIL:").strip()
        elif status.startswith("FAIL:"):
            fail[test] = status.removeprefix("FAIL:").strip()
        elif status.startswith("SKIP:"):
            continue
        else:
            fail[test] = status.strip()
    return stale, xfail, fail


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "failures",
        type=Path,
        help="path to gcc-torture-failures.txt from the GitHub Actions artifact",
    )
    parser.add_argument(
        "--expected",
        type=Path,
        default=DEFAULT_EXPECTED,
        help=f"expected-failure fixture (default: {DEFAULT_EXPECTED})",
    )
    parser.add_argument(
        "--fail-on-stale",
        action="store_true",
        help="exit non-zero when expected failures are stale",
    )
    args = parser.parse_args()

    expected = load_expected(args.expected)
    stale, xfail, fail = parse_failures(args.failures)

    stale_expected = {test: reason for test, reason in stale.items() if test in expected}
    missing = {test: reason for test, reason in fail.items() if test not in expected}
    still_expected = {test: reason for test, reason in xfail.items() if test in expected}
    absent_expected = {
        test: reason
        for test, reason in expected.items()
        if test not in stale_expected and test not in xfail and test not in fail
    }

    print(f"expected failures: {len(expected)}")
    print(f"stale xfails still in fixture: {len(stale_expected)}")
    print(f"still xfail: {len(still_expected)}")
    print(f"unexpected failures missing from fixture: {len(missing)}")
    print(f"expected entries absent from artifact: {len(absent_expected)}")

    if stale_expected:
        print("\nRemove these stale expected failures:")
        for test in sorted(stale_expected):
            print(f"  {test} | {stale_expected[test]}")

    if missing:
        print("\nAdd or fix these unexpected failures:")
        for test in sorted(missing):
            print(f"  {test} | {missing[test]}")

    if absent_expected:
        print("\nExpected entries not seen in artifact:")
        for test in sorted(absent_expected):
            print(f"  {test} | {absent_expected[test]}")

    return 1 if missing or (args.fail_on_stale and stale_expected) else 0


if __name__ == "__main__":
    raise SystemExit(main())
