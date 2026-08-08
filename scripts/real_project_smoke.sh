#!/usr/bin/env sh
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPILER="${COMPILER:-$ROOT/target/debug/rnqcc}"
AR="${AR:-ar}"
WORKDIR="${WORKDIR:-}"
KEEP_WORKDIR="${KEEP_WORKDIR:-0}"
CI_ARTIFACT_DIR="${CI_ARTIFACT_DIR:-}"
REAL_PROJECT_DIR="${REAL_PROJECT_DIR:-}"
REAL_PROJECT_CFLAGS="${REAL_PROJECT_CFLAGS:-}"
WORKDIR_CREATED=0
if [ -z "${WORKDIR:-}" ]; then
    WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/rnqcc-real-smoke.XXXXXX")"
    WORKDIR_CREATED=1
fi

cleanup() {
    status=$?
    if [ "$status" -ne 0 ] && [ -n "$CI_ARTIFACT_DIR" ]; then
        mkdir -p "$CI_ARTIFACT_DIR"
        cp -R "$WORKDIR" "$CI_ARTIFACT_DIR/real_project_smoke"
    fi
    if [ "$WORKDIR_CREATED" = "1" ] && [ "$KEEP_WORKDIR" != "1" ]; then
        rm -rf "$WORKDIR"
    fi
    exit "$status"
}
trap cleanup EXIT

if [ ! -x "$COMPILER" ]; then
    cargo build --locked --manifest-path "$ROOT/Cargo.toml" >/dev/null
fi

INCLUDE="$ROOT/tests/fixtures/smoke/include"
PROJECT="$ROOT/tests/fixtures/smoke/project"
STATICLIB="$ROOT/tests/fixtures/smoke/staticlib"

mkdir -p "$WORKDIR/project" "$WORKDIR/staticlib" "$WORKDIR/response space"

"$COMPILER" --internal-cpp -I "$INCLUDE" -MMD -MP -MF "$WORKDIR/project/main.d" \
    -c "$PROJECT/main.c" -o "$WORKDIR/project/main.o"
"$COMPILER" --internal-cpp -I "$INCLUDE" -MMD -MP -MF "$WORKDIR/project/util.d" \
    -c "$PROJECT/util.c" -o "$WORKDIR/project/util.o"
"$COMPILER" "$WORKDIR/project/main.o" "$WORKDIR/project/util.o" -o "$WORKDIR/project/app"
set +e
"$WORKDIR/project/app"
status=$?
set -e
if [ "$status" -ne 37 ]; then
    echo "project smoke returned $status, expected 37" >&2
    exit 1
fi
grep -q "smoke_config.h" "$WORKDIR/project/main.d"

"$COMPILER" --internal-cpp -c "$STATICLIB/lib.c" -o "$WORKDIR/staticlib/lib.o"
"$AR" rcs "$WORKDIR/staticlib/libsmoke.a" "$WORKDIR/staticlib/lib.o"
"$COMPILER" --internal-cpp "$STATICLIB/main.c" "$WORKDIR/staticlib/libsmoke.a" \
    -o "$WORKDIR/staticlib/app"
set +e
"$WORKDIR/staticlib/app"
status=$?
set -e
if [ "$status" -ne 37 ]; then
    echo "static library smoke returned $status, expected 37" >&2
    exit 1
fi

cat > "$WORKDIR/response space/nested.rsp" <<EOF
--internal-cpp -I "$INCLUDE"
EOF
cat > "$WORKDIR/response space/compile.rsp" <<EOF
@"$WORKDIR/response space/nested.rsp" -S -o "$WORKDIR/response space/main.s" "$PROJECT/main.c"
EOF
"$COMPILER" @"$WORKDIR/response space/compile.rsp"
test -s "$WORKDIR/response space/main.s"

if [ -n "$REAL_PROJECT_DIR" ]; then
    if [ ! -d "$REAL_PROJECT_DIR" ]; then
        echo "REAL_PROJECT_DIR is not a directory: $REAL_PROJECT_DIR" >&2
        exit 1
    fi
    mkdir -p "$WORKDIR/real-project"
    if ! find "$REAL_PROJECT_DIR" -name '*.c' -type f -print -quit | grep -q .; then
        echo "REAL_PROJECT_DIR contains no .c files: $REAL_PROJECT_DIR" >&2
        exit 1
    fi
    if ! find "$REAL_PROJECT_DIR" -name '*.c' -type f -exec sh -c '
        compiler=$1
        output_dir=$2
        flags=$3
        shift 3
        for source do
            # cksum supplies a stable, path-derived name without interpreting
            # whitespace or newlines in the source path.
            key=$(printf "%s" "$source" | cksum | awk "{print \$1 \"_\" \$2}") || exit 1
            object="$output_dir/$key.o"
            # shellcheck disable=SC2086
            "$compiler" --internal-cpp $flags -c "$source" -o "$object" || exit 1
            test -s "$object" || exit 1
        done
    ' sh "$COMPILER" "$WORKDIR/real-project" "$REAL_PROJECT_CFLAGS" {} +; then
        echo "failed to compile one or more .c files in REAL_PROJECT_DIR: $REAL_PROJECT_DIR" >&2
        exit 1
    fi
fi

echo "real project smoke passed"
