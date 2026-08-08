use clap::{Arg, ArgAction, Command};

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus};
use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) {
            if left.is_file()
                && right.is_file()
                && left.dev() == right.dev()
                && left.ino() == right.ino()
            {
                return true;
            }
        }
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
    if let Ok(metadata) = std::fs::symlink_metadata(dst) {
        // Preserve the traditional behavior for device files and symlinks:
        // copying to them writes through the destination instead of replacing
        // the directory entry with a regular file.
        if !metadata.file_type().is_file() {
            return std::fs::copy(src, dst)
                .map(|_| ())
                .map_err(|err| format!("could not write {}: {}", dst, err));
        }
    }

    let (temporary, file) = create_output_copy_temp(dst)?;
    drop(file);
    let temporary_path = temporary.to_string_lossy().into_owned();
    let temporary_guard = rnqcc::tempfile::TempFile::new(&temporary);
    let source_permissions = std::fs::metadata(src)
        .ok()
        .map(|metadata| metadata.permissions());
    let destination_permissions = std::fs::metadata(dst)
        .ok()
        .map(|metadata| metadata.permissions());

    if let Err(error) = std::fs::copy(src, &temporary) {
        return Err(format!("could not write {}: {}", dst, error));
    }
    if let Some(permissions) = destination_permissions.or(source_permissions) {
        if let Err(error) = std::fs::set_permissions(&temporary, permissions) {
            return Err(format!(
                "could not preserve permissions for {}: {}",
                dst, error
            ));
        }
    }
    match std::fs::rename(&temporary, dst) {
        Ok(()) => {}
        #[cfg(windows)]
        Err(rename_error) if Path::new(dst).exists() => {
            // Windows does not replace an existing directory entry with
            // rename. Remove it only after the complete copy has succeeded.
            std::fs::remove_file(dst).map_err(|remove_error| {
                format!(
                    "could not replace {} after staging {}: {}; initial rename failed: {}",
                    dst, temporary_path, remove_error, rename_error
                )
            })?;
            std::fs::rename(&temporary, dst).map_err(|error| {
                format!(
                    "could not publish {} from temporary {}: {}",
                    dst, temporary_path, error
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "could not publish {} from temporary {}: {}",
                dst, temporary_path, error
            ));
        }
    }
    drop(temporary_guard);
    Ok(())
}

pub fn write_text_output(path: &str, contents: &str) -> Result<(), String> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            return std::fs::write(path, contents)
                .map_err(|err| format!("could not write {}: {}", path, err));
        }
    }

    let (temporary, file) = create_output_text_temp(path)?;
    drop(file);
    let temporary_path = temporary.to_string_lossy().into_owned();
    let temporary_guard = rnqcc::tempfile::TempFile::new(&temporary);
    if let Err(error) = std::fs::write(&temporary, contents) {
        return Err(format!("could not write {}: {}", path, error));
    }
    if let Ok(metadata) = std::fs::metadata(path) {
        if let Err(error) = std::fs::set_permissions(&temporary, metadata.permissions()) {
            return Err(format!(
                "could not preserve permissions for {}: {}",
                path, error
            ));
        }
    }
    match std::fs::rename(&temporary, path) {
        Ok(()) => {}
        #[cfg(windows)]
        Err(rename_error) if Path::new(path).exists() => {
            std::fs::remove_file(path).map_err(|remove_error| {
                format!(
                    "could not replace {} after staging {}: {}; initial rename failed: {}",
                    path, temporary_path, remove_error, rename_error
                )
            })?;
            std::fs::rename(&temporary, path).map_err(|error| {
                format!(
                    "could not publish {} from temporary {}: {}",
                    path, temporary_path, error
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "could not publish {} from temporary {}: {}",
                path, temporary_path, error
            ));
        }
    }
    drop(temporary_guard);
    Ok(())
}

static OUTPUT_COPY_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn create_output_copy_temp(dst: &str) -> Result<(PathBuf, std::fs::File), String> {
    create_output_temp(dst, 0o600)
}

fn create_output_text_temp(dst: &str) -> Result<(PathBuf, std::fs::File), String> {
    create_output_temp(dst, 0o666)
}

fn create_output_temp(dst: &str, mode: u32) -> Result<(PathBuf, std::fs::File), String> {
    #[cfg(not(unix))]
    let _ = mode;
    let destination = Path::new(dst);
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let basename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    let process_id = std::process::id();

    for _ in 0..100 {
        let counter = OUTPUT_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{basename}.rnqcc-copy-{process_id}-{counter}"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        match options.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not create temporary output beside {}: {}",
                    dst, error
                ));
            }
        }
    }

    Err(format!(
        "could not reserve a unique temporary output beside {}",
        dst
    ))
}

fn cleanup_generated_artifact(path: &str, generated: bool, keep_temps: bool) {
    if generated && !keep_temps {
        let _ = std::fs::remove_file(path);
    }
}

fn cleanup_generated_artifacts(artifacts: &[DriverArtifact], keep_temps: bool) {
    for artifact in artifacts {
        cleanup_generated_artifact(&artifact.path, artifact.generated, keep_temps);
    }
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
    let output = ProcessCommand::new(program)
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
        // Build systems use `-v` for compiler identification and diagnostics.
        // rnqcc does not currently expose a verbose-driver mode, but it must
        // accept the flag so compiler probes can proceed.
        "-Wextra" | "-Wpedantic" | "-pedantic" | "-pipe" | "-v" => {}
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
                normalized.push(OsString::from(format!("-Wl,{part}")));
            }
        }
        _ if text.starts_with("-Xlinker=") => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(format!(
                "-Wl,{}",
                &text["-Xlinker=".len()..]
            )));
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
        _ if text.starts_with("-isysroot") && text.len() > "-isysroot".len() => {
            normalized.push(OsString::from("--isysroot"));
            normalized.push(OsString::from(&text["-isysroot".len()..]));
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
        _ if text.starts_with("-F") && text.len() > "-F".len() => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-L") && text.len() > "-L".len() => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-l") && text.len() > "-l".len() => {
            normalized.push(OsString::from("--linker-arg"));
            normalized.push(OsString::from(text));
        }
        _ if text.starts_with("-MF") && text.len() > "-MF".len() => {
            normalized.push(OsString::from("--MF"));
            normalized.push(OsString::from(&text["-MF".len()..]));
        }
        _ if text.starts_with("-MT") && text.len() > "-MT".len() => {
            normalized.push(OsString::from("--MT"));
            normalized.push(OsString::from(&text["-MT".len()..]));
        }
        _ if text.starts_with("-MQ") && text.len() > "-MQ".len() => {
            normalized.push(OsString::from("--MQ"));
            normalized.push(OsString::from(&text["-MQ".len()..]));
        }
        _ if text.starts_with("-imacros") && text.len() > "-imacros".len() => {
            normalized.push(OsString::from("--imacros"));
            normalized.push(OsString::from(&text["-imacros".len()..]));
        }
        _ if text.starts_with("-include") && text.len() > "-include".len() => {
            normalized.push(OsString::from("--include"));
            normalized.push(OsString::from(&text["-include".len()..]));
        }
        _ if text.starts_with("-iquote") && text.len() > "-iquote".len() => {
            normalized.push(OsString::from("--iquote"));
            normalized.push(OsString::from(&text["-iquote".len()..]));
        }
        _ if text.starts_with("-isystem") && text.len() > "-isystem".len() => {
            normalized.push(OsString::from("--isystem"));
            normalized.push(OsString::from(&text["-isystem".len()..]));
        }
        _ if text.starts_with("-idirafter") && text.len() > "-idirafter".len() => {
            normalized.push(OsString::from("--idirafter"));
            normalized.push(OsString::from(&text["-idirafter".len()..]));
        }
        _ if text.starts_with("-I") && text.len() > "-I".len() => {
            normalized.push(OsString::from("-I"));
            normalized.push(OsString::from(&text["-I".len()..]));
        }
        _ if text.starts_with("-D") && text.len() > "-D".len() => {
            normalized.push(OsString::from("-D"));
            normalized.push(OsString::from(&text["-D".len()..]));
        }
        _ if text.starts_with("-U") && text.len() > "-U".len() => {
            normalized.push(OsString::from("-U"));
            normalized.push(OsString::from(&text["-U".len()..]));
        }
        _ => normalized.push(OsString::from(text)),
    }
    normalized
}

pub fn normalize_driver_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    fn normalize_separate_arg(flag: &str, value: &str) -> Option<Vec<OsString>> {
        let mut normalized = Vec::new();
        match flag {
            "-std" => {
                normalized.push(OsString::from("--Xpreprocessor"));
                normalized.push(OsString::from(format!("-std={value}")));
            }
            "-isysroot" => {
                normalized.push(OsString::from("--isysroot"));
                normalized.push(OsString::from(value));
            }
            "-Xpreprocessor" => {
                normalized.push(OsString::from("--Xpreprocessor"));
                normalized.push(OsString::from(value));
            }
            "-Xlinker" => {
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from(format!("-Wl,{value}")));
            }
            "-Xassembler" => {
                normalized.push(OsString::from("--assembler-arg"));
                normalized.push(OsString::from(value));
            }
            "-imacros" => {
                normalized.push(OsString::from("--imacros"));
                normalized.push(OsString::from(value));
            }
            "-include" => {
                normalized.push(OsString::from("--include"));
                normalized.push(OsString::from(value));
            }
            "-iquote" => {
                normalized.push(OsString::from("--iquote"));
                normalized.push(OsString::from(value));
            }
            "-isystem" => {
                normalized.push(OsString::from("--isystem"));
                normalized.push(OsString::from(value));
            }
            "-idirafter" => {
                normalized.push(OsString::from("--idirafter"));
                normalized.push(OsString::from(value));
            }
            "-MF" => {
                normalized.push(OsString::from("--MF"));
                normalized.push(OsString::from(value));
            }
            "-MT" => {
                normalized.push(OsString::from("--MT"));
                normalized.push(OsString::from(value));
            }
            "-MQ" => {
                normalized.push(OsString::from("--MQ"));
                normalized.push(OsString::from(value));
            }
            "-I" | "-D" | "-U" => {
                normalized.push(OsString::from(flag));
                normalized.push(OsString::from(value));
            }
            "-L" => {
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from(format!("-L{value}")));
            }
            "-l" => {
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from(format!("-l{value}")));
            }
            "-F" => {
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from(format!("-F{value}")));
            }
            "-framework" => {
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from("-framework"));
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from(value));
            }
            "-arch" => {
                normalized.push(OsString::from("--Xpreprocessor"));
                normalized.push(OsString::from("-arch"));
                normalized.push(OsString::from("--Xpreprocessor"));
                normalized.push(OsString::from(value));
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from("-arch"));
                normalized.push(OsString::from("--linker-arg"));
                normalized.push(OsString::from(value));
            }
            _ => return None,
        }
        Some(normalized)
    }

    let args: Vec<OsString> = args.into_iter().collect();
    let mut normalized = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if let (Some(flag), Some(value)) = (
            args[index].to_str(),
            args.get(index + 1).and_then(|arg| arg.to_str()),
        ) {
            if let Some(pair) = normalize_separate_arg(flag, value) {
                normalized.extend(pair);
                index += 2;
                continue;
            }
        }

        let arg = args[index].clone();
        let Some(text) = arg.to_str() else {
            normalized.push(arg);
            index += 1;
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
        index += 1;
    }
    normalized
}

pub fn temp_path_for(src: &str, index: usize, extension: &str) -> Result<String, String> {
    static TEMP_PATH_COUNTER: AtomicUsize = AtomicUsize::new(0);
    let stem = Path::new(src)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("input");
    let directory = std::env::temp_dir();
    for _ in 0..100 {
        let counter = TEMP_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = directory.join(format!(
            "rnqcc-{}-{}-{}-{}.{}",
            std::process::id(),
            index,
            stem,
            counter,
            extension
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                drop(file);
                return Ok(candidate.to_string_lossy().into_owned());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not reserve temporary path {}: {}",
                    candidate.to_string_lossy(),
                    error
                ));
            }
        }
    }

    Err(format!(
        "could not reserve a unique temporary path in {}",
        directory.to_string_lossy()
    ))
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
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            ' ' | '\t' | '#' | '*' | '?' | '[' | ']' | '%' => {
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
    let mut parts = Vec::with_capacity(dependencies.len() + 1);
    parts.push(quote_make_word(src));
    let mut seen = HashSet::with_capacity(dependencies.len() + 1);
    seen.insert(PathBuf::from(src));
    let mut unique_dependencies = Vec::with_capacity(dependencies.len());
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
        write_text_output(path, &rule)
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
    write_text_output(&path, &dependency_rule(src, dependencies, options))
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
                _ => {
                    if let Err(error) = ensure_compilable_c_input(kind, &stage) {
                        cleanup_generated_artifacts(&compiled, keep_temps);
                        return Err(error);
                    }
                }
            }
        }
        let preprocessed_name = match preprocess(PreprocessInvocation {
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
        }) {
            Ok(preprocessed_name) => preprocessed_name,
            Err(error) => {
                cleanup_generated_artifacts(&compiled, keep_temps);
                return Err(error);
            }
        };
        if dependency_options.emit && !dependency_options.side_effect {
            if let Err(error) =
                emit_dependency_rule(src, &preprocessed_name.dependencies, &dependency_options)
            {
                cleanup_generated_artifact(
                    &preprocessed_name.path,
                    preprocessed_name.generated,
                    keep_temps,
                );
                return Err(error);
            }
            cleanup_generated_artifact(
                &preprocessed_name.path,
                preprocessed_name.generated,
                keep_temps,
            );
            continue;
        }
        if dependency_options.side_effect {
            if let Err(error) = write_dependency_side_effect(
                src,
                &preprocessed_name.dependencies,
                &dependency_options,
            ) {
                cleanup_generated_artifact(
                    &preprocessed_name.path,
                    preprocessed_name.generated,
                    keep_temps,
                );
                return Err(error);
            }
        }
        if stage == Stage::Preprocess {
            compiled.push(DriverArtifact {
                src: src.to_string(),
                path: preprocessed_name.path,
                generated: preprocessed_name.generated,
            });
            continue;
        }
        let mut assembly_name = match do_compile(CompileInvocation {
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
        }) {
            Ok(assembly_name) => assembly_name,
            Err(error) => {
                cleanup_generated_artifacts(&compiled, keep_temps);
                return Err(error);
            }
        };
        let mut assembly_generated = true;
        if stage == Stage::Assembly && output.is_none() {
            let final_asm = replace_extension(src, "s");
            if let Err(error) = move_or_copy_output(&assembly_name, &final_asm) {
                cleanup_generated_artifact(&assembly_name, true, keep_temps);
                return Err(error);
            }
            assembly_name = final_asm;
            assembly_generated = false;
        }
        compiled.push(DriverArtifact {
            src: src.to_string(),
            path: assembly_name,
            generated: assembly_generated,
        });
    }

    if dependency_options.emit && !dependency_options.side_effect {
        return Ok(());
    }

    if stage == Stage::Preprocess {
        if let Some(output_file) = output {
            if compiled.len() != 1 {
                cleanup_generated_artifacts(&compiled, keep_temps);
                return Err("-o with -E requires exactly one input file".to_string());
            }
            if compiled[0].generated {
                if let Err(error) = move_or_copy_output(&compiled[0].path, output_file) {
                    cleanup_generated_artifact(&compiled[0].path, true, keep_temps);
                    return Err(error);
                }
            } else {
                copy_output(&compiled[0].path, output_file)?;
            }
        } else {
            for artifact in &compiled {
                let contents = match std::fs::read_to_string(&artifact.path) {
                    Ok(contents) => contents,
                    Err(error) => {
                        cleanup_generated_artifact(&artifact.path, artifact.generated, keep_temps);
                        return Err(format!("could not read {}: {}", artifact.path, error));
                    }
                };
                print!("{}", contents);
                if !contents.ends_with('\n') {
                    println!();
                }
                cleanup_generated_artifact(&artifact.path, artifact.generated, keep_temps);
            }
        }
    } else if stage == Stage::Assembly {
        if let Some(output_file) = output {
            if compiled.len() != 1 {
                cleanup_generated_artifacts(&compiled, keep_temps);
                return Err("-o with -S requires exactly one input file".to_string());
            }
            if compiled[0].generated {
                if let Err(error) = move_or_copy_output(&compiled[0].path, output_file) {
                    cleanup_generated_artifact(&compiled[0].path, true, keep_temps);
                    return Err(error);
                }
            } else {
                copy_output(&compiled[0].path, output_file)?;
            }
        }
    } else if stage == Stage::Object {
        if output.is_some() && compiled.len() != 1 {
            cleanup_generated_artifacts(&compiled, keep_temps);
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
                if let Err(error) = result {
                    cleanup_generated_artifacts(&compiled, keep_temps);
                    return Err(error);
                }
            } else {
                if let Err(error) = run_command(cc, args) {
                    cleanup_generated_artifacts(&compiled, keep_temps || debug);
                    return Err(error);
                }
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
    let matches = Command::new("rnqcc")
        .version("0.2.0")
        .author("Dean Menezes")
        .about("A not-quite-C compiler")
        .arg(
            Arg::new("stage")
                .long("stage")
                .num_args(1)
                .value_parser(clap::builder::PossibleValuesParser::new(Stage::NAMES))
                .conflicts_with("emit_asm")
                .conflicts_with("compile_only")
                .conflicts_with("preprocess_only")
                .help("Run the specified compiler stage"),
        )
        .arg(
            Arg::new("emit_asm")
                .short('S')
                .action(ArgAction::SetTrue)
                .conflicts_with("compile_only")
                .conflicts_with("preprocess_only")
                .help("Emit assembly (like gcc -S)"),
        )
        .arg(
            Arg::new("compile_only")
                .short('c')
                .action(ArgAction::SetTrue)
                .conflicts_with("emit_asm")
                .conflicts_with("preprocess_only")
                .help("Compile to object file (like gcc -c)"),
        )
        .arg(
            Arg::new("preprocess_only")
                .short('E')
                .action(ArgAction::SetTrue)
                .conflicts_with("emit_asm")
                .conflicts_with("compile_only")
                .help("Emit preprocessed source (like gcc -E)"),
        )
        .arg(
            Arg::new("dep_only")
                .short('M')
                .action(ArgAction::SetTrue)
                .help("Emit makefile dependencies instead of compiling"),
        )
        .arg(
            Arg::new("dep_user_only")
                .long("MM")
                .action(ArgAction::SetTrue)
                .help("Emit makefile dependencies excluding system headers"),
        )
        .arg(
            Arg::new("dep_side_effect")
                .long("MD")
                .action(ArgAction::SetTrue)
                .help("Write makefile dependencies as a side effect"),
        )
        .arg(
            Arg::new("dep_missing_generated")
                .long("MG")
                .action(ArgAction::SetTrue)
                .help("Treat missing headers as generated files in dependency output"),
        )
        .arg(
            Arg::new("dep_side_effect_user")
                .long("MMD")
                .action(ArgAction::SetTrue)
                .help("Write user-header dependencies as a side effect"),
        )
        .arg(
            Arg::new("dep_file")
                .long("MF")
                .num_args(1)
                .help("Write dependency output to the specified file"),
        )
        .arg(
            Arg::new("dep_phony")
                .long("MP")
                .action(ArgAction::SetTrue)
                .help("Emit phony make targets for dependency headers"),
        )
        .arg(
            Arg::new("dep_target")
                .long("MT")
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Set a dependency target"),
        )
        .arg(
            Arg::new("dep_quoted_target")
                .long("MQ")
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Set a make-quoted dependency target"),
        )
        .arg(
            Arg::new("dump_macro_definitions")
                .long("dump-macro-definitions")
                .action(ArgAction::SetTrue)
                .help("Dump macro definitions while emitting preprocessed source"),
        )
        .arg(
            Arg::new("dump_macros")
                .long("dump-macros")
                .action(ArgAction::SetTrue)
                .help("Dump macro definitions after preprocessing"),
        )
        .arg(
            Arg::new("trace_includes")
                .long("trace-includes")
                .action(ArgAction::SetTrue)
                .help("Print include nesting while preprocessing"),
        )
        .arg(
            Arg::new("line_markers")
                .long("line-markers")
                .action(ArgAction::SetTrue)
                .conflicts_with("suppress_line_markers")
                .help("Emit preprocessor line markers in -E output"),
        )
        .arg(
            Arg::new("suppress_line_markers")
                .long("suppress-line-markers")
                .action(ArgAction::SetTrue)
                .help("Suppress preprocessor line markers"),
        )
        .arg(
            Arg::new("language")
                .short('x')
                .num_args(1)
                .help("Specify the source language for following inputs"),
        )
        .arg(
            Arg::new("output")
                .short('o')
                .num_args(1)
                .help("Write output to the specified file"),
        )
        .arg(
            Arg::new("target")
                .short('t')
                .long("target")
                .num_args(1)
                .value_parser(clap::builder::PossibleValuesParser::new(
                    Target::ALIASES.map(|(name, _)| name),
                ))
                .help("Choose target platform"),
        )
        .arg(
            Arg::new("cc")
                .long("cc")
                .num_args(1)
                .help("C compiler driver to use for preprocessing, assembly, and linking"),
        )
        .arg(
            Arg::new("sysroot")
                .long("sysroot")
                .num_args(1)
                .help("Pass a target sysroot to preprocessing and linking"),
        )
        .arg(
            Arg::new("isysroot")
                .long("isysroot")
                .num_args(1)
                .help("Pass an include sysroot to preprocessing"),
        )
        .arg(
            Arg::new("nostdlib")
                .long("nostdlib")
                .action(ArgAction::SetTrue)
                .help("Do not link standard startup files or libraries"),
        )
        .arg(
            Arg::new("nodefaultlibs")
                .long("nodefaultlibs")
                .action(ArgAction::SetTrue)
                .help("Do not link default system libraries"),
        )
        .arg(
            Arg::new("linker_arg")
                .long("linker-arg")
                .num_args(1)
                .allow_hyphen_values(true)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Pass an argument through to the linker driver"),
        )
        .arg(
            Arg::new("assembler_arg")
                .long("assembler-arg")
                .num_args(1)
                .allow_hyphen_values(true)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Pass an argument through to the assembler driver"),
        )
        .arg(
            Arg::new("xpreprocessor")
                .long("Xpreprocessor")
                .num_args(1)
                .allow_hyphen_values(true)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Pass an argument through to the external preprocessor"),
        )
        .arg(
            Arg::new("internal_cpp")
                .long("internal-cpp")
                .action(ArgAction::SetTrue)
                .help("Use rnqcc's internal preprocessor for self-contained sources and local includes"),
        )
        .arg(
            Arg::new("include_path")
                .short('I')
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Add an include search directory for preprocessing"),
        )
        .arg(
            Arg::new("macro_include")
                .long("imacros")
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Preprocess a header for macro definitions before each source file"),
        )
        .arg(
            Arg::new("forced_include")
                .long("include")
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Preprocess a header before each source file"),
        )
        .arg(
            Arg::new("iquote")
                .long("iquote")
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Add a quote-include-only search directory for preprocessing"),
        )
        .arg(
            Arg::new("isystem")
                .long("isystem")
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Add a system include search directory for preprocessing"),
        )
        .arg(
            Arg::new("idirafter")
                .long("idirafter")
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Add a late system include search directory for preprocessing"),
        )
        .arg(
            Arg::new("nostdinc")
                .long("nostdinc")
                .action(ArgAction::SetTrue)
                .help("Do not search standard system include directories"),
        )
        .arg(
            Arg::new("define")
                .short('D')
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Define a preprocessor macro"),
        )
        .arg(
            Arg::new("undefine")
                .short('U')
                .num_args(1)
                .action(ArgAction::Append)
                .num_args(1)
                .help("Undefine a preprocessor macro"),
        )
        .arg(
            Arg::new("print_targets")
                .long("print-targets")
                .action(ArgAction::SetTrue)
                .help("Print supported backend targets"),
        )
        .arg(
            Arg::new("debug")
                .short('d')
                .long("debug")
                .action(ArgAction::SetTrue)
                .help("Write out debug information"),
        )
        .arg(
            Arg::new("dump_ast")
                .long("dump-ast")
                .action(ArgAction::SetTrue)
                .help("Print the resolved AST to stderr while continuing compilation"),
        )
        .arg(
            Arg::new("dump_tacky")
                .long("dump-tacky")
                .action(ArgAction::SetTrue)
                .help("Print optimized TACKY IR to stderr while continuing compilation"),
        )
        .arg(
            Arg::new("dump_tacky_pre_opt")
                .long("dump-tacky-pre-opt")
                .action(ArgAction::SetTrue)
                .help("Print TACKY IR before optimization to stderr while continuing compilation"),
        )
        .arg(
            Arg::new("dump_asm_ir")
                .long("dump-asm-ir")
                .action(ArgAction::SetTrue)
                .help("Print assembly IR to stderr while continuing compilation"),
        )
        .arg(
            Arg::new("source_comments")
                .long("source-comments")
                .action(ArgAction::SetTrue)
                .help("Annotate generated assembly with the preprocessed source path"),
        )
        .arg(
            Arg::new("wall")
                .long("Wall")
                .action(ArgAction::SetTrue)
                .help("Enable warning diagnostics"),
        )
        .arg(
            Arg::new("werror")
                .long("Werror")
                .action(ArgAction::SetTrue)
                .help("Treat enabled warning diagnostics as errors"),
        )
        .arg(
            Arg::new("wno_unreachable")
                .long("Wno-unreachable")
                .action(ArgAction::SetTrue)
                .help("Disable unreachable statement warnings"),
        )
        .arg(
            Arg::new("wno_missing_return")
                .long("Wno-missing-return")
                .action(ArgAction::SetTrue)
                .help("Disable missing return warnings"),
        )
        .arg(
            Arg::new("wcompare_distinct_pointer_types")
                .long("Wcompare-distinct-pointer-types")
                .action(ArgAction::SetTrue)
                .help("Enable distinct pointer comparison warnings"),
        )
        .arg(
            Arg::new("wno_compare_distinct_pointer_types")
                .long("Wno-compare-distinct-pointer-types")
                .action(ArgAction::SetTrue)
                .help("Disable distinct pointer comparison warnings"),
        )
        .arg(
            Arg::new("wdeprecated_declarations")
                .long("Wdeprecated-declarations")
                .action(ArgAction::SetTrue)
                .help("Enable deprecated declaration warnings"),
        )
        .arg(
            Arg::new("wno_deprecated_declarations")
                .long("Wno-deprecated-declarations")
                .action(ArgAction::SetTrue)
                .help("Disable deprecated declaration warnings"),
        )
        .arg(
            Arg::new("keep_temps")
                .long("keep-temps")
                .action(ArgAction::SetTrue)
                .help("Keep preprocessed and assembly intermediate files"),
        )
        .arg(
            Arg::new("fold_constants")
                .long("fold-constants")
                .action(ArgAction::SetTrue)
                .help("Enable constant folding optimization"),
        )
        .arg(
            Arg::new("eliminate_unreachable_code")
                .long("eliminate-unreachable-code")
                .action(ArgAction::SetTrue)
                .help("Enable unreachable code elimination"),
        )
        .arg(
            Arg::new("propagate_copies")
                .long("propagate-copies")
                .action(ArgAction::SetTrue)
                .help("Enable copy propagation"),
        )
        .arg(
            Arg::new("eliminate_dead_stores")
                .long("eliminate-dead-stores")
                .action(ArgAction::SetTrue)
                .help("Enable dead store elimination"),
        )
        .arg(
            Arg::new("licm")
                .long("licm")
                .action(ArgAction::SetTrue)
                .help("Enable loop-invariant code motion"),
        )
        .arg(
            Arg::new("cse")
                .long("cse")
                .action(ArgAction::SetTrue)
                .help("Enable common subexpression elimination"),
        )
        .arg(
            Arg::new("inline_functions")
                .long("inline-functions")
                .action(ArgAction::SetTrue)
                .help("Enable function inlining"),
        )
        .arg(
            Arg::new("ipcp")
                .long("ipcp")
                .action(ArgAction::SetTrue)
                .help("Enable interprocedural constant propagation"),
        )
        .arg(
            Arg::new("optimize")
                .long("optimize")
                .action(ArgAction::SetTrue)
                .help("Enable all optimizations"),
        )
        .arg(
            Arg::new("no_coalescing")
                .long("no-coalescing")
                .action(ArgAction::SetTrue)
                .help("Disable register coalescing"),
        )
        .arg(
            Arg::new("instrument_functions")
                .long("finstrument-functions")
                .action(ArgAction::SetTrue)
                .help("Emit __cyg_profile_func_enter/exit calls"),
        )
        .arg(
            Arg::new("permissive")
                .long("fpermissive")
                .action(ArgAction::SetTrue)
                .help("Permit selected GCC invalid-C compatibility cases"),
        )
        .arg(
            Arg::new("src_files")
                .index(1)
                .required_unless_present("print_targets")
                .action(ArgAction::Append)
                .help("Input file(s)"),
        )
        .get_matches_from(args);

    if matches.get_flag("print_targets") {
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
    .filter(|name| matches.get_flag(name))
    .count();
    if dependency_mode_count > 1 {
        return Err("-M, -MM, -MD, and -MMD are mutually exclusive".to_string());
    }
    if dependency_mode_count == 0
        && (matches.get_one::<String>("dep_file").is_some()
            || matches.get_flag("dep_phony")
            || matches.get_many::<String>("dep_target").is_some()
            || matches.get_many::<String>("dep_quoted_target").is_some())
    {
        return Err("-MF, -MP, -MT, and -MQ require -M, -MM, -MD, or -MMD".to_string());
    }

    let dependency_options = DependencyOptions {
        emit: matches.get_flag("dep_only") || matches.get_flag("dep_user_only"),
        side_effect: matches.get_flag("dep_side_effect")
            || matches.get_flag("dep_side_effect_user"),
        user_only: matches.get_flag("dep_user_only") || matches.get_flag("dep_side_effect_user"),
        phony_targets: matches.get_flag("dep_phony"),
        missing_headers_generated: matches.get_flag("dep_missing_generated"),
        file: matches
            .get_one::<String>("dep_file")
            .map(String::as_str)
            .map(str::to_string),
        targets: dependency_targets,
    };

    if dependency_options.missing_headers_generated && !dependency_options.emit {
        return Err("-MG requires -M or -MM".to_string());
    }

    let dump_macros = matches.get_flag("dump_macros") || matches.get_flag("dump_macro_definitions");
    let suppress_preprocessed_output =
        matches.get_flag("dump_macros") && !matches.get_flag("dump_macro_definitions");
    let stage = if matches.get_flag("preprocess_only") || dependency_options.emit || dump_macros {
        Stage::Preprocess
    } else if matches.get_flag("emit_asm") {
        Stage::Assembly
    } else if matches.get_flag("compile_only") {
        Stage::Object
    } else {
        matches
            .get_one::<String>("stage")
            .map(String::as_str)
            .and_then(Stage::parse)
            .unwrap_or(Stage::Executable)
    };

    let target = match matches.get_one::<String>("target").map(String::as_str) {
        Some(target_name) => Target::parse(target_name)
            .ok_or_else(|| format!("unsupported target: {}", target_name))?,
        _ => current_target(),
    };

    let debug = matches.get_flag("debug");
    let keep_temps = matches.get_flag("keep_temps");
    let src_files: Vec<&str> = matches
        .get_many::<String>("src_files")
        .ok_or_else(|| "no input files".to_string())?
        .map(String::as_str)
        .collect();
    let output = matches.get_one::<String>("output").map(String::as_str);
    let language = matches.get_one::<String>("language").map(String::as_str);
    let sysroot = matches
        .get_one::<String>("sysroot")
        .map(String::as_str)
        .or_else(|| matches.get_one::<String>("isysroot").map(String::as_str));
    let cc = matches
        .get_one::<String>("cc")
        .map(String::as_str)
        .map(str::to_string)
        .or_else(|| std::env::var("CC").ok())
        .unwrap_or_else(|| "gcc".to_string());

    let all_opts = matches.get_flag("optimize");
    let opt_flags = optimize::OptimizationFlags::from_cli(
        all_opts,
        optimize::OptimizationFlagSelections {
            fold_constants: matches.get_flag("fold_constants"),
            eliminate_unreachable_code: matches.get_flag("eliminate_unreachable_code"),
            propagate_copies: matches.get_flag("propagate_copies"),
            eliminate_dead_stores: matches.get_flag("eliminate_dead_stores"),
            licm: matches.get_flag("licm"),
            eliminate_common_subexpressions: matches.get_flag("cse"),
            inline_functions: matches.get_flag("inline_functions"),
            interprocedural_constant_propagation: matches.get_flag("ipcp"),
        },
    );

    let no_coalescing = matches.get_flag("no_coalescing");
    let instrument_functions = matches.get_flag("instrument_functions");
    let internal_cpp = matches.get_flag("internal_cpp");
    let include_paths = IncludePaths {
        quote: matches
            .get_many::<String>("iquote")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        user: matches
            .get_many::<String>("include_path")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        system: matches
            .get_many::<String>("isystem")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        after: matches
            .get_many::<String>("idirafter")
            .map(|values| values.map(PathBuf::from).collect())
            .unwrap_or_default(),
        use_standard_system: !matches.get_flag("nostdinc"),
    };
    let macro_includes: Vec<PathBuf> = matches
        .get_many::<String>("macro_include")
        .map(|values| values.map(PathBuf::from).collect())
        .unwrap_or_default();
    let forced_includes: Vec<PathBuf> = matches
        .get_many::<String>("forced_include")
        .map(|values| values.map(PathBuf::from).collect())
        .unwrap_or_default();
    let defines: Vec<String> = matches
        .get_many::<String>("define")
        .map(|values| values.map(|value| value.to_string()).collect())
        .unwrap_or_default();
    let undefs: Vec<String> = matches
        .get_many::<String>("undefine")
        .map(|values| values.map(|value| value.to_string()).collect())
        .unwrap_or_default();
    let linker_args: Vec<OsString> = matches
        .get_many::<String>("linker_arg")
        .map(|values| values.map(OsString::from).collect())
        .unwrap_or_default();
    let assembler_args: Vec<OsString> = matches
        .get_many::<String>("assembler_arg")
        .map(|values| values.map(OsString::from).collect())
        .unwrap_or_default();
    let extra_preprocessor_args: Vec<OsString> = matches
        .get_many::<String>("xpreprocessor")
        .map(|values| values.map(OsString::from).collect())
        .unwrap_or_default();
    let dumps = compile::DumpOptions {
        ast: matches.get_flag("dump_ast"),
        tacky_pre_opt: matches.get_flag("dump_tacky_pre_opt"),
        tacky: matches.get_flag("dump_tacky"),
        asm_ir: matches.get_flag("dump_asm_ir"),
        source_comments: matches.get_flag("source_comments"),
    };
    let warnings = compile::WarningOptions {
        enabled: true,
        unreachable: !matches.get_flag("wno_unreachable"),
        missing_return: !matches.get_flag("wno_missing_return"),
        compare_distinct_pointer_types: !matches.get_flag("wno_compare_distinct_pointer_types"),
        deprecated_declarations: !matches.get_flag("wno_deprecated_declarations"),
        error: matches.get_flag("werror"),
    };
    let permissive = matches.get_flag("permissive");

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
        trace_includes: matches.get_flag("trace_includes"),
        line_markers: matches.get_flag("line_markers"),
        sysroot,
        nostdlib: matches.get_flag("nostdlib"),
        nodefaultlibs: matches.get_flag("nodefaultlibs"),
        linker_args,
        assembler_args,
        extra_preprocessor_args,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separate_driver_option_values() {
        let args = normalize_driver_args([
            OsString::from("-std"),
            OsString::from("c11"),
            OsString::from("-Xlinker"),
            OsString::from("-z"),
            OsString::from("-include"),
            OsString::from("config.h"),
            OsString::from("-L"),
            OsString::from("lib"),
            OsString::from("-l"),
            OsString::from("m"),
        ]);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            vec![
                "--Xpreprocessor",
                "-std=c11",
                "--linker-arg",
                "-Wl,-z",
                "--include",
                "config.h",
                "--linker-arg",
                "-Llib",
                "--linker-arg",
                "-lm",
            ]
        );
    }

    #[test]
    fn normalizes_cmake_arch_option() {
        let args = normalize_driver_args([OsString::from("-arch"), OsString::from("arm64")]);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            vec![
                "--Xpreprocessor",
                "-arch",
                "--Xpreprocessor",
                "arm64",
                "--linker-arg",
                "-arch",
                "--linker-arg",
                "arm64",
            ]
        );
    }

    #[test]
    fn accepts_cmake_verbose_driver_option() {
        assert!(normalize_driver_args([OsString::from("-v")]).is_empty());
    }

    #[test]
    fn preserves_glued_isysroot_path() {
        let args = normalize_driver_args([
            OsString::from("-isysroot/usr/local/sysroot"),
            OsString::from("-isysroot=/opt/sysroot"),
        ]);
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            vec![
                "--isysroot",
                "/usr/local/sysroot",
                "--isysroot",
                "=/opt/sysroot",
            ]
        );
    }

    #[test]
    fn temporary_paths_are_unique_and_reserved() -> Result<(), String> {
        let first = temp_path_for("collision.c", 0, "i")?;
        let second = temp_path_for("collision.c", 0, "i")?;
        assert_ne!(first, second);
        assert!(Path::new(&first).is_file());
        assert!(Path::new(&second).is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&first)
                    .map_err(|err| err.to_string())?
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_file(first).map_err(|err| err.to_string())?;
        std::fs::remove_file(second).map_err(|err| err.to_string())?;
        Ok(())
    }

    #[test]
    fn text_output_replaces_contents_and_preserves_permissions() -> Result<(), String> {
        let path = temp_path_for("dependency", 0, "d")?;
        std::fs::write(&path, "old\n").map_err(|err| err.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
                .map_err(|err| err.to_string())?;
        }

        write_text_output(&path, "new\n")?;
        assert_eq!(
            std::fs::read_to_string(&path).map_err(|err| err.to_string())?,
            "new\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path)
                    .map_err(|err| err.to_string())?
                    .permissions()
                    .mode()
                    & 0o777,
                0o640
            );
        }
        std::fs::remove_file(path).map_err(|err| err.to_string())?;
        Ok(())
    }

    #[test]
    fn make_words_escape_wildcards_and_pattern_markers() {
        assert_eq!(
            quote_make_word("dir/a*b?[header]%name.h"),
            "dir/a\\*b\\?\\[header\\]\\%name.h"
        );
    }

    #[cfg(unix)]
    #[test]
    fn temporary_path_reservation_does_not_follow_symlinks() -> Result<(), String> {
        use std::os::unix::fs::symlink;

        let first = temp_path_for("symlink.c", 0, "i")?;
        std::fs::remove_file(&first).map_err(|err| err.to_string())?;
        let sentinel = format!("{first}.sentinel");
        std::fs::write(&sentinel, "untouched").map_err(|err| err.to_string())?;
        symlink(&sentinel, &first).map_err(|err| err.to_string())?;

        let second = temp_path_for("symlink.c", 0, "i")?;
        assert_ne!(first, second);
        assert!(Path::new(&first).is_symlink());
        assert_eq!(
            std::fs::read_to_string(&sentinel).map_err(|err| err.to_string())?,
            "untouched"
        );

        std::fs::remove_file(first).map_err(|err| err.to_string())?;
        std::fs::remove_file(second).map_err(|err| err.to_string())?;
        std::fs::remove_file(sentinel).map_err(|err| err.to_string())?;
        Ok(())
    }
}
