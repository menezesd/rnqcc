#!/usr/bin/env python3
"""Run one command with a process-group timeout.

This is intentionally small so shell-based regression runners can use the
same timeout and descendant cleanup behavior as the Python smoke tests on
platforms that do not provide a portable ``timeout`` utility.
"""

from __future__ import annotations

import argparse
import sys

try:
    from smoke_utils import positive_finite_float, run_with_timeout
except ModuleNotFoundError as err:
    if err.name != "smoke_utils":
        raise
    from scripts.smoke_utils import positive_finite_float, run_with_timeout


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", required=True, type=positive_finite_float)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = args.command
    if command[:1] == ["--"]:
        command = command[1:]
    if not command:
        parser.error("a command is required")

    try:
        result = run_with_timeout(command, timeout=args.timeout)
    except OSError as exc:
        print(f"failed to run {command[0]}: {exc}", file=sys.stderr)
        return 127
    if result.stdout:
        sys.stdout.write(result.stdout)
    if result.stderr:
        sys.stderr.write(result.stderr)
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
