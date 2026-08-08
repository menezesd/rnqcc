use crate::backend;
use crate::diagnostic::Diagnostic;
use crate::lex;
use crate::optimize;
use crate::parse;
use crate::resolve;
use crate::tacky;
use crate::types::*;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, Default)]
pub struct DumpOptions {
    pub ast: bool,
    pub tacky_pre_opt: bool,
    pub tacky: bool,
    pub asm_ir: bool,
    pub source_comments: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct WarningOptions {
    pub enabled: bool,
    pub unreachable: bool,
    pub missing_return: bool,
    pub compare_distinct_pointer_types: bool,
    pub deprecated_declarations: bool,
    pub error: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CompatibilityOptions {
    pub permissive: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CompileOptions<'a> {
    pub target: &'a Target,
    pub opt_flags: &'a optimize::OptimizationFlags,
    pub no_coalescing: bool,
    pub instrument_functions: bool,
    pub compatibility: CompatibilityOptions,
    pub dumps: DumpOptions,
    pub warnings: WarningOptions,
}

impl Default for WarningOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            unreachable: true,
            missing_return: true,
            compare_distinct_pointer_types: true,
            deprecated_declarations: true,
            error: false,
        }
    }
}

impl WarningOptions {
    fn allows(self, warning: &crate::diagnostic::Warning) -> bool {
        if !self.enabled {
            return false;
        }
        match warning.kind {
            crate::diagnostic::WarningKind::UnreachableStatement { .. } => self.unreachable,
            crate::diagnostic::WarningKind::MissingReturn { .. } => self.missing_return,
            crate::diagnostic::WarningKind::NegativeShiftCount => true,
            crate::diagnostic::WarningKind::CompareDistinctPointerTypes => {
                self.compare_distinct_pointer_types
            }
            crate::diagnostic::WarningKind::DeprecatedDeclaration { .. } => {
                self.deprecated_declarations
            }
        }
    }
}

fn val_is_assignable(val: &TackyVal) -> bool {
    matches!(val, TackyVal::Var(_))
}

pub fn validate_tacky_program(program: &TackyProgram) -> Result<(), String> {
    let mut all_labels = HashSet::new();
    for item in &program.top_level {
        let TackyTopLevel::Function(function) = item else {
            continue;
        };
        for instr in &function.body {
            if let TackyInstr::Label(label) = instr {
                if !all_labels.insert(label.as_str()) {
                    return Err(format!(
                        "function '{}' has duplicate TACKY label '{}'",
                        function.name, label
                    ));
                }
            }
        }
    }

    for item in &program.top_level {
        let TackyTopLevel::Function(function) = item else {
            continue;
        };
        let mut labels = HashSet::new();
        for instr in &function.body {
            if let TackyInstr::Label(label) = instr {
                if !labels.insert(label.as_str()) {
                    return Err(format!(
                        "function '{}' has duplicate TACKY label '{}'",
                        function.name, label
                    ));
                }
            }
        }

        for instr in &function.body {
            match instr {
                TackyInstr::NonlocalJump(label) | TackyInstr::LoadLabelAddress(label, _)
                    if !all_labels.contains(label.as_str()) =>
                {
                    return Err(format!(
                        "function '{}' references undefined TACKY label '{}'",
                        function.name, label
                    ));
                }
                _ => {}
            }

            match instr {
                TackyInstr::Unary { dst, .. }
                | TackyInstr::Binary { dst, .. }
                | TackyInstr::Copy { dst, .. }
                | TackyInstr::SignExtend { dst, .. }
                | TackyInstr::ZeroExtend { dst, .. }
                | TackyInstr::Truncate { dst, .. }
                | TackyInstr::IntToDouble { dst, .. }
                | TackyInstr::IntToFloat { dst, .. }
                | TackyInstr::DoubleToInt { dst, .. }
                | TackyInstr::FloatToInt { dst, .. }
                | TackyInstr::UIntToDouble { dst, .. }
                | TackyInstr::UIntToFloat { dst, .. }
                | TackyInstr::DoubleToUInt { dst, .. }
                | TackyInstr::FloatToUInt { dst, .. }
                | TackyInstr::FloatToDouble { dst, .. }
                | TackyInstr::DoubleToFloat { dst, .. }
                | TackyInstr::FrameAddress { dst }
                | TackyInstr::LoadLabelAddress(_, dst)
                | TackyInstr::VaStart { dst }
                | TackyInstr::AtomicFetch { dst, .. }
                | TackyInstr::AtomicExchange { dst, .. }
                | TackyInstr::AtomicCompareExchange { dst, .. }
                | TackyInstr::AtomicCompareSwap { dst, .. }
                | TackyInstr::GetAddress { dst, .. }
                | TackyInstr::Load { dst, .. }
                | TackyInstr::CopyFromOffset { dst, .. }
                | TackyInstr::AddPtr { dst, .. } => {
                    if !val_is_assignable(dst) {
                        return Err(format!(
                            "function '{}' has non-assignable TACKY destination in {:?}",
                            function.name, instr
                        ));
                    }
                    if let TackyInstr::CopyFromOffset { offset, .. } = instr {
                        if *offset < 0 {
                            return Err(format!(
                                "function '{}' has negative aggregate offset in {:?}",
                                function.name, instr
                            ));
                        }
                    }
                    if let TackyInstr::AddPtr { scale, .. } = instr {
                        if *scale < 0 {
                            return Err(format!(
                                "function '{}' has negative pointer scale {}",
                                function.name, scale
                            ));
                        }
                    }
                }
                TackyInstr::Jump(label)
                | TackyInstr::JumpIfZero(_, label)
                | TackyInstr::JumpIfNotZero(_, label)
                    if !labels.contains(label.as_str()) =>
                {
                    return Err(format!(
                        "function '{}' jumps to undefined TACKY label '{}'",
                        function.name, label
                    ));
                }
                TackyInstr::FunCall {
                    args,
                    dst,
                    stack_arg_indices,
                    memory_arg_blocks,
                    struct_arg_groups,
                    fixed_flat_arg_count,
                    ..
                } => {
                    if !val_is_assignable(dst) {
                        return Err(format!(
                            "function '{}' has non-assignable call destination",
                            function.name
                        ));
                    }
                    if *fixed_flat_arg_count > args.len() {
                        return Err(format!(
                            "function '{}' call fixed arg count {} exceeds flattened arg count {}",
                            function.name,
                            fixed_flat_arg_count,
                            args.len()
                        ));
                    }
                    if let Some(index) =
                        stack_arg_indices.iter().find(|index| **index >= args.len())
                    {
                        return Err(format!(
                            "function '{}' call stack arg index {} exceeds flattened arg count {}",
                            function.name,
                            index,
                            args.len()
                        ));
                    }
                    for (start, count, classes) in struct_arg_groups {
                        let range_exceeds_args =
                            *start > args.len() || *count > args.len().saturating_sub(*start);
                        if range_exceeds_args || classes.len() != *count {
                            return Err(format!(
                                "function '{}' has invalid struct argument group {:?}",
                                function.name,
                                (start, count, classes)
                            ));
                        }
                    }
                    for (index, size, align) in memory_arg_blocks {
                        if *index >= args.len() || *align == 0 {
                            return Err(format!(
                                "function '{}' has invalid memory argument block {:?}",
                                function.name,
                                (index, size, align)
                            ));
                        }
                    }
                }
                TackyInstr::CopyToOffset { offset, .. } if *offset < 0 => {
                    return Err(format!(
                        "function '{}' has negative aggregate offset in {:?}",
                        function.name, instr
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn asm_operand_has_pseudo(operand: &AsmOperand) -> bool {
    matches!(operand, AsmOperand::Pseudo(_) | AsmOperand::PseudoMem(_, _))
}

fn asm_operands_have_pseudo(operands: &[&AsmOperand]) -> bool {
    operands.iter().copied().any(asm_operand_has_pseudo)
}

fn asm_instr_has_pseudo(instr: &AsmInstr) -> bool {
    match instr {
        AsmInstr::Mov(_, src, dst)
        | AsmInstr::Movsx(_, _, src, dst)
        | AsmInstr::MovZeroExtend(_, _, src, dst)
        | AsmInstr::Binary(_, _, src, dst)
        | AsmInstr::Cmp(_, src, dst)
        | AsmInstr::Lea(src, dst)
        | AsmInstr::And(_, src, dst)
        | AsmInstr::Or(_, src, dst)
        | AsmInstr::Xor(_, src, dst)
        | AsmInstr::Test(_, src, dst)
        | AsmInstr::Shl(_, src, dst)
        | AsmInstr::Shr(_, src, dst)
        | AsmInstr::Sar(_, src, dst)
        | AsmInstr::Ror(_, src, dst)
        | AsmInstr::Rol(_, src, dst)
        | AsmInstr::Cvtss2sd(src, dst)
        | AsmInstr::Cvtsd2ss(src, dst)
        | AsmInstr::AArch64FloatToDouble(src, dst)
        | AsmInstr::AArch64DoubleToFloat(src, dst) => asm_operands_have_pseudo(&[src, dst]),
        AsmInstr::Unary(_, _, operand)
        | AsmInstr::Idiv(_, operand)
        | AsmInstr::Div(_, operand)
        | AsmInstr::SetCC(_, operand)
        | AsmInstr::Push(operand)
        | AsmInstr::JmpIndirect(operand)
        | AsmInstr::LoadLabelAddress(_, operand)
        | AsmInstr::Fld(_, operand)
        | AsmInstr::Fstp(_, operand)
        | AsmInstr::Fisttp(_, operand)
        | AsmInstr::FldQ(operand)
        | AsmInstr::X87Push(_, operand)
        | AsmInstr::X87Pop(_, operand)
        | AsmInstr::X87Load(_, operand)
        | AsmInstr::X87Store(operand)
        | AsmInstr::X87StoreFloat(_, operand)
        | AsmInstr::X87StoreInt(_, operand) => asm_operand_has_pseudo(operand),
        AsmInstr::Cvtsi2sd(_, _, dst)
        | AsmInstr::Cvtsi2ss(_, _, dst)
        | AsmInstr::Cvttsd2si(_, _, dst)
        | AsmInstr::Cvttss2si(_, _, dst) => asm_operand_has_pseudo(dst),
        AsmInstr::MulFull(_, operand)
        | AsmInstr::LoadIndirect(_, _, operand)
        | AsmInstr::StoreIndirect(_, operand, _)
        | AsmInstr::AArch64LoadAdjusted(_, operand, _, _)
        | AsmInstr::AArch64StoreOutgoingArg(_, operand, _, _)
        | AsmInstr::AtomicRmw(_, _, _, operand)
        | AsmInstr::AtomicExchange(_, operand)
        | AsmInstr::AtomicCompareExchange(_, operand)
        | AsmInstr::AtomicCompareSwap(_, _, operand) => asm_operand_has_pseudo(operand),
        AsmInstr::X87LoadIndirect(_, _) | AsmInstr::X87StoreIndirect(_) => false,
        AsmInstr::AArch64Umulh(left, right, dst) => asm_operands_have_pseudo(&[left, right, dst]),
        AsmInstr::AArch64AddPtr(ptr, index, _, dst)
        | AsmInstr::AArch64Rem(_, _, ptr, index, dst) => {
            asm_operands_have_pseudo(&[ptr, index, dst])
        }
        AsmInstr::AArch64Extr(left, right, _, dst) => asm_operands_have_pseudo(&[left, right, dst]),
        AsmInstr::AArch64UIntToDouble(_, src, dst)
        | AsmInstr::AArch64UIntToFloat(_, src, dst)
        | AsmInstr::AArch64DoubleToUInt(_, src, dst)
        | AsmInstr::AArch64FloatToUInt(_, src, dst) => asm_operands_have_pseudo(&[src, dst]),
        AsmInstr::BuiltinSetjmp { buf, dst, .. } => asm_operands_have_pseudo(&[buf, dst]),
        AsmInstr::BuiltinLongjmp { buf, value } => asm_operands_have_pseudo(&[buf, value]),
        AsmInstr::CopyToStackArg { src_ptr, .. } => asm_operand_has_pseudo(src_ptr),
        AsmInstr::CopyFromStackArg { dst, .. } => asm_operand_has_pseudo(dst),
        AsmInstr::X87Binary(_)
        | AsmInstr::Fxch
        | AsmInstr::FstpQ
        | AsmInstr::Jmp(_)
        | AsmInstr::NonlocalJmp(_)
        | AsmInstr::JmpCC(_, _)
        | AsmInstr::Label(_)
        | AsmInstr::Call(_, _, _, _, _)
        | AsmInstr::Pop(_)
        | AsmInstr::Cdq(_)
        | AsmInstr::Unreachable
        | AsmInstr::Ret
        | AsmInstr::AllocateStack(_)
        | AsmInstr::DeallocateStack(_)
        | AsmInstr::AArch64SaveLink(_)
        | AsmInstr::AArch64RestoreLink(_)
        | AsmInstr::AArch64AllocateLargeStack(_)
        | AsmInstr::AArch64DeallocateLargeStack(_)
        | AsmInstr::AArch64StoreLargeLocalBase { .. }
        | AsmInstr::X86SetVarargsXmmCount(_)
        | AsmInstr::AtomicFence
        | AsmInstr::X87Compare
        | AsmInstr::X87UnaryNeg => false,
    }
}

pub fn validate_asm_program(program: &AsmProgram) -> Result<(), String> {
    let mut all_labels = HashSet::new();
    for item in &program.top_level {
        let AsmTopLevel::Function(function) = item else {
            continue;
        };
        for instr in &function.instructions {
            if let AsmInstr::Label(label) = instr {
                if !all_labels.insert(label.as_str()) {
                    return Err(format!(
                        "function '{}' has duplicate assembly label '{}'",
                        function.name, label
                    ));
                }
            }
        }
    }

    for item in &program.top_level {
        let AsmTopLevel::Function(function) = item else {
            continue;
        };
        let mut labels = HashSet::new();
        for instr in &function.instructions {
            if let AsmInstr::Label(label) = instr {
                if !labels.insert(label.as_str()) {
                    return Err(format!(
                        "function '{}' has duplicate assembly label '{}'",
                        function.name, label
                    ));
                }
            }
        }
        for instr in &function.instructions {
            match instr {
                AsmInstr::Jmp(label) | AsmInstr::JmpCC(_, label)
                    if !labels.contains(label.as_str()) =>
                {
                    return Err(format!(
                        "function '{}' jumps to undefined assembly label '{}'",
                        function.name, label
                    ));
                }
                AsmInstr::LoadLabelAddress(label, _) if !all_labels.contains(label.as_str()) => {
                    return Err(format!(
                        "function '{}' references undefined assembly label '{}'",
                        function.name, label
                    ));
                }
                _ if asm_instr_has_pseudo(instr) => {
                    return Err(format!(
                        "function '{}' has unresolved pseudo operand in {:?}",
                        function.name, instr
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn compile(stage: &Stage, src_file: &str, options: CompileOptions<'_>) -> Result<(), String> {
    let target = options.target;
    let opt_flags = options.opt_flags;
    let no_coalescing = options.no_coalescing;
    let instrument_functions = options.instrument_functions;
    let compatibility = options.compatibility;
    let dumps = options.dumps;
    let warnings = options.warnings;

    let source_bytes =
        std::fs::read(src_file).map_err(|err| format!("could not read {}: {}", src_file, err))?;
    let source = decode_c_source_bytes(&source_bytes);
    let mapped_source = strip_preprocessor_line_markers_with_map(&source);

    // Lex
    let spanned_tokens =
        lex::lex_spanned_with_line_map(&mapped_source.source, mapped_source.line_map.clone())
            .map_err(|err| {
                render_lex_error(&mapped_source.source, &mapped_source.line_map, &err)
            })?;
    let tokens: Vec<_> = spanned_tokens
        .iter()
        .map(|spanned| spanned.token.clone())
        .collect();
    if *stage == Stage::Lex {
        println!("{:#?}", tokens);
        return Ok(());
    }

    // Parse
    let ast = parse::parse_from_spanned_with_target(spanned_tokens.clone(), *target)?;
    if *stage == Stage::Parse {
        println!("{:#?}", ast);
        return Ok(());
    }
    if dumps.ast {
        eprintln!("{:#?}", ast);
    }

    // Validate (resolve variables, label loops)
    let resolved = resolve::resolve(ast).map_err(|diagnostic| diagnostic.render())?;
    let active_warnings: Vec<_> = resolved
        .warnings
        .iter()
        .filter(|warning| warnings.allows(warning))
        .collect();
    for warning in &active_warnings {
        eprintln!("rnqcc: {}", warning.render());
    }
    if warnings.error && !active_warnings.is_empty() {
        return Err("warnings treated as errors".to_string());
    }
    let resolved_ast = resolved.program;
    if *stage == Stage::Validate {
        println!("{:#?}", resolved_ast);
        return Ok(());
    }

    // Generate TACKY IR
    let tacky_output = tacky::generate_with_target_options_and_warnings(
        resolved_ast,
        *target,
        instrument_functions,
        compatibility.permissive,
    )
    .map_err(|err| Diagnostic::tacky(err).render())?;
    let active_warnings: Vec<_> = tacky_output
        .warnings
        .iter()
        .filter(|warning| warnings.allows(warning))
        .collect();
    for warning in &active_warnings {
        eprintln!("rnqcc: {}", warning.render());
    }
    if warnings.error && !active_warnings.is_empty() {
        return Err("warnings treated as errors".to_string());
    }
    let mut tacky_program = tacky_output.program;
    validate_tacky_program(&tacky_program).map_err(|err| Diagnostic::tacky(err).render())?;
    if dumps.tacky_pre_opt {
        eprintln!("{:#?}", tacky_program);
    }
    if *stage == Stage::Tacky {
        println!("{:#?}", tacky_program);
        return Ok(());
    }

    // Optimize TACKY IR
    optimize::optimize_program(&mut tacky_program, opt_flags);
    validate_tacky_program(&tacky_program).map_err(|err| Diagnostic::tacky(err).render())?;
    if dumps.tacky {
        eprintln!("{:#?}", tacky_program);
    }

    // Generate assembly IR and emit
    let asm_program = backend::codegen(&tacky_program, target, no_coalescing)?;
    validate_asm_program(&asm_program)?;
    if *stage == Stage::Codegen {
        println!("{:#?}", asm_program);
        return Ok(());
    }
    if dumps.asm_ir {
        eprintln!("{:#?}", asm_program);
    }

    let asm_filename = std::path::Path::new(src_file)
        .with_extension("s")
        .to_string_lossy()
        .into_owned();
    let temporary_asm_filename = temporary_assembly_filename(&asm_filename)?;
    let temporary_asm_guard = crate::tempfile::TempFile::new(&temporary_asm_filename);
    let emit_result = (|| -> Result<(), String> {
        backend::emit(&temporary_asm_filename, &asm_program, target)?;
        if dumps.source_comments {
            prepend_source_comment(&temporary_asm_filename, src_file)?;
        }
        publish_assembly(&temporary_asm_filename, &asm_filename)?;
        Ok(())
    })();
    drop(temporary_asm_guard);
    emit_result?;
    Ok(())
}

fn temporary_assembly_filename(assembly_filename: &str) -> Result<String, String> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::path::Path::new(assembly_filename);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("assembly.s");
    loop {
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = path
            .with_file_name(format!(
                ".{name}.rnqcc-{}-{counter}.tmp",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned();
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                drop(file);
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "could not create temporary assembly {}: {}",
                    candidate, error
                ));
            }
        }
    }
}

fn publish_assembly(temporary_filename: &str, assembly_filename: &str) -> Result<(), String> {
    let existing_permissions = match std::fs::metadata(assembly_filename) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "could not inspect existing assembly {}: {}",
                assembly_filename, error
            ));
        }
    };
    if let Some(permissions) = existing_permissions {
        std::fs::set_permissions(temporary_filename, permissions).map_err(|error| {
            format!(
                "could not preserve permissions for {}: {}",
                assembly_filename, error
            )
        })?;
    }
    match std::fs::rename(temporary_filename, assembly_filename) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            #[cfg(windows)]
            {
                if std::fs::symlink_metadata(assembly_filename).is_ok() {
                    std::fs::remove_file(assembly_filename).map_err(|remove_error| {
                        format!(
                            "could not replace existing assembly {}: {}; initial rename failed: {}",
                            assembly_filename, remove_error, rename_error
                        )
                    })?;
                    return std::fs::rename(temporary_filename, assembly_filename).map_err(
                        |retry_error| {
                            format!(
                                "could not publish {} as {} after replacing the existing file: {}",
                                temporary_filename, assembly_filename, retry_error
                            )
                        },
                    );
                }
            }

            Err(format!(
                "could not publish {} as {}: {}",
                temporary_filename, assembly_filename, rename_error
            ))
        }
    }
}

/// C source is byte-oriented. Valid UTF-8 spelling should survive for extended
/// identifiers, while raw non-UTF-8 bytes from preprocessors still need stable
/// single-byte code points for legacy escape handling.
pub fn decode_c_source_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                break;
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0 {
                    out.push_str(std::str::from_utf8(&rest[..valid_up_to]).unwrap_or(""));
                }
                let invalid_len = err.error_len().unwrap_or(1);
                for byte in &rest[valid_up_to..valid_up_to + invalid_len] {
                    out.push(char::from(*byte));
                }
                rest = &rest[valid_up_to + invalid_len..];
            }
        }
    }
    out
}

struct MappedSource {
    source: String,
    line_map: Vec<lex::SourceLineMapping>,
}

struct PreprocessorLineMarker {
    line: usize,
    file: Option<String>,
}

struct LineMarkerFilename {
    file: String,
}

fn strip_preprocessor_line_markers_with_map(source: &str) -> MappedSource {
    let mut out = String::with_capacity(source.len());
    let mut line_map = Vec::new();
    let mut logical_file: Option<String> = None;
    let mut next_logical_line = 1usize;

    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if is_preprocessor_line_marker(trimmed) {
            if let Some(marker) = parse_preprocessor_line_marker(trimmed) {
                next_logical_line = marker.line;
                if let Some(file) = marker.file {
                    logical_file = Some(file);
                }
            }
            if line.ends_with('\n') {
                out.push('\n');
                line_map.push(lex::SourceLineMapping {
                    file: logical_file.clone(),
                    line: next_logical_line,
                });
            }
        } else {
            out.push_str(line);
            line_map.push(lex::SourceLineMapping {
                file: logical_file.clone(),
                line: next_logical_line,
            });
            next_logical_line = next_logical_line.saturating_add(1);
        }
    }
    MappedSource {
        source: out,
        line_map,
    }
}

fn is_preprocessor_line_marker(trimmed_line: &str) -> bool {
    if let Some(rest) = trimmed_line.strip_prefix("#line") {
        return match rest.chars().next() {
            Some(ch) => ch.is_ascii_whitespace(),
            None => true,
        };
    }
    let Some(rest) = trimmed_line.strip_prefix('#') else {
        return false;
    };
    rest.trim_start()
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
}

fn parse_preprocessor_line_marker(trimmed_line: &str) -> Option<PreprocessorLineMarker> {
    let rest = if let Some(rest) = trimmed_line.strip_prefix("#line") {
        rest
    } else {
        trimmed_line.strip_prefix('#')?
    };
    let rest = rest.trim_start();
    let digits_len = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    if digits_len == 0 {
        return None;
    }
    let line = rest[..digits_len].parse::<usize>().ok()?;
    let rest = rest[digits_len..].trim_start();
    let file = rest
        .strip_prefix('"')
        .and_then(|rest| parse_line_marker_filename(rest).map(|filename| filename.file));
    Some(PreprocessorLineMarker { line, file })
}

fn parse_line_marker_filename(rest: &str) -> Option<LineMarkerFilename> {
    let mut file = String::with_capacity(rest.len());
    let mut escaped = false;
    for ch in rest.chars() {
        if escaped {
            file.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(LineMarkerFilename { file }),
            _ => file.push(ch),
        }
    }
    None
}

fn render_lex_error(source: &str, line_map: &[lex::SourceLineMapping], error: &str) -> String {
    let Some(offset) = lex_error_offset(error) else {
        return format!("lex failed: {}", error);
    };
    let lexer = lex::Lexer::with_line_map(source, line_map.to_vec());
    let span = lexer.span_for_offsets(offset, offset);
    let location = match &span.start.file {
        Some(file) => format!("{}:{}:{}", file, span.start.line, span.start.column),
        None => format!("{}:{}", span.start.line, span.start.column),
    };
    format!("lex failed at {}: {}", location, error)
}

fn lex_error_offset(error: &str) -> Option<usize> {
    let marker = "position ";
    let start = error.find(marker)? + marker.len();
    let digits: String = error[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

fn prepend_source_comment(asm_filename: &str, src_file: &str) -> Result<(), String> {
    let body = std::fs::read_to_string(asm_filename)
        .map_err(|err| format!("could not read {}: {}", asm_filename, err))?;
    let comment = format!("# rnqcc source: {}\n", src_file);
    let mut output = String::with_capacity(comment.len() + body.len());
    output.push_str(&comment);
    output.push_str(&body);
    std::fs::write(asm_filename, output)
        .map_err(|err| format!("could not write {}: {}", asm_filename, err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    static PUBLICATION_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn require_err<T>(result: Result<T, String>, context: &str) -> Result<String, String> {
        match result {
            Ok(_) => Err(format!("{context} unexpectedly succeeded")),
            Err(err) => Ok(err),
        }
    }

    #[test]
    fn assembly_publication_reserves_unique_temporary_paths() -> Result<(), String> {
        let id = PUBLICATION_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let assembly = std::env::temp_dir().join(format!(
            "rnqcc-assembly-publication-{}-{id}.s",
            std::process::id()
        ));
        let assembly = assembly.to_string_lossy().into_owned();
        let first = temporary_assembly_filename(&assembly)?;
        let second = temporary_assembly_filename(&assembly)?;
        assert_ne!(first, second);

        let first_guard = crate::tempfile::TempFile::new(&first);
        let second_guard = crate::tempfile::TempFile::new(&second);
        std::fs::write(&first, "new assembly\n").map_err(|err| err.to_string())?;
        std::fs::write(&assembly, "old assembly\n").map_err(|err| err.to_string())?;
        let assembly_guard = crate::tempfile::TempFile::new(&assembly);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&assembly)
                .map_err(|err| err.to_string())?
                .permissions();
            permissions.set_mode(0o600);
            std::fs::set_permissions(&assembly, permissions).map_err(|err| err.to_string())?;
        }
        publish_assembly(&first, &assembly)?;
        assert_eq!(
            std::fs::read_to_string(&assembly).map_err(|err| err.to_string())?,
            "new assembly\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&assembly)
                    .map_err(|err| err.to_string())?
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(!std::path::Path::new(&first).exists());
        drop(first_guard);
        drop(second_guard);
        drop(assembly_guard);
        Ok(())
    }

    #[test]
    fn tacky_validator_rejects_missing_jump_label() -> Result<(), String> {
        let program = TackyProgram {
            top_level: vec![TackyTopLevel::Function(TackyFunction {
                name: "main".to_string(),
                return_type: CType::Int,
                params: Vec::new(),
                global: true,
                body: vec![TackyInstr::Jump("missing".to_string())],
                stack_params: HashSet::new(),
                memory_param_blocks: Vec::new(),
                struct_param_groups: Vec::new(),
            })],
            global_vars: HashSet::new(),
            thread_local_vars: HashSet::new(),
            symbol_types: Default::default(),
            symbol_alignments: Default::default(),
            array_sizes: Default::default(),
            struct_defs: Default::default(),
            var_struct_tags: Default::default(),
        };

        let err = require_err(
            validate_tacky_program(&program),
            "validator should reject bad label",
        )?;
        assert!(err.contains("undefined TACKY label"));
        Ok(())
    }

    #[test]
    fn tacky_validator_rejects_duplicate_labels() -> Result<(), String> {
        let program = TackyProgram {
            top_level: vec![TackyTopLevel::Function(TackyFunction {
                name: "main".to_string(),
                return_type: CType::Int,
                params: Vec::new(),
                global: true,
                body: vec![
                    TackyInstr::Label("duplicate".to_string()),
                    TackyInstr::Label("duplicate".to_string()),
                ],
                stack_params: HashSet::new(),
                memory_param_blocks: Vec::new(),
                struct_param_groups: Vec::new(),
            })],
            global_vars: HashSet::new(),
            thread_local_vars: HashSet::new(),
            symbol_types: Default::default(),
            symbol_alignments: Default::default(),
            array_sizes: Default::default(),
            struct_defs: Default::default(),
            var_struct_tags: Default::default(),
        };

        let err = require_err(
            validate_tacky_program(&program),
            "validator should reject duplicate TACKY labels",
        )?;
        assert!(err.contains("duplicate TACKY label"));
        Ok(())
    }

    #[test]
    fn tacky_validator_rejects_duplicate_labels_across_functions() -> Result<(), String> {
        let function = |name: &str| {
            TackyTopLevel::Function(TackyFunction {
                name: name.to_string(),
                return_type: CType::Int,
                params: Vec::new(),
                global: true,
                body: vec![TackyInstr::Label("shared".to_string())],
                stack_params: HashSet::new(),
                memory_param_blocks: Vec::new(),
                struct_param_groups: Vec::new(),
            })
        };
        let program = TackyProgram {
            top_level: vec![function("first"), function("second")],
            global_vars: HashSet::new(),
            thread_local_vars: HashSet::new(),
            symbol_types: Default::default(),
            symbol_alignments: Default::default(),
            array_sizes: Default::default(),
            struct_defs: Default::default(),
            var_struct_tags: Default::default(),
        };

        let err = require_err(
            validate_tacky_program(&program),
            "validator should reject cross-function duplicate TACKY labels",
        )?;
        assert!(err.contains("duplicate TACKY label"));
        Ok(())
    }

    #[test]
    fn tacky_validator_rejects_undefined_nonlocal_label_references() -> Result<(), String> {
        let instructions = vec![
            TackyInstr::NonlocalJump("missing".to_string()),
            TackyInstr::LoadLabelAddress(
                "missing".to_string(),
                TackyVal::Var("target".to_string()),
            ),
        ];

        for instruction in instructions {
            let program = TackyProgram {
                top_level: vec![TackyTopLevel::Function(TackyFunction {
                    name: "main".to_string(),
                    return_type: CType::Int,
                    params: Vec::new(),
                    global: true,
                    body: vec![instruction],
                    stack_params: HashSet::new(),
                    memory_param_blocks: Vec::new(),
                    struct_param_groups: Vec::new(),
                })],
                global_vars: HashSet::new(),
                thread_local_vars: HashSet::new(),
                symbol_types: Default::default(),
                symbol_alignments: Default::default(),
                array_sizes: Default::default(),
                struct_defs: Default::default(),
                var_struct_tags: Default::default(),
            };
            let err = require_err(
                validate_tacky_program(&program),
                "validator should reject undefined nonlocal label reference",
            )?;
            assert!(err.contains("undefined TACKY label"));
        }
        Ok(())
    }

    #[test]
    fn tacky_validator_rejects_overflowing_argument_group_ranges() -> Result<(), String> {
        let program = TackyProgram {
            top_level: vec![TackyTopLevel::Function(TackyFunction {
                name: "main".to_string(),
                return_type: CType::Int,
                params: Vec::new(),
                global: true,
                body: vec![TackyInstr::FunCall {
                    name: "callee".to_string(),
                    args: Vec::new(),
                    dst: TackyVal::Var("result".to_string()),
                    stack_arg_indices: HashSet::new(),
                    memory_arg_blocks: Vec::new(),
                    struct_arg_groups: vec![(usize::MAX, 1, vec![false])],
                    variadic: false,
                    fixed_flat_arg_count: 0,
                    hidden_return: false,
                    indirect: false,
                }],
                stack_params: HashSet::new(),
                memory_param_blocks: Vec::new(),
                struct_param_groups: Vec::new(),
            })],
            global_vars: HashSet::new(),
            thread_local_vars: HashSet::new(),
            symbol_types: Default::default(),
            symbol_alignments: Default::default(),
            array_sizes: Default::default(),
            struct_defs: Default::default(),
            var_struct_tags: Default::default(),
        };

        let err = require_err(
            validate_tacky_program(&program),
            "validator should reject overflowing argument range",
        )?;
        assert!(err.contains("invalid struct argument group"));
        Ok(())
    }

    #[test]
    fn tacky_validator_rejects_nonassignable_special_destinations() -> Result<(), String> {
        let invalid_destination = TackyVal::Constant(0);
        let instructions = vec![
            TackyInstr::FrameAddress {
                dst: invalid_destination.clone(),
            },
            TackyInstr::LoadLabelAddress("label".to_string(), invalid_destination.clone()),
            TackyInstr::VaStart {
                dst: invalid_destination.clone(),
            },
            TackyInstr::AtomicFetch {
                op: TackyBinaryOp::Add,
                ptr: TackyVal::Constant(0),
                arg: TackyVal::Constant(1),
                return_old: false,
                dst: invalid_destination.clone(),
            },
            TackyInstr::AtomicExchange {
                ptr: TackyVal::Constant(0),
                value: TackyVal::Constant(1),
                dst: invalid_destination.clone(),
            },
            TackyInstr::AtomicCompareExchange {
                ptr: TackyVal::Constant(0),
                expected: TackyVal::Constant(1),
                desired: TackyVal::Constant(2),
                dst: invalid_destination.clone(),
            },
            TackyInstr::AtomicCompareSwap {
                ptr: TackyVal::Constant(0),
                expected: TackyVal::Constant(1),
                desired: TackyVal::Constant(2),
                return_old: false,
                dst: invalid_destination,
            },
        ];

        for instruction in instructions {
            let program = TackyProgram {
                top_level: vec![TackyTopLevel::Function(TackyFunction {
                    name: "main".to_string(),
                    return_type: CType::Int,
                    params: Vec::new(),
                    global: true,
                    body: vec![TackyInstr::Label("label".to_string()), instruction],
                    stack_params: HashSet::new(),
                    memory_param_blocks: Vec::new(),
                    struct_param_groups: Vec::new(),
                })],
                global_vars: HashSet::new(),
                thread_local_vars: HashSet::new(),
                symbol_types: Default::default(),
                symbol_alignments: Default::default(),
                array_sizes: Default::default(),
                struct_defs: Default::default(),
                var_struct_tags: Default::default(),
            };
            let err = require_err(
                validate_tacky_program(&program),
                "validator should reject non-assignable special destination",
            )?;
            assert!(err.contains("non-assignable TACKY destination"));
        }
        Ok(())
    }

    #[test]
    fn asm_validator_rejects_unresolved_pseudos() -> Result<(), String> {
        let instructions = vec![
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Pseudo("tmp".to_string()),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::JmpIndirect(AsmOperand::Pseudo("jump_target".to_string())),
            AsmInstr::CopyToStackArg {
                src_ptr: AsmOperand::Pseudo("src_ptr".to_string()),
                dst_offset: 0,
                size: 8,
            },
            AsmInstr::AtomicExchange(
                AsmType::Quadword,
                AsmOperand::Pseudo("atomic_value".to_string()),
            ),
            AsmInstr::Fld(AsmType::Double, AsmOperand::Pseudo("floating".to_string())),
        ];

        for instruction in instructions {
            let program = AsmProgram {
                top_level: vec![AsmTopLevel::Function(AsmFunction {
                    name: "main".to_string(),
                    global: true,
                    instructions: vec![instruction],
                })],
            };
            let err = require_err(
                validate_asm_program(&program),
                "validator should reject pseudos",
            )?;
            assert!(err.contains("unresolved pseudo operand"));
        }
        Ok(())
    }

    #[test]
    fn asm_validator_rejects_undefined_label_addresses() -> Result<(), String> {
        let instructions = vec![AsmInstr::LoadLabelAddress(
            "missing".to_string(),
            AsmOperand::Reg(Reg::AX),
        )];

        for instruction in instructions {
            let program = AsmProgram {
                top_level: vec![AsmTopLevel::Function(AsmFunction {
                    name: "main".to_string(),
                    global: true,
                    instructions: vec![instruction],
                })],
            };
            let err = require_err(
                validate_asm_program(&program),
                "validator should reject an undefined assembly label reference",
            )?;
            assert!(err.contains("undefined assembly label"));
        }
        Ok(())
    }

    #[test]
    fn asm_validator_rejects_duplicate_labels() -> Result<(), String> {
        let program = AsmProgram {
            top_level: vec![AsmTopLevel::Function(AsmFunction {
                name: "main".to_string(),
                global: true,
                instructions: vec![
                    AsmInstr::Label("duplicate".to_string()),
                    AsmInstr::Label("duplicate".to_string()),
                ],
            })],
        };

        let err = require_err(
            validate_asm_program(&program),
            "validator should reject duplicate assembly labels",
        )?;
        assert!(err.contains("duplicate assembly label"));
        Ok(())
    }

    #[test]
    fn asm_validator_rejects_duplicate_labels_across_functions() -> Result<(), String> {
        let function = |name: &str| {
            AsmTopLevel::Function(AsmFunction {
                name: name.to_string(),
                global: true,
                instructions: vec![AsmInstr::Label("shared".to_string())],
            })
        };
        let program = AsmProgram {
            top_level: vec![function("first"), function("second")],
        };

        let err = require_err(
            validate_asm_program(&program),
            "validator should reject cross-function duplicate assembly labels",
        )?;
        assert!(err.contains("duplicate assembly label"));
        Ok(())
    }

    #[test]
    fn maps_preprocessor_line_markers_to_logical_source_locations() -> Result<(), String> {
        let source = "# 1 \"input.c\"\n#line 20 \"generated.c\"\nint main(void) { return 0; }\n";
        let mapped = strip_preprocessor_line_markers_with_map(source);
        let tokens = lex::lex_spanned_with_line_map(&mapped.source, mapped.line_map)?;
        let first = tokens
            .first()
            .ok_or_else(|| "expected token after line markers".to_string())?;

        assert_eq!(first.span.start.file.as_deref(), Some("generated.c"));
        assert_eq!(first.span.start.line, 20);
        assert_eq!(first.span.start.column, 1);
        Ok(())
    }

    #[test]
    fn renders_lex_errors_with_logical_line_marker_locations() {
        let source = "# 40 \"generated.c\"\n@\n";
        let mapped = strip_preprocessor_line_markers_with_map(source);
        let err = lex::lex_spanned_with_line_map(&mapped.source, mapped.line_map.clone())
            .expect_err("lexing should fail");

        assert_eq!(
            render_lex_error(&mapped.source, &mapped.line_map, &err),
            "lex failed at generated.c:40:1: unexpected character '@' at position 1"
        );
    }
}
