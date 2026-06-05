#!/usr/bin/env python3
"""Deterministic black-box fuzz smoke runner for rnqcc.

This intentionally avoids external fuzzing tools. It generates small valid C
programs from a seed and invokes the rnqcc CLI through its public stages.
"""

from __future__ import annotations

import argparse
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

from smoke_utils import env_timeout, timeout_text


STAGES = ("lex", "parse", "validate", "tacky", "codegen")


class ProgramGenerator:
    def __init__(self, seed: int, case: int) -> None:
        self.seed = seed
        self.case = case
        self.rng = random.Random((seed << 32) ^ case)

    def const(self) -> str:
        return str(self.rng.randint(-20, 20))

    def expr(self, vars: list[str], depth: int) -> str:
        if depth <= 0 or self.rng.random() < 0.35:
            return self.rng.choice(vars + [self.const()])

        op = self.rng.choice(["+", "-", "*", "&", "|", "^", "<", "<=", ">", ">=", "==", "!="])
        left = self.expr(vars, depth - 1)
        right = self.expr(vars, depth - 1)
        return f"({left} {op} {right})"

    def assignment(self, vars: list[str], depth: int) -> str:
        dst = self.rng.choice(vars)
        return f"{dst} = {self.expr(vars, depth)};"

    def function(self) -> str:
        vars = ["x", "y", "z"]
        lines = [
            "int helper(int x, int y) {",
            f"    int z = {self.expr(['x', 'y'], 2)};",
        ]
        for _ in range(self.rng.randint(1, 3)):
            lines.append(f"    {self.assignment(vars, 2)}")
        lines.extend(
            [
                f"    if ({self.expr(vars, 2)}) {{",
                f"        {self.assignment(vars, 2)}",
                "    } else {",
                f"        {self.assignment(vars, 2)}",
                "    }",
                f"    return {self.expr(vars, 2)};",
                "}",
            ]
        )
        return "\n".join(lines)

    def main(self) -> str:
        vars = ["a", "b", "c", "i"]
        loop_limit = self.rng.randint(1, 5)
        lines = [
            "int main(void) {",
            f"    int a = {self.const()};",
            f"    int b = {self.const()};",
            f"    int c = helper(a, b);",
            "    int i = 0;",
            f"    while (i < {loop_limit}) {{",
            f"        {self.assignment(vars, 2)}",
            "        i = i + 1;",
            "    }",
            f"    if ({self.expr(vars, 2)}) {{",
            f"        {self.assignment(vars, 2)}",
            "    } else {",
            f"        {self.assignment(vars, 2)}",
            "    }",
            f"    return {self.expr(vars, 3)};",
            "}",
        ]
        return "\n".join(lines)

    def generate(self) -> str:
        return "\n\n".join(
            [
                f"/* rnqcc fuzz smoke seed={self.seed} case={self.case} */",
                self.function(),
                self.main(),
                "",
            ]
        )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate deterministic small C programs and compile them through rnqcc stages."
    )
    parser.add_argument("--seed", type=int, required=True, help="integer seed for generated cases")
    parser.add_argument("--cases", type=int, default=8, help="number of cases to generate")
    parser.add_argument(
        "--rnqcc",
        default=os.environ.get("RNQCC", "target/debug/rnqcc"),
        help="path to rnqcc binary, or set RNQCC",
    )
    parser.add_argument(
        "--cc",
        default=os.environ.get("CC", "cc"),
        help="reference C compiler for --compare-runtime, or set CC",
    )
    parser.add_argument(
        "--target",
        action="append",
        dest="targets",
        help="target triple/alias to exercise; repeatable. Defaults to rnqcc --print-targets.",
    )
    parser.add_argument(
        "--rnqcc-arg",
        action="append",
        dest="rnqcc_args",
        default=[],
        help="extra argument to pass to rnqcc for every compile invocation; repeatable.",
    )
    parser.add_argument(
        "--work-dir",
        type=Path,
        help="directory for generated sources and assembly outputs",
    )
    parser.add_argument(
        "--keep-successes",
        action="store_true",
        help="keep generated sources for passing cases",
    )
    parser.add_argument(
        "--emit-only",
        action="store_true",
        help="only write generated C inputs; do not run rnqcc",
    )
    parser.add_argument(
        "--compare-runtime",
        action="store_true",
        help="compile each generated case with rnqcc and cc, run both, and compare exit status.",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=env_timeout("FUZZ_SMOKE_TIMEOUT", "10.0"),
        help="seconds to allow each rnqcc invocation, or set FUZZ_SMOKE_TIMEOUT",
    )
    return parser.parse_args(argv)


def run(cmd: list[str], cwd: Path, timeout: float) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            cmd,
            cwd=cwd,
            text=True,
            capture_output=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        return subprocess.CompletedProcess(
            cmd,
            124,
            stdout=timeout_text(exc.stdout),
            stderr=(timeout_text(exc.stderr) + f"\ntimed out after {timeout:.1f}s").lstrip(),
        )


def discover_targets(rnqcc: str, cwd: Path, timeout: float) -> list[str]:
    result = run([rnqcc, "--print-targets"], cwd, timeout)
    if result.returncode != 0:
        raise RuntimeError(
            "could not discover targets with --print-targets\n"
            + result.stderr.strip()
        )
    targets = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    if not targets:
        raise RuntimeError("rnqcc --print-targets returned no targets")
    return targets


def compile_case(
    rnqcc: str,
    extra_args: list[str],
    src: Path,
    targets: list[str],
    cwd: Path,
    timeout: float,
) -> None:
    for stage in STAGES:
        if stage == "codegen":
            for target in targets:
                check_run(
                    [rnqcc, "--target", target, *extra_args, "--stage", stage, str(src)],
                    cwd,
                    timeout,
                )
        else:
            check_run([rnqcc, *extra_args, "--stage", stage, str(src)], cwd, timeout)

    for target in targets:
        asm = src.with_suffix(f".{target}.s")
        check_run(
            [rnqcc, "--target", target, *extra_args, "-S", "-o", str(asm), str(src)],
            cwd,
            timeout,
        )


def compare_runtime(
    rnqcc: str,
    cc: str,
    extra_args: list[str],
    src: Path,
    work_dir: Path,
    cwd: Path,
    timeout: float,
) -> None:
    host_exe = work_dir / f"{src.stem}.host"
    rnqcc_exe = work_dir / f"{src.stem}.rnqcc"
    check_run([cc, str(src), "-o", str(host_exe)], cwd, timeout)
    check_run([rnqcc, *extra_args, str(src), "-o", str(rnqcc_exe)], cwd, timeout)

    host = run([str(host_exe)], cwd, timeout)
    rnqcc_result = run([str(rnqcc_exe)], cwd, timeout)
    if host.returncode != rnqcc_result.returncode:
        raise RuntimeError(
            "runtime mismatch: "
            f"cc exited {host.returncode}, rnqcc exited {rnqcc_result.returncode}"
        )


def check_run(cmd: list[str], cwd: Path, timeout: float) -> None:
    result = run(cmd, cwd, timeout)
    if result.returncode == 0:
        return

    rendered = " ".join(cmd)
    message = [f"command failed: {rendered}"]
    if result.stderr.strip():
        message.append(result.stderr.rstrip())
    if result.stdout.strip():
        message.append(result.stdout.rstrip())
    raise RuntimeError("\n".join(message))


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.cases <= 0:
        print("--cases must be positive", file=sys.stderr)
        return 1
    if args.timeout <= 0:
        print("--timeout must be positive", file=sys.stderr)
        return 1
    repo = Path.cwd()
    work_dir = args.work_dir or Path(tempfile.mkdtemp(prefix=f"rnqcc-fuzz-smoke-{args.seed}-"))
    work_dir.mkdir(parents=True, exist_ok=True)

    rnqcc = args.rnqcc
    targets = args.targets or ([] if args.emit_only else discover_targets(rnqcc, repo, args.timeout))

    for case in range(args.cases):
        src = work_dir / f"seed_{args.seed}_case_{case}.i"
        src.write_text(ProgramGenerator(args.seed, case).generate(), encoding="utf-8")

        try:
            if not args.emit_only:
                compile_case(rnqcc, args.rnqcc_args, src, targets, repo, args.timeout)
                if args.compare_runtime:
                    compare_runtime(
                        rnqcc,
                        args.cc,
                        args.rnqcc_args,
                        src,
                        work_dir,
                        repo,
                        args.timeout,
                    )
        except Exception as exc:
            print(f"FAIL seed={args.seed} case={case} src={src}", file=sys.stderr)
            print(exc, file=sys.stderr)
            return 1

        if not args.keep_successes and not args.emit_only:
            src.unlink(missing_ok=True)
            for asm in work_dir.glob(f"{src.stem}.*.s"):
                asm.unlink(missing_ok=True)
            for exe in work_dir.glob(f"{src.stem}.*"):
                exe.unlink(missing_ok=True)

    if not args.keep_successes and not args.emit_only:
        try:
            work_dir.rmdir()
        except OSError:
            pass

    print(f"ok seed={args.seed} cases={args.cases} targets={','.join(targets) if targets else 'none'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
