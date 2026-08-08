//! Internal C preprocessor: macro expansion, include resolution, virtual system
//! headers, and pragma handling. Split out of the CLI driver module.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use rnqcc::preprocess::prefix::{escape_c_string, splice_continued_lines, strip_comments};
use rnqcc::types::*;
use rnqcc::{compile, preprocess, tempfile};

use super::*;

const MAX_INCLUDE_DEPTH: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacroDef {
    Object(String),
    Function {
        params: Vec<String>,
        variadic: bool,
        body: String,
    },
}

pub fn is_ident_start(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_start(ch)
}

pub fn is_ident_continue(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_continue(ch)
}

pub struct PreprocessorState {
    counter: usize,
    base_file: String,
    date: String,
    time: String,
}

pub fn civil_date_from_days(days: i64) -> (i32, u32, u32) {
    // Keep the intermediate arithmetic wider than the public result.  The
    // SOURCE_DATE_EPOCH environment variable is parsed as an i64, so values
    // near either limit must not overflow while converting to a civil date.
    let days = i128::from(days);
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
    let year = if year < i128::from(i32::MIN) {
        i32::MIN
    } else if year > i128::from(i32::MAX) {
        i32::MAX
    } else {
        year as i32
    };
    (year, month as u32, day as u32)
}

pub fn format_c_date_time(seconds: i64) -> (String, String) {
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

pub fn expand_macros_with_context(
    line: &str,
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<String, String> {
    expand_macros_with_tokens(line, macros, file, line_number, include_level, state)
}

pub fn expand_macros_with_tokens(
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

pub fn token_macro_table(
    macros: &HashMap<String, MacroDef>,
) -> Result<preprocess::macro_expand::MacroTable, String> {
    let mut table = preprocess::macro_expand::MacroTable::new();
    for (name, def) in macros {
        table.insert(name.clone(), string_macro_def_to_token(def)?);
    }
    Ok(table)
}

pub fn string_macro_def_to_token(
    def: &MacroDef,
) -> Result<preprocess::macro_expand::MacroDef, String> {
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

pub fn macro_defs_equivalent(left: &MacroDef, right: &MacroDef) -> Result<bool, String> {
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

pub fn replacement_tokens_equivalent(left: &str, right: &str) -> Result<bool, String> {
    let left = preprocess::lexer::lex(left)?;
    let right = preprocess::lexer::lex(right)?;
    Ok(non_ws_token_texts(&left).eq(non_ws_token_texts(&right)))
}

pub fn non_ws_token_texts(tokens: &[preprocess::token::PpToken]) -> impl Iterator<Item = &str> {
    tokens.iter().filter_map(|token| match &token.kind {
        preprocess::token::PpTokenKind::Whitespace(_)
        | preprocess::token::PpTokenKind::Newline(_) => None,
        _ => Some(token.text()),
    })
}

pub struct LiveMacroExpansionHooks<'a> {
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
pub enum IncludeSpec {
    Quoted(String),
    Angled(String),
}

#[derive(Clone, Debug)]
pub struct IncludePaths {
    pub(crate) quote: Vec<PathBuf>,
    pub(crate) user: Vec<PathBuf>,
    pub(crate) system: Vec<PathBuf>,
    pub(crate) after: Vec<PathBuf>,
    pub(crate) use_standard_system: bool,
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
        let dirs: Vec<PathBuf> = std::iter::once(base_dir.to_path_buf())
            .chain(self.quote.iter().cloned())
            .chain(self.user.iter().cloned())
            .chain(self.system.iter().cloned())
            .chain(self.after.iter().cloned())
            .collect();
        let mut start = dirs
            .iter()
            .position(|dir| same_include_dir(dir, base_dir))
            .map(|index| index + 1)
            .unwrap_or(0);
        while start < dirs.len() && same_include_dir(&dirs[start], base_dir) {
            start += 1;
        }
        dirs[start..].to_vec()
    }
}

pub fn parse_include_tokens(
    tokens: &[preprocess::token::PpToken],
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<IncludeSpec, String> {
    let token_macros = token_macro_table(macros)?;
    let mut hooks = LiveMacroExpansionHooks {
        file,
        line_number,
        include_level,
        state,
    };
    let expanded =
        preprocess::macro_expand::expand_macros_with_hooks(tokens, &token_macros, &mut hooks)?;
    strict_include_spec_from_tokens(&expanded).ok_or_else(|| {
        format!(
            "malformed include operand: {}",
            preprocess::emit::emit_tokens(&expanded).trim()
        )
    })
}

pub fn strict_include_spec_from_tokens(
    tokens: &[preprocess::token::PpToken],
) -> Option<IncludeSpec> {
    let start = skip_include_ws(tokens, 0);
    match tokens.get(start).map(|token| &token.kind) {
        Some(preprocess::token::PpTokenKind::StringLit(text)) => {
            if text.trim_matches('"').is_empty() {
                return None;
            }
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
                    if name.is_empty() {
                        return None;
                    }
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

pub fn skip_include_ws(tokens: &[preprocess::token::PpToken], mut index: usize) -> usize {
    while matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(preprocess::token::PpTokenKind::Whitespace(_))
            | Some(preprocess::token::PpTokenKind::Newline(_))
    ) {
        index += 1;
    }
    index
}

pub fn only_include_ws(tokens: &[preprocess::token::PpToken], start: usize) -> bool {
    skip_include_ws(tokens, start) == tokens.len()
}

pub fn parse_token_include_operand(
    operand: &preprocess::directive::IncludeOperand,
    macros: &HashMap<String, MacroDef>,
    file: &str,
    line_number: usize,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<IncludeSpec, String> {
    match operand {
        preprocess::directive::IncludeOperand::Literal(header) => match header {
            preprocess::directive::HeaderName::Quoted(name) => {
                if name.is_empty() {
                    return Err("malformed include operand: \"\"".to_string());
                }
                Ok(IncludeSpec::Quoted(name.clone()))
            }
            preprocess::directive::HeaderName::Angled(name) => {
                if name.is_empty() {
                    return Err("malformed include operand: <>".to_string());
                }
                Ok(IncludeSpec::Angled(name.clone()))
            }
        },
        preprocess::directive::IncludeOperand::Tokens(tokens) => {
            parse_include_tokens(tokens, macros, file, line_number, include_level, state)
        }
    }
}

#[derive(Debug, Default)]
struct EmbedParameters {
    limit: Option<usize>,
    prefix: Option<String>,
    suffix: Option<String>,
    if_empty: Option<String>,
}

fn parse_embed_parameters(
    tokens: &[preprocess::token::PpToken],
    macros: &HashMap<String, MacroDef>,
) -> Result<EmbedParameters, String> {
    use preprocess::token::PpTokenKind;

    let mut result = EmbedParameters::default();
    let mut index = skip_include_ws(tokens, 0);
    while index < tokens.len() {
        let PpTokenKind::Ident(name) = &tokens[index].kind else {
            return Err("expected #embed parameter name".to_string());
        };
        let mut name = name.clone();
        index = skip_include_ws(tokens, index + 1);
        while matches!(tokens.get(index).map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == ":")
        {
            let second_colon = skip_include_ws(tokens, index + 1);
            if !matches!(tokens.get(second_colon).map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == ":")
            {
                return Err(format!("expected '::' after #embed parameter {}", name));
            }
            let ident = skip_include_ws(tokens, second_colon + 1);
            let Some(PpTokenKind::Ident(segment)) = tokens.get(ident).map(|token| &token.kind)
            else {
                return Err(format!(
                    "expected identifier after #embed parameter {}::",
                    name
                ));
            };
            name.push_str("::");
            name.push_str(segment);
            index = skip_include_ws(tokens, ident + 1);
        }
        if !matches!(tokens.get(index).map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == "(")
        {
            return Err(format!("expected '(' after #embed parameter {}", name));
        }
        let content_start = index + 1;
        let mut depth = 1usize;
        index += 1;
        while index < tokens.len() && depth > 0 {
            match &tokens[index].kind {
                PpTokenKind::Punct(value) if value == "(" => depth += 1,
                PpTokenKind::Punct(value) if value == ")" => depth -= 1,
                _ => {}
            }
            index += 1;
        }
        if depth != 0 {
            return Err(format!("missing ')' after #embed parameter {}", name));
        }
        let content = &tokens[content_start..index - 1];
        match name.as_str() {
            "limit" => {
                if result.limit.is_some() {
                    return Err("duplicate #embed limit parameter".to_string());
                }
                let value = preprocess::emit::emit_tokens(content);
                let value = IfExprParser::new(value.trim(), macros)
                    .parse()
                    .map_err(|err| format!("invalid #embed limit: {}", err))?;
                if !value.unsigned && value.signed_value() < 0 {
                    return Err("#embed limit must be a non-negative integer".to_string());
                }
                result.limit = Some(
                    usize::try_from(value.value)
                        .map_err(|_| "#embed limit is too large".to_string())?,
                );
            }
            "prefix" => {
                if result.prefix.is_some() {
                    return Err("duplicate #embed prefix parameter".to_string());
                }
                result.prefix = Some(preprocess::emit::emit_tokens(content));
            }
            "suffix" => {
                if result.suffix.is_some() {
                    return Err("duplicate #embed suffix parameter".to_string());
                }
                result.suffix = Some(preprocess::emit::emit_tokens(content));
            }
            "if_empty" => {
                if result.if_empty.is_some() {
                    return Err("duplicate #embed if_empty parameter".to_string());
                }
                result.if_empty = Some(preprocess::emit::emit_tokens(content));
            }
            _ => return Err(format!("unsupported #embed parameter {}", name)),
        }
        index = skip_include_ws(tokens, index);
    }
    Ok(result)
}

pub fn expand_preprocessor_tokens(
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

pub fn parse_token_line_operand(
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

pub fn parse_line_marker_tokens(
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

pub fn decode_line_filename(value: String) -> String {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedPragma {
    text: String,
    line_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PragmaOperatorError {
    message: String,
    line_offset: usize,
}

pub fn process_pragma_operators_located(
    line: &str,
) -> Result<(String, Vec<LocatedPragma>), PragmaOperatorError> {
    if !line.contains("_Pragma") {
        return Ok((line.to_string(), Vec::new()));
    }
    let tokens = preprocess::lexer::lex(line).map_err(|message| PragmaOperatorError {
        message,
        line_offset: 0,
    })?;
    let mut out = Vec::new();
    let mut pragmas = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        match parse_pragma_operator(&tokens, index) {
            Ok(Some((pragma, next_index))) => {
                pragmas.push(LocatedPragma {
                    text: pragma,
                    line_offset: tokens[index].span.start.line.saturating_sub(1),
                });
                index = next_index;
            }
            Ok(None) => {
                out.push(tokens[index].clone());
                index += 1;
            }
            Err(message) => {
                return Err(PragmaOperatorError {
                    message,
                    line_offset: tokens[index].span.start.line.saturating_sub(1),
                });
            }
        }
    }
    Ok((preprocess::emit::emit_tokens(&out), pragmas))
}

pub fn parse_pragma_operator(
    tokens: &[preprocess::token::PpToken],
    start: usize,
) -> Result<Option<(String, usize)>, String> {
    if !matches!(
        tokens.get(start).map(|token| &token.kind),
        Some(preprocess::token::PpTokenKind::Ident(name)) if name == "_Pragma"
    ) {
        return Ok(None);
    }
    let mut index = skip_include_ws(tokens, start + 1);
    if !matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(preprocess::token::PpTokenKind::Punct(value)) if value == "("
    ) {
        return Err("malformed _Pragma operator: expected '('".to_string());
    }
    index = skip_include_ws(tokens, index + 1);
    let Some(preprocess::token::PpToken {
        kind: preprocess::token::PpTokenKind::StringLit(text),
        ..
    }) = tokens.get(index)
    else {
        return Err("malformed _Pragma operator: expected string literal".to_string());
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
        return Err("malformed _Pragma operator: expected ')'".to_string());
    }
    Ok(Some((pragma, index + 1)))
}

pub fn line_operand_error(error: preprocess::directive::LineOperandError) -> String {
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

pub fn token_macro_def_to_string(def: preprocess::macro_expand::MacroDef) -> MacroDef {
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

pub fn format_macro_dump(macros: &HashMap<String, MacroDef>) -> String {
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

pub fn format_macro_param_list(params: &[String], variadic: bool) -> String {
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

pub fn starts_preprocessor_directive(trimmed: &str) -> bool {
    trimmed.starts_with('#') || trimmed.starts_with("%:")
}

pub fn raw_directive_name(trimmed: &str) -> Option<&str> {
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

pub fn trim_preprocessor_prefix(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("%:"))
        .map(str::trim_start)
}

pub fn is_conditional_control_directive(name: &str) -> bool {
    matches!(
        name,
        "if" | "ifdef" | "ifndef" | "elif" | "elifdef" | "elifndef" | "else" | "endif"
    )
}

pub fn same_include_dir(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub fn resolve_include_path(
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
        IncludeSpec::Quoted(name) | IncludeSpec::Angled(name) => dirs
            .into_iter()
            .map(|dir| dir.join(name))
            .find(|path| path.exists()),
    }
}

pub fn include_not_found(spec: &IncludeSpec) -> String {
    match spec {
        IncludeSpec::Quoted(name) => format!("include not found: \"{}\"", name),
        IncludeSpec::Angled(name) => format!("include not found: <{}>", name),
    }
}

#[derive(Clone, Copy)]
pub struct VirtualHeaderInfo {
    name: &'static str,
    guard: Option<&'static str>,
    policy: VirtualHeaderPolicy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VirtualHeaderPolicy {
    Fallback,
    PreferVirtual,
}

pub const fn virtual_header(name: &'static str, guard: Option<&'static str>) -> VirtualHeaderInfo {
    VirtualHeaderInfo {
        name,
        guard,
        policy: VirtualHeaderPolicy::Fallback,
    }
}

pub const fn preferred_virtual_header(
    name: &'static str,
    guard: Option<&'static str>,
) -> VirtualHeaderInfo {
    VirtualHeaderInfo {
        name,
        guard,
        policy: VirtualHeaderPolicy::PreferVirtual,
    }
}

pub const VIRTUAL_COMPAT_HEADERS: &[VirtualHeaderInfo] = &[
    virtual_header("assert.h", None),
    virtual_header("stdbool.h", None),
    virtual_header("stddef.h", Some("__rnqcc_stddef_h")),
    preferred_virtual_header("stdarg.h", Some("__rnqcc_stdarg_h")),
    virtual_header("stdatomic.h", Some("__rnqcc_stdatomic_h")),
    virtual_header("limits.h", Some("__rnqcc_limits_h")),
    virtual_header("stdint.h", Some("__rnqcc_stdint_h")),
    virtual_header("immintrin.h", Some("__rnqcc_immintrin_h")),
    virtual_header("inttypes.h", Some("__rnqcc_inttypes_h")),
    virtual_header("float.h", Some("__rnqcc_float_h")),
    virtual_header("iso646.h", None),
    virtual_header("ctype.h", Some("__rnqcc_ctype_h")),
    virtual_header("dirent.h", Some("__rnqcc_dirent_h")),
    virtual_header("errno.h", None),
    virtual_header("memory.h", Some("__rnqcc_memory_h")),
    virtual_header("malloc.h", Some("__rnqcc_malloc_h")),
    virtual_header("alloca.h", Some("__rnqcc_alloca_h")),
    virtual_header("locale.h", Some("__rnqcc_locale_h")),
    virtual_header("math.h", Some("__rnqcc_math_h")),
    virtual_header("regex.h", Some("__rnqcc_regex_h")),
    virtual_header("glob.h", Some("__rnqcc_glob_h")),
    virtual_header("fnmatch.h", Some("__rnqcc_fnmatch_h")),
    virtual_header("features.h", Some("__rnqcc_features_h")),
    virtual_header("dlfcn.h", Some("__rnqcc_dlfcn_h")),
    virtual_header("syslog.h", Some("__rnqcc_syslog_h")),
    virtual_header("utime.h", Some("__rnqcc_utime_h")),
    virtual_header("libgen.h", Some("__rnqcc_libgen_h")),
    virtual_header("getopt.h", Some("__rnqcc_getopt_h")),
    virtual_header("paths.h", None),
    virtual_header("sysexits.h", None),
    virtual_header("fcntl.h", Some("__rnqcc_fcntl_h")),
    virtual_header("poll.h", Some("__rnqcc_poll_h")),
    virtual_header("sys/poll.h", Some("__rnqcc_sys_poll_h")),
    virtual_header("setjmp.h", Some("__rnqcc_setjmp_h")),
    virtual_header("signal.h", Some("__rnqcc_signal_h")),
    virtual_header("stdio.h", Some("__rnqcc_stdio_h")),
    virtual_header("stdlib.h", Some("__rnqcc_stdlib_h")),
    virtual_header("string.h", Some("__rnqcc_string_h")),
    virtual_header("strings.h", Some("__rnqcc_strings_h")),
    virtual_header("stdalign.h", None),
    virtual_header("stdckdint.h", Some("__rnqcc_stdckdint_h")),
    virtual_header("stdnoreturn.h", None),
    virtual_header("sys/stat.h", Some("__rnqcc_sys_stat_h")),
    virtual_header("sys/cdefs.h", Some("__rnqcc_sys_cdefs_h")),
    virtual_header("sys/errno.h", None),
    virtual_header("sys/select.h", Some("__rnqcc_sys_select_h")),
    virtual_header("sys/socket.h", Some("__rnqcc_sys_socket_h")),
    virtual_header("sys/un.h", Some("__rnqcc_sys_un_h")),
    virtual_header("sys/ioctl.h", Some("__rnqcc_sys_ioctl_h")),
    virtual_header("sys/file.h", Some("__rnqcc_sys_file_h")),
    virtual_header("sys/mman.h", Some("__rnqcc_sys_mman_h")),
    virtual_header("sys/param.h", Some("__rnqcc_sys_param_h")),
    virtual_header("sys/resource.h", Some("__rnqcc_sys_resource_h")),
    virtual_header("sys/time.h", Some("__rnqcc_sys_time_h")),
    virtual_header("sys/types.h", Some("__rnqcc_sys_types_defined")),
    virtual_header("sys/uio.h", Some("__rnqcc_sys_uio_h")),
    virtual_header("sys/sysmacros.h", None),
    virtual_header("sys/utsname.h", Some("__rnqcc_sys_utsname_h")),
    virtual_header("sys/wait.h", Some("__rnqcc_sys_wait_h")),
    virtual_header("arpa/inet.h", Some("__rnqcc_arpa_inet_h")),
    virtual_header("netinet/in.h", Some("__rnqcc_netinet_in_h")),
    virtual_header("netinet/tcp.h", Some("__rnqcc_netinet_tcp_h")),
    virtual_header("netinet/ip.h", Some("__rnqcc_netinet_ip_h")),
    virtual_header("netinet/udp.h", Some("__rnqcc_netinet_udp_h")),
    virtual_header("net/if.h", Some("__rnqcc_net_if_h")),
    virtual_header("ifaddrs.h", Some("__rnqcc_ifaddrs_h")),
    virtual_header("netdb.h", Some("__rnqcc_netdb_h")),
    virtual_header("resolv.h", Some("__rnqcc_resolv_h")),
    virtual_header("linux/limits.h", Some("__rnqcc_linux_limits_h")),
    virtual_header("time.h", Some("__rnqcc_time_h")),
    virtual_header("pthread.h", Some("__rnqcc_pthread_h")),
    virtual_header("grp.h", Some("__rnqcc_grp_h")),
    virtual_header("pwd.h", Some("__rnqcc_pwd_h")),
    virtual_header("termios.h", Some("__rnqcc_termios_h")),
    virtual_header("unistd.h", Some("__rnqcc_unistd_h")),
    virtual_header("wchar.h", Some("__rnqcc_wchar_h")),
    virtual_header("wctype.h", Some("__rnqcc_wctype_h")),
];

pub fn virtual_header_info(name: &str) -> Option<VirtualHeaderInfo> {
    VIRTUAL_COMPAT_HEADERS
        .iter()
        .copied()
        .find(|header| header.name == name)
}

pub fn virtual_header_for_include(spec: &IncludeSpec) -> Option<VirtualHeaderInfo> {
    match spec {
        IncludeSpec::Angled(name) => virtual_header_info(name),
        _ => None,
    }
}

pub fn virtual_compat_header_name(spec: &IncludeSpec) -> Option<&str> {
    virtual_header_for_include(spec).map(|header| header.name)
}

pub fn forced_virtual_header_name(spec: &IncludeSpec, include_next: bool) -> Option<&str> {
    if include_next {
        return None;
    }
    virtual_header_for_include(spec).and_then(|header| {
        (header.policy == VirtualHeaderPolicy::PreferVirtual).then_some(header.name)
    })
}

pub fn virtual_header_is_available(spec: &IncludeSpec, include_next: bool) -> bool {
    !include_next && virtual_header_for_include(spec).is_some()
}

pub fn emit_virtual_include(
    out: &mut String,
    name: &str,
    macros: &mut HashMap<String, MacroDef>,
    next_logical_line: usize,
    logical_file: &str,
    context: &mut InternalPreprocessContext<'_>,
) {
    let included = include_virtual_compat_header(name, macros);
    context.invalidate_token_macro_cache();
    if let Some(stats) = context.stats_mut() {
        stats.virtual_includes += 1;
    }
    out.push_str(&included);
    if !included.is_empty() && !included.ends_with('\n') {
        out.push('\n');
    }
    if context.line_markers && !context.suppress_preprocessed_output {
        push_line_marker(out, next_logical_line, logical_file);
    }
}

pub fn virtual_size_t_typedef(macros: &mut HashMap<String, MacroDef>) -> &'static str {
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

pub fn virtual_ssize_t_typedef(macros: &mut HashMap<String, MacroDef>) -> &'static str {
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

pub fn virtual_time_t_typedef(macros: &mut HashMap<String, MacroDef>) -> &'static str {
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

pub fn virtual_null_macro(macros: &mut HashMap<String, MacroDef>) {
    macros.insert(
        "NULL".to_string(),
        MacroDef::Object("((void *)0)".to_string()),
    );
}

pub fn virtual_include_once(macros: &mut HashMap<String, MacroDef>, key: &str) -> bool {
    if macros.contains_key(key) {
        true
    } else {
        macros.insert(key.to_string(), MacroDef::Object("1".to_string()));
        false
    }
}

pub fn virtual_header_include_once(macros: &mut HashMap<String, MacroDef>, name: &str) -> bool {
    virtual_header_info(name)
        .and_then(|header| header.guard)
        .is_some_and(|guard| virtual_include_once(macros, guard))
}

pub fn define_virtual_object_macros(
    macros: &mut HashMap<String, MacroDef>,
    entries: &[(&str, &str)],
) {
    for (name, value) in entries {
        macros.insert((*name).to_string(), MacroDef::Object((*value).to_string()));
    }
}

pub fn target_long_double_max_macro(target: &Target) -> &'static str {
    if target.long_double_size() == 8 {
        "1.7976931348623157e+308L"
    } else {
        "__builtin_huge_vall()"
    }
}

pub fn target_long_double_limits(target: &Target) -> [(&'static str, &'static str); 6] {
    if target.long_double_size() == 8 {
        return [
            ("mant_dig", "53"),
            ("dig", "15"),
            ("min_exp", "(-1021)"),
            ("max_exp", "1024"),
            ("min", "2.2250738585072014e-308L"),
            ("epsilon", "2.2204460492503131e-16L"),
        ];
    }

    if target.arch == Arch::AArch64 {
        [
            ("mant_dig", "113"),
            ("dig", "33"),
            ("min_exp", "(-16381)"),
            ("max_exp", "16384"),
            ("min", "3.36210314311209350626267781732175260e-4932L"),
            ("epsilon", "1.92592994438723585305597794258492732e-34L"),
        ]
    } else {
        [
            ("mant_dig", "64"),
            ("dig", "18"),
            ("min_exp", "(-16381)"),
            ("max_exp", "16384"),
            ("min", "3.36210314311209350626e-4932L"),
            ("epsilon", "1.08420217248550443401e-19L"),
        ]
    }
}

pub fn target_long_double_limit(target: &Target, key: &str) -> &'static str {
    target_long_double_limits(target)
        .into_iter()
        .find_map(|(name, value)| if name == key { Some(value) } else { None })
        .unwrap_or("")
}

pub fn target_long_double_min_macro(target: &Target) -> &'static str {
    target_long_double_limit(target, "min")
}

pub fn target_long_double_epsilon_macro(target: &Target) -> &'static str {
    target_long_double_limit(target, "epsilon")
}

pub fn target_long_double_mant_dig_macro(target: &Target) -> &'static str {
    target_long_double_limit(target, "mant_dig")
}

pub fn target_long_double_dig_macro(target: &Target) -> &'static str {
    target_long_double_limit(target, "dig")
}

pub fn target_long_double_min_exp_macro(target: &Target) -> &'static str {
    target_long_double_limit(target, "min_exp")
}

pub fn target_long_double_max_exp_macro(target: &Target) -> &'static str {
    target_long_double_limit(target, "max_exp")
}

pub fn virtual_long_double_max_macro(macros: &HashMap<String, MacroDef>) -> &'static str {
    if matches!(
        macros.get("__SIZEOF_LONG_DOUBLE__"),
        Some(MacroDef::Object(size)) if size == "8"
    ) {
        "1.7976931348623157e+308L"
    } else {
        "__builtin_huge_vall()"
    }
}

pub fn virtual_long_double_limits(
    macros: &HashMap<String, MacroDef>,
) -> [(&'static str, &'static str); 6] {
    if matches!(
        macros.get("__SIZEOF_LONG_DOUBLE__"),
        Some(MacroDef::Object(size)) if size == "8"
    ) {
        return [
            ("LDBL_MANT_DIG", "53"),
            ("LDBL_DIG", "15"),
            ("LDBL_MIN_EXP", "(-1021)"),
            ("LDBL_MAX_EXP", "1024"),
            ("LDBL_MIN", "2.2250738585072014e-308L"),
            ("LDBL_EPSILON", "2.2204460492503131e-16L"),
        ];
    }

    if macros.contains_key("__aarch64__") || macros.contains_key("__arm64__") {
        [
            ("LDBL_MANT_DIG", "113"),
            ("LDBL_DIG", "33"),
            ("LDBL_MIN_EXP", "(-16381)"),
            ("LDBL_MAX_EXP", "16384"),
            ("LDBL_MIN", "3.36210314311209350626267781732175260e-4932L"),
            ("LDBL_EPSILON", "1.92592994438723585305597794258492732e-34L"),
        ]
    } else {
        [
            ("LDBL_MANT_DIG", "64"),
            ("LDBL_DIG", "18"),
            ("LDBL_MIN_EXP", "(-16381)"),
            ("LDBL_MAX_EXP", "16384"),
            ("LDBL_MIN", "3.36210314311209350626e-4932L"),
            ("LDBL_EPSILON", "1.08420217248550443401e-19L"),
        ]
    }
}

pub fn include_virtual_compat_header(name: &str, macros: &mut HashMap<String, MacroDef>) -> String {
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
                include_str!("../virtual_headers/stddef.h")
            )
        }
        "stdarg.h" => {
            for (name, params, variadic, body) in [
                (
                    "va_start",
                    vec!["ap"],
                    true,
                    "__builtin_va_start(ap, ## __VA_ARGS__)",
                ),
                ("va_end", vec!["ap"], false, "__builtin_va_end(ap)"),
                ("va_copy", vec!["dst", "src"], false, "((dst) = (src))"),
                ("__va_copy", vec!["dst", "src"], false, "((dst) = (src))"),
                (
                    "va_arg",
                    vec!["ap", "type"],
                    false,
                    "__builtin_va_arg(ap, type)",
                ),
            ] {
                macros.insert(
                    name.to_string(),
                    MacroDef::Function {
                        params: params.into_iter().map(str::to_string).collect(),
                        variadic,
                        body: body.to_string(),
                    },
                );
            }
            if virtual_header_include_once(macros, "stdarg.h") {
                String::new()
            } else {
                include_str!("../virtual_headers/stdarg.h").to_string()
            }
        }
        "immintrin.h" => {
            if virtual_header_include_once(macros, "immintrin.h") {
                String::new()
            } else {
                "typedef long long __m128i __attribute__((__vector_size__(16)));\n\
                 static inline __m128i _mm_abs_epi32(__m128i __x) { return __x; }\n\
                 static inline __m128i _mm_mullo_epi32(__m128i __a, __m128i __b) { return __a * __b; }\n"
                    .to_string()
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
                include_str!("../virtual_headers/stdio.h")
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
                include_str!("../virtual_headers/stdlib.h")
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
                include_str!("../virtual_headers/string.h")
            )
        }
        "strings.h" => {
            if virtual_header_include_once(macros, "strings.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("../virtual_headers/strings.h")
            )
        }
        "ctype.h" => {
            if virtual_header_include_once(macros, "ctype.h") {
                String::new()
            } else {
                include_str!("../virtual_headers/ctype.h").to_string()
            }
        }
        "dirent.h" => {
            if virtual_header_include_once(macros, "dirent.h") {
                String::new()
            } else {
                include_str!("../virtual_headers/dirent.h").to_string()
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
        "memory.h" => include_virtual_compat_header("string.h", macros),
        "malloc.h" => {
            if virtual_header_include_once(macros, "malloc.h") {
                return String::new();
            }
            include_virtual_compat_header("stdlib.h", macros)
        }
        "alloca.h" => {
            if virtual_header_include_once(macros, "alloca.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("../virtual_headers/alloca.h")
            )
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
            include_str!("../virtual_headers/math.h").to_string()
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
                include_str!("../virtual_headers/regex.h")
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
                include_str!("../virtual_headers/glob.h")
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
            include_str!("../virtual_headers/fnmatch.h").to_string()
        }
        "features.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("__GLIBC__", "2"),
                    ("__GLIBC_MINOR__", "39"),
                    ("__GLIBC_USE_ISOC2X", "0"),
                    ("__GLIBC_USE_DEPRECATED_GETS", "0"),
                    ("__GLIBC_USE_DEPRECATED_SCANF", "0"),
                    ("__USE_ISOC95", "1"),
                    ("__USE_ISOC99", "1"),
                    ("__USE_ISOC11", "1"),
                    ("__USE_POSIX", "1"),
                    ("__USE_POSIX2", "1"),
                    ("__USE_POSIX199309", "1"),
                    ("__USE_POSIX199506", "1"),
                    ("__USE_XOPEN2K", "1"),
                    ("__USE_XOPEN2K8", "1"),
                    ("__USE_MISC", "1"),
                    ("__USE_ATFILE", "1"),
                ],
            );
            for (name, params, body) in [
                (
                    "__GNUC_PREREQ",
                    vec!["maj", "min"],
                    "(((__GNUC__ << 16) + __GNUC_MINOR__) >= (((maj) << 16) + (min)))",
                ),
                ("__glibc_clang_prereq", vec!["maj", "min"], "0"),
                ("__GLIBC_USE", vec!["F"], "0"),
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
            if virtual_header_include_once(macros, "features.h") {
                return String::new();
            }
            include_str!("../virtual_headers/features.h").to_string()
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
            include_str!("../virtual_headers/dlfcn.h").to_string()
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
                include_str!("../virtual_headers/syslog.h")
            )
        }
        "utime.h" => {
            if virtual_header_include_once(macros, "utime.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("time.h", macros),
                include_str!("../virtual_headers/utime.h")
            )
        }
        "libgen.h" => {
            if virtual_header_include_once(macros, "libgen.h") {
                return String::new();
            }
            include_str!("../virtual_headers/libgen.h").to_string()
        }
        "getopt.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("no_argument", "0"),
                    ("required_argument", "1"),
                    ("optional_argument", "2"),
                ],
            );
            if virtual_header_include_once(macros, "getopt.h") {
                return String::new();
            }
            include_str!("../virtual_headers/getopt.h").to_string()
        }
        "paths.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("_PATH_BSHELL", "\"/bin/sh\""),
                    ("_PATH_CSHELL", "\"/bin/csh\""),
                    ("_PATH_DEFPATH", "\"/usr/bin:/bin\""),
                    ("_PATH_DEV", "\"/dev/\""),
                    ("_PATH_DEVNULL", "\"/dev/null\""),
                    ("_PATH_STDPATH", "\"/usr/bin:/bin:/usr/sbin:/sbin\""),
                    ("_PATH_TTY", "\"/dev/tty\""),
                    ("_PATH_TMP", "\"/tmp/\""),
                    ("_PATH_VARDB", "\"/var/db/\""),
                    ("_PATH_VI", "\"/usr/bin/vi\""),
                ],
            );
            String::new()
        }
        "sysexits.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("EX_OK", "0"),
                    ("EX__BASE", "64"),
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
                    ("EX__MAX", "78"),
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
            include_str!("../virtual_headers/signal.h").to_string()
        }
        "setjmp.h" => {
            if virtual_header_include_once(macros, "setjmp.h") {
                String::new()
            } else {
                include_str!("../virtual_headers/setjmp.h").to_string()
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
                include_str!("../virtual_headers/locale.h").to_string()
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
                include_str!("../virtual_headers/time.h")
            )
        }
        "sys/time.h" => {
            if virtual_header_include_once(macros, "sys/time.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("../virtual_headers/sys/time.h")
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
                include_str!("../virtual_headers/sys/types.h")
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
            format!("{}{}", types, include_str!("../virtual_headers/sys/stat.h"))
        }
        "sys/cdefs.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("__ptr_t", "void *"),
                    ("__BEGIN_DECLS", ""),
                    ("__END_DECLS", ""),
                    ("__THROW", ""),
                    ("__THROWNL", ""),
                    ("__attribute_malloc__", ""),
                    ("__attribute_pure__", ""),
                    ("__attribute_const__", ""),
                    ("__attribute_used__", ""),
                    ("__attribute_noinline__", ""),
                    ("__attribute_deprecated__", ""),
                    ("__wur", ""),
                    ("__always_inline", "inline"),
                    ("__extern_inline", "extern inline"),
                    ("__fortify_function", "extern inline"),
                    ("__restrict_arr", "__restrict"),
                ],
            );
            for (name, params, body) in [
                ("__P", vec!["args"], "args"),
                ("__PMT", vec!["args"], "args"),
                ("__CONCAT", vec!["x", "y"], "x ## y"),
                ("__STRING", vec!["x"], "#x"),
                ("__NTH", vec!["fct"], "fct"),
                ("__NTHNL", vec!["fct"], "fct"),
                ("__attribute__", vec!["attrs"], ""),
                ("__attribute_alloc_size__", vec!["params"], ""),
                ("__attribute_alloc_align__", vec!["param"], ""),
                ("__attribute_format_arg__", vec!["x"], ""),
                ("__attribute_format_strfmon__", vec!["a", "b"], ""),
                ("__nonnull", vec!["params"], ""),
                ("__glibc_unlikely", vec!["cond"], "(cond)"),
                ("__glibc_likely", vec!["cond"], "(cond)"),
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
            if virtual_header_include_once(macros, "sys/cdefs.h") {
                return String::new();
            }
            include_str!("../virtual_headers/sys/cdefs.h").to_string()
        }
        "sys/errno.h" => include_virtual_compat_header("errno.h", macros),
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
                include_str!("../virtual_headers/fcntl.h")
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
                include_str!("../virtual_headers/poll.h")
            )
        }
        "sys/poll.h" => include_virtual_compat_header("poll.h", macros),
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
                include_str!("../virtual_headers/unistd.h")
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
                include_str!("../virtual_headers/sys/select.h")
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
                include_str!("../virtual_headers/sys/socket.h")
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
                include_str!("../virtual_headers/sys/un.h")
            )
        }
        "sys/uio.h" => {
            if virtual_header_include_once(macros, "sys/uio.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("../virtual_headers/sys/uio.h")
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
            include_str!("../virtual_headers/sys/ioctl.h").to_string()
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
            include_str!("../virtual_headers/sys/file.h").to_string()
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
                include_str!("../virtual_headers/sys/mman.h")
            )
        }
        "sys/param.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("MAXPATHLEN", "1024"),
                    ("MAXHOSTNAMELEN", "256"),
                    ("MAXSYMLINKS", "32"),
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
                ("powerof2", vec!["x"], "(((x) & ((x) - 1)) == 0)"),
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
            include_str!("../virtual_headers/sys/param.h").to_string()
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
                include_str!("../virtual_headers/sys/resource.h")
            )
        }
        "sys/utsname.h" => {
            if virtual_header_include_once(macros, "sys/utsname.h") {
                return String::new();
            }
            include_str!("../virtual_headers/sys/utsname.h").to_string()
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
                include_str!("../virtual_headers/sys/wait.h")
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
            include_virtual_compat_header("sys/types.h", macros)
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
                include_str!("../virtual_headers/netinet/in.h")
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
                include_str!("../virtual_headers/netinet/tcp.h")
            )
        }
        "netinet/ip.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("IPVERSION", "4"),
                    ("IP_MAXPACKET", "65535"),
                    ("IPTOS_LOWDELAY", "0x10"),
                    ("IPTOS_THROUGHPUT", "0x08"),
                    ("IPTOS_RELIABILITY", "0x04"),
                ],
            );
            if virtual_header_include_once(macros, "netinet/ip.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("netinet/in.h", macros),
                include_str!("../virtual_headers/netinet/ip.h")
            )
        }
        "netinet/udp.h" => {
            if virtual_header_include_once(macros, "netinet/udp.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("netinet/in.h", macros),
                include_str!("../virtual_headers/netinet/udp.h")
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
            include_str!("../virtual_headers/net/if.h").to_string()
        }
        "ifaddrs.h" => {
            if virtual_header_include_once(macros, "ifaddrs.h") {
                return String::new();
            }
            format!(
                "{}{}{}",
                include_virtual_compat_header("sys/socket.h", macros),
                include_virtual_compat_header("net/if.h", macros),
                include_str!("../virtual_headers/ifaddrs.h")
            )
        }
        "arpa/inet.h" => {
            if virtual_header_include_once(macros, "arpa/inet.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("netinet/in.h", macros),
                include_str!("../virtual_headers/arpa/inet.h")
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
                include_str!("../virtual_headers/netdb.h")
            )
        }
        "resolv.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("NS_PACKETSZ", "512"),
                    ("NS_MAXDNAME", "1025"),
                    ("RES_INIT", "0x00000001"),
                    ("RES_RECURSE", "0x00000040"),
                    ("RES_DEFNAMES", "0x00000080"),
                    ("RES_DNSRCH", "0x00000200"),
                ],
            );
            if virtual_header_include_once(macros, "resolv.h") {
                return String::new();
            }
            include_str!("../virtual_headers/resolv.h").to_string()
        }
        "linux/limits.h" => {
            define_virtual_object_macros(
                macros,
                &[
                    ("ARG_MAX", "131072"),
                    ("LINK_MAX", "127"),
                    ("MAX_CANON", "255"),
                    ("MAX_INPUT", "255"),
                    ("NAME_MAX", "255"),
                    ("PATH_MAX", "4096"),
                    ("PIPE_BUF", "4096"),
                ],
            );
            if virtual_header_include_once(macros, "linux/limits.h") {
                return String::new();
            }
            include_str!("../virtual_headers/linux/limits.h").to_string()
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
                include_str!("../virtual_headers/pthread.h").to_string()
            }
        }
        "grp.h" => {
            if virtual_header_include_once(macros, "grp.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("../virtual_headers/grp.h")
            )
        }
        "pwd.h" => {
            if virtual_header_include_once(macros, "pwd.h") {
                return String::new();
            }
            format!(
                "{}{}",
                include_virtual_compat_header("sys/types.h", macros),
                include_str!("../virtual_headers/pwd.h")
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
            include_str!("../virtual_headers/termios.h").to_string()
        }
        "wchar.h" => {
            virtual_null_macro(macros);
            if virtual_header_include_once(macros, "wchar.h") {
                return String::new();
            }
            format!(
                "{}{}",
                virtual_size_t_typedef(macros),
                include_str!("../virtual_headers/wchar.h")
            )
        }
        "wctype.h" => {
            let wchar = include_virtual_compat_header("wchar.h", macros);
            if virtual_header_include_once(macros, "wctype.h") {
                return String::new();
            }
            format!("{}{}", wchar, include_str!("../virtual_headers/wctype.h"))
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
            include_str!("../virtual_headers/stdatomic.h").to_string()
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
            include_str!("../virtual_headers/stdint.h").to_string()
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
                ("FLT_DIG", "6"),
                ("DBL_DIG", "15"),
                ("FLT_MIN_EXP", "(-125)"),
                ("DBL_MIN_EXP", "(-1021)"),
                ("FLT_MAX_EXP", "128"),
                ("DBL_MAX_EXP", "1024"),
                ("FLT_MIN", "1.17549435e-38F"),
                ("DBL_MIN", "2.2250738585072014e-308"),
                ("FLT_MAX", "0x1.fffffep+127F"),
                ("DBL_MAX", "0x1.fffffffffffffp+1023"),
                ("FLT_EPSILON", "1.19209290e-7F"),
                ("DBL_EPSILON", "2.2204460492503131e-16"),
            ] {
                macros.insert(name.to_string(), MacroDef::Object(value.to_string()));
            }
            for (name, value) in virtual_long_double_limits(macros) {
                macros.insert(name.to_string(), MacroDef::Object(value.to_string()));
            }
            macros.insert(
                "LDBL_MAX".to_string(),
                MacroDef::Object(virtual_long_double_max_macro(macros).to_string()),
            );
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
        "stdckdint.h" => {
            for (name, builtin) in [
                ("ckd_add", "__builtin_add_overflow"),
                ("ckd_sub", "__builtin_sub_overflow"),
                ("ckd_mul", "__builtin_mul_overflow"),
            ] {
                define_builtin_function_macro(
                    macros,
                    name,
                    &["result", "a", "b"],
                    &format!("{builtin}((a), (b), (result))"),
                );
            }
            if virtual_header_include_once(macros, "stdckdint.h") {
                return String::new();
            }
            include_str!("../virtual_headers/stdckdint.h").to_string()
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

pub fn generated_header_dependency(spec: &IncludeSpec) -> PathBuf {
    match spec {
        IncludeSpec::Quoted(name) | IncludeSpec::Angled(name) => PathBuf::from(name),
    }
}

pub fn is_system_dependency(path: &Path, include_paths: &IncludePaths) -> bool {
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

pub struct ConditionalFrame {
    parent_active: bool,
    condition_active: bool,
    branch_taken: bool,
    saw_else: bool,
}

pub struct InternalPreprocessContext<'a> {
    include_stack: &'a mut Vec<PathBuf>,
    once_files: &'a mut HashSet<PathBuf>,
    system_header_files: &'a mut HashSet<PathBuf>,
    poisoned_identifiers: &'a mut HashSet<String>,
    saved_macros: &'a mut HashMap<String, Vec<Option<MacroDef>>>,
    token_macro_cache: preprocess::macro_expand::MacroTable,
    token_macro_cache_dirty: bool,
    pragma_pack_stack: &'a mut Vec<Option<usize>>,
    pragma_pack_alignment: &'a mut Option<usize>,
    include_paths: &'a IncludePaths,
    dependencies: &'a mut Vec<PathBuf>,
    user_dependencies_only: bool,
    missing_headers_generated: bool,
    suppress_preprocessed_output: bool,
    trace_includes: bool,
    line_markers: bool,
    stats: Option<&'a mut InternalCppStats>,
}

impl InternalPreprocessContext<'_> {
    fn invalidate_token_macro_cache(&mut self) {
        self.token_macro_cache_dirty = true;
    }

    fn token_macro_table(
        &mut self,
        macros: &HashMap<String, MacroDef>,
    ) -> Result<&preprocess::macro_expand::MacroTable, String> {
        if self.token_macro_cache_dirty {
            if let Some(stats) = self.stats.as_mut() {
                stats.token_macro_cache_rebuilds += 1;
            }
            self.token_macro_cache = token_macro_table(macros)?;
            self.token_macro_cache_dirty = false;
        } else if let Some(stats) = self.stats.as_mut() {
            stats.token_macro_cache_hits += 1;
        }
        Ok(&self.token_macro_cache)
    }

    fn stats_mut(&mut self) -> Option<&mut InternalCppStats> {
        self.stats.as_deref_mut()
    }
}

#[derive(Default)]
pub struct InternalCppStats {
    files: usize,
    physical_lines: usize,
    bytes: usize,
    directives: usize,
    includes: usize,
    virtual_includes: usize,
    macro_defines: usize,
    macro_undefs: usize,
    source_blocks: usize,
    source_lines: usize,
    token_macro_cache_rebuilds: usize,
    token_macro_cache_hits: usize,
    max_include_depth: usize,
}

impl InternalCppStats {
    fn enabled_from_env() -> bool {
        std::env::var_os("RNQCC_INTERNAL_CPP_STATS").is_some()
    }

    fn record_file(&mut self, bytes: usize, lines: usize, include_depth: usize) {
        self.files += 1;
        self.bytes += bytes;
        self.physical_lines += lines;
        self.max_include_depth = self.max_include_depth.max(include_depth);
    }

    fn report(&self) {
        eprintln!(
            concat!(
                "rnqcc internal-cpp stats: ",
                "files={}, bytes={}, physical_lines={}, directives={}, includes={}, ",
                "virtual_includes={}, defines={}, undefs={}, source_blocks={}, ",
                "source_lines={}, token_cache_rebuilds={}, token_cache_hits={}, ",
                "max_include_depth={}"
            ),
            self.files,
            self.bytes,
            self.physical_lines,
            self.directives,
            self.includes,
            self.virtual_includes,
            self.macro_defines,
            self.macro_undefs,
            self.source_blocks,
            self.source_lines,
            self.token_macro_cache_rebuilds,
            self.token_macro_cache_hits,
            self.max_include_depth,
        );
    }
}

pub fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn trace_include(path: &Path, context: &InternalPreprocessContext<'_>) {
    if context.trace_includes {
        let depth = context.include_stack.len().max(1);
        eprintln!("{} {}", ".".repeat(depth), path.display());
    }
}

pub fn inactive_recursive_include_guard(source: &str, macros: &HashMap<String, MacroDef>) -> bool {
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

pub fn is_marked_system_header(path: &Path, context: &InternalPreprocessContext<'_>) -> bool {
    context.system_header_files.contains(&canonical_path(path))
}

pub fn should_record_dependency(path: &Path, context: &InternalPreprocessContext<'_>) -> bool {
    !context.user_dependencies_only
        || (!is_system_dependency(path, context.include_paths)
            && !is_marked_system_header(path, context))
}

pub fn record_dependency(path: &Path, context: &mut InternalPreprocessContext<'_>) {
    if should_record_dependency(path, context) {
        context.dependencies.push(path.to_path_buf());
    }
}

pub fn unrecord_dependency(path: &Path, context: &mut InternalPreprocessContext<'_>) {
    if context.user_dependencies_only && is_marked_system_header(path, context) {
        let canonical = canonical_path(path);
        context
            .dependencies
            .retain(|dep| canonical_path(dep) != canonical);
    }
}

pub fn poison_identifiers_from_pragma(pragma: &str, context: &mut InternalPreprocessContext<'_>) {
    let Some(rest) = pragma.strip_prefix("GCC poison") else {
        return;
    };
    for name in rest.split_whitespace() {
        if name.chars().next().is_some_and(is_ident_start) && name.chars().all(is_ident_continue) {
            context.poisoned_identifiers.insert(name.to_string());
        }
    }
}

pub fn parse_pragma_macro_name(pragma: &str, prefix: &str) -> Option<String> {
    let rest = pragma.trim().strip_prefix(prefix)?.trim();
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?.trim();
    let name = inner.strip_prefix('"')?.strip_suffix('"')?;
    if !name.chars().next().is_some_and(is_ident_start) || !name.chars().all(is_ident_continue) {
        return None;
    }
    Some(name.to_string())
}

pub enum PragmaPackAction {
    Set(Option<usize>),
    Push(Option<usize>),
    Pop,
}

pub fn parse_pack_alignment(text: &str) -> Option<usize> {
    let value = text.trim().parse::<usize>().ok()?;
    if matches!(value, 1 | 2 | 4 | 8 | 16) {
        Some(value)
    } else {
        None
    }
}

pub fn parse_pragma_pack(pragma: &str) -> Option<PragmaPackAction> {
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

pub fn handle_internal_pragma(
    pragma: &str,
    canonical: &Path,
    macros: &mut HashMap<String, MacroDef>,
    context: &mut InternalPreprocessContext<'_>,
) -> Result<(), String> {
    if pragma == "once" {
        context.once_files.insert(canonical.to_path_buf());
    } else if pragma == "GCC system_header" || pragma == "clang system_header" {
        context.system_header_files.insert(canonical.to_path_buf());
    } else if pragma.trim().starts_with("push_macro") {
        let Some(name) = parse_pragma_macro_name(pragma, "push_macro") else {
            return Err(format!("malformed #pragma push_macro: {}", pragma));
        };
        context
            .saved_macros
            .entry(name.clone())
            .or_default()
            .push(macros.get(&name).cloned());
    } else if pragma.trim().starts_with("pop_macro") {
        let Some(name) = parse_pragma_macro_name(pragma, "pop_macro") else {
            return Err(format!("malformed #pragma pop_macro: {}", pragma));
        };
        if let Some(saved) = context.saved_macros.get_mut(&name).and_then(Vec::pop) {
            match saved {
                Some(def) => {
                    macros.insert(name, def);
                }
                None => {
                    macros.remove(&name);
                }
            }
            context.invalidate_token_macro_cache();
        }
    } else if pragma.trim().starts_with("pack") {
        let Some(action) = parse_pragma_pack(pragma) else {
            return Err(format!("malformed #pragma pack: {}", pragma));
        };
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
    Ok(())
}

pub fn inject_pack_attributes(text: &str, alignment: usize) -> String {
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
        if is_ident_start(ch) {
            let start = index;
            index += 1;
            while index < chars.len() && is_ident_continue(chars[index]) {
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

pub fn check_poisoned_tokens(
    tokens: &[preprocess::token::PpToken],
    context: &InternalPreprocessContext<'_>,
) -> Result<(), String> {
    if context.poisoned_identifiers.is_empty() {
        return Ok(());
    }
    for token in tokens {
        if let preprocess::token::PpTokenKind::Ident(name) = &token.kind {
            if context.poisoned_identifiers.contains(name) {
                return Err(format!("attempt to use poisoned identifier {}", name));
            }
        }
    }
    Ok(())
}

pub fn check_poisoned_line(
    line: &str,
    context: &InternalPreprocessContext<'_>,
) -> Result<(), String> {
    if context.poisoned_identifiers.is_empty() {
        return Ok(());
    }
    let tokens = preprocess::lexer::lex(line)?;
    check_poisoned_tokens(&tokens, context)
}

pub fn pp_location(file: &str, line: usize, message: impl AsRef<str>) -> String {
    format!("{}:{}: {}", file, line, message.as_ref())
}

pub fn pp_error_at(file: &str, line: usize) -> impl Fn(String) -> String + '_ {
    move |message| pp_location(file, line, message)
}

pub fn pp_include_error(file: &str, line: usize, message: String) -> String {
    if message.starts_with("recursive include of ") {
        pp_location(file, line, message)
    } else {
        message
    }
}

pub fn recursive_include_error(src: &Path, canonical: &Path, include_stack: &[PathBuf]) -> String {
    let mut chain: Vec<String> = include_stack
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    chain.push(canonical.display().to_string());
    format!(
        "recursive include of {} (include chain: {})",
        src.display(),
        chain.join(" -> ")
    )
}

pub fn include_depth_error(src: &Path, include_stack: &[PathBuf]) -> String {
    let mut chain: Vec<String> = include_stack
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    chain.push(src.display().to_string());
    format!(
        "include nesting too deep (limit {}): {}",
        MAX_INCLUDE_DEPTH,
        chain.join(" -> ")
    )
}

pub fn push_line_marker(out: &mut String, line: usize, file: &str) {
    out.push_str(&format!("# {} \"{}\"\n", line, escape_c_string(file)));
}

pub fn conditionals_active(stack: &[ConditionalFrame]) -> bool {
    stack
        .last()
        .map(|frame| frame.parent_active && frame.condition_active)
        .unwrap_or(true)
}

pub struct IfExprParser<'a> {
    chars: Vec<char>,
    pos: usize,
    macros: &'a HashMap<String, MacroDef>,
}

#[derive(Clone, Copy)]
pub struct IfValue {
    value: u128,
    unsigned: bool,
}

impl IfValue {
    fn signed(value: i128) -> Self {
        Self {
            value: value as u64 as u128,
            unsigned: false,
        }
    }

    fn unsigned(value: u128) -> Self {
        Self {
            value: value as u64 as u128,
            unsigned: true,
        }
    }

    fn zero() -> Self {
        Self::signed(0)
    }

    fn truth(self) -> bool {
        self.value != 0
    }

    fn signed_value(self) -> i128 {
        (self.value as u64 as i64) as i128
    }

    fn truth_value(value: bool) -> Self {
        Self::signed(value as i128)
    }

    fn common_unsigned(left: Self, right: Self) -> bool {
        left.unsigned || right.unsigned
    }
}

pub struct IfEvalContext<'a> {
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

    fn parse(mut self) -> Result<IfValue, String> {
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

    fn integer_suffix_unsigned(suffix: &str, literal: &str) -> Result<bool, String> {
        if suffix.is_empty() {
            return Ok(false);
        }
        let suffix = suffix.to_ascii_lowercase();
        if suffix.ends_with("wb") {
            return match suffix.strip_suffix("wb").unwrap_or_default() {
                "" => Ok(false),
                "u" => Ok(true),
                _ => Err(format!(
                    "invalid integer literal suffix in #if expression near '{}'",
                    literal
                )),
            };
        }
        if suffix.contains('w') || suffix.contains('b') {
            return Err(format!(
                "invalid integer literal suffix in #if expression near '{}'",
                literal
            ));
        }
        if suffix.contains('z') {
            return match suffix.as_str() {
                "z" => Ok(false),
                "uz" | "zu" => Ok(true),
                _ => Err(format!(
                    "invalid integer literal suffix in #if expression near '{}'",
                    literal
                )),
            };
        }

        let unsigned_count = suffix.chars().filter(|ch| *ch == 'u').count();
        if unsigned_count > 1 {
            return Err(format!(
                "invalid integer literal suffix in #if expression near '{}'",
                literal
            ));
        }
        let without_u = suffix.replace('u', "");
        if matches!(without_u.as_str(), "" | "l" | "ll") {
            Ok(unsigned_count == 1)
        } else {
            Err(format!(
                "invalid integer literal suffix in #if expression near '{}'",
                literal
            ))
        }
    }

    fn integer_digits(&mut self, valid_digit: impl Fn(char) -> bool) -> Result<String, String> {
        let mut digits = String::new();
        let mut previous_was_digit = false;
        while self.pos < self.chars.len() {
            let ch = self.chars[self.pos];
            if valid_digit(ch) {
                digits.push(ch);
                self.pos += 1;
                previous_was_digit = true;
            } else if ch == '\'' {
                if !previous_was_digit
                    || self.pos + 1 >= self.chars.len()
                    || !valid_digit(self.chars[self.pos + 1])
                {
                    return Err(format!(
                        "invalid digit separator in #if expression near '{}'",
                        self.chars[self.pos..].iter().collect::<String>()
                    ));
                }
                self.pos += 1;
                previous_was_digit = false;
            } else {
                break;
            }
        }
        Ok(digits)
    }

    fn number(&mut self) -> Result<Option<IfValue>, String> {
        self.skip_ws();
        let start = self.pos;
        if self.pos >= self.chars.len() || !self.chars[self.pos].is_ascii_digit() {
            return Ok(None);
        }

        let mut base = 10;
        let digits;
        if self.chars[self.pos] == '0'
            && self.pos + 1 < self.chars.len()
            && matches!(self.chars[self.pos + 1], 'x' | 'X')
        {
            base = 16;
            self.pos += 2;
            digits = self.integer_digits(|ch| ch.is_ascii_hexdigit())?;
            if digits.is_empty() {
                return Err("expected hexadecimal digits in #if expression".to_string());
            }
        } else if self.chars[self.pos] == '0'
            && self.pos + 1 < self.chars.len()
            && matches!(self.chars[self.pos + 1], 'b' | 'B')
        {
            base = 2;
            self.pos += 2;
            digits = self.integer_digits(|ch| matches!(ch, '0' | '1'))?;
            if digits.is_empty() {
                return Err("expected binary digits in #if expression".to_string());
            }
            if self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                return Err("invalid binary digit in #if expression".to_string());
            }
        } else if self.chars[self.pos] == '0' {
            base = 8;
            digits = self.integer_digits(|ch| matches!(ch, '0'..='7'))?;
            if self.pos < self.chars.len() && self.chars[self.pos].is_ascii_digit() {
                return Err("invalid octal digit in #if expression".to_string());
            }
        } else {
            digits = self.integer_digits(|ch| ch.is_ascii_digit())?;
        }

        let suffix_start = self.pos;
        while self.pos < self.chars.len() && self.chars[self.pos].is_ascii_alphabetic() {
            self.pos += 1;
        }
        let suffix = self.chars[suffix_start..self.pos]
            .iter()
            .collect::<String>();

        if self.pos < self.chars.len()
            && (self.chars[self.pos].is_ascii_digit() || self.chars[self.pos] == '_')
        {
            return Err(format!(
                "invalid integer literal in #if expression near '{}'",
                self.chars[start..=self.pos].iter().collect::<String>()
            ));
        }

        let value = u128::from_str_radix(&digits, base)
            .map_err(|_| format!("invalid integer literal in #if expression: {}", digits))?;
        let literal = self.chars[start..self.pos].iter().collect::<String>();
        let unsigned =
            Self::integer_suffix_unsigned(&suffix, &literal)? || value > i64::MAX as u128;
        Ok(Some(if unsigned {
            IfValue::unsigned(value)
        } else {
            IfValue::signed(value as i128)
        }))
    }

    fn char_constant(&mut self) -> Result<Option<IfValue>, String> {
        self.skip_ws();
        if self.pos >= self.chars.len() {
            return Ok(None);
        }
        if self.chars[self.pos] == '\'' {
            self.pos += 1;
        } else if matches!(self.chars[self.pos], 'L' | 'u' | 'U')
            && self.pos + 1 < self.chars.len()
            && self.chars[self.pos + 1] == '\''
        {
            self.pos += 2;
        } else if self.chars[self.pos] == 'u'
            && self.pos + 2 < self.chars.len()
            && self.chars[self.pos + 1] == '8'
            && self.chars[self.pos + 2] == '\''
        {
            self.pos += 3;
        } else {
            return Ok(None);
        }
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
                return Ok(Some(IfValue::signed(value)));
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

    fn parse_primary(&mut self, eval: bool) -> Result<IfValue, String> {
        if self.eat("(") {
            let value = self.parse_conditional(eval)?;
            if !self.eat(")") {
                return Err("missing ')' in #if expression".to_string());
            }
            return Ok(value);
        }
        if self.eat("#") {
            let Some(_predicate) = self.ident() else {
                return Err("expected assertion predicate after # in #if expression".to_string());
            };
            if self.eat("(") {
                let mut depth = 1usize;
                while self.pos < self.chars.len() && depth > 0 {
                    match self.chars[self.pos] {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    self.pos += 1;
                }
                if depth != 0 {
                    return Err("missing ')' in #if assertion predicate".to_string());
                }
            }
            return Ok(IfValue::zero());
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
                    return Ok(IfValue::truth_value(
                        eval && self.macros.contains_key(&name),
                    ));
                }
                let Some(name) = self.ident() else {
                    return Err("expected macro name after defined".to_string());
                };
                return Ok(IfValue::truth_value(
                    eval && self.macros.contains_key(&name),
                ));
            }
            return Ok(IfValue::zero());
        }
        Err("expected value in #if expression".to_string())
    }

    fn parse_unary(&mut self, eval: bool) -> Result<IfValue, String> {
        if self.eat("!") {
            let value = self.parse_unary(eval)?;
            Ok(IfValue::truth_value(eval && !value.truth()))
        } else if self.eat("~") {
            let value = self.parse_unary(eval)?;
            Ok(if eval {
                IfValue {
                    value: (!value.value) as u64 as u128,
                    unsigned: value.unsigned,
                }
            } else {
                IfValue::zero()
            })
        } else if self.eat("-") {
            let value = self.parse_unary(eval)?;
            Ok(if eval {
                IfValue {
                    value: value.value.wrapping_neg() as u64 as u128,
                    unsigned: value.unsigned,
                }
            } else {
                IfValue::zero()
            })
        } else if self.eat("+") {
            self.parse_unary(eval)
        } else {
            self.parse_primary(eval)
        }
    }

    fn parse_mul(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_unary(eval)?;
        loop {
            if self.eat("*") {
                let right = self.parse_unary(eval)?;
                if eval {
                    left = IfValue {
                        value: left.value.wrapping_mul(right.value) as u64 as u128,
                        unsigned: IfValue::common_unsigned(left, right),
                    };
                }
            } else if self.eat("/") {
                let right = self.parse_unary(eval)?;
                if eval && !right.truth() {
                    return Err("division by zero in #if expression".to_string());
                }
                if eval {
                    let unsigned = IfValue::common_unsigned(left, right);
                    left = if unsigned {
                        IfValue::unsigned(left.value / right.value)
                    } else {
                        IfValue::signed(
                            left.signed_value()
                                .checked_div(right.signed_value())
                                .ok_or_else(|| "overflow in #if division".to_string())?,
                        )
                    };
                }
            } else if self.eat("%") {
                let right = self.parse_unary(eval)?;
                if eval && !right.truth() {
                    return Err("division by zero in #if expression".to_string());
                }
                if eval {
                    let unsigned = IfValue::common_unsigned(left, right);
                    left = if unsigned {
                        IfValue::unsigned(left.value % right.value)
                    } else {
                        IfValue::signed(
                            left.signed_value()
                                .checked_rem(right.signed_value())
                                .ok_or_else(|| "overflow in #if remainder".to_string())?,
                        )
                    };
                }
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_add(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_mul(eval)?;
        loop {
            if self.eat("+") {
                let right = self.parse_mul(eval)?;
                if eval {
                    left = IfValue {
                        value: left.value.wrapping_add(right.value) as u64 as u128,
                        unsigned: IfValue::common_unsigned(left, right),
                    };
                }
            } else if self.eat("-") {
                let right = self.parse_mul(eval)?;
                if eval {
                    left = IfValue {
                        value: left.value.wrapping_sub(right.value) as u64 as u128,
                        unsigned: IfValue::common_unsigned(left, right),
                    };
                }
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_shift(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_add(eval)?;
        loop {
            if self.eat("<<") {
                let right = self.parse_add(eval)?;
                if eval && !right.unsigned && right.signed_value() < 0 {
                    return Err("negative shift count in #if expression".to_string());
                }
                if eval {
                    left.value = left.value.wrapping_shl(right.value as u32) as u64 as u128;
                }
            } else if self.eat(">>") {
                let right = self.parse_add(eval)?;
                if eval && !right.unsigned && right.signed_value() < 0 {
                    return Err("negative shift count in #if expression".to_string());
                }
                if eval {
                    left.value = if left.unsigned {
                        left.value.wrapping_shr(right.value as u32) as u64 as u128
                    } else {
                        left.signed_value().wrapping_shr(right.value as u32) as u64 as u128
                    };
                }
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_relational(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_shift(eval)?;
        loop {
            if self.eat("<=") {
                let right = self.parse_shift(eval)?;
                left = IfValue::truth_value(eval && Self::compare_le(left, right));
            } else if self.eat(">=") {
                let right = self.parse_shift(eval)?;
                left = IfValue::truth_value(eval && Self::compare_ge(left, right));
            } else if self.eat("<") {
                let right = self.parse_shift(eval)?;
                left = IfValue::truth_value(eval && Self::compare_lt(left, right));
            } else if self.eat(">") {
                let right = self.parse_shift(eval)?;
                left = IfValue::truth_value(eval && Self::compare_gt(left, right));
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_equality(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_relational(eval)?;
        loop {
            if self.eat("==") {
                let right = self.parse_relational(eval)?;
                left = IfValue::truth_value(eval && left.value == right.value);
            } else if self.eat("!=") {
                let right = self.parse_relational(eval)?;
                left = IfValue::truth_value(eval && left.value != right.value);
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_bit_and(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_equality(eval)?;
        while !self.starts_with("&&") && self.eat("&") {
            let right = self.parse_equality(eval)?;
            if eval {
                left = IfValue {
                    value: left.value & right.value,
                    unsigned: IfValue::common_unsigned(left, right),
                };
            }
        }
        Ok(left)
    }

    fn parse_bit_xor(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_bit_and(eval)?;
        while self.eat("^") {
            let right = self.parse_bit_and(eval)?;
            if eval {
                left = IfValue {
                    value: left.value ^ right.value,
                    unsigned: IfValue::common_unsigned(left, right),
                };
            }
        }
        Ok(left)
    }

    fn parse_bit_or(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_bit_xor(eval)?;
        while !self.starts_with("||") && self.eat("|") {
            let right = self.parse_bit_xor(eval)?;
            if eval {
                left = IfValue {
                    value: left.value | right.value,
                    unsigned: IfValue::common_unsigned(left, right),
                };
            }
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_bit_or(eval)?;
        while self.eat("&&") {
            let right_eval = eval && left.truth();
            let right = self.parse_bit_or(right_eval)?;
            left = IfValue::truth_value(right_eval && right.truth());
        }
        Ok(left)
    }

    fn parse_logical_or(&mut self, eval: bool) -> Result<IfValue, String> {
        let mut left = self.parse_logical_and(eval)?;
        while self.eat("||") {
            let right_eval = eval && !left.truth();
            let right = self.parse_logical_and(right_eval)?;
            left = IfValue::truth_value(eval && (left.truth() || right.truth()));
        }
        Ok(left)
    }

    fn parse_conditional(&mut self, eval: bool) -> Result<IfValue, String> {
        let condition = self.parse_logical_or(eval)?;
        if self.eat("?") {
            let true_eval = eval && condition.truth();
            let when_true = self.parse_conditional(true_eval)?;
            if !self.eat(":") {
                return Err("missing ':' in #if conditional expression".to_string());
            }
            let false_eval = eval && !condition.truth();
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

    fn compare_lt(left: IfValue, right: IfValue) -> bool {
        if IfValue::common_unsigned(left, right) {
            left.value < right.value
        } else {
            left.signed_value() < right.signed_value()
        }
    }

    fn compare_le(left: IfValue, right: IfValue) -> bool {
        Self::compare_lt(left, right) || left.value == right.value
    }

    fn compare_gt(left: IfValue, right: IfValue) -> bool {
        !Self::compare_le(left, right)
    }

    fn compare_ge(left: IfValue, right: IfValue) -> bool {
        !Self::compare_lt(left, right)
    }
}

pub fn eval_internal_if(
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
        .map(IfValue::truth)
        .map_err(|err| format!("unsupported #if expression '{}': {}", expr, err))
}

pub struct PendingSource {
    text: String,
    logical_file: String,
    start_line: usize,
}

pub fn flush_pending_source(
    pending: &mut Option<PendingSource>,
    out: &mut String,
    macros: &mut HashMap<String, MacroDef>,
    context: &mut InternalPreprocessContext<'_>,
    canonical: &Path,
    include_level: usize,
    state: &mut PreprocessorState,
) -> Result<(), String> {
    let Some(pending_source) = pending.take() else {
        return Ok(());
    };
    if let Some(stats) = context.stats_mut() {
        stats.source_blocks += 1;
        stats.source_lines += pending_source.text.lines().count();
    }

    check_poisoned_line(&pending_source.text, context)
        .map_err(|err| pp_location(&pending_source.logical_file, pending_source.start_line, err))?;
    if context.suppress_preprocessed_output {
        return Ok(());
    }

    let tokens = preprocess::lexer::lex(&pending_source.text)
        .map_err(|err| pp_location(&pending_source.logical_file, pending_source.start_line, err))?;
    let token_macros = context
        .token_macro_table(macros)
        .map_err(|err| pp_location(&pending_source.logical_file, pending_source.start_line, err))?;
    let mut hooks = LiveMacroExpansionHooks {
        file: &pending_source.logical_file,
        line_number: pending_source.start_line,
        include_level,
        state,
    };
    let expanded_tokens =
        preprocess::macro_expand::expand_macros_with_hooks(&tokens, token_macros, &mut hooks)
            .map_err(|err| {
                pp_location(&pending_source.logical_file, pending_source.start_line, err)
            })?;
    let expanded = preprocess::emit::emit_tokens(&expanded_tokens);
    let (expanded, pragmas) = process_pragma_operators_located(&expanded).map_err(|err| {
        pp_location(
            &pending_source.logical_file,
            pending_source.start_line + err.line_offset,
            err.message,
        )
    })?;
    for pragma in pragmas {
        handle_internal_pragma(pragma.text.trim(), canonical, macros, context).map_err(|err| {
            pp_location(
                &pending_source.logical_file,
                pending_source.start_line + pragma.line_offset,
                err,
            )
        })?;
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

pub fn replace_preprocessor_predicates(
    expr: &str,
    macros: &HashMap<String, MacroDef>,
    state: &mut PreprocessorState,
    context: &IfEvalContext<'_>,
) -> Result<String, String> {
    let tokens = preprocess::lexer::lex(expr)?;
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if let Some(parsed) = preprocess::predicate::parse_predicate_operand(&tokens, index)? {
            match parsed.operand {
                preprocess::predicate::PredicateOperand::Defined { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        macros.contains_key(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasInclude {
                    operand,
                    include_next,
                } => {
                    if matches!(
                        operand,
                        preprocess::directive::IncludeOperand::Literal(
                            preprocess::directive::HeaderName::Angled(ref name)
                        ) if name.is_empty()
                    ) {
                        return Err(format!(
                            "unsupported #if expression '{}': malformed include operand: <>",
                            expr
                        ));
                    }
                    let spec = parse_token_include_operand(
                        &operand,
                        macros,
                        context.file,
                        context.line_number,
                        context.include_level,
                        state,
                    )?;
                    let found = resolve_include_path(
                        &spec,
                        context.base_dir,
                        context.include_paths,
                        include_next,
                    )
                    .is_some();
                    let found = found || virtual_header_is_available(&spec, include_next);
                    out.push(predicate_number_token(&tokens[index], found));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasEmbed { tokens: operand } => {
                    let expanded = expand_preprocessor_tokens(
                        &operand,
                        macros,
                        context.file,
                        context.line_number,
                        context.include_level,
                        state,
                    )?;
                    let (operand, parameters) =
                        preprocess::directive::parse_embed_tokens(&expanded)?;
                    let parameters = match parse_embed_parameters(&parameters, macros) {
                        Ok(parameters) => parameters,
                        // C23 requires __has_embed to report NOT_FOUND when
                        // any implementation-defined parameter is unsupported.
                        Err(error) if error.starts_with("unsupported #embed parameter ") => {
                            out.push(predicate_integer_token(&tokens[index], 0));
                            index = parsed.next_index;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    let spec = parse_token_include_operand(
                        &operand,
                        macros,
                        context.file,
                        context.line_number,
                        context.include_level,
                        state,
                    )?;
                    let result = match resolve_include_path(
                        &spec,
                        context.base_dir,
                        context.include_paths,
                        false,
                    ) {
                        None => 0,
                        Some(path) => {
                            let mut bytes = std::fs::read(&path).map_err(|err| {
                                format!("failed to read embed file {}: {}", path.display(), err)
                            })?;
                            if let Some(limit) = parameters.limit {
                                bytes.truncate(limit);
                            }
                            if bytes.is_empty() {
                                2
                            } else {
                                1
                            }
                        }
                    };
                    out.push(predicate_integer_token(&tokens[index], result));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasBuiltin { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::has_builtin(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasAttribute { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::has_attribute(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasCAttribute { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::has_c_attribute(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasDeclspecAttribute { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::has_declspec_attribute(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasFeature { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::has_feature(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasExtension { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::has_extension(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::HasWarning { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::has_warning(&name),
                    ));
                    index = parsed.next_index;
                }
                preprocess::predicate::PredicateOperand::IsIdentifier { name } => {
                    out.push(predicate_number_token(
                        &tokens[index],
                        preprocess::probes::is_identifier(&name),
                    ));
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

pub fn predicate_number_token(
    token: &preprocess::token::PpToken,
    value: bool,
) -> preprocess::token::PpToken {
    token.clone_with_text(preprocess::token::PpTokenKind::Number(
        if value { "1" } else { "0" }.to_string(),
    ))
}

pub fn predicate_integer_token(
    token: &preprocess::token::PpToken,
    value: u8,
) -> preprocess::token::PpToken {
    token.clone_with_text(preprocess::token::PpTokenKind::Number(value.to_string()))
}

pub fn define_builtin_macro(macros: &mut HashMap<String, MacroDef>, name: &str, value: &str) {
    macros.insert(name.to_string(), MacroDef::Object(value.to_string()));
}

pub fn define_builtin_function_macro(
    macros: &mut HashMap<String, MacroDef>,
    name: &str,
    params: &[&str],
    body: &str,
) {
    macros.insert(
        name.to_string(),
        MacroDef::Function {
            params: params.iter().map(|param| (*param).to_string()).collect(),
            variadic: false,
            body: body.to_string(),
        },
    );
}

pub fn define_empty_function_macro(
    macros: &mut HashMap<String, MacroDef>,
    name: &str,
    variadic: bool,
) {
    macros.insert(
        name.to_string(),
        MacroDef::Function {
            params: Vec::new(),
            variadic,
            body: String::new(),
        },
    );
}

pub fn seed_internal_predefined_macros(macros: &mut HashMap<String, MacroDef>, target: &Target) {
    define_builtin_macro(macros, "__RNQCC__", "1");
    define_builtin_macro(macros, "__STDC__", "1");
    define_builtin_macro(macros, "__STDC_HOSTED__", "0");
    define_builtin_macro(macros, "__STDC_VERSION__", "201112L");
    define_builtin_macro(macros, "__STDC_NO_COMPLEX__", "1");
    define_builtin_macro(macros, "__STDC_NO_THREADS__", "1");
    define_builtin_macro(macros, "__STDC_NO_VLA__", "1");
    define_builtin_macro(macros, "__STDC_EMBED_NOT_FOUND__", "0");
    define_builtin_macro(macros, "__STDC_EMBED_FOUND__", "1");
    define_builtin_macro(macros, "__STDC_EMBED_EMPTY__", "2");
    define_empty_function_macro(macros, "__has_embed", true);
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
    define_builtin_macro(
        macros,
        "__SIZEOF_LONG_DOUBLE__",
        &target.long_double_size().to_string(),
    );
    define_builtin_macro(macros, "__FLT_MAX__", "0x1.fffffep+127F");
    define_builtin_macro(macros, "__DBL_MAX__", "0x1.fffffffffffffp+1023");
    define_builtin_macro(macros, "__LDBL_MAX__", target_long_double_max_macro(target));
    define_builtin_macro(macros, "__FLT_MIN__", "1.17549435e-38F");
    define_builtin_macro(macros, "__DBL_MIN__", "2.2250738585072014e-308");
    define_builtin_macro(macros, "__LDBL_MIN__", target_long_double_min_macro(target));
    define_builtin_macro(macros, "__FLT_EPSILON__", "1.19209290e-7F");
    define_builtin_macro(macros, "__DBL_EPSILON__", "2.2204460492503131e-16");
    define_builtin_macro(
        macros,
        "__LDBL_EPSILON__",
        target_long_double_epsilon_macro(target),
    );
    define_builtin_macro(macros, "__FLT_MANT_DIG__", "24");
    define_builtin_macro(macros, "__DBL_MANT_DIG__", "53");
    define_builtin_macro(
        macros,
        "__LDBL_MANT_DIG__",
        target_long_double_mant_dig_macro(target),
    );
    define_builtin_macro(macros, "__FLT_DIG__", "6");
    define_builtin_macro(macros, "__DBL_DIG__", "15");
    define_builtin_macro(macros, "__LDBL_DIG__", target_long_double_dig_macro(target));
    define_builtin_macro(macros, "__FLT_MIN_EXP__", "(-125)");
    define_builtin_macro(macros, "__DBL_MIN_EXP__", "(-1021)");
    define_builtin_macro(
        macros,
        "__LDBL_MIN_EXP__",
        target_long_double_min_exp_macro(target),
    );
    define_builtin_macro(macros, "__FLT_MAX_EXP__", "128");
    define_builtin_macro(macros, "__DBL_MAX_EXP__", "1024");
    define_builtin_macro(
        macros,
        "__LDBL_MAX_EXP__",
        target_long_double_max_exp_macro(target),
    );
    define_builtin_macro(macros, "__SIZEOF_SIZE_T__", "8");
    define_builtin_macro(macros, "__SIZEOF_PTRDIFF_T__", "8");
    define_builtin_macro(macros, "__SIZEOF_WCHAR_T__", "4");
    define_builtin_macro(macros, "__SIZEOF_WINT_T__", "4");
    define_builtin_macro(macros, "__SIZE_TYPE__", "unsigned long");
    define_builtin_macro(macros, "__WCHAR_TYPE__", "int");
    define_builtin_macro(macros, "__WINT_TYPE__", "unsigned int");
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
    define_builtin_function_macro(macros, "__INT8_C", &["c"], "c");
    define_builtin_function_macro(macros, "__UINT8_C", &["c"], "c");
    define_builtin_function_macro(macros, "__INT16_C", &["c"], "c");
    define_builtin_function_macro(macros, "__UINT16_C", &["c"], "c");
    define_builtin_function_macro(macros, "__INT32_C", &["c"], "c");
    define_builtin_function_macro(macros, "__UINT32_C", &["c"], "c ## U");
    define_builtin_function_macro(macros, "__INT64_C", &["c"], "c ## L");
    define_builtin_function_macro(macros, "__UINT64_C", &["c"], "c ## UL");
    define_builtin_function_macro(macros, "__INTMAX_C", &["c"], "c ## L");
    define_builtin_function_macro(macros, "__UINTMAX_C", &["c"], "c ## UL");
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

pub fn push_existing_include_dir(dirs: &mut Vec<PathBuf>, path: PathBuf) {
    if path.is_dir() && !dirs.iter().any(|existing| existing == &path) {
        dirs.push(path);
    }
}

pub fn linux_gcc_include_triples(target: &Target) -> &'static [&'static str] {
    match target.arch {
        Arch::X86_64 => &["x86_64-linux-gnu", "x86_64-pc-linux-gnu"],
        Arch::AArch64 => &["aarch64-linux-gnu", "aarch64-unknown-linux-gnu"],
    }
}

pub fn push_linux_gcc_include_dirs(dirs: &mut Vec<PathBuf>, target: &Target) {
    for triple in linux_gcc_include_triples(target) {
        let root = PathBuf::from("/usr/lib/gcc").join(triple);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut versions: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        versions.sort();
        versions.reverse();
        for version in versions {
            push_existing_include_dir(dirs, version.join("include"));
        }
    }
}

pub fn default_system_include_dirs(target: &Target) -> Vec<PathBuf> {
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
            push_linux_gcc_include_dirs(&mut dirs, target);
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

pub fn internal_preprocess_source(
    src: &Path,
    macros: &mut HashMap<String, MacroDef>,
    context: &mut InternalPreprocessContext<'_>,
    state: &mut PreprocessorState,
) -> Result<String, String> {
    let canonical = canonical_path(src);
    if context.once_files.contains(&canonical) {
        return Ok(String::new());
    }
    let source_bytes =
        std::fs::read(src).map_err(|err| format!("could not read {}: {}", src.display(), err))?;
    let source_byte_len = source_bytes.len();
    let source = compile::decode_c_source_bytes(&source_bytes);
    let source = strip_comments(&splice_continued_lines(
        &preprocess::lexer::replace_trigraphs(&source),
    ))?;
    if context.include_stack.contains(&canonical) {
        if inactive_recursive_include_guard(&source, macros) {
            return Ok(String::new());
        }
        return Err(recursive_include_error(
            src,
            &canonical,
            context.include_stack,
        ));
    }
    if context.include_stack.len() >= MAX_INCLUDE_DEPTH {
        return Err(include_depth_error(src, context.include_stack));
    }
    context.include_stack.push(canonical.clone());
    let include_depth = context.include_stack.len().saturating_sub(1);
    if let Some(stats) = context.stats_mut() {
        stats.record_file(source_byte_len, source.lines().count(), include_depth);
    }

    let mut out = String::new();
    let base_dir = src.parent().unwrap_or_else(|| Path::new("."));
    let mut conditionals: Vec<ConditionalFrame> = Vec::new();

    let display_file = src.to_string_lossy().into_owned();
    let mut logical_file = display_file.clone();
    let mut next_logical_line = 1usize;
    let include_level = include_depth;
    let mut pending_source: Option<PendingSource> = None;
    if context.line_markers && !context.suppress_preprocessed_output {
        push_line_marker(&mut out, 1, &logical_file);
    }
    for line in source.lines() {
        let current_line_number = next_logical_line;
        next_logical_line = next_logical_line.saturating_add(1);
        let trimmed = line.trim_start();
        if starts_preprocessor_directive(trimmed) {
            if let Some(stats) = context.stats_mut() {
                stats.directives += 1;
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
            if !conditionals_active(&conditionals)
                && !raw_directive_name(trimmed).is_some_and(is_conditional_control_directive)
            {
                continue;
            }
            let tokens = preprocess::lexer::lex(line)
                .map_err(pp_error_at(&logical_file, current_line_number))?;
            if let Some(directive) = preprocess::directive::parse_directive_tokens(&tokens)
                .map_err(pp_error_at(&logical_file, current_line_number))?
            {
                use preprocess::directive::Directive;

                if !matches!(directive, Directive::Pragma { .. }) {
                    check_poisoned_tokens(&tokens, context)
                        .map_err(pp_error_at(&logical_file, current_line_number))?;
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
                            )
                            .map_err(pp_error_at(&logical_file, current_line_number))?
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
                            let message = if negated {
                                "#elifndef without #if".to_string()
                            } else {
                                "#elifdef without #if".to_string()
                            };
                            return Err(pp_location(&logical_file, current_line_number, message));
                        };
                        if frame.saw_else {
                            let message = if negated {
                                "#elifndef after #else".to_string()
                            } else {
                                "#elifdef after #else".to_string()
                            };
                            return Err(pp_location(&logical_file, current_line_number, message));
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
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                "#elif without #if",
                            ));
                        };
                        if frame.saw_else {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                "#elif after #else",
                            ));
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
                            )
                            .map_err(pp_error_at(&logical_file, current_line_number))?;
                            frame.branch_taken = frame.condition_active;
                        }
                        continue;
                    }
                    Directive::Else => {
                        let Some(frame) = conditionals.last_mut() else {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                "#else without #if",
                            ));
                        };
                        if frame.saw_else {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                "duplicate #else",
                            ));
                        }
                        frame.condition_active = frame.parent_active && !frame.branch_taken;
                        frame.branch_taken = true;
                        frame.saw_else = true;
                        continue;
                    }
                    Directive::Endif => {
                        if conditionals.pop().is_none() {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                "#endif without #if",
                            ));
                        }
                        continue;
                    }
                    Directive::Empty => {
                        if let Some((line_number, filename)) = parse_line_marker_tokens(&tokens)
                            .map_err(pp_error_at(&logical_file, current_line_number))?
                        {
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
                        if let Some(stats) = context.stats_mut() {
                            stats.includes += 1;
                        }
                        let spec = parse_token_include_operand(
                            &operand,
                            macros,
                            &logical_file,
                            current_line_number,
                            include_level,
                            state,
                        )
                        .map_err(pp_error_at(&logical_file, current_line_number))?;
                        if let Some(name) = forced_virtual_header_name(&spec, include_next) {
                            emit_virtual_include(
                                &mut out,
                                name,
                                macros,
                                next_logical_line,
                                &logical_file,
                                context,
                            );
                            continue;
                        }
                        let Some(include_path) = resolve_include_path(
                            &spec,
                            base_dir,
                            context.include_paths,
                            include_next,
                        ) else {
                            if !include_next {
                                if let Some(name) = virtual_compat_header_name(&spec) {
                                    emit_virtual_include(
                                        &mut out,
                                        name,
                                        macros,
                                        next_logical_line,
                                        &logical_file,
                                        context,
                                    );
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
                            internal_preprocess_source(&include_path, macros, context, state)
                                .map_err(|err| {
                                    pp_include_error(&logical_file, current_line_number, err)
                                })?;
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
                    Directive::Embed { tokens } => {
                        let expanded = expand_preprocessor_tokens(
                            &tokens,
                            macros,
                            &logical_file,
                            current_line_number,
                            include_level,
                            state,
                        )
                        .map_err(pp_error_at(&logical_file, current_line_number))?;
                        let (operand, parameters) =
                            preprocess::directive::parse_embed_tokens(&expanded)
                                .map_err(pp_error_at(&logical_file, current_line_number))?;
                        let parameters = parse_embed_parameters(&parameters, macros)
                            .map_err(pp_error_at(&logical_file, current_line_number))?;
                        let spec = parse_token_include_operand(
                            &operand,
                            macros,
                            &logical_file,
                            current_line_number,
                            include_level,
                            state,
                        )
                        .map_err(pp_error_at(&logical_file, current_line_number))?;
                        let Some(embed_path) =
                            resolve_include_path(&spec, base_dir, context.include_paths, false)
                        else {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                format!("embed file not found: {}", include_not_found(&spec)),
                            ));
                        };
                        record_dependency(&embed_path, context);
                        let mut bytes = std::fs::read(&embed_path).map_err(|err| {
                            pp_location(
                                &logical_file,
                                current_line_number,
                                format!(
                                    "failed to read embed file {}: {}",
                                    embed_path.display(),
                                    err
                                ),
                            )
                        })?;
                        if let Some(limit) = parameters.limit {
                            bytes.truncate(limit);
                        }
                        if bytes.is_empty() {
                            out.push_str(parameters.if_empty.as_deref().unwrap_or(""));
                            out.push('\n');
                            if context.line_markers && !context.suppress_preprocessed_output {
                                push_line_marker(&mut out, next_logical_line, &logical_file);
                            }
                            continue;
                        }
                        out.push_str(parameters.prefix.as_deref().unwrap_or(""));
                        for (index, byte) in bytes.iter().enumerate() {
                            if index > 0 {
                                out.push_str(", ");
                            }
                            out.push_str(&byte.to_string());
                        }
                        out.push_str(parameters.suffix.as_deref().unwrap_or(""));
                        out.push('\n');
                        if context.line_markers && !context.suppress_preprocessed_output {
                            push_line_marker(&mut out, next_logical_line, &logical_file);
                        }
                        continue;
                    }
                    Directive::Define { name, def } => {
                        if let Some(stats) = context.stats_mut() {
                            stats.macro_defines += 1;
                        }
                        if context.poisoned_identifiers.contains(&name) {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                format!("attempt to use poisoned identifier {}", name),
                            ));
                        }
                        let new_def = token_macro_def_to_string(def);
                        if let Some(existing) = macros.get(&name) {
                            if !macro_defs_equivalent(existing, &new_def)
                                .map_err(pp_error_at(&logical_file, current_line_number))?
                            {
                                return Err(pp_location(
                                    &logical_file,
                                    current_line_number,
                                    format!("macro {} redefined", name),
                                ));
                            }
                        } else {
                            macros.insert(name, new_def);
                            context.invalidate_token_macro_cache();
                        }
                        continue;
                    }
                    Directive::Undef { name } => {
                        if let Some(stats) = context.stats_mut() {
                            stats.macro_undefs += 1;
                        }
                        if context.poisoned_identifiers.contains(name.trim()) {
                            return Err(pp_location(
                                &logical_file,
                                current_line_number,
                                format!("attempt to use poisoned identifier {}", name.trim()),
                            ));
                        }
                        macros.remove(name.trim());
                        context.invalidate_token_macro_cache();
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
                        )
                        .map_err(pp_error_at(&logical_file, current_line_number))?;
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
                        )
                        .map_err(pp_error_at(&logical_file, current_line_number))?;
                        let pragma = preprocess::emit::emit_tokens(&expanded);
                        let pragma = pragma.trim();
                        handle_internal_pragma(pragma, &canonical, macros, context)
                            .map_err(pp_error_at(&logical_file, current_line_number))?;
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
            if line.trim().is_empty() {
                continue;
            }
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
        return Err(pp_location(
            &logical_file,
            next_logical_line.saturating_sub(1).max(1),
            "unterminated conditional directive",
        ));
    }

    context.include_stack.pop();
    Ok(out)
}

pub fn parse_define_arg(arg: &str) -> Result<(String, MacroDef), String> {
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

pub fn apply_cli_macros(
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

pub struct InternalPreprocessInvocation<'a> {
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

pub fn byte_preserving_source_bytes(source: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(source.len());
    for ch in source.chars() {
        if (ch as u32) <= u8::MAX as u32 {
            bytes.push(ch as u8);
        } else {
            let mut encoded = [0u8; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
        }
    }
    bytes
}

pub fn internal_preprocess(
    invocation: InternalPreprocessInvocation<'_>,
) -> Result<Vec<PathBuf>, String> {
    let mut macros = HashMap::new();
    seed_internal_predefined_macros(&mut macros, invocation.target);
    apply_cli_macros(&mut macros, invocation.defines, invocation.undefs)?;
    let mut include_stack = Vec::new();
    let mut once_files = HashSet::new();
    let mut system_header_files = HashSet::new();
    let mut poisoned_identifiers = HashSet::new();
    let mut saved_macros: HashMap<String, Vec<Option<MacroDef>>> = HashMap::new();
    let mut pragma_pack_stack = Vec::new();
    let mut pragma_pack_alignment = None;
    let mut state = PreprocessorState::new(invocation.src.to_string());
    let mut effective_include_paths = invocation.include_paths.clone();
    effective_include_paths.append_system_defaults(invocation.target);
    let mut dependencies = Vec::new();
    let mut stats = InternalCppStats::enabled_from_env().then(InternalCppStats::default);
    let mut context = InternalPreprocessContext {
        include_stack: &mut include_stack,
        once_files: &mut once_files,
        system_header_files: &mut system_header_files,
        poisoned_identifiers: &mut poisoned_identifiers,
        saved_macros: &mut saved_macros,
        token_macro_cache: preprocess::macro_expand::MacroTable::new(),
        token_macro_cache_dirty: true,
        pragma_pack_stack: &mut pragma_pack_stack,
        pragma_pack_alignment: &mut pragma_pack_alignment,
        include_paths: &effective_include_paths,
        dependencies: &mut dependencies,
        user_dependencies_only: invocation.user_dependencies_only,
        missing_headers_generated: invocation.missing_headers_generated,
        suppress_preprocessed_output: invocation.suppress_preprocessed_output,
        trace_includes: invocation.trace_includes,
        line_markers: invocation.line_markers,
        stats: stats.as_mut(),
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
    drop(context);
    std::fs::write(invocation.output, byte_preserving_source_bytes(&output))
        .map_err(|err| format!("could not write {}: {}", invocation.output, err))?;
    if let Some(stats) = stats.as_ref() {
        stats.report();
    }
    Ok(dependencies)
}

pub struct PreprocessedSource {
    pub(crate) path: String,
    pub(crate) generated: bool,
    pub(crate) dependencies: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct DependencyOptions {
    pub(crate) emit: bool,
    pub(crate) side_effect: bool,
    pub(crate) user_only: bool,
    pub(crate) phony_targets: bool,
    pub(crate) missing_headers_generated: bool,
    pub(crate) file: Option<String>,
    pub(crate) targets: Vec<String>,
}

pub struct PreprocessInvocation<'a> {
    pub(crate) src: &'a str,
    pub(crate) index: usize,
    pub(crate) language: Option<&'a str>,
    pub(crate) keep_temps: bool,
    pub(crate) cc: &'a str,
    pub(crate) internal_cpp: bool,
    pub(crate) include_paths: &'a IncludePaths,
    pub(crate) macro_includes: &'a [PathBuf],
    pub(crate) forced_includes: &'a [PathBuf],
    pub(crate) defines: &'a [String],
    pub(crate) undefs: &'a [String],
    pub(crate) target: &'a Target,
    pub(crate) dependency_options: &'a DependencyOptions,
    pub(crate) dump_macros: bool,
    pub(crate) suppress_preprocessed_output: bool,
    pub(crate) trace_includes: bool,
    pub(crate) line_markers: bool,
    pub(crate) sysroot: Option<&'a str>,
    pub(crate) extra_preprocessor_args: &'a [OsString],
}

pub fn preprocess(invocation: PreprocessInvocation<'_>) -> Result<PreprocessedSource, String> {
    validate_input(invocation.src, invocation.language)?;
    let mut stdin_temp_guard = None;
    let actual_src = if invocation.src == "-" {
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|err| format!("could not read stdin: {}", err))?;
        let path = temp_path_for("stdin", invocation.index, "c")?;
        stdin_temp_guard = Some(tempfile::TempFile::new(path.clone()));
        std::fs::write(&path, source)
            .map_err(|err| format!("could not write {}: {}", path, err))?;
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
        temp_path_for(&actual_src, invocation.index, "i")?
    };
    let preprocessing_result: Result<Vec<PathBuf>, String> = if invocation.internal_cpp {
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
        })
    } else {
        let mut args: Vec<OsString> = if Target::host().os == TargetOs::MacOs {
            gcc_arch_args(invocation.target)
                .into_iter()
                .map(OsString::from)
                .collect()
        } else {
            Vec::new()
        };
        args.push(OsString::from("-U__SIZEOF_LONG_DOUBLE__"));
        args.push(OsString::from(format!(
            "-D__SIZEOF_LONG_DOUBLE__={}",
            invocation.target.long_double_size()
        )));
        args.extend(external_cpp_target_macro_args(invocation.target));
        args.extend(
            invocation
                .include_paths
                .quote
                .iter()
                .flat_map(|dir| [OsString::from("-iquote"), dir.as_os_str().to_os_string()]),
        );
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
        run_command(invocation.cc, args).map(|()| Vec::new())
    };
    drop(stdin_temp_guard);
    let dependencies = match preprocessing_result {
        Ok(dependencies) => dependencies,
        Err(error) => {
            if !invocation.keep_temps {
                let _ = std::fs::remove_file(&output);
            }
            return Err(error);
        }
    };
    if !invocation.keep_temps {
        if let Ok(permissions) =
            std::fs::metadata(&actual_src).map(|metadata| metadata.permissions())
        {
            if let Err(err) = std::fs::set_permissions(&output, permissions) {
                let _ = std::fs::remove_file(&output);
                return Err(format!(
                    "could not preserve permissions for {}: {}",
                    output, err
                ));
            }
        }
    }
    Ok(PreprocessedSource {
        path: output,
        generated: true,
        dependencies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_conversion_handles_extreme_epoch_values() {
        for seconds in [i64::MIN, i64::MAX] {
            let (date, time) = format_c_date_time(seconds);
            assert!(!date.is_empty());
            assert_eq!(time.len(), 8);
            assert_eq!(time.as_bytes()[2], b':');
            assert_eq!(time.as_bytes()[5], b':');
        }
    }

    #[test]
    fn quoted_includes_search_including_directory_before_working_directory() -> Result<(), String> {
        let root = std::env::temp_dir().join(format!(
            "rnqcc-include-search-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let source_dir = root.join("source");
        std::fs::create_dir_all(&source_dir).map_err(|err| err.to_string())?;
        std::fs::write(source_dir.join("header.h"), "source\n").map_err(|err| err.to_string())?;

        let cwd_header = PathBuf::from("header.h");
        let cwd_header_was_present = cwd_header.exists();
        if cwd_header_was_present {
            let _ = std::fs::remove_dir_all(&root);
            return Err(
                "test requires no header.h in the repository working directory".to_string(),
            );
        }

        let paths = IncludePaths {
            use_standard_system: false,
            ..IncludePaths::default()
        };
        let resolved = resolve_include_path(
            &IncludeSpec::Quoted("header.h".to_string()),
            &source_dir,
            &paths,
            false,
        );
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(resolved, Some(source_dir.join("header.h")));
        Ok(())
    }

    #[test]
    fn include_next_starts_after_first_duplicate_directory() {
        let first = PathBuf::from("first");
        let second = PathBuf::from("second");
        let paths = IncludePaths {
            quote: vec![first.clone()],
            user: vec![second.clone(), first.clone()],
            use_standard_system: false,
            ..IncludePaths::default()
        };
        assert_eq!(paths.include_next_dirs(&first), vec![second, first]);
    }

    #[test]
    fn include_depth_error_reports_the_full_chain() {
        let stack = vec![PathBuf::from("root.c"), PathBuf::from("one.h")];
        let error = include_depth_error(Path::new("two.h"), &stack);
        assert!(error.contains("include nesting too deep"));
        assert!(error.contains("limit 256"));
        assert!(error.contains("root.c -> one.h -> two.h"));
    }

    #[test]
    fn if_expressions_accept_c23_digit_separators() -> Result<(), String> {
        let macros = HashMap::new();
        assert!(
            IfExprParser::new("1'024 == 0x4'00 && 0b10'10 == 0'12", &macros)
                .parse()?
                .truth()
        );
        for expression in ["1'", "1''0", "0x'1", "0b1'2", "0'8"] {
            let err = match IfExprParser::new(expression, &macros).parse() {
                Ok(_) => return Err(format!("{expression} unexpectedly parsed")),
                Err(err) => err,
            };
            assert!(err.contains("digit separator"), "{err}");
        }
        Ok(())
    }

    #[test]
    fn processes_pragma_macro_push_and_pop() -> Result<(), String> {
        let (expanded, pragmas) =
            process_pragma_operators_located(r#"_Pragma("push_macro(\"X\")") X"#)
                .map_err(|err| err.message)?;
        assert_eq!(expanded.trim(), "X");
        assert_eq!(
            pragmas,
            vec![LocatedPragma {
                text: r#"push_macro("X")"#.to_string(),
                line_offset: 0,
            }]
        );

        let mut include_stack = Vec::new();
        let mut once_files = HashSet::new();
        let mut system_header_files = HashSet::new();
        let mut poisoned_identifiers = HashSet::new();
        let mut saved_macros: HashMap<String, Vec<Option<MacroDef>>> = HashMap::new();
        let mut pragma_pack_stack = Vec::new();
        let mut pragma_pack_alignment = None;
        let mut dependencies = Vec::new();
        let include_paths = IncludePaths::default();
        let mut context = InternalPreprocessContext {
            include_stack: &mut include_stack,
            once_files: &mut once_files,
            system_header_files: &mut system_header_files,
            poisoned_identifiers: &mut poisoned_identifiers,
            saved_macros: &mut saved_macros,
            token_macro_cache: preprocess::macro_expand::MacroTable::new(),
            token_macro_cache_dirty: true,
            pragma_pack_stack: &mut pragma_pack_stack,
            pragma_pack_alignment: &mut pragma_pack_alignment,
            include_paths: &include_paths,
            dependencies: &mut dependencies,
            user_dependencies_only: false,
            missing_headers_generated: false,
            suppress_preprocessed_output: false,
            trace_includes: false,
            line_markers: false,
            stats: None,
        };
        let canonical = PathBuf::from("/tmp/pragma-test.h");
        let mut macros = HashMap::new();
        macros.insert("X".to_string(), MacroDef::Object("1".to_string()));
        handle_internal_pragma(r#"push_macro("X")"#, &canonical, &mut macros, &mut context)?;
        macros.insert("X".to_string(), MacroDef::Object("2".to_string()));
        handle_internal_pragma(r#"pop_macro("X")"#, &canonical, &mut macros, &mut context)?;
        assert!(matches!(macros.get("X"), Some(MacroDef::Object(body)) if body == "1"));
        Ok(())
    }

    #[test]
    fn parses_pragma_macro_names_and_pack_actions() {
        assert_eq!(
            parse_pragma_macro_name(r#"push_macro("VALUE")"#, "push_macro"),
            Some("VALUE".to_string())
        );
        assert_eq!(
            parse_pragma_macro_name(r#"pop_macro("VALUE")"#, "pop_macro"),
            Some("VALUE".to_string())
        );
        assert_eq!(
            parse_pragma_macro_name(r#"push_macro(VALUE)"#, "push_macro"),
            None
        );
        assert!(matches!(
            parse_pragma_pack("pack(push, 8)"),
            Some(PragmaPackAction::Push(Some(8)))
        ));
        assert!(matches!(
            parse_pragma_pack("pack(pop)"),
            Some(PragmaPackAction::Pop)
        ));
        assert!(matches!(
            parse_pragma_pack("pack(0)"),
            Some(PragmaPackAction::Set(None))
        ));
    }

    #[test]
    fn processes_multiple_pragma_operators_in_one_line() -> Result<(), String> {
        let (expanded, pragmas) = process_pragma_operators_located(
            r#"_Pragma("once") int x; _Pragma("GCC poison bad") bad"#,
        )
        .map_err(|err| err.message)?;
        assert!(expanded.contains("int x;"));
        assert_eq!(
            pragmas,
            vec![
                LocatedPragma {
                    text: "once".to_string(),
                    line_offset: 0,
                },
                LocatedPragma {
                    text: "GCC poison bad".to_string(),
                    line_offset: 0,
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn locates_multiline_pragma_operators() -> Result<(), String> {
        let (_expanded, pragmas) =
            process_pragma_operators_located("int x;\n_Pragma(\"once\")\nint y;")
                .map_err(|err| err.message)?;
        assert_eq!(
            pragmas,
            vec![LocatedPragma {
                text: "once".to_string(),
                line_offset: 1,
            }]
        );
        Ok(())
    }

    #[test]
    fn rejects_malformed_pragma_operators() {
        assert!(process_pragma_operators_located(r#"_Pragma(42)"#).is_err());
        assert!(process_pragma_operators_located(r#"_Pragma("once""#).is_err());
        let err = process_pragma_operators_located("int x;\n_Pragma(42)")
            .expect_err("malformed pragma should fail");
        assert_eq!(err.line_offset, 1);
    }

    #[test]
    fn marks_pragma_once_headers_by_canonical_path() -> Result<(), String> {
        let mut include_stack = Vec::new();
        let mut once_files = HashSet::new();
        let mut system_header_files = HashSet::new();
        let mut poisoned_identifiers = HashSet::new();
        let mut saved_macros: HashMap<String, Vec<Option<MacroDef>>> = HashMap::new();
        let mut pragma_pack_stack = Vec::new();
        let mut pragma_pack_alignment = None;
        let mut dependencies = Vec::new();
        let include_paths = IncludePaths::default();
        let mut context = InternalPreprocessContext {
            include_stack: &mut include_stack,
            once_files: &mut once_files,
            system_header_files: &mut system_header_files,
            poisoned_identifiers: &mut poisoned_identifiers,
            saved_macros: &mut saved_macros,
            token_macro_cache: preprocess::macro_expand::MacroTable::new(),
            token_macro_cache_dirty: true,
            pragma_pack_stack: &mut pragma_pack_stack,
            pragma_pack_alignment: &mut pragma_pack_alignment,
            include_paths: &include_paths,
            dependencies: &mut dependencies,
            user_dependencies_only: false,
            missing_headers_generated: false,
            suppress_preprocessed_output: false,
            trace_includes: false,
            line_markers: false,
            stats: None,
        };
        let canonical = PathBuf::from("/tmp/pragma-once-test.h");
        let mut macros = HashMap::new();
        handle_internal_pragma("once", &canonical, &mut macros, &mut context)?;
        assert!(context.once_files.contains(&canonical));
        Ok(())
    }

    #[test]
    fn rejects_malformed_push_and_pack_pragmas() {
        let mut include_stack = Vec::new();
        let mut once_files = HashSet::new();
        let mut system_header_files = HashSet::new();
        let mut poisoned_identifiers = HashSet::new();
        let mut saved_macros: HashMap<String, Vec<Option<MacroDef>>> = HashMap::new();
        let mut pragma_pack_stack = Vec::new();
        let mut pragma_pack_alignment = None;
        let mut dependencies = Vec::new();
        let include_paths = IncludePaths::default();
        let mut context = InternalPreprocessContext {
            include_stack: &mut include_stack,
            once_files: &mut once_files,
            system_header_files: &mut system_header_files,
            poisoned_identifiers: &mut poisoned_identifiers,
            saved_macros: &mut saved_macros,
            token_macro_cache: preprocess::macro_expand::MacroTable::new(),
            token_macro_cache_dirty: true,
            pragma_pack_stack: &mut pragma_pack_stack,
            pragma_pack_alignment: &mut pragma_pack_alignment,
            include_paths: &include_paths,
            dependencies: &mut dependencies,
            user_dependencies_only: false,
            missing_headers_generated: false,
            suppress_preprocessed_output: false,
            trace_includes: false,
            line_markers: false,
            stats: None,
        };
        let canonical = PathBuf::from("/tmp/pragma-malformed-test.h");
        let mut macros = HashMap::new();
        let err = handle_internal_pragma(
            r#"push_macro(VALUE)"#,
            &canonical,
            &mut macros,
            &mut context,
        )
        .unwrap_err();
        assert!(err.contains("malformed #pragma push_macro"), "{err}");
        let err =
            handle_internal_pragma(r#"pop_macro(VALUE)"#, &canonical, &mut macros, &mut context)
                .unwrap_err();
        assert!(err.contains("malformed #pragma pop_macro"), "{err}");
        let err = handle_internal_pragma("pack(push, bad)", &canonical, &mut macros, &mut context)
            .unwrap_err();
        assert!(err.contains("malformed #pragma pack"), "{err}");
    }

    #[test]
    fn reports_malformed_has_include_operands() {
        let macros = HashMap::new();
        let include_paths = IncludePaths::default();
        let mut state = PreprocessorState::new("base.c".to_string());
        let context = IfEvalContext {
            file: "source.c",
            line_number: 1,
            base_dir: Path::new("."),
            include_paths: &include_paths,
            include_level: 0,
        };
        let err = replace_preprocessor_predicates(
            "__has_include(FOO BAR)",
            &macros,
            &mut state,
            &context,
        )
        .unwrap_err();
        assert!(err.contains("malformed include operand"), "{err}");
    }

    #[test]
    fn reports_malformed_include_directives() -> Result<(), String> {
        let macros = HashMap::new();
        let mut state = PreprocessorState::new("base.c".to_string());
        let operand = match preprocess::directive::parse_directive_tokens(&preprocess::lexer::lex(
            "#include FOO BAR",
        )?)? {
            Some(preprocess::directive::Directive::Include { operand, .. }) => operand,
            other => return Err(format!("unexpected directive: {:?}", other)),
        };
        let err = parse_token_include_operand(&operand, &macros, "source.c", 1, 0, &mut state)
            .unwrap_err();
        assert!(err.contains("malformed include operand"), "{err}");
        Ok(())
    }

    #[test]
    fn parses_embed_parameters_and_rejects_duplicates() -> Result<(), String> {
        let macros = HashMap::new();
        let parameters = parse_embed_parameters(
            &preprocess::lexer::lex("limit(1 + 1) prefix(9,) suffix(, 42) if_empty(7)")?,
            &macros,
        )?;
        assert_eq!(parameters.limit, Some(2));
        assert_eq!(parameters.prefix.as_deref(), Some("9,"));
        assert_eq!(parameters.suffix.as_deref(), Some(", 42"));
        assert_eq!(parameters.if_empty.as_deref(), Some("7"));

        let err = parse_embed_parameters(&preprocess::lexer::lex("prefix() prefix()")?, &macros)
            .unwrap_err();
        assert!(err.contains("duplicate #embed prefix"), "{err}");
        Ok(())
    }
}
