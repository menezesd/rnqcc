#!/bin/bash
# Run valid tests from writing-a-c-compiler-tests
# Usage: ./run_tests.sh [chapter_numbers...]

TESTDIR="/Users/dean/writing-a-c-compiler-tests/tests"
COMPILER="/Users/dean/rnqcc/target/debug/rnqcc"
PASS=0
FAIL=0
ERRORS=""

run_single_test() {
    local src="$1"
    local name=$(basename "$src" .c)

    cd /Users/dean/rnqcc

    # Check for helper libraries
    local chapter_dir=$(echo "$src" | sed 's|/valid/.*|/|')
    local helper_dir="${chapter_dir}helper_libs"
    local helpers=""
    if [ -d "$helper_dir" ]; then
        for h in "$helper_dir"/${name}.c "$helper_dir"/${name}_*.c; do
            [ -f "$h" ] && helpers="$helpers $h"
        done
    fi

    if [ -n "$helpers" ]; then
        $COMPILER "$src" $helpers > /dev/null 2>&1
    else
        $COMPILER "$src" > /dev/null 2>&1
    fi
    if [ $? -ne 0 ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (compile): $src"
        return
    fi

    if [ ! -f "$name" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (no binary): $src"
        return
    fi

    arch -x86_64 ./"$name" > /dev/null 2>&1
    local actual_exit=$?
    rm -f "$name" "${name}.s"

    # Get reference result from gcc
    gcc -arch x86_64 -w -o "${name}_ref" "$src" $helpers 2>/dev/null
    if [ $? -ne 0 ]; then
        [ "$actual_exit" -lt 128 ] && PASS=$((PASS + 1)) || { FAIL=$((FAIL + 1)); ERRORS="$ERRORS\nFAIL (crash): $src"; }
        return
    fi

    arch -x86_64 ./"${name}_ref" > /dev/null 2>&1
    local expected_exit=$?
    rm -f "${name}_ref"

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
    local name=$(basename "$client_src" _client.c)
    local dir=$(dirname "$client_src")
    local lib_src="$dir/${name}.c"

    if [ ! -f "$lib_src" ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (no lib): $client_src"
        return
    fi

    cd /Users/dean/rnqcc

    # Compile both files together
    $COMPILER "$client_src" "$lib_src" > /dev/null 2>&1
    if [ $? -ne 0 ]; then
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (compile): $client_src"
        return
    fi

    local binary="${name}_client"
    arch -x86_64 ./"$binary" > /dev/null 2>&1
    local actual_exit=$?
    rm -f "$binary" "${name}_client.s" "${name}.s"

    # Reference
    gcc -arch x86_64 -w -o "${name}_ref" "$client_src" "$lib_src" 2>/dev/null
    if [ $? -ne 0 ]; then
        [ "$actual_exit" -lt 128 ] && PASS=$((PASS + 1)) || { FAIL=$((FAIL + 1)); ERRORS="$ERRORS\nFAIL (crash): $client_src"; }
        return
    fi

    arch -x86_64 ./"${name}_ref" > /dev/null 2>&1
    local expected_exit=$?
    rm -f "${name}_ref"

    if [ "$actual_exit" = "$expected_exit" ]; then
        PASS=$((PASS + 1))
    else
        FAIL=$((FAIL + 1))
        ERRORS="$ERRORS\nFAIL (expected=$expected_exit got=$actual_exit): $client_src"
    fi
}

run_test() {
    local src="$1"
    local name=$(basename "$src" .c)

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

    for subdir in "$valid_dir"/*/; do
        [ -d "$subdir" ] || continue
        for f in "$subdir"*.c; do
            [ -f "$f" ] || continue
            old_pass=$PASS
            run_test "$f"
            [ $? -eq 1 ] && continue
            count=$((count + 1))
            [ $PASS -gt $old_pass ] && ch_pass=$((ch_pass + 1))
        done
    done

    echo "$ch_pass/$count passed"
done

echo ""
echo "Total: $PASS passed, $FAIL failed"
if [ -n "$ERRORS" ]; then
    echo ""
    echo "Failures:"
    echo -e "$ERRORS"
fi
