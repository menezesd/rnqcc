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

import argparse
import os
import shlex
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
DEFAULT_MANIFEST = ROOT / "tests" / "fixtures" / "real_project" / "corpus.txt"
ARTIFACT_DIR = os.environ.get("CI_ARTIFACT_DIR")
DEFAULT_TIMEOUT = env_timeout("REAL_PROJECT_TIMEOUT", "60.0")


class Entry:
    def __init__(self, source: Path, flags: list[str], expected_failure: str | None) -> None:
        self.source = source
        self.flags = flags
        self.expected_failure = expected_failure


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--rnqcc",
        default=os.environ.get("RNQCC", str(ROOT / "target" / "debug" / "rnqcc")),
        help="path to rnqcc binary, or set RNQCC",
    )
    parser.add_argument(
        "--manifest",
        default=os.environ.get("REAL_PROJECT_MANIFEST", str(DEFAULT_MANIFEST)),
        help="corpus manifest path, or set REAL_PROJECT_MANIFEST",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT,
        help="seconds to allow each command, or set REAL_PROJECT_TIMEOUT",
    )
    parser.add_argument(
        "--rnqcc-arg",
        action="append",
        dest="rnqcc_args",
        default=[],
        help="extra argument to pass to rnqcc for every compile invocation; repeatable.",
    )
    return parser.parse_args(argv)


def run(cmd: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
    return run_with_timeout(cmd, timeout=timeout)


def build_rnqcc(rnqcc: Path, timeout: float) -> None:
    if rnqcc.is_file():
        return
    if rnqcc.exists():
        raise SystemExit(f"--rnqcc path is not a file: {rnqcc}")
    result = run(["cargo", "build", "--locked", "--manifest-path", str(ROOT / "Cargo.toml")], timeout)
    if result.returncode != 0:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise SystemExit(result.returncode)
    if not rnqcc.is_file():
        raise SystemExit(f"--rnqcc not found after build: {rnqcc}")


def save_failure_artifact(index: int, entry: Entry, cmd: list[str], text: str) -> None:
    if not ARTIFACT_DIR:
        return
    dest = Path(ARTIFACT_DIR) / "real_project_corpus" / f"{index:03d}"
    dest.mkdir(parents=True, exist_ok=True)
    (dest / "command.txt").write_text(" ".join(shlex.quote(part) for part in cmd) + "\n", encoding="utf-8")
    (dest / "output.txt").write_text(text, encoding="utf-8")
    if entry.source.exists():
        try:
            (dest / entry.source.name).write_bytes(entry.source.read_bytes())
        except OSError as err:
            (dest / "copy-error.txt").write_text(str(err), encoding="utf-8")


def resolve_flag(flag: str) -> str:
    for prefix in ("-I", "-iquote", "-isystem", "-idirafter"):
        if flag.startswith(prefix) and len(flag) > len(prefix):
            path = Path(flag[len(prefix) :])
            if not path.is_absolute():
                return f"{prefix}{ROOT / path}"
    return flag


def parse_manifest(path: Path) -> list[Entry]:
    entries = []
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise SystemExit(f"could not read corpus manifest {path}: {exc}") from exc
    for line_no, raw in enumerate(lines, start=1):
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


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if not is_positive_finite(args.timeout):
        print("--timeout must be positive", file=sys.stderr)
        return 1
    manifest = Path(args.manifest)
    if not manifest.is_absolute():
        manifest = ROOT / manifest
    rnqcc = Path(args.rnqcc)
    build_rnqcc(rnqcc, args.timeout)
    entries = parse_manifest(manifest)
    if not entries:
        print(f"{manifest}: no corpus entries", file=sys.stderr)
        return 1

    with tempfile.TemporaryDirectory(prefix="rnqcc-real-corpus.") as tmp:
        tmpdir = Path(tmp)
        failures = 0
        for index, entry in enumerate(entries, start=1):
            output = tmpdir / f"{index}.o"
            cmd = [
                str(rnqcc),
                "--internal-cpp",
                *args.rnqcc_args,
                *entry.flags,
                "-c",
                str(entry.source),
                "-o",
                str(output),
            ]
            result = run(cmd, args.timeout)
            text = result.stdout + result.stderr
            if entry.expected_failure is None:
                if result.returncode != 0:
                    print(f"{entry.source}: unexpected failure", file=sys.stderr)
                    sys.stderr.write(text)
                    save_failure_artifact(index, entry, cmd, text)
                    failures += 1
                elif not output.exists():
                    print(f"{entry.source}: object was not produced", file=sys.stderr)
                    save_failure_artifact(index, entry, cmd, text)
                    failures += 1
            elif result.returncode == 0:
                print(f"{entry.source}: expected failure now succeeds", file=sys.stderr)
                save_failure_artifact(index, entry, cmd, text)
                failures += 1
            elif entry.expected_failure not in text:
                print(
                    f"{entry.source}: expected failure did not contain "
                    f"`{entry.expected_failure}`",
                    file=sys.stderr,
                )
                sys.stderr.write(text)
                save_failure_artifact(index, entry, cmd, text)
                failures += 1
        if failures:
            return 1
    print(f"real project corpus passed: {len(entries)} entries")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
