#!/usr/bin/env python3
"""Bucket GCC torture smoke failures by source features."""

from __future__ import annotations

import argparse
import shutil
from dataclasses import dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath

try:
    from gcc_torture_expected import (
        DEFAULT_EXPECTED,
        display_reason,
        load_expected_failures,
        normalize_test_path,
        parse_failure_log,
        validate_test_path,
    )
except ModuleNotFoundError as err:
    if err.name != "gcc_torture_expected":
        raise
    from scripts.gcc_torture_expected import (
        DEFAULT_EXPECTED,
        display_reason,
        load_expected_failures,
        normalize_test_path,
        parse_failure_log,
        validate_test_path,
    )


@dataclass(frozen=True)
class Entry:
    test: str
    status: str
    reason: str
    source: Path | None
    output: Path | None


def source_from_suite(suite: Path | None, test: str) -> Path | None:
    if suite is None:
        return None
    candidate = suite.joinpath(*PureWindowsPath(test).parts)
    return candidate if candidate.is_file() else None


def test_basename(test: str) -> str:
    posix_name = PurePosixPath(test).name
    windows_name = PureWindowsPath(test).name
    return min(posix_name, windows_name, key=len)


def read_artifact_test_path(path: Path) -> tuple[str, str] | None:
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError:
        return None
    lines = raw.splitlines()
    if len(lines) != 1 or not lines[0].strip():
        raise SystemExit(f"{path}: expected exactly one artifact source path")
    test = lines[0]
    if test != test.strip():
        raise SystemExit(f"{path}: whitespace around artifact source path")
    validate_test_path(path, 1, test)
    return normalize_test_path(test), test_basename(test)


def index_artifacts(artifact_dir: Path | None) -> dict[str, tuple[Path | None, Path | None]]:
    artifacts: dict[str, tuple[Path | None, Path | None]] = {}
    if artifact_dir is None:
        return artifacts
    for source_path_file in sorted(artifact_dir.glob("**/source-path.txt")):
        parsed = read_artifact_test_path(source_path_file)
        if parsed is None:
            continue
        normalized_test, source_name = parsed
        if normalized_test in artifacts:
            raise SystemExit(f"{source_path_file}: duplicate artifact entry for {normalized_test}")
        test_dir = source_path_file.parent
        source = test_dir / source_name
        output = test_dir / "output.txt"
        artifacts[normalized_test] = (
            source if source.is_file() else None,
            output if output.is_file() else None,
        )
    return artifacts


def read_source(path: Path | None) -> str:
    if path is None:
        return ""
    try:
        return path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return ""


def validate_input_dir(path: Path | None, label: str) -> None:
    if path is None:
        return
    if not path.exists():
        raise SystemExit(f"{label} directory not found: {path}")
    if not path.is_dir():
        raise SystemExit(f"{label} path is not a directory: {path}")


def validate_output_dir(path: Path | None, label: str) -> None:
    if path is None:
        return
    if path.exists() and not path.is_dir():
        raise SystemExit(f"{label} path is not a directory: {path}")
    if not path.exists() and path.parent.exists() and not path.parent.is_dir():
        raise SystemExit(f"{label} parent path is not a directory: {path.parent}")


def bucket_for(entry: Entry, text: str) -> str:
    combined = f"{entry.test}\n{entry.reason}\n{text}"
    if entry.status == "stale":
        return "stale-expected-failure"
    if "timed out" in entry.reason:
        return "timeout-or-infinite-loop"
    if "va_arg" in combined or "va_start" in combined or "__builtin_va_arg_pack" in combined:
        return "varargs-abi"
    if "__complex__" in combined or "_Complex" in combined:
        return "complex-arithmetic-abi"
    if "long double" in combined or "LongDouble" in combined:
        return "long-double"
    if ":" in text and "struct" in text and ("long long" in text or "int " in text):
        return "bitfields"
    if "__builtin_isinf" in combined or "__builtin_isnan" in combined:
        return "floating-builtins"
    if "&&" in text or "goto *" in text:
        return "computed-goto"
    if "struct" in text:
        return "aggregate-abi"
    if "exit status -11" in entry.reason:
        return "runtime-crash"
    if "exit status -6" in entry.reason:
        return "runtime-abort"
    return "other"


def safe_copy_name(test: str) -> str:
    name = []
    for ch in test:
        if ch in ("/", "\\"):
            name.append("__")
        elif ch.isalnum() or ch in (".", "_", "-"):
            name.append(ch)
        else:
            name.append("_")
    return "".join(name)


def copy_entry(
    out_dir: Path,
    bucket: str,
    entry: Entry,
    copied_names: dict[tuple[str, str], str],
) -> None:
    safe_name = safe_copy_name(entry.test)
    key = (bucket, safe_name)
    previous = copied_names.setdefault(key, entry.test)
    if previous != entry.test:
        raise SystemExit(
            f"{entry.test}: copy filename collision with {previous}: {bucket}/{safe_name}"
        )
    dest = out_dir / bucket
    dest.mkdir(parents=True, exist_ok=True)
    if entry.source is not None:
        shutil.copy2(entry.source, dest / safe_name)
    if entry.output is not None:
        shutil.copy2(entry.output, dest / f"{safe_name}.output.txt")
    (dest / f"{safe_name}.reason.txt").write_text(
        f"{entry.status}: {entry.reason}\n", encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("failures", type=Path, help="gcc-torture-failures.txt")
    parser.add_argument("--suite", type=Path, help="path to gcc.c-torture")
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        help="artifact root containing gcc_torture source copies",
    )
    parser.add_argument(
        "--expected",
        type=Path,
        default=DEFAULT_EXPECTED,
        help=f"expected-failure fixture (default: {DEFAULT_EXPECTED})",
    )
    parser.add_argument("--copy-to", type=Path, help="copy sources/output into bucket dirs")
    args = parser.parse_args()

    validate_input_dir(args.suite, "suite")
    validate_input_dir(args.artifact_dir, "artifact")
    validate_output_dir(args.copy_to, "copy-to")

    expected = set(load_expected_failures(args.expected))
    artifacts = index_artifacts(args.artifact_dir)
    entries: list[Entry] = []
    for test, status, reason in parse_failure_log(args.failures):
        source, output = artifacts.get(test, (None, None))
        source = source or source_from_suite(args.suite, test)
        if status == "fail" and test in expected:
            status = "xfail-diagnostic-mismatch"
        entries.append(Entry(test, status, reason, source, output))

    buckets: dict[str, list[Entry]] = {}
    copied_names: dict[tuple[str, str], str] = {}
    for entry in entries:
        bucket = bucket_for(entry, read_source(entry.source))
        buckets.setdefault(bucket, []).append(entry)
        if args.copy_to is not None:
            copy_entry(args.copy_to, bucket, entry, copied_names)

    print(f"entries: {len(entries)}")
    print(f"with source: {sum(1 for entry in entries if entry.source is not None)}")
    for bucket in sorted(buckets):
        print(f"\n{bucket}: {len(buckets[bucket])}")
        for entry in sorted(buckets[bucket], key=lambda item: item.test):
            print(f"  {entry.status:24} {entry.test} | {display_reason(entry.reason)}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
