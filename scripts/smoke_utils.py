"""Shared helpers for smoke-test scripts."""

from __future__ import annotations

import os


def env_timeout(name: str, default: str) -> float:
    try:
        return float(os.environ.get(name, default))
    except ValueError:
        raise SystemExit(f"{name} must be a number")


def timeout_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode(errors="replace")
    return value
