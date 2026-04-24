use crate::types::*;

/// Parsed declarator tree
#[derive(Debug)]
enum Declarator {
    Ident(String),
    PointerDeclarator(Box<Declarator>),
    ArrayDeclarator(Box<Declarator>, usize),
    FunDeclarator(Vec<(String, CType, Option<(CType, usize)>)>, Vec<FullType>, Box<Declarator>),
}

fn ptr_info_from_full(ft: &FullType) -> (CType, usize) {
    match ft {
        FullType::Scalar(t) => (*t, 1),
        FullType::Pointer(inner) => {
            let (base, depth) = ptr_info_from_full(inner);
            (base, depth + 1)
        }
        FullType::Array { elem, .. } => {
            let base_ct = elem.to_ctype();
            (base_ct, 1)
        }
    }
}

// ============================================================
// Parser
// ============================================================

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or_else(|| {
            panic!("Unexpected end of tokens");
        });
        self.pos += 1;
        tok
    }

    fn expect(&mut self, expected: Token) {
        let actual = self.advance();
        if actual != expected {
            panic!("Expected {:?} but found {:?}", expected, actual);
        }
    }

    fn at(&self, expected: &Token) -> bool {
        self.peek() == Some(expected)
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.at(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    // --------------------------------------------------------
    // Top-level
    // --------------------------------------------------------

    pub fn parse_program(&mut self) -> Program {
        let mut declarations = Vec::new();
        while self.peek().is_some() {
            declarations.push(self.parse_declaration());
        }
        Program { declarations }
    }

    /// Parse optional storage class and type specifier.
    /// Returns (storage_class, CType).
    /// Handles `static int`, `int static`, `long`, `long int`, `int long`, etc.
    fn parse_specifiers(&mut self) -> (Option<StorageClass>, CType) {
        let mut sc = None;
        let mut has_int = false;
        let mut has_long = false;
        let mut has_unsigned = false;
        let mut has_signed = false;
        let mut has_void = false;

        for _ in 0..4 {
            match self.peek() {
                Some(Token::KWStatic) if sc.is_none() => {
                    self.advance();
                    sc = Some(StorageClass::Static);
                }
                Some(Token::KWExtern) if sc.is_none() => {
                    self.advance();
                    sc = Some(StorageClass::Extern);
                }
                Some(Token::KWInt) if !has_int && !has_void => {
                    self.advance();
                    has_int = true;
                }
                Some(Token::KWLong) if !has_long && !has_void => {
                    self.advance();
                    has_long = true;
                }
                Some(Token::KWUnsigned) if !has_unsigned && !has_signed && !has_void => {
                    self.advance();
                    has_unsigned = true;
                }
                Some(Token::KWSigned) if !has_signed && !has_unsigned && !has_void => {
                    self.advance();
                    has_signed = true;
                }
                Some(Token::KWDouble) if !has_int && !has_void && !has_unsigned && !has_signed => {
                    self.advance();
                    return (sc, CType::Double); // handles both 'double' and 'long double'
                }
                Some(Token::KWFloat) if !has_int && !has_long && !has_void && !has_unsigned && !has_signed => {
                    self.advance();
                    return (sc, CType::Double); // float promoted to double
                }
                Some(Token::KWVoid) if !has_int && !has_long && !has_void && !has_unsigned && !has_signed => {
                    self.advance();
                    has_void = true;
                }
                _ => break,
            }
        }

        if !has_int && !has_long && !has_void && !has_unsigned && !has_signed {
            panic!("Expected type specifier");
        }

        let ctype = if has_void {
            CType::Void
        } else if has_unsigned && has_long {
            CType::ULong
        } else if has_unsigned {
            CType::UInt
        } else if has_long {
            CType::Long
        } else {
            CType::Int // 'signed', 'signed int', 'int' all map to Int
        };

        (sc, ctype)
    }

    fn parse_type(&mut self) -> CType {
        if self.at(&Token::KWDouble) { self.advance(); return CType::Double; }
        if self.at(&Token::KWFloat) { self.advance(); return CType::Double; }
        let mut has_int = false;
        let mut has_long = false;
        let mut has_unsigned = false;
        let mut has_signed = false;
        for _ in 0..3 {
            match self.peek() {
                Some(Token::KWInt) if !has_int => { self.advance(); has_int = true; }
                Some(Token::KWLong) if !has_long => { self.advance(); has_long = true; }
                Some(Token::KWUnsigned) if !has_unsigned && !has_signed => { self.advance(); has_unsigned = true; }
                Some(Token::KWSigned) if !has_signed && !has_unsigned => { self.advance(); has_signed = true; }
                _ => break,
            }
        }
        if has_unsigned && has_long { CType::ULong }
        else if has_unsigned { CType::UInt }
        else if has_long { CType::Long }
        else { CType::Int }
    }

    fn is_type_keyword(tok: &Token) -> bool {
        matches!(tok, Token::KWInt | Token::KWLong | Token::KWVoid | Token::KWUnsigned | Token::KWSigned | Token::KWDouble | Token::KWFloat)
    }

    /// Process a declarator tree to extract name, derived type, and params
    fn process_declarator(decl: &Declarator, base_type: CType) -> (String, FullType, Option<Vec<(String, CType, Option<(CType, usize)>)>>) {
        match decl {
            Declarator::Ident(name) => (name.clone(), FullType::Scalar(base_type), None),
            Declarator::PointerDeclarator(inner) => {
                let derived = FullType::Pointer(Box::new(FullType::Scalar(base_type)));
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::ArrayDeclarator(inner, size) => {
                let derived = FullType::Array { elem: Box::new(FullType::Scalar(base_type)), size: *size };
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::FunDeclarator(params, _pfts, inner) => {
                // Inner must be Ident for valid function declarations
                if let Declarator::Ident(name) = inner.as_ref() {
                    (name.clone(), FullType::Scalar(base_type), Some(params.clone()))
                } else {
                    panic!("Function pointer types not supported");
                }
            }
        }
    }

    fn process_declarator_with_type(decl: &Declarator, current_type: FullType) -> (String, FullType, Option<Vec<(String, CType, Option<(CType, usize)>)>>) {
        match decl {
            Declarator::Ident(name) => (name.clone(), current_type, None),
            Declarator::PointerDeclarator(inner) => {
                let derived = FullType::Pointer(Box::new(current_type));
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::ArrayDeclarator(inner, size) => {
                let derived = FullType::Array { elem: Box::new(current_type), size: *size };
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::FunDeclarator(params, _pfts, inner) => {
                if let Declarator::Ident(name) = inner.as_ref() {
                    // Function returning current_type
                    (name.clone(), current_type, Some(params.clone()))
                } else {
                    panic!("Complex function declarators not supported");
                }
            }
        }
    }

    /// Parse a declarator into a tree structure
    fn parse_declarator_tree(&mut self) -> Declarator {
        // Count leading *
        let mut stars = 0;
        while self.eat(&Token::Star) { stars += 1; }

        // Direct declarator: identifier or (declarator)
        let mut decl = if self.eat(&Token::OpenParen) {
            if self.is_type_keyword_at_pos() || self.at(&Token::CloseParen) {
                // This looks like function params, not a grouped declarator
                // But we haven't seen the name yet — this shouldn't happen at this level
                panic!("Unexpected parameter list in declarator");
            }
            let inner = self.parse_declarator_tree();
            self.expect(Token::CloseParen);
            inner
        } else {
            let name = self.parse_identifier();
            Declarator::Ident(name)
        };

        // Trailing suffixes: (params) or [size]
        if self.at(&Token::OpenParen) {
            self.expect(Token::OpenParen);
            let (params, param_fts) = self.parse_param_list();
            self.expect(Token::CloseParen);
            decl = Declarator::FunDeclarator(params, param_fts, Box::new(decl));
        }
        while self.eat(&Token::OpenBracket) {
            let size = match self.peek().cloned() {
                Some(Token::IntLiteral(n)) => { self.advance(); n as usize }
                Some(Token::UIntLiteral(n)) => { self.advance(); n as usize }
                Some(Token::LongLiteral(n)) => { self.advance(); n as usize }
                Some(Token::ULongLiteral(n)) => { self.advance(); n as usize }
                Some(Token::CloseBracket) => 0, // empty []
                _ => panic!("Expected array size or ]"),
            };
            self.expect(Token::CloseBracket);
            decl = Declarator::ArrayDeclarator(Box::new(decl), size);
        }

        // Wrap in pointer declarators
        for _ in 0..stars {
            decl = Declarator::PointerDeclarator(Box::new(decl));
        }

        decl
    }

    /// Parse a declarator using tree-based parsing.
    /// Returns (name, FullType, optional_params)
    fn parse_declarator_full(&mut self, base_type: CType) -> (String, FullType, Option<Vec<(String, CType, Option<(CType, usize)>)>>) {
        let tree = self.parse_declarator_tree();
        Self::process_declarator(&tree, base_type)
    }

    /// Parse a declarator (backward-compatible wrapper)
    /// Returns (name, pointer_depth, optional_params)
    fn parse_declarator(&mut self) -> (String, usize, Option<Vec<(String, CType, Option<(CType, usize)>)>>) {
        let tree = self.parse_declarator_tree();
        fn extract(d: &Declarator) -> (String, usize, Option<Vec<(String, CType, Option<(CType, usize)>)>>) {
            match d {
                Declarator::Ident(name) => (name.clone(), 0, None),
                Declarator::PointerDeclarator(inner) => {
                    let (name, depth, params) = extract(inner);
                    (name, depth + 1, params)
                }
                Declarator::ArrayDeclarator(inner, _) => {
                    extract(inner)
                }
                Declarator::FunDeclarator(params, _pfts, inner) => {
                    let (name, depth, _) = extract(inner);
                    (name, depth, Some(params.clone()))
                }
            }
        }
        extract(&tree)
    }

    /// Parse abstract declarator into a FullType derivation from base type.
    /// Handles: *, (**), (*)[3], (*(*))[N], etc.
    fn parse_abstract_declarator_type(&mut self, base: CType) -> FullType {
        let mut stars = 0;
        while self.eat(&Token::Star) { stars += 1; }

        // Check for parenthesized abstract declarator
        let inner_type = if self.eat(&Token::OpenParen) {
            let inner = self.parse_abstract_declarator_type(base);
            self.expect(Token::CloseParen);
            Some(inner)
        } else {
            None
        };

        // Check for array dimensions [N]
        let mut dims = Vec::new();
        while self.eat(&Token::OpenBracket) {
            match self.peek().cloned() {
                Some(Token::IntLiteral(n)) => { self.advance(); dims.push(n as usize); }
                Some(Token::UIntLiteral(n)) => { self.advance(); dims.push(n as usize); }
                Some(Token::LongLiteral(n)) => { self.advance(); dims.push(n as usize); }
                Some(Token::ULongLiteral(n)) => { self.advance(); dims.push(n as usize); }
                _ => {}
            }
            self.expect(Token::CloseBracket);
        }

        // Build type from outside in
        if let Some(inner) = inner_type {
            // The inner type was parsed recursively
            // Apply array dims to it, then pointer stars
            let mut t = inner;
            for &dim in dims.iter().rev() {
                t = FullType::Array { elem: Box::new(t), size: dim };
            }
            for _ in 0..stars {
                t = FullType::Pointer(Box::new(t));
            }
            t
        } else {
            // No parens — build from base type
            let mut t = FullType::Scalar(base);
            for &dim in dims.iter().rev() {
                t = FullType::Array { elem: Box::new(t), size: dim };
            }
            for _ in 0..stars {
                t = FullType::Pointer(Box::new(t));
            }
            t
        }
    }

    /// Backward-compat: just count pointer depth
    fn parse_abstract_declarator_depth(&mut self) -> usize {
        let mut depth = 0;
        while self.eat(&Token::Star) { depth += 1; }
        if self.eat(&Token::OpenParen) {
            depth += self.parse_abstract_declarator_depth();
            self.expect(Token::CloseParen);
        }
        depth
    }

    fn parse_array_dims(&mut self) -> Option<Vec<usize>> {
        let mut dims = Vec::new();
        while self.eat(&Token::OpenBracket) {
            match self.peek().cloned() {
                Some(Token::IntLiteral(n)) => { self.advance(); dims.push(n as usize); }
                Some(Token::UIntLiteral(n)) => { self.advance(); dims.push(n as usize); }
                Some(Token::LongLiteral(n)) => { self.advance(); dims.push(n as usize); }
                Some(Token::ULongLiteral(n)) => { self.advance(); dims.push(n as usize); }
                Some(Token::CloseBracket) => { dims.push(0); } // empty [] — size inferred
                _ => panic!("Expected array size or ]"),
            }
            self.expect(Token::CloseBracket);
        }
        if dims.is_empty() { None } else { Some(dims) }
    }

    fn parse_array_init(&mut self) -> Exp {
        self.expect(Token::OpenBrace);
        let mut elems = Vec::new();
        if !self.at(&Token::CloseBrace) {
            if self.at(&Token::OpenBrace) {
                elems.push(self.parse_array_init());
            } else {
                elems.push(self.parse_assignment());
            }
            while self.eat(&Token::Comma) {
                if self.at(&Token::CloseBrace) { break; } // trailing comma
                if self.at(&Token::OpenBrace) {
                    elems.push(self.parse_array_init());
                } else {
                    elems.push(self.parse_assignment());
                }
            }
        }
        self.expect(Token::CloseBrace);
        Exp::ArrayInit(elems)
    }

    fn extract_array_dims(ft: &FullType) -> Option<Vec<usize>> {
        match ft {
            FullType::Array { elem, size } => {
                let mut dims = vec![*size];
                let mut inner = elem.as_ref();
                while let FullType::Array { elem: e, size: s } = inner {
                    dims.push(*s);
                    inner = e;
                }
                Some(dims)
            }
            _ => None,
        }
    }

    fn is_type_keyword_at_pos(&self) -> bool {
        match self.peek() {
            Some(tok) => Self::is_type_keyword(tok),
            None => false,
        }
    }

    fn parse_declaration(&mut self) -> Declaration {
        let (sc, base_type) = self.parse_specifiers();

        let decl_tree = self.parse_declarator_tree();
        let (name, full_type, decl_params) = Self::process_declarator(&decl_tree, base_type);

        // Extract backward-compat fields from FullType
        let ctype = full_type.to_ctype();
        let pi = match &full_type {
            FullType::Pointer(inner) => {
                let (base, depth) = ptr_info_from_full(inner);
                Some((base, depth))
            }
            _ => None,
        };
        // Extract array dims
        let array_dims = Self::extract_array_dims(&full_type);

        // Extract param_full_types from the declarator tree
        fn extract_param_fts(d: &Declarator) -> Vec<FullType> {
            match d {
                Declarator::FunDeclarator(_, fts, _) => fts.clone(),
                Declarator::PointerDeclarator(inner) | Declarator::ArrayDeclarator(inner, _) => extract_param_fts(inner),
                _ => vec![],
            }
        }
        let decl_param_fts = extract_param_fts(&decl_tree);

        // Is it a function?
        if let Some(params) = decl_params {
            let body = if self.at(&Token::OpenBrace) {
                Some(self.parse_block())
            } else {
                self.expect(Token::Semicolon);
                None
            };
            return Declaration::FunDecl(FunctionDeclaration {
                name,
                return_type: ctype,
                return_ptr_info: pi, return_full_type: Some(full_type.clone()),
                params,
                body,
                storage_class: sc,
                param_full_types: decl_param_fts.clone(),
            });
        }

        // Check for function (in case declarator didn't catch params)
        if self.at(&Token::OpenParen) {
            self.expect(Token::OpenParen);
            let (params, param_fts) = self.parse_param_list();
            self.expect(Token::CloseParen);
            let body = if self.at(&Token::OpenBrace) {
                Some(self.parse_block())
            } else {
                self.expect(Token::Semicolon);
                None
            };
            return Declaration::FunDecl(FunctionDeclaration {
                name,
                return_type: ctype,
                return_ptr_info: pi, return_full_type: Some(full_type.clone()),
                params,
                body,
                storage_class: sc,
                param_full_types: param_fts.clone(),
            });
        }

        if ctype == CType::Void && array_dims.is_none() {
            panic!("Cannot declare variable with void type");
        }

        let init = if self.eat(&Token::Assign) {
            if self.at(&Token::OpenBrace) {
                Some(self.parse_array_init())
            } else {
                Some(self.parse_expression())
            }
        } else {
            None
        };
        self.expect(Token::Semicolon);
        Declaration::VarDecl(VarDeclaration {
            name,
            var_type: if array_dims.is_some() { let mut t = &full_type; while let FullType::Array { elem, .. } = t { t = elem; } t.to_ctype() } else { ctype },
            ptr_info: pi,
            array_dims, decl_full_type: Some(full_type.clone()),
            init,
            storage_class: sc,
        })
    }

    fn parse_var_declaration(&mut self) -> VarDeclaration {
        let (sc, base_type) = self.parse_specifiers();
        let (name, full_type, _) = self.parse_declarator_full(base_type);
        let ctype = full_type.to_ctype();
        let pi = match &full_type {
            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
            _ => None,
        };
        let array_dims = Self::extract_array_dims(&full_type);
        if ctype == CType::Void && array_dims.is_none() {
            panic!("Cannot declare variable with void type");
        }
        let init = if self.eat(&Token::Assign) {
            if self.at(&Token::OpenBrace) {
                Some(self.parse_array_init())
            } else {
                Some(self.parse_expression())
            }
        } else {
            None
        };
        self.expect(Token::Semicolon);
        VarDeclaration {
            name,
            var_type: if array_dims.is_some() { let mut t = &full_type; while let FullType::Array { elem, .. } = t { t = elem; } t.to_ctype() } else { ctype },
            ptr_info: pi,
            array_dims, decl_full_type: Some(full_type.clone()),
            init,
            storage_class: sc,
        }
    }

    fn parse_param_list(&mut self) -> (Vec<(String, CType, Option<(CType, usize)>)>, Vec<FullType>) {
        // "void" or empty or "int x, long y, ..."
        if self.at(&Token::KWVoid) {
            if self.pos + 1 < self.tokens.len() && self.tokens[self.pos + 1] == Token::CloseParen {
                self.advance();
                return (Vec::new(), Vec::new());
            }
        }
        if self.at(&Token::CloseParen) {
            return (Vec::new(), Vec::new());
        }
        let mut params = Vec::new();
        let mut param_fts = Vec::new();
        let parse_one_param = |s: &mut Self, fts: &mut Vec<FullType>| -> (String, CType, Option<(CType, usize)>) {
            let base = s.parse_type();
            let (name, full_type, _) = s.parse_declarator_full(base);
            // Array parameters decay to pointers (first dimension dropped)
            let ft = match full_type {
                FullType::Array { elem, .. } => FullType::Pointer(elem),
                other => other,
            };
            fts.push(ft.clone());
            let t = ft.to_ctype();
            let pi = match &ft {
                FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                _ => None,
            };
            (name, t, pi)
        };
        params.push(parse_one_param(self, &mut param_fts));
        while self.eat(&Token::Comma) {
            params.push(parse_one_param(self, &mut param_fts));
        }
        (params, param_fts)
    }

    fn parse_identifier(&mut self) -> String {
        match self.advance() {
            Token::Identifier(name) => name,
            other => panic!("Expected identifier, found {:?}", other),
        }
    }

    // --------------------------------------------------------
    // Blocks and block items
    // --------------------------------------------------------

    fn parse_block(&mut self) -> Block {
        self.expect(Token::OpenBrace);
        let mut items = Vec::new();
        while !self.at(&Token::CloseBrace) {
            items.push(self.parse_block_item());
        }
        self.expect(Token::CloseBrace);
        items
    }

    fn is_declaration_start(&self) -> bool {
        matches!(
            self.peek(),
            Some(Token::KWInt)
                | Some(Token::KWLong)
                | Some(Token::KWUnsigned)
                | Some(Token::KWSigned)
                | Some(Token::KWDouble)
                | Some(Token::KWFloat)
                | Some(Token::KWVoid)
                | Some(Token::KWStatic)
                | Some(Token::KWExtern)
        )
    }

    fn parse_block_item(&mut self) -> BlockItem {
        if self.is_declaration_start() {
            let (sc, base_type) = self.parse_specifiers();
            let (name, full_type, decl_params) = self.parse_declarator_full(base_type);
            let ctype = full_type.to_ctype();
            let pi = match &full_type {
                FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                _ => None,
            };

            if decl_params.is_some() || self.at(&Token::OpenParen) {
                let params = if let Some(p) = decl_params {
                    p
                } else {
                    self.expect(Token::OpenParen);
                    let (p, _p_fts) = self.parse_param_list();
                    self.expect(Token::CloseParen);
                    p
                };

                let body = if self.at(&Token::OpenBrace) {
                    Some(self.parse_block())
                } else {
                    self.expect(Token::Semicolon);
                    None
                };

                if body.is_some() {
                    panic!("Function definitions not allowed inside blocks");
                }

                BlockItem::Declaration(Declaration::FunDecl(FunctionDeclaration {
                    name,
                    return_type: ctype,
                    return_ptr_info: pi, return_full_type: Some(full_type.clone()),
                    params,
                    body,
                    storage_class: sc,
                    param_full_types: vec![],
                }))
            } else {
                let array_dims = Self::extract_array_dims(&full_type);
                if ctype == CType::Void && array_dims.is_none() {
                    panic!("Cannot declare variable with void type");
                }
                let init = if self.eat(&Token::Assign) {
                    if self.at(&Token::OpenBrace) {
                        Some(self.parse_array_init())
                    } else {
                        Some(self.parse_expression())
                    }
                } else {
                    None
                };
                self.expect(Token::Semicolon);
                BlockItem::Declaration(Declaration::VarDecl(VarDeclaration {
                    name,
                    var_type: if array_dims.is_some() { let mut t = &full_type; while let FullType::Array { elem, .. } = t { t = elem; } t.to_ctype() } else { ctype },
                    ptr_info: pi,
                    array_dims, decl_full_type: Some(full_type.clone()),
                    init,
                    storage_class: sc,
                }))
            }
        } else {
            BlockItem::Statement(self.parse_statement())
        }
    }

    // --------------------------------------------------------
    // Statements
    // --------------------------------------------------------

    fn parse_statement(&mut self) -> Statement {
        match self.peek() {
            Some(Token::KWReturn) => {
                self.advance();
                let exp = self.parse_expression();
                self.expect(Token::Semicolon);
                Statement::Return(exp)
            }
            Some(Token::KWIf) => {
                self.advance();
                self.expect(Token::OpenParen);
                let condition = self.parse_expression();
                self.expect(Token::CloseParen);
                let then_stmt = Box::new(self.parse_statement());
                let else_stmt = if self.eat(&Token::KWElse) {
                    Some(Box::new(self.parse_statement()))
                } else {
                    None
                };
                Statement::If(condition, then_stmt, else_stmt)
            }
            Some(Token::KWWhile) => {
                self.advance();
                self.expect(Token::OpenParen);
                let condition = self.parse_expression();
                self.expect(Token::CloseParen);
                let body = Box::new(self.parse_statement());
                Statement::While {
                    condition,
                    body,
                    label: String::new(), // filled by resolve pass
                }
            }
            Some(Token::KWDo) => {
                self.advance();
                let body = Box::new(self.parse_statement());
                self.expect(Token::KWWhile);
                self.expect(Token::OpenParen);
                let condition = self.parse_expression();
                self.expect(Token::CloseParen);
                self.expect(Token::Semicolon);
                Statement::DoWhile {
                    body,
                    condition,
                    label: String::new(),
                }
            }
            Some(Token::KWFor) => {
                self.advance();
                self.expect(Token::OpenParen);
                let init = if self.is_declaration_start() {
                    ForInit::Declaration(self.parse_var_declaration())
                } else if self.eat(&Token::Semicolon) {
                    ForInit::Expression(None)
                } else {
                    let exp = self.parse_expression();
                    self.expect(Token::Semicolon);
                    ForInit::Expression(Some(exp))
                };
                let condition = if self.at(&Token::Semicolon) {
                    None
                } else {
                    Some(self.parse_expression())
                };
                self.expect(Token::Semicolon);
                let post = if self.at(&Token::CloseParen) {
                    None
                } else {
                    Some(self.parse_expression())
                };
                self.expect(Token::CloseParen);
                let body = Box::new(self.parse_statement());
                Statement::For {
                    init,
                    condition,
                    post,
                    body,
                    label: String::new(),
                }
            }
            Some(Token::KWBreak) => {
                self.advance();
                self.expect(Token::Semicolon);
                Statement::Break(String::new()) // filled by resolve pass
            }
            Some(Token::KWContinue) => {
                self.advance();
                self.expect(Token::Semicolon);
                Statement::Continue(String::new())
            }
            Some(Token::KWGoto) => {
                self.advance();
                let label = self.parse_identifier();
                self.expect(Token::Semicolon);
                Statement::Goto(label)
            }
            Some(Token::KWSwitch) => {
                self.advance();
                self.expect(Token::OpenParen);
                let control = self.parse_expression();
                self.expect(Token::CloseParen);
                let body = Box::new(self.parse_statement());
                Statement::Switch {
                    control,
                    body,
                    label: String::new(),
                    cases: Vec::new(),
                }
            }
            Some(Token::KWCase) => {
                self.advance();
                let value = self.parse_expression();
                self.expect(Token::Colon);
                let body = Box::new(self.parse_statement());
                Statement::Case {
                    value,
                    body,
                    label: String::new(),
                }
            }
            Some(Token::KWDefault) => {
                self.advance();
                self.expect(Token::Colon);
                let body = Box::new(self.parse_statement());
                Statement::Default {
                    body,
                    label: String::new(),
                }
            }
            Some(Token::OpenBrace) => Statement::Block(self.parse_block()),
            Some(Token::Semicolon) => {
                self.advance();
                Statement::Null
            }
            // Check for labeled statement: identifier ':'
            Some(Token::Identifier(_))
                if self.pos + 1 < self.tokens.len()
                    && self.tokens[self.pos + 1] == Token::Colon =>
            {
                let name = self.parse_identifier();
                self.expect(Token::Colon);
                let stmt = Box::new(self.parse_statement());
                Statement::Label(name, stmt)
            }
            _ => {
                let exp = self.parse_expression();
                self.expect(Token::Semicolon);
                Statement::Expression(exp)
            }
        }
    }

    // --------------------------------------------------------
    // Expressions (precedence climbing)
    // --------------------------------------------------------

    fn parse_expression(&mut self) -> Exp {
        self.parse_assignment()
    }

    fn parse_assignment(&mut self) -> Exp {
        let left = self.parse_conditional();

        match self.peek().cloned() {
            Some(Token::Assign) => {
                self.advance();
                let right = self.parse_assignment(); // right-associative
                Exp::Assign(Box::new(left), Box::new(right))
            }
            Some(tok) => {
                if let Some(op) = Self::compound_assign_op(&tok) {
                    self.advance();
                    let right = self.parse_assignment();
                    Exp::CompoundAssign(op, Box::new(left), Box::new(right))
                } else {
                    left
                }
            }
            None => left,
        }
    }

    fn compound_assign_op(tok: &Token) -> Option<BinaryOp> {
        match tok {
            Token::PlusAssign => Some(BinaryOp::Add),
            Token::MinusAssign => Some(BinaryOp::Sub),
            Token::StarAssign => Some(BinaryOp::Mul),
            Token::SlashAssign => Some(BinaryOp::Div),
            Token::PercentAssign => Some(BinaryOp::Mod),
            Token::AmpersandAssign => Some(BinaryOp::BitwiseAnd),
            Token::PipeAssign => Some(BinaryOp::BitwiseOr),
            Token::CaretAssign => Some(BinaryOp::BitwiseXor),
            Token::ShiftLeftAssign => Some(BinaryOp::ShiftLeft),
            Token::ShiftRightAssign => Some(BinaryOp::ShiftRight),
            _ => None,
        }
    }

    fn parse_conditional(&mut self) -> Exp {
        let cond = self.parse_logical_or();
        if self.eat(&Token::Question) {
            let then_exp = self.parse_expression();
            self.expect(Token::Colon);
            let else_exp = self.parse_conditional(); // right-associative
            Exp::Conditional(Box::new(cond), Box::new(then_exp), Box::new(else_exp))
        } else {
            cond
        }
    }

    fn parse_logical_or(&mut self) -> Exp {
        let mut left = self.parse_logical_and();
        while self.eat(&Token::LogicalOr) {
            let right = self.parse_logical_and();
            left = Exp::Binary(BinaryOp::LogicalOr, Box::new(left), Box::new(right));
        }
        left
    }

    fn parse_logical_and(&mut self) -> Exp {
        let mut left = self.parse_bitwise_or();
        while self.eat(&Token::LogicalAnd) {
            let right = self.parse_bitwise_or();
            left = Exp::Binary(BinaryOp::LogicalAnd, Box::new(left), Box::new(right));
        }
        left
    }

    fn parse_bitwise_or(&mut self) -> Exp {
        let mut left = self.parse_bitwise_xor();
        while self.eat(&Token::Pipe) {
            let right = self.parse_bitwise_xor();
            left = Exp::Binary(BinaryOp::BitwiseOr, Box::new(left), Box::new(right));
        }
        left
    }

    fn parse_bitwise_xor(&mut self) -> Exp {
        let mut left = self.parse_bitwise_and();
        while self.eat(&Token::Caret) {
            let right = self.parse_bitwise_and();
            left = Exp::Binary(BinaryOp::BitwiseXor, Box::new(left), Box::new(right));
        }
        left
    }

    fn parse_bitwise_and(&mut self) -> Exp {
        let mut left = self.parse_equality();
        while self.eat(&Token::Ampersand) {
            let right = self.parse_equality();
            left = Exp::Binary(BinaryOp::BitwiseAnd, Box::new(left), Box::new(right));
        }
        left
    }

    fn parse_equality(&mut self) -> Exp {
        let mut left = self.parse_relational();
        loop {
            match self.peek().cloned() {
                Some(Token::EqualEqual) => {
                    self.advance();
                    let right = self.parse_relational();
                    left = Exp::Binary(BinaryOp::Equal, Box::new(left), Box::new(right));
                }
                Some(Token::NotEqual) => {
                    self.advance();
                    let right = self.parse_relational();
                    left = Exp::Binary(BinaryOp::NotEqual, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        left
    }

    fn parse_relational(&mut self) -> Exp {
        let mut left = self.parse_shift();
        loop {
            match self.peek().cloned() {
                Some(Token::LessThan) => {
                    self.advance();
                    let right = self.parse_shift();
                    left = Exp::Binary(BinaryOp::LessThan, Box::new(left), Box::new(right));
                }
                Some(Token::GreaterThan) => {
                    self.advance();
                    let right = self.parse_shift();
                    left = Exp::Binary(BinaryOp::GreaterThan, Box::new(left), Box::new(right));
                }
                Some(Token::LessEqual) => {
                    self.advance();
                    let right = self.parse_shift();
                    left = Exp::Binary(BinaryOp::LessEqual, Box::new(left), Box::new(right));
                }
                Some(Token::GreaterEqual) => {
                    self.advance();
                    let right = self.parse_shift();
                    left = Exp::Binary(BinaryOp::GreaterEqual, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        left
    }

    fn parse_shift(&mut self) -> Exp {
        let mut left = self.parse_additive();
        loop {
            match self.peek().cloned() {
                Some(Token::ShiftLeft) => {
                    self.advance();
                    let right = self.parse_additive();
                    left = Exp::Binary(BinaryOp::ShiftLeft, Box::new(left), Box::new(right));
                }
                Some(Token::ShiftRight) => {
                    self.advance();
                    let right = self.parse_additive();
                    left = Exp::Binary(BinaryOp::ShiftRight, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        left
    }

    fn parse_additive(&mut self) -> Exp {
        let mut left = self.parse_multiplicative();
        loop {
            match self.peek().cloned() {
                Some(Token::Plus) => {
                    self.advance();
                    let right = self.parse_multiplicative();
                    left = Exp::Binary(BinaryOp::Add, Box::new(left), Box::new(right));
                }
                Some(Token::Minus) => {
                    self.advance();
                    let right = self.parse_multiplicative();
                    left = Exp::Binary(BinaryOp::Sub, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        left
    }

    fn parse_multiplicative(&mut self) -> Exp {
        let mut left = self.parse_unary();
        loop {
            match self.peek().cloned() {
                Some(Token::Star) => {
                    self.advance();
                    let right = self.parse_unary();
                    left = Exp::Binary(BinaryOp::Mul, Box::new(left), Box::new(right));
                }
                Some(Token::Slash) => {
                    self.advance();
                    let right = self.parse_unary();
                    left = Exp::Binary(BinaryOp::Div, Box::new(left), Box::new(right));
                }
                Some(Token::Percent) => {
                    self.advance();
                    let right = self.parse_unary();
                    left = Exp::Binary(BinaryOp::Mod, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        left
    }

    fn parse_unary(&mut self) -> Exp {
        match self.peek().cloned() {
            Some(Token::Minus) => {
                self.advance();
                Exp::Unary(UnaryOp::Negate, Box::new(self.parse_unary()))
            }
            Some(Token::Tilde) => {
                self.advance();
                Exp::Unary(UnaryOp::Complement, Box::new(self.parse_unary()))
            }
            Some(Token::Bang) => {
                self.advance();
                Exp::Unary(UnaryOp::LogicalNot, Box::new(self.parse_unary()))
            }
            Some(Token::Increment) => {
                self.advance();
                Exp::Unary(UnaryOp::PreIncrement, Box::new(self.parse_unary()))
            }
            Some(Token::Decrement) => {
                self.advance();
                Exp::Unary(UnaryOp::PreDecrement, Box::new(self.parse_unary()))
            }
            Some(Token::Star) => {
                self.advance();
                Exp::Unary(UnaryOp::Deref, Box::new(self.parse_unary()))
            }
            Some(Token::Ampersand) => {
                self.advance();
                Exp::Unary(UnaryOp::AddrOf, Box::new(self.parse_unary()))
            }
            // Cast expression: (type) unary or (type *) unary
            Some(Token::OpenParen)
                if self.pos + 1 < self.tokens.len()
                    && Self::is_type_keyword(&self.tokens[self.pos + 1]) =>
            {
                self.advance(); // consume '('
                let base_type = self.parse_type();
                let full_type = self.parse_abstract_declarator_type(base_type);
                self.expect(Token::CloseParen);
                let target_type = full_type.to_ctype();
                let operand = self.parse_unary();
                Exp::Cast(target_type, Box::new(operand))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Exp {
        let mut expr = self.parse_primary();
        loop {
            match self.peek().cloned() {
                Some(Token::Increment) => {
                    self.advance();
                    expr = Exp::Unary(UnaryOp::PostIncrement, Box::new(expr));
                }
                Some(Token::Decrement) => {
                    self.advance();
                    expr = Exp::Unary(UnaryOp::PostDecrement, Box::new(expr));
                }
                Some(Token::OpenBracket) => {
                    self.advance();
                    let index = self.parse_expression();
                    self.expect(Token::CloseBracket);
                    expr = Exp::Subscript(Box::new(expr), Box::new(index));
                }
                _ => break,
            }
        }
        expr
    }

    fn parse_primary(&mut self) -> Exp {
        match self.peek().cloned() {
            Some(Token::IntLiteral(val)) => {
                self.advance();
                if val >= i32::MIN as i64 && val <= i32::MAX as i64 {
                    Exp::Constant(val)
                } else if val >= i64::MIN && val <= i64::MAX {
                    Exp::LongConstant(val)
                } else {
                    Exp::LongConstant(val)
                }
            }
            Some(Token::LongLiteral(val)) => {
                self.advance();
                Exp::LongConstant(val)
            }
            Some(Token::UIntLiteral(val)) => {
                self.advance();
                // UInt constants > UINT_MAX are promoted to ulong
                if val > u32::MAX as i64 {
                    Exp::ULongConstant(val)
                } else {
                    Exp::UIntConstant(val)
                }
            }
            Some(Token::ULongLiteral(val)) => {
                self.advance();
                Exp::ULongConstant(val)
            }
            Some(Token::DoubleLiteral(val)) => {
                self.advance();
                Exp::DoubleConstant(val)
            }
            Some(Token::Identifier(name)) => {
                self.advance();
                // Check for function call
                if self.eat(&Token::OpenParen) {
                    let args = self.parse_arg_list();
                    self.expect(Token::CloseParen);
                    Exp::FunctionCall(name, args)
                } else {
                    Exp::Var(name)
                }
            }
            Some(Token::OpenParen) => {
                self.advance();
                let exp = self.parse_expression();
                self.expect(Token::CloseParen);
                exp
            }
            other => panic!("Expected expression, found {:?}", other),
        }
    }

    fn parse_arg_list(&mut self) -> Vec<Exp> {
        if self.at(&Token::CloseParen) {
            return Vec::new();
        }
        let mut args = vec![self.parse_assignment()];
        while self.eat(&Token::Comma) {
            args.push(self.parse_assignment());
        }
        args
    }
}

pub fn parse(tokens: Vec<Token>) -> Program {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}
