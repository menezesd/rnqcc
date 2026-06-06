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
import signal
import shlex
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

try:
    from gcc_torture_expected import (
        load_expected_failures,
        load_expected_skips,
        normalize_test_path,
        validate_test_path,
    )
except ModuleNotFoundError as err:
    if err.name != "gcc_torture_expected":
        raise
    from scripts.gcc_torture_expected import (
        load_expected_failures,
        load_expected_skips,
        normalize_test_path,
        validate_test_path,
    )


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RNQCC = ROOT / "target" / "debug" / "rnqcc"
DEFAULT_SUITE_CANDIDATES = [
    Path("/tmp/rnqcc-gcc-torture/gcc/testsuite/gcc.c-torture"),
    Path("/tmp/rnqcc-gcc-torture/gcc/gcc/testsuite/gcc.c-torture"),
]
SANDBOX_TMPNAM_HEADER = """\
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifndef GCC_TMPNAM
#define GCC_TMPNAM
static inline char *gcc_tmpnam(char *s)
{
  static char storage[4096];
  char *out = s ? s : storage;
  int fd;
  strcpy(out, "/tmp/rnqcc-gcc-torture.XXXXXX");
  fd = mkstemp(out);
  if (fd >= 0)
    {
      close(fd);
      remove(out);
    }
  return out;
}
#endif
"""


def resolve_suite(path: Path | None) -> Path:
    candidates = [path] if path is not None else DEFAULT_SUITE_CANDIDATES
    for candidate in candidates:
        if candidate is not None and candidate.is_dir():
            return candidate
    searched = ", ".join(str(candidate) for candidate in candidates if candidate is not None)
    raise SystemExit(f"gcc.c-torture suite not found; searched: {searched}")


def timeout_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value


def run(cmd: list[str], timeout: float) -> subprocess.CompletedProcess[str]:
    use_process_group = hasattr(os, "killpg")
    process = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=use_process_group,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        return subprocess.CompletedProcess(cmd, process.returncode, stdout=stdout, stderr=stderr)
    except subprocess.TimeoutExpired as exc:
        if use_process_group:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        else:
            process.kill()
        stdout, stderr = process.communicate()
        return subprocess.CompletedProcess(
            cmd,
            124,
            stdout=timeout_text(exc.stdout) + (stdout or ""),
            stderr=(
                timeout_text(exc.stderr) + (stderr or "") + f"\ntimed out after {timeout:.1f}s"
            ).lstrip(),
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
    if src.parent.name == "compile" and src.name in {
        "20001226-1.c",
        "limits-caselabels.c",
        "limits-externdecl.c",
        "pr46534.c",
        "pr110386-2.c",
    }:
        timeout *= 8.0
    if src.parent.name == "execute" and src.name == "strlen-5.c":
        timeout *= 2.0
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
    for quoted in re.findall(r"dg-(?:additional-)?options\s+\"([^\"]*)\"", text):
        for option in shlex.split(quoted):
            if option in {
                "-finstrument-functions",
                "-fpermissive",
                "-Wcompare-distinct-pointer-types",
                "-Wdeprecated-declarations",
            }:
                options.append(option)
    return options


def rnqcc_target_for_test(src: Path) -> str | None:
    if src.parent.name == "compile" and src.name in {
        "pr110386-2.c",
        "pr88423.c",
    }:
        return "x86_64-linux"
    return None


def required_warning_for_test(src: Path) -> str | None:
    if src.parent.name == "compile" and src.name == "pr103314-1.c":
        return "shift count is negative"
    if src.parent.name == "compile" and src.name in {
        "pr106537-1.c",
        "pr106537-2.c",
    }:
        return "comparison of distinct pointer types"
    if src.parent.name == "compile" and src.name == "pr84195.c":
        return "'i' is deprecated: foo.n.t.rbar"
    return None


def required_failure_for_test(src: Path) -> str | None:
    if src.parent.name != "compile":
        return None
    required = {
        "20030305-1.c": "initialization of flexible array member",
        "pr28865.c": "initialization of flexible array member",
        "pr48767.c": "__builtin_va_arg cannot read a void value",
        "pr83547.c": "void value not ignored as it ought to be",
    }
    return required.get(src.name)


def uses_tmpnam_fileio(src: Path) -> bool:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return False
    return "gcc_tmpnam.h" in text and "dg-require-effective-target fileio" in text


def materialize_source_for_test(src: Path, tmpdir: Path, idx: int) -> Path:
    if not uses_tmpnam_fileio(src):
        return src
    dest = tmpdir / "sources" / f"{idx:04d}-{src.name}"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(src.read_text(errors="ignore"), encoding="utf-8")
    (dest.parent / "gcc_tmpnam.h").write_text(SANDBOX_TMPNAM_HEADER, encoding="utf-8")
    return dest


def use_internal_cpp_for_test(src: Path) -> bool:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return False
    return (
        re.search(r"\bva_start\s*\(\s*[^,\)]+\s*\)", text) is not None
        or "#cpu(" in text
    )


def skip_reason_for_test(src: Path, internal_cpp: bool = False) -> str | None:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return None
    if "-fgimple" in text or "__GIMPLE" in text:
        return "unsupported GCC GIMPLE source extension"
    if internal_cpp and src.parent.name == "compile" and src.name in {
        "limits-exprparen.c",
    }:
        return "internal-cpp translation-limit stress timeout"
    if internal_cpp and src.parent.name == "execute" and src.name == "strlen-5.c":
        return "internal-cpp strlen stress timeout"
    if internal_cpp and src.parent.name == "compile" and src.name in {
        "20001226-1.c",
        "limits-blockid.c",
        "limits-caselabels.c",
        "limits-declparen.c",
        "limits-enumconst.c",
        "limits-externalid.c",
        "limits-externdecl.c",
        "limits-fndefn.c",
        "limits-structmem.c",
        "limits-structnest.c",
    }:
        return "internal-cpp translation-limit stress timeout"
    if internal_cpp and "dg-require-effective-target run_expensive_tests" in text:
        return "internal-cpp expensive stress test"
    portable_expected_diagnostic_smoke = {
        "20030305-1.c",
        "pr103314-1.c",
        "pr106537-1.c",
        "pr106537-2.c",
        "pr28865.c",
        "pr46534.c",
        "pr48767.c",
        "pr83547.c",
        "pr84195.c",
    }
    if (
        "dg-error" in text or "dg-warning" in text
    ) and not (src.parent.name == "compile" and src.name in portable_expected_diagnostic_smoke):
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
        "pr110386-2.c",
        "pr88347.c",
        "pr88423.c",
    }
    if target_specific_dejagnu and not (
        src.parent.name == "compile" and src.name in portable_target_compile_smoke
    ):
        return "target-specific DejaGnu test outside smoke target model"
    portable_target_execute_smoke = {
        "pr105777.c",
        "pr109938.c",
        "pr109986.c",
        "pr30314.c",
        "pr98304-2.c",
    }
    portable_included_compile_smoke = {
        "pr88347.c",
        "pr88423.c",
    }
    if ("../../gcc.target/" in text or "../../gcc.dg/" in text) and not (
        (src.parent.name == "execute" and src.name in portable_target_execute_smoke)
        or (src.parent.name == "compile" and src.name in portable_included_compile_smoke)
    ):
        return "target-specific included GCC test"
    complex_compile_gaps: dict[str, str] = {}
    complex_execute_gaps: dict[str, str] = {}
    if src.parent.name == "compile" and src.name in complex_compile_gaps:
        return complex_compile_gaps[src.name]
    if src.parent.name == "execute" and src.name in complex_execute_gaps:
        return complex_execute_gaps[src.name]
    portable_stress_compile_smoke = {
        "20000609-1.c",
        "20000804-1.c",
        "20001226-1.c",
        "20020304-1.c",
        "20020604-1.c",
        "20021015-1.c",
        "20031023-1.c",
        "20031023-2.c",
        "20031023-3.c",
        "20031023-4.c",
        "20050303-1.c",
        "20060421-1.c",
        "20071207-1.c",
        "20080806-1.c",
        "20080903-1.c",
        "20121027-1.c",
        "20151204.c",
        "920501-12.c",
        "920501-4.c",
        "920723-1.c",
        "921202-1.c",
        "930621-1.c",
        "931003-1.c",
        "931004-1.c",
        "950719-1.c",
        "951222-1.c",
        "990517-1.c",
        "991214-2.c",
        "bcopy.c",
        "limits-blockid.c",
        "limits-caselabels.c",
        "limits-declparen.c",
        "limits-enumconst.c",
        "limits-exprparen.c",
        "limits-externalid.c",
        "limits-externdecl.c",
        "limits-fndefn.c",
        "limits-fnargs.c",
        "limits-idexternal.c",
        "limits-idinternal.c",
        "limits-pointer.c",
        "limits-stringlit.c",
        "limits-structnest.c",
        "limits-structmem.c",
        "memtst.c",
        "msp.c",
        "pr23929.c",
        "pr25310.c",
        "pr34458.c",
        "pr39937.c",
        "pr41181.c",
        "pr41634.c",
        "pr43415.c",
        "pr43417.c",
        "pr44788.c",
        "sound.c",
        "stack-check-1.c",
        "string-large-1.c",
        "stuct.c",
    }
    if src.parent.name == "compile" and src.name in portable_stress_compile_smoke:
        return None
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
    stack_stress = (
        "dg-require-stack-size" in text
        or "dg-add-options stack_size" in text
        or "dg-require-stack-check" in text
        or "-fstack-check" in text
        or re.search(r"\bASIZE\s+0x[0-9a-fA-F]{8,}", text)
    )
    if stack_stress and src.parent.name != "execute":
        return "stack-size stress test"
    if "C4096" in text and "This testcase exposed" in text:
        return "oversized code-generation stress test"
    if src.name != "990413-2.c" and "! { i?86-*-* x86_64-*-* }" in text:
        return "x86-only GCC torture test"
    if src.name != "20061220-1.c" and re.search(
        r"^\s+void\s+nested\w*\s*\(", text, re.MULTILINE
    ):
        return "unsupported GCC nested-function extension"
    return None


def short_failure(result: subprocess.CompletedProcess[str]) -> str:
    text = (result.stderr or result.stdout).strip()
    if not text:
        return f"exit status {result.returncode}"
    for line in text.splitlines():
        if "timed out after" in line:
            return line[:240]
    first = text.splitlines()[0]
    return first[:240]


def failure_matches_expected(result: subprocess.CompletedProcess[str], expected: str) -> bool:
    return (
        expected in (result.stderr or "")
        or expected in (result.stdout or "")
        or expected in short_failure(result)
    )


def test_path_for_log(suite: Path, src: Path) -> str:
    try:
        rel = src.relative_to(suite)
    except ValueError:
        rel = Path(src.name)
    test = rel.as_posix()
    validate_test_path(src, 1, test)
    return normalize_test_path(test)


def write_skip_log(path: Path | None, suite: Path, skipped: list[tuple[Path, str]]) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(f"{test_path_for_log(suite, src)}\tSKIP: {reason}\n" for src, reason in skipped),
        encoding="utf-8",
    )


def validate_output_path(path: Path | None, label: str) -> None:
    if path is None:
        return
    if path.exists() and not path.is_file():
        raise SystemExit(f"{label} path is not a file: {path}")
    if path.parent.exists() and not path.parent.is_dir():
        raise SystemExit(f"{label} parent path is not a directory: {path.parent}")


def validate_output_dir(path: Path | None, label: str) -> None:
    if path is None:
        return
    if path.exists() and not path.is_dir():
        raise SystemExit(f"{label} path is not a directory: {path}")
    if path.parent.exists() and not path.parent.is_dir():
        raise SystemExit(f"{label} parent path is not a directory: {path.parent}")


def ensure_rnqcc(path: Path) -> None:
    if not path.exists():
        if path == DEFAULT_RNQCC:
            subprocess.run(["cargo", "build"], cwd=ROOT, check=True)
        else:
            raise SystemExit(f"--rnqcc not found: {path}")
    if not path.exists():
        raise SystemExit(f"--rnqcc not found after build: {path}")
    if not path.is_file():
        raise SystemExit(f"--rnqcc path is not a file: {path}")


def validate_numeric_args(args: argparse.Namespace) -> None:
    if args.start < 0:
        raise SystemExit("--start must be non-negative")
    if args.limit <= 0:
        raise SystemExit("--limit must be positive")
    if args.timeout <= 0:
        raise SystemExit("--timeout must be positive")
    if args.max_failures < 0:
        raise SystemExit("--max-failures must be non-negative")
    if args.progress_every < 0:
        raise SystemExit("--progress-every must be non-negative")


def save_failure_artifact(
    artifact_dir: Path | None,
    suite: Path,
    index: int,
    src: Path,
    cmd: list[str],
    result: subprocess.CompletedProcess[str],
    kind: str = "failures",
) -> None:
    if artifact_dir is None:
        return
    dest = artifact_dir / "gcc_torture" / kind / f"{index:04d}-{src.stem}"
    dest.mkdir(parents=True, exist_ok=True)
    (dest / "command.txt").write_text(
        " ".join(shlex.quote(part) for part in cmd) + "\n",
        encoding="utf-8",
    )
    (dest / "output.txt").write_text(
        (result.stdout or "") + (result.stderr or ""),
        encoding="utf-8",
    )
    (dest / "source-path.txt").write_text(test_path_for_log(suite, src) + "\n", encoding="utf-8")
    try:
        (dest / src.name).write_bytes(src.read_bytes())
    except OSError as err:
        (dest / "copy-error.txt").write_text(str(err), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run a bounded rnqcc smoke pass over GCC C torture tests."
    )
    parser.add_argument("--rnqcc", default=str(DEFAULT_RNQCC))
    parser.add_argument(
        "--suite",
        type=Path,
        help="gcc.c-torture suite path; defaults to common local and CI sparse-checkout paths",
    )
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
        "--skip-log",
        type=Path,
        help="write all skipped tests and reasons to this file",
    )
    parser.add_argument(
        "--print-skips",
        action="store_true",
        help="print every skipped test and reason to stdout",
    )
    parser.add_argument(
        "--progress-every",
        type=int,
        default=0,
        help="print progress after this many selected tests; 0 disables progress output",
    )
    parser.add_argument(
        "--expected-failures",
        type=Path,
        help="file of `relative/path.c | diagnostic substring` failures to treat as known",
    )
    parser.add_argument(
        "--expected-skips",
        type=Path,
        help=(
            "file of `mode | preprocessor | relative/path.c | reason` skips "
            "to treat as known"
        ),
    )
    parser.add_argument(
        "--allow-stale-expected-failures",
        action="store_true",
        help="do not fail if an expected failure passes in this run",
    )
    parser.add_argument(
        "--allow-stale-expected-skips",
        action="store_true",
        help="do not fail if an expected skip is selected but no longer skipped",
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
    validate_numeric_args(args)
    suite = resolve_suite(args.suite)
    validate_output_path(args.failure_log, "--failure-log")
    validate_output_path(args.skip_log, "--skip-log")
    validate_output_dir(args.artifact_dir, "--artifact-dir")
    ensure_rnqcc(rnqcc)

    tests = tests_for_mode(suite, args.mode)
    selected = tests[args.start : args.start + args.limit]
    if not selected:
        raise SystemExit("no tests selected")
    expected_failures = load_expected_failures(args.expected_failures)
    expected_skips = load_expected_skips(args.expected_skips)
    preprocessor = "internal-cpp" if args.internal_cpp else "external"

    passed = 0
    skipped: list[tuple[Path, str]] = []
    failures: list[tuple[Path, str]] = []
    expected_failed: list[tuple[Path, str]] = []
    stale_expected: list[tuple[str, str]] = []
    unexpected_skips: list[tuple[str, str]] = []
    seen_skip_keys: set[tuple[str, str, str]] = set()
    with tempfile.TemporaryDirectory(prefix="rnqcc-gcc-torture.") as tmp:
        tmpdir = Path(tmp)
        for offset, src in enumerate(selected):
            idx = args.start + offset
            rel = test_path_for_log(suite, src)
            if reason := skip_reason_for_test(src, args.internal_cpp):
                key = (args.mode, preprocessor, rel)
                seen_skip_keys.add(key)
                expected_skip_reason = expected_skips.get(key)
                if expected_skip_reason is not None and expected_skip_reason != reason:
                    failures.append(
                        (
                            src,
                            "expected skip reason changed: "
                            f"fixture has `{expected_skip_reason}`, runner produced `{reason}`",
                        )
                    )
                elif expected_skip_reason is None and args.expected_skips is not None:
                    unexpected_skips.append((rel, reason))
                skipped.append((src, reason))
                if args.progress_every > 0 and (offset + 1) % args.progress_every == 0:
                    print(
                        f"progress {offset + 1}/{len(selected)} selected: "
                        f"passed={passed}, skipped={len(skipped)}, "
                        f"xfail={len(expected_failed)}, failed={len(failures)}",
                        flush=True,
                    )
                continue
            stem = f"{idx:04d}-{src.stem}"
            common = [str(rnqcc), "--Wno-missing-return"]
            common.extend(rnqcc_options_for_test(src))
            if target := rnqcc_target_for_test(src):
                common.extend(["--target", target])
            timeout = timeout_for_test(src, args.timeout)
            if args.internal_cpp or use_internal_cpp_for_test(src):
                common.append("--internal-cpp")
            compile_src = materialize_source_for_test(src, tmpdir, idx)

            if args.mode == "execute":
                exe = tmpdir / stem
                compile_cmd = [*common, str(compile_src), "-o", str(exe), "-lm"]
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
                    if expected is not None and failure_matches_expected(result, expected):
                        expected_failed.append((src, failure))
                        save_failure_artifact(
                            args.artifact_dir, suite, idx, src, cmd, result, "xfail"
                        )
                    else:
                        failures.append((src, failure))
                        save_failure_artifact(args.artifact_dir, suite, idx, src, cmd, result)

            else:
                if rnqcc_target_for_test(src):
                    out = tmpdir / f"{stem}.s"
                    cmd = [*common, "-S", str(compile_src), "-o", str(out)]
                else:
                    out = tmpdir / f"{stem}.o"
                    cmd = [*common, "-c", str(compile_src), "-o", str(out)]
                result = run(cmd, timeout)
                required_failure = required_failure_for_test(src)
                if result.returncode == 0:
                    if required_failure is not None:
                        failure = f"missing expected diagnostic: {required_failure}"
                        failures.append((src, failure))
                        save_failure_artifact(args.artifact_dir, suite, idx, src, cmd, result)
                        continue
                    required_warning = required_warning_for_test(src)
                    if required_warning is not None and required_warning not in (
                        (result.stderr or "") + (result.stdout or "")
                    ):
                        failure = f"missing expected warning: {required_warning}"
                        failures.append((src, failure))
                        save_failure_artifact(args.artifact_dir, suite, idx, src, cmd, result)
                    else:
                        passed += 1
                        if rel in expected_failures:
                            stale_expected.append((rel, expected_failures[rel]))
                else:
                    if required_failure is not None:
                        output = (result.stderr or "") + (result.stdout or "")
                        if required_failure in output:
                            passed += 1
                            if rel in expected_failures:
                                stale_expected.append((rel, expected_failures[rel]))
                        else:
                            failure = short_failure(result)
                            failures.append((src, failure))
                            save_failure_artifact(args.artifact_dir, suite, idx, src, cmd, result)
                        continue
                    failure = short_failure(result)
                    expected = expected_failures.get(rel)
                    if expected is not None and failure_matches_expected(result, expected):
                        expected_failed.append((src, failure))
                        save_failure_artifact(
                            args.artifact_dir, suite, idx, src, cmd, result, "xfail"
                        )
                    else:
                        failures.append((src, failure))
                        save_failure_artifact(args.artifact_dir, suite, idx, src, cmd, result)

            if args.progress_every > 0 and (offset + 1) % args.progress_every == 0:
                print(
                    f"progress {offset + 1}/{len(selected)} selected: "
                    f"passed={passed}, skipped={len(skipped)}, "
                    f"xfail={len(expected_failed)}, failed={len(failures)}",
                    flush=True,
                )

    selected_rels = {test_path_for_log(suite, src) for src in selected}
    selected_expected_skips = {
        key: reason
        for key, reason in expected_skips.items()
        if key[0] == args.mode and key[1] == preprocessor and key[2] in selected_rels
    }
    stale_expected_skips = [
        (key[2], reason)
        for key, reason in selected_expected_skips.items()
        if key not in seen_skip_keys
    ]

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
    write_skip_log(args.skip_log, suite, skipped)
    if args.print_skips:
        for src, reason in skipped:
            print(f"SKIP {test_path_for_log(suite, src)}: {reason}")
    if args.failure_log:
        args.failure_log.parent.mkdir(parents=True, exist_ok=True)
        args.failure_log.write_text(
            "".join(
                f"{test_path_for_log(suite, src)}\t{reason}\n" for src, reason in failures
            )
            + "".join(
                f"{rel}\tSTALE-XFAIL: {reason}\n" for rel, reason in stale_expected
            )
            + "".join(
                f"{test_path_for_log(suite, src)}\tXFAIL: {reason}\n"
                for src, reason in expected_failed
            )
            + "".join(
                f"{test_path_for_log(suite, src)}\tSKIP: {reason}\n" for src, reason in skipped
            )
            + "".join(
                f"{rel}\tUNEXPECTED-SKIP: {reason}\n" for rel, reason in unexpected_skips
            )
            + "".join(
                f"{rel}\tSTALE-SKIP: {reason}\n" for rel, reason in stale_expected_skips
            )
        )

    shown = failures if args.max_failures == 0 else failures[: args.max_failures]
    for src, reason in shown:
        rel = test_path_for_log(suite, src)
        print(f"FAIL {rel}: {reason}")
    if args.max_failures != 0 and len(failures) > args.max_failures:
        print(f"... {len(failures) - args.max_failures} more failures")
    for rel, reason in stale_expected:
        print(f"STALE-XFAIL {rel}: {reason}")
    for rel, reason in unexpected_skips:
        print(f"UNEXPECTED-SKIP {rel}: {reason}")
    for rel, reason in stale_expected_skips:
        print(f"STALE-SKIP {rel}: {reason}")
    return (
        1
        if failures
        or unexpected_skips
        or (stale_expected and not args.allow_stale_expected_failures)
        or (stale_expected_skips and not args.allow_stale_expected_skips)
        else 0
    )


if __name__ == "__main__":
    sys.exit(main())
