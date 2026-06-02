#!/usr/bin/env python3
"""Bucket GCC torture smoke failures by source features."""

from __future__ import annotations

import argparse
import shutil
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_EXPECTED = ROOT / "tests" / "fixtures" / "gcc_torture_expected_failures.txt"


@dataclass(frozen=True)
class Entry:
    test: str
    status: str
    reason: str
    source: Path | None
    output: Path | None


def load_expected(path: Path) -> set[str]:
    expected: set[str] = set()
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if "|" not in line:
            raise SystemExit(f"{path}: expected '<test> | <reason>', got {raw!r}")
        test, _ = line.split("|", 1)
        expected.add(test.strip())
    return expected


def parse_log(path: Path) -> list[tuple[str, str, str]]:
    rows: list[tuple[str, str, str]] = []
    for raw in path.read_text(encoding="utf-8").splitlines():
        if "\t" not in raw:
            continue
        test, status = raw.split("\t", 1)
        if status.startswith("SKIP:"):
            continue
        if status.startswith("STALE-XFAIL:"):
            rows.append((test, "stale", status.removeprefix("STALE-XFAIL:").strip()))
        elif status.startswith("XFAIL:"):
            rows.append((test, "xfail", status.removeprefix("XFAIL:").strip()))
        elif status.startswith("FAIL:"):
            rows.append((test, "fail", status.removeprefix("FAIL:").strip()))
        else:
            rows.append((test, "fail", status.strip()))
    return rows


def source_from_suite(suite: Path | None, test: str) -> Path | None:
    if suite is None:
        return None
    candidate = suite / test
    return candidate if candidate.exists() else None


def source_from_artifact(artifact_dir: Path | None, test: str) -> tuple[Path | None, Path | None]:
    if artifact_dir is None:
        return None, None
    source_name = Path(test).name
    for source_path_file in artifact_dir.glob("**/source-path.txt"):
        try:
            if source_path_file.read_text(encoding="utf-8").strip() != test:
                continue
        except OSError:
            continue
        test_dir = source_path_file.parent
        source = test_dir / source_name
        output = test_dir / "output.txt"
        return (source if source.exists() else None, output if output.exists() else None)
    return None, None


def read_source(path: Path | None) -> str:
    if path is None:
        return ""
    try:
        return path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return ""


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


def copy_entry(out_dir: Path, bucket: str, entry: Entry) -> None:
    safe_name = entry.test.replace("/", "__")
    dest = out_dir / bucket
    dest.mkdir(parents=True, exist_ok=True)
    if entry.source is not None:
        shutil.copy2(entry.source, dest / Path(entry.test).name)
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

    expected = load_expected(args.expected)
    entries: list[Entry] = []
    for test, status, reason in parse_log(args.failures):
        source, output = source_from_artifact(args.artifact_dir, test)
        source = source or source_from_suite(args.suite, test)
        if status == "fail" and test in expected:
            status = "xfail-diagnostic-mismatch"
        entries.append(Entry(test, status, reason, source, output))

    buckets: dict[str, list[Entry]] = {}
    for entry in entries:
        bucket = bucket_for(entry, read_source(entry.source))
        buckets.setdefault(bucket, []).append(entry)
        if args.copy_to is not None:
            copy_entry(args.copy_to, bucket, entry)

    print(f"entries: {len(entries)}")
    print(f"with source: {sum(1 for entry in entries if entry.source is not None)}")
    for bucket in sorted(buckets):
        print(f"\n{bucket}: {len(buckets[bucket])}")
        for entry in sorted(buckets[bucket], key=lambda item: item.test):
            print(f"  {entry.status:24} {entry.test} | {entry.reason}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
