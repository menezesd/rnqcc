use crate::backend;
use crate::diagnostic::Diagnostic;
use crate::lex;
use crate::optimize;
use crate::parse;
use crate::resolve;
use crate::tacky;
use crate::types::*;
use std::collections::HashSet;

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
    for item in &program.top_level {
        let TackyTopLevel::Function(function) = item else {
            continue;
        };
        let labels: HashSet<&str> = function
            .body
            .iter()
            .filter_map(|instr| match instr {
                TackyInstr::Label(label) => Some(label.as_str()),
                _ => None,
            })
            .collect();

        for instr in &function.body {
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
                        if *start + *count > args.len() || classes.len() != *count {
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

pub fn validate_asm_program(program: &AsmProgram) -> Result<(), String> {
    for item in &program.top_level {
        let AsmTopLevel::Function(function) = item else {
            continue;
        };
        let labels: HashSet<&str> = function
            .instructions
            .iter()
            .filter_map(|instr| match instr {
                AsmInstr::Label(label) => Some(label.as_str()),
                _ => None,
            })
            .collect();
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
                AsmInstr::Mov(_, src, dst)
                | AsmInstr::Movsx(_, _, src, dst)
                | AsmInstr::MovZeroExtend(_, _, src, dst)
                | AsmInstr::Binary(_, _, src, dst)
                | AsmInstr::Cmp(_, src, dst)
                | AsmInstr::Cvtsi2sd(_, src, dst)
                | AsmInstr::Cvtsi2ss(_, src, dst)
                | AsmInstr::Cvttsd2si(_, src, dst)
                | AsmInstr::Cvttss2si(_, src, dst)
                | AsmInstr::AArch64UIntToDouble(_, src, dst)
                | AsmInstr::AArch64UIntToFloat(_, src, dst)
                | AsmInstr::AArch64DoubleToUInt(_, src, dst)
                | AsmInstr::AArch64FloatToUInt(_, src, dst)
                | AsmInstr::Lea(src, dst)
                    if asm_operand_has_pseudo(src) || asm_operand_has_pseudo(dst) =>
                {
                    return Err(format!(
                        "function '{}' has unresolved pseudo operand in {:?}",
                        function.name, instr
                    ));
                }
                AsmInstr::Cvtss2sd(src, dst)
                | AsmInstr::Cvtsd2ss(src, dst)
                | AsmInstr::AArch64FloatToDouble(src, dst)
                | AsmInstr::AArch64DoubleToFloat(src, dst)
                    if asm_operand_has_pseudo(src) || asm_operand_has_pseudo(dst) =>
                {
                    return Err(format!(
                        "function '{}' has unresolved pseudo operand in {:?}",
                        function.name, instr
                    ));
                }
                AsmInstr::AArch64AddPtr(ptr, index, _, dst)
                    if asm_operand_has_pseudo(ptr)
                        || asm_operand_has_pseudo(index)
                        || asm_operand_has_pseudo(dst) =>
                {
                    return Err(format!(
                        "function '{}' has unresolved pseudo operand in {:?}",
                        function.name, instr
                    ));
                }
                AsmInstr::AArch64Rem(_, _, left, right, dst)
                    if asm_operand_has_pseudo(left)
                        || asm_operand_has_pseudo(right)
                        || asm_operand_has_pseudo(dst) =>
                {
                    return Err(format!(
                        "function '{}' has unresolved pseudo operand in {:?}",
                        function.name, instr
                    ));
                }
                AsmInstr::LoadIndirect(_, _, dst)
                | AsmInstr::StoreIndirect(_, dst, _)
                | AsmInstr::AArch64LoadAdjusted(_, dst, _, _)
                | AsmInstr::AArch64StoreOutgoingArg(_, dst, _, _)
                    if asm_operand_has_pseudo(dst) =>
                {
                    return Err(format!(
                        "function '{}' has unresolved pseudo operand in {:?}",
                        function.name, instr
                    ));
                }
                AsmInstr::Unary(_, _, operand)
                | AsmInstr::Idiv(_, operand)
                | AsmInstr::Div(_, operand)
                | AsmInstr::SetCC(_, operand)
                | AsmInstr::Push(operand)
                    if asm_operand_has_pseudo(operand) =>
                {
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

    // C source is byte-oriented. External preprocessors can materialize string
    // escapes such as \377 as raw non-UTF-8 bytes in .i output, so preserve
    // each input byte as a single scalar value for the lexer.
    let source_bytes =
        std::fs::read(src_file).map_err(|err| format!("could not read {}: {}", src_file, err))?;
    let source: String = source_bytes.into_iter().map(char::from).collect();
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
    backend::emit(&asm_filename, &asm_program, target)?;
    if dumps.source_comments {
        prepend_source_comment(&asm_filename, src_file)?;
    }
    Ok(())
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

#[allow(dead_code)]
fn strip_preprocessor_line_markers(source: &str) -> String {
    strip_preprocessor_line_markers_with_map(source).source
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
    let mut file = String::new();
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
    std::fs::write(asm_filename, format!("{}{}", comment, body))
        .map_err(|err| format!("could not write {}: {}", asm_filename, err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn require_err<T>(result: Result<T, String>, context: &str) -> Result<String, String> {
        match result {
            Ok(_) => Err(format!("{context} unexpectedly succeeded")),
            Err(err) => Ok(err),
        }
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
    fn asm_validator_rejects_unresolved_pseudos() -> Result<(), String> {
        let program = AsmProgram {
            top_level: vec![AsmTopLevel::Function(AsmFunction {
                name: "main".to_string(),
                global: true,
                instructions: vec![AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Pseudo("tmp".to_string()),
                    AsmOperand::Reg(Reg::AX),
                )],
            })],
        };

        let err = require_err(
            validate_asm_program(&program),
            "validator should reject pseudos",
        )?;
        assert!(err.contains("unresolved pseudo operand"));
        Ok(())
    }

    #[test]
    fn strips_preprocessor_line_markers_before_lexing() {
        let source = "# 1 \"input.c\"\n#line 20 \"generated.c\"\nint main(void) { return 0; }\n";
        let stripped = strip_preprocessor_line_markers(source);
        assert_eq!(stripped, "\n\nint main(void) { return 0; }\n");
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
