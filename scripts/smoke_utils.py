"""Shared helpers for smoke-test scripts."""

from __future__ import annotations

import math
import os
import signal
import subprocess


def env_timeout(name: str, default: str) -> float:
    try:
        value = float(os.environ.get(name, default))
    except ValueError:
        raise SystemExit(f"{name} must be a number")
    if not is_positive_finite(value):
        raise SystemExit(f"{name} must be a finite positive number")
    return value


def is_positive_finite(value: float) -> bool:
    return math.isfinite(value) and value > 0


def positive_finite_float(value: str) -> float:
    """Argparse type for timeout values that must be usable immediately."""
    try:
        parsed = float(value)
    except ValueError as exc:
        raise ValueError("must be a number") from exc
    if not is_positive_finite(parsed):
        raise ValueError("must be a finite positive number")
    return parsed


def timeout_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value


def kill_process(process: subprocess.Popen[str]) -> None:
    """Best-effort direct-child termination for timeout cleanup races."""
    try:
        process.kill()
    except (PermissionError, ProcessLookupError):
        pass


def kill_process_tree(process: subprocess.Popen[str], use_process_group: bool) -> None:
    """Best-effort termination of a timed-out process and its descendants."""
    if use_process_group:
        try:
            os.killpg(process.pid, signal.SIGKILL)
            return
        except ProcessLookupError:
            return
        except PermissionError:
            kill_process(process)
            return

    if os.name == "nt":
        # taskkill is part of supported Windows installations and can
        # terminate the complete process tree by PID.
        try:
            subprocess.run(
                ["taskkill", "/T", "/F", "/PID", str(process.pid)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=1.0,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass
    kill_process(process)


def run_with_timeout(
    cmd: list[str],
    *,
    timeout: float,
    cwd: str | os.PathLike[str] | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run a command and terminate its descendants when it times out."""
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("timeout must be a finite positive number")
    use_process_group = hasattr(os, "killpg")
    popen_options: dict[str, object] = {}
    if use_process_group:
        popen_options["start_new_session"] = True
    elif os.name == "nt":
        popen_options["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    process = subprocess.Popen(
        cmd,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        errors="replace",
        **popen_options,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        return subprocess.CompletedProcess(cmd, process.returncode, stdout=stdout, stderr=stderr)
    except subprocess.TimeoutExpired as exc:
        kill_process_tree(process, use_process_group)
        try:
            stdout, stderr = process.communicate(timeout=1.0)
        except subprocess.TimeoutExpired:
            # A descendant may still hold the pipe open after the leader was
            # killed.  Preserve the output collected by the timed-out call and
            # close our pipe ends instead of hanging indefinitely.  Reap the
            # leader explicitly as well; otherwise repeated smoke timeouts can
            # accumulate zombies even though their process groups were killed.
            stdout, stderr = "", ""
            if process.stdout is not None:
                process.stdout.close()
            if process.stderr is not None:
                process.stderr.close()
            try:
                process.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                kill_process(process)
                try:
                    process.wait(timeout=1.0)
                except subprocess.TimeoutExpired:
                    pass
        return subprocess.CompletedProcess(
            cmd,
            124,
            stdout=timeout_text(exc.stdout) + (stdout or ""),
            stderr=(timeout_text(exc.stderr) + (stderr or "") + f"\ntimed out after {timeout:.1f}s").lstrip(),
        )
