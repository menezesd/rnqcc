use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn rnqcc() -> &'static str {
    env!("CARGO_BIN_EXE_rnqcc")
}

fn temp_file(name: &str, ext: &str) -> std::path::PathBuf {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rnqcc-test-{}-{}-{}-{:?}.{}",
        name,
        std::process::id(),
        id,
        std::thread::current().id(),
        ext
    ))
}

struct TempPath(std::path::PathBuf);

impl TempPath {
    fn new(name: &str, ext: &str) -> Self {
        Self(temp_file(name, ext))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn non_host_target() -> &'static str {
    if cfg!(target_os = "macos") {
        "x86_64-linux"
    } else {
        "x86_64-macos"
    }
}

fn stdout(output: std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_contains_in_order(haystack: &str, needles: &[&str]) -> Result<(), String> {
    let mut search_start = 0;
    for needle in needles {
        let remaining = &haystack[search_start..];
        let Some(offset) = remaining.find(needle) else {
            return Err(format!("missing `{}` after byte {}", needle, search_start));
        };
        search_start += offset + needle.len();
    }
    Ok(())
}

fn normalize_assembly_snapshot(asm: &str) -> String {
    let mut normalized = String::new();
    for line in asm.replace("\r\n", "\n").lines() {
        normalized.push_str(line.trim_end());
        normalized.push('\n');
    }
    normalized
}

fn normalize_tacky_snapshot(tacky: &str) -> String {
    let mut normalized = String::new();
    let mut skipping_symbol_types = false;
    for line in tacky.replace("\r\n", "\n").lines() {
        if line.trim() == "symbol_types: {" {
            normalized.push_str("    symbol_types: { ... },\n");
            skipping_symbol_types = true;
            continue;
        }
        if skipping_symbol_types {
            if line.trim() == "}," {
                skipping_symbol_types = false;
            }
            continue;
        }
        normalized.push_str(line.trim_end());
        normalized.push('\n');
    }
    normalized
}

#[test]
fn prints_supported_targets() {
    let output = Command::new(rnqcc())
        .arg("--print-targets")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let stdout = stdout(output);
    assert!(stdout.contains("x86_64-linux"));
    assert!(stdout.contains("x86_64-macos"));
    assert!(stdout.contains("aarch64-linux"));
    assert!(stdout.contains("aarch64-macos"));
}

#[test]
fn accepts_all_target_aliases() {
    for target in [
        "linux",
        "osx",
        "macos",
        "x86_64-linux",
        "x86_64-unknown-linux-gnu",
        "x86_64-osx",
        "x86_64-macos",
        "x86_64-apple-darwin",
        "arm64-linux",
        "aarch64-linux",
        "aarch64-unknown-linux-gnu",
        "arm64-macos",
        "aarch64-macos",
        "aarch64-apple-darwin",
    ] {
        let output = Command::new(rnqcc())
            .args(["--target", target, "--stage", "lex", "tests/return_42.c"])
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "target alias failed: {}", target);
    }
}

#[test]
fn preprocesses_to_stdout() {
    let output = Command::new(rnqcc())
        .args(["-E", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let stdout = stdout(output);
    assert!(stdout.contains("return 42;"));
}

#[test]
fn rejects_output_for_ir_stage() {
    let output = Command::new(rnqcc())
        .args([
            "--stage",
            "lex",
            "-o",
            "/tmp/rnqcc-lex.out",
            "tests/return_42.c",
        ])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("-o is only valid"));
}

#[test]
fn rejects_stage_combined_with_driver_mode_flag() {
    let output = Command::new(rnqcc())
        .args(["--stage", "lex", "-S", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("cannot be used with"));
}

#[test]
fn accepts_all_named_stages() {
    for stage in ["lex", "parse", "validate", "tacky", "codegen"] {
        let output = Command::new(rnqcc())
            .args(["--stage", stage, "tests/return_42.c"])
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "stage failed: {}", stage);
    }

    let out = temp_file("stage-alias", "s");
    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("s")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(out.exists());

    let _ = std::fs::remove_file(out);
}

#[test]
fn fuzz_smoke_script_compiles_seeded_case() -> Result<(), String> {
    let output = match Command::new("python3")
        .args([
            "scripts/fuzz_smoke.py",
            "--seed",
            "17",
            "--cases",
            "1",
            "--rnqcc",
            rnqcc(),
            "--target",
            "x86_64-linux",
        ])
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run fuzz smoke script: {err}")),
    };

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn rejects_unsupported_input_extension() {
    let src = temp_file("bad-extension", "txt");
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("expected C source, assembly, object, or library input"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn rejects_unsupported_input_extension_before_writing_artifacts() {
    let valid = temp_file("bad-extension-preflight", "c");
    let invalid = temp_file("bad-extension-preflight", "txt");
    let asm = valid.with_extension("s");
    std::fs::write(&valid, "int main(void) { return 0; }\n").expect("failed to write input");
    std::fs::write(&invalid, "not c\n").expect("failed to write invalid input");
    let _ = std::fs::remove_file(&asm);

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg(&valid)
        .arg(&invalid)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("expected C source, assembly, object, or library input"));
    assert!(!asm.exists());

    let _ = std::fs::remove_file(valid);
    let _ = std::fs::remove_file(invalid);
}

#[test]
fn rejects_output_with_multiple_preprocess_inputs() {
    let output = Command::new(rnqcc())
        .args([
            "-E",
            "-o",
            "/tmp/rnqcc-preprocess.out",
            "tests/return_42.c",
            "tests/variables.c",
        ])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("-o with -E requires exactly one input file"));
}

#[test]
fn rejects_output_with_multiple_assembly_inputs() {
    let output = Command::new(rnqcc())
        .args([
            "-S",
            "-o",
            "/tmp/rnqcc-assembly.out",
            "tests/return_42.c",
            "tests/variables.c",
        ])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("-o with -S requires exactly one input file"));
}

#[test]
fn rejects_output_with_multiple_object_inputs() {
    let output = Command::new(rnqcc())
        .args([
            "-c",
            "-o",
            "/tmp/rnqcc-object.out",
            "tests/return_42.c",
            "tests/variables.c",
        ])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("-o with -c requires exactly one input file"));
}

#[test]
fn rejects_cross_target_object_output() {
    let output = Command::new(rnqcc())
        .arg("--target")
        .arg(non_host_target())
        .args(["-c", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("cannot assemble or link target"));
}

#[test]
fn reports_parse_errors_without_rust_panic_output() {
    let src = temp_file("bad-parse", "i");
    std::fs::write(&src, "int main(\n").expect("failed to write bad input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("parse")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("rnqcc: parse failed"));
    assert!(stderr.contains("parse failed at"));
    assert!(!stderr.contains("thread 'main' panicked"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn preprocessed_line_markers_remap_parse_diagnostics() {
    let src = temp_file("bad-parse-line-marker", "i");
    std::fs::write(&src, "# 50 \"generated-probe.c\"\nint main(\n")
        .expect("failed to write bad input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("parse")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("parse failed at generated-probe.c:50:10"),
        "{stderr}"
    );
    assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn preprocessed_line_markers_remap_lex_diagnostics() {
    let src = temp_file("bad-lex-line-marker", "i");
    std::fs::write(&src, "# 70 \"generated-lex.c\"\n@\n").expect("failed to write bad input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("lex")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("lex failed at generated-lex.c:70:1"),
        "{stderr}"
    );
    assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn rejects_extreme_alignment_without_panic() {
    for (name, source) in [
        (
            "huge-gnu-alignment",
            "struct __attribute__((aligned(9223372036854775807))) S { char a; char b; };\nint main(void) { return sizeof(struct S); }\n",
        ),
        (
            "huge-alignas",
            "_Alignas(9223372036854775807) int x;\nint main(void) { return 0; }\n",
        ),
        (
            "non-power-two-alignment",
            "struct __attribute__((aligned(3))) S { char a; };\nint main(void) { return sizeof(struct S); }\n",
        ),
    ] {
        let src = temp_file(name, "i");
        std::fs::write(&src, source).expect("failed to write input");

        let output = Command::new(rnqcc())
            .arg("--stage")
            .arg("parse")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(!output.status.success(), "{name}");
        let stderr = stderr(output);
        assert!(stderr.contains("alignment"), "{stderr}");
        assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

        let _ = std::fs::remove_file(src);
    }
}

#[test]
fn compiles_struct_scope_static_asserts() {
    let src = temp_file("struct-static-assert", "c");
    let exe = temp_file("struct-static-assert", "bin");
    std::fs::write(
        &src,
        "struct probe {\n\
             _Static_assert(sizeof(int) == 4, \"int size\");\n\
             int value;\n\
             static_assert(1);\n\
         };\n\
         int main(void) { struct probe p; p.value = 42; return p.value; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn rejects_failed_struct_scope_static_asserts_without_panic() {
    let src = temp_file("bad-struct-static-assert", "c");
    std::fs::write(
        &src,
        "struct probe { _Static_assert(0, \"bad\"); int value; };\n\
         int main(void) { return 0; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("parse")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("static assertion failed"), "{stderr}");
    assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn matches_parse_diagnostic_snapshot() {
    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("parse")
        .arg("tests/fixtures/diagnostics/parse_missing_paren.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let expected = include_str!("fixtures/diagnostics/parse_missing_paren.snap");
    assert_eq!(
        normalize_assembly_snapshot(&stderr(output)),
        normalize_assembly_snapshot(expected)
    );
}

#[test]
fn reports_resolve_errors_without_rust_panic_output() {
    for (name, source, expected) in [
        (
            "undeclared-var",
            "int main(void) { return x; }\n",
            "undeclared variable: 'x'",
        ),
        (
            "break-outside-loop",
            "int main(void) { break; return 0; }\n",
            "break outside of loop or switch",
        ),
        (
            "duplicate-label",
            "int main(void) { label: label: return 0; }\n",
            "duplicate label: 'label'",
        ),
        (
            "undefined-goto",
            "int main(void) { goto missing; return 0; }\n",
            "goto references undefined label: 'missing'",
        ),
    ] {
        let src = temp_file(name, "i");
        std::fs::write(&src, source).expect("failed to write input");

        let output = Command::new(rnqcc())
            .arg("--stage")
            .arg("validate")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(!output.status.success(), "{name}");
        let stderr = stderr(output);
        assert!(stderr.contains("rnqcc: resolve failed"), "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

        let _ = std::fs::remove_file(src);
    }
}

#[test]
fn rejects_conflicting_function_parameter_types() {
    let src = temp_file("conflicting-function-types", "i");
    std::fs::write(
        &src,
        "int f(int x);\nint f(long x);\nint main(void) { return 0; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("function 'f' declared with conflicting type"));
    assert!(!stderr.contains("thread 'main' panicked"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn rejects_conflicting_function_full_types() {
    for (name, source) in [
        (
            "conflicting-return",
            "int f(void);\nlong f(void);\nint main(void) { return 0; }\n",
        ),
        (
            "conflicting-pointer-param",
            "int f(int *x);\nint f(long *x);\nint main(void) { return 0; }\n",
        ),
        (
            "conflicting-variadic",
            "int f(int x);\nint f(int x, ...);\nint main(void) { return 0; }\n",
        ),
    ] {
        let src = temp_file(name, "i");
        std::fs::write(&src, source).expect("failed to write input");

        let output = Command::new(rnqcc())
            .arg("--stage")
            .arg("validate")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(!output.status.success(), "{name}");
        let stderr = stderr(output);
        assert!(
            stderr.contains("declared with conflicting type"),
            "{stderr}"
        );
        assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

        let _ = std::fs::remove_file(src);
    }
}

#[test]
fn rejects_direct_function_calls_with_wrong_argument_count() {
    let src = temp_file("bad-call-arity", "i");
    std::fs::write(
        &src,
        "int f(int x) { return x; }\nint main(void) { return f(1, 2); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("tacky")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("function 'f' called with 2 argument(s)"));
    assert!(!stderr.contains("thread 'main' panicked"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn reports_tacky_errors_without_rust_panic_output() {
    let src = temp_file("bad-static-init", "i");
    std::fs::write(&src, "int f(void) { return 1; }\nint g = f();\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("tacky")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("rnqcc: tacky failed"));
    assert!(stderr.contains("Global initializer must be constant"));
    assert!(!stderr.contains("thread 'main' panicked"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn emits_resolve_warnings_without_failing_compilation() {
    let src = temp_file("unreachable-warning", "i");
    let exe = temp_file("unreachable-warning", "bin");
    std::fs::write(&src, "int main(void) { return 7; 1 + 2; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stderr(output).contains("resolve warning: unreachable statement after return"));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(7));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn warning_controls_can_disable_or_promote_warnings() {
    let src = temp_file("warning-controls", "i");
    std::fs::write(&src, "int main(void) { return 7; 1 + 2; }\n").expect("failed to write input");

    let disabled = Command::new(rnqcc())
        .arg("--Wno-unreachable")
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let disabled_success = disabled.status.success();
    let disabled_stderr = stderr(disabled);
    assert!(disabled_success, "{disabled_stderr}");
    assert!(!disabled_stderr.contains("unreachable statement"));

    let promoted = Command::new(rnqcc())
        .arg("--Werror")
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(!promoted.status.success());
    let stderr = stderr(promoted);
    assert!(
        stderr.contains("unreachable statement after return"),
        "{stderr}"
    );
    assert!(stderr.contains("warnings treated as errors"), "{stderr}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn warns_on_missing_return_in_non_void_function() {
    let src = temp_file("missing-return-warning", "i");
    std::fs::write(
        &src,
        "int f(int x) { if (x) return 1; }\nint main(void) { return f(0); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stderr(output).contains("may exit without returning a value"));

    let disabled = Command::new(rnqcc())
        .arg("--Wno-missing-return")
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let disabled_success = disabled.status.success();
    let disabled_stderr = stderr(disabled);
    assert!(disabled_success, "{disabled_stderr}");
    assert!(!disabled_stderr.contains("may exit without returning a value"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn source_comments_annotate_generated_assembly() {
    let src = temp_file("source-comments", "i");
    let out = temp_file("source-comments", "s");
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--source-comments")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.starts_with("# rnqcc source:"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn dump_flags_emit_intermediate_ir_without_stopping() {
    let src = temp_file("dump-flags", "i");
    let exe = temp_file("dump-flags", "bin");
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .args([
            "--dump-ast",
            "--dump-tacky-pre-opt",
            "--dump-tacky",
            "--dump-asm-ir",
        ])
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stderr = stderr(output);
    assert!(stderr.contains("FunctionDeclaration"), "{stderr}");
    assert!(stderr.contains("TackyProgram"), "{stderr}");
    assert!(stderr.contains("AsmProgram"), "{stderr}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn rejects_incompatible_pointer_assignments_without_panic() {
    for (name, source, expected) in [
        (
            "bad-pointer-assignment",
            "int main(void) { int *p; long *q; p = q; return 0; }\n",
            "incompatible types in assignment",
        ),
        (
            "bad-pointer-initializer",
            "int main(void) { long *q; int *p = q; return 0; }\n",
            "incompatible types in initializer",
        ),
    ] {
        let src = temp_file(name, "i");
        std::fs::write(&src, source).expect("failed to write input");

        let output = Command::new(rnqcc())
            .arg("--stage")
            .arg("tacky")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(!output.status.success(), "{name}");
        let stderr = stderr(output);
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

        let _ = std::fs::remove_file(src);
    }
}

#[test]
fn rejects_incompatible_pointer_calls_and_returns_without_panic() {
    for (name, source, expected) in [
        (
            "bad-pointer-call",
            "int f(long *p) { return 0; }\nint main(void) { int *p; return f(p); }\n",
            "incompatible types in function call",
        ),
        (
            "bad-function-pointer-call",
            "int f(long *p) { return 0; }\nint main(void) { int *p; int (*fp)(long *) = f; return (*fp)(p); }\n",
            "incompatible types in function pointer call",
        ),
        (
            "bad-pointer-return",
            "long *f(int *p) { return p; }\nint main(void) { return 0; }\n",
            "incompatible types in return",
        ),
    ] {
        let src = temp_file(name, "i");
        std::fs::write(&src, source).expect("failed to write input");

        let output = Command::new(rnqcc())
            .arg("--stage")
            .arg("tacky")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(!output.status.success(), "{name}");
        let stderr = stderr(output);
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

        let _ = std::fs::remove_file(src);
    }
}

#[test]
fn accepts_void_pointer_conversions_and_null_pointer_constants() {
    let src = temp_file("void-pointer-conversions", "i");
    let exe = temp_file("void-pointer-conversions", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int value = 42;
    int *p = &value;
    void *vp = p;
    int *q = vp;
    long *none = 0;
    if (none) {
        return 1;
    }
    return *q;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_nested_aggregate_abi_stress_for_host_and_aarch64() {
    let src = temp_file("nested-aggregate-abi", "i");
    let exe = temp_file("nested-aggregate-abi", "bin");
    let asm = temp_file("nested-aggregate-abi-aarch64", "s");
    std::fs::write(
        &src,
        r#"
struct inner { int a; double b; };
union choice { struct inner i; long raw[2]; };
struct outer { union choice c; int tail; };

struct outer make(struct inner i, int tail) {
    struct outer o;
    o.c.i = i;
    o.tail = tail;
    return o;
}

int use(struct outer o) {
    return o.c.i.a + (int)o.c.i.b + o.tail;
}

int main(void) {
    struct inner i = { 10, 20.0 };
    struct outer o = make(i, 12);
    return use(o);
}
"#,
    )
    .expect("failed to write input");

    let host_output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(host_output.status.success(), "{}", stderr(host_output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let aarch64_output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&asm)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(
        aarch64_output.status.success(),
        "{}",
        stderr(aarch64_output)
    );
    assert!(std::fs::read_to_string(&asm)
        .expect("failed to read assembly output")
        .contains(".globl main"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
    let _ = std::fs::remove_file(asm);
}

#[test]
fn emits_x86_64_varargs_xmm_count() {
    let src = temp_file("x86-varargs-call", "i");
    let out = temp_file("x86-varargs-call", "s");
    std::fs::write(
        &src,
        r#"
int printf(const char *fmt, ...);
int main(void) {
    return printf("%d %f", 1, 2.0);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("movb $1, %al"));
    assert!(asm.contains("call printf@PLT"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_macos_variadic_call_sets_xmm_count_or_runs_on_native_host() {
    let src = temp_file("x86-macos-varargs-call", "i");
    let out = temp_file("x86-macos-varargs-call", "s");
    let exe = temp_file("x86-macos-varargs-call", "bin");
    std::fs::write(
        &src,
        r#"
int printf(const char *fmt, ...);
int main(void) {
    return printf("%f\n", 2.0);
}
"#,
    )
    .expect("failed to write input");

    if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        let output = Command::new(rnqcc())
            .args(["--target", "x86_64-macos", "-o"])
            .arg(&exe)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{}", stderr(output));
        let run = Command::new(&exe).output().expect("failed to run output");
        assert_eq!(run.status.code(), Some(9));
        assert_eq!(
            String::from_utf8(run.stdout).expect("stdout was not utf8"),
            "2.000000\n"
        );
    } else {
        let output = Command::new(rnqcc())
            .args(["--target", "x86_64-macos", "-S", "-o"])
            .arg(&out)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{}", stderr(output));
        let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
        assert!(asm.contains("movb $1, %al"));
        assert!(asm.contains("call _printf"));
        assert!(!asm.contains("call printf@PLT"));
    }

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn emits_aarch64_macos_varargs_tail_on_stack() {
    let src = temp_file("aarch64-macos-varargs", "i");
    let out = temp_file("aarch64-macos-varargs", "s");
    std::fs::write(
        &src,
        "int printf(const char *fmt, ...);\nint main(void) { return printf(\"%f\", 2.0); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr x0, [sp]"));
    assert!(asm.contains("str d9, [sp]"));
    assert!(asm.contains("bl _printf"));
    assert!(!asm.contains("fmov d0"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn aarch64_runtime_smoke_or_cross_assembly() {
    let src = temp_file("aarch64-runtime-smoke", "i");
    let out = temp_file("aarch64-runtime-smoke", "s");
    let exe = temp_file("aarch64-runtime-smoke", "bin");
    std::fs::write(
        &src,
        "int add(int x, int y) { return x + y; }\nint main(void) { return add(19, 23); }\n",
    )
    .expect("failed to write input");

    let target = if cfg!(target_os = "macos") {
        "aarch64-macos"
    } else {
        "aarch64-linux"
    };

    if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
        let output = Command::new(rnqcc())
            .args(["--target", target, "-o"])
            .arg(&exe)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{}", stderr(output));
        let run = Command::new(&exe).status().expect("failed to run output");
        assert_eq!(run.code(), Some(42));
    } else {
        let output = Command::new(rnqcc())
            .args(["--target", target, "-S", "-o"])
            .arg(&out)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{}", stderr(output));
        let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
        assert!(asm.contains("add w9, w9, w10"));
    }

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(exe);
}

#[cfg(unix)]
#[test]
fn reports_external_command_stderr() {
    let cc = write_failing_cc_with_stderr("failing-cc-stderr", "assembler says no");

    let output = Command::new(rnqcc())
        .arg("--cc")
        .arg(&cc)
        .args(["-E", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("command failed"));
    assert!(stderr.contains("exit status 42"));
    assert!(stderr.contains("assembler says no"));

    let _ = std::fs::remove_file(cc);
}

#[test]
fn emits_cross_target_assembly() {
    let out = temp_file("cross-target-asm", "s");
    let output = Command::new(rnqcc())
        .arg("--target")
        .arg(non_host_target())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(std::fs::read_to_string(&out)
        .expect("failed to read assembly output")
        .contains("main"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_x86_64_linux_tls_storage_and_accesses() {
    let src = temp_file("x86-linux-tls", "c");
    let out = temp_file("x86-linux-tls", "s");
    std::fs::write(
        &src,
        "_Thread_local int tls_value = 7;\n\
         extern __thread int extern_tls;\n\
         int read_tls(void) { return tls_value; }\n\
         int read_extern_tls(void) { return extern_tls; }\n\
         int write_tls(void) { tls_value = 42; return tls_value; }\n\
         int *addr_tls(void) { return &tls_value; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".section .tdata,\"awT\",@progbits"), "{asm}");
    assert!(asm.contains("%fs:tls_value@tpoff"), "{asm}");
    assert!(asm.contains("%fs:extern_tls@tpoff"), "{asm}");
    assert!(asm.contains("tls_value@tpoff(%r11)"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_x86_64_macos_tlv_storage_and_accesses() {
    let src = temp_file("x86-macos-tls", "c");
    let out = temp_file("x86-macos-tls", "s");
    std::fs::write(
        &src,
        "_Thread_local int tls_value = 7;\n\
         __thread int zero_tls;\n\
         extern __thread int extern_tls;\n\
         int read_tls(void) { return tls_value; }\n\
         int read_extern_tls(void) { return extern_tls; }\n\
         int write_tls(void) { tls_value = 42; return tls_value; }\n\
         int *addr_tls(void) { return &tls_value; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-macos", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(
        asm.contains(".section __DATA,__thread_data,thread_local_regular"),
        "{asm}"
    );
    assert!(
        asm.contains(".section __DATA,__thread_vars,thread_local_variables"),
        "{asm}"
    );
    assert!(asm.contains(".tbss _zero_tls$tlv$init,4,2"), "{asm}");
    assert!(asm.contains("_tls_value$tlv$init:"), "{asm}");
    assert!(asm.contains("_tls_value:"), "{asm}");
    assert!(asm.contains("\t.quad __tlv_bootstrap"), "{asm}");
    assert!(asm.contains("\t.quad _tls_value$tlv$init"), "{asm}");
    assert!(asm.contains("_tls_value@TLVP(%rip)"), "{asm}");
    assert!(asm.contains("_extern_tls@TLVP(%rip)"), "{asm}");
    assert!(asm.contains("\tcallq *(%rdi)"), "{asm}");
    assert!(!asm.contains("@tpoff"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_linux_tls_storage_and_accesses() {
    let src = temp_file("aarch64-linux-tls", "c");
    let out = temp_file("aarch64-linux-tls", "s");
    std::fs::write(
        &src,
        "__thread int tls_value;\n\
         extern _Thread_local int extern_tls;\n\
         int read_tls(void) { return tls_value; }\n\
         int read_extern_tls(void) { return extern_tls; }\n\
         int write_tls(void) { tls_value = 42; return tls_value; }\n\
         int *addr_tls(void) { return &tls_value; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".section .tbss,\"awT\",@nobits"), "{asm}");
    assert!(asm.contains("mrs x16, tpidr_el0"), "{asm}");
    assert!(asm.contains(":tprel_hi12:tls_value"), "{asm}");
    assert!(asm.contains(":tprel_lo12_nc:tls_value"), "{asm}");
    assert!(asm.contains(":tprel_hi12:extern_tls"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_macos_tlv_storage_and_accesses() {
    let src = temp_file("aarch64-macos-tls", "c");
    let out = temp_file("aarch64-macos-tls", "s");
    std::fs::write(
        &src,
        "_Thread_local int tls_value = 7;\n\
         __thread int zero_tls;\n\
         extern __thread int extern_tls;\n\
         int read_tls(void) { return tls_value; }\n\
         int read_extern_tls(void) { return extern_tls; }\n\
         int write_tls(void) { tls_value = 42; return tls_value; }\n\
         int *addr_tls(void) { return &tls_value; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(
        asm.contains(".section __DATA,__thread_data,thread_local_regular"),
        "{asm}"
    );
    assert!(
        asm.contains(".section __DATA,__thread_vars,thread_local_variables"),
        "{asm}"
    );
    assert!(asm.contains(".tbss _zero_tls$tlv$init,4,2"), "{asm}");
    assert!(asm.contains("_tls_value$tlv$init:"), "{asm}");
    assert!(asm.contains("_tls_value:"), "{asm}");
    assert!(asm.contains("\t.quad __tlv_bootstrap"), "{asm}");
    assert!(asm.contains("\t.quad _tls_value$tlv$init"), "{asm}");
    assert!(asm.contains("_tls_value@TLVPPAGE"), "{asm}");
    assert!(asm.contains("_tls_value@TLVPPAGEOFF"), "{asm}");
    assert!(asm.contains("_extern_tls@TLVPPAGE"), "{asm}");
    assert!(asm.contains("\tldr x8, [x0]"), "{asm}");
    assert!(asm.contains("\tblr x8"), "{asm}");
    assert!(!asm.contains("tpidr_el0"), "{asm}");
    assert!(!asm.contains("tprel"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_return_constant() {
    let out = temp_file("aarch64-return-constant", "s");
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".globl main"));
    assert!(asm.contains("movz w0, #42"));
    assert!(asm.contains("ret"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn matches_aarch64_return_constant_assembly_snapshot() {
    let out = temp_file("aarch64-return-constant-snapshot", "s");
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg("tests/fixtures/assembly/aarch64_return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let actual = std::fs::read_to_string(&out).expect("failed to read assembly output");
    let expected = include_str!("fixtures/assembly/aarch64_return_42.snap");
    assert_eq!(
        normalize_assembly_snapshot(&actual),
        normalize_assembly_snapshot(expected)
    );

    let _ = std::fs::remove_file(out);
}

#[test]
fn matches_tacky_simple_expr_snapshot() {
    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("tacky")
        .arg("tests/fixtures/tacky/simple_expr.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let expected = include_str!("fixtures/tacky/simple_expr.snap");
    assert_eq!(
        normalize_tacky_snapshot(&stdout(output)),
        normalize_tacky_snapshot(expected)
    );
}

#[test]
fn emits_aarch64_assembly_for_locals_and_arithmetic() {
    let out = temp_file("aarch64-locals-arithmetic", "s");
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg("tests/variables.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("sub sp, sp, #16"));
    assert!(asm.contains("str w9, [sp"));
    assert!(asm.contains("add w9, w9, w10"));
    assert!(asm.contains("add sp, sp, #16"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_integer_if_else() {
    let out = temp_file("aarch64-if-else", "s");
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg("tests/if_else.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("cmp w9, w10"));
    assert!(asm.contains("cset w9, gt"));
    assert!(asm.contains("b.eq .Lif_else"));
    assert!(asm.contains(".Lif_else"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_logical_not() {
    let src = temp_file("aarch64-logical-not", "i");
    let out = temp_file("aarch64-logical-not", "s");
    std::fs::write(&src, "int main(void) { return !0; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("cmp w9, w10"));
    assert!(asm.contains("cset w9, eq"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_direct_integer_call() {
    let src = temp_file("aarch64-call", "i");
    let out = temp_file("aarch64-call", "s");
    std::fs::write(
        &src,
        "int add(int a, int b) { return a + b; }\nint main(void) { return add(20, 22); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str w0, [sp]"));
    assert!(asm.contains("str w1, [sp, #4]"));
    assert!(asm.contains("movz w0, #20"));
    assert!(asm.contains("movz w1, #22"));
    assert!(asm.contains("bl add"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_double_return_constant() {
    let src = temp_file("aarch64-double-return", "i");
    let out = temp_file("aarch64-double-return", "s");
    std::fs::write(&src, "double main(void) { return 1.5; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("fmov d9, x9"));
    assert!(asm.contains("str d9, [sp"));
    assert!(asm.contains("ldr d0, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_double_arithmetic() {
    let src = temp_file("aarch64-double-arithmetic", "i");
    let out = temp_file("aarch64-double-arithmetic", "s");
    std::fs::write(&src, "double f(double a, double b) { return a + b; }\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str d0, [sp]"));
    assert!(asm.contains("str d1, [sp, #8]"));
    assert!(asm.contains("fadd d9, d9, d10"));
    assert!(asm.contains("ldr d0, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_double_comparison() {
    let src = temp_file("aarch64-double-comparison", "i");
    let out = temp_file("aarch64-double-comparison", "s");
    std::fs::write(&src, "int f(double a, double b) { return a < b; }\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("fcmp d9, d10"));
    assert!(asm.contains("cset w9, lt"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_eight_double_arguments() {
    let src = temp_file("aarch64-double-args", "i");
    let out = temp_file("aarch64-double-args", "s");
    std::fs::write(
        &src,
        "double g(double a,double b,double c,double d,double e,double f,double g,double h) { return h; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str d0, [sp]"));
    assert!(asm.contains("str d7, [sp, #56]"));
    assert!(asm.contains("ldr d0, [sp, #56]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_stack_double_argument() {
    let src = temp_file("aarch64-stack-double-arg", "i");
    let out = temp_file("aarch64-stack-double-arg", "s");
    std::fs::write(
        &src,
        "double ninth(double a,double b,double c,double d,double e,double f,double g,double h,double i) { return i; }\n\
         double main(void) { return ninth(1.0,2.0,3.0,4.0,5.0,6.0,7.0,8.0,9.0); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str d7, [sp, #56]"));
    assert!(asm.contains("sub sp, sp, #16"));
    assert!(asm.contains("str d9, [sp]"));
    assert!(asm.contains("ldr d9, [sp, #80]"));
    assert!(asm.contains("bl ninth"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_signed_int_to_double() {
    let src = temp_file("aarch64-int-to-double", "i");
    let out = temp_file("aarch64-int-to-double", "s");
    std::fs::write(&src, "double f(int x) { return x; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("scvtf d9, w9"));
    assert!(asm.contains("ldr d0, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_unsigned_int_to_double() {
    let src = temp_file("aarch64-uint-to-double", "i");
    let out = temp_file("aarch64-uint-to-double", "s");
    std::fs::write(&src, "double f(unsigned x) { return x; }\n").expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ucvtf d9, w9"));
    assert!(asm.contains("ldr d0, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_double_to_integer_conversions() {
    let src = temp_file("aarch64-double-to-int", "i");
    let out = temp_file("aarch64-double-to-int", "s");
    std::fs::write(
        &src,
        "int f(double x) { return x; }\nunsigned g(double x) { return x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("fcvtzs w9, d9"));
    assert!(asm.contains("fcvtzu w9, d9"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_double_negation_and_logical_not() {
    let src = temp_file("aarch64-double-unary", "i");
    let out = temp_file("aarch64-double-unary", "s");
    std::fs::write(
        &src,
        "double neg(double x) { return -x; }\nint is_zero(double x) { return !x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("fneg d9, d9"));
    assert!(asm.contains("fmov d10, x10"));
    assert!(asm.contains("fcmp d9, d10"));
    assert!(asm.contains("cset w9, eq"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_mixed_integer_and_double_arguments() {
    let src = temp_file("aarch64-mixed-args", "i");
    let out = temp_file("aarch64-mixed-args", "s");
    std::fs::write(
        &src,
        "double f(int a,double b,int c,double d) { return b + d; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str w0, [sp]"));
    assert!(asm.contains("str d0, [sp, #8]"));
    assert!(asm.contains("str w1, [sp, #16]"));
    assert!(asm.contains("str d1, [sp, #24]"));
    assert!(asm.contains("fadd d9, d9, d10"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_ninth_integer_argument() -> Result<(), String> {
    let src = temp_file("aarch64-ninth-arg", "i");
    let out = temp_file("aarch64-ninth-arg", "s");
    std::fs::write(
        &src,
        "int ninth(int a,int b,int c,int d,int e,int f,int g,int h,int i) { return i; }\n\
         int main(void) { return ninth(1,2,3,4,5,6,7,8,9); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("movz w7, #8"));
    assert!(asm.contains("movz w9, #9"));
    assert!(asm.contains("str w9, [sp]"));
    assert!(asm.contains("str x30, [sp, #"));
    assert!(asm.contains("bl ninth"));
    assert!(asm.contains("ldr x30, [sp, #"));
    assert!(asm.contains("ldr w9, [sp, #48]"));
    assert_contains_in_order(&asm, &["str x30, [sp, #", "bl ninth", "ldr x30, [sp, #"])?;

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    Ok(())
}

#[test]
fn aarch64_stack_parameters_account_for_link_register_save() -> Result<(), String> {
    let src = temp_file("aarch64-stack-param-with-call", "i");
    let out = temp_file("aarch64-stack-param-with-call", "s");
    std::fs::write(
        &src,
        "int side(void) { return 1; }\n\
         int f(int a,int b,int c,int d,int e,int f,int g,int h,int i) { return side() + i; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("sub sp, sp, #64"), "{asm}");
    assert!(asm.contains("str x30, [sp, #56]"), "{asm}");
    assert!(asm.contains("ldr w9, [sp, #64]"), "{asm}");
    assert!(!asm.contains("ldr w9, [sp, #48]"), "{asm}");
    assert_contains_in_order(
        &asm,
        &["str x30, [sp, #56]", "ldr w9, [sp, #64]", "bl side"],
    )?;

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    Ok(())
}

#[test]
fn emits_aarch64_assembly_for_multiple_stack_arguments() -> Result<(), String> {
    let src = temp_file("aarch64-multiple-stack-args", "i");
    let out = temp_file("aarch64-multiple-stack-args", "s");
    std::fs::write(
        &src,
        "int pick(int a,int b,int c,int d,int e,int f,int g,int h,int i,int j) { return i + j; }\n\
         int main(void) { return pick(1,2,3,4,5,6,7,8,9,10); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("sub sp, sp, #16"));
    assert!(asm.contains("str w9, [sp]"));
    assert!(asm.contains("str w9, [sp, #8]"));
    assert!(asm.contains("bl pick"));
    assert!(asm.contains("add sp, sp, #16"));
    assert!(asm.contains("ldr w9, [sp, #48]"));
    assert!(asm.contains("ldr w9, [sp, #56]"));
    assert_contains_in_order(
        &asm,
        &[
            "sub sp, sp, #16",
            "str w9, [sp]",
            "bl pick",
            "add sp, sp, #16",
        ],
    )?;

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    Ok(())
}

#[test]
fn emits_aarch64_assembly_for_pointer_stack_argument() -> Result<(), String> {
    let src = temp_file("aarch64-pointer-stack-arg", "i");
    let out = temp_file("aarch64-pointer-stack-arg", "s");
    std::fs::write(
        &src,
        "int load9(int a,int b,int c,int d,int e,int f,int g,int h,int *p) { return *p; }\n\
         int main(void) { int x = 42; return load9(1,2,3,4,5,6,7,8,&x); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr x9, [sp, #24]"));
    assert!(asm.contains("str x9, [sp]"));
    assert!(asm.contains("bl load9"));
    assert!(asm.contains("ldr x9, [sp, #48]"));
    assert!(asm.contains("ldr w9, [x10]"));
    assert_contains_in_order(&asm, &["ldr x9, [sp, #24]", "str x9, [sp]", "bl load9"])?;

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    Ok(())
}

#[test]
fn emits_aarch64_assembly_for_indirect_call() -> Result<(), String> {
    let src = temp_file("aarch64-indirect-call", "i");
    let out = temp_file("aarch64-indirect-call", "s");
    std::fs::write(
        &src,
        "int call(int (*fp)(int, int)) { return fp(20, 22); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str x0, [sp]"));
    assert!(asm.contains("movz w0, #20"));
    assert!(asm.contains("movz w1, #22"));
    assert!(asm.contains("ldr x9, [sp]"));
    assert!(asm.contains("str x30, [sp, #"));
    assert!(asm.contains("blr x9"));
    assert!(asm.contains("ldr x30, [sp, #"));
    assert_contains_in_order(&asm, &["str x30, [sp, #", "blr x9", "ldr x30, [sp, #"])?;

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    Ok(())
}

#[test]
fn emits_aarch64_assembly_for_indirect_call_with_stack_argument() -> Result<(), String> {
    let src = temp_file("aarch64-indirect-stack-args", "i");
    let out = temp_file("aarch64-indirect-stack-args", "s");
    std::fs::write(
        &src,
        "int call(int (*fp)(int,int,int,int,int,int,int,int,int)) { return fp(1,2,3,4,5,6,7,8,9); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("movz w7, #8"));
    assert!(asm.contains("movz w9, #9"));
    assert!(asm.contains("str w9, [sp]"));
    assert!(asm.contains("ldr x9, [sp, #16]"));
    assert!(asm.contains("blr x9"));
    assert_contains_in_order(
        &asm,
        &[
            "sub sp, sp, #16",
            "str w9, [sp]",
            "ldr x9, [sp, #16]",
            "blr x9",
            "add sp, sp, #16",
        ],
    )?;

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    Ok(())
}

#[test]
fn emits_aarch64_assembly_for_function_designator_decay() {
    let src = temp_file("aarch64-function-designator-decay", "i");
    let out = temp_file("aarch64-function-designator-decay", "s");
    std::fs::write(
        &src,
        "int callee(int x) { return x + 1; }\n\
         int main(void) { int (*fp)(int) = callee; return fp(4); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("adrp x9, callee"));
    assert!(asm.contains("add x9, x9, :lo12:callee"));
    assert!(asm.contains("blr x9"));
    assert!(!asm.contains("ldrsw x9, [sp]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_function_designator_argument() {
    let src = temp_file("aarch64-function-designator-arg", "i");
    let out = temp_file("aarch64-function-designator-arg", "s");
    std::fs::write(
        &src,
        "int f(int (*fp)(int), int x) { return fp(x); }\n\
         int inc(int x) { return x + 1; }\n\
         int main(void) { return f(inc, 9); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("adrp x9, inc"));
    assert!(asm.contains("add x9, x9, :lo12:inc"));
    assert!(asm.contains("ldr x0, [sp]"));
    assert!(asm.contains("bl f"));
    assert!(!asm.contains("ldrsw x9, [sp]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_x86_64_varargs_xmm_count_for_indirect_call() {
    let src = temp_file("x86-indirect-varargs-call", "i");
    let out = temp_file("x86-indirect-varargs-call", "s");
    std::fs::write(
        &src,
        r#"
int printf(const char *fmt, ...);
int main(void) {
    int (*fp)(const char *, ...) = printf;
    return fp("%f", 2.0);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("movb $1, %al"));
    assert!(asm.contains("call *%r10"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_variadic_function_pointer_calls_through_cast() {
    let src = temp_file("variadic-function-pointer-cast", "i");
    let exe = temp_file("variadic-function-pointer-cast", "bin");
    std::fs::write(
        &src,
        r#"
int printf(const char *fmt, ...);
int main(void) {
    int count = ((int (*)(const char *, ...))printf)("%d %.1f\n", 7, 2.0);
    return count == 6 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).output().expect("failed to run output");
    assert_eq!(run.status.code(), Some(42));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "7 2.0\n");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_gnu_statement_expressions() {
    let src = temp_file("gnu-statement-expressions", "i");
    let exe = temp_file("gnu-statement-expressions", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int x = 10;
    int y = ({ int local = x + 20; local + 12; });
    __auto_type z = ({ long value = 40; value + 2; });
    int max = ({ __auto_type _a = 7; __auto_type _b = 42; _a > _b ? _a : _b; });
    return y == 42 && z == 42 && max == 42 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_compiles_macro_statement_expressions() {
    let src = temp_file("internal-cpp-statement-expression-macro", "c");
    let exe = temp_file("internal-cpp-statement-expression-macro", "bin");
    std::fs::write(
        &src,
        r#"
#define MAX(a, b) ({ __auto_type _a = (a); __auto_type _b = (b); _a > _b ? _a : _b; })
int main(void) {
    int x = 20;
    return MAX(x + 1, 42) == 42 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_anonymous_struct_and_union_members() {
    let src = temp_file("anonymous-aggregate-members", "i");
    let exe = temp_file("anonymous-aggregate-members", "bin");
    std::fs::write(
        &src,
        r#"
struct Outer {
    int prefix;
    union {
        int i;
        long l;
    };
    struct {
        int x;
        int y;
    };
};

int main(void) {
    struct Outer o;
    o.i = 10;
    o.x = 20;
    o.y = 12;
    return o.i + o.x + o.y == 42 && sizeof(o.l) == sizeof(long) ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_aggregate_definition_with_declarators() {
    let src = temp_file("aggregate-definition-declarators", "c");
    let exe = temp_file("aggregate-definition-declarators", "bin");
    std::fs::write(
        &src,
        r#"
struct Point { int x; int y; } origin = { 20, 22 }, *origin_ptr = &origin;
union Number { int i; long l; } number;

int main(void) {
    struct Local { int value; } local = { 11 }, *local_ptr = &local;
    number.i = origin_ptr->x + origin.y;
    return number.i + local_ptr->value == 53 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_static_pointer_address_initializers() {
    let src = temp_file("static-pointer-address-initializers", "c");
    let exe = temp_file("static-pointer-address-initializers", "bin");
    std::fs::write(
        &src,
        "int global_value = 40;\n\
         int *global_ptr = &global_value;\n\
         static int *static_global_ptr = (int *)&global_value;\n\
         int main(void) {\n\
             static int *local_static_ptr = &global_value;\n\
             return *global_ptr + *static_global_ptr + *local_static_ptr == 120 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_static_pointer_designator_initializers() {
    let src = temp_file("static-pointer-designator-initializers", "c");
    let exe = temp_file("static-pointer-designator-initializers", "bin");
    std::fs::write(
        &src,
        "int values[2] = { 41, 1 };\n\
         int *values_ptr = values;\n\
         int callee(void) { return 40; }\n\
         int (*callee_ptr)(void) = callee;\n\
         struct Holder { int *ptr; int (*fn)(void); } holder = { values, callee };\n\
         int main(void) {\n\
             static int *local_values_ptr = values;\n\
             return values_ptr[0] + values_ptr[1] == 42 &&\n\
                    local_values_ptr[0] + local_values_ptr[1] == 42 &&\n\
                    holder.ptr[0] + holder.ptr[1] == 42 &&\n\
                    callee_ptr() + holder.fn() == 80\n\
                        ? 42\n\
                        : 1;\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_static_compound_literal_initializers() {
    let src = temp_file("static-compound-literal-initializers", "c");
    let exe = temp_file("static-compound-literal-initializers", "bin");
    std::fs::write(
        &src,
        "struct Pair { int x; int y; };\n\
         struct Pair pair = (struct Pair){ 20, 22 };\n\
         int values[2] = (int[2]){ 19, 23 };\n\
         int scalar = (int){ 42 };\n\
         struct Holder { struct Pair pair; int values[2]; int scalar; } holder = { (struct Pair){ 10, 11 }, (int[2]){ 12, 9 }, (int){ 21 } };\n\
         int main(void) {\n\
             return pair.x + pair.y == 42 &&\n\
                    values[0] + values[1] == 42 &&\n\
                    scalar == 42 &&\n\
                    holder.pair.x + holder.pair.y + holder.values[0] + holder.values[1] == 42 &&\n\
                    holder.scalar == 21\n\
                        ? 42\n\
                        : 1;\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn indirect_function_pointer_uses_declared_return_type() {
    let src = temp_file("indirect-return-type", "i");
    let exe = temp_file("indirect-return-type", "bin");
    std::fs::write(
        &src,
        "long f(void) { return 4294967296L; }\n\
         int main(void) { long (*fp)(void) = f; return fp() == 4294967296L ? 0 : 1; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(0));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn emits_aarch64_assembly_for_integer_division_and_shifts() {
    let src = temp_file("aarch64-div-shift", "i");
    let out = temp_file("aarch64-div-shift", "s");
    std::fs::write(
        &src,
        "int main(void) { return (84 / 2) + (1 << 3) + (16 >> 2); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("sdiv w9, w9, w10"));
    assert!(asm.contains("lsl w9, w9, w10"));
    assert!(asm.contains("asr w9, w9, w10"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_unsigned_division_and_shift() {
    let src = temp_file("aarch64-unsigned-div-shift", "i");
    let out = temp_file("aarch64-unsigned-div-shift", "s");
    std::fs::write(
        &src,
        "int main(void) { unsigned x = 16u; return (x / 2u) + (x >> 2); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("udiv w9, w9, w10"));
    assert!(asm.contains("lsr w9, w9, w10"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_integer_remainder() {
    let src = temp_file("aarch64-remainder", "i");
    let out = temp_file("aarch64-remainder", "s");
    std::fs::write(
        &src,
        "int main(void) { unsigned x = 17u; return (17 % 5) + (x % 5u); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("sdiv w11, w9, w10"));
    assert!(asm.contains("udiv w11, w9, w10"));
    assert!(asm.contains("msub w9, w11, w10, w9"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_int128_division_helpers() {
    let src = temp_file("aarch64-int128-div", "i");
    let out = temp_file("aarch64-int128-div", "s");
    std::fs::write(
        &src,
        "unsigned __int128 f(unsigned __int128 a, unsigned __int128 b) { return a / b; }\n\
         __int128 g(__int128 a, __int128 b) { return a % b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("bl __udivti3"));
    assert!(asm.contains("bl __modti3"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_variable_int128_shifts() {
    let src = temp_file("aarch64-int128-variable-shift", "i");
    let out = temp_file("aarch64-int128-variable-shift", "s");
    std::fs::write(
        &src,
        "unsigned __int128 f(unsigned __int128 x, int y) { return x << (y & 5); }\n\
         __int128 g(__int128 x, int y) { return x >> (y & 5); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".Li128_shift_loop."));
    assert!(asm.contains("lsl x9, x9, x10"));
    assert!(asm.contains("asr x11, x11, x10"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_x86_64_assembly_for_variable_int128_shifts() {
    let src = TempPath::new("x86-int128-variable-shift", "i");
    let out = TempPath::new("x86-int128-variable-shift", "s");
    std::fs::write(
        src.path(),
        "unsigned __int128 f(unsigned __int128 x, int y) { return x << (y & 5); }\n\
         __int128 g(__int128 x, int y) { return x >> (y & 5); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(out.path())
        .arg(src.path())
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(out.path()).expect("failed to read assembly output");
    assert!(asm.contains(".Li128_shift_loop."), "{asm}");
    assert!(asm.contains("shrq $1"), "{asm}");
    assert!(asm.contains("sarq $1"), "{asm}");
}

#[test]
fn compiles_aarch64_constant_int128_shifts_across_word_halves() {
    let src = temp_file("aarch64-int128-constant-cross-half-shift", "c");
    let exe = temp_file("aarch64-int128-constant-cross-half-shift", "bin");
    std::fs::write(
        &src,
        r#"
unsigned long f(unsigned __int128 in1, unsigned long in2) {
    __int128 mask = (__int128)0xffff << 56;
    return ((in1 & mask) >> 56) | in2;
}

int main(void) {
    unsigned __int128 in = 1;
    in <<= 64;
    return f(in, 2) == 0x102 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn emits_aarch64_assembly_for_vector_cast_through_128bit_storage() {
    let src = temp_file("aarch64-vector-cast-i128-storage", "c");
    let out = temp_file("aarch64-vector-cast-i128-storage", "s");
    std::fs::write(
        &src,
        r#"
typedef unsigned char V8 __attribute__((vector_size(32)));
typedef unsigned int V32 __attribute__((vector_size(32)));
typedef unsigned long long V64 __attribute__((vector_size(32)));

static V32 foo(V64 x) {
    V64 y = (V64)(V8){((V8)(V64){65535, x[0]})[1]};
    return (V32){y[0], 255};
}

int main(void) {
    V32 x = foo((V64){});
    return x[1] == 255 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_movz_movk_for_large_immediates() {
    let src = temp_file("aarch64-large-immediate", "i");
    let out = temp_file("aarch64-large-immediate", "s");
    std::fs::write(&src, "long main(void) { return 4294967338L; }\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("movz x9, #42"));
    assert!(asm.contains("movk x9, #1, lsl #32"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_linux_static_data() {
    let src = temp_file("aarch64-linux-static-data", "i");
    let out = temp_file("aarch64-linux-static-data", "s");
    std::fs::write(
        &src,
        "int g = 3;\nstatic long z;\nunsigned u = 4294967295U;\nint main(void) { return 0; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".data"));
    assert!(asm.contains(".globl g"));
    assert!(asm.contains("g:\n\t.long 3"));
    assert!(asm.contains(".bss"));
    assert!(asm.contains("z:\n\t.zero 8"));
    assert!(asm.contains(".globl u"));
    assert!(asm.contains("u:\n\t.long 4294967295"));
    assert!(asm.contains(".section .note.GNU-stack,\"\",@progbits"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_linux_static_strings_and_pointer_init() {
    let src = temp_file("aarch64-linux-static-strings", "i");
    let out = temp_file("aarch64-linux-static-strings", "s");
    std::fs::write(
        &src,
        "char msg[4] = \"hi\";\nchar *p = \"ok\";\nint main(void) { return 0; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("msg:\n\t.asciz \"hi\"\n\t.zero 1"));
    assert!(asm.contains("p:\n\t.quad __string_const_0"));
    assert!(asm.contains(".section .rodata"));
    assert!(asm.contains("__string_const_0:\n\t.asciz \"ok\""));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_macos_static_data_sections_and_labels() {
    let src = temp_file("aarch64-macos-static-data", "i");
    let out = temp_file("aarch64-macos-static-data", "s");
    std::fs::write(
        &src,
        "int g = 3;\nstatic long z;\nchar *p = \"ok\";\nint main(void) { return 0; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".globl _g"));
    assert!(asm.contains("_g:\n\t.long 3"));
    assert!(asm.contains(".zerofill __DATA,__bss,_z,8,3"));
    assert!(asm.contains("_p:\n\t.quad ___string_const_0"));
    assert!(asm.contains(".section __TEXT,__cstring,cstring_literals"));
    assert!(asm.contains("___string_const_0:\n\t.asciz \"ok\""));
    assert!(!asm.contains(".note.GNU-stack"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_global_read() {
    let src = temp_file("aarch64-global-read", "i");
    let out = temp_file("aarch64-global-read", "s");
    std::fs::write(&src, "int g = 41;\nint main(void) { return g + 1; }\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("adrp x16, g"));
    assert!(asm.contains("ldr w9, [x16, :lo12:g]"));
    assert!(asm.contains("g:\n\t.long 41"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_global_write() {
    let src = temp_file("aarch64-global-write", "i");
    let out = temp_file("aarch64-global-write", "s");
    std::fs::write(&src, "int g;\nint main(void) { g = 7; return g; }\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("adrp x16, g"));
    assert!(asm.contains("str w9, [x16, :lo12:g]"));
    assert!(asm.contains("ldr w0, [x16, :lo12:g]") || asm.contains("ldr w9, [x16, :lo12:g]"));
    assert!(asm.contains("g:\n\t.zero 4"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_macos_assembly_for_global_address() {
    let src = temp_file("aarch64-global-address", "i");
    let out = temp_file("aarch64-global-address", "s");
    std::fs::write(
        &src,
        "int g = 42;\nint main(void) { int *p = &g; return *p; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("adrp x9, _g@GOTPAGE"));
    assert!(asm.contains("ldr x9, [x9, _g@GOTPAGEOFF]"));
    assert!(asm.contains("ldr w9, [x10]"));
    assert!(asm.contains("_g:\n\t.long 42"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_global_array_read() {
    let src = temp_file("aarch64-global-array-read", "i");
    let out = temp_file("aarch64-global-array-read", "s");
    std::fs::write(
        &src,
        "int a[3] = {1,2,3};\nint main(void) { return a[1]; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("a:\n\t.long 1\n\t.long 2\n\t.long 3"));
    assert!(asm.contains("adrp x9, a"));
    assert!(asm.contains("add x9, x9, :lo12:a"));
    assert!(asm.contains("mul x10, x10, x11"));
    assert!(asm.contains("ldr w9, [x10]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_global_array_write() {
    let src = temp_file("aarch64-global-array-write", "i");
    let out = temp_file("aarch64-global-array-write", "s");
    std::fs::write(
        &src,
        "int a[3];\nint main(void) { a[1] = 7; return a[1]; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("a:\n\t.zero 12"));
    assert!(asm.contains("adrp x9, a"));
    assert!(asm.contains("str w9, [x10]"));
    assert!(asm.contains("ldr w9, [x10]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_address_of_and_load() {
    let src = temp_file("aarch64-address-load", "i");
    let out = temp_file("aarch64-address-load", "s");
    std::fs::write(
        &src,
        "int main(void) { int x = 42; int *p = &x; return *p; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("add x9, sp, #") || asm.contains("mov x9, sp"));
    assert!(asm.contains("str x9, [sp"));
    assert!(asm.contains("ldr x10, [sp"));
    assert!(asm.contains("ldr w9, [x10]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_pointer_store() {
    let src = temp_file("aarch64-pointer-store", "i");
    let out = temp_file("aarch64-pointer-store", "s");
    std::fs::write(
        &src,
        "int main(void) { int x = 1; int *p = &x; *p = 7; return x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr x10, [sp"));
    assert!(asm.contains("str w9, [x10]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_local_array_index() {
    let src = temp_file("aarch64-local-array", "i");
    let out = temp_file("aarch64-local-array", "s");
    std::fs::write(
        &src,
        "int main(void) { int a[3] = {10, 20, 30}; return a[1]; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str w9, [sp"));
    assert!(asm.contains("mul x10, x10, x11"));
    assert!(asm.contains("add x9, x9, x10"));
    assert!(asm.contains("ldr w9, [x10]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_char_pointer_load_and_store() {
    let src = temp_file("aarch64-char-pointer", "i");
    let out = temp_file("aarch64-char-pointer", "s");
    std::fs::write(
        &src,
        "int main(void) { char c = 1; char *p = &c; *p = 5; return c; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("strb w9, [sp"));
    assert!(asm.contains("strb w9, [x10]"));
    assert!(asm.contains("ldrb w9, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_signed_char_extension() {
    let src = temp_file("aarch64-signed-char-extension", "i");
    let out = temp_file("aarch64-signed-char-extension", "s");
    std::fs::write(
        &src,
        "int main(void) { signed char c = -1; int x = c; return x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldrsb w9, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_unsigned_char_extension() {
    let src = temp_file("aarch64-unsigned-char-extension", "i");
    let out = temp_file("aarch64-unsigned-char-extension", "s");
    std::fs::write(
        &src,
        "int main(void) { unsigned char c = 255; int x = c; return x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldrb w9, [sp"));
    assert!(!asm.contains("ldrsb"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_unsigned_comparison() {
    let src = temp_file("aarch64-unsigned-comparison", "i");
    let out = temp_file("aarch64-unsigned-comparison", "s");
    std::fs::write(&src, "int main(void) { unsigned x = 1u; return x < 2u; }\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("cset w9, lo"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_local_struct_copy() {
    let src = temp_file("aarch64-local-struct-copy", "i");
    let out = temp_file("aarch64-local-struct-copy", "s");
    std::fs::write(
        &src,
        "struct pair { int a; int b; char c; };\n\
         int main(void) { struct pair x = {10, 20, 30}; struct pair y = x; return y.b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr x9, [sp"));
    assert!(asm.contains("str x9, [sp"));
    assert!(asm.contains("ldr w9, [sp"));
    assert!(asm.contains("str w9, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_global_struct_copy() {
    let src = temp_file("aarch64-global-struct-copy", "i");
    let out = temp_file("aarch64-global-struct-copy", "s");
    std::fs::write(
        &src,
        "struct pair { int a; int b; };\n\
         static struct pair g;\n\
         int main(void) { struct pair x = {11, 22}; g = x; return g.b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("adrp x16, g"));
    assert!(asm.contains("str x9, [x16, :lo12:g]"));
    assert!(asm.contains("ldr w9, [x16, :lo12:g+4]"));
    assert!(asm.contains(".bss"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_small_struct_argument() {
    let src = temp_file("aarch64-small-struct-arg", "i");
    let out = temp_file("aarch64-small-struct-arg", "s");
    std::fs::write(
        &src,
        "struct pair { int a; int b; };\n\
         int f(struct pair p) { return p.b; }\n\
         int main(void) { struct pair x = {3, 4}; return f(x); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str x0, [sp]"));
    assert!(asm.contains("ldr x0, [sp"));
    assert!(asm.contains("bl f"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_small_struct_return() {
    let src = temp_file("aarch64-small-struct-return", "i");
    let out = temp_file("aarch64-small-struct-return", "s");
    std::fs::write(
        &src,
        "struct pair { int a; int b; };\n\
         struct pair make(void) { struct pair x = {3, 4}; return x; }\n\
         int main(void) { struct pair y = make(); return y.b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr x0, [sp"));
    assert!(asm.contains("bl make"));
    assert!(asm.contains("str x0, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_double_struct_argument_and_return() {
    let src = temp_file("aarch64-double-struct-call", "i");
    let out = temp_file("aarch64-double-struct-call", "s");
    std::fs::write(
        &src,
        "struct box { double x; };\n\
         struct box id(struct box b) { return b; }\n\
         double main(void) { struct box b = {1.5}; struct box c = id(b); return c.x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str d0, [sp]"));
    assert!(asm.contains("ldr d0, [sp"));
    assert!(asm.contains("bl id"));
    assert!(asm.contains("str d0, [sp"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_large_stack_struct_argument() {
    let src = temp_file("aarch64-large-stack-struct-arg", "i");
    let out = temp_file("aarch64-large-stack-struct-arg", "s");
    std::fs::write(
        &src,
        "struct big { long a; long b; long c; };\n\
         int f(struct big p) { return p.c; }\n\
         int main(void) { struct big x = {1, 2, 3}; return f(x); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr x9, [sp, #64]"));
    assert!(asm.contains("sub sp, sp, #32"));
    assert!(asm.contains("str x9, [sp, #16]"));
    assert!(asm.contains("bl f"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn keeps_aarch64_integer_struct_argument_group_on_stack_when_not_enough_registers() {
    let src = temp_file("aarch64-int-struct-group-stack", "i");
    let out = temp_file("aarch64-int-struct-group-stack", "s");
    std::fs::write(
        &src,
        "struct pair { long a; long b; };\n\
         int f(long a, long b, long c, long d, long e, long f, long g, struct pair p) { return p.b; }\n\
         int main(void) { struct pair p = {8, 9}; return f(1, 2, 3, 4, 5, 6, 7, p); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr x6, [sp"));
    assert!(!asm.contains("ldr x7, [sp"));
    assert!(asm.contains("ldr x9, [sp, #112]"));
    assert!(asm.contains("ldr x9, [sp, #120]"));
    assert!(asm.contains("str x9, [sp]"));
    assert!(asm.contains("str x9, [sp, #8]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn keeps_aarch64_fp_struct_argument_group_on_stack_when_not_enough_registers() {
    let src = temp_file("aarch64-fp-struct-group-stack", "i");
    let out = temp_file("aarch64-fp-struct-group-stack", "s");
    std::fs::write(
        &src,
        "struct two { double a; double b; };\n\
         double f(double a, double b, double c, double d, double e, double f, double g, struct two p) { return p.b; }\n\
         double main(void) { struct two p = {8.0, 9.0}; return f(1, 2, 3, 4, 5, 6, 7, p); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("ldr d6, [sp"));
    assert!(!asm.contains("ldr d7, [sp"));
    assert!(asm.contains("ldr d9, [sp, #96]"));
    assert!(asm.contains("ldr d9, [sp, #104]"));
    assert!(asm.contains("str d9, [sp]"));
    assert!(asm.contains("str d9, [sp, #8]"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_large_struct_return_via_hidden_pointer() {
    let src = temp_file("aarch64-large-struct-return", "i");
    let out = temp_file("aarch64-large-struct-return", "s");
    std::fs::write(
        &src,
        "struct big { long a; long b; long c; };\n\
         struct big make(void) { struct big x = {1, 2, 3}; return x; }\n\
         int main(void) { struct big y = make(); return y.c; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("str x0, [sp]"));
    assert!(asm.contains("ldr x0, [sp]"));
    assert!(asm.contains("add x9, sp, #"));
    assert!(asm.contains("bl make"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_compound_assign_to_struct_members() {
    let src = temp_file("aarch64-compound-assign-struct-members", "i");
    let out = temp_file("aarch64-compound-assign-struct-members", "s");
    std::fs::write(
        &src,
        r#"
struct inner {
    double a;
    char b;
    int *ptr;
};
struct outer {
    unsigned long l;
    struct inner *in_ptr;
    struct inner in_array[4];
    int bar;
};
int main(void) {
    int i = -1;
    struct inner si = {150., -12, &i};
    struct outer o = {18446744073709551615UL, &si, {si}, 2000};
    si.a += 10;
    o.in_array[0].b -= 460;
    o.in_array[0].a *= -4;
    o.in_ptr->a /= 5;
    (&o)->l %= o.bar;
    return 0;
}
"#,
    )
    .expect("failed to write input");
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".globl main"));
    assert!(asm.contains("fadd"));
    assert!(asm.contains("fdiv"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_inc_dec_struct_members() {
    let src = temp_file("aarch64-inc-dec-struct-members", "i");
    let out = temp_file("aarch64-inc-dec-struct-members", "s");
    std::fs::write(
        &src,
        r#"
struct inner {
    char c;
    unsigned int u;
};
struct outer {
    unsigned long l;
    struct inner *in_ptr;
    int array[3];
};
void *calloc(unsigned long nmemb, unsigned long size);
int main(void) {
    struct outer my_struct = {9223372036854775900ul, calloc(3, sizeof(struct inner)), {-1000, -2000, -3000}};
    struct outer *my_struct_ptr = &my_struct;
    ++my_struct.l;
    --my_struct.in_ptr[0].u;
    my_struct.in_ptr->c++;
    my_struct_ptr->array[1]--;
    (++my_struct_ptr->in_ptr)->c--;
    my_struct_ptr->in_ptr++->u++;
    return 0;
}
"#,
    )
    .expect("failed to write input");
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".globl main"));
    assert!(asm.contains("add"));
    assert!(asm.contains("sub"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_aarch64_assembly_for_preprocessed_math_header_fixture() {
    let src = temp_file("aarch64-preprocessed-math-header", "i");
    let out = temp_file("aarch64-preprocessed-math-header", "s");
    std::fs::write(
        &src,
        "double __builtin_fabs(double);\n\
         int double_isnan(double x) { return x != x || __builtin_fabs(x) != x; }\n",
    )
    .expect("failed to write input");
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains(".globl double_isnan"));
    assert!(asm.contains("bl fabs"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[cfg(not(target_arch = "aarch64"))]
#[test]
fn rejects_aarch64_object_output_on_non_aarch64_hosts() {
    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-c", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("cannot assemble or link target"));
}

#[test]
fn compiles_preprocessed_input() {
    let src = temp_file("preprocessed-src", "i");
    let exe = temp_file("preprocessed-exe", "bin");
    std::fs::write(&src, "int main(void) { return 42; }\n").expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn optimized_and_unoptimized_outputs_match() -> Result<(), String> {
    let src = temp_file("optimization-diff", "i");
    std::fs::write(
        &src,
        r#"
int adjust(int seed) {
    int copy = seed;
    int dead = copy * 100;
    dead = 7;
    if (0) {
        return dead;
    }
    return copy;
}

int main(void) {
    int total = 0;
    int i = 0;

    while (i < 5) {
        int base = i + 1;
        int alias = base;
        int term = adjust(alias);
        total = total + term * (2 + 1);
        i = i + 1;
    }

    if (1) {
        total = total - 45;
    } else {
        total = 99;
    }

    return total;
}
"#,
    )
    .expect("failed to write test input");

    let unopt = temp_file("optimization-diff-unopt", "bin");
    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&unopt)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(output.status.success(), "{}", stderr(output));

    let baseline = Command::new(&unopt)
        .status()
        .expect("failed to run unoptimized output")
        .code();

    let opt_cases: &[(&str, &[&str])] = &[
        ("fold-constants", &["--fold-constants"]),
        (
            "eliminate-unreachable-code",
            &["--eliminate-unreachable-code"],
        ),
        ("propagate-copies", &["--propagate-copies"]),
        ("eliminate-dead-stores", &["--eliminate-dead-stores"]),
        (
            "fold-and-unreachable",
            &["--fold-constants", "--eliminate-unreachable-code"],
        ),
        (
            "copy-and-dead-store",
            &["--propagate-copies", "--eliminate-dead-stores"],
        ),
        ("all", &["--optimize"]),
    ];

    for (name, flags) in opt_cases {
        let exe = temp_file(&format!("optimization-diff-{name}"), "bin");
        let output = Command::new(rnqcc())
            .args(*flags)
            .arg("-o")
            .arg(&exe)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");
        assert!(
            output.status.success(),
            "{} compile failed: {}",
            name,
            stderr(output)
        );

        let run = Command::new(&exe)
            .status()
            .map_err(|err| format!("failed to run {name} output: {err}"))?;
        assert_eq!(
            run.code(),
            baseline,
            "{name} runtime exit status differed from unoptimized output"
        );

        let _ = std::fs::remove_file(exe);
    }

    assert_eq!(baseline, Some(0));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(unopt);
    Ok(())
}

#[test]
fn compiles_alignof_type_expression() {
    let src = temp_file("alignof-src", "i");
    let exe = temp_file("alignof-exe", "bin");
    std::fs::write(
        &src,
        "struct S { char c; long x; };\nint main(void) { return _Alignof(struct S); }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(8));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_gnu_alignof_expression() {
    let src = temp_file("gnu-alignof-expression", "i");
    let exe = temp_file("gnu-alignof-expression", "bin");
    std::fs::write(
        &src,
        r#"
struct S { char c; long x; };
int main(void) {
    struct S value;
    long scalar = 0;
    return __alignof__(value) + __alignof__ scalar + __alignof__(value.x) + 18;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_typedef_shadowing_and_qualified_pointer_declarations() {
    let src = temp_file("frontend-typedef-qualifiers", "i");
    let exe = temp_file("frontend-typedef-qualifiers", "bin");
    std::fs::write(
        &src,
        r#"
typedef int word;
int main(void) {
    word value = 40;
    {
        typedef long word;
        const word inner = 2;
        value = value + inner;
    }
    volatile int *p = &value;
    return *p;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_c11_generic_selection() {
    let src = temp_file("frontend-generic-selection", "c");
    let exe = temp_file("frontend-generic-selection", "bin");
    std::fs::write(
        &src,
        r#"
#define TYPE_CODE(x) _Generic((x), int: 1, long: 2, unsigned long: 3, double: 4, char *: 5, default: 9)

int main(void) {
    int i = 0;
    long l = 0;
    unsigned long ul = 0;
    double d = 0.0;
    char *p = "x";
    short s = 0;
    return TYPE_CODE(i)
        + TYPE_CODE(l)
        + TYPE_CODE(ul)
        + TYPE_CODE(d)
        + TYPE_CODE(p)
        + TYPE_CODE(s)
        + 18;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn ignores_common_gnu_and_msvc_attribute_annotations() {
    let src = temp_file("frontend-attributes", "i");
    let exe = temp_file("frontend-attributes", "bin");
    std::fs::write(
        &src,
        r#"
__attribute__((visibility("hidden"))) int hidden_value(void) { return 40; }
int noinline_value(void) __attribute__((noinline));
int noinline_value(void) { return 2; }
__declspec(dllexport) int exported_value(void) { return 0; }
int main(void) { return hidden_value() + noinline_value() + exported_value(); }
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn honors_common_alignment_attribute_annotations() {
    let src = temp_file("frontend-alignment-attributes", "i");
    let exe = temp_file("frontend-alignment-attributes", "bin");
    std::fs::write(
        &src,
        r#"
enum { MEMBER_ALIGNMENT = 1 << 4 };

struct gnu_aligned_member {
    char c;
    int value __attribute__((aligned(MEMBER_ALIGNMENT)));
};

typedef long alignment_word;

struct msvc_aligned_member {
    char c;
    __declspec(align(sizeof(alignment_word) * 2)) int value;
};

struct ignored_substring_attribute {
    char c;
    int value __attribute__((warn_if_not_aligned(16)));
};

__attribute__((aligned(16 + 16))) static int global_value = 42;

int main(void) {
    return sizeof(struct gnu_aligned_member)
        + sizeof(struct msvc_aligned_member)
        + sizeof(struct ignored_substring_attribute)
        + global_value
        - 72;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn honors_packed_aggregate_attribute_layout() {
    let src = temp_file("frontend-packed-aggregate-attributes", "i");
    let exe = temp_file("frontend-packed-aggregate-attributes", "bin");
    std::fs::write(
        &src,
        r#"
struct __attribute__((packed)) PrefixPacked {
    char tag;
    long value;
    int tail;
};

struct SuffixPacked {
    char tag;
    long value;
    int tail;
} __attribute__((packed));

union __attribute__((packed)) PackedUnion {
    char tag;
    long value;
};

struct __attribute__((packed)) PackedLongBits {
    long flag : 1;
    char tail;
};

union __attribute__((packed)) PackedLongBitUnion {
    long flag : 1;
    char tag;
};

struct Natural {
    char tag;
    long value;
    int tail;
};

int main(void) {
    if (sizeof(struct PrefixPacked) != 13) return 1;
    if (__builtin_offsetof(struct PrefixPacked, value) != 1) return 2;
    if (__builtin_offsetof(struct PrefixPacked, tail) != 9) return 3;
    if (sizeof(struct SuffixPacked) != 13) return 4;
    if (__builtin_offsetof(struct SuffixPacked, value) != 1) return 5;
    if (sizeof(union PackedUnion) != 8) return 6;
    if (sizeof(struct PackedLongBits) != 2) return 7;
    if (_Alignof(struct PackedLongBits) != 1) return 8;
    if (__builtin_offsetof(struct PackedLongBits, tail) != 1) return 9;
    if (sizeof(union PackedLongBitUnion) != 1) return 10;
    if (_Alignof(union PackedLongBitUnion) != 1) return 11;
    if (sizeof(struct Natural) <= sizeof(struct PrefixPacked)) return 12;
    return 42;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn honors_field_packed_and_aggregate_aligned_layout() {
    let src = temp_file("frontend-packed-aligned-attributes", "i");
    let exe = temp_file("frontend-packed-aligned-attributes", "bin");
    std::fs::write(
        &src,
        r#"
struct FieldPacked {
    char tag;
    int value __attribute__((packed));
    char tail;
};

struct __attribute__((aligned(16))) AlignedStruct {
    char tag;
    int value;
};

struct __attribute__((packed, aligned(4))) PackedAligned {
    char tag;
    int value;
};

struct SuffixAligned {
    char tag;
    int value;
} __attribute__((aligned(16)));

int main(void) {
    if (sizeof(struct FieldPacked) != 6) return 1;
    if (__builtin_offsetof(struct FieldPacked, value) != 1) return 2;
    if (__builtin_offsetof(struct FieldPacked, tail) != 5) return 3;
    if (_Alignof(struct AlignedStruct) != 16) return 4;
    if (sizeof(struct AlignedStruct) != 16) return 5;
    if (_Alignof(struct PackedAligned) != 4) return 6;
    if (sizeof(struct PackedAligned) != 8) return 7;
    if (__builtin_offsetof(struct PackedAligned, value) != 1) return 8;
    if (_Alignof(struct SuffixAligned) != 16) return 9;
    if (sizeof(struct SuffixAligned) != 16) return 10;
    return 42;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_gnu_typeof_declarations() {
    let src = temp_file("frontend-typeof", "i");
    let exe = temp_file("frontend-typeof", "bin");
    std::fs::write(
        &src,
        r#"
int add_with_typeof_param(int value) {
    typeof(value) extra = 2;
    return value + extra;
}

long long_value(void) { return 5; }

int main(void) {
    typeof(int) base = 39;
    __typeof__(base) copy = 1;
    __typeof__(&base) ptr = &base;
    typeof(ptr + 1) next = ptr + 1;
    typeof(next - ptr) diff = next - ptr;
    typeof(long_value()) function_result = long_value();
    typeof(*ptr) from_pointer = add_with_typeof_param(copy);
    __typeof_unqual__(const int) unqualified_type_name = 3;
    typeof_unqual(base) unqualified_expression = 4;
    return base + from_pointer + diff + function_result + unqualified_type_name + unqualified_expression - 13;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_bool_conversion_semantics() {
    let src = temp_file("frontend-bool", "i");
    let exe = temp_file("frontend-bool", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int value = 7;
    _Bool flag = 2;
    _Bool zero = 0;
    _Bool null_pointer = (void *)0;
    _Bool object_pointer = &value;
    return flag == 1 && zero == 0 && null_pointer == 0 && object_pointer == 1 ? 42 : 1;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_short_truncation_sign_extension_and_layout() {
    let src = temp_file("frontend-short", "i");
    let exe = temp_file("frontend-short", "bin");
    std::fs::write(
        &src,
        r#"
struct MixedShorts {
    char c;
    short s;
    unsigned short u;
};

short signed_wrap(void) { return 65535; }
unsigned short unsigned_wrap(void) { return -1; }

int main(void) {
    short s = 65535;
    unsigned short u = -1;
    struct MixedShorts item;
    item.c = 1;
    item.s = -2;
    item.u = 65535;
    if (sizeof(short) != 2 || sizeof(unsigned short) != 2) return 1;
    if (sizeof(struct MixedShorts) != 6) return 2;
    if (s != -1 || u != 65535) return 3;
    if (signed_wrap() != -1 || unsigned_wrap() != 65535) return 4;
    if (item.s != -2 || item.u != 65535) return 5;
    return 42;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_alignas_declarations_and_member_layout() {
    let src = temp_file("frontend-alignas", "i");
    let exe = temp_file("frontend-alignas", "bin");
    std::fs::write(
        &src,
        r#"
_Alignas(16) static int global_value = 7;

struct AlignedMember {
    char c;
    _Alignas(8) int value;
    char tail;
};

struct TypeAlignedMember {
    char c;
    alignas(double) int value;
};

int main(void) {
    struct AlignedMember a;
    struct TypeAlignedMember b;
    a.c = 1;
    a.value = 38;
    a.tail = 3;
    b.c = 4;
    b.value = 4;
    if (sizeof(struct AlignedMember) != 16) return 1;
    if (sizeof(struct TypeAlignedMember) != 16) return 2;
    return global_value + a.value - b.value + 1;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_c11_generic_selection_expressions() {
    let src = temp_file("frontend-generic", "i");
    let exe = temp_file("frontend-generic", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int value = 3;
    long wide = 4;
    int *ptr = &value;
    int matched_int = _Generic(value, int: 10, long: 1, default: 2);
    int matched_long = _Generic(wide, int: 1, long: 11, default: 2);
    int matched_ptr = _Generic(ptr, int *: 12, default: 1);
    int matched_string = _Generic("abc", char *: 9, default: 1);
    return matched_int + matched_long + matched_ptr + matched_string;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_gnu_auto_type_declarations() {
    let src = temp_file("frontend-auto-type", "i");
    let exe = temp_file("frontend-auto-type", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int value = 5;
    __auto_type inferred_int = value + 7;
    __auto_type inferred_ptr = &value;
    __auto_type inferred_double = 20.0;
    __auto_type inferred_string = "abc";
    *inferred_ptr = 1;
    return inferred_int + value + (int)inferred_double + inferred_string[0] - 'a' + 9;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_and_runs_complex_arithmetic() {
    let src = temp_file("complex-arithmetic", "i");
    let exe = temp_file("complex-arithmetic", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    float _Complex a = {1.0f, 2.0f};
    float _Complex b = {3.0f, 4.0f};
    float _Complex c = a * b;
    float _Complex d = -c;
    if (c != (float _Complex){-5.0f, 10.0f}) return 1;
    if (d != (float _Complex){5.0f, -10.0f}) return 2;
    if ((float _Complex){0.0f, 0.0f}) return 3;
    if (!((float _Complex){1.0f, 0.0f})) return 4;
    return 42;
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_and_runs_host_printf_variadic_call() {
    let src = temp_file("printf-varargs-src", "i");
    let exe = temp_file("printf-varargs-exe", "bin");
    std::fs::write(
        &src,
        "int printf(const char *fmt, ...);\nint main(void) { return printf(\"%d\", 7); }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).output().expect("failed to run output");
    assert_eq!(run.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(run.stdout).expect("stdout was not utf8"),
        "7"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_and_runs_host_printf_variadic_function_pointer_call() {
    let src = temp_file("printf-varargs-fnptr-src", "i");
    let exe = temp_file("printf-varargs-fnptr-exe", "bin");
    std::fs::write(
        &src,
        r#"
int printf(const char *fmt, ...);
int main(void) {
    int (*fp)(const char *, ...) = printf;
    return fp("%d", 7) + (*fp)("%d", 8);
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).output().expect("failed to run output");
    assert_eq!(run.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(run.stdout).expect("stdout was not utf8"),
        "78"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_float_layout_arithmetic_and_conversions() {
    let src = temp_file("float-layout-arithmetic", "c");
    let exe = temp_file("float-layout-arithmetic", "bin");
    std::fs::write(
        &src,
        r#"
struct pair {
    char tag;
    float value;
    char tail;
};

float addf(float a, float b) {
    return a + b;
}

int main(void) {
    float f = 1.5f;
    float g = 2.25f;
    double d = f;
    float h = d;
    struct pair p;
    p.tag = 1;
    p.value = addf(f, g);
    p.tail = 2;
    return sizeof(float) == 4
        && _Alignof(float) == 4
        && sizeof(struct pair) == 12
        && __builtin_offsetof(struct pair, value) == 4
        && (int)p.value == 3
        && (int)h == 1
        && p.tag + p.tail == 3
        ? 42
        : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_hexadecimal_float_literals() {
    let src = temp_file("hex-float-literals", "c");
    let exe = temp_file("hex-float-literals", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
             double a = 0x1p2;\n\
             double b = 0x1.8p+1;\n\
             double c = 0x.8p-1;\n\
             return (int)(a + b + c) == 7 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn emits_aarch64_assembly_for_float_operations() {
    let src = temp_file("aarch64-float-ops", "c");
    let out = temp_file("aarch64-float-ops", "s");
    std::fs::write(
        &src,
        "float addf(float a, float b) { return a + b; }\n\
         float negf(float a) { return -a; }\n\
         int main(void) { float f = 1.5f; return (int)addf(negf(-f), 2.25f); }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("fadd s"), "{asm}");
    assert!(asm.contains("fneg s"), "{asm}");
    assert!(asm.contains("fcvtzs w"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_x86_64_assembly_for_float_operations() {
    let src = temp_file("x86-float-ops", "c");
    let out = temp_file("x86-float-ops", "s");
    std::fs::write(
        &src,
        "float addf(float a, float b) { return a + b; }\n\
         int main(void) { float f = 1.5f; return (int)addf(f, 2.25f); }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("addss"), "{asm}");
    assert!(
        asm.contains("cvttss2sil") || asm.contains("cvttss2si"),
        "{asm}"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn emits_real_fences_for_atomic_fence_builtins() {
    let src = temp_file("atomic-fence-builtins", "c");
    let x86_out = temp_file("atomic-fence-builtins-x86", "s");
    let a64_out = temp_file("atomic-fence-builtins-a64", "s");
    std::fs::write(
        &src,
        "int main(void) { __atomic_thread_fence(5); __atomic_signal_fence(5); __sync_synchronize(); return 0; }\n",
    )
    .expect("failed to write source");

    let x86 = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&x86_out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(x86.status.success(), "{}", stderr(x86));
    let x86_asm = std::fs::read_to_string(&x86_out).expect("failed to read x86 assembly");
    assert_eq!(x86_asm.matches("mfence").count(), 3, "{x86_asm}");

    let a64 = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&a64_out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(a64.status.success(), "{}", stderr(a64));
    let a64_asm = std::fs::read_to_string(&a64_out).expect("failed to read AArch64 assembly");
    assert_eq!(a64_asm.matches("dmb ish").count(), 3, "{a64_asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(x86_out);
    let _ = std::fs::remove_file(a64_out);
}

#[test]
fn emits_ordering_for_atomic_load_store_and_locked_fetch_builtins() {
    let src = temp_file("atomic-op-builtins", "c");
    let x86_out = temp_file("atomic-op-builtins-x86", "s");
    let a64_out = temp_file("atomic-op-builtins-a64", "s");
    std::fs::write(
        &src,
        "int value;\n\
         int main(void) {\n\
           __atomic_store_n(&value, 1, 5);\n\
           int a = __atomic_load_n(&value, 5);\n\
           int b = __atomic_add_fetch(&value, 2, 5);\n\
           int c = __sync_xor_and_fetch(&value, 3);\n\
           int d = __atomic_exchange_n(&value, 99, 5);\n\
           int expected = 99;\n\
           int e = __atomic_compare_exchange_n(&value, &expected, 123, 0, 5, 5);\n\
           int f = __sync_bool_compare_and_swap(&value, 123, 124);\n\
           int g = __sync_val_compare_and_swap(&value, 124, 125);\n\
           int h = __atomic_fetch_add(&value, 1, 5);\n\
           int i = __sync_fetch_and_xor(&value, 3);\n\
           int j = __atomic_fetch_nand(&value, 7, 5);\n\
           int k = __sync_nand_and_fetch(&value, 1);\n\
           return a + b + c + d + e + f + g + h + i + j + k;\n\
         }\n",
    )
    .expect("failed to write source");

    let x86 = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&x86_out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(x86.status.success(), "{}", stderr(x86));
    let x86_asm = std::fs::read_to_string(&x86_out).expect("failed to read x86 assembly");
    assert_eq!(x86_asm.matches("mfence").count(), 2, "{x86_asm}");
    assert!(x86_asm.contains("lock addl"), "{x86_asm}");
    assert!(x86_asm.contains("lock xorl"), "{x86_asm}");
    assert!(x86_asm.contains("xchgl"), "{x86_asm}");
    assert!(x86_asm.matches("lock cmpxchgl").count() >= 7, "{x86_asm}");

    let a64 = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&a64_out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(a64.status.success(), "{}", stderr(a64));
    let a64_asm = std::fs::read_to_string(&a64_out).expect("failed to read AArch64 assembly");
    assert_eq!(a64_asm.matches("dmb ish").count(), 2, "{a64_asm}");
    assert!(a64_asm.contains("ldaxr"), "{a64_asm}");
    assert!(a64_asm.contains("stlxr"), "{a64_asm}");
    assert!(a64_asm.contains("mvn"), "{a64_asm}");
    assert!(a64_asm.contains("b.ne 2f"), "{a64_asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(x86_out);
    let _ = std::fs::remove_file(a64_out);
}

#[test]
fn rejects_too_few_variadic_function_pointer_arguments_without_panic() {
    let src = temp_file("bad-varargs-fnptr-call", "i");
    std::fs::write(
        &src,
        r#"
int printf(const char *fmt, ...);
int main(void) {
    int (*fp)(const char *, ...) = printf;
    return (*fp)();
}
"#,
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("tacky")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("variadic function pointer called with 0 argument(s), but prototype requires at least 1"),
        "{stderr}"
    );
    assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_case_labels_with_integer_constant_expressions() {
    let src = temp_file("case-constant-expression", "i");
    let exe = temp_file("case-constant-expression", "bin");
    std::fs::write(
        &src,
        "int main(void) { switch (6) { case (1 + 2) * 2: return 0; default: return 1; } }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(0));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn drops_unreachable_code_before_case_label_in_constant_false_if() {
    let src = temp_file("case-in-constant-false-if", "c");
    let exe = temp_file("case-in-constant-false-if", "bin");
    std::fs::write(
        &src,
        r#"
extern void link_error(void);
static int ok;

void hit(void) {
    ok = 1;
}

void run(int x) {
    switch (x) {
    case 0:
        if (0) {
            link_error();
    case 1:
            hit();
        }
    }
}

int main(void) {
    run(1);
    return ok ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_local_array_designated_initializers() {
    let src = temp_file("local-array-designators", "i");
    let exe = temp_file("local-array-designators", "bin");
    std::fs::write(
        &src,
        "int main(void) { int a[5] = { [3] = 7, [1] = 4 }; return a[0] + a[1] * 10 + a[3]; }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(47));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_local_struct_designated_initializers() {
    let src = temp_file("local-struct-designators", "i");
    let exe = temp_file("local-struct-designators", "bin");
    std::fs::write(
        &src,
        "struct S { int x; int a[4]; int y; };\n\
         int main(void) { struct S s = { .y = 5, .a = { [2] = 9 }, .x = 1 }; return s.x + s.a[2] * 10 + s.y; }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(96));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_static_designated_initializers() {
    let src = temp_file("static-designators", "i");
    let exe = temp_file("static-designators", "bin");
    std::fs::write(
        &src,
        "struct S { int a; int b; };\n\
         int g[4] = { [2] = 11 };\n\
         static struct S s = { .b = 7 };\n\
         int main(void) { return g[0] + g[2] + s.a + s.b; }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(18));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_constant_expressions_in_enums_arrays_and_static_initializers() {
    let src = temp_file("constant-expression-contexts", "i");
    let exe = temp_file("constant-expression-contexts", "bin");
    std::fs::write(
        &src,
        "enum { A = 1 + 2, B = A * 4 };\n\
         int g[1 + 2] = { [B / 6] = (3 << 2) + 5 };\n\
         static int s = (B == 12) ? 7 : 1;\n\
         int main(void) { int a[A + 1]; a[0] = 0; return g[2] + s + sizeof(a) - 40; }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(0));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_union_designated_initializers() {
    let src = temp_file("union-designators", "i");
    let exe = temp_file("union-designators", "bin");
    std::fs::write(
        &src,
        "union U { int i; long l; };\n\
         static union U g = { .l = 5L };\n\
         int main(void) { union U u = { .i = 7 }; return u.i + g.l; }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(12));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_flexible_array_member_layout() {
    let src = temp_file("flexible-array-member", "c");
    let exe = temp_file("flexible-array-member", "bin");
    std::fs::write(
        &src,
        "struct packet { int len; char data[]; };\n\
         struct aligned_packet { char tag; long words[]; };\n\
         int main(void) {\n\
           int ok_packet = sizeof(struct packet) == 4 && __builtin_offsetof(struct packet, data) == 4;\n\
           int ok_aligned = sizeof(struct aligned_packet) == 8 && __builtin_offsetof(struct aligned_packet, words) == 8;\n\
           return ok_packet && ok_aligned ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn links_multiple_inputs_to_requested_executable() {
    let main_src = temp_file("multi-main", "i");
    let helper_src = temp_file("multi-helper", "i");
    let exe = temp_file("multi-exe", "bin");
    std::fs::write(
        &main_src,
        "int answer(void);\nint main(void) { return answer(); }\n",
    )
    .expect("failed to write main input");
    std::fs::write(&helper_src, "int answer(void) { return 42; }\n")
        .expect("failed to write helper input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&main_src)
        .arg(&helper_src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(main_src);
    let _ = std::fs::remove_file(helper_src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_and_runs_representative_host_fixtures() {
    for (fixture, expected_status) in [
        ("tests/binary.c", 14),
        ("tests/variables.c", 30),
        ("tests/fibonacci.c", 55),
        ("tests/static_globals.c", 0),
        ("tests/double_simple.c", 5),
    ] {
        let exe = temp_file(
            &format!(
                "host-fixture-{}",
                fixture
                    .rsplit_once('/')
                    .map(|(_, name)| name)
                    .unwrap_or(fixture)
                    .trim_end_matches(".c")
            ),
            "bin",
        );
        let output = Command::new(rnqcc())
            .arg("-o")
            .arg(&exe)
            .arg(fixture)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{}: {}", fixture, stderr(output));
        let run = Command::new(&exe).status().expect("failed to run output");
        assert_eq!(run.code(), Some(expected_status), "fixture {}", fixture);

        let _ = std::fs::remove_file(exe);
    }
}

#[test]
fn compiles_real_project_configure_probe_fixture() {
    let exe = temp_file("real-project-configure-probes", "bin");
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg("tests/fixtures/real_project/configure_probes.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(exe);
}

#[test]
fn preprocess_existing_i_to_stdout_preserves_input() {
    let src = temp_file("preserve", "i");
    let body = "int main(void) { return 7; }\n";
    std::fs::write(&src, body).expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(&src).expect("failed to read source"),
        body
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout was not utf8"),
        body
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn preprocess_existing_i_to_output_copies_input() {
    let src = temp_file("copy-src", "i");
    let out = temp_file("copy-out", "i");
    let body = "int main(void) { return 9; }\n";
    std::fs::write(&src, body).expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-E")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(&src).expect("failed to read source"),
        body
    );
    assert_eq!(
        std::fs::read_to_string(&out).expect("failed to read output"),
        body
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn preprocess_existing_i_to_same_output_is_noop() {
    let src = temp_file("same-output", "i");
    let body = "int main(void) { return 11; }\n";
    std::fs::write(&src, body).expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-E")
        .arg("-o")
        .arg(&src)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(&src).expect("failed to read source"),
        body
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn preprocess_existing_i_to_canonical_same_output_is_noop() {
    let src = temp_file("canonical-same-output", "i");
    let body = "int main(void) { return 12; }\n";
    std::fs::write(&src, body).expect("failed to write test input");
    let canonical = std::fs::canonicalize(&src).expect("failed to canonicalize test input");

    let output = Command::new(rnqcc())
        .arg("-E")
        .arg("-o")
        .arg(&canonical)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert_eq!(
        std::fs::read_to_string(&src).expect("failed to read source"),
        body
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn preprocess_output_overwrites_existing_file() {
    let out = temp_file("preprocess-overwrite", "i");
    std::fs::write(&out, "stale\n").expect("failed to seed output");

    let output = Command::new(rnqcc())
        .arg("-E")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(std::fs::read_to_string(&out)
        .expect("failed to read preprocessed output")
        .contains("return 42;"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn internal_cpp_emits_make_dependencies() {
    let dir = temp_file("internal-cpp-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep header.h");
    let src = dir.join("main source.c");
    std::fs::write(&header, "#define DEP_VALUE 1\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep header.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("--MQ")
        .arg("obj file.o")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("obj\\ file.o:"), "{stdout}");
    assert!(stdout.contains("main\\ source.c"), "{stdout}");
    assert!(stdout.contains("dep\\ header.h"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mg_allows_missing_quoted_header_in_make_dependencies() {
    let dir = temp_file("internal-cpp-mg-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let src = dir.join("source.c");
    std::fs::write(&src, "#include \"generated_missing.h\"\nint value;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-MG")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("source.o:"), "{stdout}");
    assert!(stdout.contains("source.c"), "{stdout}");
    assert!(stdout.contains("generated_missing.h"), "{stdout}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mg_mp_emits_phony_target_for_missing_quoted_header() {
    let dir = temp_file("internal-cpp-mg-mp-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let src = dir.join("source.c");
    std::fs::write(&src, "#include \"generated_missing.h\"\nint value;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-MG")
        .arg("-MP")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("source.o:"), "{stdout}");
    assert!(stdout.contains("source.c"), "{stdout}");
    assert!(stdout.contains("generated_missing.h"), "{stdout}");
    assert!(stdout.contains("\ngenerated_missing.h:\n"), "{stdout}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mg_allows_missing_quoted_header_in_user_make_dependencies() {
    let dir = temp_file("internal-cpp-mm-mg-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let src = dir.join("source.c");
    std::fs::write(&src, "#include \"generated_user_missing.h\"\nint value;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MM")
        .arg("-MG")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("source.o:"), "{stdout}");
    assert!(stdout.contains("source.c"), "{stdout}");
    assert!(stdout.contains("generated_user_missing.h"), "{stdout}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mg_allows_missing_forced_include_in_make_dependencies() {
    let dir = temp_file("internal-cpp-mg-forced-include-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let src = dir.join("source.c");
    std::fs::write(&src, "int value;\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .current_dir(&dir)
        .args([
            "--internal-cpp",
            "-M",
            "-MG",
            "-include",
            "generated_force.h",
            "source.c",
        ])
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("source.o:"), "{stdout}");
    assert!(stdout.contains("source.c"), "{stdout}");
    assert!(stdout.contains("generated_force.h"), "{stdout}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mg_allows_missing_imacros_in_make_dependencies() {
    let dir = temp_file("internal-cpp-mg-imacros-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let src = dir.join("source.c");
    std::fs::write(&src, "int value;\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .current_dir(&dir)
        .args([
            "--internal-cpp",
            "-M",
            "-MG",
            "-imacros",
            "generated_macros.h",
            "source.c",
        ])
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("source.o:"), "{stdout}");
    assert!(stdout.contains("source.c"), "{stdout}");
    assert!(stdout.contains("generated_macros.h"), "{stdout}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_accepts_exact_gnu_mm_for_user_dependencies() {
    let dir = temp_file("internal-cpp-mm", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let sys_dir = dir.join("sys");
    std::fs::create_dir(&sys_dir).expect("failed to create system include dir");
    let header = dir.join("dep.h");
    let sys_header = sys_dir.join("sysdep.h");
    let src = dir.join("main.c");
    std::fs::write(&header, "#define DEP_VALUE 3\n").expect("failed to write header");
    std::fs::write(&sys_header, "#define SYS_VALUE 4\n").expect("failed to write system header");
    std::fs::write(
        &src,
        "#include \"dep.h\"\n#include <sysdep.h>\nint value = DEP_VALUE + SYS_VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MM")
        .arg("--isystem")
        .arg(&sys_dir)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("main.o:"), "{stdout}");
    assert!(stdout.contains("main.c"), "{stdout}");
    assert!(stdout.contains("dep.h"), "{stdout}");
    assert!(!stdout.contains("sysdep.h"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(sys_header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(sys_dir);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mm_keeps_i_headers_but_excludes_isystem_headers() {
    let dir = temp_file("internal-cpp-mm-include-classification", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let user_dir = dir.join("user");
    let sys_dir = dir.join("sys");
    std::fs::create_dir(&user_dir).expect("failed to create user include dir");
    std::fs::create_dir(&sys_dir).expect("failed to create system include dir");
    let user_header = user_dir.join("userdep.h");
    let sys_header = sys_dir.join("sysdep.h");
    let src = dir.join("main.c");
    std::fs::write(&user_header, "#define USER_VALUE 13\n").expect("failed to write header");
    std::fs::write(&sys_header, "#define SYS_VALUE 17\n").expect("failed to write system header");
    std::fs::write(
        &src,
        "#include <userdep.h>\n#include <sysdep.h>\nint value = USER_VALUE + SYS_VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MM")
        .arg("-I")
        .arg(&user_dir)
        .arg("--isystem")
        .arg(&sys_dir)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("main.o:"), "{stdout}");
    assert!(stdout.contains("main.c"), "{stdout}");
    assert!(stdout.contains("userdep.h"), "{stdout}");
    assert!(!stdout.contains("sysdep.h"), "{stdout}");

    let _ = std::fs::remove_file(user_header);
    let _ = std::fs::remove_file(sys_header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(user_dir);
    let _ = std::fs::remove_dir(sys_dir);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_make_dependencies_include_forced_user_header() {
    for flag in ["-M", "-MM"] {
        let dir = temp_file("internal-cpp-forced-deps", "d");
        std::fs::create_dir(&dir).expect("failed to create dep dir");
        let forced = dir.join("forced.h");
        let src = dir.join("main.c");
        std::fs::write(&forced, "#define FORCED_VALUE 9\n").expect("failed to write header");
        std::fs::write(&src, "int value = FORCED_VALUE;\n").expect("failed to write source");

        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .arg(flag)
            .arg("-include")
            .arg(&forced)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{flag}: {}", stderr(output));
        let stdout = stdout(output);
        assert!(stdout.contains("main.o:"), "{flag}: {stdout}");
        assert!(stdout.contains("main.c"), "{flag}: {stdout}");
        assert!(stdout.contains("forced.h"), "{flag}: {stdout}");

        let _ = std::fs::remove_file(forced);
        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_dir(dir);
    }
}

#[test]
fn internal_cpp_make_dependencies_include_imacros_header() {
    let dir = temp_file("internal-cpp-imacros-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("macros.h");
    let src = dir.join("source.c");
    std::fs::write(&header, "#define IMACROS_VALUE 23\n").expect("failed to write header");
    std::fs::write(&src, "int value = IMACROS_VALUE;\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-imacros")
        .arg(&header)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("source.o:"), "{stdout}");
    assert!(stdout.contains("source.c"), "{stdout}");
    assert!(stdout.contains("macros.h"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_user_dependencies_exclude_system_imacros_header() {
    let dir = temp_file("internal-cpp-system-imacros-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let sys_dir = dir.join("sysdir");
    std::fs::create_dir(&sys_dir).expect("failed to create system include dir");
    let header = sys_dir.join("sysmacro.h");
    let src = dir.join("source.c");
    std::fs::write(&header, "#define SYS_IMACROS_VALUE 29\n").expect("failed to write header");
    std::fs::write(&src, "int value = SYS_IMACROS_VALUE;\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MM")
        .arg("--isystem")
        .arg(&sys_dir)
        .arg("-imacros")
        .arg("sysmacro.h")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("source.o:"), "{stdout}");
    assert!(stdout.contains("source.c"), "{stdout}");
    assert!(!stdout.contains("sysmacro.h"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(sys_dir);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mp_emits_phony_dependency_targets() {
    let dir = temp_file("internal-cpp-mp-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    std::fs::write(&header, "#define DEP_VALUE 31\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-MP")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("main.o:"), "{stdout}");
    assert!(stdout.contains("main.c"), "{stdout}");
    assert!(stdout.contains("dep.h"), "{stdout}");
    assert!(
        stdout.contains(&format!("\n{}:\n", header.display())),
        "{stdout}"
    );

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mm_mp_phony_targets_follow_user_dependency_filtering() {
    let dir = temp_file("internal-cpp-mm-mp-filtered-phony-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let user_dir = dir.join("user");
    let sys_dir = dir.join("sys");
    std::fs::create_dir(&user_dir).expect("failed to create user include dir");
    std::fs::create_dir(&sys_dir).expect("failed to create system include dir");
    let user_header = user_dir.join("userdep.h");
    let sys_header = sys_dir.join("sysdep.h");
    let src = dir.join("main.c");
    std::fs::write(&user_header, "#define USER_VALUE 41\n").expect("failed to write header");
    std::fs::write(&sys_header, "#define SYS_VALUE 43\n").expect("failed to write system header");
    std::fs::write(
        &src,
        "#include <userdep.h>\n#include <sysdep.h>\nint value = USER_VALUE + SYS_VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MM")
        .arg("-MP")
        .arg("-I")
        .arg(&user_dir)
        .arg("--isystem")
        .arg(&sys_dir)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("main.o:"), "{stdout}");
    assert!(stdout.contains("main.c"), "{stdout}");
    assert!(stdout.contains("userdep.h"), "{stdout}");
    assert!(
        stdout.contains(&format!("\n{}:\n", user_header.display())),
        "{stdout}"
    );
    assert!(!stdout.contains("sysdep.h"), "{stdout}");
    assert!(
        !stdout.contains(&format!("\n{}:\n", sys_header.display())),
        "{stdout}"
    );

    let _ = std::fs::remove_file(user_header);
    let _ = std::fs::remove_file(sys_header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(user_dir);
    let _ = std::fs::remove_dir(sys_dir);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mmd_mp_writes_phony_targets_to_dependency_file() {
    let dir = temp_file("internal-cpp-mmd-mp-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("main.d");
    std::fs::write(&header, "#define DEP_VALUE 37\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--MMD")
        .arg("-MP")
        .arg("-MF")
        .arg(&dep)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(deps.contains("main.o:"), "{deps}");
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");
    assert!(
        deps.contains(&format!("\n{}:\n", header.display())),
        "{deps}"
    );

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_writes_side_effect_dependencies() {
    let dir = temp_file("internal-cpp-md", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("custom.d");
    std::fs::write(&header, "#define DEP_VALUE 2\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--MD")
        .arg("--MF")
        .arg(&dep)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(deps.contains("main.o:"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_accepts_exact_gnu_md_and_mmd_side_effect_dependencies() {
    for (flag, should_include_system) in [("-MD", true), ("-MMD", false)] {
        let dir = temp_file("internal-cpp-gnu-md", "d");
        std::fs::create_dir(&dir).expect("failed to create dep dir");
        let sys_dir = dir.join("sys");
        std::fs::create_dir(&sys_dir).expect("failed to create system include dir");
        let header = dir.join("dep.h");
        let sys_header = sys_dir.join("sysdep.h");
        let src = dir.join("main.c");
        let dep = dir.join("custom.d");
        std::fs::write(&header, "#define DEP_VALUE 5\n").expect("failed to write header");
        std::fs::write(&sys_header, "#define SYS_VALUE 6\n")
            .expect("failed to write system header");
        std::fs::write(
            &src,
            "#include \"dep.h\"\n#include <sysdep.h>\nint value = DEP_VALUE + SYS_VALUE;\n",
        )
        .expect("failed to write source");

        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .arg(flag)
            .arg("-MF")
            .arg(&dep)
            .arg("--isystem")
            .arg(&sys_dir)
            .arg("-E")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{flag}: {}", stderr(output));
        let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
        assert!(deps.contains("main.o:"), "{flag}: {deps}");
        assert!(deps.contains("main.c"), "{flag}: {deps}");
        assert!(deps.contains("dep.h"), "{flag}: {deps}");
        assert_eq!(
            deps.contains("sysdep.h"),
            should_include_system,
            "{flag}: {deps}"
        );

        let _ = std::fs::remove_file(dep);
        let _ = std::fs::remove_file(header);
        let _ = std::fs::remove_file(sys_header);
        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_dir(sys_dir);
        let _ = std::fs::remove_dir(dir);
    }
}

#[test]
fn internal_cpp_mmd_keeps_i_headers_but_excludes_isystem_headers() {
    let dir = temp_file("internal-cpp-mmd-include-classification", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let user_dir = dir.join("user");
    let sys_dir = dir.join("sys");
    std::fs::create_dir(&user_dir).expect("failed to create user include dir");
    std::fs::create_dir(&sys_dir).expect("failed to create system include dir");
    let user_header = user_dir.join("userdep.h");
    let sys_header = sys_dir.join("sysdep.h");
    let src = dir.join("main.c");
    let dep = dir.join("custom.d");
    std::fs::write(&user_header, "#define USER_VALUE 19\n").expect("failed to write header");
    std::fs::write(&sys_header, "#define SYS_VALUE 23\n").expect("failed to write system header");
    std::fs::write(
        &src,
        "#include <userdep.h>\n#include <sysdep.h>\nint value = USER_VALUE + SYS_VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--MMD")
        .arg("--MF")
        .arg(&dep)
        .arg("-I")
        .arg(&user_dir)
        .arg("--isystem")
        .arg(&sys_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(deps.contains("main.o:"), "{deps}");
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("userdep.h"), "{deps}");
    assert!(!deps.contains("sysdep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(user_header);
    let _ = std::fs::remove_file(sys_header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(user_dir);
    let _ = std::fs::remove_dir(sys_dir);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_mmd_side_effect_dependencies_include_forced_user_header() {
    for flag in ["--MMD", "-MMD"] {
        let dir = temp_file("internal-cpp-forced-mmd", "d");
        std::fs::create_dir(&dir).expect("failed to create dep dir");
        let forced = dir.join("forced.h");
        let src = dir.join("main.c");
        let dep = dir.join("forced.d");
        std::fs::write(&forced, "#define FORCED_VALUE 10\n").expect("failed to write header");
        std::fs::write(&src, "int value = FORCED_VALUE;\n").expect("failed to write source");

        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .arg(flag)
            .arg("-MF")
            .arg(&dep)
            .arg("-include")
            .arg(&forced)
            .arg("-E")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{flag}: {}", stderr(output));
        let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
        assert!(deps.contains("main.o:"), "{flag}: {deps}");
        assert!(deps.contains("main.c"), "{flag}: {deps}");
        assert!(deps.contains("forced.h"), "{flag}: {deps}");

        let _ = std::fs::remove_file(dep);
        let _ = std::fs::remove_file(forced);
        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_dir(dir);
    }
}

#[test]
fn internal_cpp_accepts_separated_gnu_dependency_option_operands() {
    let dir = temp_file("internal-cpp-separated-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("custom separated.d");
    std::fs::write(&header, "#define DEP_VALUE 7\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-MF")
        .arg(&dep)
        .arg("-MT")
        .arg("raw target.o")
        .arg("-MQ")
        .arg("quoted target.o")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(
        deps.starts_with("raw target.o quoted\\ target.o:"),
        "{deps}"
    );
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_preserves_repeated_dependency_targets_in_order() {
    let dir = temp_file("internal-cpp-repeated-dep-targets", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("custom-repeated.d");
    std::fs::write(&header, "#define DEP_VALUE 11\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-MF")
        .arg(&dep)
        .arg("-MT")
        .arg("raw first.o")
        .arg("-MQ")
        .arg("quoted first.o")
        .arg("-MT")
        .arg("raw second.o")
        .arg("-MQ")
        .arg("quoted second.o")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(
        deps.starts_with("raw first.o quoted\\ first.o raw second.o quoted\\ second.o:"),
        "{deps}"
    );
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_preserves_glued_dependency_targets_in_mixed_order() {
    let dir = temp_file("internal-cpp-glued-mixed-dep-targets", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("custom-glued-mixed.d");
    std::fs::write(&header, "#define DEP_VALUE 13\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg(format!("-MF{}", dep.display()))
        .arg(format!("-MQ{}", "quoted first.o"))
        .arg(format!("-MT{}", "raw second.o"))
        .arg(format!("-MQ{}", "quoted third.o"))
        .arg(format!("-MT{}", "raw fourth.o"))
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(
        deps.starts_with("quoted\\ first.o raw second.o quoted\\ third.o raw fourth.o:"),
        "{deps}"
    );
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_m_mf_writes_dependencies_only_to_requested_file() {
    let dir = temp_file("internal-cpp-m-mf-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("only-file.d");
    std::fs::write(&header, "#define DEP_VALUE 12\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-MF")
        .arg(&dep)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.trim().is_empty(), "{stdout}");

    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(deps.contains("main.o:"), "{deps}");
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_md_mf_preprocesses_to_stdout_without_dependency_text() {
    let dir = temp_file("internal-cpp-md-mf-preprocess-stdout", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("side-effect.d");
    std::fs::write(&header, "#define DEP_VALUE 14\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MD")
        .arg("-MF")
        .arg(&dep)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int value = 14;"), "{stdout}");
    assert!(!stdout.contains("main.o:"), "{stdout}");
    assert!(!stdout.contains("dep.h"), "{stdout}");

    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(deps.contains("main.o:"), "{deps}");
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_accepts_glued_gnu_dependency_option_operands() {
    let dir = temp_file("internal-cpp-glued-deps", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let header = dir.join("dep.h");
    let src = dir.join("main.c");
    let dep = dir.join("custom-glued.d");
    std::fs::write(&header, "#define DEP_VALUE 8\n").expect("failed to write header");
    std::fs::write(&src, "#include \"dep.h\"\nint value = DEP_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg(format!("-MF{}", dep.display()))
        .arg(format!("-MT{}", "glued target.o"))
        .arg(format!("-MQ{}", "glued quoted.o"))
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let deps = std::fs::read_to_string(&dep).expect("failed to read dependency file");
    assert!(
        deps.starts_with("glued target.o glued\\ quoted.o:"),
        "{deps}"
    );
    assert!(deps.contains("main.c"), "{deps}");
    assert!(deps.contains("dep.h"), "{deps}");

    let _ = std::fs::remove_file(dep);
    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn emits_assembly_to_requested_output() {
    let out = temp_file("asm", "s");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("main"));
    assert!(asm.contains("ret"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn assembly_output_overwrites_existing_file() {
    let out = temp_file("asm-overwrite", "s");
    std::fs::write(&out, "stale\n").expect("failed to seed output");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("main"));
    assert!(!asm.contains("stale"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn stage_s_emits_assembly_to_requested_output() {
    let out = temp_file("stage-s", "s");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("s")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(std::fs::read_to_string(&out)
        .expect("failed to read assembly output")
        .contains("main"));

    let _ = std::fs::remove_file(out);
}

#[test]
fn internal_cpp_handles_object_and_function_macros_and_quoted_includes() {
    let header = temp_file("internal-cpp-header", "h");
    let src = temp_file("internal-cpp-src", "c");
    let exe = temp_file("internal-cpp-exe", "bin");
    std::fs::write(&header, "#define ADDEND(x) ((x) + 2)\n").expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#include \"{}\"\n#define BASE 40\nint main(void) {{ return ADDEND(BASE); }}\n",
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_handles_conditional_directives() {
    let src = temp_file("internal-cpp-conditionals", "c");
    let exe = temp_file("internal-cpp-conditionals", "bin");
    std::fs::write(
        &src,
        "#define ENABLED 1\n\
         #ifdef ENABLED\n\
         #define VALUE 40\n\
         #else\n\
         #define VALUE 1\n\
         #endif\n\
         #ifndef MISSING\n\
         #define ADDEND 2\n\
         #endif\n\
         #if ENABLED\n\
         int main(void) { return VALUE + ADDEND; }\n\
         #else\n\
         int main(void) { return 0; }\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_ignores_malformed_nonconditional_directives_in_skipped_groups() {
    let src = temp_file("internal-cpp-skipped-bad-directives", "c");
    std::fs::write(
        &src,
        "#if 0\n\
         #define BAD(\n\
         #include \"unterminated.h\n\
         #unknown bad directive\n\
         #endif\n\
         int kept = 3;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int kept = 3;"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_defined_elif_undef_and_expressions() {
    let src = temp_file("internal-cpp-defined-elif", "c");
    let exe = temp_file("internal-cpp-defined-elif", "bin");
    std::fs::write(
        &src,
        "#define A 2\n\
         #define B 3\n\
         #if defined(A) && (A + B * 2 == 8)\n\
         #define VALUE 40\n\
         #elif defined(B)\n\
         #define VALUE 1\n\
         #else\n\
         #define VALUE 0\n\
         #endif\n\
         #undef A\n\
         #if defined A\n\
         int main(void) { return 7; }\n\
         #else\n\
         int main(void) { return VALUE + 2; }\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_short_circuits_if_expressions() {
    let src = temp_file("internal-cpp-short-circuit-if", "c");
    std::fs::write(
        &src,
        "#if 0 && (1 / 0)\n\
         #error skipped and branch\n\
         #endif\n\
         #if 1 || (1 / 0)\n\
         int or_value = 5;\n\
         #endif\n\
         #if 1 ? 1 : (1 / 0)\n\
         int ternary_true_value = 6;\n\
         #endif\n\
         #if 0 ? (1 / 0) : 1\n\
         int ternary_false_value = 7;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int or_value = 5;"), "{stdout}");
    assert!(stdout.contains("int ternary_true_value = 6;"), "{stdout}");
    assert!(stdout.contains("int ternary_false_value = 7;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_if_uses_logical_source_builtins() {
    let src = temp_file("internal-cpp-if-line-builtin", "c");
    std::fs::write(
        &src,
        "#line 88 \"if_line.c\"\n\
         #if __LINE__ == 88\n\
         int line_value = __LINE__;\n\
         #else\n\
         int line_value = 0;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int line_value = 89;"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_has_builtin_predicates() {
    let src = temp_file("internal-cpp-has-builtin", "c");
    std::fs::write(
        &src,
        "#if __has_builtin(__builtin_expect)\n\
         int has_expect = 1;\n\
         #else\n\
         int has_expect = 0;\n\
         #endif\n\
         #if __has_builtin(__rnqcc_missing_builtin)\n\
         int missing_builtin = 1;\n\
         #else\n\
         int missing_builtin = 0;\n\
         #endif\n\
         #if __has_builtin(__atomic_add_fetch)\n\
         int has_atomic_add_fetch = 1;\n\
         #else\n\
         int has_atomic_add_fetch = 0;\n\
         #endif\n\
         #if __has_builtin(__builtin_bswap64)\n\
         int has_bswap64 = 1;\n\
         #else\n\
         int has_bswap64 = 0;\n\
         #endif\n\
         #if __has_builtin(__builtin_expect_with_probability) && __has_builtin(__atomic_thread_fence) && __has_builtin(__sync_synchronize)\n\
         int has_more_compat_builtins = 1;\n\
         #else\n\
         int has_more_compat_builtins = 0;\n\
         #endif\n\
         #if __has_builtin(__builtin_object_size) && __has_builtin(__builtin_dynamic_object_size)\n\
         int has_object_size = 1;\n\
         #else\n\
         int has_object_size = 0;\n\
         #endif\n\
         #if __has_builtin(__builtin_trap)\n\
         int has_trap = 1;\n\
         #else\n\
         int has_trap = 0;\n\
         #endif\n\
         #if __has_builtin(__builtin___memcpy_chk) && __has_builtin(__builtin___strncpy_chk)\n\
         int has_fortified_builtins = 1;\n\
         #else\n\
         int has_fortified_builtins = 0;\n\
         #endif\n\
         #if __has_builtin(__builtin_memchr) && __has_builtin(__builtin_strstr) && __has_builtin(__builtin_strspn)\n\
         int has_search_builtins = 1;\n\
         #else\n\
         int has_search_builtins = 0;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int has_expect = 1;"), "{stdout}");
    assert!(stdout.contains("int missing_builtin = 0;"), "{stdout}");
    assert!(stdout.contains("int has_atomic_add_fetch = 1;"), "{stdout}");
    assert!(stdout.contains("int has_bswap64 = 1;"), "{stdout}");
    assert!(
        stdout.contains("int has_more_compat_builtins = 1;"),
        "{stdout}"
    );
    assert!(stdout.contains("int has_object_size = 1;"), "{stdout}");
    assert!(stdout.contains("int has_trap = 1;"), "{stdout}");
    assert!(
        stdout.contains("int has_fortified_builtins = 1;"),
        "{stdout}"
    );
    assert!(stdout.contains("int has_search_builtins = 1;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_common_has_attribute_predicates() {
    let src = temp_file("internal-cpp-has-attributes", "c");
    std::fs::write(
        &src,
        "#if __has_attribute(packed) && __has_attribute(__unused__)\n\
         int has_attrs = 1;\n\
         #else\n\
         int has_attrs = 0;\n\
         #endif\n\
         #if __has_attribute(nonnull) && __has_attribute(__warn_unused_result__) && __has_attribute(returns_nonnull) && __has_attribute(__noinline__)\n\
         int has_common_function_attrs = 1;\n\
         #else\n\
         int has_common_function_attrs = 0;\n\
         #endif\n\
         #if __has_attribute(pure) && __has_attribute(__const__) && __has_attribute(malloc) && __has_attribute(cold) && __has_attribute(__hot__)\n\
         int has_optimizer_attrs = 1;\n\
         #else\n\
         int has_optimizer_attrs = 0;\n\
         #endif\n\
         #if __has_declspec_attribute(dllexport)\n\
         int has_declspec = 1;\n\
         #else\n\
         int has_declspec = 0;\n\
         #endif\n\
         #if __has_feature(c_static_assert) && __has_extension(c_atomic)\n\
         int has_features = 1;\n\
         #else\n\
         int has_features = 0;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int has_attrs = 1;"), "{stdout}");
    assert!(
        stdout.contains("int has_common_function_attrs = 1;"),
        "{stdout}"
    );
    assert!(stdout.contains("int has_optimizer_attrs = 1;"), "{stdout}");
    assert!(stdout.contains("int has_declspec = 1;"), "{stdout}");
    assert!(stdout.contains("int has_features = 1;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_has_c_attribute_predicates() {
    let src = temp_file("internal-cpp-has-c-attribute", "c");
    std::fs::write(
        &src,
        "#if __has_c_attribute(fallthrough) && __has_c_attribute(nodiscard)\n\
         int has_c_attrs = 1;\n\
         #else\n\
         int has_c_attrs = 0;\n\
         #endif\n\
         #if __has_c_attribute(unrecognized_vendor_attribute)\n\
         int missing_c_attr = 1;\n\
         #else\n\
         int missing_c_attr = 0;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int has_c_attrs = 1;"), "{stdout}");
    assert!(stdout.contains("int missing_c_attr = 0;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_has_warning_predicates() {
    let src = temp_file("internal-cpp-has-warning", "c");
    std::fs::write(
        &src,
        "#if __has_warning(\"-Wunreachable\")\n\
         int has_warning = 1;\n\
         #else\n\
         int has_warning = 0;\n\
         #endif\n\
         #if __has_warning(\"-Wunknown-vendor-warning\")\n\
         int missing_warning = 1;\n\
         #else\n\
         int missing_warning = 0;\n\
         #endif\n\
         #if __has_warning(\"-Wunknown-pragmas\")\n\
         int has_unknown_pragmas_warning = 1;\n\
         #else\n\
         int has_unknown_pragmas_warning = 0;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int has_warning = 1;"), "{stdout}");
    assert!(stdout.contains("int missing_warning = 0;"), "{stdout}");
    assert!(
        stdout.contains("int has_unknown_pragmas_warning = 1;"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_if_handles_prefixed_character_constants() {
    let src = temp_file("internal-cpp-prefixed-character-constants", "c");
    std::fs::write(
        &src,
        "#if L'\\0' - 1 < 0 && u'a' == 'a' && U'\\n' == 10 && u8'x' == 'x'\n\
         int prefixed_chars = 1;\n\
         #else\n\
         int prefixed_chars = 0;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int prefixed_chars = 1;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_preprocesses_linux_limits_header_with_default_search_paths() {
    if !cfg!(target_os = "linux") {
        return;
    }

    let src = temp_file("internal-cpp-linux-limits-default-search", "c");
    std::fs::write(
        &src,
        "#include <limits.h>\n\
         int rnqcc_limits_probe = CHAR_BIT + (INT_MAX > 0);\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int rnqcc_limits_probe ="), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_predefines_gcc_integer_constant_macros() {
    let src = temp_file("internal-cpp-gcc-integer-constant-macros", "c");
    std::fs::write(
        &src,
        "long a = __INT16_C(-2);\n\
         unsigned int b = __UINT32_C(4);\n\
         long c = __INT64_C(5);\n\
         unsigned long d = __UINTMAX_C(8);\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("long a = -2;"), "{stdout}");
    assert!(stdout.contains("unsigned int b = 4U;"), "{stdout}");
    assert!(stdout.contains("long c = 5L;"), "{stdout}");
    assert!(stdout.contains("unsigned long d = 8UL;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn assignment_from_void_pointer_preserves_declared_pointer_depth() {
    let src = temp_file("void-pointer-assignment-keeps-declared-depth", "c");
    let exe = temp_file("void-pointer-assignment-keeps-declared-depth", "bin");
    std::fs::write(
        &src,
        "void *malloc(unsigned long);\n\
         unsigned long strlen(const char *);\n\
         char **search_path;\n\
         int main(void) {\n\
             search_path = malloc(sizeof(char *));\n\
             search_path[0] = \"abc\";\n\
             return strlen(search_path[0]) == 3 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn typedef_return_type_does_not_leak_into_function_parameters() {
    let src = temp_file("typedef-return-does-not-leak-into-params", "c");
    let exe = temp_file("typedef-return-does-not-leak-into-params", "bin");
    std::fs::write(
        &src,
        "typedef int pid_t;\n\
         struct command { int value; };\n\
         int seen;\n\
         void run_child(const struct command *cmd) { seen = cmd->value; }\n\
         pid_t fork_command(const struct command *cmd) { run_child(cmd); return 0; }\n\
         int main(void) { struct command c = { 42 }; fork_command(&c); return seen; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn aarch64_macos_links_external_libc_data_symbols() {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return;
    }

    let src = temp_file("aarch64-macos-external-libc-data", "c");
    let exe = temp_file("aarch64-macos-external-libc-data", "bin");
    std::fs::write(
        &src,
        "#include <stdio.h>\n\
         int main(void) { return stdin != 0 && stdout != 0 ? 42 : 1; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn global_initializer_accepts_sizeof_array_expression() {
    let src = temp_file("global-sizeof-array-initializer", "c");
    let exe = temp_file("global-sizeof-array-initializer", "bin");
    std::fs::write(
        &src,
        "static const char payload[] = \"abc\";\n\
         static const unsigned long payload_len = sizeof(payload) - 1;\n\
         int main(void) { return payload_len == 3 ? 42 : 1; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn global_initializer_accepts_wrapping_unsigned_sizeof_expression() {
    let src = temp_file("global-wrapping-unsigned-initializer", "c");
    let exe = temp_file("global-wrapping-unsigned-initializer", "bin");
    std::fs::write(
        &src,
        "static unsigned long aa[] = { (1UL << (sizeof(long) * 8 - 1)) - 0xfff };\n\
         static unsigned long bb[] = { (1UL << (sizeof(long) * 8 - 1)) - 0xfff };\n\
         int main(void) { return aa[0] == bb[0] ? 42 : 1; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn address_of_struct_compound_literal() {
    let src = temp_file("address-of-struct-compound-literal", "c");
    let exe = temp_file("address-of-struct-compound-literal", "bin");
    std::fs::write(
        &src,
        "struct s { char *p; int t; };\n\
         static int check(struct s *p) { return p->t == 1 && p->p[0] == 'h'; }\n\
         int main(void) { return check(&(struct s){ \"hi\", 1 }) ? 42 : 1; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn permits_old_c_implicit_function_calls() {
    let src = temp_file("old-c-implicit-function-calls", "c");
    let exe = temp_file("old-c-implicit-function-calls", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
             if (ret42() != 42) abort();\n\
             return 42;\n\
         }\n\
         int ret42(void) { return 42; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--Wno-missing-return")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn implicit_function_call_passes_arguments_in_registers() {
    let src = temp_file("implicit-function-call-args", "c");
    let exe = temp_file("implicit-function-call-args", "bin");
    std::fs::write(&src, "int main(void) { exit(42); return 1; }\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--Wno-missing-return")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn parses_and_runs_old_style_function_definitions() {
    let src = temp_file("old-style-function-definitions", "c");
    let exe = temp_file("old-style-function-definitions", "bin");
    std::fs::write(
        &src,
        "sum(p, n)\n\
             int *p;\n\
             int n;\n\
         {\n\
             return p[0] + n;\n\
         }\n\
         int main(void) {\n\
             int value = 40;\n\
             return sum(&value, 2);\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--Wno-missing-return")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn function_name_identifier_expands_to_current_function() {
    let src = temp_file("function-name-identifier", "c");
    let exe = temp_file("function-name-identifier", "bin");
    std::fs::write(
        &src,
        "int strcmp(const char *, const char *);\n\
         int main(void) { return strcmp(__FUNCTION__, \"main\") == 0 ? 42 : 1; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_basic_virtual_compatibility_headers() {
    let src = temp_file("internal-cpp-virtual-headers", "c");
    let exe = temp_file("internal-cpp-virtual-headers", "bin");
    std::fs::write(
        &src,
        "#include <assert.h>\n\
         #include <stdbool.h>\n\
         #include <stddef.h>\n\
         #include <stdarg.h>\n\
         #include <limits.h>\n\
         #include <iso646.h>\n\
         static_assert(sizeof(int) == 4, \"int size\");\n\
         struct Item { char tag; int value; };\n\
         int main(void) {\n\
             bool ok = true;\n\
             assert(ok);\n\
             size_t offset = offsetof(struct Item, value);\n\
             ptrdiff_t diff = (char *)&ok - (char *)&ok;\n\
             va_list ap;\n\
             return ok && !false && NULL == (void *)0 && offset == 4 &&\n\
                    diff == 0 && sizeof(ap) == sizeof(char *) && (ok and not false) &&\n\
                    (1 bitand 3) == 1 && (1 bitor 2) == 3 && (1 xor 3) == 2 &&\n\
                    CHAR_BIT == 8 &&\n\
                    SHRT_MAX == 32767 && USHRT_MAX == 65535 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_assert_h_honors_ndebug() {
    let src = temp_file("internal-cpp-assert-ndebug", "c");
    let exe = temp_file("internal-cpp-assert-ndebug", "bin");
    std::fs::write(
        &src,
        "#define NDEBUG\n\
         #include <assert.h>\n\
         int main(void) { assert(0); return 42; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_stdatomic_virtual_header() {
    let src = temp_file("internal-cpp-stdatomic-header", "c");
    let exe = temp_file("internal-cpp-stdatomic-header", "bin");
    std::fs::write(
        &src,
        "#include <stdatomic.h>\n\
         #ifdef __STDC_NO_ATOMICS__\n\
         #error atomics should be advertised\n\
         #endif\n\
         atomic_int initialized = ATOMIC_VAR_INIT(3);\n\
         int main(void) {\n\
             atomic_int value;\n\
             atomic_flag flag = ATOMIC_FLAG_INIT;\n\
             memory_order order = memory_order_seq_cst;\n\
             atomic_init(&value, 9);\n\
             atomic_store(&value, 10);\n\
             int loaded = atomic_load_explicit(&value, order);\n\
             int old = atomic_fetch_add(&value, 5);\n\
             int exchanged = atomic_exchange_explicit(&value, 20, memory_order_relaxed);\n\
             int expected = 20;\n\
             int matched = atomic_compare_exchange_strong(&value, &expected, 42);\n\
             expected = 42;\n\
             int weak_matched = atomic_compare_exchange_weak_explicit(&value, &expected, 43, memory_order_seq_cst, memory_order_seq_cst);\n\
             int was_set = atomic_flag_test_and_set(&flag);\n\
             int now_set = atomic_flag_test_and_set_explicit(&flag, memory_order_seq_cst);\n\
             atomic_flag_clear(&flag);\n\
             int after_clear = atomic_flag_test_and_set(&flag);\n\
             atomic_flag_clear_explicit(&flag, memory_order_seq_cst);\n\
             atomic_thread_fence(memory_order_seq_cst);\n\
             atomic_signal_fence(memory_order_seq_cst);\n\
             return ATOMIC_INT_LOCK_FREE == 2 && atomic_is_lock_free(&value) &&\n\
                    initialized == 3 && loaded == 10 && old == 10 && exchanged == 15 &&\n\
                    matched && weak_matched && value == 43 && !was_set && now_set && !after_clear ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_common_hosted_library_virtual_headers() {
    let src = temp_file("internal-cpp-hosted-library-headers", "c");
    let exe = temp_file("internal-cpp-hosted-library-headers", "bin");
    std::fs::write(
        &src,
        "#include <stddef.h>\n\
         #include <stdio.h>\n\
         #include <stdlib.h>\n\
         #include <string.h>\n\
         int main(void) {\n\
             char buf[32];\n\
             memset(buf, 0, sizeof(buf));\n\
             strcpy(buf, \"x\");\n\
             strcat(buf, \"7\");\n\
             int written = snprintf(buf + 2, sizeof(buf) - 2, \"%d\", 5);\n\
             char *heap = malloc(8);\n\
             memcpy(heap, buf, 4);\n\
             int ok = strlen(buf) == 3 && strcmp(buf, \"x75\") == 0 &&\n\
                      memcmp(heap, \"x75\", 4) == 0 && memchr(heap, '7', 3) == heap + 1 &&\n\
                      strtol(\"42\", NULL, 10) == 42 && abs(-3) == 3 && written == 1;\n\
             free(heap);\n\
             return ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_more_common_hosted_virtual_headers() {
    let src = temp_file("internal-cpp-more-hosted-headers", "c");
    let exe = temp_file("internal-cpp-more-hosted-headers", "bin");
    std::fs::write(
        &src,
        "#include <ctype.h>\n\
         #include <errno.h>\n\
         #include <math.h>\n\
         #include <time.h>\n\
         int main(void) {\n\
             errno = 0;\n\
             struct tm tm;\n\
             tm.tm_sec = 1;\n\
             tm.tm_min = 2;\n\
             tm.tm_hour = 3;\n\
             tm.tm_mday = 4;\n\
             tm.tm_mon = 5;\n\
             tm.tm_year = 126;\n\
             tm.tm_wday = 0;\n\
             tm.tm_yday = 0;\n\
             tm.tm_isdst = -1;\n\
             time_t now = 0;\n\
             size_t written = strftime((char *)0, 0, \"%Y\", &tm);\n\
             int ctype_ok = isdigit('7') && isalpha('A') && isspace(' ') &&\n\
                            toupper('a') == 'A' && tolower('Z') == 'z';\n\
             int errno_ok = EDOM > 0 && ERANGE > 0 && EILSEQ > 0 && errno == 0;\n\
             int math_ok = HUGE_VAL > 1.0 && INFINITY > 1.0 && MATH_ERRNO == 1;\n\
             int time_ok = sizeof(clock_t) == sizeof(long) && sizeof(time_t) == sizeof(long) &&\n\
                           CLOCKS_PER_SEC > 0 && time(&now) != (time_t)-1 && written == 0;\n\
             return ctype_ok && errno_ok && math_ok && time_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_common_posix_virtual_headers() {
    let src = temp_file("internal-cpp-posix-headers", "c");
    let exe = temp_file("internal-cpp-posix-headers", "bin");
    std::fs::write(
        &src,
        "#include <sys/types.h>\n\
         #include <sys/stat.h>\n\
         #include <fcntl.h>\n\
         #include <unistd.h>\n\
         int main(void) {\n\
             struct stat st;\n\
             pid_t pid = getpid();\n\
             mode_t user_rw = S_IRUSR | S_IWUSR;\n\
             int fd = open(\".\", O_RDONLY);\n\
             int closed = fd >= 0 ? close(fd) : 0;\n\
             int type_ok = sizeof(size_t) == sizeof(unsigned long) &&\n\
                           sizeof(ssize_t) == sizeof(long) && sizeof(off_t) == sizeof(long) &&\n\
                           sizeof(st.st_size) == sizeof(long);\n\
             int macro_ok = STDIN_FILENO == 0 && STDOUT_FILENO == 1 && SEEK_SET == 0 &&\n\
                            user_rw == 0600 && S_ISDIR(S_IFDIR) && S_ISREG(S_IFREG) &&\n\
                            !S_ISREG(S_IFDIR) && (O_RDONLY | O_CREAT) == O_CREAT;\n\
             return pid > 0 && closed == 0 && type_ok && macro_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_more_posix_virtual_headers() {
    let src = temp_file("internal-cpp-more-posix-headers", "c");
    let exe = temp_file("internal-cpp-more-posix-headers", "bin");
    std::fs::write(
        &src,
        "#include <sys/types.h>\n\
         #include <sys/wait.h>\n\
         #include <sys/utsname.h>\n\
         #include <pwd.h>\n\
         #include <grp.h>\n\
         #include <strings.h>\n\
         int main(void) {\n\
             int status = 42 << 8;\n\
             struct utsname uts;\n\
             struct passwd pw;\n\
             struct group gr;\n\
             char buf[8];\n\
             uts.sysname[0] = 'r';\n\
             pw.pw_uid = 7;\n\
             pw.pw_gid = 8;\n\
             pw.pw_name = \"user\";\n\
             gr.gr_gid = 9;\n\
             gr.gr_name = \"group\";\n\
             bzero(buf, sizeof(buf));\n\
             int wait_ok = WIFEXITED(status) && WEXITSTATUS(status) == 42 &&\n\
                           !WIFSIGNALED(status) && WNOHANG == 1;\n\
             int type_ok = sizeof(uid_t) == sizeof(unsigned int) &&\n\
                           sizeof(gid_t) == sizeof(unsigned int) && sizeof(pid_t) == sizeof(int);\n\
             int struct_ok = sizeof(uts.sysname) >= 64 && pw.pw_uid == 7 && gr.gr_gid == 9;\n\
             int proto_ok = sizeof(ffs(16)) == sizeof(int) && sizeof(strcasecmp(\"a\", \"A\")) == sizeof(int);\n\
             return wait_ok && type_ok && struct_ok && proto_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_posix_at_and_fcntl_probe_surface() {
    let src = temp_file("internal-cpp-posix-at-fcntl", "c");
    let exe = temp_file("internal-cpp-posix-at-fcntl", "bin");
    std::fs::write(
        &src,
        "#include <sys/types.h>\n\
         #include <fcntl.h>\n\
         #include <unistd.h>\n\
         int main(void) {\n\
             struct flock lock;\n\
             lock.l_type = F_RDLCK;\n\
             lock.l_whence = SEEK_SET;\n\
             lock.l_start = 0;\n\
             lock.l_len = 0;\n\
             lock.l_pid = 0;\n\
             int flags_ok = ((O_RDONLY | O_WRONLY | O_RDWR) & O_ACCMODE) == O_ACCMODE &&\n\
                            O_CLOEXEC != 0 && O_DIRECTORY != 0 && O_NOFOLLOW != 0 &&\n\
                            AT_FDCWD < 0 && AT_SYMLINK_NOFOLLOW != 0 && AT_REMOVEDIR != 0;\n\
             int fcntl_ok = F_DUPFD == 0 && F_GETFD == 1 && F_SETLK != F_SETLKW &&\n\
                            F_UNLCK != F_WRLCK;\n\
             int proto_ok = sizeof(openat(AT_FDCWD, \".\", O_RDONLY | O_CLOEXEC)) == sizeof(int) &&\n\
                            sizeof(faccessat(AT_FDCWD, \".\", F_OK, 0)) == sizeof(int) &&\n\
                            sizeof(ftruncate(0, (off_t)0)) == sizeof(int) &&\n\
                            sizeof(truncate(\"x\", (off_t)0)) == sizeof(int) &&\n\
                            sizeof(posix_fadvise(0, (off_t)0, (off_t)0, POSIX_FADV_NORMAL)) == sizeof(int) &&\n\
                            sizeof(posix_fallocate(0, (off_t)0, (off_t)0)) == sizeof(int);\n\
             return flags_ok && fcntl_ok && proto_ok && sizeof(lock.l_start) == sizeof(off_t) ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_networking_virtual_headers() {
    let src = temp_file("internal-cpp-networking-headers", "c");
    let exe = temp_file("internal-cpp-networking-headers", "bin");
    std::fs::write(
        &src,
        "#include <sys/types.h>\n\
         #include <sys/time.h>\n\
         #include <sys/select.h>\n\
         #include <sys/uio.h>\n\
         #include <sys/socket.h>\n\
         #include <netinet/in.h>\n\
         #include <arpa/inet.h>\n\
         #include <poll.h>\n\
         int main(void) {\n\
             fd_set set;\n\
             struct timeval tv;\n\
             struct iovec iov;\n\
             struct sockaddr_storage storage;\n\
             struct sockaddr_in addr;\n\
             struct pollfd pfd;\n\
             char text[INET6_ADDRSTRLEN];\n\
             tv.tv_sec = 0;\n\
             tv.tv_usec = 0;\n\
             iov.iov_base = text;\n\
             iov.iov_len = sizeof(text);\n\
             storage.ss_family = AF_INET;\n\
             addr.sin_family = AF_INET;\n\
             addr.sin_port = 0;\n\
             addr.sin_addr.s_addr = INADDR_LOOPBACK;\n\
             pfd.fd = -1;\n\
             pfd.events = POLLIN | POLLOUT;\n\
             socklen_t len = sizeof(addr);\n\
             int family_ok = AF_INET == PF_INET && SOCK_STREAM != SOCK_DGRAM && IPPROTO_TCP == 6;\n\
             int macro_ok = INET_ADDRSTRLEN == 16 && INET6_ADDRSTRLEN == 46 && MSG_PEEK > 0;\n\
             int type_ok = sizeof(socklen_t) == sizeof(unsigned int) && sizeof(sa_family_t) == sizeof(unsigned short) &&\n\
                           sizeof(fd_set) >= sizeof(unsigned long) && sizeof(struct iovec) >= sizeof(void *) + sizeof(size_t);\n\
             return family_ok && macro_ok && type_ok && len > 0 && pfd.events == (POLLIN | POLLOUT) ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_more_unix_probe_virtual_headers() {
    let src = temp_file("internal-cpp-more-unix-probe-headers", "c");
    let exe = temp_file("internal-cpp-more-unix-probe-headers", "bin");
    std::fs::write(
        &src,
        "#include <sys/types.h>\n\
         #include <sys/mman.h>\n\
         #include <sys/resource.h>\n\
         #include <sys/ioctl.h>\n\
         #include <termios.h>\n\
         #include <netdb.h>\n\
         int main(void) {\n\
             struct rlimit limit;\n\
             struct rusage usage;\n\
             struct winsize window;\n\
             struct termios term;\n\
             struct addrinfo hints;\n\
             struct hostent host;\n\
             limit.rlim_cur = 64;\n\
             limit.rlim_max = RLIM_INFINITY;\n\
             usage.ru_utime.tv_sec = 1;\n\
             usage.ru_stime.tv_usec = 2;\n\
             window.ws_row = 24;\n\
             window.ws_col = 80;\n\
             term.c_iflag = ICRNL | IXON;\n\
             term.c_cflag = CS8 | CREAD;\n\
             term.c_lflag = ICANON | ECHO | ISIG;\n\
             term.c_cc[VMIN] = 1;\n\
             term.c_ispeed = B9600;\n\
             term.c_ospeed = B115200;\n\
             hints.ai_flags = AI_PASSIVE | AI_NUMERICHOST;\n\
             hints.ai_family = AF_INET;\n\
             hints.ai_socktype = SOCK_STREAM;\n\
             hints.ai_addrlen = sizeof(struct sockaddr_in);\n\
             host.h_addrtype = AF_INET;\n\
             host.h_length = 4;\n\
             int mman_ok = PROT_READ == 1 && (PROT_WRITE | PROT_EXEC) == 6 &&\n\
                           MAP_ANONYMOUS == MAP_ANON && MAP_FAILED != 0;\n\
             int resource_ok = sizeof(rlim_t) == sizeof(unsigned long) &&\n\
                               limit.rlim_cur == 64 && limit.rlim_max == RLIM_INFINITY &&\n\
                               usage.ru_utime.tv_sec + usage.ru_stime.tv_usec == 3 &&\n\
                               RUSAGE_CHILDREN < RUSAGE_SELF;\n\
             int ioctl_ok = sizeof(struct winsize) == 4 * sizeof(unsigned short) &&\n\
                            window.ws_row == 24 && window.ws_col == 80 && TIOCGWINSZ != TIOCSWINSZ;\n\
             int term_ok = sizeof(cc_t) == sizeof(unsigned char) && sizeof(speed_t) == sizeof(unsigned int) &&\n\
                           sizeof(term.c_cc) == NCCS * sizeof(cc_t) && term.c_cc[VMIN] == 1 &&\n\
                           term.c_ispeed == B9600 && term.c_ospeed == B115200;\n\
             int netdb_ok = sizeof(socklen_t) == sizeof(unsigned int) && hints.ai_family == AF_INET &&\n\
                            hints.ai_socktype == SOCK_STREAM && hints.ai_addrlen > 0 &&\n\
                            host.h_addrtype == AF_INET && host.h_length == 4 && NI_MAXHOST > NI_MAXSERV;\n\
             return mman_ok && resource_ok && ioctl_ok && term_ok && netdb_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_additional_posix_probe_virtual_headers() {
    let src = temp_file("internal-cpp-additional-posix-probe-headers", "c");
    let exe = temp_file("internal-cpp-additional-posix-probe-headers", "bin");
    std::fs::write(
        &src,
        "#include <regex.h>\n\
         #include <glob.h>\n\
         #include <fnmatch.h>\n\
         #include <dlfcn.h>\n\
         #include <syslog.h>\n\
         #include <utime.h>\n\
         #include <sys/un.h>\n\
         #include <ifaddrs.h>\n\
         #include <net/if.h>\n\
         #include <netinet/tcp.h>\n\
         int main(void) {\n\
             regex_t regex;\n\
             regmatch_t match;\n\
             glob_t glob_result;\n\
             struct utimbuf times;\n\
             struct sockaddr_un un;\n\
             struct ifaddrs addrs;\n\
             struct if_nameindex nameindex;\n\
             regex.re_nsub = 2;\n\
             match.rm_so = 1;\n\
             match.rm_eo = 3;\n\
             glob_result.gl_pathc = 0;\n\
             glob_result.gl_offs = 0;\n\
             times.actime = 4;\n\
             times.modtime = 5;\n\
             un.sun_family = AF_UNIX;\n\
             un.sun_path[0] = 'x';\n\
             addrs.ifa_next = 0;\n\
             addrs.ifa_name = \"lo\";\n\
             addrs.ifa_flags = IFF_UP | IFF_LOOPBACK;\n\
             addrs.ifa_addr = (struct sockaddr *)&un;\n\
             nameindex.if_index = 1;\n\
             nameindex.if_name = \"lo\";\n\
             int regex_ok = REG_EXTENDED == 1 && REG_NOMATCH != REG_BADPAT &&\n\
                            sizeof(regex_t) >= sizeof(size_t) && match.rm_eo - match.rm_so == 2;\n\
             int glob_ok = GLOB_NOMATCH != GLOB_ABORTED && sizeof(glob_result.gl_pathv) == sizeof(char **);\n\
             int fnmatch_ok = FNM_NOMATCH == 1 && (FNM_PATHNAME | FNM_PERIOD) != 0;\n\
             int dlfcn_ok = RTLD_NOW != RTLD_LAZY && RTLD_DEFAULT == (void *)0 && RTLD_NEXT != (void *)0;\n\
             int syslog_ok = LOG_MASK(LOG_ERR) == (1 << LOG_ERR) && LOG_UPTO(LOG_WARNING) >= LOG_MASK(LOG_WARNING) && LOG_LOCAL7 > LOG_LOCAL0;\n\
             int unix_ok = un.sun_family == AF_UNIX && sizeof(un.sun_path) >= 100;\n\
             int if_ok = IF_NAMESIZE >= 16 && addrs.ifa_flags == (IFF_UP | IFF_LOOPBACK) && nameindex.if_index == 1;\n\
             int tcp_ok = TCP_NODELAY != TCP_MAXSEG && TCP_KEEPIDLE != TCP_KEEPINTVL;\n\
             return regex_ok && glob_ok && fnmatch_ok && dlfcn_ok && syslog_ok &&\n\
                    times.actime + times.modtime == 9 && unix_ok && if_ok && tcp_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_autoconf_probe_virtual_headers() {
    let src = temp_file("internal-cpp-autoconf-probe-headers", "c");
    let exe = temp_file("internal-cpp-autoconf-probe-headers", "bin");
    std::fs::write(
        &src,
        "#include <libgen.h>\n\
         #include <paths.h>\n\
         #include <sysexits.h>\n\
         #include <sys/file.h>\n\
         #include <sys/param.h>\n\
         #include <sys/sysmacros.h>\n\
         int main(void) {\n\
             char path[] = \"/tmp/file\";\n\
             char *base = basename(path);\n\
             char *dir = dirname(path);\n\
             int path_ok = _PATH_DEVNULL[0] == '/' && _PATH_TMP[0] == '/' &&\n\
                           _PATH_DEFPATH[0] == '/' && _PATH_STDPATH[0] == '/' &&\n\
                           _PATH_TTY[0] == '/' && _PATH_VI[0] == '/';\n\
             int exit_ok = EX_OK == 0 && EX__BASE == EX_USAGE && EX_CONFIG == EX__MAX;\n\
             int lock_ok = LOCK_SH != LOCK_EX && LOCK_UN != LOCK_NB;\n\
             int param_ok = MAXPATHLEN >= 1024 && MIN(3, 5) == 3 && MAX(3, 5) == 5 &&\n\
                            howmany(17, 8) == 3 && roundup(17, 8) == 24 &&\n\
                            MAXSYMLINKS > 0 && powerof2(8);\n\
             dev_t dev = makedev(3, 7);\n\
             int sysmacros_ok = major(dev) == 3 && minor(dev) == 7;\n\
             return base && dir && path_ok && exit_ok && lock_ok && param_ok && sysmacros_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_getopt_virtual_header() {
    let src = temp_file("internal-cpp-getopt-header", "c");
    let exe = temp_file("internal-cpp-getopt-header", "bin");
    std::fs::write(
        &src,
        "#include <getopt.h>\n\
         int main(void) {\n\
             struct option options[4];\n\
             int flag = 0;\n\
             int index = -1;\n\
             int (*short_fn)(int, char *const [], const char *) = getopt;\n\
             int (*long_fn)(int, char *const [], const char *, const struct option *, int *) = getopt_long;\n\
             options[0].name = \"help\";\n\
             options[0].has_arg = no_argument;\n\
             options[0].flag = &flag;\n\
             options[0].val = 'h';\n\
             options[1].name = \"output\";\n\
             options[1].has_arg = required_argument;\n\
             options[1].flag = 0;\n\
             options[1].val = 'o';\n\
             options[2].name = \"color\";\n\
             options[2].has_arg = optional_argument;\n\
             options[2].flag = 0;\n\
             options[2].val = 1;\n\
             options[3].name = 0;\n\
             options[3].has_arg = 0;\n\
             options[3].flag = 0;\n\
             options[3].val = 0;\n\
             int layout_ok = sizeof(struct option) >= sizeof(void *) * 2;\n\
             int macros_ok = no_argument == 0 && required_argument == 1 && optional_argument == 2;\n\
             return short_fn && long_fn && options[0].flag == &flag && index == -1 &&\n\
                    layout_ok && macros_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_linux_alias_probe_virtual_headers() {
    let src = temp_file("internal-cpp-linux-alias-probe-headers", "c");
    let exe = temp_file("internal-cpp-linux-alias-probe-headers", "bin");
    std::fs::write(
        &src,
        "#include <alloca.h>\n\
         #include <malloc.h>\n\
         #include <memory.h>\n\
         #include <sys/errno.h>\n\
         #include <linux/limits.h>\n\
         int main(void) {\n\
             void *(*malloc_fn)(size_t) = malloc;\n\
             void *(*memcpy_fn)(void *, const void *, size_t) = memcpy;\n\
             int limits_ok = PATH_MAX >= 1024 && NAME_MAX >= 255 && PIPE_BUF >= 512;\n\
             int errno_ok = EINVAL != ENOENT && EACCES != 0;\n\
             return malloc_fn && memcpy_fn && limits_ok && errno_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_network_probe_alias_headers() {
    let src = temp_file("internal-cpp-network-probe-alias-headers", "c");
    let exe = temp_file("internal-cpp-network-probe-alias-headers", "bin");
    std::fs::write(
        &src,
        "#include <sys/poll.h>\n\
         #include <netinet/ip.h>\n\
         #include <netinet/udp.h>\n\
         #include <resolv.h>\n\
         int main(void) {\n\
             struct pollfd pfd;\n\
             struct ip packet;\n\
             struct udphdr udp;\n\
             res_state resolver = 0;\n\
             pfd.fd = 3;\n\
             pfd.events = POLLIN | POLLOUT;\n\
             pfd.revents = 0;\n\
             packet.ip_v = IPVERSION;\n\
             packet.ip_ttl = 64;\n\
             packet.ip_src.s_addr = INADDR_LOOPBACK;\n\
             packet.ip_dst.s_addr = INADDR_ANY;\n\
             udp.uh_sport = 53;\n\
             udp.uh_dport = 5353;\n\
             udp.uh_ulen = 8;\n\
             int poll_ok = sizeof(nfds_t) == sizeof(unsigned long) && (pfd.events & POLLIN) != 0;\n\
             int ip_ok = packet.ip_v == 4 && packet.ip_ttl == 64 && IP_MAXPACKET > 1024;\n\
             int udp_ok = udp.uh_dport > udp.uh_sport && udp.uh_ulen == 8;\n\
             int resolv_ok = resolver == 0 && NS_PACKETSZ == 512 && (RES_RECURSE | RES_DEFNAMES) != 0;\n\
             return poll_ok && ip_ok && udp_ok && resolv_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_virtual_headers_tolerate_repeated_includes() {
    let src = temp_file("internal-cpp-repeat-virtual-headers", "c");
    let exe = temp_file("internal-cpp-repeat-virtual-headers", "bin");
    std::fs::write(
        &src,
        "#include <stddef.h>\n\
         #include <stddef.h>\n\
         #include <stdarg.h>\n\
         #include <stdarg.h>\n\
         #include <stdint.h>\n\
         #include <stdint.h>\n\
         #include <stdatomic.h>\n\
         #include <stdatomic.h>\n\
         #include <stdio.h>\n\
         #include <stdio.h>\n\
         #include <stdlib.h>\n\
         #include <stdlib.h>\n\
         #include <string.h>\n\
         #include <string.h>\n\
         #include <time.h>\n\
         #include <time.h>\n\
         #include <sys/types.h>\n\
         #include <sys/types.h>\n\
         #include <sys/stat.h>\n\
         #include <sys/stat.h>\n\
         #include <fcntl.h>\n\
         #include <fcntl.h>\n\
         #include <unistd.h>\n\
         #include <unistd.h>\n\
         int main(void) {\n\
             atomic_int value;\n\
             struct tm tm;\n\
             struct stat st;\n\
             int64_t wide = INT64_C(40);\n\
             atomic_store(&value, 2);\n\
             tm.tm_year = 126;\n\
             st.st_mode = S_IFREG;\n\
             return sizeof(va_list) == sizeof(char *) && sizeof(size_t) == sizeof(unsigned long) &&\n\
                    S_ISREG(st.st_mode) && tm.tm_year == 126 && atomic_load(&value) + wide == 42 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_signal_and_setjmp_virtual_headers() {
    let src = temp_file("internal-cpp-signal-setjmp-headers", "c");
    let exe = temp_file("internal-cpp-signal-setjmp-headers", "bin");
    std::fs::write(
        &src,
        "#include <signal.h>\n\
         #include <signal.h>\n\
         #include <setjmp.h>\n\
         #include <setjmp.h>\n\
         int main(void) {\n\
             jmp_buf env;\n\
             sig_atomic_t flag = 1;\n\
             void (*handler)(int) = SIG_DFL;\n\
             return sizeof(env) >= sizeof(long) && sizeof(sig_atomic_t) == sizeof(int) &&\n\
                    flag == 1 && handler == SIG_DFL && SIGINT > 0 && SIGTERM > SIGINT ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_more_system_virtual_headers() {
    let src = temp_file("internal-cpp-more-system-headers", "c");
    let exe = temp_file("internal-cpp-more-system-headers", "bin");
    std::fs::write(
        &src,
        "#include <dirent.h>\n\
         #include <locale.h>\n\
         #include <pthread.h>\n\
         #include <sys/time.h>\n\
         #include <wchar.h>\n\
         #include <wctype.h>\n\
         int thread_entry_called;\n\
         void *thread_entry(void *arg) { thread_entry_called = arg != 0; return arg; }\n\
         int main(void) {\n\
             DIR *dir = 0;\n\
             struct dirent entry;\n\
             struct lconv conv;\n\
             pthread_t thread = 0;\n\
             pthread_mutex_t mutex = PTHREAD_MUTEX_INITIALIZER;\n\
             pthread_cond_t cond = PTHREAD_COND_INITIALIZER;\n\
             struct timeval tv;\n\
             wchar_t wc = L'A';\n\
             char *u8 = u8\"ok\";\n\
             wint_t wi = wc;\n\
             mbstate_t state;\n\
             wctype_t class_name = 0;\n\
             wctrans_t trans_name = 0;\n\
             entry.d_name[0] = 'x';\n\
             conv.decimal_point = \".\";\n\
             tv.tv_sec = 1;\n\
             tv.tv_usec = 2;\n\
             state.__opaque = 0;\n\
             int locale_ok = LC_ALL == 0 && LC_TIME > LC_ALL && conv.decimal_point[0] == '.';\n\
             int dir_ok = sizeof(DIR *) == sizeof(void *) && entry.d_name[0] == 'x' && dir == 0;\n\
             int pthread_ok = sizeof(thread) == sizeof(unsigned long) && sizeof(mutex) >= sizeof(unsigned long) &&\n\
                              sizeof(cond) >= sizeof(unsigned long) && PTHREAD_CREATE_DETACHED == 1;\n\
             int time_ok = tv.tv_sec == 1 && tv.tv_usec == 2;\n\
             int wide_ok = sizeof(wchar_t) == sizeof(int) && sizeof(wint_t) == sizeof(unsigned int) &&\n\
                           sizeof(mbstate_t) >= sizeof(int) && wi == L'A' && u8[0] == 'o' && class_name == 0 && trans_name == 0;\n\
             return locale_ok && dir_ok && pthread_ok && time_ok && wide_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_feature_and_cdefs_virtual_headers() {
    let src = temp_file("internal-cpp-feature-cdefs-headers", "c");
    let exe = temp_file("internal-cpp-feature-cdefs-headers", "bin");
    std::fs::write(
        &src,
        "#include <features.h>\n\
         #include <sys/cdefs.h>\n\
         __BEGIN_DECLS\n\
         __attribute_malloc__ __attribute_alloc_size__((1)) void *make_ptr(unsigned long size) __THROW;\n\
         __END_DECLS\n\
         void *make_ptr(unsigned long size) { return (void *)size; }\n\
         int main(void) {\n\
             int joined = __CONCAT(12, 34);\n\
             char *text = __STRING(joined);\n\
             void *p = make_ptr(7);\n\
             int glibc_ok = __GLIBC__ >= 2 && __GLIBC_MINOR__ >= 0 && __GNUC_PREREQ(4, 0);\n\
             int use_ok = __USE_POSIX && __USE_XOPEN2K && __USE_MISC && __GLIBC_USE(ISOC2X) == 0;\n\
             int cdefs_ok = joined == 1234 && text[0] == 'j' && __glibc_likely(p != 0) && !__glibc_unlikely(0);\n\
             return glibc_ok && use_ok && cdefs_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_preprocesses_virtual_headers_as_fixtures() {
    let headers = [
        "assert.h",
        "alloca.h",
        "ctype.h",
        "dirent.h",
        "errno.h",
        "fcntl.h",
        "features.h",
        "float.h",
        "getopt.h",
        "grp.h",
        "inttypes.h",
        "iso646.h",
        "libgen.h",
        "linux/limits.h",
        "limits.h",
        "locale.h",
        "malloc.h",
        "math.h",
        "memory.h",
        "paths.h",
        "netdb.h",
        "poll.h",
        "pthread.h",
        "pwd.h",
        "regex.h",
        "resolv.h",
        "setjmp.h",
        "signal.h",
        "stdalign.h",
        "stdarg.h",
        "stdatomic.h",
        "stdbool.h",
        "stddef.h",
        "stdint.h",
        "stdio.h",
        "stdlib.h",
        "string.h",
        "strings.h",
        "sysexits.h",
        "arpa/inet.h",
        "dlfcn.h",
        "fnmatch.h",
        "glob.h",
        "ifaddrs.h",
        "net/if.h",
        "netinet/in.h",
        "netinet/ip.h",
        "netinet/tcp.h",
        "netinet/udp.h",
        "sys/cdefs.h",
        "sys/file.h",
        "sys/errno.h",
        "sys/poll.h",
        "sys/select.h",
        "sys/socket.h",
        "sys/ioctl.h",
        "sys/mman.h",
        "sys/param.h",
        "sys/resource.h",
        "sys/stat.h",
        "sys/sysmacros.h",
        "sys/time.h",
        "sys/types.h",
        "sys/uio.h",
        "sys/un.h",
        "sys/utsname.h",
        "sys/wait.h",
        "syslog.h",
        "termios.h",
        "time.h",
        "unistd.h",
        "utime.h",
        "wchar.h",
        "wctype.h",
    ];

    let src = temp_file("internal-cpp-all-virtual-headers", "c");
    let mut source = String::new();
    for header in headers {
        source.push_str(&format!("#include <{}>\n#include <{}>\n", header, header));
    }
    source.push_str("int virtual_header_fixture;\n");
    std::fs::write(&src, source).expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int virtual_header_fixture;"), "{stdout}");
    assert!(
        stdout.contains("typedef _Atomic int atomic_int;"),
        "{stdout}"
    );
    assert!(stdout.contains("struct dirent"), "{stdout}");
    assert!(stdout.contains("pthread_create"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_expands_function_macros_across_physical_lines() {
    let src = temp_file("internal-cpp-multiline-function-macro", "c");
    let exe = temp_file("internal-cpp-multiline-function-macro", "bin");
    std::fs::write(
        &src,
        "#include <stdatomic.h>\n\
         #define SUM3(a, b, c) ((a) + (b) + (c))\n\
         int main(void) {\n\
             atomic_int value;\n\
             int expected;\n\
             int first_line = __LINE__;\n\
             int second_line = __LINE__;\n\
             atomic_store(&value, 40);\n\
             expected = 40;\n\
             int changed = atomic_compare_exchange_weak_explicit(&value,\n\
                                                                 &expected,\n\
                                                                 42,\n\
                                                                 memory_order_seq_cst,\n\
                                                                 memory_order_seq_cst);\n\
             int sum = SUM3(10,\n\
                            20,\n\
                            12);\n\
             return changed && value == 42 && sum == 42 && second_line == first_line + 1 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_stdarg_header_exposes_standard_macros() {
    let src = temp_file("internal-cpp-stdarg-macros", "c");
    let exe = temp_file("internal-cpp-stdarg-macros", "bin");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         int probe(int first, ...) {\n\
             va_list ap;\n\
             va_list copy;\n\
             va_start(ap, first);\n\
             va_copy(copy, ap);\n\
             va_end(copy);\n\
             va_end(ap);\n\
             return sizeof(va_arg(ap, int)) == sizeof(int) ? 42 : 1;\n\
         }\n\
         int main(void) { return probe(0, 1); }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_align_and_noreturn_virtual_headers() {
    let src = temp_file("internal-cpp-align-noreturn-headers", "c");
    let exe = temp_file("internal-cpp-align-noreturn-headers", "bin");
    std::fs::write(
        &src,
        "#include <stdalign.h>\n\
         #include <stdnoreturn.h>\n\
         noreturn void fail(void) { __builtin_unreachable(); }\n\
         alignas(16) int value;\n\
         int main(void) {\n\
             return __alignas_is_defined && __alignof_is_defined &&\n\
                    alignof(int) == 4 && ((unsigned long)&value % 16) == 0 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_stdint_virtual_header() {
    let src = temp_file("internal-cpp-stdint-header", "c");
    let exe = temp_file("internal-cpp-stdint-header", "bin");
    std::fs::write(
        &src,
        "#include <stdint.h>\n\
         #ifndef INT_LEAST16_MIN\n\
         #error missing INT_LEAST16_MIN\n\
         #endif\n\
         #ifndef UINT_LEAST32_MAX\n\
         #error missing UINT_LEAST32_MAX\n\
         #endif\n\
         #ifndef INT_FAST8_MIN\n\
         #error missing INT_FAST8_MIN\n\
         #endif\n\
         #ifndef UINT_FAST16_MAX\n\
         #error missing UINT_FAST16_MAX\n\
         #endif\n\
         #ifndef INTMAX_MIN\n\
         #error missing INTMAX_MIN\n\
         #endif\n\
         #ifndef UINTMAX_MAX\n\
         #error missing UINTMAX_MAX\n\
         #endif\n\
         #ifndef PTRDIFF_MAX\n\
         #error missing PTRDIFF_MAX\n\
         #endif\n\
         int main(void) {\n\
             int8_t i8 = -1;\n\
             uint8_t u8 = UINT8_MAX;\n\
             int16_t i16 = INT16_C(-2);\n\
             uint16_t u16 = UINT16_MAX;\n\
             int32_t i32 = INT32_C(3);\n\
             uint32_t u32 = UINT32_C(4);\n\
             int64_t i64 = INT64_C(5);\n\
             uint64_t u64 = UINT64_C(6);\n\
             intmax_t imax = INTMAX_C(7);\n\
             uintmax_t umax = UINTMAX_C(8);\n\
             uintptr_t p = (uintptr_t)&i8;\n\
             int_least16_t least = INT_LEAST16_MIN < 0 ? 1 : 0;\n\
             uint_fast16_t fast = UINT_FAST16_MAX;\n\
             intmax_t imax_min = INTMAX_MIN;\n\
             uintmax_t umax_max = UINTMAX_MAX;\n\
             long ptrdiff_max = PTRDIFF_MAX;\n\
             return sizeof(int8_t) == 1 && sizeof(uint8_t) == 1 &&\n\
                    sizeof(int16_t) == 2 && sizeof(uint16_t) == 2 &&\n\
                    sizeof(int32_t) == 4 && sizeof(uint32_t) == 4 &&\n\
                    sizeof(int64_t) == 8 && sizeof(uint64_t) == 8 &&\n\
                    sizeof(intptr_t) == sizeof(void *) && sizeof(uintptr_t) == sizeof(void *) &&\n\
                    i8 == -1 && u8 == 255 && i16 == -2 && u16 == 65535 &&\n\
                    i32 == 3 && u32 == 4 && i64 == 5 && u64 == 6 &&\n\
                    sizeof(intmax_t) == 8 && sizeof(uintmax_t) == 8 && imax == 7 && umax == 8 &&\n\
                    p != 0 &&\n\
                    INT8_MIN == -128 && INT8_MAX == 127 && INT64_MAX > INT32_MAX &&\n\
                    least == 1 && fast != 0 && imax_min < imax && umax_max > umax &&\n\
                    ptrdiff_max > 0 && SIZE_MAX == UINTPTR_MAX ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_provides_inttypes_virtual_header() {
    let src = temp_file("internal-cpp-inttypes-header", "c");
    std::fs::write(
        &src,
        "#include <inttypes.h>\n\
         const char *a = \"%\" PRId64;\n\
         const char *b = \"%\" PRIuMAX;\n\
         intmax_t c = INTMAX_C(9);\n\
         uintmax_t d = UINTMAX_C(10);\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("typedef long intmax_t;"), "{stdout}");
    assert!(
        stdout.contains("typedef unsigned long uintmax_t;"),
        "{stdout}"
    );
    assert!(stdout.contains("const char *a = \"%\" \"ld\";"), "{stdout}");
    assert!(stdout.contains("const char *b = \"%\" \"lu\";"), "{stdout}");
    assert!(stdout.contains("intmax_t c = 9L;"), "{stdout}");
    assert!(stdout.contains("uintmax_t d = 10UL;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_provides_float_virtual_header() {
    let src = temp_file("internal-cpp-float-header", "c");
    let exe = temp_file("internal-cpp-float-header", "bin");
    std::fs::write(
        &src,
        "#include <float.h>\n\
         #if FLT_RADIX != 2\n\
         #error bad radix\n\
         #endif\n\
         int main(void) {\n\
             return FLT_MANT_DIG == 24 && DBL_MANT_DIG == 53 && LDBL_MANT_DIG == 53 &&\n\
                    FLT_DIG == 6 && DBL_DIG == 15 && DBL_MAX > DBL_MIN &&\n\
                    FLT_MAX > FLT_MIN && DBL_EPSILON > 0.0 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_rejects_malformed_short_circuited_if_expressions() {
    for (name, expr) in [
        ("internal-cpp-short-circuit-and-syntax", "0 && (1 + )"),
        ("internal-cpp-short-circuit-or-syntax", "1 || (1 + )"),
        (
            "internal-cpp-short-circuit-true-ternary-syntax",
            "1 ? 1 : (1 + )",
        ),
        (
            "internal-cpp-short-circuit-false-ternary-syntax",
            "0 ? (1 + ) : 1",
        ),
    ] {
        let src = temp_file(name, "c");
        std::fs::write(&src, format!("#if {expr}\nint skipped = 1;\n#endif\n"))
            .expect("failed to write source");

        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .arg("-E")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");
        let status = output.status;
        let stderr = stderr(output);

        let _ = std::fs::remove_file(src);

        assert!(!status.success(), "{expr}: {stderr}");
        assert!(
            stderr.contains("unsupported #if expression"),
            "{expr}: {stderr}"
        );
        assert!(
            stderr.contains("expected value in #if expression"),
            "{expr}: {stderr}"
        );
    }
}

#[test]
fn internal_cpp_if_arithmetic_does_not_panic_on_overflow() {
    let src = temp_file("internal-cpp-if-overflow", "c");
    std::fs::write(
        &src,
        "#if (9223372036854775807 + 1) || (1 << 100)\n\
         int overflow_value = 9;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int overflow_value = 9;"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_predefined_target_macros_and_if_literals() {
    let src = temp_file("internal-cpp-predefined", "c");
    std::fs::write(
        &src,
        "#if __RNQCC__ && __STDC__ && __STDC_VERSION__ >= 201112L\n\
         #define BASE 0x20UL\n\
         #else\n\
         #define BASE 0\n\
         #endif\n\
         #if __aarch64__ && !defined(__x86_64__) && (__CHAR_BIT__ == 8) && (__SIZEOF_POINTER__ == 8) && (__BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__) && __LITTLE_ENDIAN__ && ('*' == 42) && (010 == 8) && (0b10 ? 1 : 0)\n\
         int main(void) { return BASE + 10; }\n\
         #else\n\
         int main(void) { return 1; }\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-t")
        .arg("aarch64-linux")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 0x20UL + 10; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_elides_apple_availability_macros() {
    let src = temp_file("internal-cpp-apple-availability", "c");
    std::fs::write(
        &src,
        "__OSX_AVAILABLE(14.0) __IOS_AVAILABLE(17.0)\n\
         extern int f(void) API_AVAILABLE(macos(14.0), ios(17.0));\n\
         __API_AVAILABLE(macos(14.0)) extern int g(void);\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-t")
        .arg("aarch64-apple-darwin")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("extern int f(void) ;"), "{stdout}");
    assert!(stdout.contains("extern int g(void);"), "{stdout}");
    assert!(!stdout.contains("AVAILABLE"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn preserves_typedef_pointer_aliases_in_type_names() {
    let src = temp_file("typedef-pointer-alias-type-name", "c");
    let exe = temp_file("typedef-pointer-alias-type-name", "bin");
    std::fs::write(
        &src,
        "typedef struct s *ptr_t;\n\
         typedef ptr_t alias_t;\n\
         struct s { int value; };\n\
         struct s obj;\n\
         static alias_t f(void) { return ((alias_t)&obj); }\n\
         int main(void) {\n\
             return sizeof(alias_t) == sizeof(void *) &&\n\
                    _Alignof(alias_t) == _Alignof(void *) &&\n\
                    f() == &obj\n\
                        ? 42\n\
                        : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_handles_stateful_builtin_macros() {
    let header = temp_file("internal-cpp-stateful", "h");
    let src = temp_file("internal-cpp-stateful", "c");
    std::fs::write(
        &header,
        "int include_level_value = __INCLUDE_LEVEL__;\nchar *base_in_header = __BASE_FILE__;\n",
    )
    .expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#if __COUNTER__ == 0\n\
             #define FIRST_COUNTER_OK 1\n\
             #endif\n\
             #include \"{}\"\n\
             int counter_after_if = __COUNTER__;\n\
             int counter_after_that = __COUNTER__;\n\
             char *date_value = __DATE__;\n\
             char *time_value = __TIME__;\n\
             char *base_value = __BASE_FILE__;\n",
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .env("SOURCE_DATE_EPOCH", "0")
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int include_level_value = 1;"), "{stdout}");
    assert!(stdout.contains("int counter_after_if = 1;"), "{stdout}");
    assert!(stdout.contains("int counter_after_that = 2;"), "{stdout}");
    assert!(
        stdout.contains("char *date_value = \"Jan  1 1970\";"),
        "{stdout}"
    );
    assert!(
        stdout.contains("char *time_value = \"00:00:00\";"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("char *base_value = \"{}\";", src.display())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("char *base_in_header = \"{}\";", src.display())),
        "{stdout}"
    );

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_variadic_macros_comments_and_continuations() {
    let src = temp_file("internal-cpp-variadic", "c");
    let exe = temp_file("internal-cpp-variadic", "bin");
    std::fs::write(
        &src,
        "#define SUM(first, ...) first + __VA_ARGS__\n\
         #define OPTIONAL(first, ...) first, ##__VA_ARGS__\n\
         #define CONTINUED \\\n\
         39\n\
         /* block comments are removed */\n\
         int add(int a, int b) { return a + b; }\n\
         int main(void) { // line comments are removed\n\
             return SUM(CONTINUED, 1 + 2) + add(OPTIONAL(0), 0);\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_handles_trigraph_hash_in_directives() {
    let src = temp_file("internal-cpp-trigraph-directive", "c");
    let exe = temp_file("internal-cpp-trigraph-directive", "bin");
    std::fs::write(
        &src,
        "??=define VALUE 42\n\
         int main(void) { return VALUE; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_splices_trigraph_backslash_newlines() {
    let src = temp_file("internal-cpp-trigraph-splice", "c");
    let exe = temp_file("internal-cpp-trigraph-splice", "bin");
    std::fs::write(
        &src,
        "#define VALUE 4??/\n\
         2\n\
         int main(void) { return VALUE; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_converts_trigraph_punctuators_in_preprocess_output() {
    let src = temp_file("internal-cpp-trigraph-punctuators", "c");
    std::fs::write(
        &src,
        "int main(void) ??< int values??(1??) = ??< 42 ??>; return values??(0??); ??>\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { int values[1] = { 42 }; return values[0]; }"),
        "{stdout}"
    );
    assert!(!stdout.contains("??<"), "{stdout}");
    assert!(!stdout.contains("??("), "{stdout}");
    assert!(!stdout.contains("??>"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_c_digraph_punctuators_after_internal_cpp() {
    let src = temp_file("internal-cpp-digraph-compile", "c");
    let exe = temp_file("internal-cpp-digraph-compile", "bin");
    std::fs::write(
        &src,
        "int main(void) <% int values<:2:> = <% 40, 2 %>; return values<:0:> + values<:1:>; %>\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_handles_va_opt() {
    let src = temp_file("internal-cpp-va-opt", "c");
    std::fs::write(
        &src,
        "#define WRAP(fmt, ...) call(fmt __VA_OPT__(,) __VA_ARGS__)\n\
         int a = WRAP(1);\n\
         int b = WRAP(1, 2, 3);\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int a = call(1  );"), "{stdout}");
    assert!(stdout.contains("int b = call(1 , 2, 3);"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_honors_cli_defines_and_undefines() {
    let src = temp_file("internal-cpp-cli-macros", "c");
    std::fs::write(
        &src,
        "#ifdef ENABLED\n\
         #define A ENABLED\n\
         #else\n\
         #define A 0\n\
         #endif\n\
         #ifdef DISABLED\n\
         #define B 100\n\
         #else\n\
         #define B 2\n\
         #endif\n\
         int main(void) { return A + B; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-DENABLED=40")
        .arg("-DDISABLED")
        .arg("-UDISABLED")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 2; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_dump_macros_reports_cli_and_source_defines_without_source_output() {
    let header = temp_file("internal-cpp-dump-macros", "h");
    let src = temp_file("internal-cpp-dump-macros", "c");
    std::fs::write(
        &header,
        "#define FROM_HEADER 17\n#define HEADER_FN(x) x + FROM_HEADER\n",
    )
    .expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#include \"{}\"\n#define FROM_SOURCE CLI_VALUE\nint value = HEADER_FN(1);\n",
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-dM")
        .arg("-DCLI_VALUE=42")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("#define CLI_VALUE 42"), "{stdout}");
    assert!(stdout.contains("#define FROM_HEADER 17"), "{stdout}");
    assert!(
        stdout.contains("#define HEADER_FN(x) x + FROM_HEADER"),
        "{stdout}"
    );
    assert!(stdout.contains("#define FROM_SOURCE CLI_VALUE"), "{stdout}");
    assert!(!stdout.contains("int value"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_dump_macros_includes_forced_include_defines() {
    let header = temp_file("internal-cpp-dump-macros-forced-include", "h");
    let src = temp_file("internal-cpp-dump-macros-forced-include", "c");
    std::fs::write(
        &header,
        "#define FORCED_DUMP_VALUE 123\n#define FORCED_DUMP_FN(x) x + FORCED_DUMP_VALUE\n",
    )
    .expect("failed to write header");
    std::fs::write(&src, "#define SOURCE_DUMP_VALUE FORCED_DUMP_VALUE\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-dM")
        .arg("-include")
        .arg(&header)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("#define FORCED_DUMP_VALUE 123"), "{stdout}");
    assert!(
        stdout.contains("#define FORCED_DUMP_FN(x) x + FORCED_DUMP_VALUE"),
        "{stdout}"
    );
    assert!(
        stdout.contains("#define SOURCE_DUMP_VALUE FORCED_DUMP_VALUE"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_dump_macros_writes_to_output_file() {
    let src = temp_file("internal-cpp-dump-macros-output", "c");
    let out = temp_file("internal-cpp-dump-macros-output", "i");
    std::fs::write(
        &src,
        "#define OUTPUT_DUMP_VALUE 77\nint value = OUTPUT_DUMP_VALUE;\n",
    )
    .expect("failed to write source");
    let _ = std::fs::remove_file(&out);

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-dM")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let dump = std::fs::read_to_string(&out).expect("failed to read macro dump");
    assert!(dump.contains("#define OUTPUT_DUMP_VALUE 77"), "{dump}");
    assert!(!dump.contains("int value"), "{dump}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn internal_cpp_dump_macros_honors_undef_and_imacros() {
    let header = temp_file("internal-cpp-dump-macros-imacros", "h");
    let src = temp_file("internal-cpp-dump-macros-imacros", "c");
    std::fs::write(&header, "#define FROM_IMACROS 33\nint hidden = 1;\n")
        .expect("failed to write header");
    std::fs::write(
        &src,
        "#define LOCAL_VALUE FROM_IMACROS\nint value = LOCAL_VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-dM")
        .arg("-DREMOVED=1")
        .arg("-UREMOVED")
        .arg("-imacros")
        .arg(&header)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("#define FROM_IMACROS 33"), "{stdout}");
    assert!(
        stdout.contains("#define LOCAL_VALUE FROM_IMACROS"),
        "{stdout}"
    );
    assert!(!stdout.contains("REMOVED"), "{stdout}");
    assert!(!stdout.contains("hidden"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_dump_macro_definitions_keeps_preprocessed_output() {
    let src = temp_file("internal-cpp-dump-macro-definitions", "c");
    std::fs::write(&src, "#define DD_VALUE 40\nint value = DD_VALUE + 2;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-dD")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("#define DD_VALUE 40"), "{stdout}");
    assert!(stdout.contains("int value = 40 + 2;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_dump_macro_definitions_writes_to_output_file() {
    let src = temp_file("internal-cpp-dump-macro-definitions-output", "c");
    let out = temp_file("internal-cpp-dump-macro-definitions-output", "i");
    std::fs::write(
        &src,
        "#define DD_OUTPUT_VALUE 19\nint value = DD_OUTPUT_VALUE;\n",
    )
    .expect("failed to write source");
    let _ = std::fs::remove_file(&out);

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-dD")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let contents = std::fs::read_to_string(&out).expect("failed to read preprocessor output");
    assert!(
        contents.contains("#define DD_OUTPUT_VALUE 19"),
        "{contents}"
    );
    assert!(contents.contains("int value = 19;"), "{contents}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn internal_cpp_dump_macros_formats_variadic_function_macro_signature() {
    let src = temp_file("internal-cpp-dump-variadic-macro", "c");
    std::fs::write(
        &src,
        "#define LOG(fmt, ...) call(fmt, __VA_ARGS__)\nint value;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-dM")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    let log_define = stdout
        .lines()
        .find(|line| line.starts_with("#define LOG"))
        .unwrap_or_else(|| panic!("missing LOG macro dump in:\n{stdout}"));
    assert_eq!(log_define, "#define LOG(fmt, ...) call(fmt, __VA_ARGS__)");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_exposes_darwin_apple_compiler_markers() {
    let src = temp_file("internal-cpp-darwin-apple-markers", "c");
    std::fs::write(
        &src,
        "#if __APPLE__ && __MACH__ && __APPLE_CC__ >= 1 && __APPLE_CPP__ && __arm64__\n\
         int target_ok = 42;\n\
         #else\n\
         int target_ok = 1;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-t")
        .arg("aarch64-macos")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int target_ok = 42;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_accepts_separated_cli_define_and_undefine_operands() {
    let src = temp_file("internal-cpp-separated-cli-macros", "c");
    std::fs::write(
        &src,
        "#ifndef VALUE\n\
         #error VALUE should be defined\n\
         #endif\n\
         #ifdef REMOVED\n\
         #error REMOVED should be undefined\n\
         #endif\n\
         int value = VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-D")
        .arg("VALUE=42")
        .arg("-D")
        .arg("REMOVED=1")
        .arg("-U")
        .arg("REMOVED")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int value = 42;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_accepts_glued_cli_define_and_undefine_operands() {
    let src = temp_file("internal-cpp-glued-cli-macros", "c");
    std::fs::write(
        &src,
        "#ifndef VALUE\n\
         #error VALUE should be defined\n\
         #endif\n\
         #ifdef REMOVED\n\
         #error REMOVED should be undefined\n\
         #endif\n\
         int value = VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-DVALUE=42")
        .arg("-DREMOVED=1")
        .arg("-UREMOVED")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int value = 42;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_integer_bit_fields() {
    let src = temp_file("integer-bit-fields", "c");
    let exe = temp_file("integer-bit-fields", "bin");
    std::fs::write(
        &src,
        "struct flags { unsigned a:3; unsigned b:5; unsigned c:6; };\n\
         int main(void) {\n\
           struct flags f = {0, 0, 0};\n\
           f.a = 13;\n\
           f.b = 17;\n\
           f.c = 63;\n\
           return f.a + f.b + f.c;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(85));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_comma_separated_struct_bit_fields() {
    let src = temp_file("comma-separated-bit-fields", "c");
    let exe = temp_file("comma-separated-bit-fields", "bin");
    std::fs::write(
        &src,
        "struct flags { unsigned a:3, b:5, c:6; int x, y:4; };\n\
         int main(void) {\n\
           struct flags f = {0, 0, 0, 0, 0};\n\
           f.a = 13;\n\
           f.b = 17;\n\
           f.c = 63;\n\
           f.x = 5;\n\
           f.y = -1;\n\
           return f.a + f.b + f.c + f.x + (f.y == -1);\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(91));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn packs_adjacent_bit_fields_with_compatible_storage_units() {
    let src = temp_file("compatible-bit-field-storage", "c");
    let rnqcc_exe = temp_file("compatible-bit-field-storage-rnqcc", "bin");
    let cc_exe = temp_file("compatible-bit-field-storage-cc", "bin");
    std::fs::write(
        &src,
        "typedef unsigned int flag_t;\n\
         typedef int signed_flag_t;\n\
         struct flags { flag_t a:8; signed_flag_t b:8; unsigned int c:8; int d:8; };\n\
         int main(void) { return sizeof(struct flags); }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&rnqcc_exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(output.status.success(), "{}", stderr(output));

    let output = Command::new("cc")
        .arg(&src)
        .arg("-o")
        .arg(&cc_exe)
        .output()
        .expect("failed to run host cc");
    assert!(output.status.success(), "{}", stderr(output));

    let rnqcc_status = Command::new(&rnqcc_exe)
        .status()
        .expect("failed to run rnqcc executable");
    let cc_status = Command::new(&cc_exe)
        .status()
        .expect("failed to run cc executable");
    assert_eq!(rnqcc_status.code(), cc_status.code());

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(rnqcc_exe);
    let _ = std::fs::remove_file(cc_exe);
}

#[test]
fn compiles_signed_integer_bit_fields() {
    let src = temp_file("signed-bit-fields", "c");
    let exe = temp_file("signed-bit-fields", "bin");
    std::fs::write(
        &src,
        "struct flags { int a:3; int b:4; unsigned u:3; };\n\
         int main(void) {\n\
           struct flags f = {0, 0, 0};\n\
           f.a = -1;\n\
           f.b = -8;\n\
           f.u = 9;\n\
           return (f.a == -1) + (f.b == -8) * 2 + (f.u == 1) * 4;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(7));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compound_assign_updates_only_bit_field_bits() {
    let src = temp_file("bit-field-compound-assign", "c");
    let exe = temp_file("bit-field-compound-assign", "bin");
    std::fs::write(
        &src,
        "struct x { unsigned x1:1; unsigned x2:2; unsigned x3:3; };\n\
         int main(void) {\n\
           struct x a = {1, 2, 3};\n\
           struct x b = {1, 2, 3};\n\
           struct x *c = &b;\n\
           c->x3 += (a.x2 - a.x1) * c->x2;\n\
           return a.x1 == 1 && c->x1 == 1 && c->x2 == 2 && c->x3 == 5 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compound_literal_designated_bit_field_initializers_preserve_neighbor_bits() {
    let src = temp_file("compound-literal-bit-field-designators", "c");
    let exe = temp_file("compound-literal-bit-field-designators", "bin");
    std::fs::write(
        &src,
        r#"
struct S {
    int a:3;
    unsigned b:1;
    unsigned c:28;
};

struct S x = {1, 1, 1};

int main(void) {
    x = (struct S) { b:0, a:0, c:({ struct S o = x; o.a == 1 ? 10 : 20; }) };
    return x.a == 0 && x.b == 0 && x.c == 10 ? 42 : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn bit_field_layout_matches_host_for_mixed_storage_units() {
    let src = temp_file("mixed-bit-field-layout", "c");
    let rnqcc_exe = temp_file("mixed-bit-field-layout-rnqcc", "bin");
    let cc_exe = temp_file("mixed-bit-field-layout-cc", "bin");
    std::fs::write(
        &src,
        "struct flags { unsigned char a:3; unsigned char b:4; unsigned :0; unsigned long c:9; char tail; };\n\
         int main(void) { return sizeof(struct flags); }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&rnqcc_exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(output.status.success(), "{}", stderr(output));

    let output = Command::new("cc")
        .arg(&src)
        .arg("-o")
        .arg(&cc_exe)
        .output()
        .expect("failed to run host cc");
    assert!(output.status.success(), "{}", stderr(output));

    let rnqcc_status = Command::new(&rnqcc_exe)
        .status()
        .expect("failed to run rnqcc executable");
    let cc_status = Command::new(&cc_exe)
        .status()
        .expect("failed to run cc executable");
    assert_eq!(rnqcc_status.code(), cc_status.code());

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(rnqcc_exe);
    let _ = std::fs::remove_file(cc_exe);
}

#[test]
fn compiles_bit_field_zero_width_alignment() {
    let src = temp_file("bit-field-zero-width", "c");
    let exe = temp_file("bit-field-zero-width", "bin");
    std::fs::write(
        &src,
        "struct flags { unsigned a:3; unsigned :0; unsigned b:5; };\n\
         int main(void) { return sizeof(struct flags); }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(8));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn honors_gnu_packed_struct_and_union_layout() {
    let src = temp_file("gnu-packed-layout", "c");
    let exe = temp_file("gnu-packed-layout", "bin");
    std::fs::write(
        &src,
        r#"struct __attribute__((packed)) PackedPair { char c; int i; char tail; };
         union __attribute__((packed)) PackedUnion { char c; int i; };
         struct Inner { char c; int i; };
         struct __attribute__((packed)) Outer { char tag; struct Inner inner; char tail; };
         struct PackedSuffix { char c; long l; } __attribute__((packed));
         struct __attribute__((packed)) PackedBits { short x : 9; int : 0; };
         _Static_assert(sizeof(struct PackedPair) == 6, "packed pair size");
         _Static_assert(_Alignof(struct PackedPair) == 1, "packed pair align");
         _Static_assert(__builtin_offsetof(struct PackedPair, i) == 1, "packed pair offset");
         _Static_assert(sizeof(union PackedUnion) == 4, "packed union size");
         _Static_assert(_Alignof(union PackedUnion) == 1, "packed union align");
         _Static_assert(sizeof(struct Inner) == 8, "inner keeps natural layout");
         _Static_assert(_Alignof(struct Inner) == 4, "inner keeps natural align");
         _Static_assert(__builtin_offsetof(struct Outer, inner) == 1, "outer member placement");
         _Static_assert(__builtin_offsetof(struct Outer, tail) == 9, "outer tail offset");
         _Static_assert(sizeof(struct Outer) == 10, "outer size");
         _Static_assert(_Alignof(struct Outer) == 1, "outer align");
         _Static_assert(sizeof(struct PackedSuffix) == 9, "suffix packed size");
         _Static_assert(__builtin_offsetof(struct PackedSuffix, l) == 1, "suffix packed offset");
         _Static_assert(sizeof(struct PackedBits) == 4, "zero width bit-field boundary");
         _Static_assert(_Alignof(struct PackedBits) == 1, "zero width packed align");
         int main(void) { return 42; }
        "#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn skips_common_attribute_syntaxes() {
    let src = temp_file("common-attributes", "c");
    let exe = temp_file("common-attributes", "bin");
    std::fs::write(
        &src,
        "[[maybe_unused]] static int local_value = 40;\n\
         __attribute__((unused, aligned(4))) static int attr_value = 1;\n\
         __declspec(dllexport) int exported_value = 1;\n\
         int main(void) { return local_value + attr_value + exported_value; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn accepts_common_noreturn_attribute_spellings() {
    let src = temp_file("common-noreturn-attributes", "c");
    std::fs::write(
        &src,
        "_Noreturn void fail_keyword(void);
         [[noreturn]] void fail_c23(void);
         __declspec(noreturn) void fail_declspec(void);
         void fail_gnu(void) __attribute__((noreturn));
         int f(int x) { if (x) return 42; fail_keyword(); }
         int g(int x) { if (x) return 42; fail_c23(); }
         int h(int x) { if (x) return 42; fail_declspec(); }
         int i(int x) { if (x) return 42; fail_gnu(); }
         int main(void) { return 42; }
        ",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(!stderr(output).contains("missing return"));
    let _ = std::fs::remove_file(src);
}

#[test]
fn honors_noreturn_when_combined_with_other_attributes() {
    let src = temp_file("combined-noreturn-attributes", "c");
    std::fs::write(
        &src,
        "void fail_combined(void) __attribute__((noreturn, aligned(16)));
         void fail_before(void) __attribute__((noreturn)) __attribute__((aligned(16)));
         void fail_after(void) __attribute__((aligned(16))) __attribute__((noreturn));
         __attribute__((noreturn, aligned(16))) void fail_prefix(void);
         int f(int x) { if (x) return 42; fail_combined(); }
         int g(int x) { if (x) return 42; fail_before(); }
         int h(int x) { if (x) return 42; fail_after(); }
         int i(int x) { if (x) return 42; fail_prefix(); }
         int j(int x) { void fail_block(void) __attribute__((aligned(16))) __attribute__((noreturn)); if (x) return 42; fail_block(); }
         int main(void) { return 42; }
        ",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stderr = stderr(output);
    assert!(!stderr.contains("missing return"), "{stderr}");
    let _ = std::fs::remove_file(src);
}

#[test]
fn accepts_common_gnu_builtin_expression_compatibility() {
    let src = temp_file("gnu-builtin-expression-compat", "c");
    let exe = temp_file("gnu-builtin-expression-compat", "bin");
    std::fs::write(
        &src,
         "struct inner { char c; int value; };\n\
         struct pair { int a; int b; struct inner items[2]; };\n\
         struct other { int value; };\n\
         typedef struct pair pair_alias;\n\
         typedef int int_array[];\n\
         typedef int (*fn_ptr)(int, long);\n\
         typedef struct pair (*pair_factory)(struct other);\n\
         int main(void) {\n\
           int same = __builtin_types_compatible_p(int, int);\n\
           int typedef_same = __builtin_types_compatible_p(pair_alias, struct pair);\n\
           int array_same = __builtin_types_compatible_p(int_array, int[4]);\n\
           int function_same = __builtin_types_compatible_p(fn_ptr, int (*)(int, long));\n\
           int struct_function_same = __builtin_types_compatible_p(pair_factory, struct pair (*)(struct other));\n\
           int struct_function_different = __builtin_types_compatible_p(struct pair (*)(struct other), struct other (*)(struct other));\n\
           int array_different = __builtin_types_compatible_p(int[4], long[4]);\n\
           int chosen = __builtin_choose_expr(1, 40, 0);\n\
           int false_chosen = __builtin_choose_expr(0, 0, 2);\n\
           int hint = 0;\n\
           int expected = __builtin_expect(1, hint++);\n\
           unsigned long off = __builtin_offsetof(struct pair, b);\n\
           unsigned long nested = __builtin_offsetof(struct pair, items[1].value);\n\
           if (0) __builtin_unreachable();\n\
           if (0) __builtin_trap();\n\
           return same + typedef_same + array_same + function_same + struct_function_same +\n\
                  !struct_function_different + !array_different + chosen + false_chosen +\n\
                  expected + (hint == 1) + (off == 4) + (nested == 20);\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(53));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn rejects_non_constant_builtin_choose_expr_condition() {
    let src = temp_file("bad-builtin-choose-expr", "c");
    std::fs::write(
        &src,
        "int main(void) {\n\
           int value = 1;\n\
           return __builtin_choose_expr(value, 2, 3);\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("parse")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("__builtin_choose_expr requires an integer constant condition"),
        "{stderr}"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn compound_assignment_reads_lvalue_after_rhs_side_effects() {
    let src = temp_file("compound-assign-rhs-side-effect-order", "c");
    let exe = temp_file("compound-assign-rhs-side-effect-order", "bin");
    std::fs::write(
        &src,
        r#"
unsigned int x[1] = { 2 };

unsigned int foo(void) {
    x[0] |= 128;
    return 1;
}

int main(void) {
    x[0] |= foo();
    return x[0] == 131 ? 42 : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn emits_target_trap_for_builtin_unreachable() {
    let src = temp_file("builtin-unreachable-trap", "c");
    let x86_out = temp_file("builtin-unreachable-trap-x86", "s");
    let aarch64_out = temp_file("builtin-unreachable-trap-aarch64", "s");
    std::fs::write(
        &src,
        "int main(void) {\n\
           __builtin_unreachable();\n\
         }\n",
    )
    .expect("failed to write source");

    let x86_output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&x86_out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(x86_output.status.success(), "{}", stderr(x86_output));
    let x86_asm = std::fs::read_to_string(&x86_out).expect("failed to read assembly output");
    assert!(x86_asm.contains("ud2"));

    let aarch64_output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&aarch64_out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(
        aarch64_output.status.success(),
        "{}",
        stderr(aarch64_output)
    );
    let aarch64_asm =
        std::fs::read_to_string(&aarch64_out).expect("failed to read assembly output");
    assert!(aarch64_asm.contains("brk #0"));

    std::fs::write(
        &src,
        "int main(void) {\n\
           __builtin_trap();\n\
         }\n",
    )
    .expect("failed to write source");
    let trap_output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&x86_out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(trap_output.status.success(), "{}", stderr(trap_output));
    let trap_asm = std::fs::read_to_string(&x86_out).expect("failed to read assembly output");
    assert!(trap_asm.contains("ud2"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(x86_out);
    let _ = std::fs::remove_file(aarch64_out);
}

#[test]
fn parser_constant_evaluation_uses_struct_layout() {
    let src = temp_file("parser-layout-constant-eval", "c");
    let exe = temp_file("parser-layout-constant-eval", "bin");
    std::fs::write(
        &src,
        "struct payload { char tag; long value; };\n\
         _Static_assert(sizeof(struct payload) == 16, \"size\");\n\
         _Static_assert(_Alignof(struct payload) == 8, \"align\");\n\
         enum { PAYLOAD_SIZE = sizeof(struct payload) };\n\
         int main(void) {\n\
           int values[PAYLOAD_SIZE == 16 ? 1 : -1];\n\
           values[0] = __builtin_offsetof(struct payload, value);\n\
           return values[0] == 8 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn accepts_common_atomic_builtin_single_threaded_compatibility() {
    let src = temp_file("atomic-builtin-compat", "c");
    let exe = temp_file("atomic-builtin-compat", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
           int value = 10;\n\
           __atomic_store_n(&value, 20, 0);\n\
           int loaded = __atomic_load_n(&value, 0);\n\
           int added = __atomic_add_fetch(&value, 22, 0);\n\
           int synced = __sync_sub_and_fetch(&value, 1);\n\
           int old = __atomic_exchange_n(&value, 7, 0);\n\
           int expected = 41;\n\
           int failed = __atomic_compare_exchange_n(&value, &expected, 8, 0, 0, 0);\n\
           int matched = __atomic_compare_exchange_n(&value, &expected, 8, 0, 0, 0);\n\
           int a = 1;\n\
           int b = 2;\n\
           int *p = &a;\n\
           int *oldp = __atomic_exchange_n(&p, &b, 0);\n\
           int *expectedp = &a;\n\
           int ptr_failed = __atomic_compare_exchange_n(&p, &expectedp, &a, 0, 0, 0);\n\
           int ptr_matched = __atomic_compare_exchange_n(&p, &expectedp, &a, 0, 0, 0);\n\
           int sync_failed = __sync_bool_compare_and_swap(&value, 7, 9);\n\
           int sync_matched = __sync_bool_compare_and_swap(&value, 8, 9);\n\
           int sync_old = __sync_val_compare_and_swap(&value, 9, 10);\n\
           int sync_fail_old = __sync_val_compare_and_swap(&value, 9, 11);\n\
           int fetch_old = __atomic_fetch_add(&value, 5, 0);\n\
           int fetch_old_after_add = value;\n\
           int sync_fetch_old = __sync_fetch_and_xor(&value, 3);\n\
           int nand_old = __atomic_fetch_nand(&value, 6, 0);\n\
           int nand_new = __sync_nand_and_fetch(&value, 3);\n\
           int *sync_ptr_old = __sync_val_compare_and_swap(&p, &a, &b);\n\
           int sync_ptr_matched = __sync_bool_compare_and_swap(&p, &b, &a);\n\
           __auto_type inferred_old = __atomic_fetch_add(&value, 1, 0);\n\
           __auto_type inferred_exchange = __atomic_exchange_n(&value, 13, 0);\n\
           __auto_type inferred_match = __sync_bool_compare_and_swap(&value, 13, 14);\n\
           return loaded == 20 && added == 42 && synced == 41 && old == 41 && value == 14 && !failed && expected == 7 && matched && oldp == &a && !ptr_failed && expectedp == &b && ptr_matched && !sync_failed && sync_matched && sync_old == 9 && sync_fail_old == 10 && fetch_old == 10 && fetch_old_after_add == 15 && sync_fetch_old == 15 && nand_old == 12 && nand_new == -4 && sync_ptr_old == &a && sync_ptr_matched && p == &a && inferred_old == -4 && inferred_exchange == -3 && inferred_match ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn accepts_more_common_gnu_builtin_compatibility() {
    let src = temp_file("more-gnu-builtin-compat", "c");
    let exe = temp_file("more-gnu-builtin-compat", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
           int value = 5;\n\
           int *p = __builtin_assume_aligned(&value, 16);\n\
           __builtin_prefetch(p, 0, 3);\n\
           __atomic_thread_fence(0);\n\
           __atomic_signal_fence(0);\n\
           __sync_synchronize();\n\
           int likely = __builtin_expect_with_probability(*p, 0, 0.9);\n\
           unsigned int swapped32 = __builtin_bswap32(0x12345678U);\n\
           unsigned long swapped64 = __builtin_bswap64(0x1122334455667700UL);\n\
           int const_lit = __builtin_constant_p(1 + 2);\n\
           int const_var = __builtin_constant_p(value);\n\
           unsigned long object_size = __builtin_object_size(p, 0);\n\
           unsigned long dynamic_size = __builtin_dynamic_object_size(p, 2);\n\
           char buf[8];\n\
           char dst[8];\n\
           __builtin_memset(buf, 0, sizeof(buf));\n\
           __builtin_memcpy(buf, \"abc\", 4);\n\
           __builtin_memmove(dst, buf, 4);\n\
           int mem_ok = __builtin_memcmp(dst, \"abc\", 4) == 0;\n\
           int str_ok = __builtin_strlen(\"abc\") == 3 && __builtin_strlen(dst) == 3 &&\n\
                        __builtin_strcmp(dst, \"abc\") == 0 && __builtin_strncmp(dst, \"abc\", 3) == 0;\n\
           char haystack[] = \"zzabczz\";\n\
           char repeated[] = \"abca\";\n\
           int search_ok = __builtin_memchr(dst, 'b', 3) == dst + 1 &&\n\
                           __builtin_strchr(dst, 'b') == dst + 1 &&\n\
                           __builtin_strrchr(repeated, 'a') == repeated + 3 &&\n\
                           __builtin_strstr(haystack, \"abc\") == haystack + 2 &&\n\
                           __builtin_strspn(\"aaab\", \"a\") == 3 &&\n\
                           __builtin_strcspn(\"aaab\", \"b\") == 3;\n\
           char fortify[16];\n\
           __builtin___memset_chk(fortify, 0, sizeof(fortify), __builtin_object_size(fortify, 0));\n\
           __builtin___memcpy_chk(fortify, \"ab\", 3, __builtin_object_size(fortify, 0));\n\
           __builtin___strcat_chk(fortify, \"cd\", __builtin_object_size(fortify, 0));\n\
           __builtin___strncat_chk(fortify, \"efg\", 2, __builtin_object_size(fortify, 0));\n\
           __builtin___strncpy_chk(dst, fortify, 6, __builtin_object_size(dst, 0));\n\
           dst[6] = 0;\n\
           int fortify_ok = __builtin_strcmp(dst, \"abcdef\") == 0;\n\
           return likely == 5 && swapped32 == 0x78563412U &&\n\
                  swapped64 == 0x0077665544332211UL && const_lit && !const_var &&\n\
                  object_size == (unsigned long)-1 && dynamic_size == 0 && mem_ok && str_ok && search_ok && fortify_ok ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_hex_and_octal_string_escapes_as_bytes() {
    let src = temp_file("string-byte-escapes", "c");
    let exe = temp_file("string-byte-escapes", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
           unsigned char local[] = \"A\\x00\\377\\101\";\n\
           static unsigned char global[] = \"\\101\\102\\x43\";\n\
           return local[0] == 65 && local[1] == 0 && local[2] == 255 &&\n\
                  local[3] == 65 && global[0] == 65 && global[1] == 66 &&\n\
                  global[2] == 67 && sizeof(local) == 5 && sizeof(global) == 4 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_braced_string_char_array_initializers() {
    let src = temp_file("braced-string-char-array-init", "c");
    let exe = temp_file("braced-string-char-array-init", "bin");
    std::fs::write(
        &src,
        "char global_unsized[] = {\"hi\"};\n\
         char global_sized[5] = {\"ok\"};\n\
         static char static_unsized[] = {\"ab\"};\n\
         int main(void) {\n\
             char local_unsized[] = {\"xy\"};\n\
             char local_sized[4] = {\"z\"};\n\
             return sizeof(global_unsized) == 3 && global_unsized[0] == 'h' && global_unsized[2] == 0 &&\n\
                    sizeof(global_sized) == 5 && global_sized[1] == 'k' && global_sized[2] == 0 && global_sized[4] == 0 &&\n\
                    sizeof(static_unsized) == 3 && static_unsized[1] == 'b' && static_unsized[2] == 0 &&\n\
                    sizeof(local_unsized) == 3 && local_unsized[0] == 'x' && local_unsized[2] == 0 &&\n\
                    sizeof(local_sized) == 4 && local_sized[0] == 'z' && local_sized[1] == 0 ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_long_long_integer_suffixes_as_64_bit_integers() {
    let src = temp_file("long-long-suffixes", "c");
    let exe = temp_file("long-long-suffixes", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
           long a = 9223372036854775807LL;\n\
           unsigned long b = 18446744073709551615ULL;\n\
           unsigned long c = 0xffULL;\n\
           return a > 0 && b == 18446744073709551615UL && c == 255UL ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_alignas_globals_and_struct_members() {
    let src = temp_file("alignas-globals-members", "c");
    let exe = temp_file("alignas-globals-members", "bin");
    std::fs::write(
        &src,
        "_Alignas(32) int global_value = 7;\n\
         struct padded { char c; _Alignas(16) int x; };\n\
         _Static_assert(sizeof(struct padded) == 32, \"size\");\n\
         _Static_assert(__builtin_offsetof(struct padded, x) == 16, \"offset\");\n\
         int main(void) { struct padded p; p.c = 1; p.x = global_value; return p.x == 7 ? 42 : 1; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn emits_alignas_for_static_data() {
    let src = temp_file("alignas-static-data", "c");
    let asm = temp_file("alignas-static-data", "s");
    std::fs::write(&src, "_Alignas(32) int global_value = 1;\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&asm)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let assembly = std::fs::read_to_string(&asm).expect("failed to read assembly");
    assert!(assembly.contains(".balign 32"), "{assembly}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(asm);
}

#[test]
fn skips_simple_inline_asm_statements() {
    let src = temp_file("inline-asm-compat", "c");
    let exe = temp_file("inline-asm-compat", "bin");
    std::fs::write(
        &src,
        "int main(void) { asm volatile (\"\" ::: \"memory\"); __asm__(\"\"); return 42; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_empty_inline_asm_tied_zero_output_compatibility() {
    let src = temp_file("inline-asm-tied-zero-output", "c");
    let exe = temp_file("inline-asm-tied-zero-output", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int a;
    asm ("" : "=r" (a) : "0" (0));
    return a == 0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn accepts_hosted_header_compatibility_declarations() {
    let src = temp_file("hosted-compat-decls", "c");
    let exe = temp_file("hosted-compat-decls", "bin");
    std::fs::write(
        &src,
        "_Static_assert(sizeof(int) == 4, \"int width\");\n\
         static_assert(1, \"alias\");\n\
         _Thread_local int tls_value;\n\
         __thread int gnu_tls_value;\n\
         _Atomic(int) atomic_value;\n\
         int main(void) {\n\
           volatile _Atomic int * restrict p = &atomic_value;\n\
           *p = 41;\n\
           tls_value = 1;\n\
           gnu_tls_value = 0;\n\
           return atomic_value + tls_value + gnu_tls_value;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_accepts_cli_function_like_define() {
    let src = temp_file("internal-cpp-cli-function-macro", "c");
    let exe = temp_file("internal-cpp-cli-function-macro", "bin");
    std::fs::write(&src, "int main(void) { return ADD(20, 22); }\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-DADD(x,y)=((x)+(y))")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_converts_trigraphs_in_cli_object_define_body() {
    let src = temp_file("internal-cpp-cli-object-trigraph", "c");
    std::fs::write(
        &src,
        "int main(void) { int values[1] = INIT; return values[0]; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-DINIT=??<42??>")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { int values[1] = {42}; return values[0]; }"),
        "{stdout}"
    );
    assert!(!stdout.contains("??<"), "{stdout}");
    assert!(!stdout.contains("??>"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_converts_trigraphs_in_cli_function_define_body() {
    let src = temp_file("internal-cpp-cli-function-trigraph", "c");
    let exe = temp_file("internal-cpp-cli-function-trigraph", "bin");
    std::fs::write(
        &src,
        "int main(void) { int values[1] = WRAP(42); return PICK(values, 0); }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-DWRAP(x)=??<x??>")
        .arg("-DPICK(a,i)=a??(i??)")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_rejects_malformed_cli_function_like_define_without_panic() {
    let src = temp_file("internal-cpp-bad-cli-function-macro", "c");
    std::fs::write(&src, "int value = BAD(1);\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-DBAD(x")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(
        stderr.contains("malformed macro definition: -DBAD(x"),
        "{stderr}"
    );
    assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");
}

#[test]
fn internal_cpp_processes_forced_include_before_source() {
    let header = temp_file("internal-cpp-forced-include", "h");
    let src = temp_file("internal-cpp-forced-include", "c");
    let exe = temp_file("internal-cpp-forced-include", "bin");
    std::fs::write(&header, "#define FORCED_VALUE 40\n").expect("failed to write header");
    std::fs::write(
        &src,
        "#ifndef FORCED_VALUE\n\
         #error forced include was not processed first\n\
         #endif\n\
         int main(void) { return FORCED_VALUE + 2; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-include")
        .arg(&header)
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_processes_imacros_before_source_without_emitting_output() {
    let header = temp_file("internal-cpp-imacros", "h");
    let src = temp_file("internal-cpp-imacros", "c");
    std::fs::write(&header, "#define IMACROS_VALUE 40\nint hidden = 1;\n")
        .expect("failed to write header");
    std::fs::write(&src, "int main(void) { return IMACROS_VALUE + 2; }\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-imacros")
        .arg(&header)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 2; }"),
        "{stdout}"
    );
    assert!(!stdout.contains("hidden"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_accepts_glued_imacros_before_source_without_emitting_output() {
    let header = temp_file("internal-cpp-glued-imacros", "h");
    let src = temp_file("internal-cpp-glued-imacros", "c");
    std::fs::write(
        &header,
        "#define GLUED_IMACROS_VALUE 40\nint hidden_glued = 1;\n",
    )
    .expect("failed to write header");
    std::fs::write(&src, "int main(void) { return GLUED_IMACROS_VALUE + 2; }\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg(format!("-imacros{}", header.display()))
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 2; }"),
        "{stdout}"
    );
    assert!(!stdout.contains("hidden_glued"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_processes_imacros_before_forced_include() {
    let macro_header = temp_file("internal-cpp-imacros-order", "h");
    let forced_header = temp_file("internal-cpp-imacros-order", "h");
    let src = temp_file("internal-cpp-imacros-order", "c");
    let exe = temp_file("internal-cpp-imacros-order", "bin");
    std::fs::write(&macro_header, "#define BASE_VALUE 40\n").expect("failed to write header");
    std::fs::write(&forced_header, "#define FORCED_VALUE BASE_VALUE\n")
        .expect("failed to write forced header");
    std::fs::write(&src, "int main(void) { return FORCED_VALUE + 2; }\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-imacros")
        .arg(&macro_header)
        .arg("-include")
        .arg(&forced_header)
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(macro_header);
    let _ = std::fs::remove_file(forced_header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_processes_glued_imacros_before_earlier_forced_include() {
    let macro_header = temp_file("internal-cpp-glued-imacros-order", "h");
    let forced_header = temp_file("internal-cpp-glued-imacros-order", "h");
    let src = temp_file("internal-cpp-glued-imacros-order", "c");
    std::fs::write(
        &macro_header,
        "#define BASE_VALUE 40\nint hidden_order = 1;\n",
    )
    .expect("failed to write macro header");
    std::fs::write(&forced_header, "int forced_value = BASE_VALUE;\n")
        .expect("failed to write forced header");
    std::fs::write(&src, "int main(void) { return forced_value + 2; }\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-include")
        .arg(&forced_header)
        .arg(format!("-imacros{}", macro_header.display()))
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert_contains_in_order(
        &stdout,
        &[
            "int forced_value = 40;",
            "int main(void) { return forced_value + 2; }",
        ],
    )
    .unwrap_or_else(|err| panic!("{err}\n{stdout}"));
    assert!(!stdout.contains("hidden_order"), "{stdout}");

    let _ = std::fs::remove_file(macro_header);
    let _ = std::fs::remove_file(forced_header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_handles_stringification_token_pasting_and_source_macros() {
    let src = temp_file("internal-cpp-string-paste", "c");
    std::fs::write(
        &src,
        "#define STR(x) #x\n\
         #define CAT(a, b) a ## b\n\
         int CAT(ma, in)(void) { return __LINE__; }\n\
         char *s = STR(hello   world);\n\
         char *f = __FILE__;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int main(void) { return 3; }"), "{stdout}");
    assert!(stdout.contains("char *s = \"hello world\";"), "{stdout}");
    assert!(
        stdout.contains(&format!("char *f = \"{}\";", src.display())),
        "{stdout}"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_rejects_macro_expansion_errors() {
    for (name, source, expected) in [
        (
            "internal-cpp-invalid-token-paste",
            "#define BAD_PASTE(x) x ## +\nint value = BAD_PASTE(1);\n",
            "invalid token paste: 1+",
        ),
        (
            "internal-cpp-missing-macro-paren",
            "#define MISSING_PAREN(x\nint value = MISSING_PAREN(1);\n",
            "missing ')' in function-like macro MISSING_PAREN",
        ),
        (
            "internal-cpp-missing-invocation-paren",
            "#define ADD(x, y) x + y\nint value = ADD(1, 2;\n",
            "missing ')' in function-like macro invocation",
        ),
    ] {
        let src = temp_file(name, "c");
        std::fs::write(&src, source).expect("failed to write source");

        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .arg("-E")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");
        let status = output.status;
        let stderr = stderr(output);

        let _ = std::fs::remove_file(src);

        assert!(!status.success(), "{name}: {stderr}");
        assert!(stderr.contains(expected), "{name}: {stderr}");
    }
}

#[test]
fn internal_cpp_handles_line_directives_and_macro_expanded_includes() {
    let header = temp_file("internal-cpp-macro-include", "h");
    let src = temp_file("internal-cpp-line-include", "c");
    std::fs::write(&header, "#define FROM_MACRO_INCLUDE 42\n").expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#define HEADER \"{}\"\n\
             #define LINE_NO 200\n\
             #define VIRTUAL_FILE \"virtual.c\"\n\
             #include HEADER\n\
             #line LINE_NO VIRTUAL_FILE\n\
             int first(void) {{ return __LINE__; }}\n\
             char *file_value = __FILE__;\n\
             #line 9 \"quote\\\"file.c\"\n\
             char *escaped_file_value = __FILE__;\n\
             # 7 \"marker.c\"\n\
             int main(void) {{ return FROM_MACRO_INCLUDE + __LINE__; }}\n",
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int first(void) { return 200; }"),
        "{stdout}"
    );
    assert!(
        stdout.contains("char *file_value = \"virtual.c\";"),
        "{stdout}"
    );
    assert!(
        stdout.contains("char *escaped_file_value = \"quote\\\"file.c\";"),
        "{stdout}"
    );
    assert!(
        stdout.contains("int main(void) { return 42 + 7; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_expands_tokenized_include_names_from_fixtures() {
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("tests/fixtures/preprocess/token_include.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int from_macro_include = 42;"), "{stdout}");
    assert!(
        stdout.contains("int from_macro_punct_include = 13;"),
        "{stdout}"
    );
    assert!(stdout.contains("int from_spaced_include = 17;"), "{stdout}");
}

#[test]
fn internal_cpp_rejects_extra_tokens_after_include() {
    let header = temp_file("internal-cpp-include-trailing", "h");
    let src = temp_file("internal-cpp-include-trailing", "c");
    std::fs::write(&header, "#define TRAILING_INCLUDE_VALUE 19\n").expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#include \"{}\" extra_tokens\nint value = TRAILING_INCLUDE_VALUE;\n",
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_empty_angle_include_operand() {
    let src = temp_file("internal-cpp-empty-angle-include", "c");
    std::fs::write(&src, "#include <>\nint value = 1;\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_empty_quoted_include_operand() {
    let src = temp_file("internal-cpp-empty-quoted-include", "c");
    std::fs::write(&src, "#include \"\"\nint value = 1;\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_extra_tokens_in_has_include_operand() {
    let src = temp_file("internal-cpp-has-include-trailing", "c");
    std::fs::write(
        &src,
        "#if __has_include(\"token_include_header.h\" extra_tokens)\n\
         int trailing_has_include_value = 1;\n\
         #endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("-I")
        .arg("tests/fixtures/preprocess")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_empty_quoted_has_include_operand() {
    let src = temp_file("internal-cpp-empty-quoted-has-include", "c");
    std::fs::write(&src, "#if __has_include(\"\")\nint value = 1;\n#endif\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_empty_angle_has_include_operand() {
    let src = temp_file("internal-cpp-empty-angle-has-include", "c");
    std::fs::write(&src, "#if __has_include(<>)\nint value = 1;\n#endif\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_empty_quoted_has_include_next_operand() {
    let src = temp_file("internal-cpp-empty-quoted-has-include-next", "c");
    std::fs::write(
        &src,
        "#if __has_include_next(\"\")\nint value = 1;\n#endif\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_empty_angle_has_include_next_operand() {
    let src = temp_file("internal-cpp-empty-angle-has-include-next", "c");
    std::fs::write(&src, "#if __has_include_next(<>)\nint value = 1;\n#endif\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    let status = output.status;
    let stderr = stderr(output);

    let _ = std::fs::remove_file(src);

    assert!(!status.success(), "{stderr}");
    assert!(stderr.contains("malformed include operand"), "{stderr}");
}

#[test]
fn internal_cpp_rejects_invalid_line_directive_operands() {
    for (name, source, expected) in [
        (
            "internal-cpp-line-missing-number",
            "#line \"not-a-number.c\"\nint value = __LINE__;\n",
            "malformed #line directive",
        ),
        (
            "internal-cpp-line-unterminated-file",
            "#line 12 \"unterminated.c\nint value = __LINE__;\n",
            "unterminated literal in preprocessor input",
        ),
        (
            "internal-cpp-line-macro-missing-number",
            "#define BAD_LINE \"macro-file.c\"\n#line BAD_LINE\nint value = __LINE__;\n",
            "malformed #line directive",
        ),
        (
            "internal-cpp-line-marker-bad-flag",
            "# 12 \"file.c\" badflag\nint value = __LINE__;\n",
            "malformed line marker: badflag",
        ),
    ] {
        let src = temp_file(name, "c");
        std::fs::write(&src, source).expect("failed to write source");

        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .arg("-E")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");
        let status = output.status;
        let stderr = stderr(output);

        let _ = std::fs::remove_file(src);

        assert!(!status.success(), "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
    }
}

#[test]
fn internal_cpp_function_macro_params_do_not_replace_substrings() {
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("tests/fixtures/preprocess/token_substrings.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int xx = 40; char *literal = \"x\"; int x_value = 40; int suffixx = 40;"),
        "{stdout}"
    );
}

#[test]
fn internal_cpp_handles_token_stringify_paste_and_variadic_fixtures() {
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("tests/fixtures/preprocess/token_ops.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int add_two(int a, int b) { return a + b; }"),
        "{stdout}"
    );
    assert!(
        stdout.contains("char *text = \"alpha + beta\";"),
        "{stdout}"
    );
    assert!(stdout.contains("int total = 39 + 1 + 2;"), "{stdout}");
    assert!(stdout.contains("int only = call(1);"), "{stdout}");
    let compact_stdout = stdout.replace(' ', "");
    assert!(compact_stdout.contains("intmany=call(1,2,3);"), "{stdout}");
    assert!(stdout.contains("int opt_empty = call(7  );"), "{stdout}");
    assert!(
        stdout.contains("int opt_many = call(7 , 8, 9);"),
        "{stdout}"
    );
}

#[test]
fn internal_cpp_handles_token_directive_edge_fixtures() {
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("tests/fixtures/preprocess/token_directives.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int digraph_define_value = 11;"),
        "{stdout}"
    );
    assert!(stdout.contains("int line_from_macro = 123;"), "{stdout}");
    assert!(
        stdout.contains("char *file_from_macro = \"macro_line.c\";"),
        "{stdout}"
    );
    assert!(
        stdout.contains("int line_from_marker_flags = 777;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("char *file_from_marker_flags = \"marker_flags.c\";"),
        "{stdout}"
    );
    assert!(
        stdout.contains("int include_level_from_marker_flags = 0;"),
        "{stdout}"
    );
    assert!(stdout.contains("int line_after_marker = 50;"), "{stdout}");
    assert!(
        stdout.contains("char *file_after_marker = \"builtin_after_marker.c\";"),
        "{stdout}"
    );
    assert!(stdout.contains("int conditional_value = 42;"), "{stdout}");
    let compact_stdout = stdout.replace([' ', '\n', '\t'], "");
    assert!(
        compact_stdout.contains("intadjacent_values[]={31,31+1,(31),31+11};"),
        "{stdout}"
    );
}

#[test]
fn internal_cpp_handles_token_predicate_edge_fixtures() {
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("tests/fixtures/preprocess/token_predicates.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int defined_spacing = 1;"), "{stdout}");
    assert!(
        stdout.contains("int has_include_object_macro = 1;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("int has_include_function_macro = 1;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("int has_include_nested_macro_arg = 1;"),
        "{stdout}"
    );
    assert!(
        stdout.contains("int inactive_parent_skipped_predicates = 1;"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("inactive_has_include_evaluated"),
        "{stdout}"
    );
    assert!(!stdout.contains("inactive_defined_evaluated"), "{stdout}");
}

#[test]
fn internal_cpp_handles_token_rescan_edge_fixtures() {
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("tests/fixtures/preprocess/token_rescan_edges.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    let ordered = assert_contains_in_order(
        &stdout,
        &[
            "int object_counter_first = 0;",
            "int object_counter_second = 1;",
            "int function_counter_literal = 40 + 2;",
            "int function_counter_arg = 3 + 4;",
        ],
    );
    assert!(ordered.is_ok(), "{}\n{stdout}", ordered.unwrap_err());
    assert!(
        stdout.contains("char *literal_arg = \"x /* not a comment */ y\";"),
        "{stdout}"
    );
    assert!(stdout.contains("int comment_arg = 20 + 22;"), "{stdout}");

    let compact_stdout = stdout.replace([' ', '\n', '\t'], "");
    assert!(compact_stdout.contains("intkeyword_value=7;"), "{stdout}");
    assert!(
        compact_stdout.contains("intpasted_identifier_adjacent=alpha42+beta_value;"),
        "{stdout}"
    );
    assert!(
        compact_stdout.contains("intmember_paste=object.field_name;"),
        "{stdout}"
    );
    assert!(compact_stdout.contains("intactive_counter=5;"), "{stdout}");
    assert!(!stdout.contains("inactive_value"), "{stdout}");
    assert!(!stdout.contains("inactive_define_leaked"), "{stdout}");
}

#[test]
fn internal_cpp_honors_pragma_once() {
    let header = temp_file("internal-cpp-pragma-once", "h");
    let src = temp_file("internal-cpp-pragma-once", "c");
    std::fs::write(&header, "#pragma once\nint only_once = 42;\n").expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#include \"{}\"\n#include \"{}\"\nint main(void) {{ return only_once; }}\n",
            header.display(),
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert_eq!(stdout.matches("int only_once = 42;").count(), 1, "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_expands_pragma_operands_before_recognizing_once() {
    let header = temp_file("internal-cpp-pragma-macro-once", "h");
    let src = temp_file("internal-cpp-pragma-macro-once", "c");
    std::fs::write(
        &header,
        "#define ONCE once\n#pragma ONCE\nint only_once = 42;\n",
    )
    .expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#include \"{}\"\n#include \"{}\"\nint main(void) {{ return only_once; }}\n",
            header.display(),
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert_eq!(stdout.matches("int only_once = 42;").count(), 1, "{stdout}");
    assert!(!stdout.contains("#pragma"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_ignores_unknown_pragmas_without_emitting_them() {
    let src = temp_file("internal-cpp-unknown-pragma", "c");
    std::fs::write(
        &src,
        "#pragma rnqcc_unknown payload\n#pragma not_once\nint main(void) { return 42; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int main(void)"), "{stdout}");
    assert!(!stdout.contains("#pragma"), "{stdout}");
    assert!(!stdout.contains("rnqcc_unknown"), "{stdout}");
    assert!(!stdout.contains("not_once"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_honors_pragma_pack_push_one_and_pop() {
    let src = temp_file("internal-cpp-pragma-pack", "c");
    let exe = temp_file("internal-cpp-pragma-pack", "bin");
    std::fs::write(
        &src,
        "#pragma pack(push, 1)\n\
         struct Packed { char c; int i; };\n\
         #pragma pack(pop)\n\
         struct Natural { char c; int i; };\n\
         int main(void) {\n\
             return sizeof(struct Packed) == 5 &&\n\
                    __builtin_offsetof(struct Packed, i) == 1 &&\n\
                    sizeof(struct Natural) == 8 &&\n\
                    __builtin_offsetof(struct Natural, i) == 4\n\
                        ? 42\n\
                        : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_honors_pragma_pack_push_four_and_pop() {
    let src = temp_file("internal-cpp-pragma-pack-four", "c");
    let exe = temp_file("internal-cpp-pragma-pack-four", "bin");
    std::fs::write(
        &src,
        "#pragma pack(push, 4)\n\
         struct PackedFour { int i; unsigned long p; };\n\
         #pragma pack(pop)\n\
         struct NaturalEight { int i; unsigned long p; };\n\
         int main(void) {\n\
             return sizeof(struct PackedFour) == 12 &&\n\
                    _Alignof(struct PackedFour) == 4 &&\n\
                    __builtin_offsetof(struct PackedFour, p) == 4 &&\n\
                    sizeof(struct NaturalEight) == 16 &&\n\
                    _Alignof(struct NaturalEight) == 8 &&\n\
                    __builtin_offsetof(struct NaturalEight, p) == 8\n\
                        ? 42\n\
                        : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn internal_cpp_honors_pragma_push_macro_and_pop_macro() {
    let src = temp_file("internal-cpp-pragma-push-pop-macro", "c");
    std::fs::write(
        &src,
        "#define VALUE 1\n\
         #pragma push_macro(\"VALUE\")\n\
         #undef VALUE\n\
         #define VALUE 2\n\
         #pragma pop_macro(\"VALUE\")\n\
         int result = VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int result = 1;"), "{stdout}");
    assert!(!stdout.contains("int result = 2;"), "{stdout}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_honors_standalone_pragma_operator_once() {
    let header = temp_file("internal-cpp-pragma-operator-once", "h");
    let src = temp_file("internal-cpp-pragma-operator-once", "c");
    std::fs::write(
        &header,
        "#define ONCE _Pragma(\"once\")\nONCE\nint only_once = 42;\n",
    )
    .expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#include \"{}\"\n#include \"{}\"\nint main(void) {{ return only_once; }}\n",
            header.display(),
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert_eq!(stdout.matches("int only_once = 42;").count(), 1, "{stdout}");
    assert!(!stdout.contains("_Pragma"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_honors_macro_expanded_pragma_operator_once_with_surrounding_tokens() {
    let header = temp_file("internal-cpp-pragma-operator-once-tokens", "h");
    let src = temp_file("internal-cpp-pragma-operator-once-tokens", "c");
    std::fs::write(
        &header,
        "#define ONCE _Pragma(\"once\")\nint before_once = 7; ONCE int after_once = 42;\n",
    )
    .expect("failed to write header");
    std::fs::write(
        &src,
        format!(
            "#include \"{}\"\n#include \"{}\"\nint main(void) {{ return before_once + after_once; }}\n",
            header.display(),
            header.display()
        ),
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert_eq!(
        stdout.matches("int before_once = 7;").count(),
        1,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("int after_once = 42;").count(),
        1,
        "{stdout}"
    );
    assert!(!stdout.contains("_Pragma"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_accepts_identical_macro_redefinition_and_rejects_incompatible() {
    let identical = temp_file("internal-cpp-identical-redefine", "c");
    std::fs::write(
        &identical,
        "#define SAME 40 + 2\n#define SAME 40 + 2\nint value = SAME;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&identical)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let identical_stdout = stdout(output);
    let identical_stdout_without_whitespace: String = identical_stdout.split_whitespace().collect();
    assert!(
        identical_stdout_without_whitespace.contains("intvalue=40+2;"),
        "{identical_stdout}"
    );

    let object_whitespace = temp_file("internal-cpp-object-whitespace-redefine", "c");
    std::fs::write(
        &object_whitespace,
        "#define OBJECT_EQ 40+2\n#define OBJECT_EQ 40 + 2\nint value = OBJECT_EQ;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&object_whitespace)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let object_stdout = stdout(output);
    let object_stdout_without_whitespace: String = object_stdout.split_whitespace().collect();
    assert!(
        object_stdout_without_whitespace.contains("intvalue=40+2;"),
        "{object_stdout}"
    );

    let function_whitespace = temp_file("internal-cpp-function-whitespace-redefine", "c");
    std::fs::write(
        &function_whitespace,
        "#define FUNCTION_EQ(a,b) ((a)+(b))\n\
         #define FUNCTION_EQ(a, b) ( ( a ) + ( b ) )\n\
         int value = FUNCTION_EQ(40, 2);\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&function_whitespace)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let function_stdout = stdout(output);
    let function_stdout_without_whitespace: String = function_stdout.split_whitespace().collect();
    assert!(
        function_stdout_without_whitespace.contains("intvalue=((40)+(2));"),
        "{function_stdout}"
    );

    let incompatible = temp_file("internal-cpp-incompatible-redefine", "c");
    std::fs::write(
        &incompatible,
        "#define CONFLICT 1\n#define CONFLICT 2\nint value = CONFLICT;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&incompatible)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let incompatible_stderr = stderr(output);
    assert!(
        incompatible_stderr.contains("CONFLICT"),
        "{incompatible_stderr}"
    );
    assert!(
        incompatible_stderr.contains("redefinition") || incompatible_stderr.contains("redefined"),
        "{incompatible_stderr}"
    );

    let function_incompatible = temp_file("internal-cpp-function-incompatible-redefine", "c");
    std::fs::write(
        &function_incompatible,
        "#define FUNCTION_CONFLICT(x) ((x) + 1)\n\
         #define FUNCTION_CONFLICT(x) ((x) + 2)\n\
         int value = FUNCTION_CONFLICT(40);\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&function_incompatible)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let function_incompatible_stderr = stderr(output);
    assert!(
        function_incompatible_stderr.contains("FUNCTION_CONFLICT"),
        "{function_incompatible_stderr}"
    );
    assert!(
        function_incompatible_stderr.contains("redefinition")
            || function_incompatible_stderr.contains("redefined"),
        "{function_incompatible_stderr}"
    );

    let _ = std::fs::remove_file(identical);
    let _ = std::fs::remove_file(object_whitespace);
    let _ = std::fs::remove_file(function_whitespace);
    let _ = std::fs::remove_file(incompatible);
    let _ = std::fs::remove_file(function_incompatible);
}

#[test]
fn internal_cpp_searches_include_paths_for_angle_includes() {
    let include_dir = temp_file("internal-cpp-include-dir", "d");
    let src = temp_file("internal-cpp-angle-include", "c");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("rnqcc_angle.h");
    std::fs::write(&header, "#define VALUE_FROM_ANGLE 42\n").expect("failed to write header");
    std::fs::write(
        &src,
        "#include <rnqcc_angle.h>\nint main(void) { return VALUE_FROM_ANGLE; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-I")
        .arg(&include_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int main(void) { return 42; }"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(include_dir);
}

#[test]
fn internal_cpp_expands_macro_generated_angle_includes() {
    let include_dir = temp_file("internal-cpp-macro-angle-include-dir", "d");
    let src = temp_file("internal-cpp-macro-angle-include", "c");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("rnqcc_macro_angle.h");
    std::fs::write(&header, "#define MACRO_ANGLE_VALUE 42\n").expect("failed to write header");
    std::fs::write(
        &src,
        "#define ANGLE_HEADER <rnqcc_macro_angle.h>\n#include ANGLE_HEADER\nint main(void) { return MACRO_ANGLE_VALUE; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-I")
        .arg(&include_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int main(void) { return 42; }"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(include_dir);
}

#[test]
fn internal_cpp_accepts_separated_and_glued_i_include_paths() {
    for glued in [false, true] {
        let name = if glued {
            "internal-cpp-glued-i"
        } else {
            "internal-cpp-separated-i"
        };
        let include_dir = temp_file(name, "d");
        let src = temp_file(name, "c");
        std::fs::create_dir(&include_dir).expect("failed to create include dir");
        let header = include_dir.join("rnqcc_i_form.h");
        std::fs::write(&header, "#define VALUE_FROM_I_FORM 42\n").expect("failed to write header");
        std::fs::write(
            &src,
            "#include <rnqcc_i_form.h>\nint main(void) { return VALUE_FROM_I_FORM; }\n",
        )
        .expect("failed to write source");

        let mut command = Command::new(rnqcc());
        command.arg("--internal-cpp");
        if glued {
            command.arg(format!("-I{}", include_dir.display()));
        } else {
            command.arg("-I").arg(&include_dir);
        }
        let output = command
            .arg("-E")
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "glued={glued}: {}", stderr(output));
        let stdout = stdout(output);
        assert!(
            stdout.contains("int main(void) { return 42; }"),
            "glued={glued}: {stdout}"
        );

        let _ = std::fs::remove_file(header);
        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_dir(include_dir);
    }
}

#[test]
fn internal_cpp_honors_include_path_categories() {
    let quote_dir = temp_file("internal-cpp-iquote", "d");
    let user_dir = temp_file("internal-cpp-i", "d");
    let system_dir = temp_file("internal-cpp-isystem", "d");
    let after_dir = temp_file("internal-cpp-idirafter", "d");
    let src = temp_file("internal-cpp-include-categories", "c");
    std::fs::create_dir(&quote_dir).expect("failed to create quote dir");
    std::fs::create_dir(&user_dir).expect("failed to create user dir");
    std::fs::create_dir(&system_dir).expect("failed to create system dir");
    std::fs::create_dir(&after_dir).expect("failed to create after dir");
    std::fs::write(quote_dir.join("quote_only.h"), "#define QUOTE_ONLY 40\n")
        .expect("failed to write quote header");
    std::fs::write(user_dir.join("shared.h"), "#define SHARED_VALUE 1\n")
        .expect("failed to write user header");
    std::fs::write(system_dir.join("shared.h"), "#define SHARED_VALUE 100\n")
        .expect("failed to write system header");
    std::fs::write(after_dir.join("after_only.h"), "#define AFTER_ONLY 1\n")
        .expect("failed to write after header");
    std::fs::write(
        &src,
        "#include \"quote_only.h\"\n\
         #include <shared.h>\n\
         #include <after_only.h>\n\
         int main(void) { return QUOTE_ONLY + SHARED_VALUE + AFTER_ONLY; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--iquote")
        .arg(&quote_dir)
        .arg("-I")
        .arg(&user_dir)
        .arg("--isystem")
        .arg(&system_dir)
        .arg("--idirafter")
        .arg(&after_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 1 + 1; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(quote_dir.join("quote_only.h"));
    let _ = std::fs::remove_file(user_dir.join("shared.h"));
    let _ = std::fs::remove_file(system_dir.join("shared.h"));
    let _ = std::fs::remove_file(after_dir.join("after_only.h"));
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(quote_dir);
    let _ = std::fs::remove_dir(user_dir);
    let _ = std::fs::remove_dir(system_dir);
    let _ = std::fs::remove_dir(after_dir);
}

#[test]
fn internal_cpp_accepts_single_dash_include_path_categories() {
    let quote_dir = temp_file("internal-cpp-single-dash-iquote", "d");
    let user_dir = temp_file("internal-cpp-single-dash-i", "d");
    let system_dir = temp_file("internal-cpp-single-dash-isystem", "d");
    let after_dir = temp_file("internal-cpp-single-dash-idirafter", "d");
    let src = temp_file("internal-cpp-single-dash-include-categories", "c");
    std::fs::create_dir(&quote_dir).expect("failed to create quote dir");
    std::fs::create_dir(&user_dir).expect("failed to create user dir");
    std::fs::create_dir(&system_dir).expect("failed to create system dir");
    std::fs::create_dir(&after_dir).expect("failed to create after dir");
    std::fs::write(quote_dir.join("quote_only.h"), "#define QUOTE_ONLY 40\n")
        .expect("failed to write quote header");
    std::fs::write(user_dir.join("shared.h"), "#define SHARED_VALUE 1\n")
        .expect("failed to write user header");
    std::fs::write(system_dir.join("shared.h"), "#define SHARED_VALUE 100\n")
        .expect("failed to write system header");
    std::fs::write(after_dir.join("after_only.h"), "#define AFTER_ONLY 1\n")
        .expect("failed to write after header");
    std::fs::write(
        &src,
        "#include \"quote_only.h\"\n\
         #include <shared.h>\n\
         #include <after_only.h>\n\
         int main(void) { return QUOTE_ONLY + SHARED_VALUE + AFTER_ONLY; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-iquote")
        .arg(&quote_dir)
        .arg("-I")
        .arg(&user_dir)
        .arg("-isystem")
        .arg(&system_dir)
        .arg("-idirafter")
        .arg(&after_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 1 + 1; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(quote_dir.join("quote_only.h"));
    let _ = std::fs::remove_file(user_dir.join("shared.h"));
    let _ = std::fs::remove_file(system_dir.join("shared.h"));
    let _ = std::fs::remove_file(after_dir.join("after_only.h"));
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(quote_dir);
    let _ = std::fs::remove_dir(user_dir);
    let _ = std::fs::remove_dir(system_dir);
    let _ = std::fs::remove_dir(after_dir);
}

#[test]
fn internal_cpp_handles_has_include_and_elifdef() {
    let first_dir = temp_file("internal-cpp-has-include-first", "d");
    let second_dir = temp_file("internal-cpp-has-include-second", "d");
    let src = temp_file("internal-cpp-has-include", "c");
    std::fs::create_dir(&first_dir).expect("failed to create first include dir");
    std::fs::create_dir(&second_dir).expect("failed to create second include dir");
    let first_header = first_dir.join("rnqcc_has.h");
    let next_header = second_dir.join("rnqcc_next_has.h");
    std::fs::write(&first_header, "#define HAS_INCLUDE_VALUE 40\n")
        .expect("failed to write first header");
    std::fs::write(&next_header, "#define HAS_NEXT_VALUE 2\n")
        .expect("failed to write next header");
    std::fs::write(
        &src,
        "#define RNQCC_HAS <rnqcc_has.h>\n\
         #if __has_include(RNQCC_HAS)\n\
         #include RNQCC_HAS\n\
         #else\n\
         #define HAS_INCLUDE_VALUE 1\n\
         #endif\n\
         #define ENABLED\n\
         #if 0\n\
         #define BRANCH_VALUE 1\n\
         #elifdef ENABLED\n\
         #define BRANCH_VALUE 2\n\
         #else\n\
         #define BRANCH_VALUE 3\n\
         #endif\n\
         #if 0\n\
         #define OTHER_VALUE 1\n\
         #elifndef MISSING_FOR_ELIFNDEF\n\
         #define OTHER_VALUE 0\n\
         #endif\n\
         #include <rnqcc_next_has.h>\n\
         int main(void) { return HAS_INCLUDE_VALUE + HAS_NEXT_VALUE + BRANCH_VALUE + OTHER_VALUE; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-I")
        .arg(&first_dir)
        .arg("-I")
        .arg(&second_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 2 + 2 + 0; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(first_header);
    let _ = std::fs::remove_file(next_header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(first_dir);
    let _ = std::fs::remove_dir(second_dir);
}

#[test]
fn internal_cpp_handles_include_next() {
    let first_dir = temp_file("internal-cpp-include-next-first", "d");
    let second_dir = temp_file("internal-cpp-include-next-second", "d");
    let src = temp_file("internal-cpp-include-next", "c");
    std::fs::create_dir(&first_dir).expect("failed to create first include dir");
    std::fs::create_dir(&second_dir).expect("failed to create second include dir");
    let first_header = first_dir.join("rnqcc_next.h");
    let next = second_dir.join("rnqcc_next.h");
    std::fs::write(
        &first_header,
        "#if __has_include_next(<rnqcc_next.h>)\n#define HAS_NEXT_CHECK 1\n#else\n#define HAS_NEXT_CHECK 0\n#endif\n#include_next <rnqcc_next.h>\n#define WRAPPED 2\n",
    )
    .expect("failed to write first header");
    std::fs::write(&next, "#define NEXT_VALUE 40\n").expect("failed to write next header");
    std::fs::write(
        &src,
        "#include <rnqcc_next.h>\nint main(void) { return NEXT_VALUE + WRAPPED + HAS_NEXT_CHECK; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-I")
        .arg(&first_dir)
        .arg("-I")
        .arg(&second_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 2 + 1; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(first_header);
    let _ = std::fs::remove_file(next);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(first_dir);
    let _ = std::fs::remove_dir(second_dir);
}

#[test]
fn internal_cpp_handles_quoted_include_next() {
    let first_dir = temp_file("internal-cpp-quoted-include-next-first", "d");
    let second_dir = temp_file("internal-cpp-quoted-include-next-second", "d");
    let src = temp_file("internal-cpp-quoted-include-next", "c");
    std::fs::create_dir(&first_dir).expect("failed to create first include dir");
    std::fs::create_dir(&second_dir).expect("failed to create second include dir");
    let first_header = first_dir.join("rnqcc_quoted_next.h");
    let next = second_dir.join("rnqcc_quoted_next.h");
    std::fs::write(
        &first_header,
        "#if __has_include_next(\"rnqcc_quoted_next.h\")\n#define HAS_NEXT_CHECK 1\n#else\n#define HAS_NEXT_CHECK 0\n#endif\n#include_next \"rnqcc_quoted_next.h\"\n#define WRAPPED 2\n",
    )
    .expect("failed to write first header");
    std::fs::write(&next, "#define NEXT_VALUE 40\n").expect("failed to write next header");
    std::fs::write(
        &src,
        "#include \"rnqcc_quoted_next.h\"\nint main(void) { return NEXT_VALUE + WRAPPED + HAS_NEXT_CHECK; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-iquote")
        .arg(&first_dir)
        .arg("-I")
        .arg(&second_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 2 + 1; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(first_header);
    let _ = std::fs::remove_file(next);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(first_dir);
    let _ = std::fs::remove_dir(second_dir);
}

#[test]
fn internal_cpp_handles_macro_generated_include_next_and_has_include_next() {
    let first_dir = temp_file("internal-cpp-macro-include-next-first", "d");
    let second_dir = temp_file("internal-cpp-macro-include-next-second", "d");
    let src = temp_file("internal-cpp-macro-include-next", "c");
    std::fs::create_dir(&first_dir).expect("failed to create first include dir");
    std::fs::create_dir(&second_dir).expect("failed to create second include dir");
    let first_header = first_dir.join("rnqcc_macro_next.h");
    let next = second_dir.join("rnqcc_macro_next.h");
    std::fs::write(
        &first_header,
        "#define NEXT_HEADER <rnqcc_macro_next.h>\n\
         #if __has_include_next(NEXT_HEADER)\n\
         #define HAS_NEXT_MACRO 1\n\
         #else\n\
         #define HAS_NEXT_MACRO 0\n\
         #endif\n\
         #include_next NEXT_HEADER\n\
         #define WRAPPED_NEXT 2\n",
    )
    .expect("failed to write first header");
    std::fs::write(&next, "#define NEXT_MACRO_VALUE 40\n").expect("failed to write next header");
    std::fs::write(
        &src,
        "#include <rnqcc_macro_next.h>\nint main(void) { return NEXT_MACRO_VALUE + WRAPPED_NEXT + HAS_NEXT_MACRO; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-I")
        .arg(&first_dir)
        .arg("-I")
        .arg(&second_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("int main(void) { return 40 + 2 + 1; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(first_header);
    let _ = std::fs::remove_file(next);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(first_dir);
    let _ = std::fs::remove_dir(second_dir);
}

#[test]
fn internal_cpp_allows_guarded_recursive_include() {
    let include_dir = temp_file("internal-cpp-guarded-recursive-include", "d");
    let src = temp_file("internal-cpp-guarded-recursive-include", "c");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("guarded.h");
    std::fs::write(
        &header,
        "#ifndef RNQCC_GUARDED_H\n\
         #define RNQCC_GUARDED_H\n\
         #include <guarded.h>\n\
         #define GUARDED_VALUE 42\n\
         #endif\n",
    )
    .expect("failed to write guarded header");
    std::fs::write(
        &src,
        "#include <guarded.h>\nint main(void) { return GUARDED_VALUE; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-I")
        .arg(&include_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int main(void) { return 42; }"), "{stdout}");

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(include_dir);
}

#[test]
fn internal_cpp_rejects_unguarded_recursive_include() {
    let include_dir = temp_file("internal-cpp-unguarded-recursive-include", "d");
    let src = temp_file("internal-cpp-unguarded-recursive-include", "c");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("unguarded.h");
    std::fs::write(&header, "#include <unguarded.h>\n#define VALUE 42\n")
        .expect("failed to write unguarded header");
    std::fs::write(&src, "#include <unguarded.h>\nint value = VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-I")
        .arg(&include_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(
        stderr(output).contains("recursive include"),
        "expected recursive include diagnostic"
    );

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(include_dir);
}

#[test]
fn internal_cpp_reports_warning_directive_without_failing() {
    let src = temp_file("internal-cpp-warning", "c");
    std::fs::write(
        &src,
        "#warning keep going\n#ident \"ignored\"\nint main(void) { return 0; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stderr(output).contains("warning: keep going"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_reports_error_directive() {
    let src = temp_file("internal-cpp-error", "c");
    std::fs::write(&src, "#error stop here\nint main(void) { return 0; }\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("#error stop here"), "{stderr}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn internal_cpp_rejects_unsupported_directives() {
    let src = temp_file("internal-cpp-unsupported", "c");
    std::fs::write(
        &src,
        "#include <rnqcc_missing_system_header.h>\nint main(void) { return 0; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("include not found: <rnqcc_missing_system_header.h>"));

    let _ = std::fs::remove_file(src);
}

#[test]
fn preserves_inner_i_suffix_when_emitting_assembly() {
    let src = temp_file("double-i", "i.i");
    let asm = src.with_extension("s");
    let wrong_asm = src.with_extension("").with_extension("s");
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write input");
    let _ = std::fs::remove_file(&asm);
    let _ = std::fs::remove_file(&wrong_asm);

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(asm.exists());
    assert!(!wrong_asm.exists());

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(asm);
}

#[test]
fn emits_object_to_requested_output() {
    let out = temp_file("obj", "o");

    let output = Command::new(rnqcc())
        .arg("-c")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(
        std::fs::metadata(&out)
            .expect("missing object output")
            .len()
            > 0
    );

    let _ = std::fs::remove_file(out);
}

#[test]
fn assembles_existing_assembly_input_to_object() {
    let asm = temp_file("existing-asm-input", "s");
    let obj = temp_file("existing-asm-input", "o");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&asm)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to emit assembly");
    assert!(output.status.success(), "{}", stderr(output));

    let output = Command::new(rnqcc())
        .arg("-c")
        .arg("-o")
        .arg(&obj)
        .arg(&asm)
        .output()
        .expect("failed to assemble existing assembly");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(std::fs::metadata(&obj).expect("missing object").len() > 0);

    let _ = std::fs::remove_file(asm);
    let _ = std::fs::remove_file(obj);
}

#[test]
fn links_existing_object_input_with_c_source() {
    let helper_src = temp_file("helper-object", "c");
    let helper_obj = temp_file("helper-object", "o");
    let main_src = temp_file("main-with-helper-object", "c");
    let exe = temp_file("main-with-helper-object", "bin");

    std::fs::write(&helper_src, "int helper(void) { return 17; }\n")
        .expect("failed to write helper source");
    std::fs::write(
        &main_src,
        "int helper(void); int main(void) { return helper(); }\n",
    )
    .expect("failed to write main source");

    let output = Command::new("cc")
        .arg("-c")
        .arg(&helper_src)
        .arg("-o")
        .arg(&helper_obj)
        .output()
        .expect("failed to run host cc");
    assert!(output.status.success(), "{}", stderr(output));

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&main_src)
        .arg(&helper_obj)
        .output()
        .expect("failed to link with object input");
    assert!(output.status.success(), "{}", stderr(output));

    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(17));

    let _ = std::fs::remove_file(helper_src);
    let _ = std::fs::remove_file(helper_obj);
    let _ = std::fs::remove_file(main_src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn links_existing_static_library_input_with_c_source() {
    let helper_src = temp_file("helper-library", "c");
    let helper_obj = temp_file("helper-library", "o");
    let helper_lib = temp_file("helper-library", "a");
    let main_src = temp_file("main-with-helper-library", "c");
    let exe = temp_file("main-with-helper-library", "bin");

    std::fs::write(&helper_src, "int helper(void) { return 23; }\n")
        .expect("failed to write helper source");
    std::fs::write(
        &main_src,
        "int helper(void); int main(void) { return helper(); }\n",
    )
    .expect("failed to write main source");

    let output = Command::new("cc")
        .arg("-c")
        .arg(&helper_src)
        .arg("-o")
        .arg(&helper_obj)
        .output()
        .expect("failed to run host cc");
    assert!(output.status.success(), "{}", stderr(output));

    let output = Command::new("ar")
        .arg("rcs")
        .arg(&helper_lib)
        .arg(&helper_obj)
        .output()
        .expect("failed to run ar");
    assert!(output.status.success(), "{}", stderr(output));

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&main_src)
        .arg(&helper_lib)
        .output()
        .expect("failed to link with static library input");
    assert!(output.status.success(), "{}", stderr(output));

    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(23));

    let _ = std::fs::remove_file(helper_src);
    let _ = std::fs::remove_file(helper_obj);
    let _ = std::fs::remove_file(helper_lib);
    let _ = std::fs::remove_file(main_src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn links_existing_shared_library_input_with_c_source() {
    let helper_src = temp_file("helper-shared-library", "c");
    let helper_lib = if cfg!(target_os = "macos") {
        temp_file("helper-shared-library", "dylib")
    } else {
        temp_file("helper-shared-library", "so")
    };
    let main_src = temp_file("main-with-helper-shared-library", "c");
    let exe = temp_file("main-with-helper-shared-library", "bin");

    std::fs::write(&helper_src, "int helper(void) { return 29; }\n")
        .expect("failed to write helper source");
    std::fs::write(
        &main_src,
        "int helper(void); int main(void) { return helper(); }\n",
    )
    .expect("failed to write main source");

    let mut cc = Command::new("cc");
    if cfg!(target_os = "macos") {
        cc.arg("-dynamiclib");
    } else {
        cc.args(["-shared", "-fPIC"]);
    }
    let output = cc
        .arg(&helper_src)
        .arg("-o")
        .arg(&helper_lib)
        .output()
        .expect("failed to run host cc");
    assert!(output.status.success(), "{}", stderr(output));

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&main_src)
        .arg(&helper_lib)
        .output()
        .expect("failed to link with shared library input");
    assert!(output.status.success(), "{}", stderr(output));

    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(29));

    let _ = std::fs::remove_file(helper_src);
    let _ = std::fs::remove_file(helper_lib);
    let _ = std::fs::remove_file(main_src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn keep_temps_preserves_preprocessed_and_assembly_files() {
    let src = temp_file("keep-temps", "c");
    let preprocessed = src.with_extension("i");
    let asm = src.with_extension("s");
    let obj = src.with_extension("o");
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write source input");

    let output = Command::new(rnqcc())
        .arg("--keep-temps")
        .arg("-c")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(preprocessed.exists());
    assert!(asm.exists());
    assert!(obj.exists());

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(preprocessed);
    let _ = std::fs::remove_file(asm);
    let _ = std::fs::remove_file(obj);
}

#[cfg(unix)]
fn write_cc_script(name: &str, log: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_file(name, "sh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nexec cc \"$@\"\n",
            log.display()
        ),
    )
    .expect("failed to write cc script");
    let mut perms = std::fs::metadata(&path)
        .expect("missing cc script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("failed to chmod cc script");
    path
}

#[cfg(unix)]
fn write_failing_cc(name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_file(name, "sh");
    std::fs::write(&path, "#!/bin/sh\nexit 42\n").expect("failed to write failing cc script");
    let mut perms = std::fs::metadata(&path)
        .expect("missing cc script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("failed to chmod cc script");
    path
}

#[cfg(unix)]
fn write_failing_cc_with_stderr(name: &str, message: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_file(name, "sh");
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf '%s\\n' '{}' >&2\nexit 42\n", message),
    )
    .expect("failed to write failing cc script");
    let mut perms = std::fs::metadata(&path)
        .expect("missing cc script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("failed to chmod cc script");
    path
}

#[cfg(unix)]
fn write_stderr_cc(name: &str, message: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_file(name, "sh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{}' >&2\nexec cc \"$@\"\n",
            message
        ),
    )
    .expect("failed to write stderr cc script");
    let mut perms = std::fs::metadata(&path)
        .expect("missing cc script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("failed to chmod cc script");
    path
}

#[cfg(unix)]
fn write_logging_cc(name: &str, log: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = temp_file(name, "sh");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nexit 0\n",
            log.display()
        ),
    )
    .expect("failed to write logging cc script");
    let mut perms = std::fs::metadata(&path)
        .expect("missing cc script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("failed to chmod cc script");
    path
}

#[cfg(unix)]
#[test]
fn uses_cc_option_for_preprocessing() {
    let log_path = temp_file("cc-option", "log");
    let cc_script = write_cc_script("cc-option", &log_path);

    let output = Command::new(rnqcc())
        .arg("--cc")
        .arg(&cc_script)
        .args(["-E", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let log = std::fs::read_to_string(&log_path).expect("failed to read cc log");
    assert!(log.contains("-E"));
    assert!(log.contains("-P"));

    let _ = std::fs::remove_file(cc_script);
    let _ = std::fs::remove_file(log_path);
}

#[cfg(unix)]
#[test]
fn forwards_successful_external_cpp_stderr_without_polluting_stdout() {
    let cc_script = write_stderr_cc("cc-stderr", "rnqcc external cpp note");

    let output = Command::new(rnqcc())
        .arg("--cc")
        .arg(&cc_script)
        .args(["-E", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("rnqcc external cpp note"));
    assert!(stdout.contains("return 42;"));
    assert!(!stdout.contains("rnqcc external cpp note"));

    let _ = std::fs::remove_file(cc_script);
}

#[cfg(unix)]
#[test]
fn cc_option_overrides_cc_environment() {
    let log = temp_file("cc-precedence", "log");
    let cc_script = write_cc_script("cc-precedence", &log);

    let output = Command::new(rnqcc())
        .env("CC", "/usr/bin/false")
        .arg("--cc")
        .arg(&cc_script)
        .args(["-E", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(std::fs::read_to_string(&log)
        .expect("failed to read cc log")
        .contains("-E"));

    let _ = std::fs::remove_file(cc_script);
    let _ = std::fs::remove_file(log);
}

#[cfg(unix)]
#[test]
fn uses_cc_environment_for_preprocessing() {
    let log = temp_file("cc-env", "log");
    let cc_script = write_cc_script("cc-env", &log);

    let output = Command::new(rnqcc())
        .env("CC", &cc_script)
        .args(["-E", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    assert!(std::fs::read_to_string(&log)
        .expect("failed to read cc log")
        .contains("-E"));

    let _ = std::fs::remove_file(cc_script);
    let _ = std::fs::remove_file(log);
}

#[cfg(unix)]
#[test]
fn cleans_generated_assembly_when_object_assembly_fails() {
    let cc = write_failing_cc("failing-cc");
    let asm = std::path::Path::new("tests/return_42.s");
    let _ = std::fs::remove_file(asm);

    let output = Command::new(rnqcc())
        .arg("--cc")
        .arg(&cc)
        .args(["-c", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(!asm.exists());

    let _ = std::fs::remove_file(cc);
}

#[test]
fn internal_cpp_accepts_wp_options() {
    let src = temp_file("internal-cpp-wp", "c");
    std::fs::write(
        &src,
        "#ifndef VALUE\n#error missing VALUE\n#endif\nint value = VALUE;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg("-Wp,-DVALUE=42")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int value = 42;"));
}

#[test]
fn internal_cpp_traces_includes_with_h_option() {
    let include_dir = temp_file("internal-cpp-trace", "dir");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("trace.h");
    std::fs::write(&header, "int traced = 7;\n").expect("failed to write header");
    let src = temp_file("internal-cpp-trace", "c");
    std::fs::write(&src, "#include \"trace.h\"\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-H")
        .arg("-E")
        .arg("-I")
        .arg(&include_dir)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stderr.contains("trace.h"));
    assert!(stdout.contains("int traced = 7;"));
}

#[test]
fn internal_cpp_x_c_accepts_extensionless_source() {
    let src = temp_file("internal-cpp-x-c", "input");
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .args(["-x", "c", "-E"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int main(void)"));
}

#[test]
fn internal_cpp_reads_c_source_from_stdin() {
    let mut child = Command::new(rnqcc())
        .arg("--internal-cpp")
        .args(["-x", "c", "-E", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn rnqcc");

    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .expect("missing stdin")
            .write_all(b"#define VALUE 9\nint value = VALUE;\n")
            .expect("failed to write stdin");
    }

    let output = child.wait_with_output().expect("failed to wait for rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int value = 9;"));
}

#[test]
fn internal_cpp_supports_gcc_poison_pragma() {
    let src = temp_file("internal-cpp-poison", "c");
    std::fs::write(&src, "#pragma GCC poison BAD\nint value = BAD;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("poisoned identifier BAD"));
}

#[test]
fn internal_cpp_system_header_suppresses_warning_and_user_dependencies() {
    let include_dir = temp_file("internal-cpp-system-header", "dir");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("sys.h");
    std::fs::write(
        &header,
        "#pragma GCC system_header\n#warning should be suppressed\n#define SYS_VALUE 3\n",
    )
    .expect("failed to write header");
    let src = temp_file("internal-cpp-system-header", "c");
    std::fs::write(&src, "#include \"sys.h\"\nint value = SYS_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MM")
        .arg("-I")
        .arg(&include_dir)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stderr.contains("should be suppressed"));
    assert!(!stdout.contains("sys.h"));
}

#[test]
fn internal_cpp_clang_system_header_suppresses_warning_and_user_dependencies() {
    let include_dir = temp_file("internal-cpp-clang-system-header", "dir");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("sys.h");
    std::fs::write(
        &header,
        "#pragma clang system_header\n#warning should be suppressed\n#define SYS_VALUE 5\n",
    )
    .expect("failed to write header");
    let src = temp_file("internal-cpp-clang-system-header", "c");
    std::fs::write(&src, "#include \"sys.h\"\nint value = SYS_VALUE;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MM")
        .arg("-I")
        .arg(&include_dir)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stderr.contains("should be suppressed"));
    assert!(!stdout.contains("sys.h"));

    let _ = std::fs::remove_file(header);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(include_dir);
}

#[test]
fn internal_cpp_exposes_common_predefined_limit_macros() {
    let src = temp_file("internal-cpp-predefined-limits", "c");
    std::fs::write(
        &src,
        "#if __SIZE_MAX__ <= __INT_MAX__\n#error expected lp64 size max\n#endif\n\
         #if __SIZEOF_LONG_LONG__ != 8 || __SIZEOF_BOOL__ != 1\n#error bad sizeof macro\n#endif\n\
         #if __INT8_MAX__ != 127 || __UINT8_MAX__ != 255 || __INT16_MAX__ != 32767 || __UINT16_MAX__ != 65535\n#error bad fixed-width max\n#endif\n\
         #if __INT32_MAX__ != 2147483647 || __UINT32_MAX__ != 4294967295U || __INT64_MAX__ != 9223372036854775807L\n#error bad 32/64 max\n#endif\n\
         __INT8_TYPE__ signed_byte;\n\
         __UINTPTR_TYPE__ uintptr_value;\n\
         char *version = __VERSION__;\n\
         int ok = __INT_WIDTH__;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("signed char signed_byte;"), "{stdout}");
    assert!(stdout.contains("unsigned long uintptr_value;"), "{stdout}");
    assert!(stdout.contains("char *version = \"rnqcc\";"), "{stdout}");
    assert!(stdout.contains("int ok = 32;"), "{stdout}");
}

#[test]
fn internal_cpp_line_markers_can_be_enabled() {
    let include_dir = temp_file("internal-cpp-line-markers", "dir");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let header = include_dir.join("marker.h");
    std::fs::write(&header, "int from_header = 1;\n").expect("failed to write header");
    let src = temp_file("internal-cpp-line-markers", "c");
    std::fs::write(&src, "#include \"marker.h\"\nint from_source = 2;\n")
        .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--line-markers")
        .arg("-E")
        .arg("-I")
        .arg(&include_dir)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("# 1 \""));
    assert!(stdout.contains("marker.h"));
    assert!(stdout.contains("int from_source = 2;"));
}

#[test]
fn internal_cpp_line_marker_output_can_feed_compiler_stages() {
    let src = temp_file("internal-cpp-line-markers-compile", "c");
    let asm = temp_file("internal-cpp-line-markers-compile", "s");
    std::fs::write(&src, "int main(void) { return 42; }\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--line-markers")
        .arg("-S")
        .arg("-o")
        .arg(&asm)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let assembly = std::fs::read_to_string(&asm).expect("failed to read assembly");
    assert!(assembly.contains("main"), "{assembly}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(asm);
}

#[test]
fn internal_cpp_diagnostics_include_source_location() {
    let src = temp_file("internal-cpp-located-error", "c");
    std::fs::write(&src, "\n#error located\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains(&format!("{}:2:", src.display())));
    assert!(stderr.contains("#error located"));
}

#[test]
fn internal_cpp_if_handles_large_integer_constants() {
    let src = temp_file("internal-cpp-large-if", "c");
    std::fs::write(
        &src,
        "#if 18446744073709551615UL < 9223372036854775807L\n#error bad comparison\n#endif\nint ok = 1;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int ok = 1;"));
}

#[test]
fn internal_cpp_exposes_feature_test_macros() {
    let src = temp_file("internal-cpp-feature-macros", "c");
    std::fs::write(
        &src,
        "#ifndef __GNUC_STDC_INLINE__\n#error missing inline macro\n#endif\nint widths = __SIZEOF_LONG_DOUBLE__ + __SIZEOF_WCHAR_T__;\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(stdout(output).contains("int widths = 16 + 4;"));
}

#[cfg(unix)]
#[test]
fn expands_response_file_arguments() {
    let src = temp_file("response file source", "c");
    let out = temp_file("response file output", "s");
    let rsp = temp_file("response-file", "rsp");
    std::fs::write(&src, "int main(void) { return 42; }\n").expect("failed to write source");
    std::fs::write(
        &rsp,
        format!(
            "-S -O2 -g -Wall -Wextra -Werror -o \"{}\" \"{}\"\n",
            out.display(),
            src.display()
        ),
    )
    .expect("failed to write response file");

    let output = Command::new(rnqcc())
        .arg(format!("@{}", rsp.display()))
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(std::fs::read_to_string(&out)
        .expect("failed to read assembly output")
        .contains("main"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(rsp);
}

#[cfg(unix)]
#[test]
fn expands_nested_response_files_relative_to_parent() {
    let dir = temp_file("nested-response", "d");
    let subdir = dir.join("sub");
    let src = temp_file("nested-response-source", "c");
    let out = temp_file("nested-response-output", "s");
    let root_rsp = dir.join("root.rsp");
    let nested_rsp = subdir.join("flags.rsp");

    std::fs::create_dir(&dir).expect("failed to create response dir");
    std::fs::create_dir(&subdir).expect("failed to create response subdir");
    std::fs::write(&src, "int main(void) { return 42; }\n").expect("failed to write source");
    std::fs::write(&root_rsp, "@sub/flags.rsp\n").expect("failed to write root response file");
    std::fs::write(
        &nested_rsp,
        format!("-S -o \"{}\" \"{}\"\n", out.display(), src.display()),
    )
    .expect("failed to write nested response file");

    let output = Command::new(rnqcc())
        .arg(format!("@{}", root_rsp.display()))
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(std::fs::read_to_string(&out)
        .expect("failed to read assembly output")
        .contains("main"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(root_rsp);
    let _ = std::fs::remove_file(nested_rsp);
    let _ = std::fs::remove_dir(subdir);
    let _ = std::fs::remove_dir(dir);
}

#[cfg(unix)]
#[test]
fn expands_quoted_nested_response_file_paths_with_spaces() {
    let dir = temp_file("quoted-nested-response", "d");
    let subdir = dir.join("sub dir");
    let src = temp_file("quoted nested response source", "c");
    let out = temp_file("quoted nested response output", "s");
    let root_rsp = dir.join("root.rsp");
    let nested_rsp = subdir.join("flags file.rsp");

    std::fs::create_dir(&dir).expect("failed to create response dir");
    std::fs::create_dir(&subdir).expect("failed to create response subdir");
    std::fs::write(&src, "int main(void) { return 42; }\n").expect("failed to write source");
    std::fs::write(&root_rsp, "@\"sub dir/flags file.rsp\"\n")
        .expect("failed to write root response file");
    std::fs::write(
        &nested_rsp,
        format!("-S -o \"{}\" \"{}\"\n", out.display(), src.display()),
    )
    .expect("failed to write nested response file");

    let output = Command::new(rnqcc())
        .arg(format!("@{}", root_rsp.display()))
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(std::fs::read_to_string(&out)
        .expect("failed to read assembly output")
        .contains("main"));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(root_rsp);
    let _ = std::fs::remove_file(nested_rsp);
    let _ = std::fs::remove_dir(subdir);
    let _ = std::fs::remove_dir(dir);
}

#[cfg(unix)]
#[test]
fn driver_passes_common_build_system_flags() {
    let log = temp_file("driver-pass-through", "log");
    let cc = write_logging_cc("driver-pass-through", &log);
    let out = temp_file("driver-pass-through", "out");

    let output = Command::new(rnqcc())
        .arg("--cc")
        .arg(&cc)
        .arg("--internal-cpp")
        .arg("--sysroot")
        .arg("/tmp/rnqcc-sysroot")
        .arg("-std=c11")
        .arg("-O2")
        .arg("-g")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-fPIC")
        .arg("-m64")
        .arg("-pthread")
        .arg("-nostdlib")
        .arg("-nodefaultlibs")
        .arg("-pie")
        .arg("-no-pie")
        .arg("-L/tmp/rnqcc-lib")
        .arg("-lrnqcc")
        .arg("-Wl,-z,defs")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let log = std::fs::read_to_string(&log).expect("failed to read cc log");
    assert!(log.contains("--sysroot"));
    assert!(log.contains("/tmp/rnqcc-sysroot"));
    assert!(log.contains("-nostdlib"));
    assert!(log.contains("-nodefaultlibs"));
    assert!(log.contains("-pie"));
    assert!(log.contains("-no-pie"));
    assert!(log.contains("-fPIC"));
    assert!(log.contains("-pthread"));
    assert!(log.contains("-L/tmp/rnqcc-lib"));
    assert!(log.contains("-lrnqcc"));
    assert!(log.contains("-z"));
    assert!(log.contains("defs"));

    let _ = std::fs::remove_file(cc);
    let _ = std::fs::remove_file(out);
}

#[cfg(unix)]
#[test]
fn driver_normalizes_more_real_project_flags() {
    let log = temp_file("driver-more-flags", "log");
    let cc = write_logging_cc("driver-more-flags", &log);
    let out = temp_file("driver-more-flags", "out");

    let output = Command::new(rnqcc())
        .arg("--cc")
        .arg(&cc)
        .arg("--internal-cpp")
        .arg("-pipe")
        .arg("-Wno-unused-parameter")
        .arg("-Wno-sign-compare")
        .arg("-Wno-error=implicit-function-declaration")
        .arg("-Werror=implicit-function-declaration")
        .arg("-fsanitize=address")
        .arg("-fuse-ld=lld")
        .arg("-static-libasan")
        .arg("-shared-libgcc")
        .arg("-Xlinker")
        .arg("-dead_strip")
        .arg("-F/tmp/rnqcc-frameworks")
        .arg("-framework")
        .arg("CoreFoundation")
        .arg("-Wl,-rpath,/tmp/rnqcc-rpath")
        .arg("-o")
        .arg(&out)
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let log = std::fs::read_to_string(&log).expect("failed to read cc log");
    assert!(log.contains("-fsanitize=address"));
    assert!(log.contains("-fuse-ld=lld"));
    assert!(log.contains("-static-libasan"));
    assert!(log.contains("-shared-libgcc"));
    assert!(log.contains("-dead_strip"));
    assert!(log.contains("-F/tmp/rnqcc-frameworks"));
    assert!(log.contains("CoreFoundation"));
    assert!(log.contains("-rpath"));
    assert!(log.contains("/tmp/rnqcc-rpath"));
    assert!(!log.contains("-pipe"));
    assert!(!log.contains("-Wno-unused-parameter"));
    assert!(!log.contains("-Wno-sign-compare"));
    assert!(!log.contains("-Wno-error=implicit-function-declaration"));
    assert!(!log.contains("-Werror=implicit-function-declaration"));

    let _ = std::fs::remove_file(cc);
    let _ = std::fs::remove_file(out);
}

#[test]
fn driver_accepts_gcc_nostdinc_spelling() {
    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-E")
        .arg("tests/return_42.c")
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
}

#[test]
fn global_pointer_initializer_accepts_null_based_member_address() {
    let src = temp_file("global-null-member-ptr-init", "c");
    let exe = temp_file("global-null-member-ptr-init", "bin");
    std::fs::write(
        &src,
        r#"
struct auth_config_rec {
    char *auth_pwfile;
    int x;
};

void *ptr = &((struct auth_config_rec *)0)->x;

int main(void) {
    return (unsigned long)ptr == __builtin_offsetof(struct auth_config_rec, x) ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn global_pointer_initializer_accepts_label_plus_member_offset() {
    let src = temp_file("global-label-member-ptr-init", "c");
    let exe = temp_file("global-label-member-ptr-init", "bin");
    std::fs::write(
        &src,
        r#"
struct box {
    char pad;
    int value;
};

struct box global_box = { 3, 42 };
int *global_value = &global_box.value;

int main(void) {
    return *global_value;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn global_pointer_initializer_accepts_array_index_and_nested_member_offsets() {
    let src = temp_file("global-array-nested-ptr-init", "c");
    let exe = temp_file("global-array-nested-ptr-init", "bin");
    std::fs::write(
        &src,
        r#"
struct node {
    int a;
    struct { unsigned x; unsigned y; } s;
    int b;
    struct node *next;
};

struct node nodes[10];
struct node one = { 1, { 2, 42 }, 3, 0 };

struct node *node_ptr = &nodes[3];
unsigned *y_ptr = &one.s.y;
struct node **next_ptr = &one.next;

int main(void) {
    unsigned long array_diff = (unsigned long)node_ptr - (unsigned long)nodes;
    unsigned long y_diff = (unsigned long)y_ptr - (unsigned long)&one.a;
    unsigned long next_diff = (unsigned long)next_ptr - (unsigned long)y_ptr;
    if (array_diff != 3 * sizeof(struct node)) {
        return 1;
    }
    if (y_diff != __builtin_offsetof(struct node, s.y)) {
        return 2;
    }
    if (next_diff != __builtin_offsetof(struct node, next) - __builtin_offsetof(struct node, s.y)) {
        return 3;
    }
    return *y_ptr;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn accepts_gnu_colon_field_designated_initializer() {
    let src = temp_file("gnu-colon-designator", "c");
    let exe = temp_file("gnu-colon-designator", "bin");
    std::fs::write(
        &src,
        r#"
union u {
    double d;
    int i[3];
};

int signbit(double x) {
    union u v = { d: x };
    return v.i[1] < 0;
}

int main(void) {
    return signbit(-1.0) && !signbit(2.0) ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn accepts_implicit_int_after_register_storage_class() {
    let src = temp_file("register-implicit-int", "c");
    let exe = temp_file("register-implicit-int", "bin");
    std::fs::write(
        &src,
        r#"
sum(to, from, count)
register short *to, *from;
register count;
{
    register n = (count + 7) / 8;
    do {
        *to += *from++;
    } while (--n > 0);
}

int main(void) {
    short in[8];
    short out = 0;
    int i;
    for (i = 0; i < 8; i = i + 1) {
        in[i] = i + 1;
    }
    sum(&out, in, 64);
    return out == 36 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn treats_alloca_as_pointer_returning_builtin_without_visible_prototype() {
    let src = temp_file("alloca-builtin-return", "c");
    let exe = temp_file("alloca-builtin-return", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    char *buf = alloca(16);
    buf[0] = 42;
    return buf[0];
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn lowers_sprintf_checked_builtin_to_host_sprintf() {
    let src = temp_file("sprintf-chk-builtin", "c");
    let exe = temp_file("sprintf-chk-builtin", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    char buf[16];
    int n = __builtin___sprintf_chk(buf, 0, 16, "%d", 42);
    return n == 2 && buf[0] == '4' && buf[1] == '2' ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_gnu_range_designators_and_integer_mode_typedefs() {
    let src = temp_file("gnu-range-designator-mode", "c");
    let exe = temp_file("gnu-range-designator-mode", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned int __attribute__((mode(QI))) u8;
typedef unsigned int __attribute__((mode(HI))) u16;
typedef unsigned int __attribute__((mode(DI))) u64;

static union {
    u8 bytes[8];
    u64 word;
} filled = {{ [0 ... 7] = 0xaa }};

int main(void) {
    static union {
        u8 bytes[8];
        struct __attribute__((packed)) {
            u8 pad[1];
            u16 value;
        } view;
    } local = {{ [0 ... 7] = 0xaa }};

    local.view.value = 0x1234;
    return sizeof(u8) == 1
        && sizeof(u16) == 2
        && sizeof(u64) == 8
        && filled.bytes[0] == 0xaa
        && filled.bytes[7] == 0xaa
        && local.bytes[0] == 0xaa
        && local.bytes[1] == 0x34
        && local.bytes[2] == 0x12
        && local.bytes[3] == 0xaa
        ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_static_pointer_initializer_to_member_after_array_pointer_arithmetic() {
    let src = temp_file("static-member-pointer-init", "c");
    let exe = temp_file("static-member-pointer-init", "bin");
    std::fs::write(
        &src,
        r#"
typedef struct item {
    int id;
    char *name;
} Item;

Item items[] = {
    { 1, "one" },
    { 2, "two" },
};

int *second_id = (int *)&((items + 1)->id);
int *first_id = (int *)&(items->id);

int main(void) {
    return *first_id == 1 && *second_id == 2 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_builtin_frame_address_as_pointer_value() {
    let src = temp_file("builtin-frame-address", "c");
    let exe = temp_file("builtin-frame-address", "bin");
    std::fs::write(
        &src,
        r#"
int check(const char *caller_local) {
    const char callee_local = 0;
    const char *frame = __builtin_frame_address(0);
    if (caller_local >= &callee_local) {
        return caller_local >= frame && frame >= &callee_local;
    }
    return caller_local <= frame && frame <= &callee_local;
}

int wrapper(void) {
    const char caller_local = 0;
    return check(&caller_local);
}

int main(void) {
    return wrapper() ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_common_sub_overflow_builtin() {
    let src = temp_file("sub-overflow-builtin", "c");
    let exe = temp_file("sub-overflow-builtin", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    unsigned x = 99;
    int ok1 = __builtin_sub_overflow(10u, 6u, &x);
    int ok2 = x == 4u;
    int ov = __builtin_sub_overflow(0u, 6u, &x);
    return !ok1 && ok2 && ov && x == (unsigned)-6 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_add_overflow_when_output_aliases_input() {
    let src = temp_file("add-overflow-alias-input", "c");
    let exe = temp_file("add-overflow-alias-input", "bin");
    std::fs::write(
        &src,
        r#"
unsigned long f(unsigned long a, unsigned long b) {
    unsigned long overflow = __builtin_add_overflow(a, b, &a);
    return a + overflow;
}

int main(void) {
    return f(16UL, -16UL) == 1UL && f(16UL, -18UL) == -2UL ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_signed_long_mul_overflow_without_overcounting() {
    let src = temp_file("signed-long-mul-overflow-count", "c");
    let exe = temp_file("signed-long-mul-overflow-count", "bin");
    std::fs::write(
        &src,
        r#"
int overflows;

long test(long *x, int y) {
    long s = 1;
    for (int i = 0; i < y; i++) {
        if (__builtin_mul_overflow(s, x[i], &s)) {
            overflows++;
        }
    }
    return s;
}

int main(void) {
    long d[7] = { 975, 975, 975, 975, 975, 975, 975 };
    test(d, 7);
    return overflows == 1 ? 42 : overflows;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn mul_overflow_p_checks_destination_type_range() {
    let src = temp_file("mul-overflow-p-destination-range", "c");
    let exe = temp_file("mul-overflow-p-destination-range", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int hi = __builtin_mul_overflow_p(__INT_MAX__ / 35 + 1, 35, 0);
    int ok = __builtin_mul_overflow_p(__INT_MAX__ / 35, 35, 0);
    int lo = __builtin_mul_overflow_p((-__INT_MAX__ - 1) / -39 + 1, -39, 0);
    int wide = __builtin_mul_overflow_p(__LONG_MAX__ / 42 + 1, 42L, 0L);
    return hi == 1 && ok == 0 && lo == 1 && wide == 1 ? 42 : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn mul_overflow_p_matches_signed_boundary_cases() {
    let src = temp_file("mul-overflow-p-signed-boundaries", "c");
    let exe = temp_file("mul-overflow-p-signed-boundaries", "bin");
    std::fs::write(
        &src,
        r#"
__attribute__((noinline)) int foo(int x) { return __builtin_mul_overflow_p(x, 35, 0); }
__attribute__((noinline)) int bar(long x) { return __builtin_mul_overflow_p(x, 35L, 0L); }
__attribute__((noinline)) int baz(int x) { return __builtin_mul_overflow_p(42, x, 0); }
__attribute__((noinline)) int qux(long x) { return __builtin_mul_overflow_p(42, x, 0L); }
__attribute__((noinline)) int corge(int x) { return __builtin_mul_overflow_p(x, -39, 0); }
__attribute__((noinline)) int garply(long x) { return __builtin_mul_overflow_p(x, -39L, 0L); }
__attribute__((noinline)) int grault(int x) { return __builtin_mul_overflow_p(-46, x, 0); }
__attribute__((noinline)) int waldo(long x) { return __builtin_mul_overflow_p(-46, x, 0L); }

int main(void) {
    int failures = 0;
    failures += foo(0) != 0;
    failures += foo(__INT_MAX__ / 35) != 0;
    failures += foo(__INT_MAX__ / 35 + 1) != 1;
    failures += foo(__INT_MAX__) != 1;
    failures += foo((-__INT_MAX__ - 1) / 35) != 0;
    failures += foo((-__INT_MAX__ - 1) / 35 - 1) != 1;
    failures += foo(-__INT_MAX__ - 1) != 1;
    failures += bar(__LONG_MAX__ / 35) != 0;
    failures += bar(__LONG_MAX__ / 35 + 1) != 1;
    failures += bar(__LONG_MAX__) != 1;
    failures += bar((-__LONG_MAX__ - 1) / 35) != 0;
    failures += bar((-__LONG_MAX__ - 1) / 35 - 1) != 1;
    failures += bar(-__LONG_MAX__ - 1) != 1;
    failures += baz(__INT_MAX__ / 42) != 0;
    failures += baz(__INT_MAX__ / 42 + 1) != 1;
    failures += baz(__INT_MAX__) != 1;
    failures += baz((-__INT_MAX__ - 1) / 42) != 0;
    failures += baz((-__INT_MAX__ - 1) / 42 - 1) != 1;
    failures += baz(-__INT_MAX__ - 1) != 1;
    failures += qux(__LONG_MAX__ / 42) != 0;
    failures += qux(__LONG_MAX__ / 42 + 1) != 1;
    failures += qux(__LONG_MAX__) != 1;
    failures += qux((-__LONG_MAX__ - 1) / 42) != 0;
    failures += qux((-__LONG_MAX__ - 1) / 42 - 1) != 1;
    failures += qux(-__LONG_MAX__ - 1) != 1;
    failures += corge(__INT_MAX__ / -39) != 0;
    failures += corge(__INT_MAX__ / -39 - 1) != 1;
    failures += corge(__INT_MAX__) != 1;
    failures += corge((-__INT_MAX__ - 1) / -39) != 0;
    failures += corge((-__INT_MAX__ - 1) / -39 + 1) != 1;
    failures += corge(-__INT_MAX__ - 1) != 1;
    failures += garply(__LONG_MAX__ / -39) != 0;
    failures += garply(__LONG_MAX__ / -39 - 1) != 1;
    failures += garply(__LONG_MAX__) != 1;
    failures += garply((-__LONG_MAX__ - 1) / -39) != 0;
    failures += garply((-__LONG_MAX__ - 1) / -39 + 1) != 1;
    failures += garply(-__LONG_MAX__ - 1) != 1;
    failures += grault(__INT_MAX__ / -46) != 0;
    failures += grault(__INT_MAX__ / -46 - 1) != 1;
    failures += grault(__INT_MAX__) != 1;
    failures += grault((-__INT_MAX__ - 1) / -46) != 0;
    failures += grault((-__INT_MAX__ - 1) / -46 + 1) != 1;
    failures += grault(-__INT_MAX__ - 1) != 1;
    failures += waldo(__LONG_MAX__ / -46) != 0;
    failures += waldo(__LONG_MAX__ / -46 - 1) != 1;
    failures += waldo(__LONG_MAX__) != 1;
    failures += waldo((-__LONG_MAX__ - 1) / -46) != 0;
    failures += waldo((-__LONG_MAX__ - 1) / -46 + 1) != 1;
    failures += waldo(-__LONG_MAX__ - 1) != 1;
    return failures == 0 ? 42 : failures;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn wide_string_literal_subscript_uses_wide_element_type() {
    let src = temp_file("wide-string-literal-subscript", "c");
    let exe = temp_file("wide-string-literal-subscript", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    return L"a" "b"[1] == L'b' ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn scalar_vector_compat_subscript_reads_constant_lanes() {
    let src = temp_file("scalar-vector-compat-subscript", "c");
    let exe = temp_file("scalar-vector-compat-subscript", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned __attribute__((__vector_size__ (8))) V;

int main(void) {
    V x = 0;
    if (x[0] || x[1]) {
        return 1;
    }
    x = 42;
    return x[0] == 42 && x[1] == 0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_two_argument_builtin_shuffle_for_vectors() {
    let src = temp_file("vector-builtin-shuffle", "c");
    let exe = temp_file("vector-builtin-shuffle", "bin");
    std::fs::write(
        &src,
        r#"
typedef double V __attribute__((__vector_size__ (16)));
typedef long long W __attribute__((__vector_size__ (16)));

int main(void) {
    V y = { 1.0, 2.0 };
    W mask = { 10000000001LL, 0LL };
    V r = __builtin_shuffle(y, mask);
    return r[0] == 2.0 && r[1] == 1.0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_static_vector_array_initializers() {
    let src = temp_file("static-vector-array-init", "c");
    let exe = temp_file("static-vector-array-init", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned int V __attribute__((__vector_size__ (16)));
V values[] = { (V){ 1U, 2U, 3U, 4U }, (V){ 9U, 8U, 7U, 6U } };

int main(void) {
    return values[0][2] == 3U && values[1][3] == 6U ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_gnu_vector_typedef_function_return_and_argument_storage() {
    let src = temp_file("gnu-vector-typedef-call-storage", "c");
    let exe = temp_file("gnu-vector-typedef-call-storage", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned char v4qi __attribute__((vector_size(4)));

v4qi half(v4qi v) {
    return v / 2;
}

int main(void) {
    v4qi x = { 5, 5, 5, 5 };
    v4qi y = half(x);
    return y[0] == 2 && y[1] == 2 && y[2] == 2 && y[3] == 2 ? 42 : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_file_scope_gnu_vectors_larger_than_scalar_storage() {
    let src = temp_file("file-scope-large-gnu-vector", "c");
    let exe = temp_file("file-scope-large-gnu-vector", "bin");
    std::fs::write(
        &src,
        r#"
typedef int V __attribute__((vector_size(8 * sizeof(int))));
V a, b, d, expected;

void fill(void) {
    d = a ^ b;
}

int main(void) {
    a = (V){ 1, 2, 3, 4, 5, 6, 7, 8 };
    b = (V){ 0x40, 0x80, 0x40, 0x80, 0x40, 0x80, 0x40, 0x80 };
    expected = (V){ 0x41, 0x82, 0x43, 0x84, 0x45, 0x86, 0x47, 0x88 };
    fill();
    return __builtin_memcmp(&d, &expected, sizeof(V)) == 0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_aligned_nested_struct_array_member_stride() {
    let src = temp_file("aligned-nested-struct-array-member", "c");
    let exe = temp_file("aligned-nested-struct-array-member", "bin");
    std::fs::write(
        &src,
        r#"
void foo(int size) {
    struct S {
        __attribute__((aligned(16))) struct T { short c; } a[size];
        int b[size];
    } s;

    for (int i = 0; i < size; i++) {
        s.a[i].c = 0x1234;
    }
    for (int i = 0; i < size; i++) {
        s.b[i] = 0;
    }
    for (int i = 0; i < size; i++) {
        if (s.a[i].c != 0x1234 || s.b[i] != 0) {
            __builtin_abort();
        }
    }
}

int main(void) {
    foo(15);
    return 42;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_gcc_builtin_setjmp_longjmp_buffer() {
    let src = temp_file("builtin-setjmp-longjmp-buffer", "c");
    let exe = temp_file("builtin-setjmp-longjmp-buffer", "bin");
    std::fs::write(
        &src,
        r#"
void escape(void *p) {
    __builtin_longjmp(p, 1);
}

int main(void) {
    void *buf[5];
    int x = 7;
    if (!__builtin_setjmp(buf)) {
        x = 42;
        escape(buf);
    }
    return x == 42 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_copied_builtin_setjmp_buffer_with_zero_size_alloca() {
    let src = temp_file("copied-setjmp-zero-alloca", "c");
    let exe = temp_file("copied-setjmp-zero-alloca", "bin");
    std::fs::write(
        &src,
        r#"
int x;
char *p;
char *q;

void escape(void *src) {
    void *buf[32];
    __builtin_memcpy(buf, src, 5 * sizeof(void *));
    __builtin_longjmp(buf, 1);
}

int main(void) {
    void *buf[5];
    p = __builtin_alloca(x);
    q = __builtin_alloca(x);
    if (!__builtin_setjmp(buf)) {
        escape(buf);
    }
    p = q + (q - p);
    return p == __builtin_alloca(x) ? 42 : 1;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn gnu_vector_comparisons_produce_all_bits_set_lanes() {
    let src = temp_file("gnu-vector-comparison-mask", "c");
    let exe = temp_file("gnu-vector-comparison-mask", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned long __attribute__((__vector_size__ (8))) V;

int main(void) {
    V v = ~((V) { } <= 0);
    if (v[0]) {
        return 1;
    }
    v = ((V) { 5 } != 0);
    return v[0] == ~0ul ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn uninitialized_automatic_arrays_do_not_emit_zero_fill() {
    let src = temp_file("uninit-auto-array-no-zero-fill", "c");
    std::fs::write(
        &src,
        r#"
int main(void) {
    char buf[4096];
    buf[0] = 42;
    return buf[0];
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("tacky")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let tacky = stdout(output);
    let zero_fills = tacky.matches("CopyToOffset").count();
    assert!(zero_fills <= 2, "{zero_fills} unexpected zero-fill stores");

    let _ = std::fs::remove_file(src);
}

#[test]
fn finstrument_functions_emits_profile_hooks_and_honors_no_instrument_function() {
    let src = temp_file("instrument-functions", "c");
    let exe = temp_file("instrument-functions", "bin");
    std::fs::write(
        &src,
        r#"
int enter_count;
int exit_count;
void *last_entered;
void *last_exited;

void __cyg_profile_func_enter(void *fn, void *parent) __attribute__((no_instrument_function));
void __cyg_profile_func_exit(void *fn, void *parent) __attribute__((no_instrument_function));
int main(void) __attribute__((no_instrument_function));

void target(void) {
    if (last_entered != target) {
        __builtin_abort();
    }
}

int main(void) {
    target();
    return enter_count == 1 && exit_count == 1 && last_exited == target ? 42 : 1;
}

void __cyg_profile_func_enter(void *fn, void *parent) {
    enter_count++;
    last_entered = fn;
}

void __cyg_profile_func_exit(void *fn, void *parent) {
    exit_count++;
    last_exited = fn;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-finstrument-functions")
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn main_falling_off_end_returns_zero_without_missing_return_warning() {
    let src = temp_file("main-fallthrough-zero", "c");
    let exe = temp_file("main-fallthrough-zero", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int value = 42;
    value = value + 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    let success = output.status.success();
    let err = stderr(output);
    assert!(success, "{err}");
    assert!(!err.contains("may exit without returning a value"), "{err}");
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(0));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_brace_elided_local_struct_array_member_initializers() {
    let src = temp_file("brace-elided-struct-array-member-init", "c");
    let exe = temp_file("brace-elided-struct-array-member-init", "bin");
    std::fs::write(
        &src,
        r#"
struct S { unsigned u[4]; };

int main(void) {
    struct S a[] = {
        { 1U, 2U, 3U, 4U },
        { 5U, 6U, 7U, 8U }
    };
    return a[0].u[0] == 1U && a[0].u[3] == 4U
        && a[1].u[0] == 5U && a[1].u[3] == 8U ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn same_size_gnu_vector_casts_reinterpret_bits() {
    let src = temp_file("gnu-vector-bitcast", "c");
    let exe = temp_file("gnu-vector-bitcast", "bin");
    std::fs::write(
        &src,
        r#"
typedef long long I __attribute__((vector_size(16)));
typedef double D __attribute__((vector_size(16)));

int main(void) {
    double out[2];
    I bits = (I)(D){ 2.0, 3.0 };
    *(I *)out = bits;
    return out[0] == 2.0 && out[1] == 3.0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_int128_va_arg_values() {
    let src = temp_file("int128-va-arg", "c");
    let exe = temp_file("int128-va-arg", "bin");
    std::fs::write(
        &src,
        r#"
#include <stdarg.h>

__int128 take(int skip, ...) {
    va_list ap;
    va_start(ap, skip);
    while (skip--) {
        va_arg(ap, int);
    }
    __int128 value = va_arg(ap, __int128);
    va_end(ap);
    return value;
}

int main(void) {
    __int128 value = ((__int128)0x1122334455667788LL << 64) | 0x0102030405060708LL;
    return take(1, 0, value) == value ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn dynamic_alloca_does_not_alias_frame_address() {
    let src = temp_file("dynamic-alloca-frame-safe", "c");
    let exe = temp_file("dynamic-alloca-frame-safe", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    for (int n = 1; n < 5000; n++) {
        int *x = __builtin_alloca((n % 64 + 1) * sizeof(int));
        x[0] = 1;
        x[n % 64] = 2;
    }
    return 42;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn array_arrow_member_type_decays_for_nested_subscript_arrow() {
    let src = temp_file("array-arrow-subscript-arrow", "c");
    let exe = temp_file("array-arrow-subscript-arrow", "bin");
    std::fs::write(
        &src,
        r#"
typedef struct A {
    int a, b;
} A;

typedef struct B {
    A **a;
    int b;
} B;

A *slot;
B d[1];

int main(void) {
    A value = { 1, 2 };
    slot = &value;
    d->a = &slot;
    d->b = 0;
    d->a[d->b]->a++;
    return value.a == 2 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn bitfield_precision_does_not_overconstrain_followup_arithmetic() {
    let src = temp_file("bitfield-precision-followup-arithmetic", "c");
    let exe = temp_file("bitfield-precision-followup-arithmetic", "bin");
    std::fs::write(
        &src,
        r#"
struct S {
    unsigned long long a:2;
    unsigned long long b:40;
    unsigned long long c:22;
};

int main(void) {
    struct S s = {1, 2, 3};
    unsigned long long value = ((unsigned long long)(s.b - 8)) + 8;
    return value == 0x10000000002ULL ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn empty_inline_asm_evaluates_simple_output_operand_side_effects() {
    let src = temp_file("empty-asm-output-side-effect", "c");
    let exe = temp_file("empty-asm-output-side-effect", "bin");
    std::fs::write(
        &src,
        r#"
int count;
int dummy;

int *bar(void) {
    count++;
    return &dummy;
}

int main(void) {
    asm("" : "+r"(*bar()));
    return count == 1 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn empty_inline_asm_tied_input_copies_simple_value_to_output() {
    let src = temp_file("empty-asm-tied-input-copy", "c");
    let exe = temp_file("empty-asm-tied-input-copy", "bin");
    std::fs::write(
        &src,
        r#"
void copy_low_int(long long x, volatile int *p) {
    int i;
    asm("" : "=r"(i) : "0"(x));
    *p = i;
}

int main(void) {
    volatile int i = 0;
    copy_low_int(-2147483647LL, &i);
    return i == -2147483647 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn supports_int128_arithmetic_matrix() {
    let src = TempPath::new("int128-arithmetic-matrix", "c");
    let exe = TempPath::new("int128-arithmetic-matrix", "bin");
    std::fs::write(
        src.path(),
        r#"
typedef unsigned __int128 u128;
typedef __int128 i128;

static unsigned long lo(u128 value) { return (unsigned long)value; }
static unsigned long hi(u128 value) { return (unsigned long)(value >> 64); }
static u128 shl(u128 value, unsigned int amount) { return value << amount; }
static u128 shr(u128 value, unsigned int amount) { return value >> amount; }
static i128 sar(i128 value, unsigned int amount) { return value >> amount; }

int main(void) {
    volatile unsigned int one = 1;
    volatile unsigned int sixty_three = 63;
    volatile unsigned int sixty_five = 65;
    volatile unsigned int ninety_six = 96;

    u128 base = ((u128)0x123456789abcdef0UL << 64) | 0xfedcba9876543210UL;
    if (lo(base + 5) != 0xfedcba9876543215UL) return 1;
    if (lo(base - 0x10) != 0xfedcba9876543200UL) return 2;
    if (lo((u128)0xffffffffffffffffUL + 1) != 0) return 3;
    if (hi((u128)0xffffffffffffffffUL + 1) != 1) return 4;
    if (lo(((u128)1 << 64) - 1) != 0xffffffffffffffffUL) return 5;
    if (hi(((u128)1 << 64) - 1) != 0) return 6;
    if (lo(((u128)3 << 64) * 7) != 0) return 7;
    if (hi(((u128)3 << 64) * 7) != 21) return 8;
    if (((base ^ (u128)0xff) & (u128)0xff) != 0xef) return 9;
    if (!(base > (u128)0xffffffffffffffffUL)) return 10;
    if (!(base != (base + 1))) return 11;

    u128 shifted = shl((u128)1, sixty_five);
    if (lo(shifted) != 0 || hi(shifted) != 2) return 12;
    shifted = shl((u128)3, sixty_three);
    if (lo(shifted) != 0x8000000000000000UL || hi(shifted) != 1) return 13;
    shifted = shr(((u128)1 << 127) | 7, sixty_five);
    if (lo(shifted) != 0x4000000000000000UL || hi(shifted) != 0) return 14;
    shifted = shr(((u128)1 << 127) | ((u128)1 << 96), ninety_six);
    if (lo(shifted) != 0x80000001UL || hi(shifted) != 0) return 15;

    i128 neg = -((i128)1 << 100);
    i128 ar = sar(neg, sixty_five);
    if (ar != -((i128)1 << 35)) return 16;
    if (sar((i128)-2, one) != -1) return 17;
    if ((i128)(int)-7 != -7) return 18;
    if ((u128)(unsigned int)0xffffffffU != 0xffffffffU) return 19;
    return 42;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(exe.path())
        .arg(src.path())
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let status = Command::new(exe.path())
        .status()
        .expect("failed to run exe");
    assert_eq!(status.code(), Some(42));
}

#[test]
fn vector_comparison_produces_full_width_mask_for_uint128_lane() {
    let src = temp_file("vector-uint128-comparison-mask", "c");
    let exe = temp_file("vector-uint128-comparison-mask", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned __int128 V __attribute__((vector_size(16)));

int main(void) {
    V r = (V){5} != 0;
    return r[0] == ~(unsigned __int128)0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn sub_overflow_writes_vector_lane_destination() {
    let src = temp_file("sub-overflow-vector-lane", "c");
    let exe = temp_file("sub-overflow-vector-lane", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned V __attribute__((vector_size(64)));

int main(void) {
    V x = {0};
    __builtin_sub_overflow(0, 6, &x[5]);
    return x[5] == (unsigned)-6 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn same_size_vector_scalar_casts_bitcast_storage() {
    let src = temp_file("vector-scalar-bitcast", "c");
    let exe = temp_file("vector-scalar-bitcast", "bin");
    std::fs::write(
        &src,
        r#"
typedef int V2SI __attribute__((vector_size(8)));
typedef unsigned int V2USI __attribute__((vector_size(8)));

long long to_scalar(V2SI x) {
    return (long long)x;
}

V2USI to_vector(V2SI x) {
    return (V2USI)(V2SI)(long long)x;
}

int main(void) {
    union { V2SI v; V2USI u; long long l; int i[2]; } x;
    x.v = (V2SI){ -3, -3 };
    x.l = to_scalar(x.v);
    if (x.i[0] != -3 || x.i[1] != -3) return 1;
    x.u = to_vector(x.v);
    return x.i[0] == -3 && x.i[1] == -3 ? 42 : 2;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn vector_compound_assignment_updates_union_member_lane_wise() {
    let src = temp_file("vector-compound-union-member", "c");
    let exe = temp_file("vector-compound-union-member", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned short V4HI __attribute__((vector_size(8)));

union U {
    V4HI v;
    short s[4];
} u;

int main(void) {
    u.v += (V4HI){ 12, 32768 };
    u.v += (V4HI){ 12, 32768 };
    return u.s[0] == 24 && u.s[1] == 0 && u.s[2] == 0 && u.s[3] == 0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn brace_elided_nested_local_array_initializer_fills_inner_array() {
    let src = temp_file("brace-elided-nested-local-array", "c");
    let exe = temp_file("brace-elided-nested-local-array", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int a[1][4] = { 11, 12, 13, 14 };
    int (*p)[4] = a;
    int sum = 0;
    for (int i = 0; i < 1; i++) {
        for (int j = 0; j < 4; j++) {
            sum += *(*(p + i) + j);
        }
    }
    return sum == 50 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn sizeof_vla_typedef_uses_runtime_bound_expression() {
    let src = temp_file("sizeof-vla-typedef-runtime-bound", "c");
    let exe = temp_file("sizeof-vla-typedef-runtime-bound", "bin");
    std::fs::write(
        &src,
        r#"
int array_size(int n) {
    typedef int T[n + 2];
    return sizeof(T);
}

int struct_size(int n) {
    typedef struct { int c[n + 2]; } T;
    return sizeof(T);
}

int direct_struct_size(int n) {
    struct S { char b[n]; } __attribute__((packed));
    n++;
    return sizeof(struct S);
}

int main(void) {
    return array_size(20) == 22 * sizeof(int)
        && struct_size(20) == 22 * sizeof(int)
        && direct_struct_size(123) == 123 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn sizeof_local_vla_uses_captured_runtime_bound() {
    let src = temp_file("sizeof-local-vla-runtime-bound", "c");
    let exe = temp_file("sizeof-local-vla-runtime-bound", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int n = 1;
    int a[2][n];
    n++;
    int b[2][n];
    return sizeof(a) == 2 * 1 * sizeof(int)
        && sizeof(b) == 2 * 2 * sizeof(int)
        && n == 2 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn struct_compound_literal_member_can_copy_struct_expression() {
    let src = temp_file("struct-compound-member-copy", "c");
    let exe = temp_file("struct-compound-member-copy", "bin");
    std::fs::write(
        &src,
        r#"
struct T { int t; int r[2]; };
struct S { int a; int b; int c[2]; struct T d; };

void foo(struct S *s) {
    *s = (struct S){ s->b, s->a, { 0, 0 }, s->d };
}

int main(void) {
    struct S s = { 6, 12, { 1, 2 }, { 7, { 8, 9 } } };
    foo(&s);
    return s.a == 12 && s.b == 6 && s.c[0] == 0 && s.c[1] == 0
        && s.d.t == 7 && s.d.r[0] == 8 && s.d.r[1] == 9 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn struct_assignment_expression_yields_assigned_struct_value() {
    let src = temp_file("struct-assignment-expression-value", "c");
    let exe = temp_file("struct-assignment-expression-value", "bin");
    std::fs::write(
        &src,
        r#"
struct S { char w[8]; };

struct S f(struct S *p) {
    struct S a;
    a = ({ struct S b; b = p[1]; p[2] = b; });
    return a;
}

int main(void) {
    struct S p[3] = { "abcdefg", "zyxwvut", "ABCDEFG" };
    struct S a = f(p);
    return a.w[0] == 'z' && p[2].w[0] == 'z' ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn enum_typedef_bitfield_preserves_nonnegative_enumerator_values() {
    let src = temp_file("enum-typedef-bitfield-nonnegative", "c");
    let exe = temp_file("enum-typedef-bitfield-nonnegative", "bin");
    std::fs::write(
        &src,
        r#"
typedef enum { ZERO, ONE, TWO, THREE } E;

struct S {
    E value : 2;
};

int main(void) {
    struct S s;
    s.value = THREE;
    return s.value == THREE ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn unsigned_long_long_bitfield_shift_keeps_declared_precision() {
    let src = temp_file("bitfield-shift-precision", "c");
    let exe = temp_file("bitfield-shift-precision", "bin");
    std::fs::write(
        &src,
        r#"
struct S {
    unsigned long long b : 40;
} x;

int main(void) {
    x.b = 0x0100ULL;
    if ((x.b << 32) != 0) return 1;
    x.b = 0x0100000001ULL;
    return ((x.b << 8) + (x.b >> 32)) == 0x0000000101ULL ? 42 : 2;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn builtin_isinf_detects_overflowed_float_and_double_values() {
    let src = temp_file("builtin-isinf-overflow", "c");
    let exe = temp_file("builtin-isinf-overflow", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    float f = 1.01f * __FLT_MAX__;
    double d = 1.01 * __DBL_MAX__;
    return __builtin_isinff(f) && __builtin_isinf(d) ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn builtin_signbit_detects_negative_zero() {
    let src = temp_file("builtin-signbit-negative-zero", "c");
    let exe = temp_file("builtin-signbit-negative-zero", "bin");
    std::fs::write(
        &src,
        r#"
double not_fabs(double x) {
    return x >= 0.0 ? x : -x;
}

int main(void) {
    double x = -0.0;
    double y = not_fabs(x);
    return __builtin_signbit(y) ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn unsigned_small_integer_return_values_are_abi_extended_by_caller() {
    let src = temp_file("unsigned-small-return-abi", "c");
    let exe = temp_file("unsigned-small-return-abi", "bin");
    std::fs::write(
        &src,
        r#"
static int i(int x) { return x; }
__attribute__((noinline)) unsigned int ui(int x) { return i(x + 6); }

static signed char sc(int x) { return x; }
__attribute__((noinline)) unsigned char uc(int x) { return sc(x + 6); }

int main(void) {
    if ((unsigned long)ui(-10) != 0xfffffffcUL) {
        return 1;
    }
    if ((unsigned long)uc(-10) != 0xfcUL) {
        return 2;
    }
    return 42;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn static_unsigned_cast_initializer_truncates_before_widening() {
    let src = temp_file("static-unsigned-cast-widen", "c");
    let exe = temp_file("static-unsigned-cast-widen", "bin");
    std::fs::write(
        &src,
        r#"
volatile unsigned long from_uint = (unsigned int)-4;
volatile unsigned long from_uchar = (unsigned char)-4;

int main(void) {
    if (from_uint != 0xfffffffcUL) {
        return 1;
    }
    if (from_uchar != 0xfcUL) {
        return 2;
    }
    return 42;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn static_designated_bitfield_initializer_preserves_bit_offset() {
    let src = temp_file("static-designated-bitfield-offset", "c");
    let exe = temp_file("static-designated-bitfield-offset", "bin");
    std::fs::write(
        &src,
        r#"
static struct {
    unsigned int : 1;
    unsigned int s : 1;
} value = { .s = 1 };

int main(void) {
    return value.s == 1 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn folded_unsigned_bitwise_comparison_uses_unsigned_ordering() {
    let src = temp_file("folded-unsigned-bitwise-comparison", "c");
    let exe = temp_file("folded-unsigned-bitwise-comparison", "bin");
    std::fs::write(
        &src,
        r#"
static int y = 0x8000;

int main(void) {
    unsigned int x = (short)y;
    if (0LL > (0U ^ (short)-0x8000)) {
        return 1;
    }
    if (0LL > (0U ^ x)) {
        return 2;
    }
    if ((0U ^ (short)y) < 0LL) {
        return 3;
    }
    return 42;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn scalar_initializer_for_union_member_initializes_first_union_field() {
    let src = temp_file("scalar-union-member-initializer", "c");
    let exe = temp_file("scalar-union-member-initializer", "bin");
    std::fs::write(
        &src,
        r#"
typedef union {
    int lock;
} mutex_t;

int main(void) {
    struct { int c; mutex_t m; } r = { .m = 0 };
    return r.c == 0 && r.m.lock == 0 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn large_struct_return_from_deref_copies_pointee_value() {
    let src = temp_file("large-struct-return-deref-copy", "c");
    let exe = temp_file("large-struct-return-deref-copy", "bin");
    std::fs::write(
        &src,
        r#"
struct s {
    unsigned char a[256];
};

static struct s source;
static struct s *p = &source;

static struct s ret(void) {
    return *p;
}

int main(void) {
    source.a[7] = 99;
    struct s copy = ret();
    return copy.a[7] == 99 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn builtin_offsetof_accepts_vla_member_index_expressions() {
    let src = temp_file("offsetof-vla-member-index-expression", "c");
    let exe = temp_file("offsetof-vla-member-index-expression", "bin");
    std::fs::write(
        &src,
        r#"
long foo(int n, int i, int j) {
    typedef int T[n];
    struct S { int a; T b[n]; };
    return __builtin_offsetof(struct S, b[i][j]);
}

int main(void) {
    typedef int T[5];
    struct S { int a; T b[5]; };
    long expected = __builtin_offsetof(struct S, b) + (5 * 2 + 3) * sizeof(int);
    return foo(5, 2, 3) == expected ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn sizeof_vla_parameter_subscript_uses_captured_entry_bound() {
    let src = temp_file("sizeof-vla-param-subscript-bound", "c");
    let exe = temp_file("sizeof-vla-param-subscript-bound", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int n = 1;
    int first = 0;
    int second = 0;
    int foo(char a[2][++n]) {
        n += 4;
        return sizeof(a[0]);
    }
    first = foo(0);
    second = foo(0);
    return first == 2 && second == 7 && n == 11 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn driver_accepts_fsyntax_only_without_writing_output() {
    let src = temp_file("syntax-only", "c");
    let out = temp_file("syntax-only", "s");
    std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-fsyntax-only")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(!out.exists());

    let _ = std::fs::remove_file(src);
}

#[test]
fn supports_nested_function_local_label_addresses() {
    let src = temp_file("nested-local-label-address", "c");
    let exe = temp_file("nested-local-label-address", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    void *label = &&out;
    int i = 0;
    void test(void) {
        label = &&out2;
        goto *label;
out2:
        i++;
    }
    goto *label;
out:
    i += 2;
    test();
    return i == 3 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn emits_x86_64_linux_assembly_for_ci_regression_cases() {
    for (name, source) in [
        (
            "x86-linux-int128-cross-half-shift",
            r#"
unsigned long f(unsigned __int128 in1, unsigned long in2) {
    __int128 mask = (__int128)0xffff << 56;
    return ((in1 & mask) >> 56) | in2;
}

int main(void) {
    unsigned __int128 in = 1;
    in <<= 64;
    return f(in, 2) == 0x102 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-int128-va-arg",
            r#"
#include <stdarg.h>

__int128 take(int skip, ...) {
    va_list ap;
    va_start(ap, skip);
    while (skip--) {
        va_arg(ap, int);
    }
    __int128 value = va_arg(ap, __int128);
    va_end(ap);
    return value;
}

int main(void) {
    __int128 value = ((__int128)0x1122334455667788LL << 64) | 0x0102030405060708LL;
    return take(1, 0, value) == value ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-vector-uint128-mask",
            r#"
typedef unsigned __int128 V __attribute__((vector_size(16)));

int main(void) {
    V r = (V){5} != 0;
    return r[0] == ~(unsigned __int128)0 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-i128-eq-low-half",
            r#"
int main(void) {
    unsigned __int128 a = ((unsigned __int128)1 << 64) | 5;
    unsigned __int128 b = ((unsigned __int128)1 << 64) | 6;
    return a != b ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-i128-signed-unsigned-stress",
            r#"
int main(void) {
    __int128 minus_one = -1;
    unsigned __int128 high = (unsigned __int128)1 << 127;
    unsigned __int128 shifted = (high >> 64) << 32;
    __int128 product = ((__int128)-3037000499LL) * 3037000499LL;
    if (!(minus_one < 0)) return 1;
    if (!(high > (unsigned __int128)0xffffffffffffffffULL)) return 2;
    if ((unsigned long)(shifted >> 32) != 0x8000000000000000ULL) return 3;
    if (!(product < 0)) return 4;
    if ((__int128)(int)-7 != -7) return 5;
    if ((unsigned __int128)(unsigned int)0xffffffffU != 0xffffffffU) return 6;
    return 42;
}
"#,
        ),
        (
            "x86-linux-i128-vector-lane-stress",
            r#"
typedef unsigned __int128 U1 __attribute__((vector_size(16)));
typedef __int128 I1 __attribute__((vector_size(16)));

int main(void) {
    U1 u = (U1){ ((unsigned __int128)1 << 96) | 7 };
    I1 i = (I1){ -5 };
    U1 mask = u != 0;
    if (mask[0] != ~(unsigned __int128)0) return 1;
    if ((u[0] >> 96) != 1) return 2;
    if (!(i[0] < 0)) return 3;
    return 42;
}
"#,
        ),
        (
            "x86-linux-stack-copy-regalloc",
            r#"
struct inner { int a; double b; };
union choice { struct inner i; long raw[2]; };
struct outer { union choice c; int tail; };

int use(struct outer o) {
    return o.c.i.a + (int)o.c.i.b + o.tail;
}

int main(void) {
    struct outer o;
    o.c.i.a = 10;
    o.c.i.b = 20.0;
    o.tail = 12;
    return use(o);
}
"#,
        ),
        (
            "x86-linux-nested-local-label",
            r#"
int main(void) {
    void *label = &&out;
    int i = 0;
    void test(void) {
        label = &&out2;
        goto *label;
out2:
        i++;
    }
    goto *label;
out:
    i += 2;
    test();
    return i == 3 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-i128-label-uniqueness",
            r#"
int f(__int128 x) { return x > 1 && x < 3; }
int g(__int128 x) { return x > 4 && x < 6; }
int main(void) { return f(2) && g(5) ? 42 : 1; }
"#,
        ),
        (
            "x86-linux-signed-long-mul-overflow",
            r#"
int overflows;

long test(long *x, int y) {
    long s = 1;
    for (int i = 0; i < y; i++) {
        if (__builtin_mul_overflow(s, x[i], &s)) {
            overflows++;
        }
    }
    return s;
}

int main(void) {
    long d[7] = { 975, 975, 975, 975, 975, 975, 975 };
    test(d, 7);
    return overflows == 1 ? 42 : overflows;
}
"#,
        ),
    ] {
        let src = TempPath::new(name, "c");
        let out = TempPath::new(name, "s");
        std::fs::write(src.path(), source).expect("failed to write input");

        let output = Command::new(rnqcc())
            .args(["--target", "x86_64-linux", "-S", "-o"])
            .arg(out.path())
            .arg(src.path())
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{name}: {}", stderr(output));
        let asm = std::fs::read_to_string(out.path()).expect("failed to read assembly output");
        assert!(!asm.contains("Pseudo-register"), "{name}: {asm}");
        assert!(!asm.contains("Octword"), "{name}: {asm}");
        if name == "x86-linux-int128-va-arg" {
            assert!(asm.contains("subq $32, %rsp"), "{name}: {asm}");
            assert!(asm.contains("8(%rsp)"), "{name}: {asm}");
            assert!(asm.contains("16(%rsp)"), "{name}: {asm}");
            assert!(asm.contains("addq $32, %rsp"), "{name}: {asm}");
        }
        if name == "x86-linux-vector-uint128-mask" || name == "x86-linux-i128-eq-low-half" {
            assert!(
                !asm.contains("\tje .Li128_cmp_end") || !asm.contains(".Li128_cmp_low"),
                "{name}: equality compare skipped low-half comparison: {asm}"
            );
        }
        if name == "x86-linux-stack-copy-regalloc" {
            assert!(asm.contains("rep movsb"), "{name}: {asm}");
            assert!(
                !asm.contains("movq -88(%rbp), %rsi"),
                "{name}: stack copy used stale spill slot instead of computed source pointer: {asm}"
            );
        }
        let mut labels = std::collections::HashSet::new();
        for line in asm.lines() {
            let trimmed = line.trim();
            if let Some(label) = trimmed.strip_suffix(':') {
                assert!(
                    labels.insert(label.to_string()),
                    "{name}: duplicate {label}"
                );
            }
        }
    }
}

#[test]
fn supports_nested_function_nonlocal_goto_to_parent_label() {
    let src = temp_file("nested-nonlocal-goto", "c");
    let exe = temp_file("nested-nonlocal-goto", "bin");
    std::fs::write(
        &src,
        r#"
void *ptr;

int main(void) {
    __label__ nonlocal_lab;
    void bar(void *func) {
        ptr = func;
        goto nonlocal_lab;
    }
    bar(&&nonlocal_lab);
    return 1;
nonlocal_lab:
    return ptr == &&nonlocal_lab ? 42 : 2;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}
