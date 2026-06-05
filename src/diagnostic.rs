#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Parse,
    Resolve,
    Tacky,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    ParseError { message: String },
    DuplicateVariable { name: String },
    UndeclaredVariable { name: String },
    BreakOutsideLoopOrSwitch,
    ContinueOutsideLoop,
    DuplicateLabel { name: String },
    CaseOutsideSwitch,
    DefaultOutsideSwitch,
    UndefinedGotoLabel { name: String },
    ConflictingFunctionParameterCount { name: String },
    NonConstantCaseValue,
    ResolveError { message: String },
    TackyError { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarningKind {
    UnreachableStatement { after: String },
    MissingReturn { function: String },
    NegativeShiftCount,
    CompareDistinctPointerTypes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub phase: Phase,
    pub kind: WarningKind,
    pub message: String,
    pub span: Option<Box<crate::lex::SourceSpan>>,
}

impl Warning {
    pub fn resolve(kind: WarningKind) -> Self {
        let message = match &kind {
            WarningKind::UnreachableStatement { after } => {
                format!("unreachable statement after {}", after)
            }
            WarningKind::MissingReturn { function } => {
                format!(
                    "non-void function '{}' may exit without returning a value",
                    function
                )
            }
            WarningKind::NegativeShiftCount => "shift count is negative".to_string(),
            WarningKind::CompareDistinctPointerTypes => {
                "comparison of distinct pointer types".to_string()
            }
        };
        Self {
            phase: Phase::Resolve,
            kind,
            message,
            span: None,
        }
    }

    pub fn render(&self) -> String {
        let phase = match self.phase {
            Phase::Parse => "parse",
            Phase::Resolve => "resolve",
            Phase::Tacky => "tacky",
        };
        if let Some(span) = &self.span {
            format!(
                "{} warning at {}: {}",
                phase,
                render_location(&span.start),
                self.message
            )
        } else {
            format!("{} warning: {}", phase, self.message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub phase: Phase,
    pub kind: DiagnosticKind,
    pub message: String,
    pub span: Option<Box<crate::lex::SourceSpan>>,
}

impl Diagnostic {
    pub fn resolve(kind: DiagnosticKind) -> Self {
        let message = match &kind {
            DiagnosticKind::DuplicateVariable { name } => {
                format!("duplicate variable declaration: '{}'", name)
            }
            DiagnosticKind::UndeclaredVariable { name } => {
                format!("undeclared variable: '{}'", name)
            }
            DiagnosticKind::BreakOutsideLoopOrSwitch => "break outside of loop or switch".into(),
            DiagnosticKind::ContinueOutsideLoop => "continue outside of loop".into(),
            DiagnosticKind::DuplicateLabel { name } => format!("duplicate label: '{}'", name),
            DiagnosticKind::CaseOutsideSwitch => "case outside of switch".into(),
            DiagnosticKind::DefaultOutsideSwitch => "default outside of switch".into(),
            DiagnosticKind::UndefinedGotoLabel { name } => {
                format!("goto references undefined label: '{}'", name)
            }
            DiagnosticKind::ConflictingFunctionParameterCount { name } => {
                format!("function '{}' declared with conflicting type", name)
            }
            DiagnosticKind::NonConstantCaseValue => "case value must be a constant".into(),
            DiagnosticKind::ResolveError { message } => message.clone(),
            DiagnosticKind::TackyError { message } => message.clone(),
            DiagnosticKind::ParseError { message } => message.clone(),
        };
        Self {
            phase: Phase::Resolve,
            kind,
            message,
            span: None,
        }
    }

    pub fn render(&self) -> String {
        let phase = match self.phase {
            Phase::Parse => "parse",
            Phase::Resolve => "resolve",
            Phase::Tacky => "tacky",
        };
        if let Some(span) = &self.span {
            format!(
                "{} failed at {}: {}",
                phase,
                render_location(&span.start),
                self.message
            )
        } else {
            format!("{} failed: {}", phase, self.message)
        }
    }

    pub fn parse(message: impl Into<String>, span: Option<crate::lex::SourceSpan>) -> Self {
        let message = message.into();
        Self {
            phase: Phase::Parse,
            kind: DiagnosticKind::ParseError {
                message: message.clone(),
            },
            message,
            span: span.map(Box::new),
        }
    }

    pub fn tacky(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            phase: Phase::Tacky,
            kind: DiagnosticKind::TackyError {
                message: message.clone(),
            },
            message,
            span: None,
        }
    }
}

fn render_location(location: &crate::lex::SourceLocation) -> String {
    match &location.file {
        Some(file) => format!("{}:{}:{}", file, location.line, location.column),
        None => format!("{}:{}", location.line, location.column),
    }
}
