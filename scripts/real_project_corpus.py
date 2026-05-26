#!/usr/bin/env python3
"""Compile a manifest of project-shaped C files with rnqcc.

Manifest format is line-oriented:

  path/to/file.c
  path/to/file.c | -I include -DNAME=1
  path/to/file.c | -I include | expect-fail: diagnostic substring

Paths and relative include flags are resolved from the repository root. Expected
failures are a ratchet: if an expected-fail entry starts compiling, this script
fails so the manifest can be updated.
"""

from __future__ import annotations

import os
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = ROOT / "tests" / "fixtures" / "real_project" / "corpus.txt"
RNQCC = Path(os.environ.get("RNQCC", ROOT / "target" / "debug" / "rnqcc"))


class Entry:
    def __init__(self, source: Path, flags: list[str], expected_failure: str | None) -> None:
        self.source = source
        self.flags = flags
        self.expected_failure = expected_failure


def run(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def build_rnqcc() -> None:
    if RNQCC.exists():
        return
    result = run(["cargo", "build", "--manifest-path", str(ROOT / "Cargo.toml")])
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)


def resolve_flag(flag: str) -> str:
    for prefix in ("-I", "-iquote", "-isystem", "-idirafter"):
        if flag.startswith(prefix) and len(flag) > len(prefix):
            path = Path(flag[len(prefix) :])
            if not path.is_absolute():
                return f"{prefix}{ROOT / path}"
    return flag


def parse_manifest(path: Path) -> list[Entry]:
    entries = []
    for line_no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        fields = [field.strip() for field in line.split("|")]
        if not fields[0]:
            raise SystemExit(f"{path}:{line_no}: missing source path")
        source = Path(fields[0])
        if not source.is_absolute():
            source = ROOT / source
        flags = shlex.split(fields[1]) if len(fields) >= 2 and fields[1] else []
        flags = [resolve_flag(flag) for flag in flags]
        expected_failure = None
        if len(fields) >= 3 and fields[2]:
            marker = "expect-fail:"
            if not fields[2].startswith(marker):
                raise SystemExit(f"{path}:{line_no}: expected `{marker}`")
            expected_failure = fields[2][len(marker) :].strip()
        if len(fields) > 3:
            raise SystemExit(f"{path}:{line_no}: too many fields")
        entries.append(Entry(source, flags, expected_failure))
    return entries


def main() -> int:
    manifest = Path(os.environ.get("REAL_PROJECT_MANIFEST", DEFAULT_MANIFEST))
    if not manifest.is_absolute():
        manifest = ROOT / manifest
    build_rnqcc()
    entries = parse_manifest(manifest)
    if not entries:
        print(f"{manifest}: no corpus entries", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(prefix="rnqcc-real-corpus.") as tmp:
        tmpdir = Path(tmp)
        failures = 0
        for index, entry in enumerate(entries, start=1):
            output = tmpdir / f"{index}.o"
            cmd = [str(RNQCC), "--internal-cpp", *entry.flags, "-c", str(entry.source), "-o", str(output)]
            result = run(cmd)
            text = result.stdout + result.stderr
            if entry.expected_failure is None:
                if result.returncode != 0:
                    print(f"{entry.source}: unexpected failure", file=sys.stderr)
                    sys.stderr.write(text)
                    failures += 1
                elif not output.exists():
                    print(f"{entry.source}: object was not produced", file=sys.stderr)
                    failures += 1
            elif result.returncode == 0:
                print(f"{entry.source}: expected failure now succeeds", file=sys.stderr)
                failures += 1
            elif entry.expected_failure not in text:
                print(
                    f"{entry.source}: expected failure did not contain "
                    f"`{entry.expected_failure}`",
                    file=sys.stderr,
                )
                sys.stderr.write(text)
                failures += 1
        if failures:
            return 1
    print(f"real project corpus passed: {len(entries)} entries")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
