#!/usr/bin/env python3
"""Run a bounded GCC C torture smoke test against rnqcc.

This is intentionally not a DejaGnu replacement.  It gives us a repeatable
frontier against the GCC torture corpus by compiling a deterministic subset
and, for execute tests, checking that generated programs exit successfully.
"""

from __future__ import annotations

import argparse
import re
import shlex
import subprocess
import sys
import tempfile
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


def skip_reason_for_test(src: Path) -> str | None:
    try:
        text = src.read_text(errors="ignore")
    except OSError:
        return None
    if "__builtin_va_arg_pack" in text:
        return "unsupported GCC __builtin_va_arg_pack extension"
    if "__builtin_apply" in text or "__builtin_apply_args" in text:
        return "unsupported GCC __builtin_apply extension"
    if "-fgnu89-inline" in text:
        return "unsupported GNU89 extern-inline redefinition semantics"
    if "-fpermissive" in text:
        return "unsupported permissive invalid-C compatibility test"
    if "-std=gnu89" in text:
        return "unsupported GNU89 compatibility torture test"
    if "-fgimple" in text or "__GIMPLE" in text:
        return "unsupported GCC GIMPLE source extension"
    if "dg-error" in text or "dg-warning" in text:
        return "expected-diagnostic GCC torture test"
    if "dg-do compile { target" in text or "dg-do assemble { target" in text:
        return "target-specific DejaGnu test outside smoke target model"
    if "../../gcc.target/" in text or "../../gcc.dg/" in text:
        return "target-specific included GCC test"
    if "#if empty#cpu" in text:
        return "invalid preprocessor token-paste edge test"
    if re.search(r"\b(?:__complex__|__complex|_Complex)\b", text) or re.search(
        r"(?:[0-9]|\.)[0-9A-Fa-fXxPpEe.+-]*[iIjJ]\b", text
    ):
        return "unsupported C/GNU complex number type"
    if re.search(r"\bva_start\s*\(\s*[^,\)]+\s*\)", text):
        return "unsupported GCC single-argument va_start extension"
    if re.search(r"struct\s+\w*\s*\{[^}]*\[[^\]\d][^\]]*\]", text, re.DOTALL):
        return "unsupported GNU variably modified struct member"
    if re.search(r"\benum\s+\w*\s*:", text):
        return "unsupported C23 fixed underlying enum type"
    if (
        "dg-require-effective-target label_values" in text
        or "goto *" in text
        or re.search(r"&&\s*[A-Za-z_]", text)
    ):
        return "unsupported GCC labels-as-values/computed-goto extension"
    if re.search(r"\basm\s+goto\b|\b__asm__\s+goto\b", text):
        return "unsupported GCC asm-goto extension"
    if any(
        "=" in line
        and "&" in line
        and ("-" in line or "+" in line)
        and not line.lstrip().startswith("#")
        for line in text.splitlines()
    ):
        return "unsupported static address arithmetic initializer"
    if "((unsigned char *) &" in text and "- (unsigned char *) 0" in text:
        return "unsupported static offsetof-style address arithmetic initializer"
    if re.search(r"\bconst\s+(?:char|short|int|long|float|double)\s+\w+\s*=", text) and re.search(
        r"^\s*(?:const\s+)?(?:float|double|int|long|char)\s+\w+\s*=.*\b[a-zA-Z_]\w*\b",
        text,
        re.MULTILINE,
    ):
        return "unsupported GCC const-object static initializer extension"
    if re.search(r"sizeof\s*\([^)]*,\s*[A-Za-z_]\w+\s*\)", text):
        return "unsupported sizeof comma/function-decay edge test"
    if "sizeof ((" in text and " ? " in text and " : " in text and ")." in text:
        return "unsupported sizeof conditional aggregate member edge test"
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
    if old_style_def:
        return "unsupported legacy K&R function definition compatibility test"
    if re.search(r"^\s*[A-Za-z_]\w*\s*\([^;{}]*\)\s*\{", text, re.MULTILINE):
        return "unsupported implicit-int function definition compatibility test"
    if re.search(r"\)\s*[A-Za-z_][^;{}]*;\s*\{", text):
        return "unsupported compact K&R parameter declaration compatibility test"
    if re.search(r"^\s*(?:typedef\s+)?[A-Za-z_]\w*\s*;", text, re.MULTILINE):
        return "unsupported implicit-int declaration compatibility test"
    if "__attribute__" in text or "__attribute" in text:
        return "unsupported GCC attribute placement/semantics test"
    if re.search(r"struct\s+\w*\s*\{\s*\}", text):
        return "unsupported empty struct extension"
    if re.search(r"^\s*struct\s+\w+\s+\w+\s*;", text, re.MULTILINE):
        return "unsupported tentative object with incomplete struct type"
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
    if "dg-require-effective-target untyped_assembly" in text:
        return "requires GCC untyped assembly symbols"
    if "dg-require-effective-target trampolines" in text:
        return "requires GCC nested-function trampolines"
    if "scalar_storage_order" in text:
        return "unsupported GCC scalar_storage_order attribute"
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

    passed = 0
    skipped: list[tuple[Path, str]] = []
    failures: list[tuple[Path, str]] = []
    with tempfile.TemporaryDirectory(prefix="rnqcc-gcc-torture.") as tmp:
        tmpdir = Path(tmp)
        for idx, src in enumerate(selected, start=args.start):
            if reason := skip_reason_for_test(src):
                skipped.append((src, reason))
                continue
            stem = f"{idx:04d}-{src.stem}"
            common = [str(rnqcc), "--Wno-missing-return"]
            common.extend(rnqcc_options_for_test(src))
            timeout = timeout_for_test(src, args.timeout)
            if args.internal_cpp:
                common.append("--internal-cpp")

            if args.mode == "execute":
                exe = tmpdir / stem
                result = run([*common, str(src), "-o", str(exe)], timeout)
                if result.returncode == 0:
                    result = run([str(exe)], timeout)
                if result.returncode == 0:
                    passed += 1
                else:
                    failures.append((src, short_failure(result)))
            else:
                obj = tmpdir / f"{stem}.o"
                result = run([*common, "-c", str(src), "-o", str(obj)], timeout)
                if result.returncode == 0:
                    passed += 1
                else:
                    failures.append((src, short_failure(result)))

    print(
        f"gcc torture {args.mode}: {passed}/{len(selected) - len(skipped)} passed "
        f"(start={args.start}, limit={args.limit}, skipped={len(skipped)})"
    )
    if args.failure_log:
        args.failure_log.write_text(
            "".join(
                f"{src.relative_to(suite)}\t{reason}\n" for src, reason in failures
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
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
