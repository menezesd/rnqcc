"""Shared GCC torture expected-failure fixture parsing."""

from __future__ import annotations

from pathlib import Path, PurePosixPath, PureWindowsPath


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_EXPECTED = ROOT / "tests" / "fixtures" / "gcc_torture_expected_failures.txt"
MAX_REASON_DISPLAY = 240
FailureRow = tuple[str, str, str]


def validate_test_path(path: Path, line_no: int, test: str) -> None:
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in test):
        raise SystemExit(f"{path}:{line_no}: control character in test path")
    posix_path = PurePosixPath(test)
    windows_path = PureWindowsPath(test)
    if (
        posix_path.is_absolute()
        or windows_path.is_absolute()
        or ".." in posix_path.parts
        or ".." in windows_path.parts
    ):
        raise SystemExit(f"{path}:{line_no}: expected relative test path, got {test}")
    raw_parts = test.replace("\\", "/").split("/")
    if any(part in ("", ".") for part in raw_parts):
        raise SystemExit(f"{path}:{line_no}: expected normalized test path, got {test}")


def normalize_test_path(test: str) -> str:
    return "/".join(PureWindowsPath(test).parts)


def display_reason(reason: str) -> str:
    if len(reason) <= MAX_REASON_DISPLAY:
        return reason
    return reason[: MAX_REASON_DISPLAY - 3] + "..."


def load_expected_failures(path: Path | None) -> dict[str, str]:
    if path is None:
        return {}
    if not path.exists():
        raise SystemExit(f"{path}: expected-failure fixture not found")
    if not path.is_file():
        raise SystemExit(f"{path}: expected-failure fixture is not a file")

    expected: dict[str, str] = {}
    previous_test: str | None = None
    for line_no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.split("#", 1)[0].rstrip()
        if not line.strip():
            continue
        fields = line.split("|", 1)
        if len(fields) != 2:
            raise SystemExit(f"{path}:{line_no}: expected '<test> | <reason>', got {raw!r}")
        test, reason = fields[0].rstrip(), fields[1].strip()
        if not test or not reason:
            raise SystemExit(f"{path}:{line_no}: expected '<test> | <reason>', got {raw!r}")
        if fields[0] != fields[0].lstrip():
            raise SystemExit(f"{path}:{line_no}: whitespace around test name for {test}")
        validate_test_path(path, line_no, test)
        test = normalize_test_path(test)
        if test in expected:
            raise SystemExit(f"{path}:{line_no}: duplicate expected failure for {test}")
        if previous_test is not None and test < previous_test:
            raise SystemExit(
                f"{path}:{line_no}: expected failures must be sorted; "
                f"{test} should appear before {previous_test}"
            )
        expected[test] = reason
        previous_test = test
    return expected


def parse_failure_log(path: Path) -> list[FailureRow]:
    if not path.exists():
        raise SystemExit(f"{path}: failure log not found")
    if not path.is_file():
        raise SystemExit(f"{path}: failure log is not a file")

    rows: list[FailureRow] = []
    seen: set[str] = set()
    for line_no, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        if not raw.strip():
            continue
        if "\t" not in raw:
            raise SystemExit(f"{path}:{line_no}: missing tab separator in failure log")
        test, status = raw.split("\t", 1)
        if not test.strip():
            raise SystemExit(f"{path}:{line_no}: missing test name in failure log")
        if test != test.strip():
            raise SystemExit(f"{path}:{line_no}: whitespace around test name in failure log")
        validate_test_path(path, line_no, test)
        test = normalize_test_path(test)
        if not status.strip():
            raise SystemExit(f"{path}:{line_no}: missing status in failure log for {test}")
        if "\t" in status:
            raise SystemExit(f"{path}:{line_no}: unexpected tab in failure log status for {test}")
        if status != status.strip():
            raise SystemExit(f"{path}:{line_no}: whitespace around status in failure log for {test}")
        if test in seen:
            raise SystemExit(f"{path}:{line_no}: duplicate failure log row for {test}")
        seen.add(test)
        if status.startswith("SKIP:"):
            reason = status.removeprefix("SKIP:").strip()
            if not reason:
                raise SystemExit(f"{path}:{line_no}: missing reason in failure log for {test}")
            continue
        if status.startswith("STALE-XFAIL:"):
            reason = status.removeprefix("STALE-XFAIL:").strip()
            if not reason:
                raise SystemExit(f"{path}:{line_no}: missing reason in failure log for {test}")
            rows.append((test, "stale", reason))
        elif status.startswith("XFAIL:"):
            reason = status.removeprefix("XFAIL:").strip()
            if not reason:
                raise SystemExit(f"{path}:{line_no}: missing reason in failure log for {test}")
            rows.append((test, "xfail", reason))
        elif status.startswith("FAIL:"):
            reason = status.removeprefix("FAIL:").strip()
            if not reason:
                raise SystemExit(f"{path}:{line_no}: missing reason in failure log for {test}")
            rows.append((test, "fail", reason))
        else:
            rows.append((test, "fail", status.strip()))
    return rows
