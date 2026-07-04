use clap::{App, Arg};

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use rnqcc::types::*;
use rnqcc::{compile, optimize};

pub fn current_target() -> rnqcc::target::Target {
    rnqcc::target::Target::host()
}

pub fn extension(filename: &str) -> &str {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
}

pub fn input_kind(filename: &str, language: Option<&str>) -> Result<InputKind, String> {
    if let Some(language) = language {
        if language != "c" {
            return Err(format!("unsupported language for -x: {}", language));
        }
        return Ok(InputKind::CSource);
    }
    if filename == "-" {
        return Err("reading from stdin requires -x c".to_string());
    }
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "c" | "h" => Ok(InputKind::CSource),
        "i" => Ok(InputKind::PreprocessedC),
        "s" | "S" => Ok(InputKind::Assembly),
        "o" | "obj" => Ok(InputKind::Object),
        "a" | "so" | "dylib" => Ok(InputKind::Library),
        _ => Err(format!(
            "expected C source, assembly, object, or library input with .c, .h, .i, .s, .S, .o, .obj, .a, .so, or .dylib extension: {}",
            filename
        )),
    }
}

pub fn validate_input(filename: &str, language: Option<&str>) -> Result<(), String> {
    input_kind(filename, language).map(|_| ())
}

pub fn is_compilable_c_input(kind: InputKind) -> bool {
    matches!(kind, InputKind::CSource | InputKind::PreprocessedC)
}

pub fn is_linker_input(kind: InputKind) -> bool {
    matches!(
        kind,
        InputKind::Assembly | InputKind::Object | InputKind::Library
    )
}

pub fn ensure_compilable_c_input(kind: InputKind, stage: &Stage) -> Result<(), String> {
    if !is_compilable_c_input(kind) {
        return Err(format!(
            "{} input cannot be used with {}",
            match kind {
                InputKind::Assembly => "assembly",
                InputKind::Object => "object",
                InputKind::Library => "library",
                InputKind::CSource | InputKind::PreprocessedC => "C",
            },
            match stage {
                Stage::Preprocess => "-E",
                Stage::Lex => "--stage lex",
                Stage::Parse => "--stage parse",
                Stage::Validate => "--stage validate",
                Stage::Tacky => "--stage tacky",
                Stage::Codegen => "--stage codegen",
                Stage::Assembly => "-S",
                Stage::Object => "-c",
                Stage::Executable => "linking",
            }
        ));
    }
    Ok(())
}

pub fn replace_extension(filename: &str, new_extension: &str) -> String {
    let path = Path::new(filename);
    path.with_extension(new_extension)
        .to_string_lossy()
        .into_owned()
}

pub fn same_existing_path(left: &str, right: &str) -> bool {
    if Path::new(left) == Path::new(right) {
        return true;
    }

    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub fn move_or_copy_output(src: &str, dst: &str) -> Result<(), String> {
    if same_existing_path(src, dst) {
        return Ok(());
    }

    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            copy_output(src, dst).map_err(|copy_err| {
                format!(
                    "{}; also failed to rename from {}: {}",
                    copy_err, src, rename_err
                )
            })?;
            std::fs::remove_file(src)
                .map_err(|err| format!("could not remove temporary {}: {}", src, err))?;
            Ok(())
        }
    }
}

pub fn copy_output(src: &str, dst: &str) -> Result<(), String> {
    if same_existing_path(src, dst) {
        return Ok(());
    }
    std::fs::copy(src, dst)
        .map(|_| ())
        .map_err(|err| format!("could not write {}: {}", dst, err))
}

fn describe_exit_status(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {}", code),
        None => "terminated by signal".to_string(),
    }
}

pub fn run_command<I, S>(program: &str, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let args: Vec<OsString> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect();
    let output = Command::new(program)
        .args(&args)
        .output()
        .map_err(|err| format!("failed to run {}: {}", program, err))?;
    if !output.status.success() {
        let rendered_args = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut message = format!(
            "command failed ({}): {} {}",
            describe_exit_status(output.status),
            program,
            rendered_args
        );
        if !stderr.trim().is_empty() {
            message.push_str(&format!("\n{}", stderr.trim_end()));
        }
        if !stdout.trim().is_empty() {
            message.push_str(&format!("\n{}", stdout.trim_end()));
        }
        return Err(message);
    }
    if !output.stderr.is_empty() {
        std::io::stderr()
            .write_all(&output.stderr)
            .map_err(|err| format!("failed to write {} stderr: {}", program, err))?;
    }
    Ok(())
}

pub fn parse_response_file_args(contents: &str) -> Result<Vec<OsString>, String> {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let mut args = Vec::new();
    let mut current = String::new();
    let mut has_arg = false;
    let mut quote = None;
    let mut chars = contents.chars();

    while let Some(ch) = chars.next() {
        if let Some(active_quote) = quote {
            match ch {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        current.push(escaped);
                    } else {
                        current.push('\\');
                    }
                    has_arg = true;
                }
                _ if ch == active_quote => quote = None,
                _ => {
                    current.push(ch);
                    has_arg = true;
                }
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                has_arg = true;
            }
            '\\' => {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                } else {
                    current.push('\\');
                }
                has_arg = true;
            }
            _ if ch.is_whitespace() => {
                if has_arg {
                    args.push(OsString::from(std::mem::take(&mut current)));
                    has_arg = false;
                }
            }
            _ => {
                current.push(ch);
                has_arg = true;
            }
        }
    }

    if let Some(active_quote) = quote {
        return Err(format!(
            "unterminated {} quote in response file",
            active_quote
        ));
    }
    if has_arg {
        args.push(OsString::from(current));
    }
    Ok(args)
}

pub fn expand_response_args<I>(args: I) -> Result<Vec<OsString>, String>
where
    I: IntoIterator<Item = OsString>,
{
    fn expand_one(
        arg: OsString,
        depth: usize,
        base_dir: Option<&Path>,
        stack: &mut Vec<std::path::PathBuf>,
        out: &mut Vec<OsString>,
    ) -> Result<(), String> {
        const MAX_RESPONSE_DEPTH: usize = 16;
        let Some(text) = arg.to_str() else {
            out.push(arg);
            return Ok(());
        };
        let Some(path) = text.strip_prefix('@') else {
            out.push(arg);
            return Ok(());
        };
        if path.is_empty() {
            out.push(arg);
            return Ok(());
        }

        let response_path = Path::new(path);
        let resolved_path = if response_path.is_absolute() {
            response_path.to_path_buf()
        } else if let Some(base_dir) = base_dir {
            base_dir.join(response_path)
        } else {
            response_path.to_path_buf()
        };
        if depth > MAX_RESPONSE_DEPTH {
            return Err(format!(
                "response file nesting is too deep while reading {}",
                resolved_path.display()
            ));
        }
        let stack_path = std::fs::canonicalize(&resolved_path).map_err(|err| {
            format!(
                "could not read response file {}: {}",
                resolved_path.display(),
                err
            )
        })?;
        if stack.contains(&stack_path) {
            return Err(format!(
                "response file cycle while reading {}",
                resolved_path.display()
            ));
        }
        stack.push(stack_path);
        let result = (|| {
            let contents = std::fs::read_to_string(&resolved_path).map_err(|err| {
                format!(
                    "could not read response file {}: {}",
                    resolved_path.display(),
                    err
                )
            })?;
            let nested_base_dir = resolved_path.parent();
            for expanded in parse_response_file_args(&contents)
                .map_err(|err| format!("{}: {}", resolved_path.display(), err))?
            {
                expand_one(expanded, depth + 1, nested_base_dir, stack, out)?;
            }
            Ok(())
        })();
        stack.pop();
        result
    }

    let mut expanded = Vec::new();
    let mut stack = Vec::new();
    for arg in args {
        expand_one(arg, 0, None, &mut stack, &mut expanded)?;
    }
    Ok(expanded)
}

pub fn normalize_driver_arg_text(text: &str) -> Vec<OsString> {
    let mut normalized = Vec::new();
    match text {
        "-Wall" => normalized.push(OsString::from("--Wall")),
        "-Werror" => normalized.push(OsString::from("--Werror")),
        "-Wcompare-distinct-pointer-types" => {
            normalized.push(OsString::from("--Wcompare-distinct-pointer-types"));
        }
        "-Wdeprecated-declarations" => {
            normalized.push(OsString::from("--Wdeprecated-declarations"));
        }
        "-Wno-unreachable" => normalized.push(OsString::from("--Wno-unreachable")),
        "-Wno-missing-return" => normalized.push(OsString::from("--Wno-missing-return")),
        "-Wno-compare-distinct-pointer-types" => {
            normalized.push(OsString::from("--Wno-compare-distinct-pointer-types"));
        }
        "-Wno-deprecated-declarations" => {
            normalized.push(OsString::from("--Wno-deprecated-declarations"));
        }
        "-Wextra" | "-Wpedantic" | "-pedantic" | "-pipe" => {}
        "-O" | "-O1" | "-O2" | "-O3" | "-Os" | "-Oz" | "-Og" | "-Ofast" => {
            normalized.push(OsString::from("--optimize"));
        }
        "-O0" => {}
        "-g" | "-g0" | "-g1" | "-g2" | "-g3" => {}
        "-ansi" | "-std" => {
            normalized.push(OsString::from("--Xpreprocessor"));
            normalized.push(OsString::from(text));
        }
        "-pthread" => {
            normalized.push(OsString::from("--Xpreprocessor"));
            normalized.push(OsString::from(text));
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        "-shared" | "-static" | "-rdynamic" | "-dynamiclib" | "-pie" | "-no-pie" => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        "-fsyntax-only" => {
            normalized.push(OsString::from("--stage"));
            normalized.push(OsString::from("validate"));
        }
        "-L" | "-l" | "-F" | "-framework" | "-Xlinker" => {
            normalized.push(OsString::from("--linker-arg"));
        }
        "-Xassembler" => normalized.push(OsString::from("--assembler-arg")),
        "-MM" => normalized.push(OsString::from("--MM")),
        "-MD" => normalized.push(OsString::from("--MD")),
        "-MG" => normalized.push(OsString::from("--MG")),
        "-MMD" => normalized.push(OsString::from("--MMD")),
        "-MF" => normalized.push(OsString::from("--MF")),
        "-MP" => normalized.push(OsString::from("--MP")),
        "-MT" => normalized.push(OsString::from("--MT")),
        "-MQ" => normalized.push(OsString::from("--MQ")),
        "-dD" => normalized.push(OsString::from("--dump-macro-definitions")),
        "-dM" => normalized.push(OsString::from("--dump-macros")),
        "-H" => normalized.push(OsString::from("--trace-includes")),
        "-P" => normalized.push(OsString::from("--suppress-line-markers")),
        "-nostdinc" => normalized.push(OsString::from("--nostdinc")),
        "-nostdlib" => normalized.push(OsString::from("--nostdlib")),
        "-nodefaultlibs" => normalized.push(OsString::from("--nodefaultlibs")),
        "-isysroot" => normalized.push(OsString::from("--isysroot")),
        "-Xpreprocessor" => normalized.push(OsString::from("--Xpreprocessor")),
        "-imacros" => normalized.push(OsString::from("--imacros")),
        "-include" => normalized.push(OsString::from("--include")),
        "-iquote" => normalized.push(OsString::from("--iquote")),
        "-isystem" => normalized.push(OsString::from("--isystem")),
        "-idirafter" => normalized.push(OsString::from("--idirafter")),
        _ if text.starts_with("-Wl,") => {
            for part in text["-Wl,".len()..]
                .split(',')
                .filter(|part| !part.is_empty())
            {
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from(part));
            }
        }
        _ if text.starts_with("-Xlinker=") => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(&text["-Xlinker=".len()..]));
        }
        _ if text.starts_with("-Xassembler=") => {
            normalized.push(OsString::from("--assembler-arg"));
            normalized.push(OsString::from(&text["-Xassembler=".len()..]));
        }
        _ if text.starts_with("-Wa,") => {
            for part in text["-Wa,".len()..]
                .split(',')
                .filter(|part| !part.is_empty())
            {
                normalized.push(OsString::from("--assembler-arg"));
                normalized.push(OsString::from(part));
            }
        }
        _ if text.starts_with("-W") => {}
        _ if text.starts_with("-isysroot") && text.len() > 9 => {
            normalized.push(OsString::from("--isysroot"));
            normalized.push(OsString::from(&text[9..]));
        }
        _ if text.starts_with("--sysroot=") => {
            normalized.push(OsString::from("--sysroot"));
            normalized.push(OsString::from(&text["--sysroot=".len()..]));
        }
        _ if text.starts_with("-std=") => {
            normalized.push(OsString::from("--Xpreprocessor"));
            normalized.push(OsString::from(text));
        }
        "-fPIC" | "-fpic" | "-fPIE" | "-fpie" => {
            normalized.push(OsString::from("--Xpreprocessor"));
            normalized.push(OsString::from(text));
            normalized.push(OsString::from("--assembler-arg"));
            normalized.push(OsString::from(text));
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        "-finstrument-functions" => {
            normalized.push(OsString::from("--finstrument-functions"));
        }
        "-fpermissive" => {
            normalized.push(OsString::from("--fpermissive"));
        }
        _ if text.starts_with("-fsanitize=")
            || text.starts_with("-fuse-ld=")
            || text.starts_with("-static-lib")
            || text.starts_with("-shared-lib") =>
        {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-f") || text.starts_with("-m") => {
            normalized.push(OsString::from("--Xpreprocessor"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-F") && text.len() > 2 => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-L") && text.len() > 2 => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-l") && text.len() > 2 => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-MF") && text.len() > 3 => {
            normalized.push(OsString::from("--MF"));
            normalized.push(OsString::from(&text[3..]));
        }
        _ if text.starts_with("-MT") && text.len() > 3 => {
            normalized.push(OsString::from("--MT"));
            normalized.push(OsString::from(&text[3..]));
        }
        _ if text.starts_with("-MQ") && text.len() > 3 => {
            normalized.push(OsString::from("--MQ"));
            normalized.push(OsString::from(&text[3..]));
        }
        _ if text.starts_with("-imacros") && text.len() > 8 => {
            normalized.push(OsString::from("--imacros"));
            normalized.push(OsString::from(&text[8..]));
        }
        _ if text.starts_with("-include") && text.len() > 8 => {
            normalized.push(OsString::from("--include"));
            normalized.push(OsString::from(&text[8..]));
        }
        _ if text.starts_with("-iquote") && text.len() > 7 => {
            normalized.push(OsString::from("--iquote"));
            normalized.push(OsString::from(&text[7..]));
        }
        _ if text.starts_with("-isystem") && text.len() > 8 => {
            normalized.push(OsString::from("--isystem"));
            normalized.push(OsString::from(&text[8..]));
        }
        _ if text.starts_with("-idirafter") && text.len() > 10 => {
            normalized.push(OsString::from("--idirafter"));
            normalized.push(OsString::from(&text[10..]));
        }
        _ if text.starts_with("-I") && text.len() > 2 => {
            normalized.push(OsString::from("-I"));
            normalized.push(OsString::from(&text[2..]));
        }
        _ if text.starts_with("-D") && text.len() > 2 => {
            normalized.push(OsString::from("-D"));
            normalized.push(OsString::from(&text[2..]));
        }
        _ if text.starts_with("-U") && text.len() > 2 => {
            normalized.push(OsString::from("-U"));
            normalized.push(OsString::from(&text[2..]));
        }
        _ => normalized.push(OsString::from(text)),
    }
    normalized
}

pub fn normalize_driver_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut normalized = Vec::new();
    for arg in args {
        let Some(text) = arg.to_str() else {
            normalized.push(arg);
            continue;
        };

        if let Some(rest) = text.strip_prefix("-Wp,") {
            for part in rest.split(',').filter(|part| !part.is_empty()) {
                normalized.extend(normalize_driver_arg_text(part));
            }
        } else {
            let normalized_arg = normalize_driver_arg_text(text);
            if normalized_arg.len() == 1 && normalized_arg[0] == arg {
                normalized.push(arg);
            } else {
                normalized.extend(normalized_arg);
            }
        }
    }
    normalized
}

pub fn temp_path_for(src: &str, index: usize, extension: &str) -> String {
    let stem = Path::new(src)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("input");
    std::env::temp_dir()
        .join(format!(
            "rnqcc-{}-{}-{}.{}",
            std::process::id(),
            index,
            stem,
            extension
        ))
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    CSource,
    PreprocessedC,
    Assembly,
    Object,
    Library,
}

mod preprocess_driver;
pub use preprocess_driver::*;

pub struct CompileInvocation<'a> {
    stage: &'a Stage,
    preprocessed_src: &'a str,
    target: &'a Target,
    opt_flags: &'a optimize::OptimizationFlags,
    no_coalescing: bool,
    instrument_functions: bool,
    permissive: bool,
    keep_temps: bool,
    cleanup_preprocessed: bool,
    dumps: compile::DumpOptions,
    warnings: compile::WarningOptions,
}

pub fn do_compile(invocation: CompileInvocation<'_>) -> Result<String, String> {
    let compile_outcome = compile::compile(
        invocation.stage,
        invocation.preprocessed_src,
        compile::CompileOptions {
            target: invocation.target,
            opt_flags: invocation.opt_flags,
            no_coalescing: invocation.no_coalescing,
            instrument_functions: invocation.instrument_functions,
            compatibility: compile::CompatibilityOptions {
                permissive: invocation.permissive,
            },
            dumps: invocation.dumps,
            warnings: invocation.warnings,
        },
    );
    if invocation.cleanup_preprocessed && !invocation.keep_temps {
        let _ = std::fs::remove_file(invocation.preprocessed_src);
    }
    compile_outcome?;
    Ok(replace_extension(invocation.preprocessed_src, "s"))
}

pub fn gcc_arch_args(target: &Target) -> Vec<&'static str> {
    target.cc_arch_args()
}

pub fn external_cpp_target_macro_args(target: &Target) -> Vec<OsString> {
    let mut args = Vec::new();
    for name in [
        "__x86_64__",
        "__amd64__",
        "__aarch64__",
        "__arm64__",
        "__linux__",
        "__linux",
        "linux",
        "__ELF__",
        "__APPLE__",
        "__MACH__",
        "__APPLE_CC__",
        "__APPLE_CPP__",
        "__LDBL_MAX__",
    ] {
        args.push(OsString::from(format!("-U{name}")));
    }
    args.push(OsString::from(format!(
        "-D__LDBL_MAX__={}",
        target_long_double_max_macro(target)
    )));

    match target.arch {
        Arch::X86_64 => {
            args.push(OsString::from("-D__x86_64__=1"));
            args.push(OsString::from("-D__amd64__=1"));
        }
        Arch::AArch64 => {
            args.push(OsString::from("-D__aarch64__=1"));
            args.push(OsString::from("-D__arm64__=1"));
        }
    }

    match target.os {
        TargetOs::Linux => {
            args.push(OsString::from("-D__linux__=1"));
            args.push(OsString::from("-D__linux=1"));
            args.push(OsString::from("-Dlinux=1"));
            args.push(OsString::from("-D__ELF__=1"));
        }
        TargetOs::MacOs => {
            args.push(OsString::from("-D__APPLE__=1"));
            args.push(OsString::from("-D__MACH__=1"));
            args.push(OsString::from("-D__APPLE_CC__=6000"));
            args.push(OsString::from("-D__APPLE_CPP__=1"));
        }
    }

    args
}

pub fn can_assemble_on_host(target: &Target) -> bool {
    target.can_use_host_driver()
}

pub fn stage_accepts_output(stage: &Stage) -> bool {
    stage.accepts_output()
}

pub fn output_requires_single_input(stage: &Stage) -> bool {
    stage.output_requires_single_input()
}

pub struct LinkInvocation<'a> {
    cc: &'a str,
    inputs: &'a [DriverArtifact],
    output: &'a str,
    target: &'a Target,
    sysroot: Option<&'a str>,
    nostdlib: bool,
    nodefaultlibs: bool,
    linker_args: &'a [OsString],
    cleanup: bool,
}

pub fn assemble_and_link(invocation: LinkInvocation<'_>) -> Result<(), String> {
    let mut args: Vec<OsString> = gcc_arch_args(invocation.target)
        .into_iter()
        .map(OsString::from)
        .collect();
    args.extend(
        invocation
            .inputs
            .iter()
            .map(|artifact| OsString::from(&artifact.path)),
    );
    if let Some(sysroot) = invocation.sysroot {
        args.extend([OsString::from("--sysroot"), OsString::from(sysroot)]);
    }
    if invocation.nostdlib {
        args.push(OsString::from("-nostdlib"));
    }
    if invocation.nodefaultlibs {
        args.push(OsString::from("-nodefaultlibs"));
    }
    if invocation.target.os == TargetOs::Linux
        && !invocation
            .linker_args
            .iter()
            .any(|arg| arg == "-pie" || arg == "-no-pie" || arg == "-shared")
    {
        args.push(OsString::from("-no-pie"));
    }
    args.extend(invocation.linker_args.iter().cloned());
    args.extend([OsString::from("-o"), OsString::from(invocation.output)]);
    let result = run_command(invocation.cc, args);

    if invocation.cleanup {
        for artifact in invocation.inputs {
            if artifact.generated {
                let _ = std::fs::remove_file(&artifact.path);
            }
        }
    }
    result
}

pub struct DriverOptions<'a> {
    target: Target,
    debug: bool,
    stage: Stage,
    sources: &'a [&'a str],
    language: Option<&'a str>,
    output: Option<&'a str>,
    opt_flags: &'a optimize::OptimizationFlags,
    cc: &'a str,
    no_coalescing: bool,
    instrument_functions: bool,
    permissive: bool,
    keep_temps: bool,
    internal_cpp: bool,
    include_paths: IncludePaths,
    macro_includes: Vec<PathBuf>,
    forced_includes: Vec<PathBuf>,
    defines: Vec<String>,
    undefs: Vec<String>,
    dumps: compile::DumpOptions,
    warnings: compile::WarningOptions,
    dependency_options: DependencyOptions,
    dump_macros: bool,
    suppress_preprocessed_output: bool,
    trace_includes: bool,
    line_markers: bool,
    sysroot: Option<&'a str>,
    nostdlib: bool,
    nodefaultlibs: bool,
    linker_args: Vec<OsString>,
    assembler_args: Vec<OsString>,
    extra_preprocessor_args: Vec<OsString>,
}

pub struct DriverArtifact {
    src: String,
    path: String,
    generated: bool,
}

pub fn quote_make_word(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            ' ' | '\t' | '#' => {
                out.push('\\');
                out.push(ch);
            }
            '$' => out.push_str("$$"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    out
}

pub fn dependency_target(src: &str, options: &DependencyOptions) -> String {
    if !options.targets.is_empty() {
        return options.targets.join(" ");
    }
    quote_make_word(&replace_extension(src, "o"))
}

pub fn dependency_rule(src: &str, dependencies: &[PathBuf], options: &DependencyOptions) -> String {
    let mut parts = vec![quote_make_word(src)];
    let mut seen = HashSet::new();
    seen.insert(PathBuf::from(src));
    let mut unique_dependencies = Vec::new();
    for dep in dependencies {
        if seen.insert(dep.clone()) {
            parts.push(quote_make_word(&dep.to_string_lossy()));
            unique_dependencies.push(dep);
        }
    }
    let mut rule = format!("{}: {}\n", dependency_target(src, options), parts.join(" "));
    if options.phony_targets {
        for dep in unique_dependencies {
            rule.push('\n');
            rule.push_str(&quote_make_word(&dep.to_string_lossy()));
            rule.push_str(":\n");
        }
    }
    rule
}

pub fn emit_dependency_rule(
    src: &str,
    dependencies: &[PathBuf],
    options: &DependencyOptions,
) -> Result<(), String> {
    let rule = dependency_rule(src, dependencies, options);
    if let Some(path) = &options.file {
        std::fs::write(path, rule).map_err(|err| format!("could not write {}: {}", path, err))
    } else {
        print!("{}", rule);
        Ok(())
    }
}

pub fn write_dependency_side_effect(
    src: &str,
    dependencies: &[PathBuf],
    options: &DependencyOptions,
) -> Result<(), String> {
    let path = options
        .file
        .clone()
        .unwrap_or_else(|| replace_extension(src, "d"));
    std::fs::write(&path, dependency_rule(src, dependencies, options))
        .map_err(|err| format!("could not write {}: {}", path, err))
}

pub fn driver(options: DriverOptions<'_>) -> Result<(), String> {
    let DriverOptions {
        target,
        debug,
        stage,
        sources,
        language,
        output,
        opt_flags,
        cc,
        no_coalescing,
        instrument_functions,
        permissive,
        keep_temps,
        internal_cpp,
        include_paths,
        macro_includes,
        forced_includes,
        defines,
        undefs,
        dumps,
        warnings,
        dependency_options,
        dump_macros,
        suppress_preprocessed_output,
        trace_includes,
        line_markers,
        sysroot,
        nostdlib,
        nodefaultlibs,
        linker_args,
        assembler_args,
        extra_preprocessor_args,
    } = options;

    let input_kinds: Vec<InputKind> = sources
        .iter()
        .map(|src| input_kind(src, language))
        .collect::<Result<_, _>>()?;

    for (src, kind) in sources.iter().zip(input_kinds.iter().copied()) {
        if dependency_options.emit && !is_compilable_c_input(kind) {
            return Err(format!(
                "dependency generation is only supported for C inputs: {}",
                src
            ));
        }
    }

    if sources.iter().filter(|src| **src == "-").count() > 1 {
        return Err("stdin may only be used once".to_string());
    }

    if dependency_options.file.is_some()
        && (dependency_options.emit || dependency_options.side_effect)
        && sources.len() != 1
    {
        return Err("-MF requires exactly one input file".to_string());
    }

    if output.is_some() && !stage_accepts_output(&stage) {
        return Err("-o is only valid when producing preprocessed source, assembly, an object file, or an executable".to_string());
    }

    if output.is_some() && sources.len() != 1 && output_requires_single_input(&stage) {
        let mode = match stage {
            Stage::Preprocess => "-E",
            Stage::Assembly => "-S",
            Stage::Object => "-c",
            _ => {
                return Err(
                    "internal error: output stage does not require a single input".to_string(),
                )
            }
        };
        return Err(format!("-o with {} requires exactly one input file", mode));
    }

    if matches!(stage, Stage::Object | Stage::Executable) && !can_assemble_on_host(&target) {
        return Err(format!(
            "cannot assemble or link target {} with the host compiler driver; use -S to emit assembly",
            target.triple_name()
        ));
    }

    let mut compiled = Vec::new();

    for (index, src) in sources.iter().enumerate() {
        let kind = input_kinds[index];
        if is_linker_input(kind) {
            match stage {
                Stage::Executable => {
                    compiled.push(DriverArtifact {
                        src: src.to_string(),
                        path: src.to_string(),
                        generated: false,
                    });
                    continue;
                }
                Stage::Object if kind == InputKind::Assembly => {
                    compiled.push(DriverArtifact {
                        src: src.to_string(),
                        path: src.to_string(),
                        generated: false,
                    });
                    continue;
                }
                Stage::Assembly if kind == InputKind::Assembly => {
                    compiled.push(DriverArtifact {
                        src: src.to_string(),
                        path: src.to_string(),
                        generated: false,
                    });
                    continue;
                }
                _ => ensure_compilable_c_input(kind, &stage)?,
            }
        }
        let preprocessed_name = preprocess(PreprocessInvocation {
            src,
            index,
            language,
            keep_temps,
            cc,
            internal_cpp,
            include_paths: &include_paths,
            macro_includes: &macro_includes,
            forced_includes: &forced_includes,
            defines: &defines,
            undefs: &undefs,
            target: &target,
            dependency_options: &dependency_options,
            dump_macros,
            suppress_preprocessed_output,
            trace_includes,
            line_markers,
            sysroot,
            extra_preprocessor_args: &extra_preprocessor_args,
        })?;
        if dependency_options.emit && !dependency_options.side_effect {
            emit_dependency_rule(src, &preprocessed_name.dependencies, &dependency_options)?;
            if preprocessed_name.generated && !keep_temps {
                let _ = std::fs::remove_file(&preprocessed_name.path);
            }
            continue;
        }
        if dependency_options.side_effect {
            write_dependency_side_effect(
                src,
                &preprocessed_name.dependencies,
                &dependency_options,
            )?;
        }
        if stage == Stage::Preprocess {
            compiled.push(DriverArtifact {
                src: src.to_string(),
                path: preprocessed_name.path,
                generated: preprocessed_name.generated,
            });
            continue;
        }
        let mut assembly_name = do_compile(CompileInvocation {
            stage: &stage,
            preprocessed_src: &preprocessed_name.path,
            target: &target,
            opt_flags,
            no_coalescing,
            instrument_functions,
            permissive,
            keep_temps,
            cleanup_preprocessed: preprocessed_name.generated,
            dumps,
            warnings,
        })?;
        if stage == Stage::Assembly && output.is_none() {
            let final_asm = replace_extension(src, "s");
            move_or_copy_output(&assembly_name, &final_asm)?;
            assembly_name = final_asm;
        }
        compiled.push(DriverArtifact {
            src: src.to_string(),
            path: assembly_name,
            generated: true,
        });
    }

    if dependency_options.emit && !dependency_options.side_effect {
        return Ok(());
    }

    if stage == Stage::Preprocess {
        if let Some(output_file) = output {
            if compiled.len() != 1 {
                return Err("-o with -E requires exactly one input file".to_string());
            }
            if compiled[0].generated {
                move_or_copy_output(&compiled[0].path, output_file)?;
            } else {
                copy_output(&compiled[0].path, output_file)?;
            }
        } else {
            for artifact in &compiled {
                let contents = std::fs::read_to_string(&artifact.path)
                    .map_err(|err| format!("could not read {}: {}", artifact.path, err))?;
                print!("{}", contents);
                if !contents.ends_with('\n') {
                    println!();
                }
                if artifact.generated && !keep_temps {
                    let _ = std::fs::remove_file(&artifact.path);
                }
            }
        }
    } else if stage == Stage::Assembly {
        if let Some(output_file) = output {
            if compiled.len() != 1 {
                return Err("-o with -S requires exactly one input file".to_string());
            }
            if compiled[0].generated {
                move_or_copy_output(&compiled[0].path, output_file)?;
            } else {
                copy_output(&compiled[0].path, output_file)?;
            }
        }
    } else if stage == Stage::Object {
        if output.is_some() && compiled.len() != 1 {
            return Err("-o with -c requires exactly one input file".to_string());
        }
        // Assemble each .s to .o
        for artifact in &compiled {
            let obj_file = output
                .map(str::to_string)
                .unwrap_or_else(|| replace_extension(&artifact.src, "o"));
            let mut args = gcc_arch_args(&target);
            args.extend(assembler_args.iter().filter_map(|arg| arg.to_str()));
            args.extend(["-c", artifact.path.as_str(), "-o", obj_file.as_str()]);
            if !debug && !keep_temps {
                let result = run_command(cc, args);
                if artifact.generated {
                    let _ = std::fs::remove_file(&artifact.path);
                }
                result?;
            } else {
                run_command(cc, args)?;
            }
        }
    } else if stage == Stage::Executable {
        // Output name is based on the first source file
        let output_file = output.map(str::to_string).unwrap_or_else(|| {
            Path::new(sources[0])
                .with_extension("")
                .to_string_lossy()
                .into_owned()
        });
        assemble_and_link(LinkInvocation {
            cc,
            inputs: &compiled,
            output: &output_file,
            target: &target,
            sysroot,
            nostdlib,
            nodefaultlibs,
            linker_args: &linker_args,
            cleanup: !debug && !keep_temps,
        })?;
    }
    Ok(())
}

pub fn real_main() -> Result<(), String> {
    let args = normalize_driver_args(expand_response_args(std::env::args_os())?);
    let dependency_targets = crate::dependency_targets_from_args(&args);
    let matches = App::new("rnqcc")
        .version("0.2.0")
        .author("Dean Menezes")
        .about("A not-quite-C compiler")
        .arg(
            Arg::with_name("stage")
                .long("stage")
                .takes_value(true)
                .possible_values(Stage::NAMES)
                .conflicts_with("emit_asm")
                .conflicts_with("compile_only")
                .conflicts_with("preprocess_only")
                .help("Run the specified compiler stage"),
        )
        .arg(
            Arg::with_name("emit_asm")
                .short('S')
                .takes_value(false)
                .conflicts_with("compile_only")
                .conflicts_with("preprocess_only")
                .help("Emit assembly (like gcc -S)"),
        )
        .arg(
            Arg::with_name("compile_only")
                .short('c')
                .takes_value(false)
                .conflicts_with("emit_asm")
                .conflicts_with("preprocess_only")
                .help("Compile to object file (like gcc -c)"),
        )
        .arg(
            Arg::with_name("preprocess_only")
                .short('E')
                .takes_value(false)
                .conflicts_with("emit_asm")
                .conflicts_with("compile_only")
                .help("Emit preprocessed source (like gcc -E)"),
        )
        .arg(
            Arg::with_name("dep_only")
                .short('M')
                .takes_value(false)
                .help("Emit makefile dependencies instead of compiling"),
        )
        .arg(
            Arg::with_name("dep_user_only")
                .long("MM")
                .takes_value(false)
                .help("Emit makefile dependencies excluding system headers"),
        )
        .arg(
            Arg::with_name("dep_side_effect")
                .long("MD")
                .takes_value(false)
                .help("Write makefile dependencies as a side effect"),
        )
        .arg(
            Arg::with_name("dep_missing_generated")
                .long("MG")
                .takes_value(false)
                .help("Treat missing headers as generated files in dependency output"),
        )
        .arg(
            Arg::with_name("dep_side_effect_user")
                .long("MMD")
                .takes_value(false)
                .help("Write user-header dependencies as a side effect"),
        )
        .arg(
            Arg::with_name("dep_file")
                .long("MF")
                .takes_value(true)
                .help("Write dependency output to the specified file"),
        )
        .arg(
            Arg::with_name("dep_phony")
                .long("MP")
                .takes_value(false)
                .help("Emit phony make targets for dependency headers"),
        )
        .arg(
            Arg::with_name("dep_target")
                .long("MT")
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Set a dependency target"),
        )
        .arg(
            Arg::with_name("dep_quoted_target")
                .long("MQ")
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Set a make-quoted dependency target"),
        )
        .arg(
            Arg::with_name("dump_macro_definitions")
                .long("dump-macro-definitions")
                .takes_value(false)
                .help("Dump macro definitions while emitting preprocessed source"),
        )
        .arg(
            Arg::with_name("dump_macros")
                .long("dump-macros")
                .takes_value(false)
                .help("Dump macro definitions after preprocessing"),
        )
        .arg(
            Arg::with_name("trace_includes")
                .long("trace-includes")
                .takes_value(false)
                .help("Print include nesting while preprocessing"),
        )
        .arg(
            Arg::with_name("line_markers")
                .long("line-markers")
                .takes_value(false)
                .conflicts_with("suppress_line_markers")
                .help("Emit preprocessor line markers in -E output"),
        )
        .arg(
            Arg::with_name("suppress_line_markers")
                .long("suppress-line-markers")
                .takes_value(false)
                .help("Suppress preprocessor line markers"),
        )
        .arg(
            Arg::with_name("language")
                .short('x')
                .takes_value(true)
                .help("Specify the source language for following inputs"),
        )
        .arg(
            Arg::with_name("output")
                .short('o')
                .takes_value(true)
                .help("Write output to the specified file"),
        )
        .arg(
            Arg::with_name("target")
                .short('t')
                .long("target")
                .takes_value(true)
                .possible_values(Target::ALIASES.map(|(name, _)| name))
                .help("Choose target platform"),
        )
        .arg(
            Arg::with_name("cc")
                .long("cc")
                .takes_value(true)
                .help("C compiler driver to use for preprocessing, assembly, and linking"),
        )
        .arg(
            Arg::with_name("sysroot")
                .long("sysroot")
                .takes_value(true)
                .help("Pass a target sysroot to preprocessing and linking"),
        )
        .arg(
            Arg::with_name("isysroot")
                .long("isysroot")
                .takes_value(true)
                .help("Pass an include sysroot to preprocessing"),
        )
        .arg(
            Arg::with_name("nostdlib")
                .long("nostdlib")
                .takes_value(false)
                .help("Do not link standard startup files or libraries"),
        )
        .arg(
            Arg::with_name("nodefaultlibs")
                .long("nodefaultlibs")
                .takes_value(false)
                .help("Do not link default system libraries"),
        )
        .arg(
            Arg::with_name("linker_arg")
                .long("linker-arg")
                .takes_value(true)
                .allow_hyphen_values(true)
                .multiple(true)
                .number_of_values(1)
                .help("Pass an argument through to the linker driver"),
        )
        .arg(
            Arg::with_name("assembler_arg")
                .long("assembler-arg")
                .takes_value(true)
                .allow_hyphen_values(true)
                .multiple(true)
                .number_of_values(1)
                .help("Pass an argument through to the assembler driver"),
        )
        .arg(
            Arg::with_name("xpreprocessor")
                .long("Xpreprocessor")
                .takes_value(true)
                .allow_hyphen_values(true)
                .multiple(true)
                .number_of_values(1)
                .help("Pass an argument through to the external preprocessor"),
        )
        .arg(
            Arg::with_name("internal_cpp")
                .long("internal-cpp")
                .takes_value(false)
                .help("Use rnqcc's internal preprocessor for self-contained sources and local includes"),
        )
        .arg(
            Arg::with_name("include_path")
                .short('I')
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Add an include search directory for preprocessing"),
        )
        .arg(
            Arg::with_name("macro_include")
                .long("imacros")
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Preprocess a header for macro definitions before each source file"),
        )
        .arg(
            Arg::with_name("forced_include")
                .long("include")
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Preprocess a header before each source file"),
        )
        .arg(
            Arg::with_name("iquote")
                .long("iquote")
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Add a quote-include-only search directory for preprocessing"),
        )
        .arg(
            Arg::with_name("isystem")
                .long("isystem")
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Add a system include search directory for preprocessing"),
        )
        .arg(
            Arg::with_name("idirafter")
                .long("idirafter")
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Add a late system include search directory for preprocessing"),
        )
        .arg(
            Arg::with_name("nostdinc")
                .long("nostdinc")
                .takes_value(false)
                .help("Do not search standard system include directories"),
        )
        .arg(
            Arg::with_name("define")
                .short('D')
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Define a preprocessor macro"),
        )
        .arg(
            Arg::with_name("undefine")
                .short('U')
                .takes_value(true)
                .multiple(true)
                .number_of_values(1)
                .help("Undefine a preprocessor macro"),
        )
        .arg(
            Arg::with_name("print_targets")
                .long("print-targets")
                .takes_value(false)
                .help("Print supported backend targets"),
        )
        .arg(
            Arg::with_name("debug")
                .short('d')
                .long("debug")
                .takes_value(false)
                .help("Write out debug information"),
        )
        .arg(
            Arg::with_name("dump_ast")
                .long("dump-ast")
                .takes_value(false)
                .help("Print the resolved AST to stderr while continuing compilation"),
        )
        .arg(
            Arg::with_name("dump_tacky")
                .long("dump-tacky")
                .takes_value(false)
                .help("Print optimized TACKY IR to stderr while continuing compilation"),
        )
        .arg(
            Arg::with_name("dump_tacky_pre_opt")
                .long("dump-tacky-pre-opt")
                .takes_value(false)
                .help("Print TACKY IR before optimization to stderr while continuing compilation"),
        )
        .arg(
            Arg::with_name("dump_asm_ir")
                .long("dump-asm-ir")
                .takes_value(false)
                .help("Print assembly IR to stderr while continuing compilation"),
        )
        .arg(
            Arg::with_name("source_comments")
                .long("source-comments")
                .takes_value(false)
                .help("Annotate generated assembly with the preprocessed source path"),
        )
        .arg(
            Arg::with_name("wall")
                .long("Wall")
                .takes_value(false)
                .help("Enable warning diagnostics"),
        )
        .arg(
            Arg::with_name("werror")
                .long("Werror")
                .takes_value(false)
                .help("Treat enabled warning diagnostics as errors"),
        )
        .arg(
            Arg::with_name("wno_unreachable")
                .long("Wno-unreachable")
                .takes_value(false)
                .help("Disable unreachable statement warnings"),
        )
        .arg(
            Arg::with_name("wno_missing_return")
                .long("Wno-missing-return")
                .takes_value(false)
                .help("Disable missing return warnings"),
        )
        .arg(
            Arg::with_name("wcompare_distinct_pointer_types")
                .long("Wcompare-distinct-pointer-types")
                .takes_value(false)
                .help("Enable distinct pointer comparison warnings"),
        )
        .arg(
            Arg::with_name("wno_compare_distinct_pointer_types")
                .long("Wno-compare-distinct-pointer-types")
                .takes_value(false)
                .help("Disable distinct pointer comparison warnings"),
        )
        .arg(
            Arg::with_name("wdeprecated_declarations")
                .long("Wdeprecated-declarations")
                .takes_value(false)
                .help("Enable deprecated declaration warnings"),
        )
        .arg(
            Arg::with_name("wno_deprecated_declarations")
                .long("Wno-deprecated-declarations")
                .takes_value(false)
                .help("Disable deprecated declaration warnings"),
        )
        .arg(
            Arg::with_name("keep_temps")
                .long("keep-temps")
                .takes_value(false)
                .help("Keep preprocessed and assembly intermediate files"),
        )
        .arg(
            Arg::with_name("fold_constants")
                .long("fold-constants")
                .takes_value(false)
                .help("Enable constant folding optimization"),
        )
        .arg(
            Arg::with_name("eliminate_unreachable_code")
                .long("eliminate-unreachable-code")
                .takes_value(false)
                .help("Enable unreachable code elimination"),
        )
        .arg(
            Arg::with_name("propagate_copies")
                .long("propagate-copies")
                .takes_value(false)
                .help("Enable copy propagation"),
        )
        .arg(
            Arg::with_name("eliminate_dead_stores")
                .long("eliminate-dead-stores")
                .takes_value(false)
                .help("Enable dead store elimination"),
        )
        .arg(
            Arg::with_name("licm")
                .long("licm")
                .takes_value(false)
                .help("Enable loop-invariant code motion"),
        )
        .arg(
            Arg::with_name("cse")
                .long("cse")
                .takes_value(false)
                .help("Enable common subexpression elimination"),
        )
        .arg(
            Arg::with_name("inline_functions")
                .long("inline-functions")
                .takes_value(false)
                .help("Enable function inlining"),
        )
        .arg(
            Arg::with_name("ipcp")
                .long("ipcp")
                .takes_value(false)
                .help("Enable interprocedural constant propagation"),
        )
        .arg(
            Arg::with_name("optimize")
                .long("optimize")
                .takes_value(false)
                .help("Enable all optimizations"),
        )
        .arg(
            Arg::with_name("no_coalescing")
                .long("no-coalescing")
                .takes_value(false)
                .help("Disable register coalescing"),
        )
        .arg(
            Arg::with_name("instrument_functions")
                .long("finstrument-functions")
                .takes_value(false)
                .help("Emit __cyg_profile_func_enter/exit calls"),
        )
        .arg(
            Arg::with_name("permissive")
                .long("fpermissive")
                .takes_value(false)
                .help("Permit selected GCC invalid-C compatibility cases"),
        )
        .arg(
            Arg::with_name("src_files")
                .index(1)
                .required_unless_present("print_targets")
                .multiple(true)
                .help("Input file(s)"),
        )
        .get_matches_from(args);

    if matches.is_present("print_targets") {
        for target in Target::SUPPORTED {
            println!("{}", target.triple_name());
        }
        return Ok(());
    }

    let dependency_mode_count = [
        "dep_only",
        "dep_user_only",
        "dep_side_effect",
        "dep_side_effect_user",
    ]
    .iter()
    .filter(|name| matches.is_present(name))
    .count();
    if dependency_mode_count > 1 {
        return Err("-M, -MM, -MD, and -MMD are mutually exclusive".to_string());
    }
    if dependency_mode_count == 0
        && (matches.is_present("dep_file")
            || matches.is_present("dep_phony")
            || matches.is_present("dep_target")
            || matches.is_present("dep_quoted_target"))
    {
        return Err("-MF, -MP, -MT, and -MQ require -M, -MM, -MD, or -MMD".to_string());
    }

    let dependency_options = DependencyOptions {
        emit: matches.is_present("dep_only") || matches.is_present("dep_user_only"),
        side_effect: matches.is_present("dep_side_effect")
            || matches.is_present("dep_side_effect_user"),
        user_only: matches.is_present("dep_user_only")
            || matches.is_present("dep_side_effect_user"),
        phony_targets: matches.is_present("dep_phony"),
        missing_headers_generated: matches.is_present("dep_missing_generated"),
        file: matches.value_of("dep_file").map(str::to_string),
        targets: dependency_targets,
    };

    if dependency_options.missing_headers_generated && !dependency_options.emit {
        return Err("-MG requires -M or -MM".to_string());
    }

    let dump_macros =
        matches.is_present("dump_macros") || matches.is_present("dump_macro_definitions");
    let suppress_preprocessed_output =
        matches.is_present("dump_macros") && !matches.is_present("dump_macro_definitions");
    let stage = if matches.is_present("preprocess_only") || dependency_options.emit || dump_macros {
        Stage::Preprocess
    } else if matches.is_present("emit_asm") {
        Stage::Assembly
    } else if matches.is_present("compile_only") {
        Stage::Object
    } else {
        matches
            .value_of("stage")
            .and_then(Stage::parse)
            .unwrap_or(Stage::Executable)
    };

    let target = match matches.value_of("target") {
        Some(target_name) => Target::parse(target_name)
            .ok_or_else(|| format!("unsupported target: {}", target_name))?,
        _ => current_target(),
    };

    let debug = matches.is_present("debug");
    let keep_temps = matches.is_present("keep_temps");
    let src_files: Vec<&str> = matches
        .values_of("src_files")
        .ok_or_else(|| "no input files".to_string())?
        .collect();
    let output = matches.value_of("output");
    let language = matches.value_of("language");
    let sysroot = matches
        .value_of("sysroot")
        .or_else(|| matches.value_of("isysroot"));
    let cc = matches
        .value_of("cc")
        .map(str::to_string)
        .or_else(|| std::env::var("CC").ok())
        .unwrap_or_else(|| "gcc".to_string());

    let all_opts = matches.is_present("optimize");
    let opt_flags = optimize::OptimizationFlags::from_cli(
        all_opts,
        optimize::OptimizationFlagSelections {
            fold_constants: matches.is_present("fold_constants"),
            eliminate_unreachable_code: matches.is_present("eliminate_unreachable_code"),
            propagate_copies: matches.is_present("propagate_copies"),
            eliminate_dead_stores: matches.is_present("eliminate_dead_stores"),
            licm: matches.is_present("licm"),
            eliminate_common_subexpressions: matches.is_present("cse"),
            inline_functions: matches.is_present("inline_functions"),
            interprocedural_constant_propagation: matches.is_present("ipcp"),
        },
    );

    let no_coalescing = matches.is_present("no_coalescing");
    let instrument_functions = matches.is_present("instrument_functions");
    let internal_cpp = matches.is_present("internal_cpp");
    let include_paths = IncludePaths {
        quote: matches
            .values_of("iquote")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        user: matches
            .values_of("include_path")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        system: matches
            .values_of("isystem")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        after: matches
            .values_of("idirafter")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        use_standard_system: !matches.is_present("nostdinc"),
    };
    let macro_includes: Vec<PathBuf> = matches
        .values_of("macro_include")
        .map(|values| values.map(PathBuf::from).collect())
        .unwrap_or_default();
    let forced_includes: Vec<PathBuf> = matches
        .values_of("forced_include")
        .map(|values| values.map(PathBuf::from).collect())
        .unwrap_or_default();
    let defines: Vec<String> = matches
        .values_of("define")
        .map(|values| values.map(str::to_string).collect())
        .unwrap_or_default();
    let undefs: Vec<String> = matches
        .values_of("undefine")
        .map(|values| values.map(str::to_string).collect())
        .unwrap_or_default();
    let linker_args: Vec<OsString> = matches
        .values_of("linker_arg")
        .map(|values| values.map(OsString::from).collect())
        .unwrap_or_default();
    let assembler_args: Vec<OsString> = matches
        .values_of("assembler_arg")
        .map(|values| values.map(OsString::from).collect())
        .unwrap_or_default();
    let extra_preprocessor_args: Vec<OsString> = matches
        .values_of("xpreprocessor")
        .map(|values| values.map(OsString::from).collect())
        .unwrap_or_default();
    let dumps = compile::DumpOptions {
        ast: matches.is_present("dump_ast"),
        tacky_pre_opt: matches.is_present("dump_tacky_pre_opt"),
        tacky: matches.is_present("dump_tacky"),
        asm_ir: matches.is_present("dump_asm_ir"),
        source_comments: matches.is_present("source_comments"),
    };
    let warnings = compile::WarningOptions {
        enabled: true,
        unreachable: !matches.is_present("wno_unreachable"),
        missing_return: !matches.is_present("wno_missing_return"),
        compare_distinct_pointer_types: !matches.is_present("wno_compare_distinct_pointer_types"),
        deprecated_declarations: !matches.is_present("wno_deprecated_declarations"),
        error: matches.is_present("werror"),
    };
    let permissive = matches.is_present("permissive");

    driver(DriverOptions {
        target,
        debug,
        stage,
        sources: &src_files,
        language,
        output,
        opt_flags: &opt_flags,
        cc: &cc,
        no_coalescing,
        instrument_functions,
        permissive,
        keep_temps,
        internal_cpp,
        include_paths,
        macro_includes,
        forced_includes,
        defines,
        undefs,
        dumps,
        warnings,
        dependency_options,
        dump_macros,
        suppress_preprocessed_output,
        trace_includes: matches.is_present("trace_includes"),
        line_markers: matches.is_present("line_markers"),
        sysroot,
        nostdlib: matches.is_present("nostdlib"),
        nodefaultlibs: matches.is_present("nodefaultlibs"),
        linker_args,
        assembler_args,
        extra_preprocessor_args,
    })
}
