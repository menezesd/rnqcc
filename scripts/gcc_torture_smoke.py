#!/usr/bin/env python3
"""Run a bounded GCC C torture smoke test against rnqcc.

This is intentionally not a DejaGnu replacement.  It gives us a repeatable
frontier against the GCC torture corpus by compiling a deterministic subset
and, for execute tests, checking that generated programs exit successfully.
"""

from __future__ import annotations

import argparse
import os
import re
import shlex
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RNQCC = ROOT / "target" / "debug" / "rnqcc"
DEFAULT_SUITE = Path("/tmp/rnqcc-gcc-torture/gcc/testsuite/gcc.c-torture")


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
            stdout=(exc.stdout or b"").decode() if isinstance(exc.stdout, bytes) else exc.stdout,
            stderr=f"timed out after {timeout:.1f}s",
        )


def tests_for_mode(suite: Path, mode: str) -> list[Path]:
    subdir = suite / mode
    if not subdir.is_dir():
        raise SystemExit(f"{subdir}: not found")
    return sorted(subdir.glob("*.c"))


def timeout_for_test(src: Path, base_timeout: float) -> float:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return base_timeout
    timeout = base_timeout
    if "dg-add-options stack_size" in text or "dg-require-stack-size" in text:
        timeout *= 8.0
    match = re.search(r"dg-timeout-factor\s+([0-9]+(?:\.[0-9]+)?)", text)
    if match:
        timeout *= float(match.group(1))
    return timeout


def rnqcc_options_for_test(src: Path) -> list[str]:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return []
    options: list[str] = []
    for quoted in re.findall(r"dg-options\s+\"([^\"]*)\"", text):
        for option in shlex.split(quoted):
            if option == "-finstrument-functions":
                options.append(option)
    return options


def use_internal_cpp_for_test(src: Path) -> bool:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return False
    return re.search(r"\bva_start\s*\(\s*[^,\)]+\s*\)", text) is not None


def skip_reason_for_test(src: Path) -> str | None:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return None
    if "__builtin_va_arg_pack" in text:
        return "unsupported GCC __builtin_va_arg_pack extension"
    if "__builtin_apply" in text or "__builtin_apply_args" in text:
        return "unsupported GCC __builtin_apply extension"
    if "-fpermissive" in text:
        return "unsupported permissive invalid-C compatibility test"
    if "-fgimple" in text or "__GIMPLE" in text:
        return "unsupported GCC GIMPLE source extension"
    if "dg-error" in text or "dg-warning" in text:
        return "expected-diagnostic GCC torture test"
    target_specific_dejagnu = "dg-do compile { target" in text or "dg-do assemble { target" in text
    portable_target_compile_smoke = {
        "103818.c",
        "20010327-1.c",
        "20111209-1.c",
        "asmgoto-2.c",
        "asmgoto-3.c",
        "asmgoto-4.c",
        "asmgoto-5.c",
        "asmgoto-6.c",
        "attr-retain-1.c",
        "attr-retain-2.c",
        "mipscop-1.c",
        "mipscop-2.c",
        "mipscop-3.c",
        "mipscop-4.c",
        "pr29201.c",
        "pr30311.c",
        "pr44707.c",
        "pr65014.c",
        "pr65680.c",
        "pr84960.c",
        "pr93335.c",
        "pr96998.c",
        "pr98096.c",
    }
    if target_specific_dejagnu and not (
        src.parent.name == "compile" and src.name in portable_target_compile_smoke
    ):
        return "target-specific DejaGnu test outside smoke target model"
    if "../../gcc.target/" in text or "../../gcc.dg/" in text:
        return "target-specific included GCC test"
    if "#if empty#cpu" in text:
        return "invalid preprocessor token-paste edge test"
    complex_compile_gaps: dict[str, str] = {}
    complex_execute_gaps: dict[str, str] = {}
    if src.parent.name == "compile" and src.name in complex_compile_gaps:
        return complex_compile_gaps[src.name]
    if src.parent.name == "execute" and src.name in complex_execute_gaps:
        return complex_execute_gaps[src.name]
    if re.search(r"\bva_arg\s*\([^,]+,\s*typeof\s*\(", text):
        return "unsupported variadic VLA aggregate argument"
    if re.search(r"\bint\s+\w+\s*\[[^\]\d][^\]]*\]", text) and re.search(
        r"\bgoto\s+\w+\s*;", text
    ):
        return "unsupported VLA stack deallocation across goto"
    lines = text.splitlines()
    old_style_def = False
    for index, line in enumerate(lines[:-1]):
        if ")" not in line or line.lstrip().startswith(("if", "for", "while", "switch")):
            continue
        saw_param_decl = False
        for following in lines[index + 1 : index + 8]:
            stripped = following.strip()
            if not stripped:
                continue
            if stripped == "{":
                old_style_def = saw_param_decl
                break
            if stripped.endswith(";") and re.match(
                r"(?:register\s+)?(?:__const\s+)?(?:struct\s+\w+|union\s+\w+|enum\s+\w+|int|long|short|char|void|[A-Za-z_]\w*)\b",
                stripped,
            ):
                saw_param_decl = True
                continue
            break
        if old_style_def:
            break
    if "SIZE1 ((size_t) -1)" in text and "__builtin_" in text:
        return "builtin library stress test with huge object sizes"
    if src.name.startswith("limits-"):
        return "compiler translation-limit stress test"
    if (
        "dg-require-stack-size" in text
        or "dg-add-options stack_size" in text
        or "dg-require-stack-check" in text
        or "-fstack-check" in text
        or re.search(r"\bASIZE\s+0x[0-9a-fA-F]{8,}", text)
    ):
        return "stack-size stress test"
    if "C4096" in text and "This testcase exposed" in text:
        return "oversized code-generation stress test"
    if "! { i?86-*-* x86_64-*-* }" in text:
        return "x86-only GCC torture test"
    if "dg-require-effective-target trampolines" in text:
        return "requires GCC nested-function trampolines"
    scalar_storage_order_execute_gaps = {
        "20230630-2.c": "unsupported reverse scalar_storage_order bit-field layout",
        "20230630-4.c": "unsupported reverse scalar_storage_order bit-field layout",
    }
    if src.parent.name == "execute" and src.name in scalar_storage_order_execute_gaps:
        return scalar_storage_order_execute_gaps[src.name]
    if re.search(r"^\s+void\s+nested\w*\s*\(", text, re.MULTILINE):
        return "unsupported GCC nested-function extension"
    if "gcc_tmpnam.h" in text and "dg-require-effective-target fileio" in text:
        return "requires tmpnam file I/O unavailable in this sandbox"
    return None


def short_failure(result: subprocess.CompletedProcess[str]) -> str:
    text = (result.stderr or result.stdout).strip()
    if not text:
        return f"exit status {result.returncode}"
    first = text.splitlines()[0]
    return first[:240]


def load_expected_failures(path: Path | None) -> dict[str, str]:
    if path is None or not path.exists():
        return {}
    expected: dict[str, str] = {}
    for line_no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        fields = [field.strip() for field in line.split("|", 1)]
        if len(fields) != 2 or not fields[0] or not fields[1]:
            raise SystemExit(f"{path}:{line_no}: expected `relative/path.c | diagnostic substring`")
        expected[fields[0]] = fields[1]
    return expected


def save_failure_artifact(
    artifact_dir: Path | None,
    suite: Path,
    index: int,
    src: Path,
    cmd: list[str],
    result: subprocess.CompletedProcess[str],
) -> None:
    if artifact_dir is None:
        return
    dest = artifact_dir / "gcc_torture" / f"{index:04d}-{src.stem}"
    dest.mkdir(parents=True, exist_ok=True)
    (dest / "command.txt").write_text(
        " ".join(shlex.quote(part) for part in cmd) + "\n",
        encoding="utf-8",
    )
    (dest / "output.txt").write_text(
        (result.stdout or "") + (result.stderr or ""),
        encoding="utf-8",
    )
    try:
        rel = src.relative_to(suite)
    except ValueError:
        rel = Path(src.name)
    (dest / "source-path.txt").write_text(str(rel) + "\n", encoding="utf-8")
    try:
        (dest / src.name).write_bytes(src.read_bytes())
    except OSError as err:
        (dest / "copy-error.txt").write_text(str(err), encoding="utf-8")


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
        "--max-failures",
        type=int,
        default=20,
        help="maximum number of failures to print; use 0 for all",
    )
    parser.add_argument(
        "--failure-log",
        type=Path,
        help="write all failures to this file",
    )
    parser.add_argument(
        "--expected-failures",
        type=Path,
        help="file of `relative/path.c | diagnostic substring` failures to treat as known",
    )
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        default=Path(os.environ["CI_ARTIFACT_DIR"]) if "CI_ARTIFACT_DIR" in os.environ else None,
        help="copy failing test sources and command output under this directory",
    )
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
    expected_failures = load_expected_failures(args.expected_failures)

    passed = 0
    skipped: list[tuple[Path, str]] = []
    failures: list[tuple[Path, str]] = []
    expected_failed: list[tuple[Path, str]] = []
    stale_expected: list[tuple[str, str]] = []
    with tempfile.TemporaryDirectory(prefix="rnqcc-gcc-torture.") as tmp:
        tmpdir = Path(tmp)
        for idx, src in enumerate(selected, start=args.start):
            rel = str(src.relative_to(suite))
            if reason := skip_reason_for_test(src):
                skipped.append((src, reason))
                continue
            stem = f"{idx:04d}-{src.stem}"
            common = [str(rnqcc), "--Wno-missing-return"]
            common.extend(rnqcc_options_for_test(src))
            timeout = timeout_for_test(src, args.timeout)
            if args.internal_cpp or use_internal_cpp_for_test(src):
                common.append("--internal-cpp")

            if args.mode == "execute":
                exe = tmpdir / stem
                compile_cmd = [*common, str(src), "-o", str(exe), "-lm"]
                result = run(compile_cmd, timeout)
                cmd = compile_cmd
                if result.returncode == 0:
                    run_cmd = [str(exe)]
                    result = run(run_cmd, timeout)
                    cmd = run_cmd
                if result.returncode == 0:
                    passed += 1
                    if rel in expected_failures:
                        stale_expected.append((rel, expected_failures[rel]))
                else:
                    failure = short_failure(result)
                    expected = expected_failures.get(rel)
                    if expected is not None and expected in failure:
                        expected_failed.append((src, failure))
                    else:
                        failures.append((src, failure))
                        save_failure_artifact(args.artifact_dir, suite, idx, src, cmd, result)
            else:
                obj = tmpdir / f"{stem}.o"
                cmd = [*common, "-c", str(src), "-o", str(obj)]
                result = run(cmd, timeout)
                if result.returncode == 0:
                    passed += 1
                    if rel in expected_failures:
                        stale_expected.append((rel, expected_failures[rel]))
                else:
                    failure = short_failure(result)
                    expected = expected_failures.get(rel)
                    if expected is not None and expected in failure:
                        expected_failed.append((src, failure))
                    else:
                        failures.append((src, failure))
                        save_failure_artifact(args.artifact_dir, suite, idx, src, cmd, result)

    print(
        f"gcc torture {args.mode}: {passed}/{len(selected) - len(skipped)} passed "
        f"(start={args.start}, limit={args.limit}, skipped={len(skipped)}, "
        f"expected_failed={len(expected_failed)})"
    )
    if skipped:
        skip_counts = Counter(reason for _, reason in skipped)
        summary = ", ".join(
            f"{count} {reason}" for reason, count in skip_counts.most_common()
        )
        print(f"skip reasons: {summary}")
    if args.failure_log:
        args.failure_log.parent.mkdir(parents=True, exist_ok=True)
        args.failure_log.write_text(
            "".join(
                f"{src.relative_to(suite)}\t{reason}\n" for src, reason in failures
            )
            + "".join(
                f"{rel}\tSTALE-XFAIL: {reason}\n" for rel, reason in stale_expected
            )
            + "".join(
                f"{src.relative_to(suite)}\tXFAIL: {reason}\n"
                for src, reason in expected_failed
            )
            + "".join(
                f"{src.relative_to(suite)}\tSKIP: {reason}\n" for src, reason in skipped
            )
        )

    shown = failures if args.max_failures == 0 else failures[: args.max_failures]
    for src, reason in shown:
        rel = src.relative_to(suite)
        print(f"FAIL {rel}: {reason}")
    if args.max_failures != 0 and len(failures) > args.max_failures:
        print(f"... {len(failures) - args.max_failures} more failures")
    for rel, reason in stale_expected:
        print(f"STALE-XFAIL {rel}: {reason}")
    return 1 if failures or stale_expected else 0


if __name__ == "__main__":
    sys.exit(main())
