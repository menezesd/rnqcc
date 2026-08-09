#!/usr/bin/env python3
"""Report stale and missing GCC torture expected failures from a CI artifact."""

from __future__ import annotations

import argparse
from pathlib import Path

try:
    from gcc_torture_expected import (
        DEFAULT_EXPECTED,
        display_reason,
        load_expected_failures,
        parse_failure_log,
    )
except ModuleNotFoundError as err:
    if err.name != "gcc_torture_expected":
        raise
    from scripts.gcc_torture_expected import (
        DEFAULT_EXPECTED,
        display_reason,
        load_expected_failures,
        parse_failure_log,
    )


def parse_failures(path: Path) -> tuple[dict[str, str], dict[str, str], dict[str, str]]:
    stale: dict[str, str] = {}
    xfail: dict[str, str] = {}
    fail: dict[str, str] = {}
    for test, status, reason in parse_failure_log(path):
        if status == "stale":
            stale[test] = reason
        elif status == "xfail":
            xfail[test] = reason
        else:
            fail[test] = reason
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
    parser.add_argument(
        "--fail-on-unexpected-xfail",
        action="store_true",
        help="exit non-zero when the artifact contains XFAIL rows absent from the fixture",
    )
    parser.add_argument(
        "--fail-on-unmarked-expected",
        action="store_true",
        help="exit non-zero when expected failures appear without an XFAIL marker",
    )
    parser.add_argument(
        "--fail-on-absent-expected",
        action="store_true",
        help="exit non-zero when expected failures are absent from the artifact",
    )
    parser.add_argument(
        "--mode",
        choices=("compile", "execute"),
        help=(
            "gcc.c-torture subdirectory this artifact covers; restricts the "
            "absent-expected check to fixture entries from that subdirectory"
        ),
    )
    args = parser.parse_args()

    expected = load_expected_failures(args.expected)
    stale, xfail, fail = parse_failures(args.failures)

    stale_expected = {test: reason for test, reason in stale.items() if test in expected}
    missing = {test: reason for test, reason in fail.items() if test not in expected}
    unmarked_expected = {test: reason for test, reason in fail.items() if test in expected}
    still_expected = {test: reason for test, reason in xfail.items() if test in expected}
    unexpected_xfail = {test: reason for test, reason in xfail.items() if test not in expected}
    # A run only ever reports on its own gcc.c-torture subdirectory, so
    # entries from the other mode are absent by construction, not by regression.
    in_mode = (
        (lambda test: test.startswith(f"{args.mode}/")) if args.mode else (lambda _test: True)
    )
    absent_expected = {
        test: reason
        for test, reason in expected.items()
        if in_mode(test) and test not in stale_expected and test not in xfail and test not in fail
    }

    print(f"expected failures: {len(expected)}")
    print(f"stale xfails still in fixture: {len(stale_expected)}")
    print(f"still xfail: {len(still_expected)}")
    print(f"expected failures without xfail marker: {len(unmarked_expected)}")
    print(f"xfails absent from fixture: {len(unexpected_xfail)}")
    print(f"unexpected failures missing from fixture: {len(missing)}")
    print(f"expected entries absent from artifact: {len(absent_expected)}")

    if stale_expected:
        print("\nRemove these stale expected failures:")
        for test in sorted(stale_expected):
            print(f"  {test} | {display_reason(stale_expected[test])}")

    if missing:
        print("\nAdd or fix these unexpected failures:")
        for test in sorted(missing):
            print(f"  {test} | {display_reason(missing[test])}")

    if unmarked_expected:
        print("\nExpected failures without XFAIL marker:")
        for test in sorted(unmarked_expected):
            print(f"  {test} | {display_reason(unmarked_expected[test])}")

    if unexpected_xfail:
        print("\nArtifact XFAIL rows absent from fixture:")
        for test in sorted(unexpected_xfail):
            print(f"  {test} | {display_reason(unexpected_xfail[test])}")

    if absent_expected:
        print("\nExpected entries not seen in artifact:")
        for test in sorted(absent_expected):
            print(f"  {test} | {display_reason(absent_expected[test])}")

    return (
        1
        if missing
        or (args.fail_on_stale and stale_expected)
        or (args.fail_on_unexpected_xfail and unexpected_xfail)
        or (args.fail_on_unmarked_expected and unmarked_expected)
        or (args.fail_on_absent_expected and absent_expected)
        else 0
    )


if __name__ == "__main__":
    raise SystemExit(main())
