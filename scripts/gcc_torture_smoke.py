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

from gcc_torture_expected import load_expected_failures, normalize_test_path, validate_test_path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RNQCC = ROOT / "target" / "debug" / "rnqcc"
DEFAULT_SUITE_CANDIDATES = [
    Path("/tmp/rnqcc-gcc-torture/gcc/testsuite/gcc.c-torture"),
    Path("/tmp/rnqcc-gcc-torture/gcc/gcc/testsuite/gcc.c-torture"),
]


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
            if option in {"-finstrument-functions", "-fpermissive"}:
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
    portable_target_execute_smoke = {
        "pr105777.c",
        "pr109938.c",
        "pr109986.c",
        "pr30314.c",
        "pr98304-2.c",
    }
    if (
        "../../gcc.target/" in text or "../../gcc.dg/" in text
    ) and not (src.parent.name == "execute" and src.name in portable_target_execute_smoke):
        return "target-specific included GCC test"
    if "#if empty#cpu" in text:
        return "invalid preprocessor token-paste edge test"
    complex_compile_gaps: dict[str, str] = {}
    complex_execute_gaps: dict[str, str] = {}
    if src.parent.name == "compile" and src.name in complex_compile_gaps:
        return complex_compile_gaps[src.name]
    if src.parent.name == "execute" and src.name in complex_execute_gaps:
        return complex_execute_gaps[src.name]
    portable_stress_compile_smoke = {
        "20000609-1.c",
        "20000804-1.c",
        "20020304-1.c",
        "20020604-1.c",
        "20021015-1.c",
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
        "limits-enumconst.c",
        "limits-externalid.c",
        "limits-fnargs.c",
        "limits-idexternal.c",
        "limits-idinternal.c",
        "limits-stringlit.c",
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
    if src.name != "20061220-1.c" and re.search(
        r"^\s+void\s+nested\w*\s*\(", text, re.MULTILINE
    ):
        return "unsupported GCC nested-function extension"
    if "gcc_tmpnam.h" in text and "dg-require-effective-target fileio" in text:
        return "requires tmpnam file I/O unavailable in this sandbox"
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
    return expected in (result.stderr or "") or expected in (result.stdout or "")


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
        "--allow-stale-expected-failures",
        action="store_true",
        help="do not fail if an expected failure passes in this run",
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

    passed = 0
    skipped: list[tuple[Path, str]] = []
    failures: list[tuple[Path, str]] = []
    expected_failed: list[tuple[Path, str]] = []
    stale_expected: list[tuple[str, str]] = []
    with tempfile.TemporaryDirectory(prefix="rnqcc-gcc-torture.") as tmp:
        tmpdir = Path(tmp)
        for offset, src in enumerate(selected):
            idx = args.start + offset
            rel = test_path_for_log(suite, src)
            if reason := skip_reason_for_test(src):
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
                    if expected is not None and failure_matches_expected(result, expected):
                        expected_failed.append((src, failure))
                        save_failure_artifact(
                            args.artifact_dir, suite, idx, src, cmd, result, "xfail"
                        )
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
        )

    shown = failures if args.max_failures == 0 else failures[: args.max_failures]
    for src, reason in shown:
        rel = test_path_for_log(suite, src)
        print(f"FAIL {rel}: {reason}")
    if args.max_failures != 0 and len(failures) > args.max_failures:
        print(f"... {len(failures) - args.max_failures} more failures")
    for rel, reason in stale_expected:
        print(f"STALE-XFAIL {rel}: {reason}")
    return 1 if failures or (stale_expected and not args.allow_stale_expected_failures) else 0


if __name__ == "__main__":
    sys.exit(main())
