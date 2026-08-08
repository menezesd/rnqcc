#!/bin/bash
# Run valid tests from writing-a-c-compiler-tests
# Usage: ./run_tests.sh [chapter_numbers...]

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TESTDIR="${TESTDIR:-$ROOT/../writing-a-c-compiler-tests/tests}"
COMPILER="${COMPILER:-$ROOT/target/debug/rnqcc}"
REF_CC="${REF_CC:-gcc}"
TEST_TIMEOUT="${RNQCC_TEST_TIMEOUT:-10}"
TIMEOUT_EXIT=124
RUNNER=()
GCC_ARCH=()
COMPILER_TARGET=()
HELPER_PLATFORM=linux
if [ "$(uname)" = "Darwin" ]; then
    RUNNER=(arch -x86_64)
    GCC_ARCH=(-arch x86_64)
    COMPILER_TARGET=(--target x86_64-macos)
    HELPER_PLATFORM=osx
fi
PASS=0
FAIL=0
ERRORS=""

if [ ! -x "$COMPILER" ]; then
    cargo build --locked --manifest-path "$ROOT/Cargo.toml" || exit 1
fi

if [ ! -d "$TESTDIR" ]; then
    echo "Missing test suite: $TESTDIR" >&2
    echo "Set TESTDIR=/path/to/writing-a-c-compiler-tests/tests" >&2
    exit 1
fi
TESTDIR="$(cd "$TESTDIR" && pwd)"

run_limited() {
    PYTHONDONTWRITEBYTECODE=1 python3 "$ROOT/scripts/run_command_timeout.py" \
        --timeout "$TEST_TIMEOUT" -- "$@"
}

case "$COMPILER" in
    /*) ;;
    */*) COMPILER="$(cd "$(dirname "$COMPILER")" && pwd)/$(basename "$COMPILER")" ;;
esac

cleanup_workdir() {
    cd "$ROOT" || exit 1
    [ -n "${WORKDIR:-}" ] && rm -rf "$WORKDIR"
}

run_single_test() {
    local src="$1"
    local name
    name=$(basename "$src" .c)

    local WORKDIR
    WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/rnqcc-test.XXXXXX") || exit 1
    trap cleanup_workdir RETURN
    cd "$WORKDIR" || exit 1

    # Check for helper libraries
    local chapter_dir
    chapter_dir=$(echo "$src" | sed 's|/valid/.*|/|')
    local helper_dir="${chapter_dir}helper_libs"
    local helpers=()
    add_helper() {
        local helper="$1"
        local existing
        for existing in "${helpers[@]}"; do
            [ "$existing" = "$helper" ] && return
        done
        helpers+=("$helper")
    }
    if [ -d "$helper_dir" ]; then
        for h in "$helper_dir"/${name}.c "$helper_dir"/${name}_*.c; do
            [ -f "$h" ] && add_helper "$h"
        done
        if grep -q 'double_isnan' "$src" && [ -f "$helper_dir/nan.c" ]; then
            add_helper "$helper_dir/nan.c"
        fi
    fi
    local src_dir
    src_dir=$(dirname "$src")
    for h in "$src_dir"/${name}_"$HELPER_PLATFORM".s "$src_dir"/${name}_*_"$HELPER_PLATFORM".s; do
        [ -f "$h" ] && add_helper "$h"
    done
    case "$name" in
        pass_args_on_page_boundary|push_arg_on_page_boundary|return_struct_on_page_boundary)
            [ -f "$src_dir/data_on_page_boundary_$HELPER_PLATFORM.s" ] \
                && add_helper "$src_dir/data_on_page_boundary_$HELPER_PLATFORM.s"
            ;;
        return_big_struct_on_page_boundary)
            [ -f "$src_dir/big_data_on_page_boundary_$HELPER_PLATFORM.s" ] \
                && add_helper "$src_dir/big_data_on_page_boundary_$HELPER_PLATFORM.s"
            ;;
        return_pointer_in_rax)
            [ -f "$src_dir/validate_return_pointer_$HELPER_PLATFORM.s" ] \
                && add_helper "$src_dir/validate_return_pointer_$HELPER_PLATFORM.s"
            ;;
        return_space_overlap)
            [ -f "$src_dir/return_space_address_overlap_$HELPER_PLATFORM.s" ] \
                && add_helper "$src_dir/return_space_address_overlap_$HELPER_PLATFORM.s"
            ;;
    esac

    if [ ${#helpers[@]} -gt 0 ]; then
        run_limited "$COMPILER" "${COMPILER_TARGET[@]}" -o "$WORKDIR/$name" "$src" "${helpers[@]}" > /dev/null 2>&1
    else
        run_limited "$COMPILER" "${COMPILER_TARGET[@]}" -o "$WORKDIR/$name" "$src" > /dev/null 2>&1
    fi
    local compiler_exit=$?
    if [ "$compiler_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (compile timeout after ${TEST_TIMEOUT}s): $src"
        return
    fi
    if [ "$compiler_exit" -ne 0 ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (compile): $src"
        return
    fi

    if [ ! -f "$name" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (no binary): $src"
        return
    fi

    run_limited "${RUNNER[@]}" "$WORKDIR/$name" > /dev/null 2>&1
    local actual_exit=$?
    if [ "$actual_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (timeout after ${TEST_TIMEOUT}s): $src"
        return
    fi

    # Get reference result from gcc
    run_limited "$REF_CC" "${GCC_ARCH[@]}" -w -o "$WORKDIR/${name}_ref" "$src" "${helpers[@]}" 2>/dev/null
    local reference_compile_exit=$?
    if [ "$reference_compile_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (reference compile timeout after ${TEST_TIMEOUT}s): $src"
        return
    fi
    if [ "$reference_compile_exit" -ne 0 ]; then
        [ "$actual_exit" -lt 128 ] && PASS=$((PASS + 1)) || { FAIL=$((FAIL + 1)); ERRORS="$ERRORS\nFAIL (crash): $src"; }
        return
    fi

    run_limited "${RUNNER[@]}" "$WORKDIR/${name}_ref" > /dev/null 2>&1
    local expected_exit=$?
    if [ "$expected_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (reference timeout after ${TEST_TIMEOUT}s): $src"
        return
    fi

    if [ "$actual_exit" = "$expected_exit" ]; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (expected=$expected_exit got=$actual_exit): $src"
    fi
}

run_library_test() {
    # Library tests come in pairs: foo_client.c and foo.c
    local client_src="$1"
    local name
    name=$(basename "$client_src" _client.c)
    local dir
    dir=$(dirname "$client_src")
    local lib_src="$dir/${name}.c"

    if [ ! -f "$lib_src" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (no lib): $client_src"
        return
    fi

    local WORKDIR
    WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/rnqcc-test.XXXXXX") || exit 1
    trap cleanup_workdir RETURN
    cd "$WORKDIR" || exit 1

    # Compile both files together
    local binary="${name}_client"
    run_limited "$COMPILER" "${COMPILER_TARGET[@]}" -o "$WORKDIR/$binary" "$client_src" "$lib_src" > /dev/null 2>&1
    local compiler_exit=$?
    if [ "$compiler_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (compile timeout after ${TEST_TIMEOUT}s): $client_src"
        return
    fi
    if [ "$compiler_exit" -ne 0 ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (compile): $client_src"
        return
    fi

    run_limited "${RUNNER[@]}" "$WORKDIR/$binary" > /dev/null 2>&1
    local actual_exit=$?
    if [ "$actual_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (timeout after ${TEST_TIMEOUT}s): $client_src"
        return
    fi

    # Reference
    run_limited "$REF_CC" "${GCC_ARCH[@]}" -w -o "$WORKDIR/${name}_ref" "$client_src" "$lib_src" 2>/dev/null
    local reference_compile_exit=$?
    if [ "$reference_compile_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (reference compile timeout after ${TEST_TIMEOUT}s): $client_src"
        return
    fi
    if [ "$reference_compile_exit" -ne 0 ]; then
        [ "$actual_exit" -lt 128 ] && PASS=$((PASS + 1)) || { FAIL=$((FAIL + 1)); ERRORS="$ERRORS\nFAIL (crash): $client_src"; }
        return
    fi

    run_limited "${RUNNER[@]}" "$WORKDIR/${name}_ref" > /dev/null 2>&1
    local expected_exit=$?
    if [ "$expected_exit" -eq "$TIMEOUT_EXIT" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (reference timeout after ${TEST_TIMEOUT}s): $client_src"
        return
    fi

    if [ "$actual_exit" = "$expected_exit" ]; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (expected=$expected_exit got=$actual_exit): $client_src"
    fi
}

run_test() {
    local src="$1"
    local name
    name=$(basename "$src" .c)

    # Check if this is a library test
    if [[ "$src" == *"/libraries/"* ]]; then
        if [[ "$name" == *"_client" ]]; then
            run_library_test "$src"
        else
            # Skip non-client library files (they're compiled as part of client tests)
            return 1
        fi
    else
        run_single_test "$src"
    fi
    return 0
}

chapters="${@:-1 2 3 4 5 6 7 8 9 10}"

for ch in $chapters; do
    valid_dir="$TESTDIR/chapter_$ch/valid"
    [ -d "$valid_dir" ] || continue
    count=0
    ch_pass=0
    echo -n "Chapter $ch: "

    for f in "$valid_dir"/*.c; do
        [ -f "$f" ] || continue
        old_pass=$PASS
        run_test "$f"
        [ $? -eq 1 ] && continue
        count=$((count + 1))
        [ $PASS -gt $old_pass ] && ch_pass=$((ch_pass + 1))
    done

    while IFS= read -r -d '' f; do
        old_pass=$PASS
        run_test "$f"
        [ $? -eq 1 ] && continue
        count=$((count + 1))
        [ $PASS -gt $old_pass ] && ch_pass=$((ch_pass + 1))
    # Keep paths NUL-delimited so spaces and shell metacharacters are safe.
    # Do not rely on GNU sort -z; this runner also supports macOS/BSD tools.
    done < <(find "$valid_dir" -mindepth 2 -name "*.c" -print0)

    echo "$ch_pass/$count passed"
done

echo ""
echo "Total: $PASS passed, $FAIL failed"
if [ $((PASS + FAIL)) -eq 0 ]; then
    echo "No tests found" >&2
    exit 1
fi

if [ -n "$ERRORS" ]; then
    echo ""
    echo "Failures:"
    echo -e "$ERRORS"
fi

[ "$FAIL" -eq 0 ]
