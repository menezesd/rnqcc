#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpLocation {
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpSpan {
    pub start: PpLocation,
    pub end: PpLocation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PpTokenKind {
    Ident(String),
    Number(String),
    StringLit(String),
    CharLit(String),
    Punct(String),
    Whitespace(String),
    Newline(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PpToken {
    pub kind: PpTokenKind,
    pub span: PpSpan,
}

impl PpToken {
    pub fn text(&self) -> &str {
        match &self.kind {
            PpTokenKind::Ident(value)
            | PpTokenKind::Number(value)
            | PpTokenKind::StringLit(value)
            | PpTokenKind::CharLit(value)
            | PpTokenKind::Punct(value)
            | PpTokenKind::Whitespace(value)
            | PpTokenKind::Newline(value) => value,
        }
    }

    pub fn clone_with_text(&self, kind: PpTokenKind) -> Self {
        Self {
            kind,
            span: self.span,
        }
    }
}
