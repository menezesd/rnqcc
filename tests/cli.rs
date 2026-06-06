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
fn fuzz_smoke_script_passes_extra_rnqcc_args() -> Result<(), String> {
    let output = match Command::new("python3")
        .args([
            "scripts/fuzz_smoke.py",
            "--seed",
            "19",
            "--cases",
            "1",
            "--rnqcc",
            rnqcc(),
            "--target",
            "x86_64-linux",
            "--rnqcc-arg=--optimize",
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
fn fuzz_smoke_script_compares_runtime_with_reference_cc() -> Result<(), String> {
    let output = match Command::new("python3")
        .args([
            "scripts/fuzz_smoke.py",
            "--seed",
            "23",
            "--cases",
            "1",
            "--rnqcc",
            rnqcc(),
            "--target",
            "x86_64-linux",
            "--compare-runtime",
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

#[cfg(unix)]
#[test]
fn fuzz_smoke_script_reports_runtime_mismatches() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let fake_rnqcc = TempPath::new("fuzz-smoke-runtime-mismatch-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        "#!/bin/sh\n\
         out=\n\
         while [ \"$#\" -gt 0 ]; do\n\
           if [ \"$1\" = \"-o\" ]; then\n\
             shift\n\
             out=$1\n\
           fi\n\
           shift\n\
         done\n\
         if [ -z \"$out\" ]; then\n\
           exit 0\n\
         fi\n\
         cat > \"$out\" <<'EOF'\n\
#!/bin/sh\n\
exit 99\n\
EOF\n\
         chmod +x \"$out\"\n",
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/fuzz_smoke.py")
        .arg("--seed")
        .arg("23")
        .arg("--cases")
        .arg("1")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--target")
        .arg("x86_64-linux")
        .arg("--compare-runtime")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run fuzz smoke script: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("runtime mismatch:"), "{stderr}");
    assert!(stderr.contains("rnqcc exited 99"), "{stderr}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn fuzz_smoke_reports_timeouts_cleanly() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let fake_rnqcc = TempPath::new("fuzz-smoke-timeout-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        "#!/bin/sh\n\
         printf 'partial stdout without newline'\n\
         printf 'partial stderr without newline' >&2\n\
         sleep 5\n",
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/fuzz_smoke.py")
        .arg("--seed")
        .arg("17")
        .arg("--cases")
        .arg("1")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--target")
        .arg("x86_64-linux")
        .arg("--timeout")
        .arg("0.1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run fuzz smoke script: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("FAIL seed=17 case=0"), "{stderr}");
    assert!(stderr.contains("timed out after 0.1s"), "{stderr}");
    assert!(!stderr.contains("None"), "{stderr}");
    Ok(())
}

#[test]
fn fuzz_smoke_rejects_nonpositive_case_count() -> Result<(), String> {
    let output = match Command::new("python3")
        .arg("scripts/fuzz_smoke.py")
        .arg("--seed")
        .arg("17")
        .arg("--cases")
        .arg("0")
        .arg("--emit-only")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run fuzz smoke script: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("--cases must be positive"), "{stderr}");
    Ok(())
}

#[test]
fn fuzz_smoke_parallel_default_workdirs_do_not_collide() -> Result<(), String> {
    let first = match Command::new("python3")
        .arg("scripts/fuzz_smoke.py")
        .arg("--seed")
        .arg("17")
        .arg("--cases")
        .arg("1")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--target")
        .arg("x86_64-linux")
        .spawn()
    {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to start first fuzz smoke script: {err}")),
    };
    let second = match Command::new("python3")
        .arg("scripts/fuzz_smoke.py")
        .arg("--seed")
        .arg("17")
        .arg("--cases")
        .arg("1")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--target")
        .arg("x86_64-linux")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run second fuzz smoke script: {err}")),
    };
    let first = first
        .wait_with_output()
        .map_err(|err| format!("failed to wait for first fuzz smoke script: {err}"))?;

    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    Ok(())
}

#[test]
fn smoke_scripts_reject_nonnumeric_timeout_env() -> Result<(), String> {
    let cases: [(&str, &str, &[&str]); 3] = [
        (
            "REAL_PROJECT_TIMEOUT",
            "scripts/real_project_corpus.py",
            &[] as &[&str],
        ),
        ("LAYOUT_ORACLE_TIMEOUT", "scripts/layout_oracle.py", &[]),
        (
            "FUZZ_SMOKE_TIMEOUT",
            "scripts/fuzz_smoke.py",
            &["--seed", "17", "--emit-only"],
        ),
    ];

    for (env_name, script, args) in cases {
        let output = match Command::new("python3")
            .arg(script)
            .args(args)
            .env(env_name, "not-a-number")
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run {script}: {err}")),
        };

        assert!(!output.status.success(), "{script} unexpectedly succeeded");
        let stderr = stderr(output);
        assert!(
            stderr.contains(&format!("{env_name} must be a number")),
            "{stderr}"
        );
        assert!(!stderr.contains("Traceback"), "{stderr}");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn llvm_c_regression_smoke_reports_timeouts_cleanly() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let suite = TempPath::new("llvm-c-timeout-suite", "dir");
    std::fs::create_dir_all(suite.path())
        .map_err(|err| format!("failed to create fake LLVM suite: {err}"))?;
    std::fs::write(
        suite.path().join("slow.c"),
        "int main(void) { return 0; }\n",
    )
    .map_err(|err| format!("failed to write fake LLVM source: {err}"))?;

    let fake_rnqcc = TempPath::new("llvm-c-timeout-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        "#!/bin/sh\n\
         printf 'partial stdout without newline'\n\
         printf 'partial stderr without newline' >&2\n\
         sleep 5\n",
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/llvm_c_regression_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--limit")
        .arg("1")
        .arg("--timeout")
        .arg("0.1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run LLVM C smoke script: {err}")),
    };

    assert!(!output.status.success());
    let stdout = stdout(output);
    assert!(stdout.contains("FAIL slow.c:"), "{stdout}");
    assert!(stdout.contains("timed out after 0.1s"), "{stdout}");
    assert!(!stdout.contains("None"), "{stdout}");
    Ok(())
}

#[test]
fn llvm_c_regression_smoke_rejects_invalid_numeric_args() -> Result<(), String> {
    let suite = TempPath::new("llvm-c-invalid-args-suite", "dir");
    std::fs::create_dir_all(suite.path())
        .map_err(|err| format!("failed to create fake LLVM suite: {err}"))?;
    let cases = [
        ("--start", "-1", "--start must be non-negative"),
        ("--limit", "0", "--limit must be positive"),
        ("--timeout", "0", "--timeout must be positive"),
    ];

    for (flag, value, expected) in cases {
        let output = match Command::new("python3")
            .arg("scripts/llvm_c_regression_smoke.py")
            .arg("--rnqcc")
            .arg(rnqcc())
            .arg("--suite")
            .arg(suite.path())
            .arg(flag)
            .arg(value)
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run LLVM C smoke script: {err}")),
        };

        assert!(
            !output.status.success(),
            "{flag} {value} unexpectedly succeeded"
        );
        let stderr = stderr(output);
        assert!(stderr.contains(expected), "{stderr}");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn llvm_c_regression_smoke_passes_extra_rnqcc_args() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let suite = TempPath::new("llvm-c-extra-args-suite", "dir");
    std::fs::create_dir_all(suite.path())
        .map_err(|err| format!("failed to create fake LLVM suite: {err}"))?;
    std::fs::write(
        suite.path().join("extra.c"),
        "int main(void) { return 0; }\n",
    )
    .map_err(|err| format!("failed to write fake LLVM source: {err}"))?;

    let log = TempPath::new("llvm-c-extra-args-log", "txt");
    let fake_rnqcc = TempPath::new("llvm-c-extra-args-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
out=
while [ "$#" -gt 0 ]; do
    if [ "$1" = "-o" ]; then
        shift
        out="$1"
    fi
    shift
done
if [ -z "$out" ]; then
    exit 2
fi
{{
    printf '%s\n' '#!/bin/sh'
    printf '%s\n' 'exit 0'
}} > "$out"
chmod +x "$out"
"#,
            log.path().display()
        ),
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/llvm_c_regression_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--limit")
        .arg("1")
        .arg("--rnqcc-arg=--optimize")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run LLVM C smoke script: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let log = std::fs::read_to_string(log.path())
        .map_err(|err| format!("failed to read fake rnqcc log: {err}"))?;
    assert!(log.contains("--optimize"), "{log}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn real_project_corpus_reports_timeouts_cleanly() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempPath::new("real-project-timeout", "dir");
    std::fs::create_dir_all(dir.path())
        .map_err(|err| format!("failed to create fake corpus dir: {err}"))?;
    let src = dir.path().join("slow.c");
    let manifest = dir.path().join("corpus.txt");
    std::fs::write(&src, "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake corpus source: {err}"))?;
    std::fs::write(&manifest, format!("{}\n", src.display()))
        .map_err(|err| format!("failed to write fake corpus manifest: {err}"))?;

    let fake_rnqcc = TempPath::new("real-project-timeout-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        "#!/bin/sh\n\
         printf 'partial stdout without newline'\n\
         printf 'partial stderr without newline' >&2\n\
         sleep 5\n",
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/real_project_corpus.py")
        .env("RNQCC", fake_rnqcc.path())
        .env("REAL_PROJECT_MANIFEST", &manifest)
        .env("REAL_PROJECT_TIMEOUT", "0.1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run real project corpus script: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("slow.c: unexpected failure"), "{stderr}");
    assert!(stderr.contains("timed out after 0.1s"), "{stderr}");
    assert!(!stderr.contains("None"), "{stderr}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn real_project_corpus_passes_extra_rnqcc_args() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempPath::new("real-project-extra-args", "dir");
    std::fs::create_dir_all(dir.path())
        .map_err(|err| format!("failed to create fake corpus dir: {err}"))?;
    let src = dir.path().join("extra.c");
    let manifest = dir.path().join("corpus.txt");
    std::fs::write(&src, "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake corpus source: {err}"))?;
    std::fs::write(&manifest, format!("{}\n", src.display()))
        .map_err(|err| format!("failed to write fake corpus manifest: {err}"))?;

    let log = TempPath::new("real-project-extra-args-log", "txt");
    let fake_rnqcc = TempPath::new("real-project-extra-args-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> '{}'
out=
while [ "$#" -gt 0 ]; do
    if [ "$1" = "-o" ]; then
        shift
        out="$1"
    fi
    shift
done
if [ -z "$out" ]; then
    exit 2
fi
printf '' > "$out"
"#,
            log.path().display()
        ),
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/real_project_corpus.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--manifest")
        .arg(&manifest)
        .arg("--rnqcc-arg=--optimize")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run real project corpus script: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let log = std::fs::read_to_string(log.path())
        .map_err(|err| format!("failed to read fake rnqcc log: {err}"))?;
    assert!(log.contains("--optimize"), "{log}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn layout_oracle_reports_executable_timeouts_cleanly() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let fake_cc = TempPath::new("layout-timeout-cc", "sh");
    std::fs::write(
        fake_cc.path(),
        r#"#!/bin/sh
out=
while [ "$#" -gt 0 ]; do
    if [ "$1" = "-o" ]; then
        shift
        out="$1"
    fi
    shift
done
if [ -z "$out" ]; then
    exit 2
fi
{
    printf '%s\n' '#!/bin/sh'
    printf '%s\n' 'exit 0'
} > "$out"
chmod +x "$out"
"#,
    )
    .map_err(|err| format!("failed to write fake cc: {err}"))?;
    let mut perms = std::fs::metadata(fake_cc.path())
        .map_err(|err| format!("missing fake cc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_cc.path(), perms)
        .map_err(|err| format!("failed to chmod fake cc: {err}"))?;

    let fake_rnqcc = TempPath::new("layout-timeout-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        r#"#!/bin/sh
out=
while [ "$#" -gt 0 ]; do
    if [ "$1" = "-o" ]; then
        shift
        out="$1"
    fi
    shift
done
if [ -z "$out" ]; then
    exit 2
fi
{
    printf '%s\n' '#!/bin/sh'
    printf '%s\n' "printf 'partial stdout without newline'"
    printf '%s\n' "printf 'partial stderr without newline' >&2"
    printf '%s\n' 'sleep 5'
} > "$out"
chmod +x "$out"
"#,
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/layout_oracle.py")
        .env("CC", fake_cc.path())
        .env("RNQCC", fake_rnqcc.path())
        .env("LAYOUT_ORACLE_TIMEOUT", "0.1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run layout oracle script: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("timed out after 0.1s"), "{stderr}");
    assert!(!stderr.contains("None"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_lists_artifact_xfails_absent_from_fixture() -> Result<(), String> {
    let expected = TempPath::new("gcc-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-xfail-report-failures", "txt");
    std::fs::write(expected.path(), "execute/known.c | exit status -6\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "execute/known.c\tXFAIL: exit status -6\n\
         execute/obsolete.c\tXFAIL: exit status -11\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("xfails absent from fixture: 1"), "{stdout}");
    assert!(
        stdout.contains("execute/obsolete.c | exit status -11"),
        "{stdout}"
    );

    let strict_output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--fail-on-unexpected-xfail")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run strict xfail reporter: {err}")),
    };

    assert!(!strict_output.status.success());
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_strict_mode_fails_on_stale_xfails() -> Result<(), String> {
    let expected = TempPath::new("gcc-stale-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-stale-xfail-report-failures", "txt");
    std::fs::write(expected.path(), "execute/stale.c | exit status -6\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "execute/stale.c\tSTALE-XFAIL: exit status -6\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("stale xfails still in fixture: 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("execute/stale.c | exit status -6"),
        "{stdout}"
    );

    let strict_output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--fail-on-stale")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run strict xfail reporter: {err}")),
    };

    assert!(!strict_output.status.success());
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_strict_mode_fails_on_unmarked_expected() -> Result<(), String> {
    let expected = TempPath::new("gcc-unmarked-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-unmarked-xfail-report-failures", "txt");
    std::fs::write(expected.path(), "execute/raw.c | timed out after 10.0s\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\ttimed out after 10.0s\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("expected failures without xfail marker: 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("execute/raw.c | timed out after 10.0s"),
        "{stdout}"
    );

    let strict_output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--fail-on-unmarked-expected")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run strict xfail reporter: {err}")),
    };

    assert!(!strict_output.status.success());
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_strict_mode_fails_on_absent_expected() -> Result<(), String> {
    let expected = TempPath::new("gcc-absent-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-absent-xfail-report-failures", "txt");
    std::fs::write(expected.path(), "execute/absent.c | exit status -6\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("expected entries absent from artifact: 1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("execute/absent.c | exit status -6"),
        "{stdout}"
    );

    let strict_output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--fail-on-absent-expected")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run strict xfail reporter: {err}")),
    };

    assert!(!strict_output.status.success());
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_truncates_long_display_reasons() -> Result<(), String> {
    let expected = TempPath::new("gcc-long-reason-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-long-reason-xfail-report-failures", "txt");
    let long_reason = format!("{}{}", "diagnostic ".repeat(40), "sentinel-tail");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        format!("execute/noisy.c\tFAIL: {long_reason}\n"),
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stdout = stdout(output);
    assert!(stdout.contains("execute/noisy.c | diagnostic "), "{stdout}");
    assert!(stdout.contains("..."), "{stdout}");
    assert!(!stdout.contains("sentinel-tail"), "{stdout}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn gcc_torture_smoke_marks_expected_timeout_as_xfail() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let suite = TempPath::new("gcc-smoke-timeout-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let expected = TempPath::new("gcc-smoke-timeout-expected", "txt");
    let failure_log = TempPath::new("gcc-smoke-timeout-failures", "txt");
    let artifact_dir = TempPath::new("gcc-smoke-timeout-artifacts", "dir");
    let fake_rnqcc = TempPath::new("gcc-smoke-timeout-rnqcc", "sh");
    std::fs::write(expected.path(), "execute/raw.c | timed out after 0.1s\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        fake_rnqcc.path(),
        "#!/bin/sh\n\
         out=\n\
         while [ \"$#\" -gt 0 ]; do\n\
           if [ \"$1\" = \"-o\" ]; then\n\
             shift\n\
             out=$1\n\
           fi\n\
           shift\n\
         done\n\
         cat > \"$out\" <<'EOF'\n\
#!/bin/sh\n\
printf 'started without newline'\n\
sleep 5\n\
EOF\n\
         chmod +x \"$out\"\n",
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("missing fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--timeout")
        .arg("0.1")
        .arg("--expected-failures")
        .arg(expected.path())
        .arg("--failure-log")
        .arg(failure_log.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("expected_failed=1"), "{stdout}");
    let failures = std::fs::read_to_string(failure_log.path())
        .map_err(|err| format!("failed to read failure log: {err}"))?;
    assert!(
        failures.contains("execute/raw.c\tXFAIL: timed out after 0.1s"),
        "{failures}"
    );
    let source_path = std::fs::read_to_string(
        artifact_dir
            .path()
            .join("gcc_torture")
            .join("xfail")
            .join("0000-raw")
            .join("source-path.txt"),
    )
    .map_err(|err| format!("failed to read artifact source path: {err}"))?;
    assert_eq!(source_path, "execute/raw.c\n");

    Ok(())
}

#[test]
fn gcc_torture_smoke_emits_canonical_skip_paths() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-skip-path-suite", "dir");
    let compile_dir = suite.path().join("compile");
    std::fs::create_dir_all(&compile_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        compile_dir.join("raw.c"),
        "/* { dg-error \"expected diagnostic\" } */\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let skip_log = TempPath::new("gcc-smoke-skip-path-skips", "txt");
    let fake_rnqcc = TempPath::new("gcc-smoke-skip-path-rnqcc", "sh");
    std::fs::write(fake_rnqcc.path(), "#!/bin/sh\nexit 0\n")
        .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("compile")
        .arg("--limit")
        .arg("1")
        .arg("--skip-log")
        .arg(skip_log.path())
        .arg("--print-skips")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("SKIP compile/raw.c: expected-diagnostic GCC torture test"),
        "{stdout}"
    );
    let skips = std::fs::read_to_string(skip_log.path())
        .map_err(|err| format!("failed to read skip log: {err}"))?;
    assert!(
        skips.contains("compile/raw.c\tSKIP: expected-diagnostic GCC torture test"),
        "{skips}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_unexpected_skip_with_expected_skip_fixture() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-unexpected-skip-suite", "dir");
    let compile_dir = suite.path().join("compile");
    std::fs::create_dir_all(&compile_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        compile_dir.join("raw.c"),
        "/* { dg-error \"expected diagnostic\" } */\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let expected_skips = TempPath::new("gcc-smoke-unexpected-skip-expected", "txt");
    std::fs::write(expected_skips.path(), "")
        .map_err(|err| format!("failed to write expected skip fixture: {err}"))?;
    let failure_log = TempPath::new("gcc-smoke-unexpected-skip-failures", "txt");
    let fake_rnqcc = TempPath::new("gcc-smoke-unexpected-skip-rnqcc", "sh");
    std::fs::write(fake_rnqcc.path(), "#!/bin/sh\nexit 0\n")
        .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("compile")
        .arg("--limit")
        .arg("1")
        .arg("--expected-skips")
        .arg(expected_skips.path())
        .arg("--failure-log")
        .arg(failure_log.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success(), "{}", stdout(output));
    let failures = std::fs::read_to_string(failure_log.path())
        .map_err(|err| format!("failed to read failure log: {err}"))?;
    assert!(
        failures.contains("compile/raw.c\tUNEXPECTED-SKIP: expected-diagnostic GCC torture test"),
        "{failures}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_stale_expected_skip() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-stale-skip-suite", "dir");
    let compile_dir = suite.path().join("compile");
    std::fs::create_dir_all(&compile_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(compile_dir.join("raw.c"), "int value;\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let expected_skips = TempPath::new("gcc-smoke-stale-skip-expected", "txt");
    std::fs::write(
        expected_skips.path(),
        "compile | external | compile/raw.c | old skip reason\n",
    )
    .map_err(|err| format!("failed to write expected skip fixture: {err}"))?;
    let failure_log = TempPath::new("gcc-smoke-stale-skip-failures", "txt");

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("compile")
        .arg("--limit")
        .arg("1")
        .arg("--expected-skips")
        .arg(expected_skips.path())
        .arg("--failure-log")
        .arg(failure_log.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success(), "{}", stdout(output));
    let failures = std::fs::read_to_string(failure_log.path())
        .map_err(|err| format!("failed to read failure log: {err}"))?;
    assert!(
        failures.contains("compile/raw.c\tSTALE-SKIP: old skip reason"),
        "{failures}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_skips_internal_cpp_expensive_tests() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-internal-cpp-expensive-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        execute_dir.join("expensive.c"),
        "/* { dg-require-effective-target run_expensive_tests } */\nint main(void) { return 0; }\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let skip_log = TempPath::new("gcc-smoke-internal-cpp-expensive-skips", "txt");
    let fake_rnqcc = TempPath::new("gcc-smoke-internal-cpp-expensive-rnqcc", "sh");
    std::fs::write(fake_rnqcc.path(), "#!/bin/sh\nexit 0\n")
        .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--internal-cpp")
        .arg("--limit")
        .arg("1")
        .arg("--skip-log")
        .arg(skip_log.path())
        .arg("--print-skips")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(
        stdout.contains("SKIP execute/expensive.c: internal-cpp expensive stress test"),
        "{stdout}"
    );
    let skips = std::fs::read_to_string(skip_log.path())
        .map_err(|err| format!("failed to read skip log: {err}"))?;
    assert!(
        skips.contains("execute/expensive.c\tSKIP: internal-cpp expensive stress test"),
        "{skips}"
    );
    Ok(())
}

#[test]
fn gcc_torture_helpers_are_importable_from_repo_root() -> Result<(), String> {
    let output = match Command::new("python3")
        .arg("-c")
        .arg(
            "import scripts.gcc_torture_smoke\n\
             import scripts.report_gcc_torture_xfails\n\
             import scripts.triage_gcc_torture_xfails\n",
        )
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to import GCC torture helpers: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    Ok(())
}

#[test]
fn gcc_torture_smoke_admits_portable_execute_stack_stress() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-stack-stress-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        execute_dir.join("20011008-3.c"),
        "/* { dg-add-options stack_size } */\n\
         int main(void) { return 0; }\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("skipped=0"), "{stdout}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn gcc_torture_smoke_uses_assembly_for_cross_target_compile() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let suite = TempPath::new("gcc-smoke-cross-target-suite", "dir");
    let compile_dir = suite.path().join("compile");
    std::fs::create_dir_all(&compile_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        compile_dir.join("pr88423.c"),
        "/* { dg-do compile { target i?86-*-* x86_64-*-* } } */\n\
         int value;\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let log = TempPath::new("gcc-smoke-cross-target-args", "txt");
    let fake_rnqcc = TempPath::new("gcc-smoke-cross-target-rnqcc", "sh");
    std::fs::write(
        fake_rnqcc.path(),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            log.path().display()
        ),
    )
    .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("failed to stat fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("compile")
        .arg("--limit")
        .arg("1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let args =
        std::fs::read_to_string(log.path()).map_err(|err| format!("failed to read log: {err}"))?;
    assert_contains_in_order(
        &args,
        &[
            "--Wno-missing-return",
            "--target",
            "x86_64-linux",
            "-S",
            "pr88423.c",
            "-o",
        ],
    )?;
    assert!(!args.lines().any(|arg| arg == "-c"), "{args}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn gcc_torture_smoke_requires_verified_warning_output() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let suite = TempPath::new("gcc-smoke-verified-warning-suite", "dir");
    let compile_dir = suite.path().join("compile");
    std::fs::create_dir_all(&compile_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        compile_dir.join("pr103314-1.c"),
        "int main(void) { return 0; } /* { dg-warning \"is negative\" } */\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let fake_rnqcc = TempPath::new("gcc-smoke-verified-warning-rnqcc", "sh");
    std::fs::write(fake_rnqcc.path(), "#!/bin/sh\nexit 0\n")
        .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("failed to stat fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("compile")
        .arg("--limit")
        .arg("1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    let success = output.status.success();
    let out = stdout(output);
    assert!(!success, "{out}");
    assert!(!out.is_empty(), "expected failure output");
    assert!(
        out.contains("missing expected warning: shift count is negative"),
        "{out}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn gcc_torture_smoke_requires_verified_failure_output() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    let suite = TempPath::new("gcc-smoke-verified-failure-suite", "dir");
    let compile_dir = suite.path().join("compile");
    std::fs::create_dir_all(&compile_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        compile_dir.join("pr48767.c"),
        "int main(void) { return 0; } /* { dg-error \"void value\" } */\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let fake_rnqcc = TempPath::new("gcc-smoke-verified-failure-rnqcc", "sh");
    std::fs::write(fake_rnqcc.path(), "#!/bin/sh\nexit 0\n")
        .map_err(|err| format!("failed to write fake rnqcc: {err}"))?;
    let mut perms = std::fs::metadata(fake_rnqcc.path())
        .map_err(|err| format!("failed to stat fake rnqcc: {err}"))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(fake_rnqcc.path(), perms)
        .map_err(|err| format!("failed to chmod fake rnqcc: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(fake_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("compile")
        .arg("--limit")
        .arg("1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    let success = output.status.success();
    let out = stdout(output);
    assert!(!success, "{out}");
    assert!(
        out.contains("missing expected diagnostic: __builtin_va_arg cannot read a void value"),
        "{out}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn gcc_torture_smoke_materializes_tmpnam_fileio_tests() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-tmpnam-fileio-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(
        execute_dir.join("fprintf-2.c"),
        "#include <stdio.h>\n\
         #include <stdlib.h>\n\
         #include \"gcc_tmpnam.h\"\n\
         /* { dg-require-effective-target fileio } */\n\
         int main(void) {\n\
           char *path = gcc_tmpnam(0);\n\
           FILE *f = fopen(path, \"w\");\n\
           if (!f) return 1;\n\
           fputs(\"ok\", f);\n\
           fclose(f);\n\
           remove(path);\n\
           return 0;\n\
         }\n",
    )
    .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("skipped=0"), "{stdout}");
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_missing_explicit_expected_fixture() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-missing-expected-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let missing_expected = TempPath::new("gcc-smoke-missing-expected", "txt");

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--expected-failures")
        .arg(missing_expected.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected-failure fixture not found"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_directory_expected_fixture() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-directory-expected-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let expected = TempPath::new("gcc-smoke-directory-expected", "dir");
    std::fs::create_dir_all(expected.path())
        .map_err(|err| format!("failed to create expected fixture dir: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--expected-failures")
        .arg(expected.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected-failure fixture is not a file"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_missing_explicit_rnqcc_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-missing-rnqcc-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let missing_rnqcc = TempPath::new("gcc-smoke-missing-rnqcc", "bin");

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(missing_rnqcc.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("--rnqcc not found"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_directory_rnqcc_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-directory-rnqcc-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let rnqcc_dir = TempPath::new("gcc-smoke-directory-rnqcc", "dir");
    std::fs::create_dir_all(rnqcc_dir.path())
        .map_err(|err| format!("failed to create rnqcc dir: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc_dir.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("--rnqcc path is not a file"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_invalid_numeric_args() -> Result<(), String> {
    for (flag, value, expected) in [
        ("--start", "-1", "--start must be non-negative"),
        ("--limit", "0", "--limit must be positive"),
        ("--timeout", "0", "--timeout must be positive"),
        (
            "--max-failures",
            "-1",
            "--max-failures must be non-negative",
        ),
        (
            "--progress-every",
            "-1",
            "--progress-every must be non-negative",
        ),
    ] {
        let suite = TempPath::new("gcc-smoke-invalid-numeric-suite", "dir");
        let execute_dir = suite.path().join("execute");
        std::fs::create_dir_all(&execute_dir)
            .map_err(|err| format!("failed to create fake suite: {err}"))?;
        std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
            .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/gcc_torture_smoke.py")
            .arg("--rnqcc")
            .arg(rnqcc())
            .arg("--suite")
            .arg(suite.path())
            .arg("--mode")
            .arg("execute")
            .arg(flag)
            .arg(value)
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
        };

        assert!(
            !output.status.success(),
            "{flag} {value} unexpectedly succeeded"
        );
        let stderr = stderr(output);
        assert!(
            stderr.contains(expected),
            "{flag} {value} stderr did not contain {expected:?}: {stderr}"
        );
    }
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_directory_failure_log_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-directory-failure-log-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let failure_log = TempPath::new("gcc-smoke-directory-failure-log", "dir");
    std::fs::create_dir_all(failure_log.path())
        .map_err(|err| format!("failed to create failure log dir: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--failure-log")
        .arg(failure_log.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("--failure-log path is not a file"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_directory_skip_log_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-directory-skip-log-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let skip_log = TempPath::new("gcc-smoke-directory-skip-log", "dir");
    std::fs::create_dir_all(skip_log.path())
        .map_err(|err| format!("failed to create skip log dir: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--skip-log")
        .arg(skip_log.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("--skip-log path is not a file"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_file_parent_failure_log_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-file-parent-failure-log-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let parent = TempPath::new("gcc-smoke-file-parent-failure-log", "txt");
    std::fs::write(parent.path(), "")
        .map_err(|err| format!("failed to write failure log parent file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--failure-log")
        .arg(parent.path().join("failures.txt"))
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("--failure-log parent path is not a directory"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_file_parent_skip_log_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-file-parent-skip-log-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let parent = TempPath::new("gcc-smoke-file-parent-skip-log", "txt");
    std::fs::write(parent.path(), "")
        .map_err(|err| format!("failed to write skip log parent file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--skip-log")
        .arg(parent.path().join("skips.txt"))
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("--skip-log parent path is not a directory"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_file_artifact_dir_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-file-artifact-dir-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let artifact_dir = TempPath::new("gcc-smoke-file-artifact-dir", "txt");
    std::fs::write(artifact_dir.path(), "")
        .map_err(|err| format!("failed to write artifact dir file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("--artifact-dir path is not a directory"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_smoke_rejects_file_parent_artifact_dir_path() -> Result<(), String> {
    let suite = TempPath::new("gcc-smoke-file-parent-artifact-dir-suite", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create fake suite: {err}"))?;
    std::fs::write(execute_dir.join("raw.c"), "int main(void) { return 0; }\n")
        .map_err(|err| format!("failed to write fake GCC torture source: {err}"))?;

    let parent = TempPath::new("gcc-smoke-file-parent-artifact-dir", "txt");
    std::fs::write(parent.path(), "")
        .map_err(|err| format!("failed to write artifact dir parent file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/gcc_torture_smoke.py")
        .arg("--rnqcc")
        .arg(rnqcc())
        .arg("--suite")
        .arg(suite.path())
        .arg("--mode")
        .arg("execute")
        .arg("--limit")
        .arg("1")
        .arg("--artifact-dir")
        .arg(parent.path().join("artifacts"))
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run gcc torture smoke: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("--artifact-dir parent path is not a directory"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_duplicate_expected_entries() -> Result<(), String> {
    let expected = TempPath::new("gcc-duplicate-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-duplicate-xfail-report-failures", "txt");
    std::fs::write(
        expected.path(),
        "execute/dup.c | exit status -6\n\
         execute\\dup.c | timed out after 10.0s\n",
    )
    .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("duplicate expected failure"), "{stderr}");
    assert!(stderr.contains("execute/dup.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_unsorted_expected_entries() -> Result<(), String> {
    let expected = TempPath::new("gcc-unsorted-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-unsorted-xfail-report-failures", "txt");
    std::fs::write(
        expected.path(),
        "execute/z.c | exit status -6\n\
         execute/a.c | timed out after 10.0s\n",
    )
    .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected failures must be sorted"),
        "{stderr}"
    );
    assert!(stderr.contains("execute/a.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_missing_expected_fixture() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-xfail-report-expected", "txt");
    let failures = TempPath::new("gcc-missing-xfail-report-failures", "txt");
    std::fs::write(failures.path(), "")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected-failure fixture not found"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_directory_expected_fixture() -> Result<(), String> {
    let expected = TempPath::new("gcc-directory-xfail-report-expected", "dir");
    let failures = TempPath::new("gcc-directory-xfail-report-failures", "txt");
    std::fs::create_dir_all(expected.path())
        .map_err(|err| format!("failed to create expected fixture dir: {err}"))?;
    std::fs::write(failures.path(), "")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected-failure fixture is not a file"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_whitespace_padded_expected_test_name() -> Result<(), String> {
    let expected = TempPath::new("gcc-padded-expected-test-report", "txt");
    let failures = TempPath::new("gcc-padded-expected-test-report-failures", "txt");
    std::fs::write(expected.path(), " execute/raw.c | exit status -6\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("whitespace around test name"), "{stderr}");
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_nonrelative_expected_test_path() -> Result<(), String> {
    for bad_path in [
        "/tmp/raw.c",
        "../execute/raw.c",
        r"C:\tmp\raw.c",
        r"..\execute\raw.c",
    ] {
        let expected = TempPath::new("gcc-nonrelative-expected-test-report", "txt");
        let failures = TempPath::new("gcc-nonrelative-expected-test-report-failures", "txt");
        std::fs::write(expected.path(), format!("{bad_path} | exit status -6\n"))
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(failures.path(), "")
            .map_err(|err| format!("failed to write failure artifact: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/report_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected relative test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_control_character_expected_test_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-control-xfail-report-expected-test", "txt");
    let failures = TempPath::new("gcc-control-xfail-report-expected-test-failures", "txt");
    std::fs::write(expected.path(), "execute/raw\u{001f}.c | exit status -6\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("control character in test path"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_missing_failure_log() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-failure-log-expected", "txt");
    let failures = TempPath::new("gcc-missing-failure-log", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("failure log not found"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_directory_failure_log() -> Result<(), String> {
    let expected = TempPath::new("gcc-directory-failure-log-expected", "txt");
    let failures = TempPath::new("gcc-directory-failure-log", "dir");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::create_dir_all(failures.path())
        .map_err(|err| format!("failed to create failure log dir: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("failure log is not a file"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_missing_failure_log_tab_separator() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-failure-log-tab-expected", "txt");
    let failures = TempPath::new("gcc-missing-failure-log-tab-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c FAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("missing tab separator"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_duplicate_failure_log_rows() -> Result<(), String> {
    let expected = TempPath::new("gcc-duplicate-failure-log-expected", "txt");
    let failures = TempPath::new("gcc-duplicate-failure-log-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "execute/dup.c\tFAIL: exit status -6\n\
         execute\\dup.c\tFAIL: timed out after 10.0s\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("duplicate failure log row"), "{stderr}");
    assert!(stderr.contains("execute/dup.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_duplicate_skip_failure_log_rows() -> Result<(), String> {
    let expected = TempPath::new("gcc-duplicate-skip-failure-log-expected", "txt");
    let failures = TempPath::new("gcc-duplicate-skip-failure-log-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "execute/dup.c\tSKIP: requires unsupported builtin\n\
         execute\\dup.c\tFAIL: exit status -6\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("duplicate failure log row"), "{stderr}");
    assert!(stderr.contains("execute/dup.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_missing_failure_log_test_name() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-failure-log-test-expected", "txt");
    let failures = TempPath::new("gcc-missing-failure-log-test-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("missing test name"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_missing_failure_log_status() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-failure-log-status-expected", "txt");
    let failures = TempPath::new("gcc-missing-failure-log-status-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\t\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("missing status"), "{stderr}");
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_extra_failure_log_status_tab() -> Result<(), String> {
    let expected = TempPath::new("gcc-extra-tab-failure-log-status-expected", "txt");
    let failures = TempPath::new("gcc-extra-tab-failure-log-status-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "execute/raw.c\tFAIL: exit status -6\textra\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("unexpected tab in failure log status"),
        "{stderr}"
    );
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_missing_failure_log_reason() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-failure-log-reason-expected", "txt");
    let failures = TempPath::new("gcc-missing-failure-log-reason-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL:\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("missing reason"), "{stderr}");
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_missing_skip_failure_log_reason() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-skip-failure-log-reason-expected", "txt");
    let failures = TempPath::new("gcc-missing-skip-failure-log-reason-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tSKIP:\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("missing reason"), "{stderr}");
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_whitespace_padded_failure_log_status() -> Result<(), String> {
    let expected = TempPath::new("gcc-padded-failure-log-status-expected", "txt");
    let failures = TempPath::new("gcc-padded-failure-log-status-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\t FAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("whitespace around status"), "{stderr}");
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_whitespace_padded_failure_log_test_name() -> Result<(), String>
{
    let expected = TempPath::new("gcc-padded-failure-log-test-expected", "txt");
    let failures = TempPath::new("gcc-padded-failure-log-test-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), " execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/report_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("whitespace around test name"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_nonrelative_failure_log_test_path() -> Result<(), String> {
    for bad_path in [
        "/tmp/raw.c",
        "../execute/raw.c",
        r"C:\tmp\raw.c",
        r"..\execute\raw.c",
    ] {
        let expected = TempPath::new("gcc-nonrelative-failure-log-report-expected", "txt");
        let failures = TempPath::new("gcc-nonrelative-failure-log-report-failures", "txt");
        std::fs::write(expected.path(), "")
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(
            failures.path(),
            format!("{bad_path}\tFAIL: exit status -6\n"),
        )
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/report_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected relative test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_xfail_reporter_rejects_nonnormal_failure_log_test_path() -> Result<(), String> {
    for bad_path in ["execute//raw.c", "execute/./raw.c", r"execute\\raw.c"] {
        let expected = TempPath::new("gcc-nonnormal-failure-log-report-expected", "txt");
        let failures = TempPath::new("gcc-nonnormal-failure-log-report-failures", "txt");
        std::fs::write(expected.path(), "")
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(
            failures.path(),
            format!("{bad_path}\tFAIL: exit status -6\n"),
        )
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/report_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail reporter: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected normalized test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_unsorted_expected_entries() -> Result<(), String> {
    let expected = TempPath::new("gcc-unsorted-triage-expected", "txt");
    let failures = TempPath::new("gcc-unsorted-triage-failures", "txt");
    std::fs::write(
        expected.path(),
        "execute/z.c | exit status -6\n\
         execute/a.c | timed out after 10.0s\n",
    )
    .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected failures must be sorted"),
        "{stderr}"
    );
    assert!(stderr.contains("execute/a.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_duplicate_expected_entries() -> Result<(), String> {
    let expected = TempPath::new("gcc-duplicate-triage-expected", "txt");
    let failures = TempPath::new("gcc-duplicate-triage-failures", "txt");
    std::fs::write(
        expected.path(),
        "execute/dup.c | exit status -6\n\
         execute/dup.c | timed out after 10.0s\n",
    )
    .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/dup.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("duplicate expected failure"), "{stderr}");
    assert!(stderr.contains("execute/dup.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_whitespace_padded_expected_test_name() -> Result<(), String> {
    let expected = TempPath::new("gcc-padded-triage-expected-test", "txt");
    let failures = TempPath::new("gcc-padded-triage-expected-test-failures", "txt");
    std::fs::write(expected.path(), " execute/raw.c | exit status -6\n")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("whitespace around test name"), "{stderr}");
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_nonrelative_expected_test_path() -> Result<(), String> {
    for bad_path in [
        "/tmp/raw.c",
        "../execute/raw.c",
        r"C:\tmp\raw.c",
        r"..\execute\raw.c",
    ] {
        let expected = TempPath::new("gcc-nonrelative-triage-expected-test", "txt");
        let failures = TempPath::new("gcc-nonrelative-triage-expected-test-failures", "txt");
        std::fs::write(expected.path(), format!("{bad_path} | exit status -6\n"))
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
            .map_err(|err| format!("failed to write failure artifact: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/triage_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail triage: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected relative test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_nonnormal_expected_test_path() -> Result<(), String> {
    for bad_path in ["execute//raw.c", "execute/./raw.c", r"execute\\raw.c"] {
        let expected = TempPath::new("gcc-nonnormal-triage-expected-test", "txt");
        let failures = TempPath::new("gcc-nonnormal-triage-expected-test-failures", "txt");
        std::fs::write(expected.path(), format!("{bad_path} | exit status -6\n"))
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
            .map_err(|err| format!("failed to write failure artifact: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/triage_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail triage: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected normalized test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_missing_expected_fixture() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-triage-expected", "txt");
    let failures = TempPath::new("gcc-missing-triage-failures", "txt");
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected-failure fixture not found"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_directory_expected_fixture() -> Result<(), String> {
    let expected = TempPath::new("gcc-directory-triage-expected", "dir");
    let failures = TempPath::new("gcc-directory-triage-failures", "txt");
    std::fs::create_dir_all(expected.path())
        .map_err(|err| format!("failed to create expected fixture dir: {err}"))?;
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected-failure fixture is not a file"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_missing_failure_log() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-triage-failure-log-expected", "txt");
    let failures = TempPath::new("gcc-missing-triage-failure-log", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("failure log not found"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_directory_failure_log() -> Result<(), String> {
    let expected = TempPath::new("gcc-directory-triage-failure-log-expected", "txt");
    let failures = TempPath::new("gcc-directory-triage-failure-log", "dir");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::create_dir_all(failures.path())
        .map_err(|err| format!("failed to create failure log dir: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("failure log is not a file"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_missing_failure_log_tab_separator() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-triage-failure-log-tab-expected", "txt");
    let failures = TempPath::new("gcc-missing-triage-failure-log-tab-failures", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c FAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("missing tab separator"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_nonrelative_failure_log_test_path() -> Result<(), String> {
    for bad_path in [
        "/tmp/raw.c",
        "../execute/raw.c",
        r"C:\tmp\raw.c",
        r"..\execute\raw.c",
    ] {
        let expected = TempPath::new("gcc-nonrelative-triage-failure-log-expected", "txt");
        let failures = TempPath::new("gcc-nonrelative-triage-failure-log-failures", "txt");
        std::fs::write(expected.path(), "")
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(
            failures.path(),
            format!("{bad_path}\tFAIL: exit status -6\n"),
        )
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/triage_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail triage: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected relative test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_nonnormal_failure_log_test_path() -> Result<(), String> {
    for bad_path in ["execute//raw.c", "execute/./raw.c", r"execute\\raw.c"] {
        let expected = TempPath::new("gcc-nonnormal-triage-failure-log-expected", "txt");
        let failures = TempPath::new("gcc-nonnormal-triage-failure-log-failures", "txt");
        std::fs::write(expected.path(), "")
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(
            failures.path(),
            format!("{bad_path}\tFAIL: exit status -6\n"),
        )
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/triage_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail triage: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected normalized test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_file_suite_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-file-triage-suite-expected", "txt");
    let failures = TempPath::new("gcc-file-triage-suite-failures", "txt");
    let suite = TempPath::new("gcc-file-triage-suite", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::write(suite.path(), "")
        .map_err(|err| format!("failed to write suite path file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--suite")
        .arg(suite.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("suite path is not a directory"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_missing_artifact_dir() -> Result<(), String> {
    let expected = TempPath::new("gcc-missing-triage-artifact-expected", "txt");
    let failures = TempPath::new("gcc-missing-triage-artifact-failures", "txt");
    let artifact_dir = TempPath::new("gcc-missing-triage-artifact", "dir");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("artifact directory not found"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_file_artifact_dir_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-file-triage-artifact-expected", "txt");
    let failures = TempPath::new("gcc-file-triage-artifact-failures", "txt");
    let artifact_dir = TempPath::new("gcc-file-triage-artifact", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::write(artifact_dir.path(), "")
        .map_err(|err| format!("failed to write artifact path file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("artifact path is not a directory"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_file_copy_to_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-file-triage-copy-expected", "txt");
    let failures = TempPath::new("gcc-file-triage-copy-failures", "txt");
    let copy_to = TempPath::new("gcc-file-triage-copy", "txt");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::write(copy_to.path(), "")
        .map_err(|err| format!("failed to write copy-to path file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("copy-to path is not a directory"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_file_parent_copy_to_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-file-parent-triage-copy-expected", "txt");
    let failures = TempPath::new("gcc-file-parent-triage-copy-failures", "txt");
    let parent = TempPath::new("gcc-file-parent-triage-copy-parent", "txt");
    let copy_to = parent.path().join("bucketed");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/a.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::write(parent.path(), "")
        .map_err(|err| format!("failed to write copy-to parent file: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--copy-to")
        .arg(&copy_to)
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("copy-to parent path is not a directory"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_indexes_artifact_sources_once() -> Result<(), String> {
    let expected = TempPath::new("gcc-artifact-index-triage-expected", "txt");
    let failures = TempPath::new("gcc-artifact-index-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-artifact-index-triage-artifacts", "dir");
    let copy_to = TempPath::new("gcc-artifact-index-triage-copy", "dir");
    let artifact_test_dir = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&artifact_test_dir)
        .map_err(|err| format!("failed to create artifact dir: {err}"))?;
    std::fs::write(artifact_test_dir.join("source-path.txt"), "execute/raw.c\n")
        .map_err(|err| format!("failed to write source path: {err}"))?;
    std::fs::write(
        artifact_test_dir.join("raw.c"),
        "struct sample { int value; };\n",
    )
    .map_err(|err| format!("failed to write copied source: {err}"))?;
    std::fs::write(artifact_test_dir.join("output.txt"), "compiler output\n")
        .map_err(|err| format!("failed to write output artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("with source: 1"), "{stdout}");
    assert!(stdout.contains("aggregate-abi: 1"), "{stdout}");
    assert!(
        copy_to
            .path()
            .join("aggregate-abi")
            .join("execute__raw.c")
            .is_file(),
        "missing copied source"
    );
    assert!(
        copy_to
            .path()
            .join("aggregate-abi")
            .join("execute__raw.c.output.txt")
            .is_file(),
        "missing copied output"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_indexes_windows_style_artifact_source_paths() -> Result<(), String> {
    let expected = TempPath::new("gcc-windows-artifact-index-triage-expected", "txt");
    let failures = TempPath::new("gcc-windows-artifact-index-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-windows-artifact-index-triage-artifacts", "dir");
    let copy_to = TempPath::new("gcc-windows-artifact-index-triage-copy", "dir");
    let artifact_test_dir = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute\\raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&artifact_test_dir)
        .map_err(|err| format!("failed to create artifact dir: {err}"))?;
    std::fs::write(
        artifact_test_dir.join("source-path.txt"),
        "execute\\raw.c\n",
    )
    .map_err(|err| format!("failed to write source path: {err}"))?;
    std::fs::write(
        artifact_test_dir.join("raw.c"),
        "struct sample { int value; };\n",
    )
    .map_err(|err| format!("failed to write copied source: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("with source: 1"), "{stdout}");
    assert!(stdout.contains("aggregate-abi: 1"), "{stdout}");
    assert!(
        copy_to
            .path()
            .join("aggregate-abi")
            .join("execute__raw.c")
            .is_file(),
        "missing copied source"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_finds_windows_style_test_paths_in_suite() -> Result<(), String> {
    let expected = TempPath::new("gcc-windows-suite-triage-expected", "txt");
    let failures = TempPath::new("gcc-windows-suite-triage-failures", "txt");
    let suite = TempPath::new("gcc-windows-suite-triage-suite", "dir");
    let copy_to = TempPath::new("gcc-windows-suite-triage-copy", "dir");
    let execute_dir = suite.path().join("execute");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute\\raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create suite dir: {err}"))?;
    std::fs::write(
        execute_dir.join("raw.c"),
        "struct suite_case { int value; };\n",
    )
    .map_err(|err| format!("failed to write suite source: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("with source: 1"), "{stdout}");
    assert!(stdout.contains("aggregate-abi: 1"), "{stdout}");
    let copied =
        std::fs::read_to_string(copy_to.path().join("aggregate-abi").join("execute__raw.c"))
            .map_err(|err| format!("failed to read copied source: {err}"))?;
    assert!(copied.contains("suite_case"), "{copied}");
    Ok(())
}

#[test]
fn gcc_torture_triage_ignores_directory_suite_source_paths() -> Result<(), String> {
    let expected = TempPath::new("gcc-directory-source-triage-expected", "txt");
    let failures = TempPath::new("gcc-directory-source-triage-failures", "txt");
    let suite = TempPath::new("gcc-directory-source-triage-suite", "dir");
    let copy_to = TempPath::new("gcc-directory-source-triage-copy", "dir");
    let source_dir = suite.path().join("execute").join("raw.c");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&source_dir)
        .map_err(|err| format!("failed to create directory source path: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("with source: 0"), "{stdout}");
    assert!(
        !copy_to
            .path()
            .join("runtime-abort")
            .join("execute__raw.c")
            .exists(),
        "directory source path should not be copied"
    );
    assert!(
        copy_to
            .path()
            .join("runtime-abort")
            .join("execute__raw.c.reason.txt")
            .is_file(),
        "missing copied reason"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_copy_to_keeps_same_basename_sources() -> Result<(), String> {
    let expected = TempPath::new("gcc-copy-collision-triage-expected", "txt");
    let failures = TempPath::new("gcc-copy-collision-triage-failures", "txt");
    let suite = TempPath::new("gcc-copy-collision-triage-suite", "dir");
    let copy_to = TempPath::new("gcc-copy-collision-triage-copy", "dir");
    let compile_dir = suite.path().join("compile");
    let execute_dir = suite.path().join("execute");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "compile/raw.c\tFAIL: exit status -6\n\
         execute/raw.c\tFAIL: exit status -6\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&compile_dir)
        .map_err(|err| format!("failed to create compile suite dir: {err}"))?;
    std::fs::create_dir_all(&execute_dir)
        .map_err(|err| format!("failed to create execute suite dir: {err}"))?;
    std::fs::write(
        compile_dir.join("raw.c"),
        "struct compile_case { int value; };\n",
    )
    .map_err(|err| format!("failed to write compile source: {err}"))?;
    std::fs::write(
        execute_dir.join("raw.c"),
        "struct execute_case { int value; };\n",
    )
    .map_err(|err| format!("failed to write execute source: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--suite")
        .arg(suite.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let bucket = copy_to.path().join("aggregate-abi");
    let compile_copy = bucket.join("compile__raw.c");
    let execute_copy = bucket.join("execute__raw.c");
    assert!(compile_copy.is_file(), "missing compile source copy");
    assert!(execute_copy.is_file(), "missing execute source copy");
    let compile_text = std::fs::read_to_string(compile_copy)
        .map_err(|err| format!("failed to read compile source copy: {err}"))?;
    let execute_text = std::fs::read_to_string(execute_copy)
        .map_err(|err| format!("failed to read execute source copy: {err}"))?;
    assert!(compile_text.contains("compile_case"), "{compile_text}");
    assert!(execute_text.contains("execute_case"), "{execute_text}");
    Ok(())
}

#[test]
fn gcc_torture_triage_copy_to_flattens_windows_style_test_paths() -> Result<(), String> {
    let expected = TempPath::new("gcc-copy-windows-path-triage-expected", "txt");
    let failures = TempPath::new("gcc-copy-windows-path-triage-failures", "txt");
    let copy_to = TempPath::new("gcc-copy-windows-path-triage-copy", "dir");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), r"execute\raw.c	FAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    assert!(
        copy_to
            .path()
            .join("runtime-abort")
            .join(r"execute__raw.c.reason.txt")
            .is_file(),
        "missing flattened copied reason"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_copy_to_sanitizes_punctuation_in_test_paths() -> Result<(), String> {
    let expected = TempPath::new("gcc-copy-punctuation-path-triage-expected", "txt");
    let failures = TempPath::new("gcc-copy-punctuation-path-triage-failures", "txt");
    let copy_to = TempPath::new("gcc-copy-punctuation-path-triage-copy", "dir");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "execute/raw:case one.c\tFAIL: exit status -6\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    assert!(
        copy_to
            .path()
            .join("runtime-abort")
            .join("execute__raw_case_one.c.reason.txt")
            .is_file(),
        "missing sanitized copied reason"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_sanitized_copy_name_collisions() -> Result<(), String> {
    let expected = TempPath::new("gcc-copy-name-collision-triage-expected", "txt");
    let failures = TempPath::new("gcc-copy-name-collision-triage-failures", "txt");
    let copy_to = TempPath::new("gcc-copy-name-collision-triage-copy", "dir");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        "execute/raw:case.c\tFAIL: exit status -6\nexecute/raw_case.c\tFAIL: exit status -6\n",
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success(), "expected failure");
    let stderr = stderr(output);
    assert!(stderr.contains("copy filename collision"), "{}", stderr);
    Ok(())
}

#[test]
fn gcc_torture_triage_truncates_stdout_reasons_but_copies_full_reason() -> Result<(), String> {
    let expected = TempPath::new("gcc-long-reason-triage-expected", "txt");
    let failures = TempPath::new("gcc-long-reason-triage-failures", "txt");
    let copy_to = TempPath::new("gcc-long-reason-triage-copy", "dir");
    let long_reason = format!("{}{}", "diagnostic ".repeat(40), "sentinel-tail");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(
        failures.path(),
        format!("execute/noisy.c\tFAIL: {long_reason}\n"),
    )
    .map_err(|err| format!("failed to write failure artifact: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--copy-to")
        .arg(copy_to.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("execute/noisy.c | diagnostic "), "{stdout}");
    assert!(stdout.contains("..."), "{stdout}");
    assert!(!stdout.contains("sentinel-tail"), "{stdout}");
    let reason = std::fs::read_to_string(
        copy_to
            .path()
            .join("other")
            .join("execute__noisy.c.reason.txt"),
    )
    .map_err(|err| format!("failed to read copied reason: {err}"))?;
    assert!(reason.contains("sentinel-tail"), "{reason}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_nonrelative_artifact_source_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-nonrelative-artifact-triage-expected", "txt");
    let failures = TempPath::new("gcc-nonrelative-artifact-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-nonrelative-artifact-triage-artifacts", "dir");
    let artifact_test_dir = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&artifact_test_dir)
        .map_err(|err| format!("failed to create artifact dir: {err}"))?;
    std::fs::write(
        artifact_test_dir.join("source-path.txt"),
        "../execute/raw.c\n",
    )
    .map_err(|err| format!("failed to write source path: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("expected relative test path"), "{stderr}");
    assert!(stderr.contains("../execute/raw.c"), "{stderr}");
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_nonnormal_artifact_source_path() -> Result<(), String> {
    for bad_path in ["execute//raw.c", "execute/./raw.c", r"execute\\raw.c"] {
        let expected = TempPath::new("gcc-nonnormal-artifact-triage-expected", "txt");
        let failures = TempPath::new("gcc-nonnormal-artifact-triage-failures", "txt");
        let artifact_dir = TempPath::new("gcc-nonnormal-artifact-triage-artifacts", "dir");
        let artifact_test_dir = artifact_dir
            .path()
            .join("gcc_torture")
            .join("failures")
            .join("0000-raw");
        std::fs::write(expected.path(), "")
            .map_err(|err| format!("failed to write expected fixture: {err}"))?;
        std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
            .map_err(|err| format!("failed to write failure artifact: {err}"))?;
        std::fs::create_dir_all(&artifact_test_dir)
            .map_err(|err| format!("failed to create artifact dir: {err}"))?;
        std::fs::write(
            artifact_test_dir.join("source-path.txt"),
            format!("{bad_path}\n"),
        )
        .map_err(|err| format!("failed to write source path: {err}"))?;

        let output = match Command::new("python3")
            .arg("scripts/triage_gcc_torture_xfails.py")
            .arg("--expected")
            .arg(expected.path())
            .arg("--artifact-dir")
            .arg(artifact_dir.path())
            .arg(failures.path())
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("failed to run xfail triage: {err}")),
        };

        assert!(!output.status.success(), "{bad_path} unexpectedly passed");
        let stderr = stderr(output);
        assert!(stderr.contains("expected normalized test path"), "{stderr}");
        assert!(stderr.contains(bad_path), "{stderr}");
    }
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_control_character_artifact_source_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-control-artifact-triage-expected", "txt");
    let failures = TempPath::new("gcc-control-artifact-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-control-artifact-triage-artifacts", "dir");
    let artifact_test_dir = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&artifact_test_dir)
        .map_err(|err| format!("failed to create artifact dir: {err}"))?;
    std::fs::write(
        artifact_test_dir.join("source-path.txt"),
        "execute/raw\u{001f}.c\n",
    )
    .map_err(|err| format!("failed to write source path: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("control character in test path"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_whitespace_padded_artifact_source_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-padded-artifact-triage-expected", "txt");
    let failures = TempPath::new("gcc-padded-artifact-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-padded-artifact-triage-artifacts", "dir");
    let artifact_test_dir = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&artifact_test_dir)
        .map_err(|err| format!("failed to create artifact dir: {err}"))?;
    std::fs::write(
        artifact_test_dir.join("source-path.txt"),
        " execute/raw.c\n",
    )
    .map_err(|err| format!("failed to write source path: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("whitespace around artifact source path"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_multiline_artifact_source_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-multiline-artifact-triage-expected", "txt");
    let failures = TempPath::new("gcc-multiline-artifact-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-multiline-artifact-triage-artifacts", "dir");
    let artifact_test_dir = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&artifact_test_dir)
        .map_err(|err| format!("failed to create artifact dir: {err}"))?;
    std::fs::write(
        artifact_test_dir.join("source-path.txt"),
        "execute/raw.c\nexecute/other.c\n",
    )
    .map_err(|err| format!("failed to write source path: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected exactly one artifact source path"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_empty_artifact_source_path() -> Result<(), String> {
    let expected = TempPath::new("gcc-empty-artifact-triage-expected", "txt");
    let failures = TempPath::new("gcc-empty-artifact-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-empty-artifact-triage-artifacts", "dir");
    let artifact_test_dir = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&artifact_test_dir)
        .map_err(|err| format!("failed to create artifact dir: {err}"))?;
    std::fs::write(artifact_test_dir.join("source-path.txt"), "")
        .map_err(|err| format!("failed to write source path: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("expected exactly one artifact source path"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn gcc_torture_triage_rejects_duplicate_artifact_sources() -> Result<(), String> {
    let expected = TempPath::new("gcc-duplicate-artifact-triage-expected", "txt");
    let failures = TempPath::new("gcc-duplicate-artifact-triage-failures", "txt");
    let artifact_dir = TempPath::new("gcc-duplicate-artifact-triage-artifacts", "dir");
    let first = artifact_dir
        .path()
        .join("gcc_torture")
        .join("failures")
        .join("0000-raw");
    let second = artifact_dir
        .path()
        .join("gcc_torture")
        .join("xfail")
        .join("0000-raw");
    std::fs::write(expected.path(), "")
        .map_err(|err| format!("failed to write expected fixture: {err}"))?;
    std::fs::write(failures.path(), "execute/raw.c\tFAIL: exit status -6\n")
        .map_err(|err| format!("failed to write failure artifact: {err}"))?;
    std::fs::create_dir_all(&first)
        .map_err(|err| format!("failed to create first artifact dir: {err}"))?;
    std::fs::create_dir_all(&second)
        .map_err(|err| format!("failed to create second artifact dir: {err}"))?;
    std::fs::write(first.join("source-path.txt"), "execute/raw.c\n")
        .map_err(|err| format!("failed to write first source path: {err}"))?;
    std::fs::write(second.join("source-path.txt"), "execute\\raw.c\n")
        .map_err(|err| format!("failed to write second source path: {err}"))?;

    let output = match Command::new("python3")
        .arg("scripts/triage_gcc_torture_xfails.py")
        .arg("--expected")
        .arg(expected.path())
        .arg("--artifact-dir")
        .arg(artifact_dir.path())
        .arg(failures.path())
        .output()
    {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("failed to run xfail triage: {err}")),
    };

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(stderr.contains("duplicate artifact entry"), "{stderr}");
    assert!(stderr.contains("execute/raw.c"), "{stderr}");
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
fn direct_function_prototypes_follow_source_order() {
    let src = temp_file("source-order-function-prototype", "c");
    let out = temp_file("source-order-function-prototype", "s");
    std::fs::write(
        &src,
        "typedef struct node node;\n\
         struct node { node *next; int value; };\n\
         extern void use();\n\
         static void before(node *p) { use(p); }\n\
         void use(node **p) { if (*p) before(*p); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_long_double_uses_x87_stack_abi() {
    let src = temp_file("x86-ld-x87", "c");
    let out = temp_file("x86-ld-x87", "s");
    std::fs::write(
        &src,
        "long double id(long double x) { return x; }\n\
         long double add(long double a, long double b) { return a + b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("fldt 16(%rbp)"), "{asm}");
    assert!(asm.contains("fldt 32(%rbp)"), "{asm}");
    assert!(asm.contains("fstpt"), "{asm}");
    assert!(asm.contains("faddp %st, %st(1)"), "{asm}");
    assert!(!asm.contains("%xmm0"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_static_long_double_initializers_emit_x87_bytes() {
    let src = temp_file("x86-static-ld-x87", "c");
    let out = temp_file("x86-static-ld-x87", "s");
    std::fs::write(
        &src,
        "long double x = 27.0L;\n\
         long double z = 2;\n\
         struct S { char c; long double y; } s = { 'e', 29.0L };\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("x:\n\t.byte 0\n\t.byte 0\n\t.byte 0"), "{asm}");
    assert!(asm.contains("\t.byte 216\n\t.byte 3\n\t.byte 64"), "{asm}");
    assert!(asm.contains("\t.byte 232\n\t.byte 3\n\t.byte 64"), "{asm}");
    assert!(asm.contains("z:\n\t.byte 0\n\t.byte 0\n\t.byte 0"), "{asm}");
    assert!(asm.contains("\t.byte 128\n\t.byte 0\n\t.byte 64"), "{asm}");
    assert!(!asm.contains("x:\n\t.zero 16"), "{asm}");
    assert!(
        !asm.contains("s:\n\t.byte 101\n\t.zero 15\n\t.zero 16"),
        "{asm}"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_static_long_double_subnormal_initializers_preserve_significand() {
    let src = temp_file("x86-static-ld-subnormal", "c");
    let out = temp_file("x86-static-ld-subnormal", "s");
    std::fs::write(&src, "long double tiny = 4.9406564584124654e-324L;\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("tiny:\n"), "{asm}");
    assert!(
        asm.contains("\t.byte 128\n\t.byte 205\n\t.byte 59"),
        "{asm}"
    );
    assert!(!asm.contains("tiny:\n\t.zero 16"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_internal_temporaries_do_not_collide_with_user_tmp_names() {
    let src = temp_file("x86-temp-user-name-collision", "c");
    let out = temp_file("x86-temp-user-name-collision", "s");
    std::fs::write(
        &src,
        r#"
typedef struct { int v[4]; } Test1;

Test1 func2(void);

int func1(void) {
    Test1 test;
    test = func2();
    return test.v[0] != 10;
}

Test1 func2(void) {
    Test1 tmp;
    tmp.v[0] = 10;
    tmp.v[1] = 20;
    tmp.v[2] = 30;
    tmp.v[3] = 40;
    return tmp;
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
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(!asm.contains("movl %eax, -"), "{asm}");
    assert!(asm.contains("leaq -"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn tacky_temporaries_do_not_collide_with_user_dunder_tmp_names() {
    let src = temp_file("tacky-dunder-temp-user-name-collision", "c");
    let out = temp_file("tacky-dunder-temp-user-name-collision", "s");
    std::fs::write(
        &src,
        "struct L { int ntxns; int maxn; int *p; };\n\
         struct E { int type; union { struct L l; } u; };\n\
         int f(struct E *e, unsigned flags) {\n\
             struct L __tmp;\n\
             __tmp.ntxns = 7;\n\
             return (!(flags & 1) ? __tmp.ntxns : e->u.l.ntxns);\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(
        !asm.contains("movl %eax, -16(%rbp)\n\tmovq -16(%rbp), %r11"),
        "{asm}"
    );
    assert!(
        asm.contains("addq $8, %rdi\n\tmovq %rdi, %r11\n\tmovl (%r11), %eax"),
        "{asm}"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_typedef_alignment_applies_to_static_object() {
    let src = temp_file("x86-typedef-alignment-static", "c");
    let out = temp_file("x86-typedef-alignment-static", "s");
    std::fs::write(
        &src,
        "typedef struct { char c[8]; } V __attribute__((aligned(8)));\n\
         V v;\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains(".balign 8\nv:"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_stack_memory_param_copy_preserves_register_params() {
    let src = temp_file("x86-memory-param-preserves-register", "c");
    let out = temp_file("x86-memory-param-preserves-register", "s");
    std::fs::write(
        &src,
        "struct s { int i[18]; };\n\
         int f(struct s pa, int pb, ...) { return pb; }\n\
         struct s gs;\n\
         int main(void) { return f(gs, 0x1234) == 0x1234 ? 42 : 1; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    let f_start = asm.find("f:\n").expect("missing f label");
    let copy = asm[f_start..]
        .find("rep movsb")
        .map(|index| f_start + index)
        .expect("missing stack memory param copy");
    let save = asm[f_start..copy]
        .find("movl %edi,")
        .expect("register param was not saved before memory copy");
    assert!(save < copy - f_start, "{asm}");
    assert!(
        !asm[f_start..].contains("rep movsb\n\tmovl %edi, %eax"),
        "{asm}"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_long_double_comparisons_use_x87_status_flags() {
    let src = temp_file("x86-ld-x87-cmp", "c");
    let out = temp_file("x86-ld-x87-cmp", "s");
    std::fs::write(
        &src,
        "int eq(long double a, long double b) { return a == b; }\n\
         int ne(long double a, long double b) { return a != b; }\n\
         int lt(long double a, long double b) { return a < b; }\n\
         int le(long double a, long double b) { return a <= b; }\n\
         int gt(long double a, long double b) { return a > b; }\n\
         int ge(long double a, long double b) { return a >= b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("fucomip %st(1), %st"), "{asm}");
    assert!(asm.contains("fstp %st(0)"), "{asm}");
    assert!(asm.contains("sete"), "{asm}");
    assert!(asm.contains("setne"), "{asm}");
    assert!(asm.contains("setb"), "{asm}");
    assert!(asm.contains("setbe"), "{asm}");
    assert!(asm.contains("seta"), "{asm}");
    assert!(asm.contains("setae"), "{asm}");
    assert!(asm.contains("setp"), "{asm}");
    assert!(asm.contains("setnp"), "{asm}");
    assert!(!asm.contains("cmpl %xmm"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_double_comparisons_are_unordered_aware() {
    let src = temp_file("x86-double-nan-cmp", "c");
    let out = temp_file("x86-double-nan-cmp", "s");
    std::fs::write(
        &src,
        "int eq(double a, double b) { return a == b; }\n\
         int ne(double a, double b) { return a != b; }\n\
         int lt(double a, double b) { return a < b; }\n\
         int le(double a, double b) { return a <= b; }\n\
         int gt(double a, double b) { return a > b; }\n\
         int ge(double a, double b) { return a >= b; }\n\
         int logical_not(double a) { return !a; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("comisd"), "{asm}");
    assert!(asm.contains("setp"), "{asm}");
    assert!(asm.contains("setnp"), "{asm}");
    assert!(asm.contains("orb"), "{asm}");
    assert!(asm.contains("andb"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_uint_to_double_labels_are_unique_across_functions() {
    let src = temp_file("x86-uint-to-double-labels", "c");
    let out = temp_file("x86-uint-to-double-labels", "s");
    std::fs::write(
        &src,
        "double f(unsigned long x) { return x; }\n\
         double g(unsigned long x) { return x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    for prefix in [".Luint_to_double_ok", ".Luint_to_double_end"] {
        let labels: Vec<&str> = asm
            .lines()
            .filter_map(|line| line.strip_suffix(':'))
            .filter(|line| line.starts_with(prefix))
            .collect();
        let unique: std::collections::HashSet<&str> = labels.iter().copied().collect();
        assert_eq!(labels.len(), unique.len(), "{asm}");
        assert!(labels.len() >= 2, "{asm}");
    }

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_long_double_va_arg_uses_x87_loads_and_conversions() {
    let src = temp_file("x86-ld-va-arg", "c");
    let out = temp_file("x86-ld-va-arg", "s");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         long double take(int tag, ...) {\n\
             va_list ap;\n\
             va_start(ap, tag);\n\
             long double x = va_arg(ap, long double);\n\
             va_end(ap);\n\
             return x;\n\
         }\n\
         int cmp(int tag, ...) {\n\
             va_list ap;\n\
             va_start(ap, tag);\n\
             long double x = va_arg(ap, long double);\n\
             long double y = va_arg(ap, long double);\n\
             va_end(ap);\n\
             return x == y;\n\
         }\n\
         int cmp_int(int tag, ...) {\n\
             va_list ap;\n\
             va_start(ap, tag);\n\
             long double x = va_arg(ap, long double);\n\
             va_end(ap);\n\
             return x != 131;\n\
         }\n\
         int add_to_int(int n, ...) {\n\
             va_list ap;\n\
             va_start(ap, n);\n\
             n += va_arg(ap, long double);\n\
             va_end(ap);\n\
             return n;\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(!asm.contains("__rnqcc_va_arg_long double"), "{asm}");
    assert!(asm.contains("fldt (%r11)"), "{asm}");
    assert!(asm.contains("fstpt"), "{asm}");
    assert!(asm.contains("fildl"), "{asm}");
    assert!(asm.contains("fisttpl"), "{asm}");
    assert!(asm.contains("fucomip %st(1), %st"), "{asm}");
    assert!(!asm.contains("movt"), "{asm}");
    assert!(!asm.contains("movl %xmm"), "{asm}");
    assert!(!asm.contains("cvtsi2sdl %eax"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn aarch64_linux_long_double_uses_binary128_helpers() {
    let src = temp_file("aarch64-ld-binary128", "c");
    let out = temp_file("aarch64-ld-binary128", "s");
    std::fs::write(
        &src,
        "long double id(long double x) { return x; }\n\
         long double add(long double a, long double b) { return a + b; }\n\
         long double sub(long double a, long double b) { return a - b; }\n\
         long double mul(long double a, long double b) { return a * b; }\n\
         long double divv(long double a, long double b) { return a / b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("str q0, [sp"), "{asm}");
    assert!(asm.contains("str q1, [sp"), "{asm}");
    assert!(asm.contains("bl __addtf3"), "{asm}");
    assert!(asm.contains("bl __subtf3"), "{asm}");
    assert!(asm.contains("bl __multf3"), "{asm}");
    assert!(asm.contains("bl __divtf3"), "{asm}");
    assert!(asm.contains("str x30"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn aarch64_linux_long_double_supports_comparisons_and_negation() {
    let src = temp_file("aarch64-ld-cmp-neg", "c");
    let out = temp_file("aarch64-ld-cmp-neg", "s");
    std::fs::write(
        &src,
        "int eq(long double a, long double b) { return a == b; }\n\
         int ne(long double a, long double b) { return a != b; }\n\
         int lt(long double a, long double b) { return a < b; }\n\
         int le(long double a, long double b) { return a <= b; }\n\
         int gt(long double a, long double b) { return a > b; }\n\
         int ge(long double a, long double b) { return a >= b; }\n\
         long double neg(long double x) { return -x; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("bl __eqtf2"), "{asm}");
    assert!(asm.contains("bl __netf2"), "{asm}");
    assert!(asm.contains("bl __lttf2"), "{asm}");
    assert!(asm.contains("bl __letf2"), "{asm}");
    assert!(asm.contains("bl __gttf2"), "{asm}");
    assert!(asm.contains("bl __getf2"), "{asm}");
    assert!(asm.contains("cmp w0, w10"), "{asm}");
    assert!(asm.contains("eor w9, w9, w10"), "{asm}");
    assert!(asm.contains("str x30"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn aarch64_linux_long_double_literals_use_static_constant_pool() {
    let src = temp_file("aarch64-ld-literal-pool", "c");
    let out = temp_file("aarch64-ld-literal-pool", "s");
    std::fs::write(
        &src,
        "long double scale(long double x) { return 1.01L * x; }\n\
         int is_max_inf(void) { return __builtin_isinfl(1.01L * __LDBL_MAX__); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("__aarch64_long_double_const_0"), "{asm}");
    assert!(asm.contains("ldr q"), "{asm}");
    assert!(asm.contains("\t.quad"), "{asm}");
    assert!(asm.contains("bl __multf3"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn aarch64_linux_static_long_double_subnormal_initializers_preserve_significand() {
    let src = temp_file("aarch64-static-ld-subnormal", "c");
    let out = temp_file("aarch64-static-ld-subnormal", "s");
    std::fs::write(&src, "long double tiny = 4.9406564584124654e-324L;\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    let high = 0x3bcdu64 << 48;
    assert!(asm.contains("tiny:\n"), "{asm}");
    assert!(asm.contains(&format!("\t.quad 0\n\t.quad {high}")), "{asm}");
    assert!(!asm.contains("tiny:\n\t.zero 16"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn long_double_size_follows_target() {
    for (target, expected) in [
        ("x86_64-linux", 16),
        ("aarch64-linux", 16),
        ("aarch64-macos", 8),
    ] {
        let src = temp_file("target-long-double-size", "c");
        let out = temp_file("target-long-double-size", "s");
        std::fs::write(
            &src,
            format!(
                "int a[sizeof(long double) == {expected} ? 1 : -1];\n\
                 int b[__SIZEOF_LONG_DOUBLE__ == {expected} ? 1 : -1];\n"
            ),
        )
        .expect("failed to write input");

        let output = Command::new(rnqcc())
            .args(["--target", target, "-S", "-o"])
            .arg(&out)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(output.status.success(), "{}", stderr(output));
        let _ = std::fs::remove_file(src);
        let _ = std::fs::remove_file(out);
    }
}

#[test]
fn compiles_old_style_function_definition_after_promoted_prototype() {
    let src = temp_file("old-style-promoted-prototype", "c");
    let out = temp_file("old-style-promoted-prototype", "s");
    std::fs::write(&src, "void f(int);\nvoid f(x) unsigned char x; {}\n")
        .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_old_style_unspecified_function_pointer_assignment() {
    let src = temp_file("old-style-unspecified-function-pointer-assignment", "c");
    std::fs::write(
        &src,
        "bar(foo, a)\n\
              int (**foo)();\n\
         {\n\
           foo[1] = bar;\n\
           foo[a](1);\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_call_through_function_pointer_return_value() {
    let src = temp_file("function-pointer-return-call", "c");
    std::fs::write(
        &src,
        "int n;\n\
         typedef void (*fnptr)();\n\
         fnptr get_me();\n\
         inline void test(void) {\n\
           if (n < 10) (get_me())();\n\
           n++;\n\
         }\n\
         fnptr get_me() { return test; }\n\
         void foo() { test(); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_function_typedef_declarator_attributes() {
    let src = temp_file("function-typedef-declarator-attributes", "c");
    std::fs::write(
        &src,
        "typedef void ft(int);\n\
         void f(int args)__attribute__((noreturn));\n\
         void f2(ft *p __attribute__((noreturn))) { p = f; }\n\
         volatile ft g;\n\
         void f3(ft *p __attribute__((const))) { p = g; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_pointer_declarator_alignment_attribute() {
    let src = temp_file("pointer-declarator-alignment-attribute", "c");
    std::fs::write(
        &src,
        "int *__attribute__((__aligned__(16))) *p;\n\
         int main(void) { return **p; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_transparent_union_function_redeclaration() {
    let src = temp_file("transparent-union-function-redeclaration", "c");
    std::fs::write(
        &src,
        "typedef union { const struct sockaddr *__restrict __sockaddr__; } \
         __CONST_SOCKADDR_ARG __attribute__((__transparent_union__));\n\
         extern int _pure_socketcall(const struct sockaddr *);\n\
         extern int sendto(__CONST_SOCKADDR_ARG __addr);\n\
         int send(void) { return sendto((void *)0); }\n\
         int sendto(const struct sockaddr *to) { return _pure_socketcall(to); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_transparent_union_typeof_redeclaration() {
    let src = temp_file("transparent-union-typeof-redeclaration", "c");
    std::fs::write(
        &src,
        "extern void *malloc(__SIZE_TYPE__);\n\
         typedef struct T T;\n\
         struct T { void (*destroy)(void *); };\n\
         void destroy(union { void *this; } __attribute__((transparent_union)));\n\
         static const typeof(destroy) *_destroy = (const typeof(destroy)*)destroy;\n\
         void destroy(void *this);\n\
         static T *create_empty(void) {\n\
           T *this = malloc(sizeof(*this));\n\
           *this = (typeof(*this)){ _destroy };\n\
           return this;\n\
         }\n\
         void openssl_crl_load(void) { T *this = create_empty(); destroy(this); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_post_struct_storage_class_and_thread_local() {
    let src = temp_file("post-struct-storage-class-thread-local", "c");
    std::fs::write(
        &src,
        "struct wrapper { int value; } extern __thread a;\n\
         int f(void) { return a.value; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_struct_definition_function_return_declarator() {
    let src = temp_file("struct-definition-function-return-declarator", "c");
    std::fs::write(
        &src,
        "struct a b;\n\
         struct a { unsigned c:4; } d(void) { return b; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_static_cast_pointer_difference_integer_initializer() {
    let src = temp_file("static-cast-pointer-difference-integer-init", "c");
    std::fs::write(
        &src,
        "struct s { char p[2]; };\n\
         static struct s v;\n\
         const int o0 = (int)((void *)&v.p[0] - (void *)&v) + 0U;\n\
         const int o1 = (int)((void *)&v.p[0] - (void *)&v) + 1;\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_static_float_initializers_from_prior_scalar_constant() {
    let src = temp_file("static-float-init-from-prior-scalar-constant", "c");
    std::fs::write(
        &src,
        "const char a = 0x42;\n\
         const double b = (double) a;\n\
         double c[] = { (double) a, a, 1 + (double) a, 1 + a };\n\
         void f(void) { static const double d = (double) a; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_builtin_convertvector_to_typedef_vector_type() {
    let src = temp_file("builtin-convertvector-typedef-vector", "c");
    std::fs::write(
        &src,
        "typedef long long V __attribute__((vector_size(16)));\n\
         typedef double W __attribute__((vector_size(16)));\n\
         void foo(V *v) { __builtin_convertvector(*v, W); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_alignof_aligned_vector_variable_bound() {
    let src = temp_file("alignof-aligned-vector-variable-bound", "c");
    std::fs::write(
        &src,
        "#define alignment 128\n\
         char x __attribute__((aligned(alignment), vector_size(2)));\n\
         int f[__alignof__(x) == alignment ? 1 : -1];\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_anonymous_empty_aggregate_declaration() {
    let src = temp_file("anonymous-empty-aggregate-declaration", "c");
    std::fs::write(
        &src,
        "typedef union { struct s { __extension__ union { }; } data; } named;\n\
         typedef union { struct t { union { }; } data; };\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_zero_sized_struct_array_subscript_argument() {
    let src = temp_file("zero-sized-struct-array-subscript-argument", "c");
    std::fs::write(
        &src,
        "struct U {};\n\
         static struct U a[1];\n\
         extern void bar(struct U);\n\
         void foo(void) { bar(a[0]); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_forward_struct_return_tag_before_later_prototypes() {
    let src = temp_file("forward-struct-return-tag-before-prototypes", "c");
    std::fs::write(
        &src,
        "struct outer { int x; };\n\
         static inline struct hidden *to_hidden(struct outer *value)\n\
         {\n\
             const struct outer *tmp = value;\n\
             return (struct hidden *)(char *)tmp;\n\
         }\n\
         void use_a(struct hidden *);\n\
         void use_b(struct hidden *);\n\
         void call(struct outer *value) { use_a(to_hidden(value)); use_b(to_hidden(value)); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_sizeof_expression_edges_in_file_scope_bounds() {
    let src = temp_file("sizeof-expression-edges-file-scope-bounds", "c");
    std::fs::write(
        &src,
        "void foo(void);\n\
         void (*fp)(void);\n\
         char x[sizeof(1, foo) == sizeof(fp) ? 1 : -1];\n\
         struct s { char c; } a, b;\n\
         int c;\n\
         char y[sizeof((c ? a : b).c) == 1 ? 1 : -1];\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn sizeof_expression_types_follow_casts_and_integer_promotions() {
    let src = temp_file("sizeof-expression-promotions", "c");
    let exe = temp_file("sizeof-expression-promotions", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
             double d;\n\
             static char c = 0;\n\
             int i = 0;\n\
             long l = 0;\n\
             return sizeof((int)d) == 4\n\
                 && sizeof(c ^ c) == 4\n\
                 && sizeof(c << i) == 4\n\
                 && sizeof(i << l) == 4\n\
                 && sizeof(l >> c) == 8\n\
                 && sizeof(d ? c : 10l) == 8\n\
                 && sizeof(c = 10.0) == 1\n\
                 ? 42 : 1;\n\
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
    let status = Command::new(&exe)
        .status()
        .expect("failed to run executable");
    assert_eq!(status.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn compiles_sizeof_incomplete_extern_array_redeclaration() {
    let src = temp_file("sizeof-incomplete-extern-array-redeclaration", "c");
    std::fs::write(
        &src,
        "void foo(void)\n\
         {\n\
             extern char i[10];\n\
             { extern char i[]; char x[sizeof(i) == 10 ? 1 : -1]; }\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_c23_fixed_underlying_enum_type() {
    let src = temp_file("c23-fixed-underlying-enum-type", "c");
    std::fs::write(
        &src,
        "enum e : bool { X };\n\
         enum e f(void) { return __INT_MAX__ + 1; }\n\
         int g(void) { return !(enum e)(__INT_MAX__ + 1); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_gnu89_extern_inline_redefinition_once() {
    let src = temp_file("gnu89-extern-inline-redefinition-once", "c");
    let out = temp_file("gnu89-extern-inline-redefinition-once", "o");
    std::fs::write(
        &src,
        "extern __inline__ int odd(int i) { return i & 1; }\n\
         int foo(int i, int j) { return odd(i + j); }\n\
         int odd(int i) { return i & 1; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--Wno-missing-return", "-c"])
        .arg(&src)
        .args(["-o"])
        .arg(&out)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_gnu_qualified_old_style_parameter_declarations() {
    let src = temp_file("gnu-qualified-old-style-params", "c");
    let out = temp_file("gnu-qualified-old-style-params", "s");
    std::fs::write(
        &src,
        "struct rule { int x; };\n\
         typedef long time_t;\n\
         static time_t f(janfirst, year, rulep, offset)\n\
              __const time_t janfirst;\n\
              __const int year;\n\
              register __const struct rule * __const rulep;\n\
              __const long offset;\n\
         { return janfirst + year + rulep->x + offset; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_old_style_nested_function_with_vla_parameter() {
    let src = temp_file("old-style-nested-vla-param", "c");
    let out = temp_file("old-style-nested-vla-param", "s");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         int foo(int x, ...) {\n\
           va_list a;\n\
           va_start(a, x);\n\
           int b[6] = {};\n\
           int bar(c)\n\
             int c[1][va_arg(a, int)];\n\
           { return sizeof c[0]; }\n\
           int r = bar(b);\n\
           va_end(a);\n\
           return r;\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_function_declarator_after_object_in_mixed_declaration() {
    let src = temp_file("mixed-object-function-declarator", "c");
    let out = temp_file("mixed-object-function-declarator", "s");
    std::fs::write(
        &src,
        "union u { union u *a; double d; };\n\
         union u *s, g();\n\
         void f(void) { union u x = g(); s[0] = *x.a; s[1] = g(); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_function_typedef_object_declaration_as_function() {
    let src = temp_file("function-typedef-object-declaration", "c");
    let out = temp_file("function-typedef-object-declaration", "s");
    std::fs::write(
        &src,
        "typedef void visitor();\n\
         visitor callback;\n\
         void run(void *arg) { callback(arg); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn user_declared_abs_can_return_struct() {
    let src = temp_file("user-declared-abs-struct-return", "c");
    let out = temp_file("user-declared-abs-struct-return", "s");
    std::fs::write(
        &src,
        "struct S { int a; };\n\
         struct S abs(int);\n\
         struct S bar(int j) { return abs(j); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_float_indirect_load_to_stack_uses_xmm_scratch() {
    let src = temp_file("x86-float-indirect-stack-load", "c");
    let out = temp_file("x86-float-indirect-stack-load", "s");
    std::fs::write(
        &src,
        "void f(float *a, float *b, float *c) {\n\
             float t[2];\n\
             t[0] = b[0] - (float)__builtin_pow(c[0], 2);\n\
             t[1] = b[1] - (float)__builtin_pow(c[1], 2);\n\
             a[0] = t[0];\n\
             a[1] = t[1];\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn runs_gnu_vla_struct_member_pointer_copies() {
    let src = temp_file("gnu-vla-struct-member-copies", "c");
    let exe = temp_file("gnu-vla-struct-member-copies", "bin");
    std::fs::write(
        &src,
        r#"
typedef __SIZE_TYPE__ size_t;
int memcmp(const void *, const void *, size_t);
void abort(void);

void __attribute__((noinline)) bar(void *x, void *y) {
    struct S { char w[8]; } *p = x, *q = y;
    if (memcmp(p->w, "zyxwvut", 8) != 0) abort();
    if (memcmp(q[0].w, "abcdefg", 8) != 0) abort();
    if (memcmp(q[1].w, "ABCDEFG", 8) != 0) abort();
    if (memcmp(q[2].w, "zyxwvut", 8) != 0) abort();
    if (memcmp(q[3].w, "zyxwvut", 8) != 0) abort();
}

void __attribute__((noinline)) foo(void *x, int y) {
    struct S { char w[y]; } *p = x, a;
    a = ({ struct S b; b = p[2]; p[3] = b; });
    bar(&a, x);
}

int main(void) {
    struct S { char w[8]; } p[4] = {
        "abcdefg", "ABCDEFG", "zyxwvut", "ZYXWVUT"
    };
    foo(p, 8);
    return 0;
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
    assert_eq!(run.code(), Some(0));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
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
fn warns_on_comparison_of_distinct_pointer_types() {
    let src = temp_file("distinct-pointer-comparison-warning", "i");
    let out = temp_file("distinct-pointer-comparison-warning", "s");
    std::fs::write(
        &src,
        "int f(int *ip, long *lp, void *vp) {\n\
         int warnings = 0;\n\
         if (ip > vp) warnings++;\n\
         if (vp == ip) warnings++;\n\
         if (ip != lp) warnings++;\n\
         return warnings;\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let warning_stderr = stderr(output);
    assert_eq!(
        warning_stderr
            .matches("comparison of distinct pointer types")
            .count(),
        2,
        "{warning_stderr}"
    );

    let promoted = Command::new(rnqcc())
        .arg("--Werror")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(!promoted.status.success());
    let promoted_stderr = stderr(promoted);
    assert!(
        promoted_stderr.contains("comparison of distinct pointer types"),
        "{promoted_stderr}"
    );
    assert!(
        promoted_stderr.contains("warnings treated as errors"),
        "{promoted_stderr}"
    );

    let disabled = Command::new(rnqcc())
        .arg("-Wno-compare-distinct-pointer-types")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(disabled.status.success(), "{}", stderr(disabled));
    let disabled_stderr = stderr(disabled);
    assert!(
        !disabled_stderr.contains("comparison of distinct pointer types"),
        "{disabled_stderr}"
    );

    let enabled = Command::new(rnqcc())
        .arg("-Wcompare-distinct-pointer-types")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(enabled.status.success(), "{}", stderr(enabled));
    let enabled_stderr = stderr(enabled);
    assert!(
        enabled_stderr.contains("comparison of distinct pointer types"),
        "{enabled_stderr}"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn warns_on_deprecated_parameter_declarations() {
    let src = temp_file("deprecated-parameter-warning", "i");
    let out = temp_file("deprecated-parameter-warning", "s");
    std::fs::write(
        &src,
        "int f(int i __attribute__((deprecated(\"foo\\n\\t\\rbar\")))) {\n\
         return 0 ? i : 0;\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-Wdeprecated-declarations")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(output.status.success(), "{}", stderr(output));
    let warning_stderr = stderr(output);
    assert!(
        warning_stderr.contains("'i' is deprecated: foo.n.t.rbar"),
        "{warning_stderr}"
    );

    let disabled = Command::new(rnqcc())
        .arg("-Wno-deprecated-declarations")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(disabled.status.success(), "{}", stderr(disabled));
    let disabled_stderr = stderr(disabled);
    assert!(
        !disabled_stderr.contains("is deprecated"),
        "{disabled_stderr}"
    );

    let promoted = Command::new(rnqcc())
        .arg("--Werror")
        .arg("-Wdeprecated-declarations")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(!promoted.status.success());
    let promoted_stderr = stderr(promoted);
    assert!(
        promoted_stderr.contains("'i' is deprecated: foo.n.t.rbar"),
        "{promoted_stderr}"
    );
    assert!(
        promoted_stderr.contains("warnings treated as errors"),
        "{promoted_stderr}"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
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
fn x86_64_linux_local_calls_do_not_use_plt() {
    let src = temp_file("x86-local-call-no-plt", "i");
    let out = temp_file("x86-local-call-no-plt", "s");
    std::fs::write(
        &src,
        r#"
extern int printf(const char *, ...);

static int local_test(double x) {
    return x > 0.0;
}

int main(void) {
    return local_test(1.0) + printf("%d", 1);
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
    assert!(asm.contains("call local_test\n"), "{asm}");
    assert!(!asm.contains("call local_test@PLT"), "{asm}");
    assert!(asm.contains("call printf@PLT"), "{asm}");

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
    assert!(asm.contains("b.ne 1f"));
    assert!(asm.contains("b .Lif_else"));
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
fn aarch64_preserves_unsigned_long_constant_argument_width_after_copy_prop() {
    let src = temp_file("aarch64-ulong-constant-arg-width", "c");
    let out = temp_file("aarch64-ulong-constant-arg-width", "s");
    std::fs::write(
        &src,
        r#"
__attribute__((noinline)) unsigned long id(unsigned long x) { return x; }
double f(void) {
    unsigned long x = id(18446744073709551615UL);
    return (double)x;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "--optimize", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly output");
    assert!(asm.contains("\tmovz x9, #65535"), "{asm}");
    assert!(asm.contains("\tmovk x9, #65535, lsl #48"), "{asm}");
    assert!(asm.contains("\tldr x0, [sp]"), "{asm}");
    assert!(asm.contains("\tucvtf d9, x9"), "{asm}");
    assert!(!asm.contains("\tmovz w0, #65535"), "{asm}");

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
fn compiles_static_pointer_difference_initializers() {
    let src = temp_file("static-pointer-difference-initializers", "c");
    let exe = temp_file("static-pointer-difference-initializers", "bin");
    std::fs::write(
        &src,
        "int x[60];\n\
         char *y = ((char *)&(x[2 * 8 + 2]) - 8);\n\
         int z = (&\"Foobar\"[1] - &\"Foobar\"[0]);\n\
         int main(void) {\n\
             return z == 1 && y == (char *)&x[16] ? 42 : 1;\n\
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
fn compiles_static_nested_member_address_initializers() {
    let src = temp_file("static-nested-member-address-initializers", "c");
    let exe = temp_file("static-nested-member-address-initializers", "bin");
    std::fs::write(
        &src,
        "struct { struct { int x; int y; } p; } v;\n\
         int *z = &((&(v.p))->y);\n\
         int main(void) {\n\
             v.p.y = 42;\n\
             return *z;\n\
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
fn compiles_struct_member_function_pointer_returning_struct_pointer() {
    let src = temp_file("member-function-pointer-struct-return", "c");
    let exe = temp_file("member-function-pointer-struct-return", "bin");
    std::fs::write(
        &src,
        "struct chunk { int value; };\n\
         struct holder { struct chunk *(*make)(long); };\n\
         struct chunk storage;\n\
         struct chunk *make_chunk(long value) { storage.value = value; return &storage; }\n\
         int main(void) {\n\
             struct holder h;\n\
             h.make = make_chunk;\n\
             return h.make(42)->value;\n\
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
fn compiles_gnu_qualified_pointer_cast_static_initializer() {
    let src = temp_file("gnu-qualified-pointer-cast-static-init", "c");
    std::fs::write(
        &src,
        "struct S { char s; };\n\
         struct T { struct S t; };\n\
         struct S *const p = &((struct T * const)(0x4000))->t;\n\
         void foo(void) { }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn compiles_typeof_offsetof_member_designator() {
    let src = temp_file("typeof-offsetof-member-designator", "c");
    let exe = temp_file("typeof-offsetof-member-designator", "bin");
    std::fs::write(
        &src,
        "struct list_head { struct list_head *next; };\n\
         struct xt_target { struct list_head list; int value; };\n\
         const struct xt_target *t;\n\
         int main(void) {\n\
             return __builtin_offsetof(typeof (*t), value) == sizeof(struct list_head) ? 42 : 1;\n\
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
fn compiles_gnu89_pointer_integer_conversions() {
    let src = temp_file("gnu89-pointer-integer-conversions", "c");
    std::fs::write(
        &src,
        "typedef unsigned long si;\n\
         si move_si(p) si *p; { si x = p; p = (si *)x; return p[0]; }\n\
         x(p) int *p; { int y = p; return y; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
}

#[test]
fn recovers_missing_struct_semicolon_before_implicit_int_function() {
    let src = temp_file("missing-struct-semicolon-implicit-int", "c");
    let exe = temp_file("missing-struct-semicolon-implicit-int", "bin");
    std::fs::write(
        &src,
        "struct st { char a, b, c, d; }\n\
         zloop(struct st *s, int *p, int *q) {\n\
             int i;\n\
             struct st ss;\n\
             for (i = 0; i < 1; i++) { ss = s[i]; p[i] = ss.c; q[i] = ss.b; }\n\
         }\n\
         int main(void) {\n\
             struct st s[1]; int p[1]; int q[1];\n\
             s[0].b = 40; s[0].c = 2;\n\
             zloop(s, p, q);\n\
             return p[0] + q[0];\n\
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
fn local_object_declaration_shadows_typedef_name_in_statements() {
    let src = temp_file("local-object-shadows-typedef", "c");
    let exe = temp_file("local-object-shadows-typedef", "bin");
    std::fs::write(
        &src,
        "typedef struct { int x; } p;\n\
         typedef struct { int y; } t;\n\
         int main(void) {\n\
             t src;\n\
             t p;\n\
             src.y = 42;\n\
             p = src;\n\
             return p.y;\n\
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
fn compiles_static_array_member_pointer_difference() {
    let src = temp_file("static-array-member-pointer-diff", "c");
    let exe = temp_file("static-array-member-pointer-diff", "bin");
    std::fs::write(
        &src,
        "struct { char a, b, f[3]; } s;\n\
         long i = s.f - &s.b;\n\
         long long j = s.f - &s.b;\n\
         int main(void) { return i == 1 && j == 1 ? 42 : 1; }\n",
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
fn aarch64_indirect_call_callee_gets_stack_slot() {
    let src = temp_file("aarch64-indirect-call-callee-stack-slot", "c");
    let out = temp_file("aarch64-indirect-call-callee-stack-slot", "s");
    std::fs::write(
        &src,
        "typedef void foo(void);\n\
         int f(int x) {\n\
             if (x) { const foo *v; (*v)(); } else g(0);\n\
         }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "--stage", "s", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
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
fn function_pointer_parameter_call_shadows_global_function() {
    let src = temp_file("function-pointer-param-shadow", "i");
    let exe = temp_file("function-pointer-param-shadow", "bin");
    std::fs::write(
        &src,
        "int target(int x) { return x + 41; }\n\
         int fp(int x) { return x; }\n\
         int call(int (*fp)(int)) { return fp(1); }\n\
         int main(void) { return call(target); }\n",
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
fn block_function_prototype_shadows_outer_local_variable() {
    let src = temp_file("block-function-prototype-shadow", "i");
    let exe = temp_file("block-function-prototype-shadow", "bin");
    std::fs::write(
        &src,
        "int main(void) {\n\
             int foo = 3;\n\
             int bar = 4;\n\
             if (foo + bar > 0) {\n\
                 int foo(void);\n\
                 bar = foo();\n\
             }\n\
             return foo + bar;\n\
         }\n\
         int foo(void) { return 8; }\n",
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
    assert_eq!(run.code(), Some(11));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
}

#[test]
fn extern_block_function_prototype_reshadows_local_variable() {
    let client_src = temp_file("extern-block-prototype-client", "i");
    let helper_src = temp_file("extern-block-prototype-helper", "i");
    let exe = temp_file("extern-block-prototype", "bin");
    std::fs::write(
        &client_src,
        "int add_one_and_two(void) {\n\
             extern int sum(int a, int b);\n\
             int sum(int a, int b);\n\
             return sum(1, 2);\n\
         }\n\
         extern int sum(int x, int y);\n\
         int sum(int x, int y);\n\
         int add_three_and_four(void) {\n\
             int sum = 3;\n\
             if (sum > 2) {\n\
                 extern int sum(int one, int two);\n\
                 return sum(3, 4);\n\
             }\n\
             return 1;\n\
         }\n\
         int main(void) {\n\
             if (add_three_and_four() != 7) return 1;\n\
             if (add_one_and_two() != 3) return 1;\n\
             return 0;\n\
         }\n",
    )
    .expect("failed to write client input");
    std::fs::write(
        &helper_src,
        "extern int sum(int a, int b);\n\
         int sum(int i, int j) { return i + j; }\n\
         int sum(int x, int y);\n",
    )
    .expect("failed to write helper input");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&client_src)
        .arg(&helper_src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(0));

    let _ = std::fs::remove_file(client_src);
    let _ = std::fs::remove_file(helper_src);
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
fn emits_x86_64_assembly_for_int128_division_helpers() {
    let src = temp_file("x86-int128-div", "i");
    let out = temp_file("x86-int128-div", "s");
    std::fs::write(
        &src,
        "unsigned __int128 f(unsigned __int128 a, unsigned __int128 b) { return a / b; }\n\
         __int128 g(__int128 a, __int128 b) { return a % b; }\n",
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
    assert!(asm.contains("call __udivti3@PLT"), "{asm}");
    assert!(asm.contains("call __modti3@PLT"), "{asm}");
    assert!(!asm.contains("Octword"), "{asm}");

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

#[test]
fn treats_standard_abs_names_as_builtins_even_with_visible_declarations() {
    let src = temp_file("standard-abs-builtins", "c");
    let exe = temp_file("standard-abs-builtins", "bin");
    std::fs::write(
        &src,
        r#"
long long a = -1;
long long llabs(long long);

int main(void) {
    return llabs(a) == 1 ? 42 : 1;
}

long long llabs(long long value) {
    return value == 0 ? 0 : -100;
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
fn optimizer_converges_when_folding_nan_result() {
    let src = temp_file("optimization-fold-nan", "c");
    let exe = temp_file("optimization-fold-nan", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    double value = 0.0 / 0.0;
    return value != value ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--optimize")
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
fn optimized_long_return_constants_keep_return_width() {
    let src = temp_file("optimized-long-return-width", "c");
    let exe = temp_file("optimized-long-return-width", "bin");
    std::fs::write(
        &src,
        r#"
long cast_to_long(void) {
    return (long)18446744073709551615UL;
}

long implicit_to_long(void) {
    return 18446744073709551615UL;
}

int main(void) {
    long one = 1;
    if (cast_to_long() != -one) return 1;
    if (implicit_to_long() != -one) return 2;
    return 42;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--optimize")
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
fn compiles_and_runs_complex_component_lvalues() {
    let src = temp_file("complex-component-lvalues", "i");
    let exe = temp_file("complex-component-lvalues", "bin");
    std::fs::write(
        &src,
        r#"
typedef struct { double _Complex z; } Box;

int main(void) {
    double _Complex z = 1.0 + 2.0i;
    double _Complex arr[1];
    Box box;
    double _Complex *p;

    __real__ z = 3.0;
    __imag__ z = 4.0;
    if (__real__ z != 3.0) return 1;
    if (__imag__ z != 4.0) return 2;

    arr[0] = z;
    __imag__ arr[0] = 5.0;
    if (arr[0] != 3.0 + 5.0i) return 3;

    box.z = arr[0];
    __real__ box.z = 6.0;
    if (box.z != 6.0 + 5.0i) return 4;

    p = &box.z;
    __imag__ *p = 7.0;
    if (box.z != 6.0 + 7.0i) return 5;

    double *rp = &(__real__ z);
    *rp = 8.0;
    if (z != 8.0 + 4.0i) return 6;

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
fn compiles_and_runs_complex_conjugate_operator() {
    let src = temp_file("complex-conjugate", "i");
    let exe = temp_file("complex-conjugate", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    double _Complex x = {1.0, 2.0};
    double _Complex y = ~x;
    if (y != (double _Complex){1.0, -2.0}) return 1;
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
fn compiles_and_runs_gnu_imaginary_literals() {
    let src = temp_file("gnu-imaginary-literals", "i");
    let exe = temp_file("gnu-imaginary-literals", "bin");
    std::fs::write(
        &src,
        r#"
double _Complex ag = 1.0 + 1.0i;
double _Complex bg = -2.0 + 2.0i;
_Complex bare = 3.0 + 1.0iF;
typedef struct { _Complex char a; _Complex char b; } Scc2;
Scc2 s = { 1+2i, 3+4i };

int main(void) {
    double _Complex x = 1.0 + 2.0i;
    double _Complex y = ~x;
    if (x != (double _Complex){1.0, 2.0}) return 1;
    if (y != (double _Complex){1.0, -2.0}) return 2;
    if (ag != (double _Complex){1.0, 1.0}) return 3;
    if (bg != (double _Complex){-2.0, 2.0}) return 4;
    if (bare != (double _Complex){3.0, 1.0}) return 5;
    if (s.a != 1+2i || s.b != 3+4i) return 6;
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
fn compiles_and_runs_complex_function_returns() {
    let src = temp_file("complex-function-returns", "i");
    let exe = temp_file("complex-function-returns", "bin");
    std::fs::write(
        &src,
        r#"
double _Complex add(double _Complex x, double _Complex y) {
    return x + y;
}

double _Complex make(void) {
    return 1.0 + 2.0i;
}

int main(void) {
    double _Complex a = 1.0 + 1.0i;
    double _Complex b = -2.0 + 2.0i;
    double _Complex c = add(a, b);
    double _Complex d = make();
    if (c != (double _Complex){-1.0, 3.0}) return 1;
    if (d != (double _Complex){1.0, 2.0}) return 2;
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
fn compiles_and_runs_static_complex_constant_products() {
    let src = temp_file("static-complex-products", "i");
    let exe = temp_file("static-complex-products", "bin");
    std::fs::write(
        &src,
        r#"
float _Complex x = 1.0 + 14.0 * (1.0fi);
float _Complex y = 7.0 + 5.0 * (1.0fi);
float _Complex w = 8.0 + 19.0 * (1.0fi);

float _Complex p(float _Complex a, float _Complex b) {
    return a + b;
}

int main(void) {
    float _Complex z = p(x, y);
    if (z != w) return 1;
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
fn compiles_and_runs_complex_torture_edges() {
    let src = temp_file("complex-torture-edges", "i");
    let exe = temp_file("complex-torture-edges", "bin");
    std::fs::write(
        &src,
        r#"
typedef __complex__ float cf;
struct packed_x { char c; cf f; } __attribute__ ((__packed__));
unsigned char g;

__complex__ int ctest_int(__complex__ int x) {
    return ~x;
}

__complex__ float ctest_float(__complex__ float x) {
    return __builtin_conjf(x);
}

unsigned char div_unsigned(_Complex unsigned c) {
    unsigned char v = g;
    _Complex unsigned t = 42;
    t /= c;
    return v + t;
}

int takes_complex_pointer(_Complex float *p) {
    return *p == 2.0f + 3.0fi;
}

int main(void) {
    struct packed_x s;
    s.f = 1;
    s.c = 42;
    if (s.f != 1 || s.c != 42) return 1;

    __complex__ float f = ctest_float(1.0f + 2.0fi);
    if (f != 1.0f - 2.0fi) return 2;

    __complex__ int i = ctest_int(1.0 + 2.0i);
    if (i != 1.0 - 2.0i) return 3;

    if (div_unsigned(7) != 6) return 4;

    if (!takes_complex_pointer(&(_Complex float){2.0f + 3.0fi})) return 5;

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
fn aarch64_complex_int_arguments_use_integer_registers() {
    let src = temp_file("aarch64-complex-int-args", "c");
    let out = temp_file("aarch64-complex-int-args", "s");
    std::fs::write(
        &src,
        "extern void u(int, int);\n\
         void f(__complex__ int x) { u(0, x); }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(!asm.contains("fmov d"), "{asm}");
    assert!(!asm.contains("str d0"), "{asm}");
    assert!(asm.contains("str w0") || asm.contains("str x0"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
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
fn x86_float_to_int_overflow_saturates_to_int_max() {
    let src = temp_file("x86-float-to-int-overflow", "c");
    let exe = temp_file("x86-float-to-int-overflow", "bin");
    std::fs::write(
        &src,
        "#include <limits.h>\n\
         int f1(void) { return (int)2147483648.0f; }\n\
         int f2(void) { return (int)(float)(2147483647); }\n\
         int main(void) {\n\
             if (INT_MAX != 2147483647) return 0;\n\
             if (f1() != INT_MAX) return 1;\n\
             if (f2() != INT_MAX) return 2;\n\
             return 42;\n\
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
fn x86_variadic_libc_call_uses_only_abi_stack_arguments() {
    let src = temp_file("x86-variadic-libc-stack", "c");
    let out = temp_file("x86-variadic-libc-stack", "s");
    std::fs::write(
        &src,
        "#include <stdio.h>\n\
         char buf[64];\n\
         int main(void) {\n\
             return sprintf(buf, \"%d%d%d%d%d%d%d%d%d%d\", 1,2,3,4,5,6,7,8,9,10);\n\
         }\n",
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
    assert!(asm.contains("call sprintf@PLT"), "{asm}");
    assert!(
        asm.contains("movl $5, 0(%rsp)") || asm.contains("movl $5, (%rsp)"),
        "{asm}"
    );
    assert!(!asm.contains("movl $1, 0(%rsp)"), "{asm}");
    assert!(!asm.contains("movl $2, 8(%rsp)"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_local_variadic_shadow_arguments_are_not_duplicated() {
    let src = temp_file("x86-local-variadic-shadow", "c");
    let out = temp_file("x86-local-variadic-shadow", "s");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         int take(int a, double b, int c, ...) {\n\
             va_list ap;\n\
             va_start(ap, c);\n\
             int x = va_arg(ap, int);\n\
             va_end(ap);\n\
             return a + c + x + (int)b;\n\
         }\n\
         int main(void) {\n\
             return take(1, 1.0, 2, 3,4,5,6,7,8,9,10,11,12,13,14,15);\n\
         }\n",
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
    assert!(asm.contains("call take\n"), "{asm}");
    assert!(!asm.contains("call take@PLT"), "{asm}");
    assert!(
        asm.contains("movl $3, 0(%rsp)") || asm.contains("movl $3, (%rsp)"),
        "{asm}"
    );
    assert!(asm.contains("movl $15, 96(%rsp)"), "{asm}");
    assert!(!asm.contains("movl $7, 40(%rsp)"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_va_start_skips_fixed_stack_parameters() {
    let src = temp_file("x86-va-start-fixed-stack", "c");
    let out = temp_file("x86-va-start-fixed-stack", "s");
    std::fs::write(
        &src,
        "typedef unsigned long L;\n\
         L take(L p0, L p1, L p2, L p3, L p4, L p5, L p6, L p7, L p8, ...) {\n\
             __builtin_va_list ap;\n\
             __builtin_va_start(ap, p8);\n\
             return __builtin_va_arg(ap, L);\n\
         }\n\
         int main(void) {\n\
             return take(1,2,3,4,5,6,7,8,9,42) == 42 ? 0 : 1;\n\
         }\n",
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
    assert!(asm.contains("leaq 40(%rbp)"), "{asm}");
    assert!(!asm.contains("leaq 16(%rbp)"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_va_start_skips_fixed_sse_stack_parameters() {
    let src = temp_file("x86-va-start-fixed-sse-stack", "c");
    let out = temp_file("x86-va-start-fixed-sse-stack", "s");
    std::fs::write(
        &src,
        "typedef double D;\n\
         int take(D p0, D p1, D p2, D p3, D p4, D p5, D p6, D p7, D p8, ...) {\n\
             __builtin_va_list ap;\n\
             __builtin_va_start(ap, p8);\n\
             return __builtin_va_arg(ap, int);\n\
         }\n\
         int main(void) {\n\
             return take(1,2,3,4,5,6,7,8,9,42) == 42 ? 0 : 1;\n\
         }\n",
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
    assert!(asm.contains("leaq 24(%rbp)"), "{asm}");
    assert!(!asm.contains("leaq 16(%rbp)"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn rejects_builtin_va_arg_void_type() {
    let src = temp_file("va-arg-void", "c");
    std::fs::write(
        &src,
        "void f(__builtin_va_list ap) {\n\
             __builtin_va_arg(ap, void);\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-c")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success(), "unexpected success");
    assert!(
        stderr(output).contains("__builtin_va_arg cannot read a void value"),
        "missing va_arg void diagnostic"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn rejects_void_statement_expression_condition() {
    let src = temp_file("void-statement-expression-condition", "c");
    std::fs::write(
        &src,
        "void f(void) {\n\
             if (({ })) ;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-c")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success(), "unexpected success");
    assert!(
        stderr(output).contains("void value not ignored as it ought to be"),
        "missing void condition diagnostic"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn warns_on_negative_shift_count() {
    let src = temp_file("negative-shift-count", "c");
    std::fs::write(
        &src,
        "int f(int t) {\n\
             return t ? 1 >> (-1) : 0;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--stage")
        .arg("validate")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    assert!(
        stderr(output).contains("shift count is negative"),
        "missing negative shift diagnostic"
    );

    let _ = std::fs::remove_file(src);
}

#[test]
fn x86_va_start_accounts_for_aligned_fixed_long_double_stack_parameters() {
    let src = temp_file("x86-va-start-fixed-long-double-stack", "c");
    let out = temp_file("x86-va-start-fixed-long-double-stack", "s");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         int take(int a, int b, int c, int d, int e, int f, int g, long double h, ...) {\n\
             va_list ap;\n\
             va_start(ap, h);\n\
             return va_arg(ap, int);\n\
         }\n\
         double take_double(double a, double b, double c, double d, double e, double f,\n\
                            double g, long double h, ...) {\n\
             va_list ap;\n\
             va_start(ap, h);\n\
             return va_arg(ap, double);\n\
         }\n",
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
    assert!(asm.contains("fldt 32(%rbp)"), "{asm}");
    assert!(asm.contains("leaq 48(%rbp)"), "{asm}");
    assert!(asm.contains("fldt 16(%rbp)"), "{asm}");
    assert!(asm.contains("leaq 32(%rbp)"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_shadow_varargs_store_unnamed_long_double_with_x87() {
    let src = temp_file("x86-shadow-varargs-long-double", "c");
    let out = temp_file("x86-shadow-varargs-long-double", "s");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         void sink(int n, ...) {\n\
             va_list ap;\n\
             va_start(ap, n);\n\
             if (va_arg(ap, long double) != 3.14L) __builtin_abort();\n\
         }\n\
         int main(void) {\n\
             long double x = 3.14L;\n\
             sink(1, x);\n\
             return 0;\n\
         }\n",
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
    assert!(asm.contains("fldt"), "{asm}");
    assert!(asm.contains("fstpt 0(%rsp)"), "{asm}");
    assert!(!asm.contains("movt"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_shadow_varargs_aligns_long_double_literal_slots() {
    let src = temp_file("x86-shadow-varargs-long-double-literal", "c");
    let out = temp_file("x86-shadow-varargs-long-double-literal", "s");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         void sink(int n, ...) {\n\
             va_list ap;\n\
             va_start(ap, n);\n\
             if (va_arg(ap, int) != 10) __builtin_abort();\n\
             if (va_arg(ap, long long) != 10000000000LL) __builtin_abort();\n\
             if (va_arg(ap, int) != 11) __builtin_abort();\n\
             if (va_arg(ap, long double) != 3.14L) __builtin_abort();\n\
             if (va_arg(ap, int) != 12) __builtin_abort();\n\
         }\n\
         int main(void) {\n\
             sink(4, 10, 10000000000LL, 11, 3.14L, 12);\n\
             return 0;\n\
         }\n",
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
    assert!(asm.contains("fldt (%r11)"), "{asm}");
    assert!(asm.contains("fstpt 32(%rsp)"), "{asm}");
    assert!(asm.contains("movl $12, 48(%rsp)"), "{asm}");
    assert!(!asm.contains("movsd %xmm"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_va_arg_struct_temporary_gets_real_stack_storage() {
    let src = temp_file("x86-va-arg-struct-storage", "c");
    let out = temp_file("x86-va-arg-struct-storage", "s");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         typedef struct { double x, y; } point;\n\
         int take(int n, ...) {\n\
             va_list ap;\n\
             point p;\n\
             va_start(ap, n);\n\
             p = va_arg(ap, point);\n\
             va_end(ap);\n\
             return p.x == 1.0 && p.y == 2.0;\n\
         }\n",
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
    assert!(!asm.contains("0(%rbp)"), "{asm}");
    assert!(
        asm.contains("-32(%rbp)") || asm.contains("-24(%rbp)"),
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
fn rejects_flexible_array_member_initializer() {
    let src = temp_file("flexible-array-member-init", "c");
    std::fs::write(
        &src,
        "struct packet { int len; char data[]; };\n\
         static struct packet packets[] = { { 3, \"abc\" } };\n",
    )
    .expect("failed to write test input");

    let output = Command::new(rnqcc())
        .arg("-c")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success(), "unexpected success");
    assert!(
        stderr(output).contains("initialization of flexible array member"),
        "missing flexible array initializer diagnostic"
    );

    let _ = std::fs::remove_file(src);
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
fn internal_cpp_rejects_m_mf_with_multiple_inputs_before_writing_dependencies() {
    let dir = temp_file("internal-cpp-m-mf-multi-input", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let first = dir.join("first.c");
    let second = dir.join("second.c");
    let dep = dir.join("multi.d");
    std::fs::write(&first, "int first;\n").expect("failed to write first source");
    std::fs::write(&second, "int second;\n").expect("failed to write second source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-M")
        .arg("-MF")
        .arg(&dep)
        .arg(&first)
        .arg(&second)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("-MF requires exactly one input file"));
    assert!(!dep.exists());

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
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
fn internal_cpp_rejects_md_mf_with_multiple_inputs_before_writing_dependencies() {
    let dir = temp_file("internal-cpp-md-mf-multi-input", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let first = dir.join("first.c");
    let second = dir.join("second.c");
    let dep = dir.join("multi.d");
    std::fs::write(&first, "int first;\n").expect("failed to write first source");
    std::fs::write(&second, "int second;\n").expect("failed to write second source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-MD")
        .arg("-MF")
        .arg(&dep)
        .arg(&first)
        .arg(&second)
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    assert!(stderr(output).contains("-MF requires exactly one input file"));
    assert!(!dep.exists());

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_rejects_conflicting_dependency_modes_before_writing_dependencies() {
    let dir = temp_file("internal-cpp-conflicting-dep-modes", "d");
    std::fs::create_dir(&dir).expect("failed to create dep dir");
    let src = dir.join("main.c");
    let dep = dir.join("conflict.d");
    std::fs::write(&src, "int value;\n").expect("failed to write source");

    for modes in [["-M", "-MD"], ["-M", "-MM"]] {
        let _ = std::fs::remove_file(&dep);
        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .args(modes)
            .arg("-MF")
            .arg(&dep)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(!output.status.success());
        assert!(
            stderr(output).contains("-M, -MM, -MD, and -MMD are mutually exclusive"),
            "{modes:?}"
        );
        assert!(!dep.exists(), "{modes:?}");
    }

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn internal_cpp_rejects_dependency_modifiers_without_dependency_mode() {
    for args in [
        vec!["-MF", "unused.d"],
        vec!["-MP"],
        vec!["-MT", "target.o"],
        vec!["-MQ", "quoted target.o"],
    ] {
        let src = temp_file("dep-modifier-without-mode", "c");
        let asm = src.with_extension("s");
        std::fs::write(&src, "int main(void) { return 0; }\n").expect("failed to write source");
        let _ = std::fs::remove_file(&asm);

        let output = Command::new(rnqcc())
            .arg("--internal-cpp")
            .arg("-S")
            .args(&args)
            .arg(&src)
            .output()
            .expect("failed to run rnqcc");

        assert!(!output.status.success(), "{args:?}");
        assert!(
            stderr(output).contains("-MF, -MP, -MT, and -MQ require -M, -MM, -MD, or -MMD"),
            "{args:?}"
        );
        assert!(!asm.exists(), "{args:?}");

        let _ = std::fs::remove_file(src);
    }
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
fn internal_cpp_handles_gcc_assertion_predicate_if_expression() {
    let src = temp_file("internal-cpp-assertion-predicate", "c");
    std::fs::write(
        &src,
        "#define empty\n\
         #if empty#cpu(m68k)\n\
         int skipped = 1;\n\
         #endif\n\
         int kept = 2;\n",
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
    assert!(!stdout.contains("int skipped"), "{stdout}");
    assert!(stdout.contains("int kept = 2;"), "{stdout}");

    let _ = std::fs::remove_file(src);
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
         #endif\n\
         #if __has_builtin(__builtin_sqrtl) && __has_builtin(__builtin_atan2l)\n\
         int has_x87_math_builtins = 1;\n\
         #else\n\
         int has_x87_math_builtins = 0;\n\
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
    assert!(
        stdout.contains("int has_x87_math_builtins = 1;"),
        "{stdout}"
    );

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
fn treats_identifier_list_definition_as_non_prototype_without_param_decls() {
    let src = temp_file("old-style-implicit-param-decls", "c");
    let out = temp_file("old-style-implicit-param-decls", "s");
    std::fs::write(
        &src,
        "foo(a, b)\n\
         {\n\
             return foo();\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--Wno-missing-return")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_extern_void_assembly_symbol_reference() {
    let src = temp_file("extern-void-assembly-symbol", "c");
    let out = temp_file("extern-void-assembly-symbol", "s");
    std::fs::write(
        &src,
        "extern void _text;\n\
         unsigned long addr(void) {\n\
             return (unsigned long)&_text;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn compiles_external_symbol_arithmetic_static_initializer() {
    let src = temp_file("external-symbol-arithmetic-static-initializer", "c");
    let out = temp_file("external-symbol-arithmetic-static-initializer", "s");
    std::fs::write(
        &src,
        "extern void _text;\n\
         static unsigned long x = (unsigned long)&_text - 16 - 1;\n\
         unsigned long *addr(void) { return &x; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let assembly = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(assembly.contains("_text-17"), "{assembly}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
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
fn internal_cpp_va_end_preserves_argument_side_effects() {
    let src = temp_file("internal-cpp-va-end-side-effect", "c");
    let exe = temp_file("internal-cpp-va-end-side-effect", "bin");
    std::fs::write(
        &src,
        "#include <stdarg.h>\n\
         void consume(const char *fmt, ...) {\n\
             va_list ap0;\n\
             va_list ap1;\n\
             va_list *items[3];\n\
             va_list **cursor = items;\n\
             items[0] = &ap0;\n\
             items[1] = 0;\n\
             items[2] = &ap1;\n\
             va_start(ap0, fmt);\n\
             va_end(**cursor++);\n\
             cursor++;\n\
             va_start(ap1, fmt);\n\
             va_end(**cursor);\n\
             if (*cursor == 0) __builtin_abort();\n\
         }\n\
         int main(void) { consume(\"%d\", 7); return 0; }\n",
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
    assert_eq!(status.code(), Some(0));

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
             return FLT_MANT_DIG == 24 && DBL_MANT_DIG == 53 && LDBL_MANT_DIG >= DBL_MANT_DIG &&\n\
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
fn internal_cpp_float_header_long_double_limits_follow_target() {
    let src = temp_file("internal-cpp-float-header-ld-target", "c");
    std::fs::write(
        &src,
        "#include <float.h>\n\
         int mant = LDBL_MANT_DIG;\n\
         int dig = LDBL_DIG;\n\
         int min_exp = LDBL_MIN_EXP;\n\
         int max_exp = LDBL_MAX_EXP;\n\
         long double min = LDBL_MIN;\n\
         long double eps = LDBL_EPSILON;\n",
    )
    .expect("failed to write source");

    let x86 = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--target")
        .arg("x86_64-linux")
        .arg("-nostdinc")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(x86.status.success(), "{}", stderr(x86));
    let x86_stdout = stdout(x86);
    assert!(x86_stdout.contains("int mant = 64;"), "{x86_stdout}");
    assert!(x86_stdout.contains("int dig = 18;"), "{x86_stdout}");
    assert!(
        x86_stdout.contains("int min_exp = (-16381);"),
        "{x86_stdout}"
    );
    assert!(x86_stdout.contains("int max_exp = 16384;"), "{x86_stdout}");
    assert!(
        x86_stdout.contains("long double eps = 1.08420217248550443401e-19L;"),
        "{x86_stdout}"
    );

    let aarch64 = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--target")
        .arg("aarch64-linux")
        .arg("-nostdinc")
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");
    assert!(aarch64.status.success(), "{}", stderr(aarch64));
    let aarch64_stdout = stdout(aarch64);
    assert!(
        aarch64_stdout.contains("int mant = 113;"),
        "{aarch64_stdout}"
    );
    assert!(aarch64_stdout.contains("int dig = 33;"), "{aarch64_stdout}");
    assert!(
        aarch64_stdout.contains("int min_exp = (-16381);"),
        "{aarch64_stdout}"
    );
    assert!(
        aarch64_stdout.contains("int max_exp = 16384;"),
        "{aarch64_stdout}"
    );
    assert!(
        aarch64_stdout.contains("long double eps = 1.92592994438723585305597794258492732e-34L;"),
        "{aarch64_stdout}"
    );

    let _ = std::fs::remove_file(src);
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
fn internal_cpp_exposes_builtin_type_size_macros() {
    let src = temp_file("internal-cpp-builtin-type-size-macros", "c");
    let out = temp_file("internal-cpp-builtin-type-size-macros", "s");
    std::fs::write(
        &src,
        "int a[sizeof(__SIZE_TYPE__) == __SIZEOF_SIZE_T__ ? 1 : -1];\n\
         int b[sizeof(__WCHAR_TYPE__) == __SIZEOF_WCHAR_T__ ? 1 : -1];\n\
         int c[sizeof(__WINT_TYPE__) == __SIZEOF_WINT_T__ ? 1 : -1];\n\
         int d[sizeof(__PTRDIFF_TYPE__) == __SIZEOF_PTRDIFF_T__ ? 1 : -1];\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn internal_cpp_provides_minimal_immintrin_virtual_header() {
    let src = temp_file("internal-cpp-immintrin-minimal", "c");
    let out = temp_file("internal-cpp-immintrin-minimal", "s");
    std::fs::write(
        &src,
        "#include <immintrin.h>\n\
         __m128i do_stuff(__m128i x) {\n\
             __m128i y = _mm_abs_epi32(x);\n\
             return _mm_mullo_epi32(y, x);\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("--target")
        .arg("x86_64-linux")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
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
fn skips_empty_asm_goto_statements() {
    let src = temp_file("asm-goto-compat", "c");
    let exe = temp_file("asm-goto-compat", "bin");
    std::fs::write(
        &src,
        r#"
int main(void) {
    asm goto ("" : : : : label);
    return 42;
label:
    return 1;
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
fn accepts_label_at_end_of_block_after_skipped_asm_goto() {
    let src = temp_file("asm-goto-label-at-end", "c");
    let out = temp_file("asm-goto-label-at-end", "s");
    std::fs::write(
        &src,
        r#"
void g(void);
void f(void) {
    int value;
    asm goto ("" : "=&r"(value) : : : done);
    g();
done:
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--Wno-missing-return")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
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
fn supports_x87_inline_asm_math_compatibility() {
    let src = temp_file("inline-asm-x87-math-compat", "c");
    let exe = temp_file("inline-asm-x87-math-compat", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);

static long double atan_wrapper(long double y, long double x) {
    register long double value;
    __asm __volatile__ ("fpatan\n\t" : "=t" (value) : "0" (x), "u" (y) : "st(1)");
    return value;
}

static long double sqrt_wrapper(long double x) {
    register long double value;
    __asm __volatile__ ("fsqrt" : "=t" (value) : "0" (x));
    return value;
}

int main(void) {
    long double x = sqrt_wrapper(1.0L);
    long double y = atan_wrapper(0.0L, x);
    if (x != 1.0L || y != 0.0L)
        abort();
    return 42;
}
"#,
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("-o")
        .arg(&exe)
        .arg(&src)
        .arg("-lm")
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
fn internal_cpp_include_next_stdarg_uses_next_filesystem_header() {
    let first_dir = temp_file("internal-cpp-include-next-stdarg-first", "d");
    let second_dir = temp_file("internal-cpp-include-next-stdarg-second", "d");
    let src = temp_file("internal-cpp-include-next-stdarg", "c");
    std::fs::create_dir(&first_dir).expect("failed to create first include dir");
    std::fs::create_dir(&second_dir).expect("failed to create second include dir");
    let wrapper = first_dir.join("wrapper.h");
    let next_stdarg = second_dir.join("stdarg.h");
    std::fs::write(
        &wrapper,
        "#if __has_include_next(<stdarg.h>)\n\
         #define HAS_NEXT_STDARG 1\n\
         #else\n\
         #define HAS_NEXT_STDARG 0\n\
         #endif\n\
         #include_next <stdarg.h>\n",
    )
    .expect("failed to write wrapper header");
    std::fs::write(&next_stdarg, "#define NEXT_STDARG_VALUE 41\n")
        .expect("failed to write next stdarg header");
    std::fs::write(
        &src,
        "#include <wrapper.h>\n\
         int main(void) { return NEXT_STDARG_VALUE + HAS_NEXT_STDARG; }\n",
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
        stdout.contains("int main(void) { return 41 + 1; }"),
        "{stdout}"
    );

    let _ = std::fs::remove_file(wrapper);
    let _ = std::fs::remove_file(next_stdarg);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(first_dir);
    let _ = std::fs::remove_dir(second_dir);
}

#[test]
fn internal_cpp_has_include_next_stdarg_ignores_virtual_header() {
    let include_dir = temp_file("internal-cpp-has-include-next-stdarg", "d");
    let src = temp_file("internal-cpp-has-include-next-stdarg", "c");
    std::fs::create_dir(&include_dir).expect("failed to create include dir");
    let wrapper = include_dir.join("wrapper.h");
    std::fs::write(
        &wrapper,
        "#if __has_include_next(<stdarg.h>)\n\
         #define HAS_NEXT_STDARG 1\n\
         #else\n\
         #define HAS_NEXT_STDARG 0\n\
         #endif\n",
    )
    .expect("failed to write wrapper header");
    std::fs::write(
        &src,
        "#include <wrapper.h>\n\
         int main(void) { return HAS_NEXT_STDARG; }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("-nostdinc")
        .arg("-I")
        .arg(&include_dir)
        .arg("-E")
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int main(void) { return 0; }"), "{stdout}");

    let _ = std::fs::remove_file(wrapper);
    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_dir(include_dir);
}

#[test]
fn internal_cpp_handles_macro_generated_virtual_header_includes() {
    let src = temp_file("internal-cpp-macro-virtual-includes", "c");
    let exe = temp_file("internal-cpp-macro-virtual-includes", "bin");
    std::fs::write(
        &src,
        "#define RNQCC_STDARG <stdarg.h>\n\
         #define RNQCC_STDINT <stdint.h>\n\
         #define RNQCC_TYPES <sys/types.h>\n\
         #include RNQCC_STDARG\n\
         #include RNQCC_STDINT\n\
         #include RNQCC_TYPES\n\
         int take(int x, ...) {\n\
             va_list ap;\n\
             va_start(ap, x);\n\
             int y = va_arg(ap, int);\n\
             va_end(ap);\n\
             return y;\n\
         }\n\
         int main(void) {\n\
             uint32_t u = 40;\n\
             size_t s = 1;\n\
             ssize_t ss = 1;\n\
             return (int)u + (int)s + (int)ss == take(0, 42) ? 42 : 1;\n\
         }\n",
    )
    .expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(&exe).status().expect("failed to run output");
    assert_eq!(run.code(), Some(42));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
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
fn external_cpp_receives_target_platform_macros() {
    let log_path = temp_file("cc-target-macros", "log");
    let cc_script = write_cc_script("cc-target-macros", &log_path);

    let output = Command::new(rnqcc())
        .arg("--cc")
        .arg(&cc_script)
        .arg("--target")
        .arg("x86_64-linux")
        .args(["-E", "tests/return_42.c"])
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success());
    let log = std::fs::read_to_string(&log_path).expect("failed to read cc log");
    assert!(log.contains("-U__APPLE__"), "{log}");
    assert!(log.contains("-U__MACH__"), "{log}");
    assert!(log.contains("-D__x86_64__=1"), "{log}");
    assert!(log.contains("-D__linux__=1"), "{log}");
    assert!(log.contains("-D__ELF__=1"), "{log}");

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
fn internal_cpp_accepts_raw_non_utf8_source_bytes() {
    let src = temp_file("internal-cpp-raw-byte", "c");
    let exe = temp_file("internal-cpp-raw-byte", "bin");
    let mut source = b"static const unsigned char g[] = \"\\0".to_vec();
    source.push(0xff);
    source.extend_from_slice(b"\";\nint main(void) { return sizeof g != 3 || g[1] != 255; }\n");
    std::fs::write(&src, source).expect("failed to write source");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
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
    let expected_long_double_size = if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        8
    } else {
        16
    };
    assert!(stdout(output).contains(&format!("int widths = {expected_long_double_size} + 4;")));
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
fn expands_bom_prefixed_response_file_arguments() {
    let src = temp_file("bom response file source", "c");
    let out = temp_file("bom response file output", "s");
    let rsp = temp_file("bom-response-file", "rsp");
    std::fs::write(&src, "int main(void) { return 42; }\n").expect("failed to write source");
    let mut contents = vec![0xef, 0xbb, 0xbf];
    contents.extend_from_slice(
        format!("-S -o \"{}\" \"{}\"\n", out.display(), src.display()).as_bytes(),
    );
    std::fs::write(&rsp, contents).expect("failed to write response file");

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
fn reports_malformed_response_file_path() {
    let rsp = TempPath::new("bad-response-file", "rsp");
    std::fs::write(rsp.path(), "-S 'unterminated\n").expect("failed to write response file");

    let output = Command::new(rnqcc())
        .arg(format!("@{}", rsp.path().display()))
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains(&rsp.path().display().to_string()),
        "{stderr}"
    );
    assert!(
        stderr.contains("unterminated ' quote in response file"),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn reports_too_deep_response_file_path() {
    let dir = TempPath::new("deep-response-files", "d");
    std::fs::create_dir(dir.path()).expect("failed to create response dir");
    let files: Vec<_> = (0..18)
        .map(|index| dir.path().join(format!("{index}.rsp")))
        .collect();

    for pair in files.windows(2) {
        std::fs::write(&pair[0], format!("@{}\n", pair[1].display()))
            .expect("failed to write response file");
    }
    std::fs::write(files.last().expect("missing final response file"), "-S\n")
        .expect("failed to write final response file");

    let output = Command::new(rnqcc())
        .arg(format!("@{}", files[0].display()))
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("response file nesting is too deep while reading"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&files[17].display().to_string()),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn reports_response_file_cycles() {
    let dir = TempPath::new("cyclic-response-files", "d");
    std::fs::create_dir(dir.path()).expect("failed to create response dir");
    let first = dir.path().join("first.rsp");
    let second = dir.path().join("second.rsp");
    std::fs::write(&first, "@second.rsp\n").expect("failed to write first response file");
    std::fs::write(&second, "@./first.rsp\n").expect("failed to write second response file");

    let output = Command::new(rnqcc())
        .arg(format!("@{}", first.display()))
        .output()
        .expect("failed to run rnqcc");

    assert!(!output.status.success());
    let stderr = stderr(output);
    assert!(
        stderr.contains("response file cycle while reading"),
        "{stderr}"
    );
    assert!(stderr.contains("./first.rsp"), "{stderr}");
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
fn scalarized_vector_binary_reads_lanes_by_offset() {
    let src = temp_file("vector-binary-lane-offsets", "c");
    std::fs::write(
        &src,
        r#"
typedef unsigned short V __attribute__((vector_size(16)));

V divv(V x) {
    return x / ((V){1, 2, 4, 8, 16, 32, 64, 128});
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let tacky = stdout(output);
    assert_eq!(tacky.matches("CopyFromOffset").count(), 16, "{tacky}");
    assert!(!tacky.contains("GetAddress"), "{tacky}");
    assert!(!tacky.contains("AddPtr"), "{tacky}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn runs_gcc_pr53645_vector_division_remainder_shape() {
    let src = temp_file("gcc-pr53645-vector-div-rem", "c");
    let exe = temp_file("gcc-pr53645-vector-div-rem", "bin");
    std::fs::write(
        &src,
        r#"
typedef unsigned short UV __attribute__((vector_size(16)));
typedef short SV __attribute__((vector_size(16)));

__attribute__((noinline)) UV uq(UV y) { return y / ((UV){1, 4, 2, 8, 16, 64, 32, 128}); }
__attribute__((noinline)) UV ur(UV y) { return y % ((UV){1, 4, 2, 8, 16, 64, 32, 128}); }
__attribute__((noinline)) SV sq(SV y) { return y / ((SV){6, 5, 6, 5, 6, 5, 6, 5}); }
__attribute__((noinline)) SV sr(SV y) { return y % ((SV){6, 5, 6, 5, 6, 5, 6, 5}); }

int main(void) {
    UV u = (UV){73U, 65531U, 0U, 174U, 921U, 65535U, 17U, 178U};
    SV s = (SV){73, -9123, 32761, 8191, 16371, 1201, 12701, 9999};
    UV uquot = uq(u);
    UV urem = ur(u);
    SV squot = sq(s);
    SV srem = sr(s);
    if (uquot[0] != 73U || uquot[1] != 16382U || uquot[7] != 1U) return 1;
    if (urem[0] != 0U || urem[1] != 3U || urem[7] != 50U) return 2;
    if (squot[0] != 12 || squot[1] != -1824 || squot[7] != 1999) return 3;
    if (srem[0] != 1 || srem[1] != -3 || srem[7] != 4) return 4;
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
fn x86_64_linux_pools_repeated_double_constants() {
    let src = temp_file("x86-double-constant-pool", "c");
    let out = temp_file("x86-double-constant-pool", "s");
    std::fs::write(
        &src,
        r#"
double a(void) { return 3.5; }
double b(void) { return 3.5 + 3.5; }
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
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    let labels = asm
        .lines()
        .filter(|line| line.starts_with("__double_const_"))
        .count();
    assert_eq!(labels, 1, "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_linux_float_returns_do_not_use_sse_immediates() {
    let src = temp_file("x86-float-return-immediates", "c");
    let out = temp_file("x86-float-return-immediates", "s");
    std::fs::write(
        &src,
        r#"
float addf(float a, float b) { return a + b; }
double absish(double x) { return x >= 0.0 ? x : -x; }
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
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(!asm.contains("\tmovss $"), "{asm}");
    assert!(!asm.contains("\tmovsd $"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_linux_positive_zero_double_returns_use_xmm_zeroing() {
    let src = temp_file("x86-positive-zero-double-returns", "c");
    let out = temp_file("x86-positive-zero-double-returns", "s");
    std::fs::write(
        &src,
        r#"
double g(void) { return 0; }
double h(void) { return 0.0; }
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--optimize", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert_eq!(asm.matches("\txorpd %xmm0, %xmm0").count(), 2, "{asm}");
    assert!(!asm.contains("__float_const_"), "{asm}");
    assert!(!asm.contains("__double_const_"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn x86_64_linux_negative_zero_float_returns_preserve_sign_bit() {
    let src = temp_file("x86-negative-zero-float-returns", "c");
    let out = temp_file("x86-negative-zero-float-returns", "s");
    std::fs::write(
        &src,
        r#"
float f(void) { return -0.0f; }
double g(void) { return -0.0; }
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--optimize", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("__double_const_"), "{asm}");
    assert!(asm.contains("\t.quad 9223372036854775808"), "{asm}");
    assert!(!asm.contains("\txorpd %xmm0, %xmm0"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn aarch64_optimized_float_return_constants_use_float_width() {
    let src = temp_file("aarch64-float-return-constants", "c");
    let out = temp_file("aarch64-float-return-constants", "s");
    std::fs::write(
        &src,
        r#"
float f(void) { return 7; }
double d(void) { return 7; }
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "--optimize", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("\tmovz w9, #0"), "{asm}");
    assert!(asm.contains("\tmovk w9, #16608, lsl #16"), "{asm}");
    assert!(asm.contains("\tfmov s0, w9"), "{asm}");
    assert!(asm.contains("\tmovz x9, #0"), "{asm}");
    assert!(asm.contains("\tmovk x9, #16412, lsl #48"), "{asm}");
    assert!(asm.contains("\tfmov d0, x9"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn aarch64_optimized_float_negative_zero_return_preserves_sign_bit() {
    let src = temp_file("aarch64-float-negative-zero-return", "c");
    let out = temp_file("aarch64-float-negative-zero-return", "s");
    std::fs::write(
        &src,
        r#"
float f(void) { return -0.0f; }
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "aarch64-linux", "--optimize", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm = std::fs::read_to_string(&out).expect("failed to read assembly");
    assert!(asm.contains("\tmovk x9, #32768, lsl #48"), "{asm}");
    assert!(asm.contains("\tfmov d9, x9"), "{asm}");
    assert!(asm.contains("\tfcvt s10, d9"), "{asm}");
    assert!(asm.contains("\tldr s0, [sp]"), "{asm}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
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
fn large_initialized_automatic_arrays_use_memset_zero_fill() {
    let src = temp_file("large-auto-array-memset-zero-fill", "c");
    std::fs::write(
        &src,
        r#"
int main(void) {
    int grid[100][100] = { {}, {4} };
    return grid[1][0];
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
    assert!(tacky.contains("FunCall"));
    assert!(tacky.contains("\"memset\""));
    assert!(tacky.contains("40000"));

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
fn contentless_local_tag_redeclaration_keeps_prior_definition() {
    let src = temp_file("contentless-local-tag-redecl", "c");
    let exe = temp_file("contentless-local-tag-redecl", "bin");
    std::fs::write(
        &src,
        r#"
struct S { int a; int b; };
union U { int i; long l; };
struct S;
union U;

int main(void) {
    struct T { int a; int b; };
    union V { int i; long l; };
    struct T;
    union V;
    struct S s = { 17, 25 };
    union U u = { 13 };
    struct T t = { 8, 34 };
    union V v = { 21 };
    return s.a == 17 && s.b == 25 && u.i == 13
        && t.a == 8 && t.b == 34 && v.i == 21 ? 42 : 1;
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
fn initializes_single_char_array_struct_member_from_string_literal() {
    let src = temp_file("single-char-array-member-string-init", "c");
    let exe = temp_file("single-char-array-member-string-init", "bin");
    std::fs::write(
        &src,
        r#"
struct Buffer { char text[7]; };

int main(void) {
    struct Buffer b = { "abcdef" };
    struct Buffer copy = b;
    return copy.text[0] == 'a' && copy.text[1] == 'b'
        && copy.text[5] == 'f' && copy.text[6] == '\0' ? 42 : 1;
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
fn initializes_struct_array_member_from_struct_expression() {
    let src = temp_file("struct-array-member-struct-expression-init", "c");
    let exe = temp_file("struct-array-member-struct-expression-init", "bin");
    std::fs::write(
        &src,
        r#"
struct Inner {
    double a;
    char b;
    int *p;
};

struct Outer {
    int prefix;
    struct Inner items[3];
    int suffix;
};

int main(void) {
    int value = 9;
    struct Inner inner = { 150.0, -12, &value };
    struct Outer outer = { 5, { inner, { 25.0, 3, &value } }, 7 };

    inner.a += 10.0;
    if (inner.a != 160.0) return 1;
    if (outer.items[0].a != 150.0 || outer.items[0].b != -12) return 2;
    if (*outer.items[0].p != 9) return 3;
    if (outer.items[1].a != 25.0 || outer.items[1].b != 3) return 4;
    if (outer.prefix != 5 || outer.suffix != 7) return 5;
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
fn empty_inline_asm_tied_address_input_copies_to_output() {
    let src = temp_file("empty-asm-tied-address-input-copy", "c");
    let exe = temp_file("empty-asm-tied-address-input-copy", "bin");
    std::fs::write(
        &src,
        r#"
struct S { int a, b; char c[10]; };
const struct S s = { 0, 0, "" };

int main(void) {
    const struct S *p;
    asm ("" : "=r" (p) : "0" (&s));
    return p == &s ? 42 : 1;
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
fn sizeof_inferred_static_string_array_drives_later_array_bound() {
    let src = temp_file("sizeof-inferred-static-string-array", "c");
    let exe = temp_file("sizeof-inferred-static-string-array", "bin");
    std::fs::write(
        &src,
        r#"
static const char data[] = "abcd";

int main(void) {
    unsigned char input[sizeof data + 16] __attribute__((aligned(16)));
    return sizeof input == 21 ? 42 : 1;
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
    float f = __builtin_huge_valf();
    double d = -__builtin_huge_val();
    return __builtin_isinff(f) && __builtin_isinf(d) && !__builtin_isinf(1.0) ? 42 : 1;
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
fn builtin_isinf_lowers_float_and_double_to_finite_limit_checks() {
    let src = temp_file("builtin-isinf-finite-limit-checks", "c");
    std::fs::write(
        &src,
        r#"
int main(float f, double d) {
    return __builtin_isinff(f) + __builtin_isinf(d);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let tacky = stdout(output);
    assert!(tacky.contains("GreaterEqual"), "{tacky}");
    assert!(tacky.contains("LessEqual"), "{tacky}");
    assert!(tacky.contains("3.4028234663852886e38"), "{tacky}");
    assert!(tacky.contains("1.7976931348623157e308"), "{tacky}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn builtin_isinfl_keeps_long_double_precision() {
    let src = temp_file("builtin-isinfl-long-double", "c");
    std::fs::write(
        &src,
        r#"
int main(void) {
    long double x = 1.0L;
    return __builtin_isinfl(x);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let tacky = stdout(output);
    assert!(tacky.contains("LongDouble"), "{tacky}");
    assert!(
        tacky.contains("DoubleConstant(\n                            inf,"),
        "{tacky}"
    );
    assert!(!tacky.contains("Truncate"), "{tacky}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn x86_64_linux_ldbl_max_macro_is_long_double_infinity() {
    let src = temp_file("x86-ldbl-max-target-macro", "c");
    std::fs::write(
        &src,
        r#"
int main(void) {
    long double x = __LDBL_MAX__;
    return __builtin_isinfl(1.01L * x);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let tacky = stdout(output);
    assert!(tacky.contains("LongDouble"), "{tacky}");
    assert!(
        tacky.contains("DoubleConstant(\n                            inf,"),
        "{tacky}"
    );
    assert!(!tacky.contains("Truncate"), "{tacky}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn aarch64_macos_isinfl_uses_double_width_long_double() {
    let src = temp_file("aarch64-macos-isinfl-double-ldbl", "c");
    let asm = temp_file("aarch64-macos-isinfl-double-ldbl", "s");
    std::fs::write(
        &src,
        r#"
extern void abort(void);

static inline int testl(long double b) {
    long double c = 1.01L * b;
    return __builtin_isinfl(c);
}

int main(void) {
    if (testl(__LDBL_MAX__) < 1) abort();
    return 0;
}
"#,
    )
    .expect("failed to write input");

    let tacky_output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "--stage", "tacky"])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    if !tacky_output.status.success() {
        panic!("{}", stderr(tacky_output));
    }
    let tacky = stdout(tacky_output);
    assert!(!tacky.contains("LongDouble"), "{tacky}");
    assert!(!tacky.contains("__multf3"), "{tacky}");
    assert!(!tacky.contains("__eqtf2"), "{tacky}");

    let asm_output = Command::new(rnqcc())
        .args(["--target", "aarch64-macos", "-S"])
        .arg(&src)
        .arg("-o")
        .arg(&asm)
        .output()
        .expect("failed to run rnqcc");

    if !asm_output.status.success() {
        panic!("{}", stderr(asm_output));
    }
    let asm_text = std::fs::read_to_string(&asm).expect("failed to read assembly");
    assert!(!asm_text.contains("__multf3"), "{asm_text}");
    assert!(!asm_text.contains("__eqtf2"), "{asm_text}");

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(asm);
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
fn copy_propagation_preserves_unsigned_long_call_argument_width() {
    let src = temp_file("copy-prop-ulong-call-arg-width", "c");
    let exe = temp_file("copy-prop-ulong-call-arg-width", "bin");
    std::fs::write(
        &src,
        r#"
__attribute__((noinline)) unsigned long id(unsigned long x) { return x; }

double f(void) {
    unsigned long x = id(18446744073709551615UL);
    return (double)x;
}

int main(void) {
    return f() > 1.0e19 ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--optimize")
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
fn copy_propagation_preserves_unsigned_long_conversion_source_width() {
    let src = temp_file("copy-prop-ulong-conv-source-width", "c");
    std::fs::write(
        &src,
        r#"
double f(void) {
    unsigned long x = 18446744073709551615UL;
    return (double)x;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args([
            "--target",
            "aarch64-linux",
            "--optimize",
            "--stage",
            "tacky",
        ])
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let tacky = stdout(output);
    assert!(tacky.contains("UIntToDouble"), "{tacky}");
    assert!(tacky.contains("\"x.0\""), "{tacky}");
    assert!(
        !tacky.contains("UIntToDouble {\n                        src: Constant"),
        "{tacky}"
    );

    let _ = std::fs::remove_file(src);
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
fn supports_reverse_scalar_storage_order_bitfields() {
    let src = temp_file("reverse-scalar-storage-order-bitfields", "c");
    let exe = temp_file("reverse-scalar-storage-order-bitfields", "bin");
    std::fs::write(
        &src,
        r#"
struct S {
    short int i : 12;
    char c1 : 1;
    char c2 : 1;
    char c3 : 1;
    char c4 : 1;
} __attribute__((scalar_storage_order("big-endian")));

int main(void) {
    struct S s = { 341, 1, 1, 1, 1 };
    unsigned char *p = (unsigned char *)&s;
    if (p[0] != 21) return 1;
    if (p[1] != 80) return 2;
    if (s.i != 341 || !s.c1 || !s.c2 || !s.c3 || !s.c4) return 3;
    s.i = 0x123;
    s.c1 = 0;
    s.c4 = 0;
    if (p[0] != 18) return 4;
    if (p[1] != 48) return 5;
    if (p[2] != 96) return 6;
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
fn vla_subscript_uses_element_stride_after_backward_goto() {
    let src = temp_file("vla-subscript-backward-goto", "c");
    let exe = temp_file("vla-subscript-backward-goto", "bin");
    std::fs::write(
        &src,
        r#"
void *volatile sink;

int main(void) {
    int n = 0;
lab:
    {
        int x[n % 8 + 1];
        x[0] = 1;
        x[n % 8] = 2;
        sink = x;
    }
    n++;
    if (n < 256) goto lab;
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
fn builtin_apply_forwards_current_integer_argument() {
    let src = temp_file("builtin-apply-forward-int", "c");
    let exe = temp_file("builtin-apply-forward-int", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);

static void check(int arg) {
    if (arg != 5) abort();
}

static void forward(int arg) {
    __builtin_apply(check, __builtin_apply_args(), 16);
}

int main(void) {
    forward(5);
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
fn variadic_va_arg_struct_temporary_gets_full_storage() {
    let src = temp_file("variadic-va-arg-struct-temp", "c");
    let exe = temp_file("variadic-va-arg-struct-temp", "bin");
    std::fs::write(
        &src,
        r#"
#include <stdarg.h>

void abort(void);

static void take(int size, ...) {
    struct { char x[size]; } d;
    va_list ap;
    int i;
    va_start(ap, size);
    d = va_arg(ap, typeof(d));
    for (i = 0; i < size; i++) {
        if (d.x[i] != '0' + i) abort();
    }
    d = va_arg(ap, typeof(d));
    for (i = 0; i < size; i++) {
        if (d.x[i] != '5' + i) abort();
    }
    va_end(ap);
}

int main(void) {
    int n = 5;
    struct { char x[n]; } a, b;
    a.x[0] = '0'; a.x[1] = '1'; a.x[2] = '2'; a.x[3] = '3'; a.x[4] = '4';
    b.x[0] = '5'; b.x[1] = '6'; b.x[2] = '7'; b.x[3] = '8'; b.x[4] = '9';
    take(n, a, b);
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
fn runs_gcc_pr92904_aligned_variadic_aggregate_shape() {
    let src = temp_file("gcc-pr92904-aligned-varargs", "c");
    let exe = temp_file("gcc-pr92904-aligned-varargs", "bin");
    let out = temp_file("gcc-pr92904-aligned-varargs-x86", "s");
    std::fs::write(
        &src,
        r#"
#include <stdarg.h>

struct __attribute__((aligned(16))) T { long long a, b; };
struct __attribute__((aligned(32))) V { double a, b, c, d; };
struct __attribute__((aligned(16))) X { double a; long long b; };

__attribute__((noinline)) struct T take_t(int skip, ...) {
    va_list ap;
    va_start(ap, skip);
    while (skip--) va_arg(ap, int);
    struct T value = va_arg(ap, struct T);
    va_end(ap);
    return value;
}

__attribute__((noinline)) struct V take_v(int skip, ...) {
    va_list ap;
    va_start(ap, skip);
    while (skip--) va_arg(ap, double);
    struct V value = va_arg(ap, struct V);
    va_end(ap);
    return value;
}

__attribute__((noinline)) struct X take_x(int skip, ...) {
    va_list ap;
    va_start(ap, skip);
    while (skip--) {
        va_arg(ap, int);
        va_arg(ap, double);
    }
    struct X value = va_arg(ap, struct X);
    va_end(ap);
    return value;
}

int main(void) {
    struct T t = { 0x1111111122222222LL, 0x3333333344444444LL };
    struct V v = { 1.25, 2.75, -3.5, -2.0 };
    struct X x = { 9.5, 0x5555555566666666LL };
    struct T rt = take_t(7, 0, 0, 0, 0, 0, 0, 0, t);
    struct V rv = take_v(8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, v);
    struct X rx = take_x(7, 0, 0.0, 0, 0.0, 0, 0.0, 0, 0.0, 0, 0.0, 0, 0.0, 0, 0.0, x);
    if (rt.a != t.a || rt.b != t.b) return 1;
    if (rv.a != v.a || rv.b != v.b || rv.c != v.c || rv.d != v.d) return 2;
    if (rx.a != x.a || rx.b != x.b) return 3;
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

    let x86_output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(x86_output.status.success(), "{}", stderr(x86_output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(exe);
    let _ = std::fs::remove_file(out);
}

#[test]
fn accepts_permissive_implicit_int_file_scope_declarations() {
    let src = temp_file("permissive-implicit-int-file-scope", "c");
    let out = temp_file("permissive-implicit-int-file-scope", "s");
    std::fs::write(
        &src,
        r#"
a, b;
two52 = 4.50359962737049600000e+15;
static c[];
*p;

e() {
    return a + b + c[0] + (p != 0);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn accepts_permissive_missing_final_struct_member_semicolon() {
    let src = temp_file("permissive-missing-struct-member-semi", "c");
    let out = temp_file("permissive-missing-struct-member-semi", "s");
    std::fs::write(
        &src,
        r#"
struct S {
    short a;
    signed b
};

struct S s;
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn extern_inline_body_is_available_for_calls() {
    let src = temp_file("extern-inline-body", "c");
    let exe = temp_file("extern-inline-body", "bin");
    std::fs::write(
        &src,
        r#"
extern inline int add1(int x) {
    return x + 1;
}

int main(void) {
    return add1(41);
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
fn builtin_va_arg_pack_forwards_inline_variadic_tail() {
    let src = temp_file("builtin-va-arg-pack-inline-tail", "c");
    let exe = temp_file("builtin-va-arg-pack-inline-tail", "bin");
    std::fs::write(
        &src,
        r#"
#include <stdarg.h>

void abort(void);

static int seen;

__attribute__((noinline)) int sink(int x, int y, ...) {
    va_list ap;
    int a, b;
    va_start(ap, y);
    a = va_arg(ap, int);
    b = va_arg(ap, int);
    va_end(ap);
    if (x != 3 || y != 6 || a != 5 || b != 9) abort();
    seen = 1;
    return 42;
}

extern inline __attribute__((always_inline, gnu_inline)) int wrap(int x, ...) {
    return sink(x, 6, 5, __builtin_va_arg_pack());
}

int main(void) {
    int result = wrap(3, 9);
    if (!seen) abort();
    return result;
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
fn builtin_va_arg_pack_inline_wrapper_allows_simple_locals() {
    let src = temp_file("builtin-va-arg-pack-inline-locals", "c");
    let exe = temp_file("builtin-va-arg-pack-inline-locals", "bin");
    std::fs::write(
        &src,
        r#"
#include <stdarg.h>

void abort(void);

static int seed;

__attribute__((noinline)) int sink(int x, int y, ...) {
    va_list ap;
    int a;
    va_start(ap, y);
    a = va_arg(ap, int);
    va_end(ap);
    if (x != 4 || y != 7 || a != 31) abort();
    return 42;
}

extern inline __attribute__((always_inline, gnu_inline)) int wrap(int x, ...) {
    int y = seed + 2;
    seed = 5;
    return sink(x, y, __builtin_va_arg_pack());
}

int main(void) {
    seed = 5;
    return wrap(4, 31);
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
fn escaped_nested_nonlocal_goto_uses_captured_parent_state() {
    let src = temp_file("escaped-nested-nonlocal-goto", "c");
    let exe = temp_file("escaped-nested-nonlocal-goto", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);
void exit(int);

static void recursive(int n, void (*proc)(void)) {
    __label__ l1;

    void do_goto(void) {
        goto l1;
    }

    if (n == 3)
        recursive(n - 1, do_goto);
    else if (n > 0)
        recursive(n - 1, proc);
    else
        (*proc)();
    return;

l1:
    if (n == 3)
        exit(42);
    else
        abort();
}

int main(void) {
    recursive(10, abort);
    abort();
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
fn escaped_nested_nonlocal_goto_captures_parent_parameter() {
    let src = temp_file("escaped-nested-nonlocal-goto-param", "c");
    let exe = temp_file("escaped-nested-nonlocal-goto-param", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);
void exit(int);

static void walk(int n, int expected, void (*proc)(void)) {
    __label__ done;

    void jump_done(void) {
        goto done;
    }

    if (n == expected)
        walk(n - 1, expected, jump_done);
    else if (n > 0)
        walk(n - 1, expected, proc);
    else
        (*proc)();
    return;

done:
    if (n == expected)
        exit(42);
    abort();
}

int main(void) {
    walk(8, 4, abort);
    abort();
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
fn nested_function_transitively_forwards_parent_capture() {
    let src = temp_file("nested-function-transitive-capture", "c");
    let exe = temp_file("nested-function-transitive-capture", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);

static long use(long (*func)(long, long), long a, long b) {
    return func(b, a);
}

static long foo(long a, long b, long (*func)(long, long)) {
    return func(a, b);
}

int main(void) {
    long sum = 0;
    long i;

    long nested_0(long a, long b) {
        if (a > 2 * b)
            return a - b;
        return b - a;
    }

    long nested_1(long a, long b) {
        return use(nested_0, b, a) + sum;
    }

    long nested_2(long a, long b) {
        return nested_1(b, a);
    }

    for (i = 0; i < 10; ++i) {
        long j;
        for (j = 0; j < 10; ++j) {
            long k;
            for (k = 0; k < 10; ++k)
                sum += foo(i, j > k ? j - k : k - j, nested_2);
        }
    }

    if ((sum & 0xffffffff) != 0xbecfcbf5)
        abort();
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
fn nested_function_forwards_grandparent_capture_to_inner_nested_function() {
    let src = temp_file("nested-function-grandparent-capture", "c");
    let exe = temp_file("nested-function-grandparent-capture", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);

int main(void) {
    unsigned int x = 0;

    void nested(void) {
        void nested2(void) {
            x += 4;
        }
        nested2();
        nested2();
    }

    nested();
    if (x != 8)
        abort();
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
fn fpermissive_allows_gcc_invalid_pointer_compatibility_cases() {
    let src = temp_file("fpermissive-pointer-compat", "c");
    let out = temp_file("fpermissive-pointer-compat", "s");
    std::fs::write(
        &src,
        r#"
int func(char *);
void callee(const int *, const double *);
void d(void);
void a(void) {}

void (*foo(void))(float) {
    void (*(*x)(void))(float) = d;
    return (*x)();
}

void test(float *fp, const double *dp) {
    long long *lp = a;
    func(fp);
    callee(dp, lp);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-fpermissive")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn driver_normalizes_fpermissive_to_compatibility_mode() {
    let src = temp_file("driver-fpermissive-pointer-compat", "c");
    let out = temp_file("driver-fpermissive-pointer-compat", "s");
    std::fs::write(
        &src,
        r#"
int func(char *);
void test(float *fp) {
    func(fp);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-fpermissive")
        .arg("-S")
        .arg("-o")
        .arg(&out)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

fn x86_64_linux_assembly_regression_cases() -> &'static [(&'static str, &'static str)] {
    &[
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
            "x86-linux-label-address-arithmetic",
            r#"
int tab[4];

void execute(unsigned short *base, unsigned short *ip) {
    int x = 0;
    int *out = tab;
again:
    x++;
    if (x == 4) {
        *out = 0;
        return;
    }
    *out++ = ip - base;
    goto *(&&again + *ip++);
}

int main(void) {
    unsigned short ip[4] = {0, 0, 0, 0};
    execute(ip, ip);
    return tab[0] == 0 && tab[1] == 1 && tab[2] == 2 && tab[3] == 0 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-label-address-table-walk",
            r#"
short optab[5];
char buf[10];

void execute(short *ip) {
    void *base = &&x;
    char *bp = buf;
    static void *tab[] = {&&x, &&y, &&z};
    if (ip == 0) {
        for (int i = 0; i < 3; ++i)
            optab[i] = (short)(tab[i] - base);
        return;
    }
x:
    *bp++ = 'x';
    goto *(base + *ip++);
y:
    *bp++ = 'y';
    goto *(base + *ip++);
z:
    *bp++ = 'z';
    *bp = 0;
}

int main(void) {
    short p[5];
    execute((short *)0);
    p[0] = optab[1];
    p[1] = optab[0];
    p[2] = optab[1];
    p[3] = optab[2];
    execute(p);
    return __builtin_strcmp(buf, "xyxyz") == 0 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-struct-return-copy-abi",
            r#"
struct box { int v[4]; };

struct box make(void) {
    struct box b = {{10, 20, 30, 40}};
    return b;
}

int main(void) {
    struct box b = make();
    return b.v[0] == 10 && b.v[1] == 20 && b.v[2] == 30 && b.v[3] == 40 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-vector-u16-divmod",
            r#"
typedef unsigned short U __attribute__((vector_size(16)));
typedef short S __attribute__((vector_size(16)));

int main(void) {
    U u = (U){73U, 65531U, 8U, 174U, 921U, 65535U, 17U, 178U};
    U q = u / ((U){4U, 4U, 2U, 8U, 16U, 64U, 32U, 128U});
    U r = u % ((U){4U, 4U, 2U, 8U, 16U, 64U, 32U, 128U});
    S s = (S){73, -9123, 32761, 8191, 16371, 1201, 12701, 9999};
    S sq = s / ((S){4, 4, 2, 8, 16, 64, 32, 128});
    S sr = s % ((S){4, 4, 2, 8, 16, 64, 32, 128});
    if (q[0] != 18 || r[1] != 3) return 1;
    if (sq[2] != 16380 || sr[3] != 7) return 2;
    return 42;
}
"#,
        ),
        (
            "x86-linux-word-to-ulong-zero-extend",
            r#"
unsigned long f(unsigned long a, unsigned short b, unsigned long c) {
    return (a + b) - c;
}

int main(void) {
    unsigned long high = 1UL << 63;
    return f(high - 1, 1, high) == 0 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-byte-to-word-sign-extend",
            r#"
signed char c = (signed char)0xffffffff;

int foo(void) {
    return (unsigned short)c ^ (signed char)0x99999999;
}

int main(void) {
    return foo() == (int)0xffff0066 ? 42 : 1;
}
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
        (
            "x86-linux-vprintf-shadow-va-list-bridge",
            r#"
#include <stdarg.h>

int vprintf(const char *fmt, va_list ap);

void forward(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vprintf(fmt, ap);
    va_end(ap);
}
"#,
        ),
        (
            "x86-linux-vfprintf-shadow-va-list-bridge",
            r#"
#include <stdarg.h>

typedef struct FILE FILE;
int vfprintf(FILE *stream, const char *fmt, va_list ap);

void forward(FILE *stream, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    vfprintf(stream, fmt, ap);
    va_end(ap);
}
"#,
        ),
        (
            "x86-linux-vprintf-chk-shadow-va-list-bridge",
            r#"
#include <stdarg.h>

int __vprintf_chk(int flag, const char *fmt, va_list ap);

void forward(const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    __vprintf_chk(1, fmt, ap);
    va_end(ap);
}
"#,
        ),
        (
            "x86-linux-complex-long-double-raw-copies",
            r#"
_Complex long double sink;

_Complex long double id(_Complex long double x) {
    sink = x;
    return sink;
}

int main(void) {
    _Complex long double z = 1.0L + 2.0iL;
    return id(z) == z ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-long-double-x87-does-not-coalesce-with-double",
            r#"
extern void abort(void);

static inline int __attribute__((always_inline)) testf(float b) {
    float c = 1.01f * b;
    return __builtin_isinff(c);
}

static inline int __attribute__((always_inline)) test(double b) {
    double c = 1.01 * b;
    return __builtin_isinf(c);
}

static inline int __attribute__((always_inline)) testl(long double b) {
    long double c = 1.01L * b;
    return __builtin_isinfl(c);
}

int main(void) {
    if (testf(__FLT_MAX__) < 1) abort();
    if (test(__DBL_MAX__) < 1) abort();
    if (testl(__LDBL_MAX__) < 1) abort();
    return 0;
}
"#,
        ),
        (
            "x86-linux-local-vprintf-chk-keeps-shadow-va-list",
            r#"
#include <stdarg.h>

int vprintf(const char *fmt, va_list ap);

int __vprintf_chk(int flag, const char *fmt, va_list ap) {
    return vprintf(fmt, ap);
}

int inner(int x, ...) {
    va_list ap;
    va_start(ap, x);
    int ret = __vprintf_chk(1, "%d", ap);
    va_end(ap);
    return ret;
}
"#,
        ),
        (
            "x86-linux-aligned-struct-shadow-va-arg",
            r#"
#include <stdarg.h>

struct __attribute__((aligned(16))) T { long long a, b; };

struct T take(int skip, ...) {
    va_list ap;
    va_start(ap, skip);
    while (skip--) {
        va_arg(ap, int);
    }
    struct T value = va_arg(ap, struct T);
    va_end(ap);
    return value;
}

int main(void) {
    struct T input = { 11, 22 };
    struct T output = take(1, 0, input);
    return output.a == 11 && output.b == 22 ? 42 : 1;
}
"#,
        ),
        (
            "x86-linux-complex-char-struct-arg-compare",
            r#"
typedef struct { _Complex char a; _Complex char b; } Scc2;

Scc2 s = { 1+2i, 3+4i };

int checkScc2(Scc2 s) {
    return s.a != 1+2i || s.b != 3+4i;
}

int main(void) {
    return checkScc2(s);
}
"#,
        ),
    ]
}

#[track_caller]
fn assert_x86_64_linux_assembly_regression_case(name: &str, source: &str) {
    let src = TempPath::new(name, "c");
    let out = TempPath::new(name, "s");
    std::fs::write(src.path(), source).expect("failed to write input");

    let mut command = Command::new(rnqcc());
    command.args(["--target", "x86_64-linux"]);
    if name == "x86-linux-long-double-x87-does-not-coalesce-with-double" {
        command.arg("--optimize");
    }
    let output = command
        .args(["-S", "-o"])
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
        assert!(asm.contains("16(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("24(%rsp)"), "{name}: {asm}");
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
    if name == "x86-linux-word-to-ulong-zero-extend" {
        assert!(!asm.contains("movzwq %si, %eax"), "{name}: {asm}");
    }
    if name == "x86-linux-byte-to-word-sign-extend" {
        assert!(asm.contains("movsbw"), "{name}: {asm}");
        assert!(!asm.contains("movslq"), "{name}: {asm}");
    }
    if name == "x86-linux-label-address-arithmetic" {
        assert!(asm.contains("jmp *"), "{name}: {asm}");
        assert!(
            !asm.contains("cmpl $4, %eax\n\tmovl $0, %eax")
                && !asm.contains("cmpl $4, %ecx\n\tmovl $0, %ecx")
                && !asm.contains("cmpl $4, %edi\n\tmovl $0, %edi"),
            "{name}: comparison result clobbered the computed-goto loop counter: {asm}"
        );
    }
    if name == "x86-linux-label-address-table-walk" {
        assert!(asm.contains("jmp *"), "{name}: {asm}");
    }
    if name == "x86-linux-struct-return-copy-abi" {
        assert!(asm.contains("call make"), "{name}: {asm}");
        assert!(!asm.contains("PseudoMem"), "{name}: {asm}");
    }
    if name == "x86-linux-vector-u16-divmod" {
        assert!(!asm.contains("Nand"), "{name}: {asm}");
    }
    if name == "x86-linux-vprintf-shadow-va-list-bridge" {
        assert!(asm.contains("subq $32, %rsp"), "{name}: {asm}");
        assert!(asm.contains("movl $48, 0(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("movl $304, 4(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("8(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("movq $0, 16(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("leaq 0(%rsp), %rsi"), "{name}: {asm}");
        assert!(asm.contains("call vprintf"), "{name}: {asm}");
        assert!(asm.contains("addq $32, %rsp"), "{name}: {asm}");
    }
    if name == "x86-linux-vfprintf-shadow-va-list-bridge" {
        assert!(asm.contains("movl $48, 0(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("movl $304, 4(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("leaq 0(%rsp), %rdx"), "{name}: {asm}");
        assert!(asm.contains("call vfprintf"), "{name}: {asm}");
    }
    if name == "x86-linux-vprintf-chk-shadow-va-list-bridge" {
        assert!(asm.contains("movl $48, 0(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("movl $304, 4(%rsp)"), "{name}: {asm}");
        assert!(asm.contains("leaq 0(%rsp), %rdx"), "{name}: {asm}");
        assert!(asm.contains("call __vprintf_chk"), "{name}: {asm}");
    }
    if name == "x86-linux-complex-long-double-raw-copies" {
        assert!(!asm.contains("movt"), "{name}: {asm}");
        assert!(!asm.contains("movups"), "{name}: {asm}");
        assert!(asm.contains("fldt"), "{name}: {asm}");
        assert!(asm.contains("fstpt"), "{name}: {asm}");
    }
    if name == "x86-linux-long-double-x87-does-not-coalesce-with-double" {
        let testl_body = asm
            .split("testl:")
            .nth(1)
            .and_then(|tail| tail.split("\t.text\n\t.globl main").next())
            .unwrap_or(&asm);
        assert!(testl_body.contains("fucomip %st(1), %st"), "{name}: {asm}");
        assert!(
            !asm.contains("\tmulsd -"),
            "{name}: double multiply read an x87 long-double spill slot: {asm}"
        );
        assert!(
            !testl_body.contains("\tmovsd -"),
            "{name}: x87 long-double value was read as an SSE double: {asm}"
        );
    }
    if name == "x86-linux-local-vprintf-chk-keeps-shadow-va-list" {
        assert!(asm.contains("call __vprintf_chk"), "{name}: {asm}");
        assert!(asm.contains("call vprintf"), "{name}: {asm}");
        assert_eq!(
            asm.matches("movl $48, 0(%rsp)").count(),
            1,
            "{name}: local __vprintf_chk call should not be bridged: {asm}"
        );
    }
    if name == "x86-linux-aligned-struct-shadow-va-arg" {
        assert!(asm.contains("subq $32, %rsp"), "{name}: {asm}");
        assert!(asm.contains("leaq 16(%rsp), %rdi"), "{name}: {asm}");
        assert!(asm.contains("rep movsb"), "{name}: {asm}");
    }
    if name == "x86-linux-complex-char-struct-arg-compare" {
        let narrow_extends = asm.matches("movsbl").count() + asm.matches("movzbl").count();
        assert!(
            narrow_extends >= 4,
            "{name}: narrow compare operands were not explicitly extended: {asm}"
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

fn assert_x86_64_linux_assembly_regression_bucket(names: &[&str]) {
    let mut matched = 0usize;
    for &(name, source) in x86_64_linux_assembly_regression_cases() {
        if names.contains(&name) {
            matched += 1;
            assert_x86_64_linux_assembly_regression_case(name, source);
        }
    }
    assert_eq!(matched, names.len(), "unknown x86_64-linux regression case");
}

fn x86_64_linux_assembly_regression_buckets() -> &'static [(&'static str, &'static [&'static str])]
{
    &[
        (
            "int128-vector",
            &[
                "x86-linux-int128-cross-half-shift",
                "x86-linux-int128-va-arg",
                "x86-linux-vector-uint128-mask",
                "x86-linux-i128-eq-low-half",
                "x86-linux-i128-signed-unsigned-stress",
                "x86-linux-i128-vector-lane-stress",
                "x86-linux-vector-u16-divmod",
            ],
        ),
        (
            "computed-goto-struct",
            &[
                "x86-linux-stack-copy-regalloc",
                "x86-linux-nested-local-label",
                "x86-linux-i128-label-uniqueness",
                "x86-linux-label-address-arithmetic",
                "x86-linux-label-address-table-walk",
                "x86-linux-struct-return-copy-abi",
                "x86-linux-complex-char-struct-arg-compare",
            ],
        ),
        (
            "varargs-long-double",
            &[
                "x86-linux-vprintf-shadow-va-list-bridge",
                "x86-linux-vfprintf-shadow-va-list-bridge",
                "x86-linux-vprintf-chk-shadow-va-list-bridge",
                "x86-linux-complex-long-double-raw-copies",
                "x86-linux-long-double-x87-does-not-coalesce-with-double",
                "x86-linux-local-vprintf-chk-keeps-shadow-va-list",
                "x86-linux-aligned-struct-shadow-va-arg",
            ],
        ),
        (
            "scalar-conversion",
            &[
                "x86-linux-word-to-ulong-zero-extend",
                "x86-linux-byte-to-word-sign-extend",
                "x86-linux-signed-long-mul-overflow",
            ],
        ),
    ]
}

fn x86_64_linux_assembly_regression_bucket(name: &str) -> &'static [&'static str] {
    x86_64_linux_assembly_regression_buckets()
        .iter()
        .find_map(|(bucket_name, cases)| (*bucket_name == name).then_some(*cases))
        .expect("unknown x86_64-linux regression bucket")
}

#[test]
fn x86_64_linux_assembly_regression_buckets_cover_each_case_once() {
    let case_names: std::collections::HashSet<_> = x86_64_linux_assembly_regression_cases()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let mut seen = std::collections::HashSet::new();
    for &(bucket_name, cases) in x86_64_linux_assembly_regression_buckets() {
        for &case_name in cases {
            assert!(
                case_names.contains(case_name),
                "{bucket_name}: unknown x86_64-linux regression case {case_name}"
            );
            assert!(
                seen.insert(case_name),
                "{bucket_name}: duplicate x86_64-linux regression case {case_name}"
            );
        }
    }
    assert_eq!(
        seen.len(),
        case_names.len(),
        "x86_64-linux regression case missing from buckets"
    );
}

#[test]
fn emits_x86_64_linux_int128_and_vector_regression_assembly() {
    assert_x86_64_linux_assembly_regression_bucket(x86_64_linux_assembly_regression_bucket(
        "int128-vector",
    ));
}

#[test]
fn emits_x86_64_linux_computed_goto_and_struct_regression_assembly() {
    assert_x86_64_linux_assembly_regression_bucket(x86_64_linux_assembly_regression_bucket(
        "computed-goto-struct",
    ));
}

#[test]
fn emits_x86_64_linux_varargs_and_long_double_regression_assembly() {
    assert_x86_64_linux_assembly_regression_bucket(x86_64_linux_assembly_regression_bucket(
        "varargs-long-double",
    ));
}

#[test]
fn emits_x86_64_linux_scalar_conversion_regression_assembly() {
    assert_x86_64_linux_assembly_regression_bucket(x86_64_linux_assembly_regression_bucket(
        "scalar-conversion",
    ));
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

#[test]
fn recursive_nested_function_nonlocal_goto_unwinds_to_parent_label() {
    let src = temp_file("recursive-nested-nonlocal-goto", "c");
    let exe = temp_file("recursive-nested-nonlocal-goto", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);

static int x(int a) {
    __label__ xlab;

    void y(int a) {
        if (a == 0)
            goto xlab;
        y(a - 1);
    }

    y(a);
xlab:
    return a;
}

int main(void) {
    return x(64) == 64 ? 42 : 1;
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
fn recursive_nested_function_nonlocal_goto_preserves_local_label_address() {
    let src = temp_file("recursive-nested-nonlocal-goto-local-label", "c");
    let exe = temp_file("recursive-nested-nonlocal-goto-local-label", "bin");
    std::fs::write(
        &src,
        r#"
void abort(void);

static int x(int a) {
    __label__ xlab;

    void y(int a) {
        void *local = &&llab;
        if (a == -1)
            goto *local;
        if (a == 0)
            goto xlab;
llab:
        y(a - 1);
    }

    y(a);
xlab:
    return a;
}

int main(void) {
    return x(64) == 64 ? 42 : 1;
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
fn supports_narrow_static_label_difference_initializers() {
    let src = temp_file("narrow-label-diff-init", "c");
    let out = temp_file("narrow-label-diff-init", "o");
    std::fs::write(
        &src,
        r#"
int foo(int a)
{
    static const short offsets[] = { &&l1 - &&l1, &&l2 - &&l1 };
    void *p = &&l1 + offsets[a];
    goto *p;
l1:
    return 1;
l2:
    return 2;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("-c")
        .arg(&src)
        .args(["-o"])
        .arg(&out)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn supports_gnu_single_argument_va_start_for_zero_fixed_variadic_function() {
    let src = temp_file("gnu-single-arg-va-start", "c");
    let exe = temp_file("gnu-single-arg-va-start", "bin");
    std::fs::write(
        &src,
        r#"
long long r;

void qux(...)
{
    __builtin_va_list ap;
    __builtin_va_start(ap);
    if (!r)
        r = __builtin_va_arg(ap, long long);
    else
        r = __builtin_va_arg(ap, int);
}

int main(void)
{
    qux(-2LL, 0);
    if (r != -2LL)
        return 1;
    qux(-2, 0);
    return r == -2 ? 42 : 2;
}
"#,
    )
    .expect("failed to write input");

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
fn internal_cpp_expands_single_argument_va_start_macro() {
    let src = temp_file("single-arg-va-start-macro", "c");
    let exe = temp_file("single-arg-va-start-macro", "bin");
    std::fs::write(
        &src,
        r#"
#include <stdarg.h>

long long r;

void qux(...)
{
    va_list ap;
    va_start(ap);
    if (!r)
        r = va_arg(ap, long long);
    else
        r = va_arg(ap, int);
    va_end(ap);
}

int main(void)
{
    qux(-2LL, 0);
    if (r != -2LL)
        return 1;
    qux(-2, 0);
    return r == -2 ? 42 : 2;
}
"#,
    )
    .expect("failed to write input");

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
fn internal_cpp_uses_virtual_stdarg_before_host_system_header() {
    let dir = TempPath::new("fake-system-stdarg", "d");
    let src = TempPath::new("virtual-stdarg-before-system", "c");
    let exe = TempPath::new("virtual-stdarg-before-system", "bin");
    std::fs::create_dir(dir.path()).expect("failed to create fake system include dir");
    std::fs::write(
        dir.path().join("stdarg.h"),
        "typedef __builtin_va_list va_list;\n\
         void va_start(va_list, ...);\n\
         #define va_arg(ap, type) __builtin_va_arg(ap, type)\n\
         #define va_end(ap) ((void)0)\n",
    )
    .expect("failed to write fake stdarg");
    std::fs::write(
        src.path(),
        r#"
#include <stdarg.h>

long long r;

void qux(...)
{
    va_list ap;
    va_start(ap);
    r = va_arg(ap, long long);
    va_end(ap);
}

int main(void)
{
    qux(-2LL, 0);
    return r == -2LL ? 42 : 1;
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--isystem")
        .arg(dir.path())
        .arg("-o")
        .arg(exe.path())
        .arg(src.path())
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let run = Command::new(exe.path())
        .status()
        .expect("failed to run output");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn internal_cpp_uses_filesystem_header_before_fallback_virtual_header() {
    let dir = TempPath::new("fake-system-stdint", "d");
    let src = TempPath::new("fallback-virtual-stdint-after-system", "c");
    std::fs::create_dir(dir.path()).expect("failed to create fake system include dir");
    std::fs::write(
        dir.path().join("stdint.h"),
        "#define RNQCC_FAKE_STDINT_VALUE 42\n",
    )
    .expect("failed to write fake stdint");
    std::fs::write(
        src.path(),
        "#include <stdint.h>\n\
         int main(void) { return RNQCC_FAKE_STDINT_VALUE; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .arg("--isystem")
        .arg(dir.path())
        .arg("-E")
        .arg(src.path())
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let stdout = stdout(output);
    assert!(stdout.contains("int main(void) { return 42; }"), "{stdout}");
}

#[test]
fn compiles_unicode_local_identifiers() {
    let src = TempPath::new("unicode-identifiers", "c");
    let exe = TempPath::new("unicode-identifiers", "bin");
    std::fs::write(
        src.path(),
        "int main(void) { int α = 40; int β = 2; return α + β; }\n",
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg(src.path())
        .arg("-o")
        .arg(exe.path())
        .output()
        .expect("failed to run rnqcc");
    assert!(output.status.success(), "{}", stderr(output));

    let run = Command::new(exe.path())
        .status()
        .expect("failed to run output");
    assert_eq!(run.code(), Some(42));
}

#[test]
fn compiles_unicode_macro_identifiers_with_both_preprocessors() {
    for internal_cpp in [false, true] {
        let mode = if internal_cpp {
            "internal-cpp"
        } else {
            "external-cpp"
        };
        let src = TempPath::new(&format!("unicode-macro-identifiers-{mode}"), "c");
        let exe = TempPath::new(&format!("unicode-macro-identifiers-{mode}"), "bin");
        std::fs::write(
            src.path(),
            "#define CAT(a, b) a ## b\n\
             #define MAKE_LOCAL(name, value) int name = value\n\
             int main(void) {\n\
                 MAKE_LOCAL(α, 20);\n\
                 MAKE_LOCAL(β, 19);\n\
                 MAKE_LOCAL(CAT(α, β), 3);\n\
                 return α + β + αβ;\n\
             }\n",
        )
        .expect("failed to write input");

        let mut command = Command::new(rnqcc());
        if internal_cpp {
            command.arg("--internal-cpp");
        }
        let output = command
            .arg(src.path())
            .arg("-o")
            .arg(exe.path())
            .output()
            .expect("failed to run rnqcc");
        assert!(output.status.success(), "{mode}: {}", stderr(output));

        let run = Command::new(exe.path())
            .status()
            .expect("failed to run output");
        assert_eq!(run.code(), Some(42), "{mode}");
    }
}

#[test]
fn compiles_unicode_global_symbols_with_both_preprocessors() {
    for internal_cpp in [false, true] {
        let mode = if internal_cpp {
            "internal-cpp"
        } else {
            "external-cpp"
        };
        let src = TempPath::new(&format!("unicode-global-symbols-{mode}"), "c");
        let exe = TempPath::new(&format!("unicode-global-symbols-{mode}"), "bin");
        std::fs::write(
            src.path(),
            "int αβ_global = 40;\n\
             int γδ(void) { return 2; }\n\
             int main(void) { return αβ_global + γδ(); }\n",
        )
        .expect("failed to write input");

        let mut command = Command::new(rnqcc());
        if internal_cpp {
            command.arg("--internal-cpp");
        }
        let output = command
            .arg(src.path())
            .arg("-o")
            .arg(exe.path())
            .output()
            .expect("failed to run rnqcc");
        assert!(output.status.success(), "{mode}: {}", stderr(output));

        let run = Command::new(exe.path())
            .status()
            .expect("failed to run output");
        assert_eq!(run.code(), Some(42), "{mode}");
    }
}

#[test]
fn x86_linux_allows_zero_sized_variadic_memory_arg_block() {
    let src = temp_file("x86-linux-zero-vararg-struct", "c");
    let asm = temp_file("x86-linux-zero-vararg-struct", "s");
    std::fs::write(
        &src,
        r#"
#include <stdarg.h>

typedef struct { char x[0]; } A0;
typedef struct { char x[1]; } A1;

void foo(int size, ...)
{
    va_list ap;
    A0 a0;
    A1 a1;
    va_start(ap, size);
    a0 = va_arg(ap, A0);
    a1 = va_arg(ap, A1);
    va_end(ap);
}

void call(void)
{
    A0 a0;
    A1 a1 = { { 7 } };
    foo(2, a0, a1);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .arg("--internal-cpp")
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(&asm)
        .arg(&src)
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(asm);
}

#[test]
fn x86_linux_large_stack_frame_uses_materialized_adjustment() {
    let src = TempPath::new("x86-linux-large-stack-frame", "c");
    let asm = TempPath::new("x86-linux-large-stack-frame", "s");
    std::fs::write(
        src.path(),
        r#"
void sink(char *);

void f(void)
{
    char s[0x80000000UL];
    s[0] = 'a';
    s[0x80000000UL - 1] = 'b';
    sink(s);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(asm.path())
        .arg(src.path())
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm_text = std::fs::read_to_string(asm.path()).expect("failed to read assembly");
    assert_contains_in_order(
        &asm_text,
        &[
            "movq $2147483648, %r10",
            "subq %r10, %rsp",
            "leaq -2147483648(%rbp)",
        ],
    )
    .expect("large stack frame did not use materialized adjustment");
}

#[test]
fn x86_linux_huge_stack_slot_access_materializes_address() {
    let src = TempPath::new("x86-linux-huge-stack-slot-access", "c");
    let asm = TempPath::new("x86-linux-huge-stack-slot-access", "s");
    std::fs::write(
        src.path(),
        r#"
void sink(char *);

void f(void)
{
    char s[0x10000000000UL];
    s[0] = 'a';
    s[0x10000000000UL - 1] = 'b';
    sink(s);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(asm.path())
        .arg(src.path())
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm_text = std::fs::read_to_string(asm.path()).expect("failed to read assembly");
    assert_contains_in_order(
        &asm_text,
        &["movq $-1099511627776, %r10", "addq %rbp, %r10"],
    )
    .expect("huge stack slot store did not materialize address");
    assert!(
        !asm_text.contains("-1099511627776(%rbp)"),
        "assembly used an unencodable x86-64 stack displacement:\n{asm_text}"
    );
}

#[test]
fn x86_linux_promotes_small_integer_division_to_longword() {
    let src = TempPath::new("x86-linux-small-div", "c");
    let asm = TempPath::new("x86-linux-small-div", "s");
    std::fs::write(
        src.path(),
        r#"
int f(signed char a, signed char b, short c, short d)
{
    return (a / b) + (a % b) + (c / d) + (c % d);
}
"#,
    )
    .expect("failed to write input");

    let output = Command::new(rnqcc())
        .args(["--target", "x86_64-linux", "-S", "-o"])
        .arg(asm.path())
        .arg(src.path())
        .output()
        .expect("failed to run rnqcc");

    assert!(output.status.success(), "{}", stderr(output));
    let asm_text = std::fs::read_to_string(asm.path()).expect("failed to read assembly");
    assert!(asm_text.contains("movsbl"), "{asm_text}");
    assert!(asm_text.contains("movswl"), "{asm_text}");
    assert!(asm_text.contains("idivl"), "{asm_text}");
    assert!(!asm_text.contains("idivb"), "{asm_text}");
    assert!(!asm_text.contains("idivw"), "{asm_text}");
}
