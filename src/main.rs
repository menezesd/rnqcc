use clap::{App, Arg};

mod backend;
mod cfg;
mod compile;
mod diagnostic;
mod lex;
mod optimize;
mod parse;
mod preprocess;
mod resolve;
mod tacky;
mod tempfile;
mod types;

use crate::types::*;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

fn current_target() -> Target {
    Target::host()
}

fn extension(filename: &str) -> &str {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    CSource,
    PreprocessedC,
    Assembly,
    Object,
    Library,
}

fn input_kind(filename: &str, language: Option<&str>) -> Result<InputKind, String> {
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

fn validate_input(filename: &str, language: Option<&str>) -> Result<(), String> {
    input_kind(filename, language).map(|_| ())
}

fn is_compilable_c_input(kind: InputKind) -> bool {
    matches!(kind, InputKind::CSource | InputKind::PreprocessedC)
}

fn is_linker_input(kind: InputKind) -> bool {
    matches!(
        kind,
        InputKind::Assembly | InputKind::Object | InputKind::Library
    )
}

fn ensure_compilable_c_input(kind: InputKind, stage: &Stage) -> Result<(), String> {
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

fn replace_extension(filename: &str, new_extension: &str) -> String {
    let path = Path::new(filename);
    path.with_extension(new_extension)
        .to_string_lossy()
        .into_owned()
}

fn same_existing_path(left: &str, right: &str) -> bool {
    if Path::new(left) == Path::new(right) {
        return true;
    }

    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn move_or_copy_output(src: &str, dst: &str) -> Result<(), String> {
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

fn copy_output(src: &str, dst: &str) -> Result<(), String> {
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

fn run_command<I, S>(program: &str, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
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

fn parse_response_file_args(contents: &str) -> Result<Vec<OsString>, String> {
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

fn expand_response_args<I>(args: I) -> Result<Vec<OsString>, String>
where
    I: IntoIterator<Item = OsString>,
{
    fn expand_one(
        arg: OsString,
        depth: usize,
        base_dir: Option<&Path>,
        out: &mut Vec<OsString>,
    ) -> Result<(), String> {
        const MAX_RESPONSE_DEPTH: usize = 16;
        if depth > MAX_RESPONSE_DEPTH {
            return Err("response file nesting is too deep".to_string());
        }

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
        let contents = std::fs::read_to_string(&resolved_path).map_err(|err| {
            format!(
                "could not read response file {}: {}",
                resolved_path.display(),
                err
            )
        })?;
        let nested_base_dir = resolved_path.parent();
        for expanded in parse_response_file_args(&contents)? {
            expand_one(expanded, depth + 1, nested_base_dir, out)?;
        }
        Ok(())
    }

    let mut expanded = Vec::new();
    for arg in args {
        expand_one(arg, 0, None, &mut expanded)?;
    }
    Ok(expanded)
}

fn normalize_driver_arg_text(text: &str) -> Vec<OsString> {
    let mut normalized = Vec::new();
    match text {
        "-Wall" => normalized.push(OsString::from("--Wall")),
        "-Werror" => normalized.push(OsString::from("--Werror")),
        "-Wno-unreachable" => normalized.push(OsString::from("--Wno-unreachable")),
        "-Wno-missing-return" => normalized.push(OsString::from("--Wno-missing-return")),
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

fn normalize_driver_args<I>(args: I) -> Vec<OsString>
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

fn dependency_targets_from_args(args: &[OsString]) -> Vec<String> {
    let mut targets = Vec::new();
    let mut index = 0usize;
    while let Some(arg) = args.get(index) {
        let Some(text) = arg.to_str() else {
            index += 1;
            continue;
        };
        match text {
            "--MT" => {
                if let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) {
                    targets.push(value.to_string());
                }
                index += 2;
            }
            "--MQ" => {
                if let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) {
                    targets.push(quote_make_word(value));
                }
                index += 2;
            }
            _ if text.starts_with("--MT=") => {
                targets.push(text["--MT=".len()..].to_string());
                index += 1;
            }
            _ if text.starts_with("--MQ=") => {
                targets.push(quote_make_word(&text["--MQ=".len()..]));
                index += 1;
            }
            _ => index += 1,
        }
    }
    targets
}

fn temp_path_for(src: &str, index: usize, extension: &str) -> String {
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum MacroDef {
    Object(String),
    Function {
        params: Vec<String>,
        variadic: bool,
        body: String,
    },
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn splice_continued_lines(source: &str) -> String {
    let mut out = String::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some('\n') => {
                    chars.next();
                    continue;
                }
                Some('\r') => {
                    chars.next();
                    if matches!(chars.peek(), Some('\n')) {
                        chars.next();
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(ch);
    }
    out
}

fn strip_comments(source: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' | '\'' => {
                out.push(ch);
                let quote = ch;
                let mut escaped = false;
                for inner in chars.by_ref() {
                    out.push(inner);
                    if escaped {
                        escaped = false;
                    } else if inner == '\\' {
                        escaped = true;
                    } else if inner == quote {
                        break;
                    }
                }
            }
            '/' if matches!(chars.peek(), Some('/')) => {
                chars.next();
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if matches!(chars.peek(), Some('*')) => {
                chars.next();
                let mut closed = false;
                let mut previous = '\0';
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        out.push('\n');
                    } else if previous == '*' && inner == '/' {
                        closed = true;
                        break;
                    }
                    previous = inner;
                }
                if !closed {
                    return Err("unterminated block comment in preprocessor input".to_string());
                }
                out.push(' ');
            }
            _ => out.push(ch),
        }
    }
    Ok(out)
}

fn escape_c_string(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

struct PreprocessorState {
    counter: usize,
    base_file: String,
    date: String,
    time: String,
}

fn civil_date_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn format_c_date_time(seconds: i64) -> (String, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_days(days);
    let hour = second_of_day / 3600;
    let minute = (second_of_day % 3600) / 60;
    let second = second_of_day % 60;
    (
        format!("{} {:>2} {}", MONTHS[(month - 1) as usize], day, year),
        format!("{:02}:{:02}:{:02}", hour, minute, second),
    )
}

impl PreprocessorState {
    fn new(base_file: String) -> Self {
        let seconds = std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_secs() as i64)
            })
            .unwrap_or(0);
        let (date, time) = format_c_date_time(seconds);
        Self {
            counter: 0,
            base_file,
            date,
            time,
        }
    }
}

fn expand_macros_with_context(
    line: &str,
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<String, String> {
    expand_macros_with_tokens(line, macros, file, line_number, include_level, state)
}

fn expand_macros_with_tokens(
    line: &str,
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<String, String> {
    let tokens = preprocess::lexer::lex(line)?;
    let token_macros = token_macro_table(macros)?;
    let mut hooks = LiveMacroExpansionHooks {
        file,
        line_number,
        include_level,
        state,
    };
    let expanded =
        preprocess::macro_expand::expand_macros_with_hooks(&tokens, &token_macros, &mut hooks)?;
    Ok(preprocess::emit::emit_tokens(&expanded))
}

fn token_macro_table(
    macros: &HashMap<String, MacroDef>,
) -> Result<preprocess::macro_expand::MacroTable, String> {
    let mut table = preprocess::macro_expand::MacroTable::new();
    for (name, def) in macros {
        table.insert(name.clone(), string_macro_def_to_token(def)?);
    }
    Ok(table)
}

fn string_macro_def_to_token(def: &MacroDef) -> Result<preprocess::macro_expand::MacroDef, String> {
    match def {
        MacroDef::Object(body) => Ok(preprocess::macro_expand::MacroDef::Object(
            preprocess::lexer::lex(body)?,
        )),
        MacroDef::Function {
            params,
            variadic,
            body,
        } => Ok(preprocess::macro_expand::MacroDef::Function {
            params: params.clone(),
            variadic: *variadic,
            body: preprocess::lexer::lex(body)?,
        }),
    }
}

fn macro_defs_equivalent(left: &MacroDef, right: &MacroDef) -> Result<bool, String> {
    match (left, right) {
        (MacroDef::Object(left_body), MacroDef::Object(right_body)) => {
            Ok(replacement_tokens_equivalent(left_body, right_body)?)
        }
        (
            MacroDef::Function {
                params: left_params,
                variadic: left_variadic,
                body: left_body,
            },
            MacroDef::Function {
                params: right_params,
                variadic: right_variadic,
                body: right_body,
            },
        ) => Ok(left_params == right_params
            && left_variadic == right_variadic
            && replacement_tokens_equivalent(left_body, right_body)?),
        _ => Ok(false),
    }
}

fn replacement_tokens_equivalent(left: &str, right: &str) -> Result<bool, String> {
    let left = preprocess::lexer::lex(left)?;
    let right = preprocess::lexer::lex(right)?;
    Ok(non_ws_token_texts(&left).eq(non_ws_token_texts(&right)))
}

fn non_ws_token_texts(tokens: &[preprocess::token::PpToken]) -> impl Iterator<Item = &str> {
    tokens.iter().filter_map(|token| match &token.kind {
        preprocess::token::PpTokenKind::Whitespace(_)
        | preprocess::token::PpTokenKind::Newline(_) => None,
        _ => Some(token.text()),
    })
}

struct LiveMacroExpansionHooks<'a> {
    file: &'a str,
    line_number: usize,
    include_level: usize,
    state: &'a mut PreprocessorState,
}

impl preprocess::macro_expand::MacroExpansionHooks for LiveMacroExpansionHooks<'_> {
    fn expand_unknown_ident(
        &mut self,
        token: &preprocess::token::PpToken,
        name: &str,
    ) -> Result<Option<Vec<preprocess::token::PpToken>>, String> {
        let replacement = match name {
            "__LINE__" => self
                .line_number
                .saturating_add(token.span.start.line.saturating_sub(1))
                .to_string(),
            "__FILE__" => format!("\"{}\"", escape_c_string(self.file)),
            "__BASE_FILE__" => format!("\"{}\"", escape_c_string(&self.state.base_file)),
            "__INCLUDE_LEVEL__" => self.include_level.to_string(),
            "__COUNTER__" => {
                let value = self.state.counter.to_string();
                self.state.counter += 1;
                value
            }
            "__DATE__" => format!("\"{}\"", self.state.date),
            "__TIME__" => format!("\"{}\"", self.state.time),
            _ => return Ok(None),
        };
        preprocess::lexer::lex(&replacement).map(Some)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum IncludeSpec {
    Quoted(String),
    Angled(String),
}

#[derive(Clone, Debug)]
struct IncludePaths {
    quote: Vec<PathBuf>,
    user: Vec<PathBuf>,
    system: Vec<PathBuf>,
    after: Vec<PathBuf>,
    use_standard_system: bool,
}

impl Default for IncludePaths {
    fn default() -> Self {
        Self {
            quote: Vec::new(),
            user: Vec::new(),
            system: Vec::new(),
            after: Vec::new(),
            use_standard_system: true,
        }
    }
}

impl IncludePaths {
    fn append_system_defaults(&mut self, target: &Target) {
        if self.use_standard_system {
            for dir in default_system_include_dirs(target) {
                if !self.system.iter().any(|existing| existing == &dir) {
                    self.system.push(dir);
                }
            }
        }
    }

    fn quoted_dirs<'a>(&'a self, base_dir: &'a Path) -> Vec<PathBuf> {
        std::iter::once(base_dir.to_path_buf())
            .chain(self.quote.iter().cloned())
            .chain(self.user.iter().cloned())
            .chain(self.system.iter().cloned())
            .chain(self.after.iter().cloned())
            .collect()
    }

    fn angled_dirs(&self) -> Vec<PathBuf> {
        self.user
            .iter()
            .cloned()
            .chain(self.system.iter().cloned())
            .chain(self.after.iter().cloned())
            .collect()
    }

    fn include_next_dirs(&self, base_dir: &Path) -> Vec<PathBuf> {
        let dirs: Vec<PathBuf> = self
            .quote
            .iter()
            .cloned()
            .chain(self.user.iter().cloned())
            .chain(self.system.iter().cloned())
            .chain(self.after.iter().cloned())
            .collect();
        let start = dirs
            .iter()
            .position(|dir| same_include_dir(dir, base_dir))
            .map(|index| index + 1)
            .unwrap_or(0);
        dirs[start..].to_vec()
    }
}

fn parse_include_tokens(
    tokens: &[preprocess::token::PpToken],
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Option<IncludeSpec> {
    strict_include_spec_from_tokens(tokens).or_else(|| {
        let token_macros = token_macro_table(macros).ok()?;
        let mut hooks = LiveMacroExpansionHooks {
            file,
            line_number,
            include_level,
            state,
        };
        let expanded =
            preprocess::macro_expand::expand_macros_with_hooks(tokens, &token_macros, &mut hooks)
                .ok()?;
        strict_include_spec_from_tokens(&expanded)
    })
}

fn strict_include_spec_from_tokens(tokens: &[preprocess::token::PpToken]) -> Option<IncludeSpec> {
    let start = skip_include_ws(tokens, 0);
    match tokens.get(start).map(|token| &token.kind) {
        Some(preprocess::token::PpTokenKind::StringLit(text)) => {
            if !only_include_ws(tokens, start + 1) {
                return None;
            }
            Some(IncludeSpec::Quoted(
                text.trim_start_matches('"')
                    .trim_end_matches('"')
                    .to_string(),
            ))
        }
        Some(preprocess::token::PpTokenKind::Punct(open)) if open == "<" => {
            let mut name = String::new();
            for (index, token) in tokens.iter().enumerate().skip(start + 1) {
                if matches!(&token.kind, preprocess::token::PpTokenKind::Punct(value) if value == ">")
                {
                    if !only_include_ws(tokens, index + 1) {
                        return None;
                    }
                    return Some(IncludeSpec::Angled(name));
                }
                name.push_str(token.text());
            }
            None
        }
        _ => None,
    }
}

fn skip_include_ws(tokens: &[preprocess::token::PpToken], mut index: usize) -> usize {
    while matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(preprocess::token::PpTokenKind::Whitespace(_))
            | Some(preprocess::token::PpTokenKind::Newline(_))
    ) {
        index += 1;
    }
    index
}

fn only_include_ws(tokens: &[preprocess::token::PpToken], start: usize) -> bool {
    skip_include_ws(tokens, start) == tokens.len()
}

fn parse_token_include_operand(
    operand: &preprocess::directive::IncludeOperand,
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Option<IncludeSpec> {
    match operand {
        preprocess::directive::IncludeOperand::Literal(header) => match header {
            preprocess::directive::HeaderName::Quoted(name) => {
                Some(IncludeSpec::Quoted(name.clone()))
            }
            preprocess::directive::HeaderName::Angled(name) => {
                Some(IncludeSpec::Angled(name.clone()))
            }
        },
        preprocess::directive::IncludeOperand::Tokens(tokens) => {
            parse_include_tokens(tokens, macros, file, line_number, include_level, state)
        }
    }
}

fn expand_preprocessor_tokens(
    tokens: &[preprocess::token::PpToken],
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<Vec<preprocess::token::PpToken>, String> {
    let token_macros = token_macro_table(macros)?;
    let mut hooks = LiveMacroExpansionHooks {
        file,
        line_number,
        include_level,
        state,
    };
    preprocess::macro_expand::expand_macros_with_hooks(tokens, &token_macros, &mut hooks)
}

fn parse_token_line_operand(
    tokens: &[preprocess::token::PpToken],
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<(usize, Option<String>), String> {
    match preprocess::directive::parse_line_operand(tokens) {
        preprocess::directive::LineOperand::Literal { line, filename } => {
            Ok((line, filename.map(decode_line_filename)))
        }
        preprocess::directive::LineOperand::Tokens(tokens) => {
            let expanded = expand_preprocessor_tokens(
                &tokens,
                macros,
                file,
                line_number,
                include_level,
                state,
            )?;
            match preprocess::directive::parse_line_operand(&expanded) {
                preprocess::directive::LineOperand::Literal { line, filename } => {
                    Ok((line, filename.map(decode_line_filename)))
                }
                preprocess::directive::LineOperand::Tokens(_) => Err(format!(
                    "malformed #line directive: {}",
                    preprocess::emit::emit_tokens(&expanded).trim()
                )),
                preprocess::directive::LineOperand::Malformed(err) => Err(line_operand_error(err)),
            }
        }
        preprocess::directive::LineOperand::Malformed(err) => Err(line_operand_error(err)),
    }
}

fn parse_line_marker_tokens(
    tokens: &[preprocess::token::PpToken],
) -> Result<Option<(usize, Option<String>)>, String> {
    let mut index = skip_include_ws(tokens, 0);
    if !matches!(tokens.get(index).map(|token| &token.kind), Some(preprocess::token::PpTokenKind::Punct(value)) if value == "#")
    {
        return Ok(None);
    }
    index = skip_include_ws(tokens, index + 1);
    let Some(preprocess::token::PpToken {
        kind: preprocess::token::PpTokenKind::Number(line_text),
        ..
    }) = tokens.get(index)
    else {
        return Ok(None);
    };
    let line = line_text
        .parse::<usize>()
        .map_err(|_| format!("invalid #line number: {}", line_text))?;
    index = skip_include_ws(tokens, index + 1);
    let filename = match tokens.get(index).map(|token| &token.kind) {
        Some(preprocess::token::PpTokenKind::StringLit(text)) => {
            index = skip_include_ws(tokens, index + 1);
            Some(decode_line_filename(
                text.trim_start_matches('"')
                    .trim_end_matches('"')
                    .to_string(),
            ))
        }
        _ => None,
    };
    while let Some(token) = tokens.get(index) {
        match &token.kind {
            preprocess::token::PpTokenKind::Number(_) => {
                index = skip_include_ws(tokens, index + 1);
            }
            preprocess::token::PpTokenKind::Whitespace(_)
            | preprocess::token::PpTokenKind::Newline(_) => {
                index += 1;
            }
            _ => {
                return Err(format!(
                    "malformed line marker: {}",
                    preprocess::emit::emit_tokens(&tokens[index..]).trim()
                ));
            }
        }
    }
    Ok(Some((line, filename)))
}

fn decode_line_filename(value: String) -> String {
    let mut decoded = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            decoded.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            decoded.push(ch);
        }
    }
    if escaped {
        decoded.push('\\');
    }
    decoded
}

fn process_pragma_operators(line: &str) -> Result<(String, Vec<String>), String> {
    let tokens = preprocess::lexer::lex(line)?;
    let mut out = Vec::new();
    let mut pragmas = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if let Some((pragma, next_index)) = parse_pragma_operator(&tokens, index) {
            pragmas.push(pragma);
            index = next_index;
        } else {
            out.push(tokens[index].clone());
            index += 1;
        }
    }
    Ok((preprocess::emit::emit_tokens(&out), pragmas))
}

fn parse_pragma_operator(
    tokens: &[preprocess::token::PpToken],
    start: usize,
) -> Option<(String, usize)> {
    if !matches!(
        tokens.get(start).map(|token| &token.kind),
        Some(preprocess::token::PpTokenKind::Ident(name)) if name == "_Pragma"
    ) {
        return None;
    }
    let mut index = skip_include_ws(tokens, start + 1);
    if !matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(preprocess::token::PpTokenKind::Punct(value)) if value == "("
    ) {
        return None;
    }
    index = skip_include_ws(tokens, index + 1);
    let Some(preprocess::token::PpToken {
        kind: preprocess::token::PpTokenKind::StringLit(text),
        ..
    }) = tokens.get(index)
    else {
        return None;
    };
    let pragma = decode_line_filename(
        text.trim_start_matches('"')
            .trim_end_matches('"')
            .to_string(),
    );
    index = skip_include_ws(tokens, index + 1);
    if !matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(preprocess::token::PpTokenKind::Punct(value)) if value == ")"
    ) {
        return None;
    }
    Some((pragma, index + 1))
}

fn line_operand_error(error: preprocess::directive::LineOperandError) -> String {
    match error {
        preprocess::directive::LineOperandError::MissingLine => {
            "malformed #line directive: missing line number".to_string()
        }
        preprocess::directive::LineOperandError::InvalidLine(line) => {
            format!("invalid #line number: {}", line)
        }
        preprocess::directive::LineOperandError::InvalidFilename(tokens) => {
            format!(
                "malformed #line directive: invalid filename {}",
                preprocess::emit::emit_tokens(&tokens).trim()
            )
        }
    }
}

fn token_macro_def_to_string(def: preprocess::macro_expand::MacroDef) -> MacroDef {
    match def {
        preprocess::macro_expand::MacroDef::Object(body) => {
            MacroDef::Object(preprocess::emit::emit_tokens(&body).trim().to_string())
        }
        preprocess::macro_expand::MacroDef::Function {
            params,
            variadic,
            body,
        } => MacroDef::Function {
            params,
            variadic,
            body: preprocess::emit::emit_tokens(&body).trim().to_string(),
        },
    }
}

fn format_macro_dump(macros: &HashMap<String, MacroDef>) -> String {
    let mut names: Vec<&String> = macros.keys().collect();
    names.sort();
    let mut out = String::new();
    for name in names {
        out.push_str("#define ");
        out.push_str(name);
        match &macros[name] {
            MacroDef::Object(body) => {
                if !body.is_empty() {
                    out.push(' ');
                    out.push_str(body);
                }
            }
            MacroDef::Function {
                params,
                variadic,
                body,
            } => {
                out.push('(');
                out.push_str(&format_macro_param_list(params, *variadic));
                out.push(')');
                if !body.is_empty() {
                    out.push(' ');
                    out.push_str(body);
                }
            }
        }
        out.push('\n');
    }
    out
}

fn format_macro_param_list(params: &[String], variadic: bool) -> String {
    if !variadic {
        return params.join(", ");
    }
    if params.is_empty() {
        return "...".to_string();
    }
    let mut rendered = params.join(", ");
    rendered.push_str(", ...");
    rendered
}

fn starts_preprocessor_directive(trimmed: &str) -> bool {
    trimmed.starts_with('#') || trimmed.starts_with("%:")
}

fn raw_directive_name(trimmed: &str) -> Option<&str> {
    let rest = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("%:"))?
        .trim_start();
    let end = rest
        .char_indices()
        .find(|(_, ch)| !is_ident_continue(*ch))
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    (end > 0).then_some(&rest[..end])
}

fn trim_preprocessor_prefix(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("%:"))
        .map(str::trim_start)
}

fn is_conditional_control_directive(name: &str) -> bool {
    matches!(
        name,
        "if" | "ifdef" | "ifndef" | "elif" | "elifdef" | "elifndef" | "else" | "endif"
    )
}

fn same_include_dir(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn resolve_include_path(
    spec: &IncludeSpec,
    base_dir: &Path,
    include_paths: &IncludePaths,
    include_next: bool,
) -> Option<PathBuf> {
    let dirs = if include_next {
        include_paths.include_next_dirs(base_dir)
    } else {
        match spec {
            IncludeSpec::Quoted(_) => include_paths.quoted_dirs(base_dir),
            IncludeSpec::Angled(_) => include_paths.angled_dirs(),
        }
    };

    match spec {
        IncludeSpec::Quoted(name) | IncludeSpec::Angled(name) => {
            let direct = PathBuf::from(name);
            if direct.exists() {
                return Some(direct);
            }
            dirs.into_iter()
                .map(|dir| dir.join(name))
                .find(|path| path.exists())
        }
    }
}

fn include_not_found(spec: &IncludeSpec) -> String {
    match spec {
        IncludeSpec::Quoted(name) => format!("include not found: \"{}\"", name),
        IncludeSpec::Angled(name) => format!("include not found: <{}>", name),
    }
}

#[derive(Clone, Copy)]
struct VirtualHeaderInfo {
    name: &'static str,
    guard: Option<&'static str>,
}

const VIRTUAL_COMPAT_HEADERS: &[VirtualHeaderInfo] = &[
    VirtualHeaderInfo {
        name: "assert.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "stdbool.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "stddef.h",
        guard: Some("__rnqcc_stddef_h"),
    },
    VirtualHeaderInfo {
        name: "stdarg.h",
        guard: Some("__rnqcc_stdarg_h"),
    },
    VirtualHeaderInfo {
        name: "stdatomic.h",
        guard: Some("__rnqcc_stdatomic_h"),
    },
    VirtualHeaderInfo {
        name: "limits.h",
        guard: Some("__rnqcc_limits_h"),
    },
    VirtualHeaderInfo {
        name: "stdint.h",
        guard: Some("__rnqcc_stdint_h"),
    },
    VirtualHeaderInfo {
        name: "inttypes.h",
        guard: Some("__rnqcc_inttypes_h"),
    },
    VirtualHeaderInfo {
        name: "float.h",
        guard: Some("__rnqcc_float_h"),
    },
    VirtualHeaderInfo {
        name: "iso646.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "ctype.h",
        guard: Some("__rnqcc_ctype_h"),
    },
    VirtualHeaderInfo {
        name: "dirent.h",
        guard: Some("__rnqcc_dirent_h"),
    },
    VirtualHeaderInfo {
        name: "errno.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "locale.h",
        guard: Some("__rnqcc_locale_h"),
    },
    VirtualHeaderInfo {
        name: "math.h",
        guard: Some("__rnqcc_math_h"),
    },
    VirtualHeaderInfo {
        name: "regex.h",
        guard: Some("__rnqcc_regex_h"),
    },
    VirtualHeaderInfo {
        name: "glob.h",
        guard: Some("__rnqcc_glob_h"),
    },
    VirtualHeaderInfo {
        name: "fnmatch.h",
        guard: Some("__rnqcc_fnmatch_h"),
    },
    VirtualHeaderInfo {
        name: "dlfcn.h",
        guard: Some("__rnqcc_dlfcn_h"),
    },
    VirtualHeaderInfo {
        name: "syslog.h",
        guard: Some("__rnqcc_syslog_h"),
    },
    VirtualHeaderInfo {
        name: "utime.h",
        guard: Some("__rnqcc_utime_h"),
    },
    VirtualHeaderInfo {
        name: "libgen.h",
        guard: Some("__rnqcc_libgen_h"),
    },
    VirtualHeaderInfo {
        name: "paths.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "sysexits.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "fcntl.h",
        guard: Some("__rnqcc_fcntl_h"),
    },
    VirtualHeaderInfo {
        name: "poll.h",
        guard: Some("__rnqcc_poll_h"),
    },
    VirtualHeaderInfo {
        name: "setjmp.h",
        guard: Some("__rnqcc_setjmp_h"),
    },
    VirtualHeaderInfo {
        name: "signal.h",
        guard: Some("__rnqcc_signal_h"),
    },
    VirtualHeaderInfo {
        name: "stdio.h",
        guard: Some("__rnqcc_stdio_h"),
    },
    VirtualHeaderInfo {
        name: "stdlib.h",
        guard: Some("__rnqcc_stdlib_h"),
    },
    VirtualHeaderInfo {
        name: "string.h",
        guard: Some("__rnqcc_string_h"),
    },
    VirtualHeaderInfo {
        name: "strings.h",
        guard: Some("__rnqcc_strings_h"),
    },
    VirtualHeaderInfo {
        name: "stdalign.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "stdnoreturn.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "sys/stat.h",
        guard: Some("__rnqcc_sys_stat_h"),
    },
    VirtualHeaderInfo {
        name: "sys/select.h",
        guard: Some("__rnqcc_sys_select_h"),
    },
    VirtualHeaderInfo {
        name: "sys/socket.h",
        guard: Some("__rnqcc_sys_socket_h"),
    },
    VirtualHeaderInfo {
        name: "sys/un.h",
        guard: Some("__rnqcc_sys_un_h"),
    },
    VirtualHeaderInfo {
        name: "sys/ioctl.h",
        guard: Some("__rnqcc_sys_ioctl_h"),
    },
    VirtualHeaderInfo {
        name: "sys/file.h",
        guard: Some("__rnqcc_sys_file_h"),
    },
    VirtualHeaderInfo {
        name: "sys/mman.h",
        guard: Some("__rnqcc_sys_mman_h"),
    },
    VirtualHeaderInfo {
        name: "sys/param.h",
        guard: Some("__rnqcc_sys_param_h"),
    },
    VirtualHeaderInfo {
        name: "sys/resource.h",
        guard: Some("__rnqcc_sys_resource_h"),
    },
    VirtualHeaderInfo {
        name: "sys/time.h",
        guard: Some("__rnqcc_sys_time_h"),
    },
    VirtualHeaderInfo {
        name: "sys/types.h",
        guard: Some("__rnqcc_sys_types_defined"),
    },
    VirtualHeaderInfo {
        name: "sys/uio.h",
        guard: Some("__rnqcc_sys_uio_h"),
    },
    VirtualHeaderInfo {
        name: "sys/sysmacros.h",
        guard: None,
    },
    VirtualHeaderInfo {
        name: "sys/utsname.h",
        guard: Some("__rnqcc_sys_utsname_h"),
    },
    VirtualHeaderInfo {
        name: "sys/wait.h",
        guard: Some("__rnqcc_sys_wait_h"),
    },
    VirtualHeaderInfo {
        name: "arpa/inet.h",
        guard: Some("__rnqcc_arpa_inet_h"),
    },
    VirtualHeaderInfo {
        name: "netinet/in.h",
        guard: Some("__rnqcc_netinet_in_h"),
    },
    VirtualHeaderInfo {
        name: "netinet/tcp.h",
        guard: Some("__rnqcc_netinet_tcp_h"),
    },
    VirtualHeaderInfo {
        name: "net/if.h",
        guard: Some("__rnqcc_net_if_h"),
    },
    VirtualHeaderInfo {
        name: "ifaddrs.h",
        guard: Some("__rnqcc_ifaddrs_h"),
    },
    VirtualHeaderInfo {
        name: "netdb.h",
        guard: Some("__rnqcc_netdb_h"),
    },
    VirtualHeaderInfo {
        name: "time.h",
        guard: Some("__rnqcc_time_h"),
    },
    VirtualHeaderInfo {
        name: "pthread.h",
        guard: Some("__rnqcc_pthread_h"),
    },
    VirtualHeaderInfo {
        name: "grp.h",
        guard: Some("__rnqcc_grp_h"),
    },
    VirtualHeaderInfo {
        name: "pwd.h",
        guard: Some("__rnqcc_pwd_h"),
    },
    VirtualHeaderInfo {
        name: "termios.h",
        guard: Some("__rnqcc_termios_h"),
    },
    VirtualHeaderInfo {
        name: "unistd.h",
        guard: Some("__rnqcc_unistd_h"),
    },
    VirtualHeaderInfo {
        name: "wchar.h",
        guard: Some("__rnqcc_wchar_h"),
    },
    VirtualHeaderInfo {
        name: "wctype.h",
        guard: Some("__rnqcc_wctype_h"),
    },
];

fn virtual_header_info(name: &str) -> Option<VirtualHeaderInfo> {
    VIRTUAL_COMPAT_HEADERS
        .iter()
        .copied()
        .find(|header| header.name == name)
}

fn virtual_compat_header_name(spec: &IncludeSpec) -> Option<&str> {
    match spec {
        IncludeSpec::Angled(name) if virtual_header_info(name).is_some() => Some(name),
        _ => None,
    }
}

fn virtual_size_t_typedef(macros: &mut HashMap<String, MacroDef>) -> &'static str {
    if macros.contains_key("__rnqcc_size_t_defined") {
        ""
    } else {
        macros.insert(
            "__rnqcc_size_t_defined".to_string(),
            MacroDef::Object("1".to_string()),
        );
        "typedef unsigned long size_t;\n"
    }
}

fn virtual_ssize_t_typedef(macros: &mut HashMap<String, MacroDef>) -> &'static str {
    if macros.contains_key("__rnqcc_ssize_t_defined") {
        ""
    } else {
        macros.insert(
            "__rnqcc_ssize_t_defined".to_string(),
            MacroDef::Object("1".to_string()),
        );
        "typedef long ssize_t;\n"
    }
}

fn virtual_time_t_typedef(macros: &mut HashMap<String, MacroDef>) -> &'static str {
    if macros.contains_key("__rnqcc_time_t_defined") {
        ""
    } else {
        macros.insert(
            "__rnqcc_time_t_defined".to_string(),
            MacroDef::Object("1".to_string()),
        );
        "typedef long time_t;\n"
    }
}

fn virtual_null_macro(macros: &mut HashMap<String, MacroDef>) {
    macros.insert(
        "NULL".to_string(),
        MacroDef::Object("((void *)0)".to_string()),
    );
}

fn virtual_include_once(macros: &mut HashMap<String, MacroDef>, key: &str) -> bool {
    if macros.contains_key(key) {
        true
    } else {
        macros.insert(key.to_string(), MacroDef::Object("1".to_string()));
        false
    }
}

fn virtual_header_include_once(macros: &mut HashMap<String, MacroDef>, name: &str) -> bool {
    virtual_header_info(name)
        .and_then(|header| header.guard)
        .is_some_and(|guard| virtual_include_once(macros, guard))
}

fn define_virtual_object_macros(macros: &mut HashMap<String, MacroDef>, entries: &[(&str, &str)]) {
    for (name, value) in entries {
        macros.insert((*name).to_string(), MacroDef::Object((*value).to_string()));
    }
}

fn include_virtual_compat_header(name: &str, macros: &mut HashMap<String, MacroDef>) -> String {
    match name {
        "assert.h" => {
            let body = if macros.contains_key("NDEBUG") {
                "0"
            } else {
                "((expr) || (__builtin_trap(), 0))"
            };
            macros.insert(
                "assert".to_string(),
                MacroDef::Function {
                    params: vec!["expr".to_string()],
                    variadic: false,
                    body: body.to_string(),
                },
            );
            macros.insert(
                "static_assert".to_string(),
                MacroDef::Object("_Static_assert".to_string()),
            );
            String::new()
        }
        "stdbool.h" => {
            macros.insert("bool".to_string(), MacroDef::Object("_Bool".to_string()));
            macros.insert("true".to_string(), MacroDef::Object("1".to_string()));
            macros.insert("false".to_string(), MacroDef::Object("0".to_string()));
            macros.insert(
                "__bool_true_false_are_defined".to_string(),
                MacroDef::Object("1".to_string()),
            );
            String::new()
        }
        "stddef.h" => {
            virtual_null_macro(macros);
            macros.insert(
                "offsetof".to_string(),
                MacroDef::Function {
                    params: vec!["type".to_string(), "member".to_string()],
                    variadic: false,
                    body: "__builtin_offsetof(type, member)".to_string(),
                },
            );
            if virtual_header_include_once(macros, "stddef.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/stddef.h")
            )
        }
        "stdarg.h" => {
            for (name, params, body) in [
                ("va_start", vec!["ap", "last"], "((void)0)"),
                ("va_end", vec!["ap"], "((void)0)"),
                ("va_copy", vec!["dst", "src"], "((dst) = (src))"),
                ("__va_copy", vec!["dst", "src"], "((dst) = (src))"),
                ("va_arg", vec!["ap", "type"], "(*((type *)0))"),
            ] {
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: params.into_iter().map(str::to_string).collect(),
                        variadic: false,
                        body: body.to_string(),
                    },
                );
            }
            if virtual_header_include_once(macros, "stdarg.h") {
                String::new()
            } else {
                include_str!("virtual_headers/stdarg.h").to_string()
            }
        }
        "stdio.h" => {
            virtual_null_macro(macros);
            define_virtual_object_macros(
                macros,
                &[
                    ("EOF", "(-1)"),
                    ("BUFSIZ", "1024"),
                    ("FILENAME_MAX", "1024"),
                    ("FOPEN_MAX", "16"),
                    ("TMP_MAX", "10000"),
                    ("SEEK_SET", "0"),
                    ("SEEK_CUR", "1"),
                    ("SEEK_END", "2"),
                ],
            );
            if virtual_header_include_once(macros, "stdio.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/stdio.h")
            )
        }
        "stdlib.h" => {
            virtual_null_macro(macros);
            define_virtual_object_macros(
                macros,
                &[
                    ("EXIT_FAILURE", "1"),
                    ("EXIT_SUCCESS", "0"),
                    ("RAND_MAX", "2147483647"),
                ],
            );
            if virtual_header_include_once(macros, "stdlib.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/stdlib.h")
            )
        }
        "string.h" => {
            virtual_null_macro(macros);
            if virtual_header_include_once(macros, "string.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/string.h")
            )
        }
        "strings.h" => {
            if virtual_header_include_once(macros, "strings.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/strings.h")
            )
        }
        "ctype.h" => {
            if virtual_header_include_once(macros, "ctype.h") {
                String::new()
            } else {
                include_str!("virtual_headers/ctype.h").to_string()
            }
        }
        "dirent.h" => {
            if virtual_header_include_once(macros, "dirent.h") {
                String::new()
            } else {
                include_str!("virtual_headers/dirent.h").to_string()
            }
        }
        "errno.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("EPERM", "1"),
                    ("ENOENT", "2"),
                    ("ESRCH", "3"),
                    ("EINTR", "4"),
                    ("EIO", "5"),
                    ("ENXIO", "6"),
                    ("E2BIG", "7"),
                    ("ENOEXEC", "8"),
                    ("EBADF", "9"),
                    ("ECHILD", "10"),
                    ("EAGAIN", "11"),
                    ("ENOMEM", "12"),
                    ("EACCES", "13"),
                    ("EFAULT", "14"),
                    ("EBUSY", "16"),
                    ("EEXIST", "17"),
                    ("EXDEV", "18"),
                    ("ENODEV", "19"),
                    ("ENOTDIR", "20"),
                    ("EISDIR", "21"),
                    ("EINVAL", "22"),
                    ("ENFILE", "23"),
                    ("EMFILE", "24"),
                    ("ENOTTY", "25"),
                    ("EFBIG", "27"),
                    ("ENOSPC", "28"),
                    ("ESPIPE", "29"),
                    ("EROFS", "30"),
                    ("EMLINK", "31"),
                    ("EPIPE", "32"),
                    ("EDOM", "33"),
                    ("ERANGE", "34"),
                    ("EILSEQ", "92"),
                ],
            );
            if macros.contains_key("__APPLE__") {
                macros.insert(
                    "errno".to_string(),
                    MacroDef::Object("(*__error())".to_string()),
                );
                "int *__error(void);\n".to_string()
            } else {
                macros.insert(
                    "errno".to_string(),
                    MacroDef::Object("(*__errno_location())".to_string()),
                );
                "int *__errno_location(void);\n".to_string()
            }
        }
        "math.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("HUGE_VAL", "1.0e999"),
                    ("INFINITY", "1.0e999F"),
                    ("NAN", "(0.0F / 0.0F)"),
                    ("FP_ILOGB0", "(-2147483647 - 1)"),
                    ("FP_ILOGBNAN", "(-2147483647 - 1)"),
                    ("MATH_ERRNO", "1"),
                    ("MATH_ERREXCEPT", "2"),
                    ("math_errhandling", "MATH_ERRNO"),
                ],
            );
            if virtual_header_include_once(macros, "math.h") {
                return String::new();
            }
            include_str!("virtual_headers/math.h").to_string()
        }
        "regex.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("REG_EXTENDED", "1"),
                    ("REG_ICASE", "2"),
                    ("REG_NOSUB", "4"),
                    ("REG_NEWLINE", "8"),
                    ("REG_NOTBOL", "1"),
                    ("REG_NOTEOL", "2"),
                    ("REG_NOMATCH", "1"),
                    ("REG_BADPAT", "2"),
                ],
            );
            if virtual_header_include_once(macros, "regex.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/regex.h")
            )
        }
        "glob.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("GLOB_ERR", "1"),
                    ("GLOB_MARK", "2"),
                    ("GLOB_NOSORT", "4"),
                    ("GLOB_DOOFFS", "8"),
                    ("GLOB_NOCHECK", "16"),
                    ("GLOB_APPEND", "32"),
                    ("GLOB_NOESCAPE", "64"),
                    ("GLOB_NOSPACE", "1"),
                    ("GLOB_ABORTED", "2"),
                    ("GLOB_NOMATCH", "3"),
                ],
            );
            if virtual_header_include_once(macros, "glob.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/glob.h")
            )
        }
        "fnmatch.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("FNM_NOMATCH", "1"),
                    ("FNM_PATHNAME", "1"),
                    ("FNM_NOESCAPE", "2"),
                    ("FNM_PERIOD", "4"),
                ],
            );
            if virtual_header_include_once(macros, "fnmatch.h") {
                return String::new();
            }
            include_str!("virtual_headers/fnmatch.h").to_string()
        }
        "dlfcn.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("RTLD_LAZY", "1"),
                    ("RTLD_NOW", "2"),
                    ("RTLD_LOCAL", "0"),
                    ("RTLD_GLOBAL", "0x100"),
                    ("RTLD_DEFAULT", "((void *)0)"),
                    ("RTLD_NEXT", "((void *)-1)"),
                ],
            );
            if virtual_header_include_once(macros, "dlfcn.h") {
                return String::new();
            }
            include_str!("virtual_headers/dlfcn.h").to_string()
        }
        "syslog.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("LOG_PID", "0x01"),
                    ("LOG_CONS", "0x02"),
                    ("LOG_NDELAY", "0x08"),
                    ("LOG_PERROR", "0x20"),
                    ("LOG_EMERG", "0"),
                    ("LOG_ALERT", "1"),
                    ("LOG_CRIT", "2"),
                    ("LOG_ERR", "3"),
                    ("LOG_WARNING", "4"),
                    ("LOG_NOTICE", "5"),
                    ("LOG_INFO", "6"),
                    ("LOG_DEBUG", "7"),
                    ("LOG_KERN", "(0 << 3)"),
                    ("LOG_USER", "(1 << 3)"),
                    ("LOG_DAEMON", "(3 << 3)"),
                    ("LOG_AUTH", "(4 << 3)"),
                    ("LOG_LOCAL0", "(16 << 3)"),
                    ("LOG_LOCAL7", "(23 << 3)"),
                ],
            );
            for (name, body) in [
                ("LOG_MASK", "(1 << (pri))"),
                ("LOG_UPTO", "((1 << ((pri) + 1)) - 1)"),
            ] {
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: vec!["pri".to_string()],
                        variadic: false,
                        body: body.to_string(),
                    },
                );
            }
            if virtual_header_include_once(macros, "syslog.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("stdarg.h", macros),
                include_str!("virtual_headers/syslog.h")
            )
        }
        "utime.h" => {
            if virtual_header_include_once(macros, "utime.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("time.h", macros),
                include_str!("virtual_headers/utime.h")
            )
        }
        "libgen.h" => {
            if virtual_header_include_once(macros, "libgen.h") {
                return String::new();
            }
            include_str!("virtual_headers/libgen.h").to_string()
        }
        "paths.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("_PATH_BSHELL", "\"/bin/sh\""),
                    ("_PATH_CSHELL", "\"/bin/csh\""),
                    ("_PATH_DEV", "\"/dev/\""),
                    ("_PATH_DEVNULL", "\"/dev/null\""),
                    ("_PATH_TMP", "\"/tmp/\""),
                    ("_PATH_VARDB", "\"/var/db/\""),
                ],
            );
            String::new()
        }
        "sysexits.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("EX_OK", "0"),
                    ("EX_USAGE", "64"),
                    ("EX_DATAERR", "65"),
                    ("EX_NOINPUT", "66"),
                    ("EX_NOUSER", "67"),
                    ("EX_NOHOST", "68"),
                    ("EX_UNAVAILABLE", "69"),
                    ("EX_SOFTWARE", "70"),
                    ("EX_OSERR", "71"),
                    ("EX_OSFILE", "72"),
                    ("EX_CANTCREAT", "73"),
                    ("EX_IOERR", "74"),
                    ("EX_TEMPFAIL", "75"),
                    ("EX_PROTOCOL", "76"),
                    ("EX_NOPERM", "77"),
                    ("EX_CONFIG", "78"),
                ],
            );
            String::new()
        }
        "signal.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("SIGABRT", "6"),
                    ("SIGFPE", "8"),
                    ("SIGILL", "4"),
                    ("SIGINT", "2"),
                    ("SIGSEGV", "11"),
                    ("SIGTERM", "15"),
                    ("SIG_DFL", "((void (*)(int))0)"),
                    ("SIG_ERR", "((void (*)(int))-1)"),
                    ("SIG_IGN", "((void (*)(int))1)"),
                ],
            );
            if virtual_header_include_once(macros, "signal.h") {
                return String::new();
            }
            include_str!("virtual_headers/signal.h").to_string()
        }
        "setjmp.h" => {
            if virtual_header_include_once(macros, "setjmp.h") {
                String::new()
            } else {
                include_str!("virtual_headers/setjmp.h").to_string()
            }
        }
        "locale.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("LC_ALL", "0"),
                    ("LC_COLLATE", "1"),
                    ("LC_CTYPE", "2"),
                    ("LC_MONETARY", "3"),
                    ("LC_NUMERIC", "4"),
                    ("LC_TIME", "5"),
                    ("NULL", "((void *)0)"),
                ],
            );
            if virtual_header_include_once(macros, "locale.h") {
                String::new()
            } else {
                include_str!("virtual_headers/locale.h").to_string()
            }
        }
        "time.h" => {
            virtual_null_macro(macros);
            define_virtual_object_macros(macros, &[("CLOCKS_PER_SEC", "1000000L")]);
            if virtual_header_include_once(macros, "time.h") {
                return String::new();
            }
            format!(
                "{}{}{}",
                virtual_size_t_typedef(macros),
                virtual_time_t_typedef(macros),
                include_str!("virtual_headers/time.h")
            )
        }
        "sys/time.h" => {
            if virtual_header_include_once(macros, "sys/time.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/sys/time.h")
            )
        }
        "sys/types.h" => {
            if virtual_header_include_once(macros, "sys/types.h") {
                return String::new();
            }
            format!(
                "{}{}{}{}",
                virtual_size_t_typedef(macros),
                virtual_ssize_t_typedef(macros),
                virtual_time_t_typedef(macros),
                include_str!("virtual_headers/sys/types.h")
            )
        }
        "sys/stat.h" => {
            let types = include_virtual_compat_header("sys/types.h", macros);
            define_virtual_object_macros(
                macros,
                &[
                    ("S_IFMT", "0170000"),
                    ("S_IFDIR", "0040000"),
                    ("S_IFCHR", "0020000"),
                    ("S_IFBLK", "0060000"),
                    ("S_IFREG", "0100000"),
                    ("S_IFIFO", "0010000"),
                    ("S_IFLNK", "0120000"),
                    ("S_IFSOCK", "0140000"),
                    ("S_IRUSR", "0400"),
                    ("S_IWUSR", "0200"),
                    ("S_IXUSR", "0100"),
                    ("S_IRGRP", "0040"),
                    ("S_IWGRP", "0020"),
                    ("S_IXGRP", "0010"),
                    ("S_IROTH", "0004"),
                    ("S_IWOTH", "0002"),
                    ("S_IXOTH", "0001"),
                    ("S_IRWXU", "0700"),
                    ("S_IRWXG", "0070"),
                    ("S_IRWXO", "0007"),
                ],
            );
            for (name, body) in [
                ("S_ISDIR", "(((mode) & S_IFMT) == S_IFDIR)"),
                ("S_ISCHR", "(((mode) & S_IFMT) == S_IFCHR)"),
                ("S_ISBLK", "(((mode) & S_IFMT) == S_IFBLK)"),
                ("S_ISREG", "(((mode) & S_IFMT) == S_IFREG)"),
                ("S_ISFIFO", "(((mode) & S_IFMT) == S_IFIFO)"),
                ("S_ISLNK", "(((mode) & S_IFMT) == S_IFLNK)"),
                ("S_ISSOCK", "(((mode) & S_IFMT) == S_IFSOCK)"),
            ] {
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: vec!["mode".to_string()],
                        variadic: false,
                        body: body.to_string(),
                    },
                );
            }
            if virtual_header_include_once(macros, "sys/stat.h") {
                return String::new();
            }
            format!("{}{}", types, include_str!("virtual_headers/sys/stat.h"))
        }
        "fcntl.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("O_RDONLY", "0"),
                    ("O_WRONLY", "1"),
                    ("O_RDWR", "2"),
                    ("O_ACCMODE", "3"),
                    ("O_CREAT", "0100"),
                    ("O_EXCL", "0200"),
                    ("O_NOCTTY", "0400"),
                    ("O_TRUNC", "01000"),
                    ("O_APPEND", "02000"),
                    ("O_NONBLOCK", "04000"),
                    ("O_DSYNC", "010000"),
                    ("O_DIRECTORY", "0200000"),
                    ("O_NOFOLLOW", "0400000"),
                    ("O_CLOEXEC", "02000000"),
                    ("O_SYNC", "04010000"),
                    ("AT_FDCWD", "-100"),
                    ("AT_SYMLINK_NOFOLLOW", "0x100"),
                    ("AT_REMOVEDIR", "0x200"),
                    ("AT_SYMLINK_FOLLOW", "0x400"),
                    ("AT_EACCESS", "0x200"),
                    ("FD_CLOEXEC", "1"),
                    ("F_DUPFD", "0"),
                    ("F_GETFD", "1"),
                    ("F_SETFD", "2"),
                    ("F_GETFL", "3"),
                    ("F_SETFL", "4"),
                    ("F_GETLK", "5"),
                    ("F_SETLK", "6"),
                    ("F_SETLKW", "7"),
                    ("F_SETOWN", "8"),
                    ("F_GETOWN", "9"),
                    ("F_RDLCK", "0"),
                    ("F_WRLCK", "1"),
                    ("F_UNLCK", "2"),
                    ("POSIX_FADV_NORMAL", "0"),
                    ("POSIX_FADV_RANDOM", "1"),
                    ("POSIX_FADV_SEQUENTIAL", "2"),
                    ("POSIX_FADV_WILLNEED", "3"),
                    ("POSIX_FADV_DONTNEED", "4"),
                    ("POSIX_FADV_NOREUSE", "5"),
                ],
            );
            if virtual_header_include_once(macros, "fcntl.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/fcntl.h")
            )
        }
        "poll.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("POLLIN", "0x0001"),
                    ("POLLPRI", "0x0002"),
                    ("POLLOUT", "0x0004"),
                    ("POLLERR", "0x0008"),
                    ("POLLHUP", "0x0010"),
                    ("POLLNVAL", "0x0020"),
                ],
            );
            if virtual_header_include_once(macros, "poll.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/poll.h")
            )
        }
        "unistd.h" => {
            virtual_null_macro(macros);
            define_virtual_object_macros(
                macros,
                &[
                    ("STDIN_FILENO", "0"),
                    ("STDOUT_FILENO", "1"),
                    ("STDERR_FILENO", "2"),
                    ("SEEK_SET", "0"),
                    ("SEEK_CUR", "1"),
                    ("SEEK_END", "2"),
                    ("R_OK", "4"),
                    ("W_OK", "2"),
                    ("X_OK", "1"),
                    ("F_OK", "0"),
                ],
            );
            if virtual_header_include_once(macros, "unistd.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/unistd.h")
            )
        }
        "sys/select.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("FD_SETSIZE", "1024"),
                    ("NFDBITS", "(8 * sizeof(unsigned long))"),
                ],
            );
            if virtual_header_include_once(macros, "sys/select.h") {
                return String::new();
            }
            format!(
                "{}{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_virtual_compat_header("sys/time.h", macros),
                include_str!("virtual_headers/sys/select.h")
            )
        }
        "sys/socket.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("AF_UNSPEC", "0"),
                    ("AF_INET", "2"),
                    ("AF_INET6", "10"),
                    ("PF_UNSPEC", "0"),
                    ("PF_INET", "2"),
                    ("PF_INET6", "10"),
                    ("SOCK_STREAM", "1"),
                    ("SOCK_DGRAM", "2"),
                    ("SOCK_RAW", "3"),
                    ("SOL_SOCKET", "1"),
                    ("SO_REUSEADDR", "2"),
                    ("SHUT_RD", "0"),
                    ("SHUT_WR", "1"),
                    ("SHUT_RDWR", "2"),
                    ("MSG_OOB", "0x1"),
                    ("MSG_PEEK", "0x2"),
                    ("MSG_DONTWAIT", "0x40"),
                ],
            );
            if virtual_header_include_once(macros, "sys/socket.h") {
                return String::new();
            }
            format!(
                "{}{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_virtual_compat_header("sys/uio.h", macros),
                include_str!("virtual_headers/sys/socket.h")
            )
        }
        "sys/un.h" => {
            define_virtual_object_macros(macros, &[("AF_UNIX", "1"), ("PF_UNIX", "1")]);
            if virtual_header_include_once(macros, "sys/un.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/socket.h", macros),
                include_str!("virtual_headers/sys/un.h")
            )
        }
        "sys/uio.h" => {
            if virtual_header_include_once(macros, "sys/uio.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/sys/uio.h")
            )
        }
        "sys/ioctl.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("TIOCGWINSZ", "0x5413"),
                    ("TIOCSWINSZ", "0x5414"),
                    ("FIONBIO", "0x5421"),
                ],
            );
            if virtual_header_include_once(macros, "sys/ioctl.h") {
                return String::new();
            }
            include_str!("virtual_headers/sys/ioctl.h").to_string()
        }
        "sys/file.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("LOCK_SH", "1"),
                    ("LOCK_EX", "2"),
                    ("LOCK_NB", "4"),
                    ("LOCK_UN", "8"),
                ],
            );
            if virtual_header_include_once(macros, "sys/file.h") {
                return String::new();
            }
            include_str!("virtual_headers/sys/file.h").to_string()
        }
        "sys/mman.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("PROT_NONE", "0x0"),
                    ("PROT_READ", "0x1"),
                    ("PROT_WRITE", "0x2"),
                    ("PROT_EXEC", "0x4"),
                    ("MAP_SHARED", "0x01"),
                    ("MAP_PRIVATE", "0x02"),
                    ("MAP_FIXED", "0x10"),
                    ("MAP_ANON", "0x20"),
                    ("MAP_ANONYMOUS", "MAP_ANON"),
                    ("MAP_FAILED", "((void *)-1)"),
                    ("MS_ASYNC", "0x1"),
                    ("MS_INVALIDATE", "0x2"),
                    ("MS_SYNC", "0x4"),
                ],
            );
            if virtual_header_include_once(macros, "sys/mman.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/sys/mman.h")
            )
        }
        "sys/param.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("MAXPATHLEN", "1024"),
                    ("MAXHOSTNAMELEN", "256"),
                    ("NBBY", "8"),
                    ("NGROUPS", "16"),
                ],
            );
            for (name, params, body) in [
                ("MIN", vec!["a", "b"], "((a) < (b) ? (a) : (b))"),
                ("MAX", vec!["a", "b"], "((a) > (b) ? (a) : (b))"),
                ("howmany", vec!["x", "y"], "(((x) + ((y) - 1)) / (y))"),
                (
                    "roundup",
                    vec!["x", "y"],
                    "((((x) + ((y) - 1)) / (y)) * (y))",
                ),
            ] {
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: params.into_iter().map(str::to_string).collect(),
                        variadic: false,
                        body: body.to_string(),
                    },
                );
            }
            if virtual_header_include_once(macros, "sys/param.h") {
                return String::new();
            }
            include_str!("virtual_headers/sys/param.h").to_string()
        }
        "sys/resource.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("RUSAGE_SELF", "0"),
                    ("RUSAGE_CHILDREN", "-1"),
                    ("RLIM_INFINITY", "((rlim_t)-1)"),
                    ("RLIMIT_CPU", "0"),
                    ("RLIMIT_FSIZE", "1"),
                    ("RLIMIT_DATA", "2"),
                    ("RLIMIT_STACK", "3"),
                    ("RLIMIT_CORE", "4"),
                    ("RLIMIT_NOFILE", "7"),
                    ("RLIMIT_AS", "9"),
                ],
            );
            if virtual_header_include_once(macros, "sys/resource.h") {
                return String::new();
            }
            format!(
                "{}{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_virtual_compat_header("sys/time.h", macros),
                include_str!("virtual_headers/sys/resource.h")
            )
        }
        "sys/utsname.h" => {
            if virtual_header_include_once(macros, "sys/utsname.h") {
                return String::new();
            }
            include_str!("virtual_headers/sys/utsname.h").to_string()
        }
        "sys/wait.h" => {
            define_virtual_object_macros(
                macros,
                &[("WNOHANG", "1"), ("WUNTRACED", "2"), ("WCONTINUED", "8")],
            );
            for (name, body) in [
                ("WEXITSTATUS", "(((status) >> 8) & 0xff)"),
                ("WTERMSIG", "((status) & 0x7f)"),
                ("WSTOPSIG", "WEXITSTATUS(status)"),
                ("WIFEXITED", "(WTERMSIG(status) == 0)"),
                (
                    "WIFSIGNALED",
                    "(WTERMSIG(status) != 0 && WTERMSIG(status) != 0x7f)",
                ),
                ("WIFSTOPPED", "(WTERMSIG(status) == 0x7f)"),
                ("WIFCONTINUED", "((status) == 0xffff)"),
            ] {
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: vec!["status".to_string()],
                        variadic: false,
                        body: body.to_string(),
                    },
                );
            }
            if virtual_header_include_once(macros, "sys/wait.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/sys/wait.h")
            )
        }
        "sys/sysmacros.h" => {
            for (name, params, body) in [
                ("major", vec!["dev"], "(((dev) >> 8) & 0xfff)"),
                ("minor", vec!["dev"], "((dev) & 0xff)"),
                ("makedev", vec!["maj", "min"], "(((maj) << 8) | (min))"),
            ] {
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: params.into_iter().map(str::to_string).collect(),
                        variadic: false,
                        body: body.to_string(),
                    },
                );
            }
            String::new()
        }
        "netinet/in.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("IPPROTO_IP", "0"),
                    ("IPPROTO_TCP", "6"),
                    ("IPPROTO_UDP", "17"),
                    ("INADDR_ANY", "0x00000000U"),
                    ("INADDR_LOOPBACK", "0x7f000001U"),
                    ("INADDR_NONE", "0xffffffffU"),
                    ("INET_ADDRSTRLEN", "16"),
                    ("INET6_ADDRSTRLEN", "46"),
                ],
            );
            if virtual_header_include_once(macros, "netinet/in.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/socket.h", macros),
                include_str!("virtual_headers/netinet/in.h")
            )
        }
        "netinet/tcp.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("TCP_NODELAY", "1"),
                    ("TCP_MAXSEG", "2"),
                    ("TCP_KEEPIDLE", "4"),
                    ("TCP_KEEPINTVL", "5"),
                    ("TCP_KEEPCNT", "6"),
                ],
            );
            if virtual_header_include_once(macros, "netinet/tcp.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("netinet/in.h", macros),
                include_str!("virtual_headers/netinet/tcp.h")
            )
        }
        "net/if.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("IF_NAMESIZE", "16"),
                    ("IFF_UP", "0x1"),
                    ("IFF_BROADCAST", "0x2"),
                    ("IFF_LOOPBACK", "0x8"),
                    ("IFF_POINTOPOINT", "0x10"),
                    ("IFF_RUNNING", "0x40"),
                    ("IFF_MULTICAST", "0x1000"),
                ],
            );
            if virtual_header_include_once(macros, "net/if.h") {
                return String::new();
            }
            include_str!("virtual_headers/net/if.h").to_string()
        }
        "ifaddrs.h" => {
            if virtual_header_include_once(macros, "ifaddrs.h") {
                return String::new();
            }
            format!(
                "{}{}{}",
                include_virtual_compat_header("sys/socket.h", macros),
                include_virtual_compat_header("net/if.h", macros),
                include_str!("virtual_headers/ifaddrs.h")
            )
        }
        "arpa/inet.h" => {
            if virtual_header_include_once(macros, "arpa/inet.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("netinet/in.h", macros),
                include_str!("virtual_headers/arpa/inet.h")
            )
        }
        "netdb.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("AI_PASSIVE", "0x0001"),
                    ("AI_CANONNAME", "0x0002"),
                    ("AI_NUMERICHOST", "0x0004"),
                    ("AI_NUMERICSERV", "0x0400"),
                    ("NI_MAXHOST", "1025"),
                    ("NI_MAXSERV", "32"),
                    ("NI_NUMERICHOST", "0x0001"),
                    ("NI_NUMERICSERV", "0x0002"),
                    ("EAI_BADFLAGS", "-1"),
                    ("EAI_NONAME", "-2"),
                    ("EAI_AGAIN", "-3"),
                    ("EAI_FAIL", "-4"),
                    ("EAI_MEMORY", "-10"),
                    ("EAI_SYSTEM", "-11"),
                ],
            );
            if virtual_header_include_once(macros, "netdb.h") {
                return String::new();
            }
            format!(
                "{}{}{}",
                include_virtual_compat_header("sys/socket.h", macros),
                include_virtual_compat_header("netinet/in.h", macros),
                include_str!("virtual_headers/netdb.h")
            )
        }
        "pthread.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("PTHREAD_MUTEX_INITIALIZER", "{0}"),
                    ("PTHREAD_COND_INITIALIZER", "{0}"),
                    ("PTHREAD_CREATE_JOINABLE", "0"),
                    ("PTHREAD_CREATE_DETACHED", "1"),
                ],
            );
            if virtual_header_include_once(macros, "pthread.h") {
                String::new()
            } else {
                include_str!("virtual_headers/pthread.h").to_string()
            }
        }
        "grp.h" => {
            if virtual_header_include_once(macros, "grp.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/grp.h")
            )
        }
        "pwd.h" => {
            if virtual_header_include_once(macros, "pwd.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("virtual_headers/pwd.h")
            )
        }
        "termios.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("NCCS", "32"),
                    ("VINTR", "0"),
                    ("VQUIT", "1"),
                    ("VERASE", "2"),
                    ("VKILL", "3"),
                    ("VEOF", "4"),
                    ("VMIN", "5"),
                    ("VTIME", "6"),
                    ("BRKINT", "0x0002"),
                    ("ICRNL", "0x0100"),
                    ("IXON", "0x0400"),
                    ("OPOST", "0x0001"),
                    ("CS8", "0x0030"),
                    ("CREAD", "0x0080"),
                    ("CLOCAL", "0x0800"),
                    ("ECHO", "0x0008"),
                    ("ICANON", "0x0002"),
                    ("ISIG", "0x0001"),
                    ("TCSANOW", "0"),
                    ("TCSADRAIN", "1"),
                    ("TCSAFLUSH", "2"),
                    ("B0", "0"),
                    ("B9600", "9600"),
                    ("B38400", "38400"),
                    ("B115200", "115200"),
                ],
            );
            if virtual_header_include_once(macros, "termios.h") {
                return String::new();
            }
            include_str!("virtual_headers/termios.h").to_string()
        }
        "wchar.h" => {
            virtual_null_macro(macros);
            if virtual_header_include_once(macros, "wchar.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("virtual_headers/wchar.h")
            )
        }
        "wctype.h" => {
            let wchar = include_virtual_compat_header("wchar.h", macros);
            if virtual_header_include_once(macros, "wctype.h") {
                return String::new();
            }
            format!("{}{}", wchar, include_str!("virtual_headers/wctype.h"))
        }
        "stdatomic.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("memory_order_relaxed", "0"),
                    ("memory_order_consume", "1"),
                    ("memory_order_acquire", "2"),
                    ("memory_order_release", "3"),
                    ("memory_order_acq_rel", "4"),
                    ("memory_order_seq_cst", "5"),
                    ("ATOMIC_BOOL_LOCK_FREE", "2"),
                    ("ATOMIC_CHAR_LOCK_FREE", "2"),
                    ("ATOMIC_SHORT_LOCK_FREE", "2"),
                    ("ATOMIC_INT_LOCK_FREE", "2"),
                    ("ATOMIC_LONG_LOCK_FREE", "2"),
                    ("ATOMIC_LLONG_LOCK_FREE", "2"),
                    ("ATOMIC_POINTER_LOCK_FREE", "2"),
                    ("ATOMIC_FLAG_INIT", "0"),
                ],
            );
            for (name, body) in [
                ("ATOMIC_VAR_INIT", "(value)"),
                (
                    "atomic_init",
                    "__atomic_store_n((object), (desired), memory_order_relaxed)",
                ),
                ("atomic_load", "__atomic_load_n((object), memory_order_seq_cst)"),
                (
                    "atomic_load_explicit",
                    "__atomic_load_n((object), (order))",
                ),
                (
                    "atomic_store",
                    "__atomic_store_n((object), (desired), memory_order_seq_cst)",
                ),
                (
                    "atomic_store_explicit",
                    "__atomic_store_n((object), (desired), (order))",
                ),
                (
                    "atomic_exchange",
                    "__atomic_exchange_n((object), (desired), memory_order_seq_cst)",
                ),
                (
                    "atomic_exchange_explicit",
                    "__atomic_exchange_n((object), (desired), (order))",
                ),
                (
                    "atomic_fetch_add",
                    "__atomic_fetch_add((object), (operand), memory_order_seq_cst)",
                ),
                (
                    "atomic_fetch_add_explicit",
                    "__atomic_fetch_add((object), (operand), (order))",
                ),
                (
                    "atomic_fetch_sub",
                    "__atomic_fetch_sub((object), (operand), memory_order_seq_cst)",
                ),
                (
                    "atomic_fetch_sub_explicit",
                    "__atomic_fetch_sub((object), (operand), (order))",
                ),
                (
                    "atomic_fetch_or",
                    "__atomic_fetch_or((object), (operand), memory_order_seq_cst)",
                ),
                (
                    "atomic_fetch_or_explicit",
                    "__atomic_fetch_or((object), (operand), (order))",
                ),
                (
                    "atomic_fetch_xor",
                    "__atomic_fetch_xor((object), (operand), memory_order_seq_cst)",
                ),
                (
                    "atomic_fetch_xor_explicit",
                    "__atomic_fetch_xor((object), (operand), (order))",
                ),
                (
                    "atomic_fetch_and",
                    "__atomic_fetch_and((object), (operand), memory_order_seq_cst)",
                ),
                (
                    "atomic_fetch_and_explicit",
                    "__atomic_fetch_and((object), (operand), (order))",
                ),
                (
                    "atomic_compare_exchange_strong",
                    "__atomic_compare_exchange_n((object), (expected), (desired), 0, memory_order_seq_cst, memory_order_seq_cst)",
                ),
                (
                    "atomic_compare_exchange_strong_explicit",
                    "__atomic_compare_exchange_n((object), (expected), (desired), 0, (success), (failure))",
                ),
                (
                    "atomic_compare_exchange_weak",
                    "__atomic_compare_exchange_n((object), (expected), (desired), 1, memory_order_seq_cst, memory_order_seq_cst)",
                ),
                (
                    "atomic_compare_exchange_weak_explicit",
                    "__atomic_compare_exchange_n((object), (expected), (desired), 1, (success), (failure))",
                ),
                (
                    "atomic_flag_test_and_set",
                    "__atomic_exchange_n((object), 1, memory_order_seq_cst)",
                ),
                (
                    "atomic_flag_test_and_set_explicit",
                    "__atomic_exchange_n((object), 1, (order))",
                ),
                (
                    "atomic_flag_clear",
                    "__atomic_store_n((object), 0, memory_order_seq_cst)",
                ),
                (
                    "atomic_flag_clear_explicit",
                    "__atomic_store_n((object), 0, (order))",
                ),
                ("atomic_thread_fence", "__atomic_thread_fence((order))"),
                ("atomic_signal_fence", "__atomic_signal_fence((order))"),
                ("atomic_is_lock_free", "1"),
            ] {
                let params = match name {
                    "ATOMIC_VAR_INIT" => vec!["value"],
                    "atomic_init" => vec!["object", "desired"],
                    "atomic_load" => vec!["object"],
                    "atomic_load_explicit" => vec!["object", "order"],
                    "atomic_store" => vec!["object", "desired"],
                    "atomic_store_explicit" => vec!["object", "desired", "order"],
                    "atomic_exchange" => vec!["object", "desired"],
                    "atomic_exchange_explicit" => vec!["object", "desired", "order"],
                    "atomic_fetch_add"
                    | "atomic_fetch_sub"
                    | "atomic_fetch_or"
                    | "atomic_fetch_xor"
                    | "atomic_fetch_and" => vec!["object", "operand"],
                    "atomic_fetch_add_explicit"
                    | "atomic_fetch_sub_explicit"
                    | "atomic_fetch_or_explicit"
                    | "atomic_fetch_xor_explicit"
                    | "atomic_fetch_and_explicit" => vec!["object", "operand", "order"],
                    "atomic_compare_exchange_strong" => vec!["object", "expected", "desired"],
                    "atomic_compare_exchange_strong_explicit" => {
                        vec!["object", "expected", "desired", "success", "failure"]
                    }
                    "atomic_compare_exchange_weak" => vec!["object", "expected", "desired"],
                    "atomic_compare_exchange_weak_explicit" => {
                        vec!["object", "expected", "desired", "success", "failure"]
                    }
                    "atomic_flag_test_and_set" | "atomic_flag_clear" => vec!["object"],
                    "atomic_flag_test_and_set_explicit" | "atomic_flag_clear_explicit" => {
                        vec!["object", "order"]
                    }
                    "atomic_thread_fence" | "atomic_signal_fence" => vec!["order"],
                    "atomic_is_lock_free" => vec!["object"],
                    _ => Vec::new(),
                };
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: params.into_iter().map(str::to_string).collect(),
                        variadic: false,
                        body: body.to_string(),
                    },
                );
            }
            if virtual_header_include_once(macros, "stdatomic.h") {
                return String::new();
            }
            include_str!("virtual_headers/stdatomic.h").to_string()
        }
        "stdint.h" => {
            macros.insert(
                "INT8_MIN".to_string(),
                MacroDef::Object("(-128)".to_string()),
            );
            macros.insert("INT8_MAX".to_string(), MacroDef::Object("127".to_string()));
            macros.insert("UINT8_MAX".to_string(), MacroDef::Object("255".to_string()));
            macros.insert(
                "INT16_MIN".to_string(),
                MacroDef::Object("(-32768)".to_string()),
            );
            macros.insert(
                "INT16_MAX".to_string(),
                MacroDef::Object("32767".to_string()),
            );
            macros.insert(
                "UINT16_MAX".to_string(),
                MacroDef::Object("65535".to_string()),
            );
            macros.insert(
                "INT32_MIN".to_string(),
                MacroDef::Object("(-2147483647 - 1)".to_string()),
            );
            macros.insert(
                "INT32_MAX".to_string(),
                MacroDef::Object("2147483647".to_string()),
            );
            macros.insert(
                "UINT32_MAX".to_string(),
                MacroDef::Object("4294967295U".to_string()),
            );
            macros.insert(
                "INT64_MIN".to_string(),
                MacroDef::Object("(-9223372036854775807L - 1L)".to_string()),
            );
            macros.insert(
                "INT64_MAX".to_string(),
                MacroDef::Object("9223372036854775807L".to_string()),
            );
            macros.insert(
                "UINT64_MAX".to_string(),
                MacroDef::Object("18446744073709551615UL".to_string()),
            );
            macros.insert(
                "INT_LEAST8_MIN".to_string(),
                MacroDef::Object("(-128)".to_string()),
            );
            macros.insert(
                "INT_LEAST8_MAX".to_string(),
                MacroDef::Object("127".to_string()),
            );
            macros.insert(
                "UINT_LEAST8_MAX".to_string(),
                MacroDef::Object("255".to_string()),
            );
            macros.insert(
                "INT_LEAST16_MIN".to_string(),
                MacroDef::Object("(-32768)".to_string()),
            );
            macros.insert(
                "INT_LEAST16_MAX".to_string(),
                MacroDef::Object("32767".to_string()),
            );
            macros.insert(
                "UINT_LEAST16_MAX".to_string(),
                MacroDef::Object("65535".to_string()),
            );
            macros.insert(
                "INT_LEAST32_MIN".to_string(),
                MacroDef::Object("(-2147483647 - 1)".to_string()),
            );
            macros.insert(
                "INT_LEAST32_MAX".to_string(),
                MacroDef::Object("2147483647".to_string()),
            );
            macros.insert(
                "UINT_LEAST32_MAX".to_string(),
                MacroDef::Object("4294967295U".to_string()),
            );
            macros.insert(
                "INT_LEAST64_MIN".to_string(),
                MacroDef::Object("(-9223372036854775807L - 1L)".to_string()),
            );
            macros.insert(
                "INT_LEAST64_MAX".to_string(),
                MacroDef::Object("9223372036854775807L".to_string()),
            );
            macros.insert(
                "UINT_LEAST64_MAX".to_string(),
                MacroDef::Object("18446744073709551615UL".to_string()),
            );
            macros.insert(
                "INT_FAST8_MIN".to_string(),
                MacroDef::Object("(-2147483647 - 1)".to_string()),
            );
            macros.insert(
                "INT_FAST8_MAX".to_string(),
                MacroDef::Object("2147483647".to_string()),
            );
            macros.insert(
                "UINT_FAST8_MAX".to_string(),
                MacroDef::Object("4294967295U".to_string()),
            );
            macros.insert(
                "INT_FAST16_MIN".to_string(),
                MacroDef::Object("(-2147483647 - 1)".to_string()),
            );
            macros.insert(
                "INT_FAST16_MAX".to_string(),
                MacroDef::Object("2147483647".to_string()),
            );
            macros.insert(
                "UINT_FAST16_MAX".to_string(),
                MacroDef::Object("4294967295U".to_string()),
            );
            macros.insert(
                "INT_FAST32_MIN".to_string(),
                MacroDef::Object("(-2147483647 - 1)".to_string()),
            );
            macros.insert(
                "INT_FAST32_MAX".to_string(),
                MacroDef::Object("2147483647".to_string()),
            );
            macros.insert(
                "UINT_FAST32_MAX".to_string(),
                MacroDef::Object("4294967295U".to_string()),
            );
            macros.insert(
                "INT_FAST64_MIN".to_string(),
                MacroDef::Object("(-9223372036854775807L - 1L)".to_string()),
            );
            macros.insert(
                "INT_FAST64_MAX".to_string(),
                MacroDef::Object("9223372036854775807L".to_string()),
            );
            macros.insert(
                "UINT_FAST64_MAX".to_string(),
                MacroDef::Object("18446744073709551615UL".to_string()),
            );
            macros.insert(
                "INTMAX_MIN".to_string(),
                MacroDef::Object("(-9223372036854775807L - 1L)".to_string()),
            );
            macros.insert(
                "INTMAX_MAX".to_string(),
                MacroDef::Object("9223372036854775807L".to_string()),
            );
            macros.insert(
                "UINTMAX_MAX".to_string(),
                MacroDef::Object("18446744073709551615UL".to_string()),
            );
            macros.insert(
                "INTPTR_MIN".to_string(),
                MacroDef::Object("(-9223372036854775807L - 1L)".to_string()),
            );
            macros.insert(
                "INTPTR_MAX".to_string(),
                MacroDef::Object("9223372036854775807L".to_string()),
            );
            macros.insert(
                "UINTPTR_MAX".to_string(),
                MacroDef::Object("18446744073709551615UL".to_string()),
            );
            macros.insert(
                "PTRDIFF_MIN".to_string(),
                MacroDef::Object("(-9223372036854775807L - 1L)".to_string()),
            );
            macros.insert(
                "PTRDIFF_MAX".to_string(),
                MacroDef::Object("9223372036854775807L".to_string()),
            );
            macros.insert(
                "SIZE_MAX".to_string(),
                MacroDef::Object("18446744073709551615UL".to_string()),
            );
            macros.insert(
                "INT8_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c".to_string(),
                },
            );
            macros.insert(
                "UINT8_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c".to_string(),
                },
            );
            macros.insert(
                "INT16_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c".to_string(),
                },
            );
            macros.insert(
                "UINT16_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c".to_string(),
                },
            );
            macros.insert(
                "INT32_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c".to_string(),
                },
            );
            macros.insert(
                "UINT32_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c ## U".to_string(),
                },
            );
            macros.insert(
                "INT64_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c ## L".to_string(),
                },
            );
            macros.insert(
                "UINT64_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c ## UL".to_string(),
                },
            );
            macros.insert(
                "INTMAX_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c ## L".to_string(),
                },
            );
            macros.insert(
                "UINTMAX_C".to_string(),
                MacroDef::Function {
                    params: vec!["c".to_string()],
                    variadic: false,
                    body: "c ## UL".to_string(),
                },
            );
            if virtual_header_include_once(macros, "stdint.h") {
                return String::new();
            }
            include_str!("virtual_headers/stdint.h").to_string()
        }
        "inttypes.h" => {
            let stdint = include_virtual_compat_header("stdint.h", macros);
            for (name, value) in [
                ("PRId8", "\"d\""),
                ("PRIi8", "\"i\""),
                ("PRIo8", "\"o\""),
                ("PRIu8", "\"u\""),
                ("PRIx8", "\"x\""),
                ("PRIX8", "\"X\""),
                ("PRId16", "\"d\""),
                ("PRIi16", "\"i\""),
                ("PRIo16", "\"o\""),
                ("PRIu16", "\"u\""),
                ("PRIx16", "\"x\""),
                ("PRIX16", "\"X\""),
                ("PRId32", "\"d\""),
                ("PRIi32", "\"i\""),
                ("PRIo32", "\"o\""),
                ("PRIu32", "\"u\""),
                ("PRIx32", "\"x\""),
                ("PRIX32", "\"X\""),
                ("PRId64", "\"ld\""),
                ("PRIi64", "\"li\""),
                ("PRIo64", "\"lo\""),
                ("PRIu64", "\"lu\""),
                ("PRIx64", "\"lx\""),
                ("PRIX64", "\"lX\""),
                ("PRIdMAX", "\"ld\""),
                ("PRIiMAX", "\"li\""),
                ("PRIoMAX", "\"lo\""),
                ("PRIuMAX", "\"lu\""),
                ("PRIxMAX", "\"lx\""),
                ("PRIXMAX", "\"lX\""),
                ("PRIdPTR", "\"ld\""),
                ("PRIiPTR", "\"li\""),
                ("PRIoPTR", "\"lo\""),
                ("PRIuPTR", "\"lu\""),
                ("PRIxPTR", "\"lx\""),
                ("PRIXPTR", "\"lX\""),
            ] {
                macros.insert(name.to_string(), MacroDef::Object(value.to_string()));
            }
            stdint
        }
        "float.h" => {
            for (name, value) in [
                ("FLT_RADIX", "2"),
                ("FLT_MANT_DIG", "24"),
                ("DBL_MANT_DIG", "53"),
                ("LDBL_MANT_DIG", "53"),
                ("FLT_DIG", "6"),
                ("DBL_DIG", "15"),
                ("LDBL_DIG", "15"),
                ("FLT_MIN_EXP", "(-125)"),
                ("DBL_MIN_EXP", "(-1021)"),
                ("LDBL_MIN_EXP", "(-1021)"),
                ("FLT_MAX_EXP", "128"),
                ("DBL_MAX_EXP", "1024"),
                ("LDBL_MAX_EXP", "1024"),
                ("FLT_MIN", "1.17549435e-38F"),
                ("DBL_MIN", "2.2250738585072014e-308"),
                ("LDBL_MIN", "2.2250738585072014e-308L"),
                ("FLT_MAX", "3.40282347e+38F"),
                ("DBL_MAX", "1.7976931348623157e+308"),
                ("LDBL_MAX", "1.7976931348623157e+308L"),
                ("FLT_EPSILON", "1.19209290e-7F"),
                ("DBL_EPSILON", "2.2204460492503131e-16"),
                ("LDBL_EPSILON", "2.2204460492503131e-16L"),
            ] {
                macros.insert(name.to_string(), MacroDef::Object(value.to_string()));
            }
            String::new()
        }
        "stdalign.h" => {
            macros.insert(
                "alignas".to_string(),
                MacroDef::Object("_Alignas".to_string()),
            );
            macros.insert(
                "alignof".to_string(),
                MacroDef::Object("_Alignof".to_string()),
            );
            macros.insert(
                "__alignas_is_defined".to_string(),
                MacroDef::Object("1".to_string()),
            );
            macros.insert(
                "__alignof_is_defined".to_string(),
                MacroDef::Object("1".to_string()),
            );
            String::new()
        }
        "stdnoreturn.h" => {
            macros.insert(
                "noreturn".to_string(),
                MacroDef::Object("_Noreturn".to_string()),
            );
            String::new()
        }
        "iso646.h" => {
            for (name, replacement) in [
                ("and", "&&"),
                ("and_eq", "&="),
                ("bitand", "&"),
                ("bitor", "|"),
                ("compl", "~"),
                ("not", "!"),
                ("not_eq", "!="),
                ("or", "||"),
                ("or_eq", "|="),
                ("xor", "^"),
                ("xor_eq", "^="),
            ] {
                macros.insert(name.to_string(), MacroDef::Object(replacement.to_string()));
            }
            String::new()
        }
        "limits.h" => {
            macros.insert("CHAR_BIT".to_string(), MacroDef::Object("8".to_string()));
            macros.insert(
                "SCHAR_MIN".to_string(),
                MacroDef::Object("(-128)".to_string()),
            );
            macros.insert("SCHAR_MAX".to_string(), MacroDef::Object("127".to_string()));
            macros.insert("UCHAR_MAX".to_string(), MacroDef::Object("255".to_string()));
            macros.insert(
                "SHRT_MIN".to_string(),
                MacroDef::Object("(-32768)".to_string()),
            );
            macros.insert(
                "SHRT_MAX".to_string(),
                MacroDef::Object("32767".to_string()),
            );
            macros.insert(
                "USHRT_MAX".to_string(),
                MacroDef::Object("65535".to_string()),
            );
            macros.insert(
                "INT_MIN".to_string(),
                MacroDef::Object("(-2147483647 - 1)".to_string()),
            );
            macros.insert(
                "INT_MAX".to_string(),
                MacroDef::Object("2147483647".to_string()),
            );
            macros.insert(
                "UINT_MAX".to_string(),
                MacroDef::Object("4294967295U".to_string()),
            );
            macros.insert(
                "LONG_MIN".to_string(),
                MacroDef::Object("(-9223372036854775807L - 1L)".to_string()),
            );
            macros.insert(
                "LONG_MAX".to_string(),
                MacroDef::Object("9223372036854775807L".to_string()),
            );
            macros.insert(
                "ULONG_MAX".to_string(),
                MacroDef::Object("18446744073709551615UL".to_string()),
            );
            String::new()
        }
        _ => String::new(),
    }
}

fn generated_header_dependency(spec: &IncludeSpec) -> PathBuf {
    match spec {
        IncludeSpec::Quoted(name) | IncludeSpec::Angled(name) => PathBuf::from(name),
    }
}

fn is_system_dependency(path: &Path, include_paths: &IncludePaths) -> bool {
    let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    include_paths
        .system
        .iter()
        .chain(include_paths.after.iter())
        .any(|dir| {
            let canonical_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
            canonical_path.starts_with(canonical_dir)
        })
}

struct ConditionalFrame {
    parent_active: bool,
    condition_active: bool,
    branch_taken: bool,
    saw_else: bool,
}

struct InternalPreprocessContext<'a> {
    include_stack: &'a mut Vec<PathBuf>,
    once_files: &'a mut HashSet<PathBuf>,
    system_header_files: &'a mut HashSet<PathBuf>,
    poisoned_identifiers: &'a mut HashSet<String>,
    pragma_pack_stack: &'a mut Vec<Option<usize>>,
    pragma_pack_alignment: &'a mut Option<usize>,
    include_paths: &'a IncludePaths,
    dependencies: &'a mut Vec<PathBuf>,
    user_dependencies_only: bool,
    missing_headers_generated: bool,
    suppress_preprocessed_output: bool,
    trace_includes: bool,
    line_markers: bool,
}

fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn trace_include(path: &Path, context: &InternalPreprocessContext<'_>) {
    if context.trace_includes {
        let depth = context.include_stack.len().max(1);
        eprintln!("{} {}", ".".repeat(depth), path.display());
    }
}

fn inactive_recursive_include_guard(source: &str, macros: &HashMap<String, MacroDef>) -> bool {
    let Some(line) = source.lines().find(|line| !line.trim().is_empty()) else {
        return false;
    };
    let Some(rest) = trim_preprocessor_prefix(line.trim_start()) else {
        return false;
    };
    if let Some(name) = rest.strip_prefix("ifndef").and_then(|rest| {
        let rest = rest.trim_start();
        let end = rest
            .char_indices()
            .find(|(_, ch)| !is_ident_continue(*ch))
            .map(|(index, _)| index)
            .unwrap_or(rest.len());
        (end > 0).then_some(&rest[..end])
    }) {
        return macros.contains_key(name);
    }
    let Some(expr) = rest.strip_prefix("if").map(str::trim_start) else {
        return false;
    };
    let Some(defined_operand) = expr.strip_prefix("!defined").map(str::trim_start) else {
        return false;
    };
    let name = if let Some(inner) = defined_operand
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    {
        inner.trim()
    } else {
        defined_operand.trim()
    };
    !name.is_empty() && macros.contains_key(name)
}

fn is_marked_system_header(path: &Path, context: &InternalPreprocessContext<'_>) -> bool {
    context.system_header_files.contains(&canonical_path(path))
}

fn should_record_dependency(path: &Path, context: &InternalPreprocessContext<'_>) -> bool {
    !context.user_dependencies_only
        || (!is_system_dependency(path, context.include_paths)
            && !is_marked_system_header(path, context))
}

fn record_dependency(path: &Path, context: &mut InternalPreprocessContext<'_>) {
    if should_record_dependency(path, context) {
        context.dependencies.push(path.to_path_buf());
    }
}

fn unrecord_dependency(path: &Path, context: &mut InternalPreprocessContext<'_>) {
    if context.user_dependencies_only && is_marked_system_header(path, context) {
        let canonical = canonical_path(path);
        context
            .dependencies
            .retain(|dep| canonical_path(dep) != canonical);
    }
}

fn poison_identifiers_from_pragma(pragma: &str, context: &mut InternalPreprocessContext<'_>) {
    let Some(rest) = pragma.strip_prefix("GCC poison") else {
        return;
    };
    for name in rest.split_whitespace() {
        if name.chars().next().is_some_and(is_ident_start) && name.chars().all(is_ident_continue) {
            context.poisoned_identifiers.insert(name.to_string());
        }
    }
}

enum PragmaPackAction {
    Set(Option<usize>),
    Push(Option<usize>),
    Pop,
}

fn parse_pack_alignment(text: &str) -> Option<usize> {
    let value = text.trim().parse::<usize>().ok()?;
    if matches!(value, 1 | 2 | 4 | 8 | 16) {
        Some(value)
    } else {
        None
    }
}

fn parse_pragma_pack(pragma: &str) -> Option<PragmaPackAction> {
    let rest = pragma.trim().strip_prefix("pack")?.trim();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?.trim();
    if inner.is_empty() || inner == "0" {
        return Some(PragmaPackAction::Set(None));
    }
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    match parts.as_slice() {
        ["push"] => Some(PragmaPackAction::Push(None)),
        ["push", value] => {
            parse_pack_alignment(value).map(|value| PragmaPackAction::Push(Some(value)))
        }
        ["pop"] => Some(PragmaPackAction::Pop),
        [value] => parse_pack_alignment(value).map(|value| PragmaPackAction::Set(Some(value))),
        _ => None,
    }
}

fn handle_internal_pragma(
    pragma: &str,
    canonical: &Path,
    context: &mut InternalPreprocessContext<'_>,
) {
    if pragma == "once" {
        context.once_files.insert(canonical.to_path_buf());
    } else if pragma == "GCC system_header" || pragma == "clang system_header" {
        context.system_header_files.insert(canonical.to_path_buf());
    } else if let Some(action) = parse_pragma_pack(pragma) {
        match action {
            PragmaPackAction::Set(alignment) => *context.pragma_pack_alignment = alignment,
            PragmaPackAction::Push(alignment) => {
                context
                    .pragma_pack_stack
                    .push(*context.pragma_pack_alignment);
                if let Some(alignment) = alignment {
                    *context.pragma_pack_alignment = Some(alignment);
                }
            }
            PragmaPackAction::Pop => {
                *context.pragma_pack_alignment = context.pragma_pack_stack.pop().unwrap_or(None);
            }
        }
    } else {
        poison_identifiers_from_pragma(pragma, context);
    }
}

fn inject_pack_attributes(text: &str, alignment: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;
    let mut in_string = false;
    let mut in_char = false;
    while index < chars.len() {
        let ch = chars[index];
        if in_string || in_char {
            out.push(ch);
            if ch == '\\' {
                index += 1;
                if let Some(next) = chars.get(index) {
                    out.push(*next);
                }
            } else if in_string && ch == '"' {
                in_string = false;
            } else if in_char && ch == '\'' {
                in_char = false;
            }
            index += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            index += 1;
            continue;
        }
        if ch == '\'' {
            in_char = true;
            out.push(ch);
            index += 1;
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = index;
            index += 1;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
            {
                index += 1;
            }
            let ident: String = chars[start..index].iter().collect();
            out.push_str(&ident);
            if ident == "struct" || ident == "union" {
                if alignment == 1 {
                    out.push_str(" __attribute__((packed))");
                } else {
                    out.push_str(&format!(" __attribute__((packed, aligned({alignment})))"));
                }
            }
            continue;
        }
        out.push(ch);
        index += 1;
    }
    out
}

fn check_poisoned_tokens(
    tokens: &[preprocess::token::PpToken],
    context: &InternalPreprocessContext<'_>,
) -> Result<(), String> {
    for token in tokens {
        if let preprocess::token::PpTokenKind::Ident(name) = &token.kind {
            if context.poisoned_identifiers.contains(name) {
                return Err(format!("attempt to use poisoned identifier {}", name));
            }
        }
    }
    Ok(())
}

fn check_poisoned_line(line: &str, context: &InternalPreprocessContext<'_>) -> Result<(), String> {
    let tokens = preprocess::lexer::lex(line)?;
    check_poisoned_tokens(&tokens, context)
}

fn pp_location(file: &str, line: usize, message: impl AsRef<str>) -> String {
    format!("{}:{}: {}", file, line, message.as_ref())
}

fn push_line_marker(out: &mut String, line: usize, file: &str) {
    out.push_str(&format!("# {} \"{}\"\n", line, escape_c_string(file)));
}

fn conditionals_active(stack: &[ConditionalFrame]) -> bool {
    stack
        .last()
        .map(|frame| frame.parent_active && frame.condition_active)
        .unwrap_or(true)
}

struct IfExprParser<'a> {
    chars: Vec<char>,
    pos: usize,
    macros: &'a HashMap<String, MacroDef>,
}

struct IfEvalContext<'a> {
    file: &'a str,
    line_number: usize,
    base_dir: &'a Path,
    include_paths: &'a IncludePaths,
    include_level: usize,
}

impl<'a> IfExprParser<'a> {
    fn new(expr: &str, macros: &'a HashMap<String, MacroDef>) -> Self {
        Self {
            chars: expr.chars().collect(),
            pos: 0,
            macros,
        }
    }

    fn parse(mut self) -> Result<i128, String> {
        let value = self.parse_conditional(true)?;
        self.skip_ws();
        if self.pos != self.chars.len() {
            return Err(format!(
                "unsupported #if expression near '{}'",
                self.chars[self.pos..].iter().collect::<String>()
            ));
        }
        Ok(value)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, token: &str) -> bool {
        self.skip_ws();
        self.starts_with(token) && {
            self.pos += token.chars().count();
            true
        }
    }

    fn starts_with(&mut self, token: &str) -> bool {
        self.skip_ws();
        let token_chars: Vec<char> = token.chars().collect();
        self.chars[self.pos..].starts_with(&token_chars)
    }

    fn ident(&mut self) -> Option<String> {
        self.skip_ws();
        if self.pos >= self.chars.len() || !is_ident_start(self.chars[self.pos]) {
            return None;
        }
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.chars.len() && is_ident_continue(self.chars[self.pos]) {
            self.pos += 1;
        }
        Some(self.chars[start..self.pos].iter().collect())
    }

    fn number(&mut self) -> Result<Option<i128>, String> {
        self.skip_ws();
        let start = self.pos;
        if self.pos >= self.chars.len() || !self.chars[self.pos].is_ascii_digit() {
            return Ok(None);
        }

        let mut base = 10;
        let digits_start;
        if self.chars[self.pos] == '0'
            && self.pos + 1 < self.chars.len()
            && matches!(self.chars[self.pos + 1], 'x' | 'X')
        {
            base = 16;
            self.pos += 2;
            digits_start = self.pos;
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            if self.pos == digits_start {
                return Err("expected hexadecimal digits in #if expression".to_string());
            }
        } else if self.chars[self.pos] == '0'
            && self.pos + 1 < self.chars.len()
            && matches!(self.chars[self.pos + 1], 'b' | 'B')
        {
            base = 2;
            self.pos += 2;
            digits_start = self.pos;
            while self.pos < self.chars.len() && matches!(self.chars[self.pos], '0' | '1') {
                self.pos += 1;
            }
            if self.pos == digits_start {
                return Err("expected binary digits in #if expression".to_string());
            }
        } else if self.chars[self.pos] == '0' {
            base = 8;
            digits_start = self.pos;
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                if !matches!(self.chars[self.pos], '0'..='7') {
                    return Err("invalid octal digit in #if expression".to_string());
                }
                self.pos += 1;
            }
        } else {
            digits_start = self.pos;
            while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }

        let digits_end = self.pos;
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_alphabetic() {
            if !matches!(self.chars[self.pos], 'u' | 'U' | 'l' | 'L') {
                return Err(format!(
                    "invalid integer literal suffix in #if expression near '{}'",
                    self.chars[start..=self.pos].iter().collect::<String>()
                ));
            }
            self.pos += 1;
        }

        if self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_digit() || self.chars[self.pos] == '_')
        {
            return Err(format!(
                "invalid integer literal in #if expression near '{}'",
                self.chars[start..=self.pos].iter().collect::<String>()
            ));
        }

        let digits = self.chars[digits_start..digits_end]
            .iter()
            .collect::<String>();
        let value = u128::from_str_radix(&digits, base)
            .map_err(|_| format!("invalid integer literal in #if expression: {}", digits))?;
        let value = value.min(i128::MAX as u128) as i128;
        Ok(Some(value))
    }

    fn char_constant(&mut self) -> Result<Option<i128>, String> {
        self.skip_ws();
        if self.pos >= self.chars.len() || self.chars[self.pos] != '\'' {
            return Ok(None);
        }
        self.pos += 1;
        let mut value = 0i128;
        let mut saw_char = false;
        loop {
            if self.pos >= self.chars.len() {
                return Err("unterminated character constant in #if expression".to_string());
            }
            let ch = self.chars[self.pos];
            self.pos += 1;
            if ch == '\'' {
                if !saw_char {
                    return Err("empty character constant in #if expression".to_string());
                }
                return Ok(Some(value));
            }
            let unit = if ch == '\\' {
                if self.pos >= self.chars.len() {
                    return Err("unterminated escape in #if expression".to_string());
                }
                let escaped = self.chars[self.pos];
                self.pos += 1;
                match escaped {
                    '0' => 0,
                    'a' => 7,
                    'b' => 8,
                    't' => 9,
                    'n' => 10,
                    'v' => 11,
                    'f' => 12,
                    'r' => 13,
                    '\\' => '\\' as i128,
                    '\'' => '\'' as i128,
                    '"' => '"' as i128,
                    other => other as i128,
                }
            } else {
                ch as i128
            };
            value = (value << 8) | unit;
            saw_char = true;
        }
    }

    fn parse_primary(&mut self, eval: bool) -> Result<i128, String> {
        if self.eat("(") {
            let value = self.parse_conditional(eval)?;
            if !self.eat(")") {
                return Err("missing ')' in #if expression".to_string());
            }
            return Ok(value);
        }
        if let Some(number) = self.number()? {
            return Ok(number);
        }
        if let Some(value) = self.char_constant()? {
            return Ok(value);
        }
        if let Some(ident) = self.ident() {
            if ident == "defined" {
                if self.eat("(") {
                    let Some(name) = self.ident() else {
                        return Err("expected macro name after defined(".to_string());
                    };
                    if !self.eat(")") {
                        return Err("missing ')' after defined macro name".to_string());
                    }
                    return Ok((eval && self.macros.contains_key(&name)) as i128);
                }
                let Some(name) = self.ident() else {
                    return Err("expected macro name after defined".to_string());
                };
                return Ok((eval && self.macros.contains_key(&name)) as i128);
            }
            return Ok(0);
        }
        Err("expected value in #if expression".to_string())
    }

    fn parse_unary(&mut self, eval: bool) -> Result<i128, String> {
        if self.eat("!") {
            let value = self.parse_unary(eval)?;
            Ok((eval && value == 0) as i128)
        } else if self.eat("~") {
            let value = self.parse_unary(eval)?;
            Ok(if eval { !value } else { 0 })
        } else if self.eat("-") {
            let value = self.parse_unary(eval)?;
            Ok(if eval { value.wrapping_neg() } else { 0 })
        } else if self.eat("+") {
            self.parse_unary(eval)
        } else {
            self.parse_primary(eval)
        }
    }

    fn parse_mul(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_unary(eval)?;
        loop {
            if self.eat("*") {
                let right = self.parse_unary(eval)?;
                if eval {
                    left = left.wrapping_mul(right);
                }
            } else if self.eat("/") {
                let right = self.parse_unary(eval)?;
                if eval && right == 0 {
                    return Err("division by zero in #if expression".to_string());
                }
                if eval {
                    left = left
                        .checked_div(right)
                        .ok_or_else(|| "overflow in #if division".to_string())?;
                }
            } else if self.eat("%") {
                let right = self.parse_unary(eval)?;
                if eval && right == 0 {
                    return Err("division by zero in #if expression".to_string());
                }
                if eval {
                    left = left
                        .checked_rem(right)
                        .ok_or_else(|| "overflow in #if remainder".to_string())?;
                }
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_add(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_mul(eval)?;
        loop {
            if self.eat("+") {
                let right = self.parse_mul(eval)?;
                if eval {
                    left = left.wrapping_add(right);
                }
            } else if self.eat("-") {
                let right = self.parse_mul(eval)?;
                if eval {
                    left = left.wrapping_sub(right);
                }
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_shift(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_add(eval)?;
        loop {
            if self.eat("<<") {
                let right = self.parse_add(eval)?;
                if eval && right < 0 {
                    return Err("negative shift count in #if expression".to_string());
                }
                if eval {
                    left = left.wrapping_shl(right as u32);
                }
            } else if self.eat(">>") {
                let right = self.parse_add(eval)?;
                if eval && right < 0 {
                    return Err("negative shift count in #if expression".to_string());
                }
                if eval {
                    left = left.wrapping_shr(right as u32);
                }
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_relational(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_shift(eval)?;
        loop {
            if self.eat("<=") {
                let right = self.parse_shift(eval)?;
                left = (eval && left <= right) as i128;
            } else if self.eat(">=") {
                let right = self.parse_shift(eval)?;
                left = (eval && left >= right) as i128;
            } else if self.eat("<") {
                let right = self.parse_shift(eval)?;
                left = (eval && left < right) as i128;
            } else if self.eat(">") {
                let right = self.parse_shift(eval)?;
                left = (eval && left > right) as i128;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_equality(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_relational(eval)?;
        loop {
            if self.eat("==") {
                let right = self.parse_relational(eval)?;
                left = (eval && left == right) as i128;
            } else if self.eat("!=") {
                let right = self.parse_relational(eval)?;
                left = (eval && left != right) as i128;
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_bit_and(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_equality(eval)?;
        while !self.starts_with("&&") && self.eat("&") {
            let right = self.parse_equality(eval)?;
            if eval {
                left &= right;
            }
        }
        Ok(left)
    }

    fn parse_bit_xor(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_bit_and(eval)?;
        while self.eat("^") {
            let right = self.parse_bit_and(eval)?;
            if eval {
                left ^= right;
            }
        }
        Ok(left)
    }

    fn parse_bit_or(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_bit_xor(eval)?;
        while !self.starts_with("||") && self.eat("|") {
            let right = self.parse_bit_xor(eval)?;
            if eval {
                left |= right;
            }
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_bit_or(eval)?;
        while self.eat("&&") {
            let right_eval = eval && left != 0;
            let right = self.parse_bit_or(right_eval)?;
            left = (right_eval && right != 0) as i128;
        }
        Ok(left)
    }

    fn parse_logical_or(&mut self, eval: bool) -> Result<i128, String> {
        let mut left = self.parse_logical_and(eval)?;
        while self.eat("||") {
            let right_eval = eval && left == 0;
            let right = self.parse_logical_and(right_eval)?;
            left = (eval && (left != 0 || right != 0)) as i128;
        }
        Ok(left)
    }

    fn parse_conditional(&mut self, eval: bool) -> Result<i128, String> {
        let condition = self.parse_logical_or(eval)?;
        if self.eat("?") {
            let true_eval = eval && condition != 0;
            let when_true = self.parse_conditional(true_eval)?;
            if !self.eat(":") {
                return Err("missing ':' in #if conditional expression".to_string());
            }
            let false_eval = eval && condition == 0;
            let when_false = self.parse_conditional(false_eval)?;
            if true_eval {
                Ok(when_true)
            } else {
                Ok(when_false)
            }
        } else {
            Ok(condition)
        }
    }
}

fn eval_internal_if(
    expr: &str,
    macros: &HashMap<String, MacroDef>,
    state: &mut PreprocessorState,
    context: IfEvalContext<'_>,
) -> Result<bool, String> {
    let expr = replace_preprocessor_predicates(expr.trim(), macros, state, &context)?;
    let expanded = expand_macros_with_context(
        &expr,
        macros,
        context.file,
        context.line_number,
        context.include_level,
        state,
    )?;
    IfExprParser::new(&expanded, macros)
        .parse()
        .map(|value| value != 0)
        .map_err(|err| format!("unsupported #if expression '{}': {}", expr, err))
}

struct PendingSource {
    text: String,
    logical_file: String,
    start_line: usize,
}

fn flush_pending_source(
    pending: &mut Option<PendingSource>,
    out: &mut String,
    macros: &HashMap<String, MacroDef>,
    context: &mut InternalPreprocessContext<'_>,
    canonical: &Path,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<(), String> {
    let Some(pending_source) = pending.take() else {
        return Ok(());
    };

    check_poisoned_line(&pending_source.text, context)
        .map_err(|err| pp_location(&pending_source.logical_file, pending_source.start_line, err))?;
    if context.suppress_preprocessed_output {
        return Ok(());
    }

    let expanded = expand_macros_with_context(
        &pending_source.text,
        macros,
        &pending_source.logical_file,
        pending_source.start_line,
        include_level,
        state,
    )?;
    let (expanded, pragmas) = process_pragma_operators(&expanded)?;
    for pragma in pragmas {
        handle_internal_pragma(pragma.trim(), canonical, context);
    }
    let expanded = context
        .pragma_pack_alignment
        .map(|alignment| inject_pack_attributes(&expanded, alignment))
        .unwrap_or(expanded);
    check_poisoned_line(&expanded, context)
        .map_err(|err| pp_location(&pending_source.logical_file, pending_source.start_line, err))?;
    if !expanded.trim().is_empty() {
        out.push_str(&expanded);
        if !expanded.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(())
}

fn replace_preprocessor_predicates(
    expr: &str,
    macros: &HashMap<String, MacroDef>,
    state: &mut PreprocessorState,
    context: &IfEvalContext<'_>,
) -> Result<String, String> {
    let tokens = preprocess::lexer::lex(expr)?;
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if let Some(parsed) = preprocess::predicate::parse_predicate_operand(&tokens, index) {
            match parsed.operand {
                preprocess::predicate::PredicateOperand::Defined { name } => {
                    out.push(tokens[index].clone_with_text(
                        preprocess::token::PpTokenKind::Number(
                            if macros.contains_key(&name) { "1" } else { "0" }.to_string(),
                        ),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasInclude {
                    operand,
                    include_next,
                } => {
                    let Some(spec) = parse_token_include_operand(
                        &operand,
                        macros,
                        context.file,
                        context.line_number,
                        context.include_level,
                        state,
                    ) else {
                        return Err(format!(
                            "unsupported #if expression '{}': malformed __has_include operand",
                            expr
                        ));
                    };
                    let found = resolve_include_path(
                        &spec,
                        context.base_dir,
                        context.include_paths,
                        include_next,
                    )
                    .is_some();
                    let found =
                        found || (!include_next && virtual_compat_header_name(&spec).is_some());
                    out.push(tokens[index].clone_with_text(
                        preprocess::token::PpTokenKind::Number(
                            if found { "1" } else { "0" }.to_string(),
                        ),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasBuiltin { name } => {
                    out.push(
                        tokens[index].clone_with_text(preprocess::token::PpTokenKind::Number(
                            if internal_has_builtin(&name) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                        )),
                    );
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasAttribute { name } => {
                    out.push(
                        tokens[index].clone_with_text(preprocess::token::PpTokenKind::Number(
                            if internal_has_attribute(&name) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                        )),
                    );
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasCAttribute { name } => {
                    out.push(
                        tokens[index].clone_with_text(preprocess::token::PpTokenKind::Number(
                            if internal_has_c_attribute(&name) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                        )),
                    );
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasDeclspecAttribute { name } => {
                    out.push(
                        tokens[index].clone_with_text(preprocess::token::PpTokenKind::Number(
                            if internal_has_declspec_attribute(&name) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                        )),
                    );
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasFeature { name } => {
                    out.push(
                        tokens[index].clone_with_text(preprocess::token::PpTokenKind::Number(
                            if internal_has_feature(&name) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                        )),
                    );
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasExtension { name } => {
                    out.push(
                        tokens[index].clone_with_text(preprocess::token::PpTokenKind::Number(
                            if internal_has_extension(&name) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                        )),
                    );
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasWarning { name } => {
                    out.push(
                        tokens[index].clone_with_text(preprocess::token::PpTokenKind::Number(
                            if internal_has_warning(&name) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                        )),
                    );
                    index = parsed.next_index;
                }
            }
        } else {
            out.push(tokens[index].clone());
            index += 1;
        }
    }
    Ok(preprocess::emit::emit_tokens(&out))
}

fn internal_has_builtin(name: &str) -> bool {
    matches!(
        name,
        "__builtin_expect"
            | "__builtin_expect_with_probability"
            | "__builtin_types_compatible_p"
            | "__builtin_choose_expr"
            | "__builtin_offsetof"
            | "__builtin_unreachable"
            | "__builtin_trap"
            | "__builtin_constant_p"
            | "__builtin_assume_aligned"
            | "__builtin_prefetch"
            | "__builtin_bswap32"
            | "__builtin_bswap64"
            | "__builtin_object_size"
            | "__builtin_dynamic_object_size"
            | "__builtin_memcpy"
            | "__builtin_memmove"
            | "__builtin_memset"
            | "__builtin_memcmp"
            | "__builtin_memchr"
            | "__builtin_strlen"
            | "__builtin_strcmp"
            | "__builtin_strncmp"
            | "__builtin_strchr"
            | "__builtin_strrchr"
            | "__builtin_strstr"
            | "__builtin_strspn"
            | "__builtin_strcspn"
            | "__builtin_strcpy"
            | "__builtin_strncpy"
            | "__builtin_strcat"
            | "__builtin_strncat"
            | "__builtin___memcpy_chk"
            | "__builtin___memmove_chk"
            | "__builtin___memset_chk"
            | "__builtin___strcpy_chk"
            | "__builtin___strncpy_chk"
            | "__builtin___strcat_chk"
            | "__builtin___strncat_chk"
            | "__atomic_load_n"
            | "__atomic_store_n"
            | "__atomic_exchange_n"
            | "__atomic_compare_exchange_n"
            | "__atomic_thread_fence"
            | "__atomic_signal_fence"
            | "__atomic_add_fetch"
            | "__atomic_sub_fetch"
            | "__atomic_and_fetch"
            | "__atomic_or_fetch"
            | "__atomic_xor_fetch"
            | "__atomic_fetch_add"
            | "__atomic_fetch_sub"
            | "__atomic_fetch_and"
            | "__atomic_fetch_nand"
            | "__atomic_fetch_or"
            | "__atomic_fetch_xor"
            | "__atomic_nand_fetch"
            | "__sync_add_and_fetch"
            | "__sync_sub_and_fetch"
            | "__sync_and_and_fetch"
            | "__sync_nand_and_fetch"
            | "__sync_or_and_fetch"
            | "__sync_xor_and_fetch"
            | "__sync_fetch_and_add"
            | "__sync_fetch_and_sub"
            | "__sync_fetch_and_and"
            | "__sync_fetch_and_nand"
            | "__sync_fetch_and_or"
            | "__sync_fetch_and_xor"
            | "__sync_bool_compare_and_swap"
            | "__sync_val_compare_and_swap"
            | "__sync_synchronize"
    )
}

fn internal_has_attribute(name: &str) -> bool {
    matches!(
        name,
        "aligned"
            | "__aligned__"
            | "always_inline"
            | "__always_inline__"
            | "deprecated"
            | "fallthrough"
            | "__fallthrough__"
            | "format"
            | "__format__"
            | "cold"
            | "__cold__"
            | "const"
            | "__const__"
            | "hot"
            | "__hot__"
            | "malloc"
            | "__malloc__"
            | "noinline"
            | "__noinline__"
            | "nonnull"
            | "__nonnull__"
            | "noreturn"
            | "__noreturn__"
            | "packed"
            | "__packed__"
            | "pure"
            | "__pure__"
            | "returns_nonnull"
            | "__returns_nonnull__"
            | "unused"
            | "__unused__"
            | "visibility"
            | "__visibility__"
            | "warn_unused_result"
            | "__warn_unused_result__"
    )
}

fn internal_has_c_attribute(name: &str) -> bool {
    matches!(
        name,
        "deprecated" | "fallthrough" | "maybe_unused" | "nodiscard" | "noreturn"
    )
}

fn internal_has_declspec_attribute(name: &str) -> bool {
    matches!(name, "dllexport" | "dllimport" | "noreturn")
}

fn internal_has_feature(name: &str) -> bool {
    matches!(
        name,
        "c_alignas"
            | "c_alignof"
            | "c_atomic"
            | "c_static_assert"
            | "c_thread_local"
            | "attribute_deprecated_with_message"
            | "attribute_unavailable_with_message"
    )
}

fn internal_has_extension(name: &str) -> bool {
    internal_has_feature(name)
}

fn internal_has_warning(name: &str) -> bool {
    matches!(
        name,
        "-Wall" | "-Wunreachable" | "-Wmissing-return" | "-Werror" | "-Wunknown-pragmas"
    )
}

fn define_builtin_macro(macros: &mut HashMap<String, MacroDef>, name: &str, value: &str) {
    macros.insert(name.to_string(), MacroDef::Object(value.to_string()));
}

fn define_empty_function_macro(macros: &mut HashMap<String, MacroDef>, name: &str, variadic: bool) {
    macros.insert(
        name.to_string(),
        MacroDef::Function {
            params: Vec::new(),
            variadic,
            body: String::new(),
        },
    );
}

fn seed_internal_predefined_macros(macros: &mut HashMap<String, MacroDef>, target: &Target) {
    define_builtin_macro(macros, "__RNQCC__", "1");
    define_builtin_macro(macros, "__STDC__", "1");
    define_builtin_macro(macros, "__STDC_HOSTED__", "0");
    define_builtin_macro(macros, "__STDC_VERSION__", "201112L");
    define_builtin_macro(macros, "__STDC_NO_COMPLEX__", "1");
    define_builtin_macro(macros, "__STDC_NO_THREADS__", "1");
    define_builtin_macro(macros, "__STDC_NO_VLA__", "1");
    define_builtin_macro(macros, "__GNUC_STDC_INLINE__", "1");
    define_builtin_macro(macros, "__REGISTER_PREFIX__", "");
    define_builtin_macro(macros, "__CHAR_BIT__", "8");
    define_builtin_macro(macros, "__LP64__", "1");
    define_builtin_macro(macros, "_LP64", "1");
    define_builtin_macro(macros, "__BOOL_WIDTH__", "8");
    define_builtin_macro(macros, "__CHAR_WIDTH__", "8");
    define_builtin_macro(macros, "__SCHAR_WIDTH__", "8");
    define_builtin_macro(macros, "__SHRT_WIDTH__", "16");
    define_builtin_macro(macros, "__INT_WIDTH__", "32");
    define_builtin_macro(macros, "__LONG_WIDTH__", "64");
    define_builtin_macro(macros, "__LONG_LONG_WIDTH__", "64");
    define_builtin_macro(macros, "__SIZEOF_POINTER__", "8");
    define_builtin_macro(macros, "__SIZEOF_LONG__", "8");
    define_builtin_macro(macros, "__SIZEOF_LONG_LONG__", "8");
    define_builtin_macro(macros, "__SIZEOF_INT__", "4");
    define_builtin_macro(macros, "__SIZEOF_SHORT__", "2");
    define_builtin_macro(macros, "__SIZEOF_BOOL__", "1");
    define_builtin_macro(macros, "__SIZEOF_FLOAT__", "4");
    define_builtin_macro(macros, "__SIZEOF_DOUBLE__", "8");
    define_builtin_macro(macros, "__SIZEOF_LONG_DOUBLE__", "16");
    define_builtin_macro(macros, "__SIZEOF_SIZE_T__", "8");
    define_builtin_macro(macros, "__SIZEOF_PTRDIFF_T__", "8");
    define_builtin_macro(macros, "__SIZEOF_WCHAR_T__", "4");
    define_builtin_macro(macros, "__SIZEOF_WINT_T__", "4");
    define_builtin_macro(macros, "__SIZE_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__PTRDIFF_TYPE__", "long");
    define_builtin_macro(macros, "__INTPTR_TYPE__", "long");
    define_builtin_macro(macros, "__UINTPTR_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__INTMAX_TYPE__", "long");
    define_builtin_macro(macros, "__UINTMAX_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__INT8_TYPE__", "signed char");
    define_builtin_macro(macros, "__UINT8_TYPE__", "unsigned char");
    define_builtin_macro(macros, "__INT16_TYPE__", "short");
    define_builtin_macro(macros, "__UINT16_TYPE__", "unsigned short");
    define_builtin_macro(macros, "__INT32_TYPE__", "int");
    define_builtin_macro(macros, "__UINT32_TYPE__", "unsigned int");
    define_builtin_macro(macros, "__INT64_TYPE__", "long");
    define_builtin_macro(macros, "__UINT64_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__INT_LEAST8_TYPE__", "signed char");
    define_builtin_macro(macros, "__UINT_LEAST8_TYPE__", "unsigned char");
    define_builtin_macro(macros, "__INT_LEAST16_TYPE__", "short");
    define_builtin_macro(macros, "__UINT_LEAST16_TYPE__", "unsigned short");
    define_builtin_macro(macros, "__INT_LEAST32_TYPE__", "int");
    define_builtin_macro(macros, "__UINT_LEAST32_TYPE__", "unsigned int");
    define_builtin_macro(macros, "__INT_LEAST64_TYPE__", "long");
    define_builtin_macro(macros, "__UINT_LEAST64_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__INT_FAST8_TYPE__", "signed char");
    define_builtin_macro(macros, "__UINT_FAST8_TYPE__", "unsigned char");
    define_builtin_macro(macros, "__INT_FAST16_TYPE__", "long");
    define_builtin_macro(macros, "__UINT_FAST16_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__INT_FAST32_TYPE__", "long");
    define_builtin_macro(macros, "__UINT_FAST32_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__INT_FAST64_TYPE__", "long");
    define_builtin_macro(macros, "__UINT_FAST64_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__SCHAR_MAX__", "127");
    define_builtin_macro(macros, "__SHRT_MAX__", "32767");
    define_builtin_macro(macros, "__INT_MAX__", "2147483647");
    define_builtin_macro(macros, "__LONG_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__LONG_LONG_MAX__", "9223372036854775807LL");
    define_builtin_macro(macros, "__INT8_MAX__", "127");
    define_builtin_macro(macros, "__UINT8_MAX__", "255");
    define_builtin_macro(macros, "__INT16_MAX__", "32767");
    define_builtin_macro(macros, "__UINT16_MAX__", "65535");
    define_builtin_macro(macros, "__INT32_MAX__", "2147483647");
    define_builtin_macro(macros, "__UINT32_MAX__", "4294967295U");
    define_builtin_macro(macros, "__INT_LEAST8_MAX__", "127");
    define_builtin_macro(macros, "__UINT_LEAST8_MAX__", "255");
    define_builtin_macro(macros, "__INT_LEAST16_MAX__", "32767");
    define_builtin_macro(macros, "__UINT_LEAST16_MAX__", "65535");
    define_builtin_macro(macros, "__INT_LEAST32_MAX__", "2147483647");
    define_builtin_macro(macros, "__UINT_LEAST32_MAX__", "4294967295U");
    define_builtin_macro(macros, "__INT_LEAST64_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__UINT_LEAST64_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__INT_FAST8_MAX__", "127");
    define_builtin_macro(macros, "__UINT_FAST8_MAX__", "255");
    define_builtin_macro(macros, "__INT_FAST16_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__UINT_FAST16_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__INT_FAST32_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__UINT_FAST32_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__INT_FAST64_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__UINT_FAST64_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__SIZE_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__PTRDIFF_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__INTPTR_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__UINTPTR_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__INTMAX_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__UINTMAX_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__UINT_MAX__", "4294967295U");
    define_builtin_macro(macros, "__UINT64_MAX__", "18446744073709551615UL");
    define_builtin_macro(macros, "__INT64_MAX__", "9223372036854775807L");
    define_builtin_macro(macros, "__WCHAR_MAX__", "2147483647");
    define_builtin_macro(macros, "__WINT_MAX__", "4294967295U");
    define_builtin_macro(macros, "__ORDER_LITTLE_ENDIAN__", "1234");
    define_builtin_macro(macros, "__ORDER_BIG_ENDIAN__", "4321");
    define_builtin_macro(macros, "__BYTE_ORDER__", "__ORDER_LITTLE_ENDIAN__");
    define_builtin_macro(macros, "__LITTLE_ENDIAN__", "1");
    define_builtin_macro(macros, "__GNUC__", "4");
    define_builtin_macro(macros, "__GNUC_MINOR__", "2");
    define_builtin_macro(macros, "__GNUC_PATCHLEVEL__", "1");
    define_builtin_macro(macros, "__VERSION__", "\"rnqcc\"");

    match target.arch {
        Arch::X86_64 => {
            define_builtin_macro(macros, "__x86_64__", "1");
            define_builtin_macro(macros, "__amd64__", "1");
        }
        Arch::AArch64 => {
            define_builtin_macro(macros, "__aarch64__", "1");
            define_builtin_macro(macros, "__arm64__", "1");
        }
    }

    match target.os {
        TargetOs::Linux => {
            define_builtin_macro(macros, "__linux__", "1");
            define_builtin_macro(macros, "__linux", "1");
            define_builtin_macro(macros, "linux", "1");
            define_builtin_macro(macros, "__ELF__", "1");
        }
        TargetOs::MacOs => {
            define_builtin_macro(macros, "__APPLE__", "1");
            define_builtin_macro(macros, "__MACH__", "1");
            define_builtin_macro(macros, "__APPLE_CC__", "6000");
            define_builtin_macro(macros, "__APPLE_CPP__", "1");
            for name in [
                "__OSX_AVAILABLE",
                "__IOS_AVAILABLE",
                "__TVOS_AVAILABLE",
                "__WATCHOS_AVAILABLE",
                "__API_AVAILABLE",
                "__API_DEPRECATED",
                "__API_DEPRECATED_WITH_REPLACEMENT",
                "__API_OBSOLETED",
                "__API_OBSOLETED_WITH_REPLACEMENT",
                "__API_UNAVAILABLE",
                "API_AVAILABLE",
                "API_DEPRECATED",
                "API_DEPRECATED_WITH_REPLACEMENT",
                "API_OBSOLETED",
                "API_OBSOLETED_WITH_REPLACEMENT",
                "API_UNAVAILABLE",
            ] {
                define_empty_function_macro(macros, name, true);
            }
            for name in [
                "__API_AVAILABLE_BEGIN",
                "__API_AVAILABLE_END",
                "__API_DEPRECATED_BEGIN",
                "__API_DEPRECATED_END",
                "__API_DEPRECATED_WITH_REPLACEMENT_BEGIN",
                "__API_DEPRECATED_WITH_REPLACEMENT_END",
                "__API_OBSOLETED_BEGIN",
                "__API_OBSOLETED_END",
                "__API_OBSOLETED_WITH_REPLACEMENT_BEGIN",
                "__API_OBSOLETED_WITH_REPLACEMENT_END",
                "__API_UNAVAILABLE_BEGIN",
                "__API_UNAVAILABLE_END",
                "API_AVAILABLE_BEGIN",
                "API_AVAILABLE_END",
                "API_DEPRECATED_BEGIN",
                "API_DEPRECATED_END",
                "API_DEPRECATED_WITH_REPLACEMENT_BEGIN",
                "API_DEPRECATED_WITH_REPLACEMENT_END",
                "API_OBSOLETED_BEGIN",
                "API_OBSOLETED_END",
                "API_OBSOLETED_WITH_REPLACEMENT_BEGIN",
                "API_OBSOLETED_WITH_REPLACEMENT_END",
                "API_UNAVAILABLE_BEGIN",
                "API_UNAVAILABLE_END",
            ] {
                define_builtin_macro(macros, name, "");
            }
        }
    }
}

fn push_existing_include_dir(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if path.is_dir() && !dirs.iter().any(|existing| existing == &path) {
        dirs.push(path);
    }
}

fn default_system_include_dirs(target: &Target) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in ["CPATH", "C_INCLUDE_PATH"] {
        if let Some(paths) = std::env::var_os(var) {
            for path in std::env::split_paths(&paths) {
                push_existing_include_dir(&mut dirs, path);
            }
        }
    }

    match target.os {
        TargetOs::Linux => {
            push_existing_include_dir(&mut dirs, PathBuf::from("/usr/local/include"));
            match target.arch {
                Arch::X86_64 => push_existing_include_dir(
                    &mut dirs,
                    PathBuf::from("/usr/include/x86_64-linux-gnu"),
                ),
                Arch::AArch64 => push_existing_include_dir(
                    &mut dirs,
                    PathBuf::from("/usr/include/aarch64-linux-gnu"),
                ),
            }
            push_existing_include_dir(&mut dirs, PathBuf::from("/usr/include"));
        }
        TargetOs::MacOs => {
            push_existing_include_dir(&mut dirs, PathBuf::from("/opt/homebrew/include"));
            push_existing_include_dir(&mut dirs, PathBuf::from("/usr/local/include"));
            push_existing_include_dir(
                &mut dirs,
                PathBuf::from("/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk/usr/include"),
            );
        }
    }

    dirs
}

fn internal_preprocess_source(
    src: &Path,
    macros: &mut HashMap<String, MacroDef>,
    context: &mut InternalPreprocessContext<'_>,
    state: &mut PreprocessorState,
) -> Result<String, String> {
    let canonical = canonical_path(src);
    if context.once_files.contains(&canonical) {
        return Ok(String::new());
    }
    let source = std::fs::read_to_string(src)
        .map_err(|err| format!("could not read {}: {}", src.display(), err))?;
    let source = strip_comments(&splice_continued_lines(
        &preprocess::lexer::replace_trigraphs(&source),
    ))?;
    if context.include_stack.contains(&canonical) {
        if inactive_recursive_include_guard(&source, macros) {
            return Ok(String::new());
        }
        return Err(format!("recursive include of {}", src.display()));
    }
    context.include_stack.push(canonical.clone());

    let mut out = String::new();
    let base_dir = src.parent().unwrap_or_else(|| Path::new("."));
    let mut conditionals: Vec<ConditionalFrame> = Vec::new();

    let display_file = src.to_string_lossy().into_owned();
    let mut logical_file = display_file.clone();
    let mut next_logical_line = 1usize;
    let include_level = context.include_stack.len().saturating_sub(1);
    let mut pending_source: Option<PendingSource> = None;
    if context.line_markers && !context.suppress_preprocessed_output {
        push_line_marker(&mut out, 1, &logical_file);
    }
    for line in source.lines() {
        let current_line_number = next_logical_line;
        next_logical_line = next_logical_line.saturating_add(1);
        let trimmed = line.trim_start();
        if starts_preprocessor_directive(trimmed) {
            flush_pending_source(
                &mut pending_source,
                &mut out,
                macros,
                context,
                &canonical,
                include_level,
                state,
            )?;
            if !conditionals_active(&conditionals)
                && !raw_directive_name(trimmed).is_some_and(is_conditional_control_directive)
            {
                continue;
            }
            let tokens = preprocess::lexer::lex(line)?;
            if let Some(directive) = preprocess::directive::parse_directive_tokens(&tokens)? {
                use preprocess::directive::Directive;

                if !matches!(directive, Directive::Pragma { .. }) {
                    check_poisoned_tokens(&tokens, context)?;
                }

                match directive {
                    Directive::Ifdef { name, negated } => {
                        let parent_active = conditionals_active(&conditionals);
                        let defined = macros.contains_key(name.trim());
                        let condition_active = if negated { !defined } else { defined };
                        conditionals.push(ConditionalFrame {
                            parent_active,
                            condition_active: parent_active && condition_active,
                            branch_taken: parent_active && condition_active,
                            saw_else: false,
                        });
                        continue;
                    }
                    Directive::If { expr } => {
                        let parent_active = conditionals_active(&conditionals);
                        let condition_active = if parent_active {
                            eval_internal_if(
                                &preprocess::emit::emit_tokens(&expr),
                                macros,
                                state,
                                IfEvalContext {
                                    file: &logical_file,
                                    line_number: current_line_number,
                                    base_dir,
                                    include_paths: context.include_paths,
                                    include_level,
                                },
                            )?
                        } else {
                            false
                        };
                        conditionals.push(ConditionalFrame {
                            parent_active,
                            condition_active,
                            branch_taken: condition_active,
                            saw_else: false,
                        });
                        continue;
                    }
                    Directive::Elifdef { name, negated } => {
                        let Some(frame) = conditionals.last_mut() else {
                            return Err(if negated {
                                "#elifndef without #if".to_string()
                            } else {
                                "#elifdef without #if".to_string()
                            });
                        };
                        if frame.saw_else {
                            return Err(if negated {
                                "#elifndef after #else".to_string()
                            } else {
                                "#elifdef after #else".to_string()
                            });
                        }
                        if !frame.parent_active || frame.branch_taken {
                            frame.condition_active = false;
                        } else {
                            let defined = macros.contains_key(name.trim());
                            frame.condition_active = if negated { !defined } else { defined };
                            frame.branch_taken = frame.condition_active;
                        }
                        continue;
                    }
                    Directive::Elif { expr } => {
                        let Some(frame) = conditionals.last_mut() else {
                            return Err("#elif without #if".to_string());
                        };
                        if frame.saw_else {
                            return Err("#elif after #else".to_string());
                        }
                        if !frame.parent_active || frame.branch_taken {
                            frame.condition_active = false;
                        } else {
                            frame.condition_active = eval_internal_if(
                                &preprocess::emit::emit_tokens(&expr),
                                macros,
                                state,
                                IfEvalContext {
                                    file: &logical_file,
                                    line_number: current_line_number,
                                    base_dir,
                                    include_paths: context.include_paths,
                                    include_level,
                                },
                            )?;
                            frame.branch_taken = frame.condition_active;
                        }
                        continue;
                    }
                    Directive::Else => {
                        let Some(frame) = conditionals.last_mut() else {
                            return Err("#else without #if".to_string());
                        };
                        if frame.saw_else {
                            return Err("duplicate #else".to_string());
                        }
                        frame.condition_active = frame.parent_active && !frame.branch_taken;
                        frame.branch_taken = true;
                        frame.saw_else = true;
                        continue;
                    }
                    Directive::Endif => {
                        if conditionals.pop().is_none() {
                            return Err("#endif without #if".to_string());
                        }
                        continue;
                    }
                    Directive::Empty => {
                        if let Some((line_number, filename)) = parse_line_marker_tokens(&tokens)? {
                            next_logical_line = line_number;
                            if let Some(filename) = filename {
                                logical_file = filename;
                            }
                        }
                        continue;
                    }
                    other if !conditionals_active(&conditionals) => {
                        let _ = other;
                        continue;
                    }
                    Directive::Include {
                        operand,
                        include_next,
                    } => {
                        let Some(spec) = parse_token_include_operand(
                            &operand,
                            macros,
                            &logical_file,
                            current_line_number,
                            include_level,
                            state,
                        ) else {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                format!("unsupported include directive: {}", line.trim()),
                            ));
                        };
                        let Some(include_path) = resolve_include_path(
                            &spec,
                            base_dir,
                            context.include_paths,
                            include_next,
                        ) else {
                            if !include_next {
                                if let Some(name) = virtual_compat_header_name(&spec) {
                                    let included = include_virtual_compat_header(name, macros);
                                    out.push_str(&included);
                                    if !included.is_empty() && !included.ends_with('\n') {
                                        out.push('\n');
                                    }
                                    if context.line_markers && !context.suppress_preprocessed_output
                                    {
                                        push_line_marker(
                                            &mut out,
                                            next_logical_line,
                                            &logical_file,
                                        );
                                    }
                                    continue;
                                }
                            }
                            if context.missing_headers_generated {
                                context
                                    .dependencies
                                    .push(generated_header_dependency(&spec));
                                continue;
                            }
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                include_not_found(&spec),
                            ));
                        };
                        trace_include(&include_path, context);
                        record_dependency(&include_path, context);
                        let included =
                            internal_preprocess_source(&include_path, macros, context, state)?;
                        unrecord_dependency(&include_path, context);
                        out.push_str(&included);
                        if !included.ends_with('\n') {
                            out.push('\n');
                        }
                        if context.line_markers && !context.suppress_preprocessed_output {
                            push_line_marker(&mut out, next_logical_line, &logical_file);
                        }
                        continue;
                    }
                    Directive::Define { name, def } => {
                        if context.poisoned_identifiers.contains(&name) {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                format!("attempt to use poisoned identifier {}", name),
                            ));
                        }
                        let new_def = token_macro_def_to_string(def);
                        if let Some(existing) = macros.get(&name) {
                            if !macro_defs_equivalent(existing, &new_def)? {
                                return Err(format!("macro {} redefined", name));
                            }
                        } else {
                            macros.insert(name, new_def);
                        }
                        continue;
                    }
                    Directive::Undef { name } => {
                        if context.poisoned_identifiers.contains(name.trim()) {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                format!("attempt to use poisoned identifier {}", name.trim()),
                            ));
                        }
                        macros.remove(name.trim());
                        continue;
                    }
                    Directive::Error { tokens } => {
                        return Err(pp_location(
                            &logical_file,
                            current_line_number,
                            format!("#error {}", preprocess::emit::emit_tokens(&tokens).trim()),
                        ));
                    }
                    Directive::Warning { tokens } => {
                        if !is_marked_system_header(&canonical, context) {
                            eprintln!(
                                "rnqcc: {}:{}: warning: {}",
                                logical_file,
                                current_line_number,
                                preprocess::emit::emit_tokens(&tokens).trim()
                            );
                        }
                        continue;
                    }
                    Directive::Line { tokens } => {
                        let (line_number, filename) = parse_token_line_operand(
                            &tokens,
                            macros,
                            &logical_file,
                            current_line_number,
                            include_level,
                            state,
                        )?;
                        next_logical_line = line_number;
                        if let Some(filename) = filename {
                            logical_file = filename;
                        }
                        continue;
                    }
                    Directive::Pragma { tokens } => {
                        let expanded = expand_preprocessor_tokens(
                            &tokens,
                            macros,
                            &logical_file,
                            current_line_number,
                            include_level,
                            state,
                        )?;
                        let pragma = preprocess::emit::emit_tokens(&expanded);
                        let pragma = pragma.trim();
                        handle_internal_pragma(pragma, &canonical, context);
                        continue;
                    }
                    Directive::Ident => continue,
                    Directive::Unknown { name, .. } => {
                        return Err(pp_location(
                            &logical_file,
                            current_line_number,
                            format!("unsupported preprocessor directive: #{}", name),
                        ));
                    }
                }
            }
        }
        if !conditionals_active(&conditionals) {
            continue;
        }
        if pending_source.is_none() {
            pending_source = Some(PendingSource {
                text: String::new(),
                logical_file: logical_file.clone(),
                start_line: current_line_number,
            });
        }
        if let Some(pending) = pending_source.as_mut() {
            pending.text.push_str(line);
            pending.text.push('\n');
        }
    }
    flush_pending_source(
        &mut pending_source,
        &mut out,
        macros,
        context,
        &canonical,
        include_level,
        state,
    )?;

    if !conditionals.is_empty() {
        return Err("unterminated conditional directive".to_string());
    }

    context.include_stack.pop();
    Ok(out)
}

fn parse_define_arg(arg: &str) -> Result<(String, MacroDef), String> {
    let line = match arg.split_once('=') {
        Some((name, value)) => format!("#define {} {}", name, value),
        None => format!("#define {} 1", arg),
    };
    let tokens = preprocess::lexer::lex(&line)
        .map_err(|_| format!("malformed macro definition: -D{}", arg))?;
    match preprocess::directive::parse_directive_tokens(&tokens)
        .map_err(|_| format!("malformed macro definition: -D{}", arg))?
    {
        Some(preprocess::directive::Directive::Define { name, def }) => {
            Ok((name, token_macro_def_to_string(def)))
        }
        _ => Err(format!("malformed macro definition: -D{}", arg)),
    }
}

fn apply_cli_macros(
    macros: &mut HashMap<String, MacroDef>,
    defines: &[String],
    undefs: &[String],
) -> Result<(), String> {
    for define in defines {
        let (name, definition) = parse_define_arg(define)?;
        macros.insert(name, definition);
    }
    for name in undefs {
        if !name.chars().next().is_some_and(is_ident_start) || !name.chars().all(is_ident_continue)
        {
            return Err(format!("malformed macro undefinition: -U{}", name));
        }
        macros.remove(name);
    }
    Ok(())
}

struct InternalPreprocessInvocation<'a> {
    src: &'a str,
    output: &'a str,
    include_paths: &'a IncludePaths,
    macro_includes: &'a [PathBuf],
    forced_includes: &'a [PathBuf],
    defines: &'a [String],
    undefs: &'a [String],
    target: &'a Target,
    user_dependencies_only: bool,
    missing_headers_generated: bool,
    dump_macros: bool,
    suppress_preprocessed_output: bool,
    trace_includes: bool,
    line_markers: bool,
}

fn internal_preprocess(
    invocation: InternalPreprocessInvocation<'_>,
) -> Result<Vec<PathBuf>, String> {
    let mut macros = HashMap::new();
    seed_internal_predefined_macros(&mut macros, invocation.target);
    apply_cli_macros(&mut macros, invocation.defines, invocation.undefs)?;
    let mut include_stack = Vec::new();
    let mut once_files = HashSet::new();
    let mut system_header_files = HashSet::new();
    let mut poisoned_identifiers = HashSet::new();
    let mut pragma_pack_stack = Vec::new();
    let mut pragma_pack_alignment = None;
    let mut state = PreprocessorState::new(invocation.src.to_string());
    let mut effective_include_paths = invocation.include_paths.clone();
    effective_include_paths.append_system_defaults(invocation.target);
    let mut dependencies = Vec::new();
    let mut context = InternalPreprocessContext {
        include_stack: &mut include_stack,
        once_files: &mut once_files,
        system_header_files: &mut system_header_files,
        poisoned_identifiers: &mut poisoned_identifiers,
        pragma_pack_stack: &mut pragma_pack_stack,
        pragma_pack_alignment: &mut pragma_pack_alignment,
        include_paths: &effective_include_paths,
        dependencies: &mut dependencies,
        user_dependencies_only: invocation.user_dependencies_only,
        missing_headers_generated: invocation.missing_headers_generated,
        suppress_preprocessed_output: invocation.suppress_preprocessed_output,
        trace_includes: invocation.trace_includes,
        line_markers: invocation.line_markers,
    };
    let mut preprocessed = String::new();
    for macro_include in invocation.macro_includes {
        let spec = IncludeSpec::Quoted(macro_include.to_string_lossy().into_owned());
        let Some(include_path) =
            resolve_include_path(&spec, Path::new("."), &effective_include_paths, false)
        else {
            if context.missing_headers_generated {
                context
                    .dependencies
                    .push(generated_header_dependency(&spec));
                continue;
            }
            return Err(include_not_found(&spec));
        };
        trace_include(&include_path, &context);
        record_dependency(&include_path, &mut context);
        let _ = internal_preprocess_source(&include_path, &mut macros, &mut context, &mut state)?;
        unrecord_dependency(&include_path, &mut context);
    }
    for forced in invocation.forced_includes {
        let spec = IncludeSpec::Quoted(forced.to_string_lossy().into_owned());
        let Some(include_path) =
            resolve_include_path(&spec, Path::new("."), &effective_include_paths, false)
        else {
            if context.missing_headers_generated {
                context
                    .dependencies
                    .push(generated_header_dependency(&spec));
                continue;
            }
            return Err(include_not_found(&spec));
        };
        trace_include(&include_path, &context);
        record_dependency(&include_path, &mut context);
        let included =
            internal_preprocess_source(&include_path, &mut macros, &mut context, &mut state)?;
        unrecord_dependency(&include_path, &mut context);
        preprocessed.push_str(&included);
        if !included.ends_with('\n') {
            preprocessed.push('\n');
        }
    }
    preprocessed.push_str(&internal_preprocess_source(
        Path::new(invocation.src),
        &mut macros,
        &mut context,
        &mut state,
    )?);
    let output = match (
        invocation.dump_macros,
        invocation.suppress_preprocessed_output,
    ) {
        (true, true) => format_macro_dump(&macros),
        (true, false) => {
            let mut output = format_macro_dump(&macros);
            output.push_str(&preprocessed);
            output
        }
        (false, _) => preprocessed,
    };
    std::fs::write(invocation.output, output)
        .map_err(|err| format!("could not write {}: {}", invocation.output, err))?;
    Ok(dependencies)
}

struct PreprocessedSource {
    path: String,
    generated: bool,
    dependencies: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct DependencyOptions {
    emit: bool,
    side_effect: bool,
    user_only: bool,
    phony_targets: bool,
    missing_headers_generated: bool,
    file: Option<String>,
    targets: Vec<String>,
}

struct PreprocessInvocation<'a> {
    src: &'a str,
    index: usize,
    language: Option<&'a str>,
    keep_temps: bool,
    cc: &'a str,
    internal_cpp: bool,
    include_paths: &'a IncludePaths,
    macro_includes: &'a [PathBuf],
    forced_includes: &'a [PathBuf],
    defines: &'a [String],
    undefs: &'a [String],
    target: &'a Target,
    dependency_options: &'a DependencyOptions,
    dump_macros: bool,
    suppress_preprocessed_output: bool,
    trace_includes: bool,
    line_markers: bool,
    sysroot: Option<&'a str>,
    extra_preprocessor_args: &'a [OsString],
}

fn preprocess(invocation: PreprocessInvocation<'_>) -> Result<PreprocessedSource, String> {
    validate_input(invocation.src, invocation.language)?;
    let mut stdin_temp_guard = None;
    let actual_src = if invocation.src == "-" {
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|err| format!("could not read stdin: {}", err))?;
        let path = temp_path_for("stdin", invocation.index, "c");
        std::fs::write(&path, source)
            .map_err(|err| format!("could not write {}: {}", path, err))?;
        stdin_temp_guard = Some(tempfile::TempFile::new(path.clone()));
        path
    } else {
        invocation.src.to_string()
    };
    if extension(&actual_src) == "i" {
        return Ok(PreprocessedSource {
            path: actual_src,
            generated: false,
            dependencies: Vec::new(),
        });
    }
    let output = if invocation.keep_temps {
        replace_extension(&actual_src, "i")
    } else {
        temp_path_for(&actual_src, invocation.index, "i")
    };
    let preprocessing_result = if invocation.internal_cpp {
        internal_preprocess(InternalPreprocessInvocation {
            src: &actual_src,
            output: &output,
            include_paths: invocation.include_paths,
            macro_includes: invocation.macro_includes,
            forced_includes: invocation.forced_includes,
            defines: invocation.defines,
            undefs: invocation.undefs,
            target: invocation.target,
            user_dependencies_only: invocation.dependency_options.user_only,
            missing_headers_generated: invocation.dependency_options.missing_headers_generated,
            dump_macros: invocation.dump_macros,
            suppress_preprocessed_output: invocation.suppress_preprocessed_output,
            trace_includes: invocation.trace_includes,
            line_markers: invocation.line_markers,
        })?
    } else {
        let mut args: Vec<OsString> = invocation
            .include_paths
            .quote
            .iter()
            .flat_map(|dir| [OsString::from("-iquote"), dir.as_os_str().to_os_string()])
            .collect();
        args.extend(
            invocation
                .include_paths
                .user
                .iter()
                .flat_map(|dir| [OsString::from("-I"), dir.as_os_str().to_os_string()]),
        );
        args.extend(
            invocation
                .include_paths
                .system
                .iter()
                .flat_map(|dir| [OsString::from("-isystem"), dir.as_os_str().to_os_string()]),
        );
        args.extend(
            invocation
                .include_paths
                .after
                .iter()
                .flat_map(|dir| [OsString::from("-idirafter"), dir.as_os_str().to_os_string()]),
        );
        args.extend(
            invocation
                .macro_includes
                .iter()
                .flat_map(|path| [OsString::from("-imacros"), path.as_os_str().to_os_string()]),
        );
        args.extend(
            invocation
                .forced_includes
                .iter()
                .flat_map(|path| [OsString::from("-include"), path.as_os_str().to_os_string()]),
        );
        if !invocation.include_paths.use_standard_system {
            args.push(OsString::from("-nostdinc"));
        }
        if let Some(sysroot) = invocation.sysroot {
            args.extend([OsString::from("-isysroot"), OsString::from(sysroot)]);
        }
        args.extend(invocation.extra_preprocessor_args.iter().cloned());
        args.extend(
            invocation
                .defines
                .iter()
                .flat_map(|define| [OsString::from("-D"), OsString::from(define)]),
        );
        args.extend(
            invocation
                .undefs
                .iter()
                .flat_map(|undef| [OsString::from("-U"), OsString::from(undef)]),
        );
        args.push(OsString::from("-E"));
        if !invocation.line_markers {
            args.push(OsString::from("-P"));
        }
        if invocation.dump_macros {
            if invocation.suppress_preprocessed_output {
                args.push(OsString::from("-dM"));
            } else {
                args.push(OsString::from("-dD"));
            }
        }
        if invocation.trace_includes {
            args.push(OsString::from("-H"));
        }
        if invocation.language == Some("c") {
            args.extend([OsString::from("-x"), OsString::from("c")]);
        }
        args.extend([
            OsString::from(actual_src.as_str()),
            OsString::from("-o"),
            OsString::from(output.as_str()),
        ]);
        run_command(invocation.cc, args)?;
        Vec::new()
    };
    drop(stdin_temp_guard);
    let dependencies = preprocessing_result;
    Ok(PreprocessedSource {
        path: output,
        generated: true,
        dependencies,
    })
}

struct CompileInvocation<'a> {
    stage: &'a Stage,
    preprocessed_src: &'a str,
    target: &'a Target,
    opt_flags: &'a optimize::OptimizationFlags,
    no_coalescing: bool,
    keep_temps: bool,
    cleanup_preprocessed: bool,
    dumps: compile::DumpOptions,
    warnings: compile::WarningOptions,
}

fn do_compile(invocation: CompileInvocation<'_>) -> Result<String, String> {
    let compile_outcome = compile::compile(
        invocation.stage,
        invocation.preprocessed_src,
        invocation.target,
        invocation.opt_flags,
        invocation.no_coalescing,
        invocation.dumps,
        invocation.warnings,
    );
    if invocation.cleanup_preprocessed && !invocation.keep_temps {
        let _ = std::fs::remove_file(invocation.preprocessed_src);
    }
    compile_outcome?;
    Ok(replace_extension(invocation.preprocessed_src, "s"))
}

fn gcc_arch_args(target: &Target) -> Vec<&'static str> {
    target.cc_arch_args()
}

fn can_assemble_on_host(target: &Target) -> bool {
    target.can_use_host_driver()
}

fn stage_accepts_output(stage: &Stage) -> bool {
    stage.accepts_output()
}

fn output_requires_single_input(stage: &Stage) -> bool {
    stage.output_requires_single_input()
}

struct LinkInvocation<'a> {
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

fn assemble_and_link(invocation: LinkInvocation<'_>) -> Result<(), String> {
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

struct DriverOptions<'a> {
    target: Target,
    debug: bool,
    stage: Stage,
    sources: &'a [&'a str],
    language: Option<&'a str>,
    output: Option<&'a str>,
    opt_flags: &'a optimize::OptimizationFlags,
    cc: &'a str,
    no_coalescing: bool,
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

struct DriverArtifact {
    src: String,
    path: String,
    generated: bool,
}

fn quote_make_word(value: &str) -> String {
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

fn dependency_target(src: &str, options: &DependencyOptions) -> String {
    if !options.targets.is_empty() {
        return options.targets.join(" ");
    }
    quote_make_word(&replace_extension(src, "o"))
}

fn dependency_rule(src: &str, dependencies: &[PathBuf], options: &DependencyOptions) -> String {
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

fn emit_dependency_rule(
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

fn write_dependency_side_effect(
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

fn driver(options: DriverOptions<'_>) -> Result<(), String> {
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

fn main() {
    if let Err(err) = real_main() {
        eprintln!("rnqcc: {}", err);
        std::process::exit(1);
    }
}

fn real_main() -> Result<(), String> {
    let args = normalize_driver_args(expand_response_args(std::env::args_os())?);
    let dependency_targets = dependency_targets_from_args(&args);
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
    let opt_flags = optimize::OptimizationFlags {
        fold_constants: all_opts || matches.is_present("fold_constants"),
        eliminate_unreachable_code: all_opts || matches.is_present("eliminate_unreachable_code"),
        propagate_copies: all_opts || matches.is_present("propagate_copies"),
        eliminate_dead_stores: all_opts || matches.is_present("eliminate_dead_stores"),
    };

    let no_coalescing = matches.is_present("no_coalescing");
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
        error: matches.is_present("werror"),
    };

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
