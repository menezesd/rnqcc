use crate::diagnostic::Diagnostic;
use crate::lex;
use crate::types::*;
use indexmap::IndexMap;

mod stmt_expr;

type ParseResult<T> = Result<T, String>;
const MAX_SUPPORTED_ALIGNMENT: usize = 1 << 30;
const VLA_STATIC_SCALE_FALLBACK: usize = 16;

#[derive(Debug, Clone, Copy, Default)]
struct AggregateAttributes {
    packed: bool,
    transparent_union: bool,
    alignment: Option<std::num::NonZeroUsize>,
    reverse_storage_order: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct MemberAttributes {
    alignment: Option<std::num::NonZeroUsize>,
    packed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct DeclarationAttributes {
    alignment: Option<std::num::NonZeroUsize>,
    noreturn: bool,
    vector_size: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BitIntSpec {
    width: i64,
    unsigned: bool,
    storage: CType,
}

#[derive(Debug, Clone, Copy)]
struct IntegerConstantValue {
    value: i64,
    is_unsigned: bool,
}

impl IntegerConstantValue {
    fn signed(value: i64) -> Self {
        Self {
            value,
            is_unsigned: false,
        }
    }

    fn unsigned(value: i64) -> Self {
        Self {
            value,
            is_unsigned: true,
        }
    }
}

#[derive(Debug)]
enum AbstractDecl {
    Base,
    Pointer(Box<AbstractDecl>),
    Array(Box<AbstractDecl>, usize),
    Function(Vec<FullType>, bool, Box<AbstractDecl>),
}

/// Parsed declarator tree
#[derive(Debug)]
enum Declarator {
    Ident(String),
    Pointer(Box<Declarator>),
    Array(Box<Declarator>, usize),
    Function(
        Vec<ParamDecl>,
        Vec<FullType>,
        Vec<DeprecatedParam>,
        bool,
        bool,
        bool,
        Vec<Exp>,
        Box<Declarator>,
    ),
}

#[derive(Debug, Clone)]
struct FunctionDeclaratorInfo {
    params: Vec<ParamDecl>,
    param_full_types: Vec<FullType>,
    deprecated_params: Vec<DeprecatedParam>,
    variadic: bool,
    zero_fixed_variadic: bool,
    old_style: bool,
    param_vla_bounds: Vec<Exp>,
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
        FullType::Vector { elem, .. } => (elem.to_ctype(), 1),
        FullType::Function { return_type, .. } => ptr_info_from_full(return_type),
        FullType::Struct(_) => (CType::Struct, 1),
    }
}

// ============================================================
// Parser
// ============================================================

/// Stored typedef: the base CType, the optional struct tag, and the FullType.
#[derive(Debug, Clone)]
struct TypedefInfo {
    base_type: CType,
    full_type: FullType,
    struct_tag: Option<String>,
    is_enum: bool,
    vla_size: Option<Exp>,
    alignment: Option<std::num::NonZeroUsize>,
}

#[derive(Debug, Clone, Default)]
struct CompatTypeMeta {
    pointee_qualified: bool,
    long_double: bool,
    enum_typedef: Option<String>,
}

#[derive(Debug, Clone)]
struct StructMemberVlaElemSize {
    member: String,
    elem_size: Exp,
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    target: Target,
    last_struct_tag: Option<String>,
    /// Scoped typedef table: each scope maps typedef names to their resolved type info.
    typedef_scopes: Vec<std::collections::HashMap<String, TypedefInfo>>,
    /// Struct/union definitions encountered during type specifier parsing
    pending_struct_decls: Vec<StructDeclaration>,
    /// Parser-time struct/union layouts for constant-expression helpers like offsetof.
    struct_defs: IndexMap<String, StructDef>,
    /// Dynamic element sizes for struct members declared with VLA typedef element types.
    struct_member_vla_elem_sizes: IndexMap<(String, String), Exp>,
    pending_struct_member_vla_elem_sizes: Vec<Vec<StructMemberVlaElemSize>>,
    /// Full type from the last typedef used as a type specifier
    last_typedef_full_type: Option<FullType>,
    /// Dynamic byte-size expression from the last VLA typedef used as a type specifier.
    last_typedef_vla_size: Option<Exp>,
    /// Dynamic byte-size expression from the most recently parsed type name.
    last_type_name_vla_size: Option<Exp>,
    /// Scoped enum constant table: maps constant names to their integer values.
    enum_scopes: Vec<std::collections::HashMap<String, i64>>,
    /// Scoped object table for parser-time typeof(expr) support.
    value_scopes: Vec<std::collections::HashMap<String, FullType>>,
    value_vla_size_scopes: Vec<std::collections::HashMap<String, Exp>>,
    value_vla_elem_size_scopes: Vec<std::collections::HashMap<String, Exp>>,
    /// Function-specific alignment attributes for GNU __alignof__(function).
    function_alignments: std::collections::HashMap<String, usize>,
    /// Extra block items from multi-declarator parsing (e.g., `int x, y;`)
    pending_block_items: Vec<BlockItem>,
    /// Block items that must precede the currently parsed item, such as captured VLA bounds.
    pending_pre_block_items: Vec<BlockItem>,
    /// Extra top-level declarations from multi-declarator parsing
    pending_declarations: Vec<Declaration>,
    /// Alignment specifier collected while parsing declaration specifiers.
    pending_alignment: Option<std::num::NonZeroUsize>,
    /// True when declaration specifiers used GNU __auto_type.
    pending_auto_type: bool,
    /// True when declaration specifiers/attributes mark a function as noreturn.
    pending_noreturn: bool,
    /// True when declaration attributes disable function instrumentation.
    pending_no_instrument_function: bool,
    /// Deprecated attribute collected while parsing the current declarator.
    pending_deprecated_param: Option<Option<String>>,
    /// True when declaration attributes mark the current union typedef transparent.
    pending_transparent_union: bool,
    /// GNU alias attribute collected for the next object declaration.
    pending_alias: Option<String>,
    /// True when declaration specifiers include inline.
    pending_inline: bool,
    /// True when the most recently parsed type specifier was an enum.
    last_type_was_enum: bool,
    /// Last nonconstant array bound parsed for minimal automatic VLA lowering.
    pending_vla_bound: Option<Exp>,
    vla_bound_counter: usize,
    param_parse_depth: usize,
    /// True when the most recent declarator contained a syntactic flexible array bound `[]`.
    pending_flexible_array_bound: bool,
    /// Function currently being parsed, for predefined function-name identifiers.
    current_function_name: Option<String>,
}

impl Parser {
    fn merge_aggregate_attributes(
        prefix: AggregateAttributes,
        suffix: AggregateAttributes,
    ) -> AggregateAttributes {
        AggregateAttributes {
            packed: prefix.packed || suffix.packed,
            transparent_union: prefix.transparent_union || suffix.transparent_union,
            reverse_storage_order: prefix.reverse_storage_order || suffix.reverse_storage_order,
            alignment: match (prefix.alignment, suffix.alignment) {
                (Some(prefix), Some(suffix)) => Some(prefix.max(suffix)),
                (Some(prefix), None) => Some(prefix),
                (None, Some(suffix)) => Some(suffix),
                (None, None) => None,
            },
        }
    }

    fn merge_member_attributes(
        prefix: MemberAttributes,
        suffix: MemberAttributes,
    ) -> MemberAttributes {
        MemberAttributes {
            packed: prefix.packed || suffix.packed,
            alignment: match (prefix.alignment, suffix.alignment) {
                (Some(prefix), Some(suffix)) => Some(prefix.max(suffix)),
                (Some(prefix), None) => Some(prefix),
                (None, Some(suffix)) => Some(suffix),
                (None, None) => None,
            },
        }
    }

    fn consume_post_type_storage_class(
        &mut self,
        mut sc: Option<StorageClass>,
    ) -> ParseResult<Option<StorageClass>> {
        loop {
            match self.peek() {
                Some(Token::KWStatic) if sc.as_ref().is_none_or(StorageClass::is_thread_local) => {
                    self.advance()?;
                    sc = Some(match sc {
                        Some(existing) => existing.with_static(),
                        None => StorageClass::Static,
                    });
                }
                Some(Token::KWExtern) if sc.as_ref().is_none_or(StorageClass::is_thread_local) => {
                    self.advance()?;
                    sc = Some(match sc {
                        Some(existing) => existing.with_extern(),
                        None => StorageClass::Extern,
                    });
                }
                Some(Token::KWThreadLocal) => {
                    self.advance()?;
                    sc = Some(match sc {
                        Some(existing) => existing.with_thread_local(),
                        None => StorageClass::ThreadLocal,
                    });
                }
                _ => break,
            }
        }
        Ok(sc)
    }

    fn mark_pending_transparent_union(&mut self, tag: &str) {
        for declaration in self.pending_struct_decls.iter_mut().rev() {
            if declaration.tag == tag && declaration.is_union {
                declaration.transparent_union = true;
                break;
            }
        }
    }

    pub fn new(tokens: Vec<Token>) -> Self {
        Self::new_with_target(tokens, Target::host())
    }

    pub fn new_with_target(tokens: Vec<Token>, target: Target) -> Self {
        let mut builtin_typedefs = std::collections::HashMap::new();
        builtin_typedefs.insert(
            "__builtin_va_list".to_string(),
            TypedefInfo {
                base_type: CType::Pointer,
                full_type: FullType::Pointer(Box::new(FullType::Scalar(CType::Char))),
                struct_tag: None,
                is_enum: false,
                vla_size: None,
                alignment: None,
            },
        );
        builtin_typedefs.insert(
            "__gnuc_va_list".to_string(),
            TypedefInfo {
                base_type: CType::Pointer,
                full_type: FullType::Pointer(Box::new(FullType::Scalar(CType::Char))),
                struct_tag: None,
                is_enum: false,
                vla_size: None,
                alignment: None,
            },
        );
        builtin_typedefs.insert(
            "__int128_t".to_string(),
            TypedefInfo {
                base_type: CType::Int128,
                full_type: FullType::Scalar(CType::Int128),
                struct_tag: None,
                is_enum: false,
                vla_size: None,
                alignment: None,
            },
        );
        builtin_typedefs.insert(
            "__uint128_t".to_string(),
            TypedefInfo {
                base_type: CType::UInt128,
                full_type: FullType::Scalar(CType::UInt128),
                struct_tag: None,
                is_enum: false,
                vla_size: None,
                alignment: None,
            },
        );

        Parser {
            tokens,
            pos: 0,
            target,
            last_struct_tag: None,
            typedef_scopes: vec![builtin_typedefs],
            pending_struct_decls: Vec::new(),
            struct_defs: IndexMap::new(),
            struct_member_vla_elem_sizes: IndexMap::new(),
            pending_struct_member_vla_elem_sizes: Vec::new(),
            last_typedef_full_type: None,
            last_typedef_vla_size: None,
            last_type_name_vla_size: None,
            enum_scopes: vec![std::collections::HashMap::new()],
            value_scopes: vec![std::collections::HashMap::new()],
            value_vla_size_scopes: vec![std::collections::HashMap::new()],
            value_vla_elem_size_scopes: vec![std::collections::HashMap::new()],
            function_alignments: std::collections::HashMap::new(),
            pending_block_items: Vec::new(),
            pending_pre_block_items: Vec::new(),
            pending_declarations: Vec::new(),
            pending_alignment: None,
            pending_auto_type: false,
            pending_noreturn: false,
            pending_no_instrument_function: false,
            pending_deprecated_param: None,
            pending_transparent_union: false,
            pending_alias: None,
            pending_inline: false,
            last_type_was_enum: false,
            pending_vla_bound: None,
            vla_bound_counter: 0,
            param_parse_depth: 0,
            pending_flexible_array_bound: false,
            current_function_name: None,
        }
    }

    fn long_double_ctype(&self) -> CType {
        if self.target.long_double_size() > CType::Double.size() as usize {
            CType::LongDouble
        } else {
            CType::Double
        }
    }

    fn push_typedef_scope(&mut self) {
        self.typedef_scopes.push(std::collections::HashMap::new());
        self.enum_scopes.push(std::collections::HashMap::new());
        self.value_scopes.push(std::collections::HashMap::new());
        self.value_vla_size_scopes
            .push(std::collections::HashMap::new());
        self.value_vla_elem_size_scopes
            .push(std::collections::HashMap::new());
    }

    fn pop_typedef_scope(&mut self) {
        self.typedef_scopes.pop();
        self.enum_scopes.pop();
        self.value_scopes.pop();
        self.value_vla_size_scopes.pop();
        self.value_vla_elem_size_scopes.pop();
    }

    fn add_typedef(&mut self, name: String, info: TypedefInfo) -> ParseResult<()> {
        let Some(scope) = self.typedef_scopes.last_mut() else {
            return Err(self.format_error("parser typedef scope stack is empty"));
        };
        scope.insert(name, info);
        Ok(())
    }

    fn lookup_visible_typedef(&self, name: &str) -> Option<&TypedefInfo> {
        for index in (0..self.typedef_scopes.len()).rev() {
            if self
                .value_scopes
                .get(index)
                .is_some_and(|scope| scope.contains_key(name))
            {
                return None;
            }
            if let Some(info) = self.typedef_scopes[index].get(name) {
                return Some(info);
            }
        }
        None
    }

    fn is_typedef_name(&self, name: &str) -> bool {
        self.lookup_visible_typedef(name).is_some()
    }

    fn is_builtin_float_type_name(name: &str) -> bool {
        matches!(
            name,
            "_Float16"
                | "_Float32"
                | "_Float64"
                | "_Float128"
                | "_Float32x"
                | "_Float64x"
                | "_Float128x"
                | "_Decimal32"
                | "_Decimal64"
                | "_Decimal128"
                | "__bf16"
                | "__float128"
                | "__float80"
                | "__fp16"
        )
    }

    fn is_complex_type_name(name: &str) -> bool {
        name == "_Complex" || name == "__complex" || name == "__complex__"
    }

    fn complex_full_type(elem: CType) -> FullType {
        FullType::Vector {
            elem: Box::new(FullType::Scalar(elem)),
            lanes: 2,
            complex: true,
        }
    }

    fn is_builtin_int128_type_name(name: &str) -> bool {
        name == "__int128" || name == "__int128__"
    }

    fn nullptr_expression() -> Exp {
        Exp::Cast(
            CType::Pointer,
            Some(FullType::Pointer(Box::new(FullType::Scalar(CType::Void)))),
            Box::new(Exp::Constant(0)),
        )
    }

    fn is_gnu_qualifier_name(name: &str) -> bool {
        matches!(
            name,
            "__const" | "__const__" | "__volatile" | "__volatile__" | "__restrict" | "__restrict__"
        )
    }

    fn is_declarator_qualifier(&self) -> bool {
        matches!(
            self.peek(),
            Some(Token::KWConst)
                | Some(Token::KWVolatile)
                | Some(Token::KWRestrict)
                | Some(Token::KWAtomic)
                | Some(Token::AttributeAligned(_))
                | Some(Token::AttributeAlignedNoreturn(_))
                | Some(Token::AttributeNoreturn)
                | Some(Token::AttributeNoInstrumentFunction)
                | Some(Token::AttributeDeprecated(_))
                | Some(Token::AttributePacked)
                | Some(Token::AttributePackedAligned(_))
                | Some(Token::AttributePackedAlignedNoreturn(_))
                | Some(Token::AttributeTransparentUnion)
                | Some(Token::AttributeMode(_))
                | Some(Token::AttributeScalarStorageOrderReverse)
        ) || matches!(
            self.peek(),
            Some(Token::Identifier(name)) if Self::is_gnu_qualifier_name(name)
        )
    }

    fn consume_declarator_qualifiers(&mut self) -> ParseResult<()> {
        while self.is_declarator_qualifier() {
            if self.at(&Token::AttributeTransparentUnion) {
                self.pending_transparent_union = true;
            }
            if let Some(Token::AttributeDeprecated(message)) = self.peek().cloned() {
                self.pending_deprecated_param = Some(message);
            }
            self.advance()?;
        }
        Ok(())
    }

    fn make_var_decl(
        &mut self,
        name: String,
        full_type: &FullType,
        ctype: CType,
        pi: Option<(CType, usize)>,
        sc: Option<StorageClass>,
        alignment: Option<std::num::NonZeroUsize>,
    ) -> ParseResult<VarDeclaration> {
        let array_dims = Self::extract_array_dims(full_type);
        let is_extern_void_symbol =
            ctype == CType::Void && sc.as_ref().is_some_and(StorageClass::is_extern);
        if ctype == CType::Void && array_dims.is_none() && !is_extern_void_symbol {
            return Err(self.format_error("cannot declare variable with void type"));
        }
        let init = if self.eat(&Token::Assign) {
            if self.at(&Token::OpenBrace) {
                Some(self.parse_array_init()?)
            } else {
                Some(self.parse_assignment()?) // assignment-expr, not full expr (avoids comma operator)
            }
        } else {
            None
        };
        let full_type = self.infer_unsized_array_type(full_type.clone(), init.as_ref())?;
        let array_dims = Self::extract_array_dims(&full_type);
        if let (Some(bound), Some(_)) = (self.pending_vla_bound.take(), array_dims.as_ref()) {
            let elem = match &full_type {
                FullType::Array { elem, .. } => elem.as_ref().clone(),
                other => other.clone(),
            };
            let size_exp = Self::vla_size_expr_from_bound(bound.clone(), &full_type)
                .unwrap_or_else(|| {
                    Exp::Binary(
                        BinaryOp::Mul,
                        Box::new(bound),
                        Box::new(Exp::SizeOfType(elem.to_ctype(), elem.clone())),
                    )
                });
            let ptr_ft = FullType::Pointer(Box::new(elem));
            let ptr_info = match &ptr_ft {
                FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                _ => None,
            };
            if let Some(alignment) = alignment {
                self.function_alignments
                    .insert(name.clone(), alignment.get());
            }
            self.add_value_vla_size(name.clone(), size_exp.clone())?;
            return Ok(VarDeclaration {
                name,
                var_type: CType::Pointer,
                ptr_info,
                array_dims: None,
                decl_full_type: Some(ptr_ft),
                dynamic_size: Some(Box::new(size_exp.clone())),
                init: Some(Exp::FunctionCall("alloca".to_string(), vec![size_exp])),
                storage_class: sc,
                alignment,
                alias: self.pending_alias.take(),
            });
        }
        let dynamic_size = self.dynamic_size_expr_for_decl_type(&full_type);
        let var_type = if array_dims.is_some() {
            let mut t = &full_type;
            while let FullType::Array { elem, .. } = t {
                t = elem;
            }
            t.to_ctype()
        } else {
            ctype
        };
        if let Some(alignment) = alignment {
            self.function_alignments
                .insert(name.clone(), alignment.get());
        }
        Ok(VarDeclaration {
            name,
            var_type,
            ptr_info: pi,
            array_dims,
            decl_full_type: Some(full_type),
            dynamic_size: dynamic_size.map(Box::new),
            init,
            storage_class: sc,
            alignment,
            alias: self.pending_alias.take(),
        })
    }

    fn make_auto_var_decl(
        &mut self,
        name: String,
        sc: Option<StorageClass>,
        alignment: Option<std::num::NonZeroUsize>,
    ) -> ParseResult<VarDeclaration> {
        self.expect_token(Token::Assign)?;
        let init = if self.at(&Token::OpenBrace) {
            return Err(self.format_error("__auto_type requires an expression initializer"));
        } else {
            self.parse_assignment()?
        };
        let full_type = self.typeof_expression(&init)?.decay();
        if full_type.to_ctype() == CType::Void {
            return Err(self.format_error("__auto_type cannot infer void type"));
        }
        let pi = match &full_type {
            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
            _ => None,
        };
        Ok(VarDeclaration {
            name,
            var_type: full_type.to_ctype(),
            ptr_info: pi,
            array_dims: Self::extract_array_dims(&full_type),
            decl_full_type: Some(full_type),
            dynamic_size: None,
            init: Some(init),
            storage_class: sc,
            alignment,
            alias: None,
        })
    }

    fn var_decl_full_type(&self, decl: &VarDeclaration) -> ParseResult<FullType> {
        decl.decl_full_type.clone().ok_or_else(|| {
            self.format_error("internal parser error: declaration is missing full type")
        })
    }

    fn infer_unsized_array_type(
        &self,
        full_type: FullType,
        init: Option<&Exp>,
    ) -> ParseResult<FullType> {
        let FullType::Array { elem, size } = full_type else {
            return Ok(full_type);
        };
        if size != 0 {
            return Ok(FullType::Array { elem, size });
        }
        let Some(init) = init else {
            return Ok(FullType::Array { elem, size });
        };
        let inferred = match init {
            Exp::StringLiteral(s) if elem.to_ctype().is_char() => c_string_byte_len(s) + 1,
            Exp::WideStringLiteral(s) => s.chars().count() + 1,
            Exp::Utf16StringLiteral(s) => s.encode_utf16().count() + 1,
            Exp::Utf32StringLiteral(s) => s.chars().count() + 1,
            Exp::StringLiteral(s) => s.chars().count() + 1,
            Exp::ArrayInit(elems) if elem.to_ctype().is_char() => match elems.as_slice() {
                [Exp::StringLiteral(s)] => c_string_byte_len(s) + 1,
                _ => self.infer_array_init_len(elems)?,
            },
            Exp::ArrayInit(elems) => match elems.as_slice() {
                [Exp::WideStringLiteral(s)] => s.chars().count() + 1,
                [Exp::Utf16StringLiteral(s)] => s.encode_utf16().count() + 1,
                [Exp::Utf32StringLiteral(s)] => s.chars().count() + 1,
                [Exp::StringLiteral(s)] => s.chars().count() + 1,
                _ if matches!(elem.as_ref(), FullType::Struct(_)) => {
                    self.infer_struct_array_init_len(elem.as_ref(), elems)?
                }
                _ => self.infer_array_init_len(elems)?,
            },
            _ => 0,
        };
        Ok(FullType::Array {
            elem,
            size: inferred,
        })
    }

    fn infer_array_init_len(&self, elems: &[Exp]) -> ParseResult<usize> {
        let mut next = 0usize;
        let mut max_len = 0usize;
        for elem in elems {
            let index = if let Exp::DesignatedInit(designators, _) = elem {
                match designators.first() {
                    Some(Designator::Index(index_exp)) => {
                        let value = self
                            .eval_integer_constant_exp_with_layout(index_exp)
                            .ok_or_else(|| {
                                self.format_error("array designator must be constant")
                            })?;
                        if value < 0 {
                            return Err(self.format_error("array designator must be non-negative"));
                        }
                        value as usize
                    }
                    Some(Designator::IndexRange(_, end_exp)) => {
                        let value = self
                            .eval_integer_constant_exp_with_layout(end_exp)
                            .ok_or_else(|| {
                                self.format_error("array designator must be constant")
                            })?;
                        if value < 0 {
                            return Err(self.format_error("array designator must be non-negative"));
                        }
                        value as usize
                    }
                    _ => next,
                }
            } else {
                next
            };
            max_len = max_len.max(index + 1);
            next = index + 1;
        }
        Ok(max_len)
    }

    fn infer_struct_array_init_len(&self, elem: &FullType, elems: &[Exp]) -> ParseResult<usize> {
        let FullType::Struct(tag) = elem else {
            return self.infer_array_init_len(elems);
        };
        let Some(def) = self.struct_defs.get(tag) else {
            return self.infer_array_init_len(elems);
        };
        let max_members = if def.is_union {
            1
        } else {
            def.members.len().max(1)
        };
        let mut next = 0usize;
        let mut max_len = 0usize;
        let mut member_index = 0usize;
        for elem in elems {
            let index = if let Exp::DesignatedInit(designators, _) = elem {
                match designators.first() {
                    Some(Designator::Index(index_exp)) => {
                        let value = self
                            .eval_integer_constant_exp_with_layout(index_exp)
                            .ok_or_else(|| {
                                self.format_error("array designator must be constant")
                            })?;
                        if value < 0 {
                            return Err(self.format_error("array designator must be non-negative"));
                        }
                        member_index = 0;
                        value as usize
                    }
                    Some(Designator::IndexRange(_, end_exp)) => {
                        let value = self
                            .eval_integer_constant_exp_with_layout(end_exp)
                            .ok_or_else(|| {
                                self.format_error("array designator must be constant")
                            })?;
                        if value < 0 {
                            return Err(self.format_error("array designator must be non-negative"));
                        }
                        member_index = 0;
                        value as usize
                    }
                    _ => next,
                }
            } else {
                next
            };
            max_len = max_len.max(index + 1);
            if matches!(elem, Exp::ArrayInit(_)) {
                next = index + 1;
                member_index = 0;
            } else {
                member_index += 1;
                if member_index >= max_members {
                    next = index + 1;
                    member_index = 0;
                } else {
                    next = index;
                }
            }
        }
        Ok(max_len)
    }

    fn function_full_type(return_type: FullType, info: &FunctionDeclaratorInfo) -> FullType {
        FullType::Function {
            return_type: Box::new(return_type),
            params: info.param_full_types.clone(),
            variadic: info.variadic,
        }
    }

    fn param_value_types(params: &[ParamDecl], full_types: &[FullType]) -> Vec<(String, FullType)> {
        params
            .iter()
            .zip(full_types.iter())
            .filter_map(|((name, _, _), full_type)| {
                if name.starts_with("__unnamed_") {
                    None
                } else {
                    Some((name.clone(), full_type.clone()))
                }
            })
            .collect()
    }

    fn add_enum_constant(&mut self, name: String, value: i64) -> ParseResult<()> {
        let Some(scope) = self.enum_scopes.last_mut() else {
            return Err(self.format_error("parser enum scope stack is empty"));
        };
        scope.insert(name, value);
        Ok(())
    }

    fn lookup_enum_constant(&self, name: &str) -> Option<i64> {
        for scope in self.enum_scopes.iter().rev() {
            if let Some(&val) = scope.get(name) {
                return Some(val);
            }
        }
        None
    }

    fn add_value_type(&mut self, name: String, full_type: FullType) -> ParseResult<()> {
        let full_type = if let FullType::Array { elem, size: 0 } = &full_type {
            match self.lookup_value_type(&name) {
                Some(FullType::Array {
                    elem: existing_elem,
                    size,
                }) if size > 0 && existing_elem.as_ref() == elem.as_ref() => FullType::Array {
                    elem: existing_elem,
                    size,
                },
                _ => full_type,
            }
        } else {
            full_type
        };
        let Some(scope) = self.value_scopes.last_mut() else {
            return Err(self.format_error("parser value scope stack is empty"));
        };
        scope.insert(name, full_type);
        Ok(())
    }

    fn lookup_value_type(&self, name: &str) -> Option<FullType> {
        for (index, scope) in self.value_scopes.iter().enumerate().rev() {
            if let Some(full_type) = scope.get(name) {
                if let FullType::Array { elem, size: 0 } = full_type {
                    for outer in self.value_scopes[..index].iter().rev() {
                        if let Some(FullType::Array {
                            elem: existing_elem,
                            size,
                        }) = outer.get(name)
                        {
                            if *size > 0 && existing_elem.as_ref() == elem.as_ref() {
                                return Some(FullType::Array {
                                    elem: existing_elem.clone(),
                                    size: *size,
                                });
                            }
                        }
                    }
                }
                return Some(full_type.clone());
            }
        }
        None
    }

    fn add_value_vla_size(&mut self, name: String, size: Exp) -> ParseResult<()> {
        let Some(scope) = self.value_vla_size_scopes.last_mut() else {
            return Err(self.format_error("parser value scope stack is empty"));
        };
        scope.insert(name, size);
        Ok(())
    }

    fn lookup_value_vla_size(&self, name: &str) -> Option<Exp> {
        for scope in self.value_vla_size_scopes.iter().rev() {
            if let Some(size) = scope.get(name) {
                return Some(size.clone());
            }
        }
        None
    }

    fn add_value_vla_elem_size(&mut self, name: String, size: Exp) -> ParseResult<()> {
        let Some(scope) = self.value_vla_elem_size_scopes.last_mut() else {
            return Err(self.format_error("parser value VLA elem-size scope stack is empty"));
        };
        scope.insert(name, size);
        Ok(())
    }

    fn lookup_value_vla_elem_size(&self, name: &str) -> Option<Exp> {
        for scope in self.value_vla_elem_size_scopes.iter().rev() {
            if let Some(size) = scope.get(name) {
                return Some(size.clone());
            }
        }
        None
    }

    fn record_struct_definition(&mut self, sd: &StructDeclaration) -> ParseResult<()> {
        let def = StructDef::from_declaration(sd, &self.struct_defs)
            .map_err(|err| self.format_error(&err))?;
        self.struct_defs.insert(sd.tag.clone(), def);
        Ok(())
    }

    fn record_struct_member_vla_elem_sizes(
        &mut self,
        tag: &str,
        sizes: Vec<StructMemberVlaElemSize>,
    ) {
        for size in sizes {
            self.struct_member_vla_elem_sizes
                .insert((tag.to_string(), size.member), size.elem_size);
        }
    }

    fn add_offset_exp(left: Exp, right: Exp) -> Exp {
        if matches!(left, Exp::ULongConstant(0)) {
            right
        } else if matches!(right, Exp::ULongConstant(0)) {
            left
        } else {
            Exp::Binary(BinaryOp::Add, Box::new(left), Box::new(right))
        }
    }

    fn offsetof_member_designator(&mut self, base_type: FullType) -> ParseResult<Exp> {
        let mut current_type = base_type;
        let mut offset = Exp::ULongConstant(0);
        let first_member = self.parse_identifier()?;
        let (member_offset, member_dynamic_elem_size) =
            self.offsetof_member_step(&mut current_type, &first_member)?;
        offset = Self::add_offset_exp(offset, Exp::ULongConstant(member_offset as i64));
        let mut dynamic_elem_size = member_dynamic_elem_size;

        loop {
            if self.eat(&Token::Dot) {
                let member = self.parse_identifier()?;
                let (member_offset, member_dynamic_elem_size) =
                    self.offsetof_member_step(&mut current_type, &member)?;
                offset = Self::add_offset_exp(offset, Exp::ULongConstant(member_offset as i64));
                dynamic_elem_size = member_dynamic_elem_size;
            } else if self.eat(&Token::OpenBracket) {
                let index_exp = self.parse_expression()?;
                if let Some(index) = self.eval_integer_constant_exp_with_layout(&index_exp) {
                    if index < 0 {
                        return Err(self.format_error("offsetof array index may not be negative"));
                    }
                }
                let FullType::Array { elem, .. } = current_type.clone() else {
                    return Err(self.format_error("offsetof array index requires an array member"));
                };
                let elem_size = dynamic_elem_size.take().unwrap_or_else(|| {
                    Exp::ULongConstant(elem.byte_size_with(&self.struct_defs) as i64)
                });
                offset = Self::add_offset_exp(
                    offset,
                    Exp::Binary(BinaryOp::Mul, Box::new(index_exp), Box::new(elem_size)),
                );
                current_type = *elem;
                self.expect_token(Token::CloseBracket)?;
            } else {
                break;
            }
        }

        Ok(offset)
    }

    fn offsetof_member_step(
        &self,
        current_type: &mut FullType,
        member: &str,
    ) -> ParseResult<(usize, Option<Exp>)> {
        let FullType::Struct(tag) = current_type else {
            return Err(self.format_error("offsetof member access requires a struct or union"));
        };
        let tag_name = tag.clone();
        let def = self
            .struct_defs
            .get(tag)
            .ok_or_else(|| self.format_error(&format!("undefined struct or union '{}'", tag)))?;
        let mem = def
            .find_member(member)
            .ok_or_else(|| self.format_error(&format!("no member '{}' in {}", member, tag)))?;
        if mem.bit_width.is_some() {
            return Err(self.format_error("offsetof may not name a bit-field"));
        }
        *current_type = mem.member_full_type.clone();
        let dynamic_elem_size = self
            .struct_member_vla_elem_sizes
            .get(&(tag_name, member.to_string()))
            .cloned();
        Ok((mem.offset, dynamic_elem_size))
    }

    fn member_expression_type(&self, base_type: &FullType, member: &str) -> ParseResult<FullType> {
        let FullType::Struct(tag) = base_type else {
            return Err(self.format_error("member access requires a struct or union expression"));
        };
        let def = self
            .struct_defs
            .get(tag)
            .ok_or_else(|| self.format_error(&format!("undefined struct or union '{}'", tag)))?;
        let mem = def
            .find_member(member)
            .ok_or_else(|| self.format_error(&format!("no member '{}' in {}", member, tag)))?;
        Ok(mem.member_full_type.clone())
    }

    fn atomic_pointee_expression_type(&self, ptr: &Exp) -> ParseResult<FullType> {
        match self.typeof_expression(ptr)?.decay() {
            FullType::Pointer(inner) => Ok(*inner),
            _ => Ok(FullType::Scalar(CType::Int)),
        }
    }

    fn eval_integer_constant_exp_with_layout(&self, exp: &Exp) -> Option<i64> {
        self.eval_integer_constant_value_with_layout(exp)
            .map(|constant| constant.value)
    }

    fn integer_constant_target_unsigned(ctype: CType) -> bool {
        matches!(
            ctype,
            CType::Bool
                | CType::UChar
                | CType::UShort
                | CType::UInt
                | CType::ULong
                | CType::UInt128
        )
    }

    fn cast_integer_constant(value: IntegerConstantValue, target: CType) -> IntegerConstantValue {
        let converted = match target {
            CType::Char | CType::SChar => value.value as i8 as i64,
            CType::UChar => value.value as u8 as i64,
            CType::Short => value.value as i16 as i64,
            CType::UShort => value.value as u16 as i64,
            CType::Bool => (value.value != 0) as i64,
            CType::Int => value.value as i32 as i64,
            CType::UInt => value.value as u32 as i64,
            CType::Long => value.value,
            CType::ULong => value.value as u64 as i64,
            CType::Int128 => value.value,
            CType::UInt128 => value.value as u64 as i64,
            _ => value.value,
        };
        IntegerConstantValue {
            value: converted,
            is_unsigned: Self::integer_constant_target_unsigned(target),
        }
    }

    fn unsigned_integer_constant_value(value: IntegerConstantValue, ctype: CType) -> u64 {
        if ctype.size() <= CType::UInt.size() {
            value.value as u32 as u64
        } else {
            value.value as u64
        }
    }

    fn integer_constant_from_unsigned(value: u64, ctype: CType) -> i64 {
        if ctype.size() <= CType::UInt.size() {
            value as u32 as i64
        } else {
            value as i64
        }
    }

    fn binary_constant_operand_type(
        &self,
        op: &BinaryOp,
        left: &Exp,
        right: &Exp,
    ) -> Option<CType> {
        let left_type = self.typeof_expression(left).ok()?.to_ctype();
        let right_type = self.typeof_expression(right).ok()?.to_ctype();
        if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
            Some(left_type.promote())
        } else {
            Some(CType::common(left_type, right_type))
        }
    }

    fn binary_constant_result_is_int(op: &BinaryOp) -> bool {
        matches!(
            op,
            BinaryOp::LogicalAnd
                | BinaryOp::LogicalOr
                | BinaryOp::Equal
                | BinaryOp::NotEqual
                | BinaryOp::LessThan
                | BinaryOp::GreaterThan
                | BinaryOp::LessEqual
                | BinaryOp::GreaterEqual
        )
    }

    fn eval_integer_constant_value_with_layout(&self, exp: &Exp) -> Option<IntegerConstantValue> {
        match exp {
            Exp::SizeOf(inner) => {
                let ft = self.typeof_expression(inner).ok()?;
                Some(IntegerConstantValue::unsigned(
                    ft.byte_size_with(&self.struct_defs) as i64,
                ))
            }
            Exp::Cast(target, _, inner) => {
                let value = self.eval_integer_constant_value_with_layout(inner)?;
                Some(Self::cast_integer_constant(value, *target))
            }
            Exp::Unary(op, inner) => {
                let value = self.eval_integer_constant_value_with_layout(inner)?;
                match op {
                    UnaryOp::Negate => Some(IntegerConstantValue {
                        value: value.value.wrapping_neg(),
                        is_unsigned: value.is_unsigned,
                    }),
                    UnaryOp::Complement => {
                        let result_type = self
                            .typeof_expression(exp)
                            .ok()
                            .map(|ft| ft.to_ctype())
                            .unwrap_or(CType::Long);
                        Some(Self::cast_integer_constant(
                            IntegerConstantValue {
                                value: !value.value,
                                is_unsigned: value.is_unsigned,
                            },
                            result_type,
                        ))
                    }
                    UnaryOp::LogicalNot => {
                        Some(IntegerConstantValue::signed((value.value == 0) as i64))
                    }
                    _ => None,
                }
            }
            Exp::Binary(op, left, right) => {
                let left_value = self.eval_integer_constant_value_with_layout(left)?;
                let right_value = self.eval_integer_constant_value_with_layout(right)?;
                let op_type = self.binary_constant_operand_type(op, left, right);
                let use_unsigned = op_type.is_some_and(|ctype| !ctype.is_signed())
                    || left_value.is_unsigned
                    || right_value.is_unsigned;
                if use_unsigned {
                    let op_type = op_type.unwrap_or(CType::ULong);
                    let left = Self::unsigned_integer_constant_value(left_value, op_type);
                    let right = Self::unsigned_integer_constant_value(right_value, op_type);
                    let value = match op {
                        BinaryOp::Add => left.wrapping_add(right),
                        BinaryOp::Sub => left.wrapping_sub(right),
                        BinaryOp::Mul => left.wrapping_mul(right),
                        BinaryOp::Div => {
                            if right == 0 {
                                return None;
                            }
                            left / right
                        }
                        BinaryOp::Mod => {
                            if right == 0 {
                                return None;
                            }
                            left % right
                        }
                        BinaryOp::BitwiseAnd => left & right,
                        BinaryOp::BitwiseNand => !(left & right),
                        BinaryOp::BitwiseOr => left | right,
                        BinaryOp::BitwiseXor => left ^ right,
                        BinaryOp::ShiftLeft => {
                            let amount = u32::try_from(right_value.value).ok()?;
                            left.checked_shl(amount)?
                        }
                        BinaryOp::ShiftRight => {
                            let amount = u32::try_from(right_value.value).ok()?;
                            left.checked_shr(amount)?
                        }
                        BinaryOp::LogicalAnd => (left != 0 && right != 0) as u64,
                        BinaryOp::LogicalOr => (left != 0 || right != 0) as u64,
                        BinaryOp::Equal => (left == right) as u64,
                        BinaryOp::NotEqual => (left != right) as u64,
                        BinaryOp::LessThan => (left < right) as u64,
                        BinaryOp::GreaterThan => (left > right) as u64,
                        BinaryOp::LessEqual => (left <= right) as u64,
                        BinaryOp::GreaterEqual => (left >= right) as u64,
                    };
                    return Some(IntegerConstantValue {
                        value: Self::integer_constant_from_unsigned(value, op_type),
                        is_unsigned: !Self::binary_constant_result_is_int(op),
                    });
                }
                let left = left_value.value;
                let right = right_value.value;
                let value = match op {
                    BinaryOp::Add => left.wrapping_add(right),
                    BinaryOp::Sub => left.wrapping_sub(right),
                    BinaryOp::Mul => left.wrapping_mul(right),
                    BinaryOp::Div => left.checked_div(right)?,
                    BinaryOp::Mod => left.checked_rem(right)?,
                    BinaryOp::BitwiseAnd => left & right,
                    BinaryOp::BitwiseNand => !(left & right),
                    BinaryOp::BitwiseOr => left | right,
                    BinaryOp::BitwiseXor => left ^ right,
                    BinaryOp::ShiftLeft => {
                        let amount = u32::try_from(right).ok()?;
                        left.checked_shl(amount)?
                    }
                    BinaryOp::ShiftRight => {
                        let amount = u32::try_from(right).ok()?;
                        left.checked_shr(amount)?
                    }
                    BinaryOp::LogicalAnd => (left != 0 && right != 0) as i64,
                    BinaryOp::LogicalOr => (left != 0 || right != 0) as i64,
                    BinaryOp::Equal => (left == right) as i64,
                    BinaryOp::NotEqual => (left != right) as i64,
                    BinaryOp::LessThan => (left < right) as i64,
                    BinaryOp::GreaterThan => (left > right) as i64,
                    BinaryOp::LessEqual => (left <= right) as i64,
                    BinaryOp::GreaterEqual => (left >= right) as i64,
                };
                Some(IntegerConstantValue::signed(value))
            }
            Exp::Conditional(cond, then_exp, else_exp) => {
                if self.eval_integer_constant_value_with_layout(cond)?.value != 0 {
                    self.eval_integer_constant_value_with_layout(then_exp)
                } else {
                    self.eval_integer_constant_value_with_layout(else_exp)
                }
            }
            _ => Self::eval_integer_constant_exp_with_defs(exp, &self.struct_defs)
                .map(IntegerConstantValue::signed),
        }
    }

    fn eval_integer_constant_exp_with_defs(
        exp: &Exp,
        struct_defs: &IndexMap<String, StructDef>,
    ) -> Option<i64> {
        match exp {
            Exp::Constant(c)
            | Exp::LongConstant(c)
            | Exp::UIntConstant(c)
            | Exp::ULongConstant(c) => Some(*c),
            Exp::Int128Constant(c) => i64::try_from(*c).ok(),
            Exp::UInt128Constant(c) => i64::try_from(*c).ok(),
            Exp::SizeOfType(_, ft) => Some(ft.byte_size_with(struct_defs) as i64),
            Exp::AlignOfType(ft) => Some(ft.alignment_with(struct_defs) as i64),
            Exp::Cast(_, _, inner) => Self::eval_integer_constant_exp_with_defs(inner, struct_defs),
            Exp::Unary(op, inner) => {
                let value = Self::eval_integer_constant_exp_with_defs(inner, struct_defs)?;
                match op {
                    UnaryOp::Negate => Some(-value),
                    UnaryOp::Complement => Some(!value),
                    UnaryOp::LogicalNot => Some((value == 0) as i64),
                    _ => None,
                }
            }
            Exp::Binary(op, left, right) => {
                let left = Self::eval_integer_constant_exp_with_defs(left, struct_defs)?;
                let right = Self::eval_integer_constant_exp_with_defs(right, struct_defs)?;
                match op {
                    BinaryOp::Add => Some(left.wrapping_add(right)),
                    BinaryOp::Sub => Some(left.wrapping_sub(right)),
                    BinaryOp::Mul => Some(left.wrapping_mul(right)),
                    BinaryOp::Div => left.checked_div(right),
                    BinaryOp::Mod => left.checked_rem(right),
                    BinaryOp::BitwiseAnd => Some(left & right),
                    BinaryOp::BitwiseNand => Some(!(left & right)),
                    BinaryOp::BitwiseOr => Some(left | right),
                    BinaryOp::BitwiseXor => Some(left ^ right),
                    BinaryOp::ShiftLeft => u32::try_from(right)
                        .ok()
                        .and_then(|amount| left.checked_shl(amount)),
                    BinaryOp::ShiftRight => u32::try_from(right)
                        .ok()
                        .and_then(|amount| left.checked_shr(amount)),
                    BinaryOp::LogicalAnd => Some((left != 0 && right != 0) as i64),
                    BinaryOp::LogicalOr => Some((left != 0 || right != 0) as i64),
                    BinaryOp::Equal => Some((left == right) as i64),
                    BinaryOp::NotEqual => Some((left != right) as i64),
                    BinaryOp::LessThan => Some((left < right) as i64),
                    BinaryOp::GreaterThan => Some((left > right) as i64),
                    BinaryOp::LessEqual => Some((left <= right) as i64),
                    BinaryOp::GreaterEqual => Some((left >= right) as i64),
                }
            }
            Exp::Conditional(cond, then_exp, else_exp) => {
                if Self::eval_integer_constant_exp_with_defs(cond, struct_defs)? != 0 {
                    Self::eval_integer_constant_exp_with_defs(then_exp, struct_defs)
                } else {
                    Self::eval_integer_constant_exp_with_defs(else_exp, struct_defs)
                }
            }
            _ => None,
        }
    }

    fn parse_enum_fixed_underlying_type(&mut self) -> ParseResult<()> {
        if self.eat(&Token::Colon) {
            let _ = self.parse_type()?;
        }
        Ok(())
    }

    fn atomic_fetch_op(name: &str) -> Option<BinaryOp> {
        match name {
            "__atomic_add_fetch"
            | "__atomic_fetch_add"
            | "__sync_add_and_fetch"
            | "__sync_fetch_and_add" => Some(BinaryOp::Add),
            "__atomic_sub_fetch"
            | "__atomic_fetch_sub"
            | "__sync_sub_and_fetch"
            | "__sync_fetch_and_sub" => Some(BinaryOp::Sub),
            "__atomic_and_fetch"
            | "__atomic_fetch_and"
            | "__sync_and_and_fetch"
            | "__sync_fetch_and_and" => Some(BinaryOp::BitwiseAnd),
            "__atomic_nand_fetch"
            | "__atomic_fetch_nand"
            | "__sync_nand_and_fetch"
            | "__sync_fetch_and_nand" => Some(BinaryOp::BitwiseNand),
            "__atomic_or_fetch"
            | "__atomic_fetch_or"
            | "__sync_or_and_fetch"
            | "__sync_fetch_and_or" => Some(BinaryOp::BitwiseOr),
            "__atomic_xor_fetch"
            | "__atomic_fetch_xor"
            | "__sync_xor_and_fetch"
            | "__sync_fetch_and_xor" => Some(BinaryOp::BitwiseXor),
            _ => None,
        }
    }

    fn atomic_fetch_returns_old(name: &str) -> bool {
        name.starts_with("__atomic_fetch_") || name.starts_with("__sync_fetch_and_")
    }

    fn ordered_atomic_builtin_exp(exp: Exp) -> Exp {
        Exp::Comma(Box::new(Exp::AtomicFence), Box::new(exp))
    }

    fn int_const(value: i64) -> Exp {
        Exp::Constant(value)
    }

    fn ulong_const(value: u64) -> Exp {
        Exp::ULongConstant(value as i64)
    }

    fn binary_exp(op: BinaryOp, left: Exp, right: Exp) -> Exp {
        Exp::Binary(op, Box::new(left), Box::new(right))
    }

    fn bswap_exp(value: Exp, bits: u8) -> Exp {
        let bytes = bits / 8;
        let mut result: Option<Exp> = None;
        for i in 0..bytes {
            let source_shift = i * 8;
            let dest_shift = (bytes - 1 - i) * 8;
            let masked = Self::binary_exp(
                BinaryOp::BitwiseAnd,
                value.clone(),
                Self::ulong_const(0xffu64 << source_shift),
            );
            let shifted = if dest_shift > source_shift {
                Self::binary_exp(
                    BinaryOp::ShiftLeft,
                    masked,
                    Self::int_const((dest_shift - source_shift) as i64),
                )
            } else if source_shift > dest_shift {
                Self::binary_exp(
                    BinaryOp::ShiftRight,
                    masked,
                    Self::int_const((source_shift - dest_shift) as i64),
                )
            } else {
                masked
            };
            result = Some(if let Some(current) = result {
                Self::binary_exp(BinaryOp::BitwiseOr, current, shifted)
            } else {
                shifted
            });
        }
        result.unwrap_or_else(|| Self::int_const(0))
    }

    fn fortified_builtin_fallback(name: &str, args: &[Exp]) -> Option<ParseResult<Exp>> {
        let (fallback, keep_args) = match name {
            "__builtin___memcpy_chk" => ("__builtin_memcpy", 3),
            "__builtin___memmove_chk" => ("__builtin_memmove", 3),
            "__builtin___memset_chk" => ("__builtin_memset", 3),
            "__builtin___strcpy_chk" => ("__builtin_strcpy", 2),
            "__builtin___stpcpy_chk" => ("__builtin_stpcpy", 2),
            "__builtin___strncpy_chk" => ("__builtin_strncpy", 3),
            "__builtin___strcat_chk" => ("__builtin_strcat", 2),
            "__builtin___strncat_chk" => ("__builtin_strncat", 3),
            _ => return None,
        };
        Some(if args.len() > keep_args {
            Ok(Exp::FunctionCall(
                fallback.to_string(),
                args.iter().take(keep_args).cloned().collect(),
            ))
        } else {
            Err(format!(
                "{} requires at least {} argument(s)",
                name,
                keep_args + 1
            ))
        })
    }

    fn parse_type_name_full(&mut self) -> ParseResult<FullType> {
        self.last_type_name_vla_size = None;
        let base_type = self.parse_type()?;
        let base_struct_tag = if base_type == CType::Struct {
            self.last_struct_tag.clone()
        } else {
            None
        };
        let typedef_full_type = self.last_typedef_full_type.take();
        let typedef_vla_size = self.last_typedef_vla_size.take();
        let type_attrs = self.consume_type_name_attributes()?;
        let tree = self.parse_abstract_decl_tree()?;
        let base_full_type = if let Some(base_full_type) = typedef_full_type {
            base_full_type
        } else {
            FullType::Scalar(base_type)
        };
        let base_full_type = self.apply_vector_size_attr(base_full_type, type_attrs.vector_size);
        let full_type = Self::process_abstract_tree(&tree, base_full_type);
        if matches!(tree, AbstractDecl::Base) {
            self.last_type_name_vla_size = typedef_vla_size;
        }
        Ok(if base_type == CType::Struct {
            if let Some(ref tag) = base_struct_tag {
                Self::replace_scalar_struct(&full_type, tag)
            } else {
                full_type
            }
        } else {
            full_type
        })
    }

    fn gnu_types_compatible(left: &FullType, right: &FullType) -> bool {
        match (left, right) {
            (FullType::Scalar(left), FullType::Scalar(right)) => left == right,
            (FullType::Struct(left), FullType::Struct(right)) => left == right,
            (FullType::Pointer(left), FullType::Pointer(right)) => {
                Self::gnu_types_compatible(left, right)
            }
            (
                FullType::Array {
                    elem: left_elem,
                    size: left_size,
                },
                FullType::Array {
                    elem: right_elem,
                    size: right_size,
                },
            ) => {
                (left_size == right_size || *left_size == 0 || *right_size == 0)
                    && Self::gnu_types_compatible(left_elem, right_elem)
            }
            (
                FullType::Function {
                    return_type: left_return,
                    params: left_params,
                    variadic: left_variadic,
                },
                FullType::Function {
                    return_type: right_return,
                    params: right_params,
                    variadic: right_variadic,
                },
            ) => {
                left_variadic == right_variadic
                    && Self::gnu_types_compatible(left_return, right_return)
                    && left_params.len() == right_params.len()
                    && left_params
                        .iter()
                        .zip(right_params.iter())
                        .all(|(left, right)| Self::gnu_types_compatible(left, right))
            }
            _ => false,
        }
    }

    fn is_type_qualifier_token(token: &Token) -> bool {
        matches!(
            token,
            Token::KWConst | Token::KWVolatile | Token::KWRestrict | Token::KWAtomic
        )
    }

    fn compat_type_meta(&self, full_type: &FullType, tokens: &[Token]) -> CompatTypeMeta {
        let first_star = tokens
            .iter()
            .position(|token| matches!(token, Token::Star))
            .unwrap_or(tokens.len());
        let pointee_qualified = matches!(full_type, FullType::Pointer(_))
            && tokens[..first_star]
                .iter()
                .any(Self::is_type_qualifier_token);
        let long_double = tokens
            .windows(2)
            .any(|pair| matches!(pair, [Token::KWLong, Token::KWDouble]));
        let enum_typedef = match tokens {
            [Token::Identifier(name)] => self
                .lookup_visible_typedef(name)
                .filter(|info| info.is_enum)
                .map(|_| name.clone()),
            [Token::KWTypeOf | Token::KWTypeOfUnqual, Token::OpenParen, Token::Identifier(name), Token::CloseParen] => {
                self.lookup_visible_typedef(name)
                    .filter(|info| info.is_enum)
                    .map(|_| name.clone())
            }
            _ => None,
        };

        let pointee_qualified =
            pointee_qualified && !matches!(tokens.first(), Some(Token::KWTypeOfUnqual));

        CompatTypeMeta {
            pointee_qualified,
            long_double,
            enum_typedef,
        }
    }

    fn gnu_type_meta_compatible(left: &CompatTypeMeta, right: &CompatTypeMeta) -> bool {
        left.pointee_qualified == right.pointee_qualified
            && left.long_double == right.long_double
            && match (&left.enum_typedef, &right.enum_typedef) {
                (Some(left), Some(right)) => left == right,
                (Some(_), None) | (None, Some(_)) => false,
                (None, None) => true,
            }
    }

    fn parse_alignment_specifier(&mut self) -> ParseResult<std::num::NonZeroUsize> {
        self.expect_token(Token::KWAlignAs)?;
        self.expect_token(Token::OpenParen)?;
        let alignment = if self.peek().is_some_and(|tok| self.is_type_keyword(tok)) {
            let ft = self.parse_type_name_full()?;
            Self::validate_alignment_value(ft.alignment_with(&self.struct_defs) as i64)?
        } else {
            let exp = self.parse_assignment()?;
            let value = self
                .eval_integer_constant_exp_with_layout(&exp)
                .ok_or_else(|| self.format_error("expected constant alignment"))?;
            Self::validate_alignment_value(value)?
        };
        self.expect_token(Token::CloseParen)?;
        Ok(alignment)
    }

    fn validate_alignment_value(value: i64) -> ParseResult<std::num::NonZeroUsize> {
        if value <= 0 {
            return Err("alignment must be positive".to_string());
        }
        let alignment = usize::try_from(value).map_err(|_| "alignment is too large".to_string())?;
        if alignment > MAX_SUPPORTED_ALIGNMENT {
            return Err("alignment is too large".to_string());
        }
        if !alignment.is_power_of_two() {
            return Err("alignment must be a power of two".to_string());
        }
        std::num::NonZeroUsize::new(alignment)
            .ok_or_else(|| "alignment must be positive".to_string())
    }

    fn merge_alignment(
        current: Option<std::num::NonZeroUsize>,
        next: std::num::NonZeroUsize,
    ) -> Option<std::num::NonZeroUsize> {
        Some(current.map_or(next, |existing| existing.max(next)))
    }

    fn parse_attribute_alignment(&self, expression: &str) -> ParseResult<std::num::NonZeroUsize> {
        let tokens = lex::lex(expression).map_err(|err| self.format_error(&err))?;
        let mut parser = Parser::new(tokens);
        parser.typedef_scopes = self.typedef_scopes.clone();
        parser.enum_scopes = self.enum_scopes.clone();
        parser.value_scopes = self.value_scopes.clone();
        parser.struct_defs = self.struct_defs.clone();
        let exp = parser.parse_assignment()?;
        if parser.peek().is_some() {
            return Err(self.format_error("unexpected tokens in alignment attribute"));
        }
        let value = parser
            .eval_integer_constant_exp_with_layout(&exp)
            .ok_or_else(|| self.format_error("expected constant alignment"))?;
        Self::validate_alignment_value(value).map_err(|err| self.format_error(&err))
    }

    fn parse_attribute_vector_size(&self, expression: &str) -> ParseResult<usize> {
        let tokens = lex::lex(expression).map_err(|err| self.format_error(&err))?;
        let mut parser = Parser::new(tokens);
        parser.typedef_scopes = self.typedef_scopes.clone();
        parser.enum_scopes = self.enum_scopes.clone();
        parser.value_scopes = self.value_scopes.clone();
        parser.struct_defs = self.struct_defs.clone();
        let exp = parser.parse_assignment()?;
        if parser.peek().is_some() {
            return Err(self.format_error("unexpected tokens in vector_size attribute"));
        }
        let value = parser
            .eval_integer_constant_exp_with_layout(&exp)
            .ok_or_else(|| self.format_error("expected constant vector_size"))?;
        if value <= 0 {
            return Err(self.format_error("vector_size must be positive"));
        }
        Ok(value as usize)
    }

    fn ctype_for_gnu_mode(mode: &str, unsigned: bool) -> Option<CType> {
        let normalized = mode.trim_matches('_').to_ascii_uppercase();
        match normalized.as_str() {
            "QI" => Some(if unsigned { CType::UChar } else { CType::SChar }),
            "HI" => Some(if unsigned {
                CType::UShort
            } else {
                CType::Short
            }),
            "SI" => Some(if unsigned { CType::UInt } else { CType::Int }),
            "DI" => Some(if unsigned { CType::ULong } else { CType::Long }),
            "TI" => Some(if unsigned {
                CType::UInt128
            } else {
                CType::Int128
            }),
            _ => None,
        }
    }

    fn bitint_spec(width: i64, unsigned: bool) -> ParseResult<BitIntSpec> {
        if width <= 0 {
            return Err("_BitInt width must be positive".to_string());
        }
        if !unsigned && width == 1 {
            return Err("signed _BitInt width must be greater than 1".to_string());
        }
        let storage = match width {
            32 => {
                if unsigned {
                    CType::UInt
                } else {
                    CType::Int
                }
            }
            64 => {
                if unsigned {
                    CType::ULong
                } else {
                    CType::Long
                }
            }
            128 => {
                if unsigned {
                    CType::UInt128
                } else {
                    CType::Int128
                }
            }
            _ => return Err("_BitInt width is not supported by an exact storage type".to_string()),
        };
        Ok(BitIntSpec {
            width,
            unsigned,
            storage,
        })
    }

    fn parse_bitint_width(&mut self) -> ParseResult<i64> {
        match self.peek() {
            Some(Token::Identifier(name)) if name == "_BitInt" => {
                self.advance()?;
            }
            _ => return Err(self.format_error("expected _BitInt")),
        }
        self.expect_token(Token::OpenParen)?;
        if self.at(&Token::CloseParen) {
            return Err(self.format_error("expected integer constant _BitInt width"));
        }
        let width_exp = self.parse_expression()?;
        self.expect_token(Token::CloseParen)?;
        self.eval_integer_constant_exp_with_layout(&width_exp)
            .ok_or_else(|| self.format_error("expected integer constant _BitInt width"))
    }

    fn consume_member_attributes(&mut self) -> ParseResult<MemberAttributes> {
        let mut attrs = MemberAttributes::default();
        loop {
            match self.peek().cloned() {
                Some(Token::KWAlignAs) => {
                    let value = self.parse_alignment_specifier()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributeAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributeAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.pending_noreturn = true;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributePacked) => {
                    self.advance()?;
                    attrs.packed = true;
                }
                Some(Token::AttributeTransparentUnion) => {
                    self.advance()?;
                }
                Some(Token::AttributePackedAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                    attrs.packed = true;
                }
                Some(Token::AttributePackedAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.pending_noreturn = true;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                    attrs.packed = true;
                }
                Some(Token::AttributeMode(_))
                | Some(Token::AttributeVectorSize(_))
                | Some(Token::AttributeDeprecated(_))
                | Some(Token::AttributeScalarStorageOrderReverse) => {
                    self.advance()?;
                }
                _ => break,
            }
        }
        Ok(attrs)
    }

    fn consume_alignment_specifiers(&mut self) -> ParseResult<Option<std::num::NonZeroUsize>> {
        Ok(self.consume_member_attributes()?.alignment)
    }

    fn consume_declaration_attributes(&mut self) -> ParseResult<DeclarationAttributes> {
        let mut attrs = DeclarationAttributes::default();
        loop {
            match self.peek().cloned() {
                Some(Token::KWAlignAs) => {
                    let value = self.parse_alignment_specifier()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributeAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributeAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                    attrs.noreturn = true;
                }
                Some(Token::AttributeNoreturn) | Some(Token::KWNoreturn) => {
                    self.advance()?;
                    attrs.noreturn = true;
                }
                Some(Token::AttributeNoInstrumentFunction) => {
                    self.advance()?;
                    self.pending_no_instrument_function = true;
                }
                Some(Token::AttributeAlias(target)) => {
                    self.advance()?;
                    self.pending_alias = Some(target);
                }
                Some(Token::AttributePacked) => {
                    self.advance()?;
                }
                Some(Token::AttributeTransparentUnion) => {
                    self.advance()?;
                    self.pending_transparent_union = true;
                }
                Some(Token::AttributePackedAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributePackedAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                    attrs.noreturn = true;
                }
                Some(Token::AttributeMode(_))
                | Some(Token::AttributeDeprecated(_))
                | Some(Token::AttributeScalarStorageOrderReverse) => {
                    self.advance()?;
                }
                Some(Token::AttributeVectorSize(expression)) => {
                    attrs.vector_size = Some(self.parse_attribute_vector_size(&expression)?);
                    self.advance()?;
                }
                _ => break,
            }
        }
        Ok(attrs)
    }

    fn consume_type_name_attributes(&mut self) -> ParseResult<DeclarationAttributes> {
        // Type names accept the same adjacent GNU attributes as declarations, but
        // declaration-only effects must not leak into the next real declaration.
        let pending_no_instrument_function = self.pending_no_instrument_function;
        let pending_transparent_union = self.pending_transparent_union;
        let pending_alias = self.pending_alias.clone();
        let attrs = self.consume_declaration_attributes();
        self.pending_no_instrument_function = pending_no_instrument_function;
        self.pending_transparent_union = pending_transparent_union;
        self.pending_alias = pending_alias;
        attrs
    }

    fn apply_vector_size_attr(&self, full_type: FullType, vector_size: Option<usize>) -> FullType {
        let Some(vector_size) = vector_size else {
            return full_type;
        };
        let elem = match full_type {
            FullType::Vector { elem, .. } => *elem,
            other => other,
        };
        let lane_size = elem.byte_size().max(1);
        FullType::Vector {
            elem: Box::new(elem),
            lanes: std::cmp::max(vector_size / lane_size, 1),
            complex: false,
        }
    }

    fn consume_aggregate_attributes(&mut self) -> ParseResult<AggregateAttributes> {
        let mut attrs = AggregateAttributes::default();
        loop {
            match self.peek().cloned() {
                Some(Token::AttributePacked) => {
                    self.advance()?;
                    attrs.packed = true;
                }
                Some(Token::AttributeTransparentUnion) => {
                    self.advance()?;
                    attrs.transparent_union = true;
                }
                Some(Token::AttributeAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributeAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                }
                Some(Token::AttributePackedAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                    attrs.packed = true;
                }
                Some(Token::AttributePackedAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    attrs.alignment = Self::merge_alignment(attrs.alignment, value);
                    attrs.packed = true;
                }
                Some(Token::AttributeMode(_))
                | Some(Token::AttributeVectorSize(_))
                | Some(Token::AttributeDeprecated(_)) => {
                    self.advance()?;
                }
                Some(Token::AttributeScalarStorageOrderReverse) => {
                    self.advance()?;
                    attrs.reverse_storage_order = true;
                }
                _ => break,
            }
        }
        Ok(attrs)
    }

    fn parse_typeof_full_type(&mut self) -> ParseResult<FullType> {
        if !matches!(self.peek(), Some(Token::KWTypeOf | Token::KWTypeOfUnqual)) {
            return Err(self.format_error("expected typeof"));
        }
        self.advance()?;
        self.expect_token(Token::OpenParen)?;
        let full_type = if self.is_type_keyword_at_pos() {
            self.parse_type_name_full()?
        } else {
            let expression = self.parse_expression()?;
            self.typeof_expression(&expression)?
        };
        self.expect_token(Token::CloseParen)?;
        Ok(full_type)
    }

    fn typeof_expression(&self, exp: &Exp) -> ParseResult<FullType> {
        match exp {
            Exp::Constant(_) => Ok(FullType::Scalar(CType::Int)),
            Exp::LongConstant(_) => Ok(FullType::Scalar(CType::Long)),
            Exp::Int128Constant(_) => Ok(FullType::Scalar(CType::Int128)),
            Exp::UIntConstant(_) => Ok(FullType::Scalar(CType::UInt)),
            Exp::ULongConstant(_) => Ok(FullType::Scalar(CType::ULong)),
            Exp::UInt128Constant(_) => Ok(FullType::Scalar(CType::UInt128)),
            Exp::DoubleConstant(_) => Ok(FullType::Scalar(CType::Double)),
            Exp::LongDoubleConstant(_) => Ok(FullType::Scalar(CType::LongDouble)),
            Exp::ImaginaryIntConstant(_) => Ok(Self::complex_full_type(CType::Int)),
            Exp::ImaginaryDoubleConstant(_) => Ok(Self::complex_full_type(CType::Double)),
            Exp::StringLiteral(value) => Ok(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size: c_string_byte_len(value) + 1,
            }),
            Exp::WideStringLiteral(value) => Ok(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Int)),
                size: value.chars().count() + 1,
            }),
            Exp::Utf16StringLiteral(value) => Ok(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::UShort)),
                size: value.encode_utf16().count() + 1,
            }),
            Exp::Utf32StringLiteral(value) => Ok(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::UInt)),
                size: value.chars().count() + 1,
            }),
            Exp::Var(name) => self.lookup_value_type(name).ok_or_else(|| {
                self.format_error(&format!("unknown expression type for typeof({})", name))
            }),
            Exp::Cast(ctype, full_type, _) => {
                Ok(full_type.clone().unwrap_or(FullType::Scalar(*ctype)))
            }
            Exp::Unary(UnaryOp::AddrOf, inner) => {
                Ok(FullType::Pointer(Box::new(self.typeof_expression(inner)?)))
            }
            Exp::Unary(UnaryOp::Deref, inner) => match self.typeof_expression(inner)? {
                FullType::Pointer(inner) | FullType::Array { elem: inner, .. } => Ok(*inner),
                _ => Err(self.format_error("typeof cannot dereference non-pointer expression")),
            },
            Exp::Unary(UnaryOp::RealPart | UnaryOp::ImagPart, inner) => {
                match self.typeof_expression(inner)? {
                    FullType::Vector {
                        elem,
                        complex: true,
                        ..
                    } => Ok(*elem),
                    inner_type => Ok(inner_type),
                }
            }
            Exp::Subscript(array, _) => match self.typeof_expression(array)? {
                FullType::Pointer(inner)
                | FullType::Array { elem: inner, .. }
                | FullType::Vector { elem: inner, .. } => Ok(*inner),
                _ => Err(self.format_error("typeof cannot subscript non-array expression")),
            },
            Exp::Dot(base, member) => {
                let base_type = self.typeof_expression(base)?;
                self.member_expression_type(&base_type, member)
            }
            Exp::Arrow(base, member) => match self.typeof_expression(base)?.decay() {
                FullType::Pointer(inner) => self.member_expression_type(&inner, member),
                _ => Err(self.format_error("typeof cannot use -> on non-pointer expression")),
            },
            Exp::Unary(UnaryOp::LogicalNot, _) => Ok(FullType::Scalar(CType::Int)),
            Exp::Unary(UnaryOp::Negate | UnaryOp::Complement, inner) => {
                match self.typeof_expression(inner)? {
                    FullType::Scalar(ct) => Ok(FullType::Scalar(ct.promote())),
                    inner_type => Ok(inner_type),
                }
            }
            Exp::Unary(_, inner) => self.typeof_expression(inner),
            Exp::Binary(op, left, right) => {
                if matches!(
                    op,
                    BinaryOp::LogicalAnd
                        | BinaryOp::LogicalOr
                        | BinaryOp::Equal
                        | BinaryOp::NotEqual
                        | BinaryOp::LessThan
                        | BinaryOp::GreaterThan
                        | BinaryOp::LessEqual
                        | BinaryOp::GreaterEqual
                ) {
                    return Ok(FullType::Scalar(CType::Int));
                }
                let left_type = self.typeof_expression(left)?;
                let right_type = self.typeof_expression(right)?;
                if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
                    return match left_type {
                        FullType::Scalar(ct) => Ok(FullType::Scalar(ct.promote())),
                        other => Ok(other),
                    };
                }
                if matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                    if matches!(left_type, FullType::Pointer(_))
                        && matches!(right_type, FullType::Pointer(_))
                    {
                        return Ok(FullType::Scalar(CType::Long));
                    }
                    if matches!(left_type, FullType::Pointer(_)) {
                        return Ok(left_type);
                    }
                    if matches!(right_type, FullType::Pointer(_)) && matches!(op, BinaryOp::Add) {
                        return Ok(right_type);
                    }
                }
                Ok(FullType::Scalar(CType::common(
                    left_type.to_ctype(),
                    right_type.to_ctype(),
                )))
            }
            Exp::Conditional(_, then_expr, else_expr) => {
                let then_type = self.typeof_expression(then_expr)?;
                let else_type = self.typeof_expression(else_expr)?;
                if then_type == else_type {
                    return Ok(then_type);
                }
                Ok(FullType::Scalar(CType::common(
                    then_type.to_ctype(),
                    else_type.to_ctype(),
                )))
            }
            Exp::Comma(_, right) => self.typeof_expression(right),
            Exp::BuiltinExpect(value, _) => self.typeof_expression(value),
            Exp::Assign(left, _) | Exp::CompoundAssign(_, left, _) => self.typeof_expression(left),
            Exp::StatementExpr(_, _, Some(full_type)) => Ok(full_type.clone()),
            Exp::StatementExpr(_, _, None) => Ok(FullType::Scalar(CType::Void)),
            Exp::FunctionCall(name, _) => match self.lookup_value_type(name) {
                Some(FullType::Function { return_type, .. }) => Ok(*return_type),
                Some(FullType::Pointer(inner)) => match *inner {
                    FullType::Function { return_type, .. } => Ok(*return_type),
                    _ => Err(self.format_error(&format!("{} is not a function", name))),
                },
                Some(full_type) => Ok(full_type),
                None => Ok(FullType::Scalar(CType::Int)),
            },
            Exp::ImplicitFunctionCall(_, _) => Ok(FullType::Scalar(CType::Int)),
            Exp::IndirectCall(callee, _) => match self.typeof_expression(callee)?.decay() {
                FullType::Function { return_type, .. } => Ok(*return_type),
                FullType::Pointer(inner) => match *inner {
                    FullType::Function { return_type, .. } => Ok(*return_type),
                    _ => Err(self.format_error("typeof cannot call non-function pointer")),
                },
                _ => Err(self.format_error("typeof cannot call non-function expression")),
            },
            Exp::SizeOf(_) | Exp::SizeOfType(_, _) | Exp::AlignOfType(_) => {
                Ok(FullType::Scalar(CType::ULong))
            }
            Exp::AtomicFence => Ok(FullType::Scalar(CType::Int)),
            Exp::Unreachable => Ok(FullType::Scalar(CType::Void)),
            Exp::AtomicFetch { ptr, .. } | Exp::AtomicExchange { ptr, .. } => {
                self.atomic_pointee_expression_type(ptr)
            }
            Exp::AtomicCompareExchange { .. } => Ok(FullType::Scalar(CType::Bool)),
            Exp::AtomicCompareSwap {
                ptr, return_old, ..
            } => {
                if *return_old {
                    self.atomic_pointee_expression_type(ptr)
                } else {
                    Ok(FullType::Scalar(CType::Bool))
                }
            }
            _ => Err(self.format_error("unsupported typeof expression")),
        }
    }

    fn parse_array_size(&mut self, allow_empty: bool) -> ParseResult<usize> {
        while matches!(
            self.peek(),
            Some(Token::KWConst)
                | Some(Token::KWVolatile)
                | Some(Token::KWRestrict)
                | Some(Token::KWAtomic)
        ) {
            self.advance()?;
        }
        if self.eat(&Token::KWStatic) {
            while matches!(
                self.peek(),
                Some(Token::KWConst)
                    | Some(Token::KWVolatile)
                    | Some(Token::KWRestrict)
                    | Some(Token::KWAtomic)
            ) {
                self.advance()?;
            }
        }
        if allow_empty && self.at(&Token::CloseBracket) {
            self.pending_flexible_array_bound = true;
            return Ok(0);
        }
        if self.eat(&Token::Star) {
            return Ok(0);
        }
        let exp = self.parse_assignment()?;
        let Some(value) = self.eval_integer_constant_exp_with_layout(&exp) else {
            let bound = if self.current_function_name.is_some() && self.param_parse_depth == 0 {
                let name = format!("__rnqcc_vla_bound_{}", self.vla_bound_counter);
                self.vla_bound_counter += 1;
                let decl = VarDeclaration {
                    name: name.clone(),
                    var_type: CType::Long,
                    ptr_info: None,
                    array_dims: None,
                    decl_full_type: Some(FullType::Scalar(CType::Long)),
                    dynamic_size: None,
                    init: Some(exp),
                    storage_class: None,
                    alignment: None,
                    alias: None,
                };
                let _ = self.add_value_type(name.clone(), FullType::Scalar(CType::Long));
                self.pending_pre_block_items
                    .push(BlockItem::Declaration(Declaration::VarDecl(decl)));
                Exp::Var(name)
            } else {
                exp
            };
            self.pending_vla_bound = Some(bound);
            return Ok(VLA_STATIC_SCALE_FALLBACK);
        };
        if value < 0 {
            return Err(self.format_error("array size must be non-negative"));
        }
        Ok(value as usize)
    }

    fn vla_size_expr_from_bound(bound: Exp, full_type: &FullType) -> Option<Exp> {
        match full_type {
            FullType::Array { elem, size } => {
                if *size == VLA_STATIC_SCALE_FALLBACK {
                    return Some(Exp::Binary(
                        BinaryOp::Mul,
                        Box::new(bound),
                        Box::new(Exp::SizeOfType(elem.to_ctype(), elem.as_ref().clone())),
                    ));
                }
                let inner = Self::vla_size_expr_from_bound(bound, elem)?;
                Some(Exp::Binary(
                    BinaryOp::Mul,
                    Box::new(Exp::ULongConstant(*size as i64)),
                    Box::new(inner),
                ))
            }
            FullType::Struct(_) => None,
            _ => None,
        }
    }

    fn pending_vla_size_expr_for_type(&mut self, full_type: &FullType) -> Option<Exp> {
        let bound = self.pending_vla_bound.take()?;
        if let Some(size) = Self::vla_size_expr_from_bound(bound.clone(), full_type) {
            return Some(size);
        }
        match full_type {
            FullType::Struct(tag) => {
                let def = self.struct_defs.get(tag)?;
                let mem = def.members.iter().find(|mem| {
                    matches!(
                        mem.member_full_type,
                        FullType::Array {
                            size: VLA_STATIC_SCALE_FALLBACK,
                            ..
                        }
                    ) || matches!(mem.member_full_type, FullType::Array { .. })
                })?;
                let size = Self::vla_size_expr_from_bound(bound, &mem.member_full_type)?;
                if mem.offset == 0 {
                    Some(size)
                } else {
                    Some(Exp::Binary(
                        BinaryOp::Add,
                        Box::new(Exp::ULongConstant(mem.offset as i64)),
                        Box::new(size),
                    ))
                }
            }
            _ => None,
        }
    }

    fn dynamic_size_expr_for_full_type(&self, full_type: &FullType) -> Option<Exp> {
        match full_type {
            FullType::Array { elem, size } if *size == VLA_STATIC_SCALE_FALLBACK => {
                Some(Exp::SizeOfType(elem.to_ctype(), elem.as_ref().clone()))
            }
            FullType::Array { elem, size } => {
                self.dynamic_size_expr_for_full_type(elem).map(|inner| {
                    Exp::Binary(
                        BinaryOp::Mul,
                        Box::new(Exp::ULongConstant(*size as i64)),
                        Box::new(inner),
                    )
                })
            }
            FullType::Struct(tag) => {
                let def = self.struct_defs.get(tag)?;
                for mem in &def.members {
                    if !matches!(
                        mem.member_full_type,
                        FullType::Array {
                            size: VLA_STATIC_SCALE_FALLBACK,
                            ..
                        }
                    ) {
                        continue;
                    }
                    if let Some(size) = self
                        .struct_member_vla_elem_sizes
                        .get(&(tag.clone(), mem.name.clone()))
                    {
                        return Some(if mem.offset == 0 {
                            size.clone()
                        } else {
                            Exp::Binary(
                                BinaryOp::Add,
                                Box::new(Exp::ULongConstant(mem.offset as i64)),
                                Box::new(size.clone()),
                            )
                        });
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn dynamic_size_expr_for_decl_type(&self, full_type: &FullType) -> Option<Exp> {
        match full_type {
            FullType::Pointer(inner) => self.dynamic_size_expr_for_full_type(inner),
            other => self.dynamic_size_expr_for_full_type(other),
        }
    }

    fn typedef_vla_size_expr(&mut self, full_type: &FullType) -> Option<Exp> {
        self.pending_vla_size_expr_for_type(full_type)
            .or_else(|| self.dynamic_size_expr_for_full_type(full_type))
    }

    /// Parse enum body: { A, B = 5, C }
    /// Records constants in the enum scope and returns CType::Int.
    fn parse_enum_body(&mut self) -> ParseResult<()> {
        self.expect_token(Token::OpenBrace)?;
        let mut next_val: i64 = 0;
        loop {
            if self.at(&Token::CloseBrace) {
                break;
            }
            let name = self.parse_identifier()?;
            if self.eat(&Token::Assign) {
                let exp = self.parse_assignment()?;
                let val = self
                    .eval_integer_constant_exp_with_layout(&exp)
                    .ok_or_else(|| self.format_error("expected integer constant in enum"))?;
                next_val = val;
            }
            self.add_enum_constant(name, next_val)?;
            next_val += 1;
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect_token(Token::CloseBrace)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn format_error(&self, msg: &str) -> String {
        // Show context: the 3 tokens around the current position
        let start = self.pos.saturating_sub(2);
        let end = std::cmp::min(self.pos + 3, self.tokens.len());
        let context: Vec<String> = self.tokens[start..end]
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if start + i == self.pos {
                    format!(">>>{:?}<<<", t)
                } else {
                    format!("{:?}", t)
                }
            })
            .collect();
        format!(
            "Parse error at token {}: {}\n  Context: {}",
            self.pos,
            msg,
            context.join(" ")
        )
    }

    fn advance(&mut self) -> ParseResult<Token> {
        let tok = self
            .tokens
            .get(self.pos)
            .cloned()
            .ok_or_else(|| self.format_error("unexpected end of input"))?;
        self.pos += 1;
        Ok(tok)
    }

    fn expect_token(&mut self, expected: Token) -> ParseResult<()> {
        let actual = self.peek().cloned().ok_or_else(|| {
            self.format_error(&format!("expected {:?} but found end of input", expected))
        })?;
        self.pos += 1;
        if actual != expected {
            self.pos -= 1; // point at the unexpected token
            Err(self.format_error(&format!("expected {:?} but found {:?}", expected, actual)))
        } else {
            Ok(())
        }
    }

    fn at(&self, expected: &Token) -> bool {
        self.peek() == Some(expected)
    }

    fn eat(&mut self, expected: &Token) -> bool {
        if self.at(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    // --------------------------------------------------------
    // Top-level
    // --------------------------------------------------------

    pub fn parse_program(&mut self) -> ParseResult<Program> {
        let mut declarations = Vec::new();
        while self.peek().is_some() {
            let decl = self.parse_declaration()?;
            // Emit any pending struct/union definitions from type specifier parsing
            for sd in self.pending_struct_decls.drain(..) {
                declarations.push(Declaration::StructDecl(sd));
            }
            declarations.push(decl);
            // Emit extra declarations from multi-declarator parsing
            declarations.append(&mut self.pending_declarations);
        }
        Ok(Program { declarations })
    }

    fn parse_specifiers(&mut self) -> ParseResult<(Option<StorageClass>, CType)> {
        self.last_type_was_enum = false;
        let mut sc: Option<StorageClass> = None;
        let mut has_int = false;
        let mut has_long = false;
        let mut has_short = false;
        let mut has_char = false;
        let mut has_unsigned = false;
        let mut has_signed = false;
        let mut has_void = false;
        let mut has_float = false;
        let mut has_double = false;
        let mut has_int128 = false;
        let mut bitint_width: Option<i64> = None;
        let mut saw_complex = false;
        let mut mode_attr: Option<String> = None;
        let mut vector_size_attr: Option<usize> = None;
        loop {
            match self.peek().cloned() {
                // Ignored qualifiers/specifiers
                Some(Token::KWConst) | Some(Token::KWVolatile) | Some(Token::KWRestrict) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::Identifier(name)) if Self::is_gnu_qualifier_name(&name) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::KWInline) => {
                    self.pending_inline = true;
                    self.advance()?;
                    continue;
                }
                Some(Token::Identifier(name)) if Self::is_complex_type_name(&name) => {
                    saw_complex = true;
                    self.advance()?;
                    continue;
                }
                Some(Token::Identifier(name))
                    if Self::is_builtin_float_type_name(&name)
                        && self.lookup_visible_typedef(&name).is_none()
                        && !has_int
                        && !has_long
                        && !has_int128
                        && !has_void
                        && !has_unsigned
                        && !has_signed
                        && !has_char
                        && !has_float
                        && !has_double =>
                {
                    self.advance()?;
                    has_double = true;
                }
                Some(Token::Identifier(name)) if Self::is_builtin_int128_type_name(&name) => {
                    self.advance()?;
                    has_int128 = true;
                }
                Some(Token::Identifier(name)) if name == "_BitInt" && bitint_width.is_none() => {
                    bitint_width = Some(self.parse_bitint_width()?);
                }
                Some(Token::Identifier(name)) if name == "_BitInt" => {
                    return Err(self.format_error("duplicate _BitInt type specifier"));
                }
                Some(Token::KWNoreturn) | Some(Token::AttributeNoreturn) => {
                    self.pending_noreturn = true;
                    self.advance()?;
                    continue;
                }
                Some(Token::AttributeDeprecated(_)) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::AttributeNoInstrumentFunction) => {
                    self.pending_no_instrument_function = true;
                    self.advance()?;
                    continue;
                }
                Some(Token::KWThreadLocal) => {
                    self.advance()?;
                    sc = Some(match sc {
                        Some(existing) => existing.with_thread_local(),
                        None => StorageClass::ThreadLocal,
                    });
                }
                Some(Token::KWAlignAs) => {
                    let alignment = self.parse_alignment_specifier()?;
                    self.pending_alignment =
                        Self::merge_alignment(self.pending_alignment, alignment);
                    continue;
                }
                Some(Token::AttributeAligned(value))
                | Some(Token::AttributePackedAligned(value)) => {
                    let alignment = self.parse_attribute_alignment(&value)?;
                    self.advance()?;
                    self.pending_alignment =
                        Self::merge_alignment(self.pending_alignment, alignment);
                    continue;
                }
                Some(Token::AttributeAlignedNoreturn(value))
                | Some(Token::AttributePackedAlignedNoreturn(value)) => {
                    let alignment = self.parse_attribute_alignment(&value)?;
                    self.advance()?;
                    self.pending_alignment =
                        Self::merge_alignment(self.pending_alignment, alignment);
                    self.pending_noreturn = true;
                    continue;
                }
                Some(Token::AttributePacked) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::AttributeMode(mode)) => {
                    mode_attr = Some(mode);
                    self.advance()?;
                    continue;
                }
                Some(Token::AttributeVectorSize(expression)) => {
                    vector_size_attr = Some(self.parse_attribute_vector_size(&expression)?);
                    self.advance()?;
                    continue;
                }
                Some(Token::KWAtomic) => {
                    self.advance()?;
                    if self.eat(&Token::OpenParen) {
                        let (_atomic_sc, atomic_type) = self.parse_specifiers()?;
                        self.expect_token(Token::CloseParen)?;
                        return Ok((sc, atomic_type));
                    }
                    continue;
                }
                Some(Token::KWTypeOf | Token::KWTypeOfUnqual)
                    if !has_int
                        && !has_long
                        && !has_int128
                        && !has_short
                        && !has_void
                        && !has_unsigned
                        && !has_signed
                        && !has_char =>
                {
                    let full_type = self.parse_typeof_full_type()?;
                    self.last_typedef_full_type = Some(full_type.clone());
                    return Ok((sc, full_type.to_ctype()));
                }
                Some(Token::KWAutoType)
                    if !has_int
                        && !has_long
                        && !has_int128
                        && !has_short
                        && !has_void
                        && !has_unsigned
                        && !has_signed
                        && !has_char =>
                {
                    self.advance()?;
                    self.pending_auto_type = true;
                    return Ok((sc, CType::Void));
                }
                Some(Token::KWStatic) if sc.as_ref().is_none_or(StorageClass::is_thread_local) => {
                    self.advance()?;
                    sc = Some(match sc {
                        Some(existing) => existing.with_static(),
                        None => StorageClass::Static,
                    });
                }
                Some(Token::KWExtern) if sc.as_ref().is_none_or(StorageClass::is_thread_local) => {
                    self.advance()?;
                    sc = Some(match sc {
                        Some(existing) => existing.with_extern(),
                        None => StorageClass::Extern,
                    });
                }
                Some(Token::KWTypedef) if sc.is_none() => {
                    self.advance()?;
                    sc = Some(StorageClass::Typedef);
                }
                Some(Token::KWRegister) if sc.is_none() => {
                    self.advance()?; // ignore register storage class
                }
                Some(Token::KWAuto) if sc.is_none() => {
                    self.advance()?; // ignore auto storage class
                }
                Some(Token::KWInt) if !has_int && !has_void && !has_char => {
                    self.advance()?;
                    has_int = true;
                }
                Some(Token::KWLong) if !has_void && !has_char && !has_int128 => {
                    self.advance()?;
                    has_long = true; // long long is the same as long (both 64-bit)
                }
                Some(Token::KWChar)
                    if !has_char && !has_int && !has_long && !has_void && !has_int128 =>
                {
                    self.advance()?;
                    has_char = true;
                }
                Some(Token::KWShort) if !has_long && !has_char && !has_void && !has_short => {
                    self.advance()?;
                    has_short = true;
                }
                Some(Token::KWUnsigned) if !has_unsigned && !has_signed && !has_void => {
                    self.advance()?;
                    has_unsigned = true;
                }
                Some(Token::KWSigned) if !has_signed && !has_unsigned && !has_void => {
                    self.advance()?;
                    has_signed = true;
                }
                Some(Token::KWDouble)
                    if !has_int && !has_void && !has_unsigned && !has_signed && !has_char =>
                {
                    self.advance()?;
                    has_double = true;
                }
                Some(Token::KWFloat)
                    if !has_int
                        && !has_long
                        && !has_int128
                        && !has_void
                        && !has_unsigned
                        && !has_signed
                        && !has_char =>
                {
                    self.advance()?;
                    has_float = true;
                }
                Some(Token::KWVoid)
                    if !has_int
                        && !has_long
                        && !has_int128
                        && !has_void
                        && !has_unsigned
                        && !has_signed
                        && !has_char =>
                {
                    self.advance()?;
                    has_void = true;
                }
                Some(Token::KWBool)
                    if !has_int
                        && !has_long
                        && !has_void
                        && !has_unsigned
                        && !has_signed
                        && !has_char =>
                {
                    self.advance()?;
                    return Ok((sc, CType::Bool));
                }
                _ => break,
            }
        }

        if !has_int
            && !has_long
            && !has_int128
            && !has_short
            && !has_void
            && !has_unsigned
            && !has_signed
            && !has_char
            && !has_float
            && !has_double
            && bitint_width.is_none()
        {
            // Check for struct or union
            if self.at(&Token::KWStruct) || self.at(&Token::KWUnion) {
                let ct_ft = self.parse_struct_type_specifier()?;
                return Ok((sc, ct_ft.0));
            }
            // Check for enum
            if self.at(&Token::KWEnum) {
                self.advance()?;
                // Optional tag name
                if let Some(Token::Identifier(_)) = self.peek() {
                    self.advance()?; // consume tag (we don't track enum tags)
                }
                self.parse_enum_fixed_underlying_type()?;
                // Optional body
                if self.at(&Token::OpenBrace) {
                    self.parse_enum_body()?;
                }
                self.last_type_was_enum = true;
                return Ok((sc, CType::Int));
            }
            // Check for typedef name
            if let Some(Token::Identifier(name)) = self.peek() {
                if name == "bool" {
                    self.advance()?;
                    self.last_typedef_full_type = None;
                    return Ok((sc, CType::Bool));
                }
                if Self::is_builtin_int128_type_name(name) {
                    self.advance()?;
                    self.last_typedef_full_type = None;
                    return Ok((sc, CType::Int128));
                }
                if let Some(info) = self.lookup_visible_typedef(name) {
                    let ct = info.base_type;
                    let tag = info.struct_tag.clone();
                    let ft = info.full_type.clone();
                    let vla_size = info.vla_size.clone();
                    let is_enum = info.is_enum;
                    let alignment = info.alignment;
                    self.advance()?;
                    if let Some(tag) = tag {
                        self.last_struct_tag = Some(tag);
                    }
                    self.last_typedef_full_type = Some(ft);
                    self.last_typedef_vla_size = vla_size;
                    self.last_type_was_enum = is_enum;
                    if let Some(alignment) = alignment {
                        self.pending_alignment =
                            Self::merge_alignment(self.pending_alignment, alignment);
                    }
                    return Ok((sc, ct));
                }
                if Self::is_builtin_float_type_name(name) {
                    self.advance()?;
                    self.last_typedef_full_type = None;
                    return Ok((sc, CType::Double));
                }
            }
            if saw_complex {
                self.last_typedef_full_type = Some(Self::complex_full_type(CType::Double));
                self.last_type_was_enum = false;
                return Ok((sc, CType::Double));
            }
            if matches!(
                self.peek(),
                Some(Token::Identifier(_)) | Some(Token::Star) | Some(Token::OpenParen)
            ) {
                return Ok((sc, CType::Int));
            }
            return Err(self.format_error("expected type specifier"));
        }
        self.last_typedef_full_type = None;

        if bitint_width.is_some()
            && (has_int
                || has_long
                || has_short
                || has_char
                || has_void
                || has_float
                || has_double
                || has_int128)
        {
            return Err(self.format_error("_BitInt cannot be combined with another type specifier"));
        }

        let bitint_spec = bitint_width
            .map(|width| Self::bitint_spec(width, has_unsigned))
            .transpose()
            .map_err(|msg| self.format_error(&msg))?;

        let mut ctype = if has_void {
            CType::Void
        } else if let Some(spec) = bitint_spec {
            spec.storage
        } else if has_int128 && has_unsigned {
            CType::UInt128
        } else if has_int128 {
            CType::Int128
        } else if has_char && has_unsigned {
            CType::UChar
        } else if has_char && has_signed {
            CType::SChar
        } else if has_char {
            CType::Char
        } else if has_unsigned && has_short {
            CType::UShort
        } else if has_short {
            CType::Short
        } else if has_unsigned && has_long {
            CType::ULong
        } else if has_unsigned {
            CType::UInt
        } else if has_float {
            CType::Float
        } else if has_double && has_long {
            self.long_double_ctype()
        } else if has_double {
            CType::Double
        } else if has_long {
            CType::Long
        } else {
            CType::Int // 'signed', 'signed int', 'int' all map to Int
        };

        if let Some(mode) = mode_attr {
            if let Some(mode_ctype) = Self::ctype_for_gnu_mode(&mode, has_unsigned) {
                ctype = mode_ctype;
            }
        }

        if let Some(vector_size) = vector_size_attr {
            let lane_size = ctype.size().max(1) as usize;
            let lanes = std::cmp::max(vector_size / lane_size, 1);
            self.last_typedef_full_type = Some(FullType::Vector {
                elem: Box::new(FullType::Scalar(ctype)),
                lanes,
                complex: false,
            });
        } else if saw_complex {
            self.last_typedef_full_type = Some(Self::complex_full_type(ctype));
        }

        self.last_type_was_enum = false;
        Ok((sc, ctype))
    }

    fn parse_type(&mut self) -> ParseResult<CType> {
        self.last_type_was_enum = false;
        self.last_typedef_full_type = None;
        self.last_struct_tag = None;
        let mut vector_size_attr = None;
        let mut mode_attr: Option<String> = None;
        let mut saw_complex = false;

        // Skip type qualifiers
        loop {
            match self.peek().cloned() {
                Some(
                    Token::KWConst
                    | Token::KWVolatile
                    | Token::KWRestrict
                    | Token::KWThreadLocal
                    | Token::KWRegister
                    | Token::KWAuto,
                ) => {
                    self.advance()?;
                }
                Some(Token::AttributeVectorSize(expression)) => {
                    vector_size_attr = Some(self.parse_attribute_vector_size(&expression)?);
                    self.advance()?;
                }
                Some(Token::AttributeMode(mode)) => {
                    mode_attr = Some(mode);
                    self.advance()?;
                }
                Some(
                    Token::AttributeAligned(_)
                    | Token::AttributeAlignedNoreturn(_)
                    | Token::AttributePacked
                    | Token::AttributePackedAligned(_)
                    | Token::AttributePackedAlignedNoreturn(_)
                    | Token::AttributeDeprecated(_),
                ) => {
                    self.advance()?;
                }
                _ => break,
            }
        }
        while self.peek().is_some_and(
            |tok| matches!(tok, Token::Identifier(name) if Self::is_gnu_qualifier_name(name)),
        ) {
            self.advance()?;
        }
        if self.eat(&Token::KWAtomic) && self.eat(&Token::OpenParen) {
            let ty = self.parse_type()?;
            self.expect_token(Token::CloseParen)?;
            return Ok(ty);
        }
        if self.at(&Token::KWVoid) {
            self.advance()?;
            return Ok(CType::Void);
        }
        if matches!(self.peek(), Some(Token::KWTypeOf | Token::KWTypeOfUnqual)) {
            let full_type = self.parse_typeof_full_type()?;
            let ctype = full_type.to_ctype();
            self.last_typedef_full_type = Some(full_type);
            return Ok(ctype);
        }
        if self.at(&Token::KWStruct) || self.at(&Token::KWUnion) {
            let (ct, _) = self.parse_struct_type_specifier()?;
            return Ok(ct);
        }
        if self.at(&Token::KWDouble) {
            self.advance()?;
            let has_complex = self.peek().is_some_and(
                |tok| matches!(tok, Token::Identifier(name) if Self::is_complex_type_name(name)),
            );
            if has_complex {
                self.advance()?;
            }
            if let Some(vector_size) = vector_size_attr {
                self.last_typedef_full_type = Some(FullType::Vector {
                    elem: Box::new(FullType::Scalar(CType::Double)),
                    lanes: std::cmp::max(vector_size / CType::Double.size() as usize, 1),
                    complex: false,
                });
            } else if has_complex {
                self.last_typedef_full_type = Some(Self::complex_full_type(CType::Double));
            }
            return Ok(CType::Double);
        }
        if self.at(&Token::KWFloat) {
            self.advance()?;
            let has_complex = self.peek().is_some_and(
                |tok| matches!(tok, Token::Identifier(name) if Self::is_complex_type_name(name)),
            );
            if has_complex {
                self.advance()?;
            }
            if let Some(vector_size) = vector_size_attr {
                self.last_typedef_full_type = Some(FullType::Vector {
                    elem: Box::new(FullType::Scalar(CType::Float)),
                    lanes: std::cmp::max(vector_size / CType::Float.size() as usize, 1),
                    complex: false,
                });
            } else if has_complex {
                self.last_typedef_full_type = Some(Self::complex_full_type(CType::Float));
            }
            return Ok(CType::Float);
        }
        if self.peek().is_some_and(
            |tok| matches!(tok, Token::Identifier(name) if Self::is_complex_type_name(name)),
        ) {
            saw_complex = true;
            self.advance()?;
            if self.at(&Token::KWFloat) {
                self.advance()?;
                self.last_typedef_full_type = Some(Self::complex_full_type(CType::Float));
                return Ok(CType::Float);
            }
            if self.at(&Token::KWDouble) {
                self.advance()?;
                self.last_typedef_full_type = Some(Self::complex_full_type(CType::Double));
                return Ok(CType::Double);
            }
            if self
                .peek()
                .is_some_and(|tok| matches!(tok, Token::Identifier(name) if Self::is_builtin_float_type_name(name)))
            {
                self.advance()?;
                self.last_typedef_full_type = Some(Self::complex_full_type(CType::Double));
                return Ok(CType::Double);
            }
            if !matches!(
                self.peek(),
                Some(Token::KWInt)
                    | Some(Token::KWLong)
                    | Some(Token::KWUnsigned)
                    | Some(Token::KWSigned)
                    | Some(Token::KWShort)
                    | Some(Token::KWChar)
                    | Some(Token::KWBool)
            ) {
                self.last_typedef_full_type = Some(Self::complex_full_type(CType::Double));
                return Ok(CType::Double);
            }
        }
        if self.at(&Token::KWBool) {
            self.advance()?;
            return Ok(CType::Bool);
        }
        if self
            .peek()
            .is_some_and(|tok| matches!(tok, Token::Identifier(name) if name == "bool"))
        {
            self.advance()?;
            return Ok(CType::Bool);
        }
        if self.at(&Token::KWEnum) {
            self.advance()?;
            if let Some(Token::Identifier(_)) = self.peek() {
                self.advance()?;
            }
            self.parse_enum_fixed_underlying_type()?;
            if self.at(&Token::OpenBrace) {
                self.parse_enum_body()?;
            }
            self.last_type_was_enum = true;
            return Ok(CType::Int);
        }
        // Check for typedef name before parsing int/long/etc.
        if let Some(Token::Identifier(name)) = self.peek() {
            if Self::is_builtin_int128_type_name(name) {
                self.advance()?;
                self.last_typedef_full_type = None;
                return Ok(CType::Int128);
            }
            if let Some(info) = self.lookup_visible_typedef(name) {
                let ct = info.base_type;
                let tag = info.struct_tag.clone();
                let ft = info.full_type.clone();
                let vla_size = info.vla_size.clone();
                let is_enum = info.is_enum;
                self.advance()?;
                if let Some(tag) = tag {
                    self.last_struct_tag = Some(tag);
                }
                self.last_typedef_full_type = Some(ft);
                self.last_typedef_vla_size = vla_size;
                self.last_type_was_enum = is_enum;
                return Ok(ct);
            }
            if Self::is_builtin_float_type_name(name) {
                self.advance()?;
                let has_complex = self.peek().is_some_and(
                    |tok| matches!(tok, Token::Identifier(name) if Self::is_complex_type_name(name)),
                );
                if has_complex {
                    self.advance()?;
                    self.last_typedef_full_type = Some(Self::complex_full_type(CType::Double));
                } else {
                    self.last_typedef_full_type = None;
                }
                return Ok(CType::Double);
            }
        }
        self.last_typedef_full_type = None;
        let mut has_int = false;
        let mut has_long = false;
        let mut has_short = false;
        let mut has_char = false;
        let mut has_unsigned = false;
        let mut has_signed = false;
        let mut has_double = false;
        let mut has_int128 = false;
        let mut bitint_width: Option<i64> = None;
        loop {
            match self.peek() {
                Some(Token::KWConst)
                | Some(Token::KWVolatile)
                | Some(Token::KWRestrict)
                | Some(Token::KWThreadLocal)
                | Some(Token::KWRegister)
                | Some(Token::KWAuto) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::Identifier(name)) if Self::is_gnu_qualifier_name(name) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::Identifier(name)) if Self::is_complex_type_name(name) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::Identifier(name)) if Self::is_builtin_int128_type_name(name) => {
                    self.advance()?;
                    has_int128 = true;
                }
                Some(Token::Identifier(name)) if name == "_BitInt" && bitint_width.is_none() => {
                    bitint_width = Some(self.parse_bitint_width()?);
                }
                Some(Token::Identifier(name)) if name == "_BitInt" => {
                    return Err(self.format_error("duplicate _BitInt type specifier"));
                }
                Some(Token::KWAlignAs) => {
                    let alignment = self.parse_alignment_specifier()?;
                    self.pending_alignment = Some(
                        self.pending_alignment
                            .map_or(alignment, |current| current.max(alignment)),
                    );
                    continue;
                }
                Some(Token::KWAtomic) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::KWInt) if !has_int && !has_char => {
                    self.advance()?;
                    has_int = true;
                }
                Some(Token::KWLong) if !has_char && !has_int128 => {
                    self.advance()?;
                    has_long = true;
                }
                Some(Token::KWDouble)
                    if has_long && !has_int && !has_char && !has_unsigned && !has_signed =>
                {
                    self.advance()?;
                    has_double = true;
                }
                Some(Token::KWChar) if !has_char && !has_int && !has_long => {
                    self.advance()?;
                    has_char = true;
                }
                Some(Token::KWShort) if !has_long && !has_char && !has_short => {
                    self.advance()?;
                    has_short = true;
                }
                Some(Token::KWUnsigned) if !has_unsigned && !has_signed => {
                    self.advance()?;
                    has_unsigned = true;
                }
                Some(Token::KWSigned) if !has_signed && !has_unsigned => {
                    self.advance()?;
                    has_signed = true;
                }
                _ => break,
            }
        }
        if bitint_width.is_some()
            && (has_int || has_long || has_short || has_char || has_double || has_int128)
        {
            return Err(self.format_error("_BitInt cannot be combined with another type specifier"));
        }
        let bitint_spec = bitint_width
            .map(|width| Self::bitint_spec(width, has_unsigned))
            .transpose()
            .map_err(|msg| self.format_error(&msg))?;

        let ctype = if let Some(spec) = bitint_spec {
            spec.storage
        } else if has_char && has_unsigned {
            CType::UChar
        } else if has_int128 && has_unsigned {
            CType::UInt128
        } else if has_int128 {
            CType::Int128
        } else if has_char && has_signed {
            CType::SChar
        } else if has_char {
            CType::Char
        } else if has_unsigned && has_short {
            CType::UShort
        } else if has_short {
            CType::Short
        } else if has_unsigned && has_long {
            CType::ULong
        } else if has_unsigned {
            CType::UInt
        } else if has_double && has_long {
            self.long_double_ctype()
        } else if has_double {
            CType::Double
        } else if has_long {
            CType::Long
        } else if has_int || has_signed {
            CType::Int
        } else {
            return Err(self.format_error("expected type specifier"));
        };
        let ctype = mode_attr
            .and_then(|mode| Self::ctype_for_gnu_mode(&mode, has_unsigned))
            .unwrap_or(ctype);
        if let Some(vector_size) = vector_size_attr {
            let lane_size = ctype.size().max(1) as usize;
            self.last_typedef_full_type = Some(FullType::Vector {
                elem: Box::new(FullType::Scalar(ctype)),
                lanes: std::cmp::max(vector_size / lane_size, 1),
                complex: false,
            });
        } else if saw_complex {
            self.last_typedef_full_type = Some(Self::complex_full_type(ctype));
        }
        self.last_type_was_enum = false;
        Ok(ctype)
    }

    fn is_type_keyword(&self, tok: &Token) -> bool {
        match tok {
            Token::KWInt
            | Token::KWLong
            | Token::KWVoid
            | Token::KWAlignAs
            | Token::KWUnsigned
            | Token::KWSigned
            | Token::KWDouble
            | Token::KWFloat
            | Token::KWChar
            | Token::KWStruct
            | Token::KWUnion
            | Token::KWEnum
            | Token::KWConst
            | Token::KWVolatile
            | Token::KWAtomic
            | Token::KWThreadLocal
            | Token::KWRegister
            | Token::KWAuto
            | Token::KWStaticAssert
            | Token::KWRestrict
            | Token::KWBool
            | Token::KWShort
            | Token::KWTypeOf
            | Token::KWTypeOfUnqual
            | Token::KWAutoType
            | Token::AttributePacked
            | Token::AttributePackedAligned(_)
            | Token::AttributePackedAlignedNoreturn(_)
            | Token::AttributeTransparentUnion
            | Token::AttributeAlignedNoreturn(_)
            | Token::AttributeNoreturn
            | Token::AttributeNoInstrumentFunction
            | Token::AttributeDeprecated(_)
            | Token::AttributeMode(_)
            | Token::AttributeVectorSize(_)
            | Token::AttributeScalarStorageOrderReverse
            | Token::KWNoreturn => true,
            Token::Identifier(name) => {
                name == "_BitInt"
                    || self.is_typedef_name(name)
                    || Self::is_builtin_float_type_name(name)
                    || Self::is_builtin_int128_type_name(name)
                    || Self::is_complex_type_name(name)
                    || Self::is_gnu_qualifier_name(name)
            }
            _ => false,
        }
    }

    /// Process a declarator tree to extract name, derived type, and params
    fn process_declarator(
        decl: &Declarator,
        base_type: CType,
        base_full_type: Option<&FullType>,
    ) -> (String, FullType, Option<FunctionDeclaratorInfo>) {
        let base_ft = base_full_type
            .cloned()
            .unwrap_or(FullType::Scalar(base_type));
        match decl {
            Declarator::Ident(name) => (name.clone(), base_ft, None),
            Declarator::Pointer(inner) => {
                let derived = FullType::Pointer(Box::new(base_ft));
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::Array(inner, size) => {
                let derived = FullType::Array {
                    elem: Box::new(base_ft),
                    size: *size,
                };
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::Function(
                params,
                pfts,
                deprecated_params,
                variadic,
                zero_fixed_variadic,
                old_style,
                bounds,
                inner,
            ) => {
                if let Declarator::Ident(name) = inner.as_ref() {
                    (
                        name.clone(),
                        base_ft,
                        Some(FunctionDeclaratorInfo {
                            params: params.clone(),
                            param_full_types: pfts.clone(),
                            deprecated_params: deprecated_params.clone(),
                            variadic: *variadic,
                            zero_fixed_variadic: *zero_fixed_variadic,
                            old_style: *old_style,
                            param_vla_bounds: bounds.clone(),
                        }),
                    )
                } else {
                    let fn_type = FullType::Function {
                        return_type: Box::new(base_ft),
                        params: pfts.clone(),
                        variadic: *variadic,
                    };
                    Self::process_declarator_with_type(inner, fn_type)
                }
            }
        }
    }

    fn process_declarator_with_type(
        decl: &Declarator,
        current_type: FullType,
    ) -> (String, FullType, Option<FunctionDeclaratorInfo>) {
        match decl {
            Declarator::Ident(name) => (name.clone(), current_type, None),
            Declarator::Pointer(inner) => {
                let derived = FullType::Pointer(Box::new(current_type));
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::Array(inner, size) => {
                let derived = FullType::Array {
                    elem: Box::new(current_type),
                    size: *size,
                };
                Self::process_declarator_with_type(inner, derived)
            }
            Declarator::Function(
                params,
                pfts,
                deprecated_params,
                variadic,
                zero_fixed_variadic,
                old_style,
                bounds,
                inner,
            ) => {
                if let Declarator::Ident(name) = inner.as_ref() {
                    // Function returning current_type: void *func() or int *func()
                    (
                        name.clone(),
                        current_type,
                        Some(FunctionDeclaratorInfo {
                            params: params.clone(),
                            param_full_types: pfts.clone(),
                            deprecated_params: deprecated_params.clone(),
                            variadic: *variadic,
                            zero_fixed_variadic: *zero_fixed_variadic,
                            old_style: *old_style,
                            param_vla_bounds: bounds.clone(),
                        }),
                    )
                } else {
                    let fn_type = FullType::Function {
                        return_type: Box::new(current_type),
                        params: pfts.clone(),
                        variadic: *variadic,
                    };
                    Self::process_declarator_with_type(inner, fn_type)
                }
            }
        }
    }

    /// Parse a declarator into a tree structure
    fn parse_declarator_tree(&mut self) -> ParseResult<Declarator> {
        self.parse_declarator_tree_inner(false)
    }

    /// Parse a declarator tree. If `allow_abstract` is true, the name is optional
    /// (for function pointer param lists and abstract declarators).
    fn parse_declarator_tree_inner(&mut self, allow_abstract: bool) -> ParseResult<Declarator> {
        self.consume_declarator_qualifiers()?;
        // Count leading * (skip const/volatile/restrict after each star)
        let mut stars = 0;
        while self.eat(&Token::Star) {
            stars += 1;
            self.consume_declarator_qualifiers()?;
        }

        // Direct declarator: identifier, (declarator), or abstract (no name)
        let mut decl = if self.eat(&Token::OpenParen) {
            if self.eat(&Token::Caret) {
                while self.is_declarator_qualifier()
                    || matches!(
                        self.peek(),
                        Some(Token::Identifier(name))
                            if matches!(
                                name.as_str(),
                                "_Nonnull" | "_Nullable" | "_Null_unspecified"
                            )
                    )
                {
                    self.advance()?;
                }
                let inner = if self.at(&Token::CloseParen) {
                    Declarator::Ident(String::new())
                } else {
                    self.parse_declarator_tree_inner(true)?
                };
                self.expect_token(Token::CloseParen)?;
                Declarator::Pointer(Box::new(inner))
            } else
            // Check if this is a grouped declarator like (*fp) or just (params)
            if self.at(&Token::Star) || matches!(self.peek(), Some(Token::Identifier(_))) {
                // Could be grouped declarator: (*name) or (name)
                // But only if NOT followed by a type keyword inside (which would indicate params)
                let save = self.pos;
                // Peek ahead: skip stars, check for identifier
                let mut temp_stars = 0;
                while self.eat(&Token::Star) {
                    temp_stars += 1;
                    while self.is_declarator_qualifier()
                        || matches!(
                            self.peek(),
                            Some(Token::Identifier(name))
                                if matches!(
                                    name.as_str(),
                                    "_Nonnull" | "_Nullable" | "_Null_unspecified"
                                )
                        )
                    {
                        self.advance()?;
                    }
                }
                let is_grouped = matches!(self.peek(), Some(Token::Identifier(_)))
                    || (temp_stars > 0
                        && (self.at(&Token::CloseParen) || self.at(&Token::OpenParen)));
                self.pos = save;

                if is_grouped {
                    let inner = self.parse_declarator_tree_inner(allow_abstract)?;
                    self.expect_token(Token::CloseParen)?;
                    inner
                } else {
                    // It's a parameter list, not a grouped declarator
                    if allow_abstract {
                        // Abstract declarator with no name — put back '(' and stop
                        self.pos -= 1; // un-eat the '('
                        Declarator::Ident(String::new())
                    } else {
                        return Err(self.format_error("unexpected parameter list in declarator"));
                    }
                }
            } else if allow_abstract
                && (self.is_type_keyword_at_pos() || self.at(&Token::CloseParen))
            {
                // Abstract: (int, int) — this is a function parameter list, not a grouped decl
                self.pos -= 1; // un-eat the '('
                Declarator::Ident(String::new())
            } else if self.at(&Token::CloseParen) {
                // Empty parens in grouped declarator — abstract
                self.pos -= 1;
                Declarator::Ident(String::new())
            } else {
                let inner = self.parse_declarator_tree_inner(allow_abstract)?;
                self.expect_token(Token::CloseParen)?;
                inner
            }
        } else if let Some(Token::Identifier(_)) = self.peek() {
            let name = self.parse_identifier()?;
            Declarator::Ident(name)
        } else if allow_abstract {
            // No name — abstract declarator
            Declarator::Ident(String::new())
        } else {
            let name = self.parse_identifier()?;
            Declarator::Ident(name)
        };

        // Trailing suffixes: (params) or [size]
        if self.at(&Token::OpenParen) {
            self.expect_token(Token::OpenParen)?;
            let (
                params,
                param_fts,
                deprecated_params,
                variadic,
                zero_fixed_variadic,
                old_style,
                bounds,
            ) = self.parse_param_list()?;
            self.expect_token(Token::CloseParen)?;
            decl = Declarator::Function(
                params,
                param_fts,
                deprecated_params,
                variadic,
                zero_fixed_variadic,
                old_style,
                bounds,
                Box::new(decl),
            );
        }
        while self.eat(&Token::OpenBracket) {
            let size = self.parse_array_size(true)?;
            self.expect_token(Token::CloseBracket)?;
            decl = Declarator::Array(Box::new(decl), size);
        }

        // Wrap in pointer declarators
        for _ in 0..stars {
            decl = Declarator::Pointer(Box::new(decl));
        }

        Ok(decl)
    }

    /// Parse a declarator using tree-based parsing.
    /// Returns (name, FullType, optional_params)
    fn parse_declarator_full(
        &mut self,
        base_type: CType,
    ) -> ParseResult<(String, FullType, Option<FunctionDeclaratorInfo>)> {
        let tree = self.parse_declarator_tree()?;
        let td_ft = self.last_typedef_full_type.take();
        Ok(Self::process_declarator(&tree, base_type, td_ft.as_ref()))
    }

    /// Parse abstract declarator into a FullType derivation from base type.
    /// Handles: *, (**), (*)[3], (*(*))[N], etc.
    /// Parse an abstract declarator and apply it to the base type.
    /// Uses the same inside-out approach as concrete declarator trees.
    fn parse_abstract_declarator_type(&mut self, base: CType) -> ParseResult<FullType> {
        // Parse into a tree, then process
        let tree = self.parse_abstract_decl_tree()?;
        Ok(Self::process_abstract_tree(&tree, FullType::Scalar(base)))
    }

    /// Abstract declarator tree (mirrors Declarator but without names)
    fn parse_abstract_decl_tree(&mut self) -> ParseResult<AbstractDecl> {
        self.consume_declarator_qualifiers()?;
        let mut stars = 0;
        while self.eat(&Token::Star) {
            stars += 1;
            self.consume_declarator_qualifiers()?;
        }

        let mut decl = if self.eat(&Token::OpenParen) {
            let inner = self.parse_abstract_decl_tree()?;
            self.expect_token(Token::CloseParen)?;
            inner
        } else {
            AbstractDecl::Base
        };

        if self.at(&Token::OpenParen) {
            self.expect_token(Token::OpenParen)?;
            let (_, param_fts, _, variadic, _, _, _) = self.parse_param_list()?;
            self.expect_token(Token::CloseParen)?;
            decl = AbstractDecl::Function(param_fts, variadic, Box::new(decl));
        }

        // Trailing array dims
        while self.eat(&Token::OpenBracket) {
            let size = self.parse_array_size(true)?;
            self.expect_token(Token::CloseBracket)?;
            decl = AbstractDecl::Array(Box::new(decl), size);
        }

        for _ in 0..stars {
            decl = AbstractDecl::Pointer(Box::new(decl));
        }

        Ok(decl)
    }

    fn process_abstract_tree(tree: &AbstractDecl, current_type: FullType) -> FullType {
        match tree {
            AbstractDecl::Base => current_type,
            AbstractDecl::Pointer(inner) => {
                let derived = FullType::Pointer(Box::new(current_type));
                Self::process_abstract_tree(inner, derived)
            }
            AbstractDecl::Array(inner, size) => {
                let derived = FullType::Array {
                    elem: Box::new(current_type),
                    size: *size,
                };
                Self::process_abstract_tree(inner, derived)
            }
            AbstractDecl::Function(params, variadic, inner) => {
                let derived = FullType::Function {
                    return_type: Box::new(current_type),
                    params: params.clone(),
                    variadic: *variadic,
                };
                Self::process_abstract_tree(inner, derived)
            }
        }
    }
}

pub fn parse(tokens: Vec<Token>) -> ParseResult<Program> {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

pub fn parse_with_target(tokens: Vec<Token>, target: Target) -> ParseResult<Program> {
    let mut parser = Parser::new_with_target(tokens, target);
    parser.parse_program()
}

fn parse_token_index(message: &str) -> Option<usize> {
    let marker = "Parse error at token ";
    let start = message.find(marker)? + marker.len();
    let digits: String = message[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

pub fn parse_from_spanned(tokens: Vec<lex::SpannedToken>) -> Result<Program, String> {
    parse_from_spanned_with_target(tokens, Target::host())
}

pub fn parse_from_spanned_with_target(
    tokens: Vec<lex::SpannedToken>,
    target: Target,
) -> Result<Program, String> {
    let plain_tokens: Vec<Token> = tokens.iter().map(|spanned| spanned.token.clone()).collect();
    parse_with_target(plain_tokens, target).map_err(|message| {
        let message = message.trim_start_matches("parse failed: ").to_string();
        let span = parse_token_index(&message)
            .and_then(|index| tokens.get(index).map(|spanned| spanned.span.clone()))
            .or_else(|| {
                tokens.last().map(|spanned| lex::SourceSpan {
                    start: spanned.span.end.clone(),
                    end: spanned.span.end.clone(),
                    start_offset: spanned.span.end_offset,
                    end_offset: spanned.span.end_offset,
                })
            });
        Diagnostic::parse(message, span).render()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex;

    fn parse_source(source: &str) -> ParseResult<Program> {
        parse(lex::lex(source)?)
    }

    fn parse_source_target(source: &str, target: Target) -> ParseResult<Program> {
        parse_with_target(lex::lex(source)?, target)
    }

    fn parse_source_err(source: &str) -> ParseResult<Program> {
        parse(lex::lex(source)?)
    }

    fn parser_source(source: &str) -> ParseResult<Parser> {
        Ok(Parser::new(lex::lex(source)?))
    }

    fn require_err<T>(result: ParseResult<T>, context: &str) -> ParseResult<String> {
        match result {
            Ok(_) => Err(format!("{context} unexpectedly succeeded")),
            Err(err) => Ok(err),
        }
    }

    #[test]
    fn parses_nonconstant_local_array_as_vla_pointer() -> Result<(), String> {
        let program = parse_source("void f(void) { int x; int a[x]; }\n")?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function".to_string());
        };
        let Some(body) = func.body.as_ref() else {
            return Err("expected function body".to_string());
        };
        let Some(vd) = body.iter().find_map(|item| match item {
            BlockItem::Declaration(Declaration::VarDecl(vd))
                if matches!(vd.init, Some(Exp::FunctionCall(ref name, _)) if name == "alloca") =>
            {
                Some(vd)
            }
            _ => None,
        }) else {
            return Err("expected VLA declaration".to_string());
        };
        assert_eq!(vd.var_type, CType::Pointer);
        Ok(())
    }

    #[test]
    fn parse_reports_negative_array_size() -> Result<(), String> {
        let err = require_err(parse_source_err("int a[-1];\n"), "parse should fail")?;
        assert!(err.contains("array size must be non-negative"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_reports_nonconstant_enum_value() -> Result<(), String> {
        let err = require_err(
            parse_source_err("int x; enum E { A = x };\n"),
            "parse should fail",
        )?;
        assert!(err.contains("expected integer constant in enum"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_reports_missing_top_level_semicolon() -> Result<(), String> {
        let err = require_err(parse_source_err("int x"), "parse should fail")?;
        assert!(err.contains("expected Semicolon"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_reports_bad_global_initializer_expression() -> Result<(), String> {
        let err = require_err(parse_source_err("int x = ;\n"), "parse should fail")?;
        assert!(err.contains("expected expression"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_array_init_reports_missing_designator_assign() -> Result<(), String> {
        let mut parser = parser_source("{ [0] 1 }")?;
        let err = require_err(parser.parse_array_init(), "initializer should fail")?;
        assert!(err.contains("expected Assign"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_array_init_reports_missing_designator_close_bracket() -> Result<(), String> {
        let mut parser = parser_source("{ [0 = 1 }")?;
        let err = require_err(parser.parse_array_init(), "initializer should fail")?;
        assert!(err.contains("expected CloseBracket"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_abstract_declarator_reports_missing_close_paren() -> Result<(), String> {
        let mut parser = parser_source("(*[3]")?;
        let err = require_err(
            parser.parse_abstract_declarator_type(CType::Int),
            "abstract declarator should fail",
        )?;
        assert!(err.contains("expected CloseParen"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_alignment_specifier_reports_bad_values() -> Result<(), String> {
        let mut zero = parser_source("alignas(0)")?;
        let err = require_err(zero.parse_alignment_specifier(), "alignment should fail")?;
        assert!(err.contains("alignment must be positive"), "{err}");

        let mut non_power_two = parser_source("alignas(3)")?;
        let err = require_err(
            non_power_two.parse_alignment_specifier(),
            "alignment should fail",
        )?;
        assert!(err.contains("alignment must be a power of two"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_attribute_alignment_reports_trailing_tokens() -> Result<(), String> {
        let parser = parser_source("4 extra")?;
        let err = require_err(
            parser.parse_attribute_alignment("4 extra"),
            "attribute alignment should fail",
        )?;
        assert!(
            err.contains("unexpected tokens in alignment attribute"),
            "{err}"
        );
        Ok(())
    }

    #[test]
    fn parse_attribute_vector_size_reports_bad_values() -> Result<(), String> {
        let parser = parser_source("0")?;
        let err = require_err(
            parser.parse_attribute_vector_size("0"),
            "vector_size should fail",
        )?;
        assert!(err.contains("vector_size must be positive"), "{err}");

        let parser = parser_source("4 extra")?;
        let err = require_err(
            parser.parse_attribute_vector_size("4 extra"),
            "vector_size should fail",
        )?;
        assert!(
            err.contains("unexpected tokens in vector_size attribute"),
            "{err}"
        );
        Ok(())
    }

    #[test]
    fn parse_expression_reports_missing_builtin_argument() -> Result<(), String> {
        for (source, expected) in [
            (
                "__builtin_constant_p()",
                "__builtin_constant_p requires an argument",
            ),
            (
                "__builtin_bswap64()",
                "__builtin_bswap64 requires an argument",
            ),
        ] {
            let mut parser = parser_source(source)?;
            let err = require_err(parser.parse_expression(), "expression should fail")?;
            assert!(err.contains(expected), "{source}: {err}");
        }
        Ok(())
    }

    #[test]
    fn parse_expression_reports_more_missing_builtin_arguments() -> Result<(), String> {
        for (source, expected) in [
            (
                "__builtin_classify_type()",
                "__builtin_classify_type requires an argument",
            ),
            (
                "__builtin_signbit()",
                "__builtin_signbit requires an argument",
            ),
        ] {
            let mut parser = parser_source(source)?;
            let err = require_err(parser.parse_expression(), "expression should fail")?;
            assert!(err.contains(expected), "{source}: {err}");
        }
        Ok(())
    }

    #[test]
    fn parses_nonconstant_abstract_array_bound() -> Result<(), String> {
        let mut parser = parser_source("[x]")?;
        let ft = parser.parse_abstract_declarator_type(CType::Int)?;
        assert_eq!(
            ft,
            FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Int)),
                size: VLA_STATIC_SCALE_FALLBACK
            }
        );
        Ok(())
    }

    #[test]
    fn parses_array_parameter_qualifiers() -> Result<(), String> {
        let program = parse_source(
            "extern int f(char *argv[restrict], int counts[static restrict 4], long values[*]);\n",
        )?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };

        assert_eq!(func.params.len(), 3);
        assert!(matches!(
            &func.param_full_types[0],
            FullType::Pointer(inner) if matches!(inner.as_ref(), FullType::Pointer(_))
        ));
        assert_eq!(
            func.param_full_types[1],
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int)))
        );
        assert_eq!(
            func.param_full_types[2],
            FullType::Pointer(Box::new(FullType::Scalar(CType::Long)))
        );
        Ok(())
    }

    #[test]
    fn parses_post_typedef_parameter_qualifiers_before_pointer() -> Result<(), String> {
        let program = parse_source(
            "typedef unsigned char u_char;\nextern int f(u_char const *, u_char volatile *);\n",
        )?;
        let Declaration::FunDecl(func) = &program.declarations[1] else {
            return Err("expected function declaration".to_string());
        };

        assert_eq!(func.params.len(), 2);
        assert!(matches!(
            &func.param_full_types[0],
            FullType::Pointer(inner) if matches!(inner.as_ref(), FullType::Scalar(CType::UChar))
        ));
        assert!(matches!(
            &func.param_full_types[1],
            FullType::Pointer(inner) if matches!(inner.as_ref(), FullType::Scalar(CType::UChar))
        ));
        Ok(())
    }

    #[test]
    fn parses_empty_top_level_declarations() -> Result<(), String> {
        let program = parse_source("; struct s { int x; }; ; int y;\n")?;
        assert!(matches!(
            program.declarations.as_slice(),
            [
                Declaration::TypedefDecl,
                Declaration::StructDecl(_),
                Declaration::TypedefDecl,
                Declaration::VarDecl(_)
            ]
        ));
        Ok(())
    }

    #[test]
    fn parses_comma_separated_typedef_declarators() -> Result<(), String> {
        let program =
            parse_source("typedef unsigned long word_t, *word_ptr_t;\nword_t x;\nword_ptr_t p;\n")?;
        let Declaration::VarDecl(x) = &program.declarations[1] else {
            return Err("expected word_t variable".to_string());
        };
        let Declaration::VarDecl(p) = &program.declarations[2] else {
            return Err("expected word_ptr_t variable".to_string());
        };

        assert_eq!(x.var_type, CType::ULong);
        assert_eq!(
            p.decl_full_type,
            Some(FullType::Pointer(Box::new(FullType::Scalar(CType::ULong))))
        );
        Ok(())
    }

    #[test]
    fn parses_static_asserts_inside_struct_members() -> Result<(), String> {
        let program = parse_source(
            "struct s { _Static_assert(sizeof(int) == 4, \"int size\"); int x; static_assert(1); };\n",
        )?;
        let Declaration::StructDecl(decl) = &program.declarations[0] else {
            return Err("expected struct declaration".to_string());
        };

        assert_eq!(decl.members.len(), 1);
        assert_eq!(decl.members[0].name, "x");
        Ok(())
    }

    #[test]
    fn rejects_failed_static_asserts_inside_struct_members() -> Result<(), String> {
        let err = parse_source("struct s { _Static_assert(0, \"bad\"); int x; };\n")
            .expect_err("expected static assertion failure");

        if !err.contains("static assertion failed") {
            return Err(format!("unexpected error: {err}"));
        }
        Ok(())
    }

    #[test]
    fn parse_expression_reports_missing_rhs() -> Result<(), String> {
        let mut parser = parser_source("1 +")?;
        let err = require_err(parser.parse_expression(), "expression should fail")?;
        assert!(err.contains("expected expression"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_expression_reports_missing_close_paren() -> Result<(), String> {
        let mut parser = parser_source("(1 + 2")?;
        let err = require_err(parser.parse_expression(), "expression should fail")?;
        assert!(err.contains("expected CloseParen"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_expression_reports_missing_function_call_close_paren() -> Result<(), String> {
        let mut parser = parser_source("f(1, 2")?;
        let err = require_err(parser.parse_expression(), "expression should fail")?;
        assert!(err.contains("expected CloseParen"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_statement_reports_missing_return_semicolon() -> Result<(), String> {
        let mut parser = parser_source("return 1")?;
        let err = require_err(parser.parse_statement(), "statement should fail")?;
        assert!(err.contains("expected Semicolon"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_statement_reports_missing_if_close_paren() -> Result<(), String> {
        let mut parser = parser_source("if (x return 1;")?;
        let err = require_err(parser.parse_statement(), "statement should fail")?;
        assert!(err.contains("expected CloseParen"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_statement_reports_missing_goto_label() -> Result<(), String> {
        let mut parser = parser_source("goto ;")?;
        let err = require_err(parser.parse_statement(), "statement should fail")?;
        assert!(err.contains("expected identifier"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_block_reports_missing_close_brace() -> Result<(), String> {
        let mut parser = parser_source("{ return 1;")?;
        let err = require_err(parser.parse_block(), "block should fail")?;
        assert!(err.contains("expected CloseBrace"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_block_reports_bad_local_initializer() -> Result<(), String> {
        let mut parser = parser_source("{ int x = ; }")?;
        let err = require_err(parser.parse_block(), "block should fail")?;
        assert!(err.contains("expected expression"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_block_item_accepts_nested_function_definition() -> Result<(), String> {
        let mut parser = parser_source("int f() { return 1; }")?;
        let item = parser.parse_block_item()?;
        assert!(matches!(
            item,
            BlockItem::Declaration(Declaration::FunDecl(FunctionDeclaration {
                body: Some(_),
                ..
            }))
        ));
        Ok(())
    }

    #[test]
    fn parse_block_item_accepts_old_style_nested_function_definition() -> Result<(), String> {
        let program = parse_source(
            "int outer(int x, ...) { __builtin_va_list a; __builtin_va_start(a, x); int bar(c) int c[1][__builtin_va_arg(a, int)]; { return sizeof c[0]; } return 0; }",
        )?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected outer function".to_string());
        };
        let Some(body) = func.body.as_ref() else {
            return Err("expected outer function body".to_string());
        };
        assert!(body.iter().any(|item| matches!(
            item,
            BlockItem::Declaration(Declaration::FunDecl(FunctionDeclaration {
                name,
                old_style: true,
                body: Some(_),
                ..
            })) if name == "bar"
        )));
        Ok(())
    }

    #[test]
    fn parse_var_declaration_reports_missing_semicolon() -> Result<(), String> {
        let mut parser = parser_source("int x = 1")?;
        let err = require_err(parser.parse_var_declaration(), "declaration should fail")?;
        assert!(err.contains("expected Semicolon"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_var_declaration_reports_bad_initializer_expression() -> Result<(), String> {
        let mut parser = parser_source("int x = ;")?;
        let err = require_err(parser.parse_var_declaration(), "declaration should fail")?;
        assert!(err.contains("expected expression"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_statement_reports_bad_for_declaration_initializer() -> Result<(), String> {
        let mut parser = parser_source("for (int i = ; i < 3; i = i + 1) ;")?;
        let err = require_err(parser.parse_statement(), "statement should fail")?;
        assert!(err.contains("expected expression"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_declarator_reports_missing_identifier() -> Result<(), String> {
        let mut parser = parser_source(";")?;
        let err = require_err(
            parser.parse_declarator_full(CType::Int),
            "declarator should fail",
        )?;
        assert!(err.contains("expected identifier"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_declarator_reports_missing_group_close_paren() -> Result<(), String> {
        let mut parser = parser_source("(*p")?;
        let err = require_err(
            parser.parse_declarator_full(CType::Int),
            "declarator should fail",
        )?;
        assert!(err.contains("expected CloseParen"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_declarator_reports_missing_array_close_bracket() -> Result<(), String> {
        let mut parser = parser_source("a[2")?;
        let err = require_err(
            parser.parse_declarator_full(CType::Int),
            "declarator should fail",
        )?;
        assert!(err.contains("expected CloseBracket"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_param_list_reports_bad_parameter_declarator() -> Result<(), String> {
        let mut parser = parser_source("int (*")?;
        let err = require_err(parser.parse_param_list(), "parameter list should fail")?;
        assert!(err.contains("expected type specifier"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_struct_members_accepts_missing_final_member_semicolon() -> Result<(), String> {
        let mut parser = parser_source("{ int x }")?;
        parser.parse_struct_members()?;
        Ok(())
    }

    #[test]
    fn parse_struct_members_reports_missing_close_brace() -> Result<(), String> {
        let mut parser = parser_source("{ int x;")?;
        let err = require_err(parser.parse_struct_members(), "struct members should fail")?;
        assert!(err.contains("expected CloseBrace"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_struct_type_specifier_reports_missing_struct_keyword() -> Result<(), String> {
        let mut parser = parser_source("int")?;
        let err = require_err(
            parser.parse_struct_type_specifier(),
            "struct type should fail",
        )?;
        assert!(err.contains("expected KWStruct"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_specifiers_reports_missing_type() -> Result<(), String> {
        let mut parser = parser_source("static ;")?;
        let err = require_err(parser.parse_specifiers(), "specifiers should fail")?;
        assert!(err.contains("expected type specifier"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_reports_missing_type_after_storage_class() -> Result<(), String> {
        let err = require_err(parse_source_err("static ;\n"), "parse should fail")?;
        assert!(err.contains("expected type specifier"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_preserves_thread_local_storage_combinations() -> Result<(), String> {
        let program = parse_source(
            "_Thread_local int a;\n\
             static _Thread_local int b;\n\
             extern __thread int c;\n",
        )?;
        let storage_classes: Vec<Option<StorageClass>> = program
            .declarations
            .iter()
            .filter_map(|decl| {
                if let Declaration::VarDecl(vd) = decl {
                    Some(vd.storage_class.clone())
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(
            storage_classes,
            vec![
                Some(StorageClass::ThreadLocal),
                Some(StorageClass::StaticThreadLocal),
                Some(StorageClass::ExternThreadLocal),
            ]
        );
        Ok(())
    }

    #[test]
    fn parser_typedef_stack_underflow_reports_parse_error() -> Result<(), String> {
        let mut parser = Parser::new(Vec::new());
        parser.typedef_scopes.clear();

        let err = require_err(
            parser.add_typedef(
                "T".to_string(),
                TypedefInfo {
                    base_type: CType::Int,
                    full_type: FullType::Scalar(CType::Int),
                    struct_tag: None,
                    is_enum: false,
                    vla_size: None,
                    alignment: None,
                },
            ),
            "missing typedef scope should fail",
        )?;

        assert!(err.contains("parser typedef scope stack is empty"), "{err}");
        Ok(())
    }

    #[test]
    fn parser_enum_stack_underflow_reports_parse_error() -> Result<(), String> {
        let mut parser = Parser::new(Vec::new());
        parser.enum_scopes.clear();

        let err = require_err(
            parser.add_enum_constant("E".to_string(), 1),
            "missing enum scope should fail",
        )?;

        assert!(err.contains("parser enum scope stack is empty"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_type_reports_missing_type() -> Result<(), String> {
        let mut parser = parser_source(")")?;
        let err = require_err(parser.parse_type(), "type should fail")?;
        assert!(err.contains("expected type specifier"), "{err}");
        Ok(())
    }

    #[test]
    fn parse_expression_reports_bad_sizeof_type() -> Result<(), String> {
        let mut parser = parser_source("sizeof(;)")?;
        let err = require_err(parser.parse_expression(), "expression should fail")?;
        assert!(err.contains("expected expression"), "{err}");
        Ok(())
    }

    #[test]
    fn parses_long_double_parameters_for_target() -> Result<(), String> {
        let program = parse_source_target(
            "extern long double f(long double x);\n",
            Target::x86_64_linux(),
        )?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };
        assert_eq!(func.return_type, CType::LongDouble);
        assert_eq!(func.params[0].1, CType::LongDouble);

        let program = parse_source_target(
            "extern long double f(long double x);\n",
            Target::aarch64_macos(),
        )?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };
        assert_eq!(func.return_type, CType::Double);
        assert_eq!(func.params[0].1, CType::Double);
        Ok(())
    }

    #[test]
    fn parses_float_as_distinct_type() -> Result<(), String> {
        let program = parse_source("extern float f(float x);\n")?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };
        assert_eq!(func.return_type, CType::Float);
        assert_eq!(func.params[0].1, CType::Float);
        Ok(())
    }

    #[test]
    fn parses_gnu_signed_aliases_as_signed_specifiers() -> Result<(), String> {
        let program = parse_source("__signed char a;\n__signed__ int b;\n")?;
        let Declaration::VarDecl(a) = &program.declarations[0] else {
            return Err("expected a declaration".to_string());
        };
        assert_eq!(a.var_type, CType::SChar);
        assert_eq!(a.decl_full_type, Some(FullType::Scalar(CType::SChar)));
        let Declaration::VarDecl(b) = &program.declarations[1] else {
            return Err("expected b declaration".to_string());
        };
        assert_eq!(b.var_type, CType::Int);
        assert_eq!(b.decl_full_type, Some(FullType::Scalar(CType::Int)));
        Ok(())
    }

    #[test]
    fn parses_gnu_qualifier_aliases_as_type_qualifiers() -> Result<(), String> {
        let program =
            parse_source("__const int * __volatile p;\n__const__ int * __volatile__ q;\n")?;
        for declaration in &program.declarations {
            let Declaration::VarDecl(var) = declaration else {
                return Err("expected a variable declaration".to_string());
            };
            assert_eq!(var.var_type, CType::Pointer);
            assert_eq!(
                var.decl_full_type,
                Some(FullType::Pointer(Box::new(FullType::Scalar(CType::Int))))
            );
        }
        Ok(())
    }

    #[test]
    fn parses_builtin_float_type_names_as_double() -> Result<(), String> {
        let program = parse_source("extern _Float16 f(_Float64 x, __float128 y, __fp16 z);\n")?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };
        assert_eq!(func.return_type, CType::Double);
        assert_eq!(func.params[0].1, CType::Double);
        assert_eq!(func.params[1].1, CType::Double);
        assert_eq!(func.params[2].1, CType::Double);
        Ok(())
    }

    #[test]
    fn parses_builtin_float_names_as_typedef_declarators_after_float() -> Result<(), String> {
        let program = parse_source(
            "typedef float _Float32;\ntypedef float __fp16;\n_Float32 x;\n__fp16 y;\n",
        )?;
        assert_eq!(program.declarations.len(), 4);
        let Declaration::VarDecl(x) = &program.declarations[2] else {
            return Err("expected variable declaration".to_string());
        };
        assert_eq!(x.var_type, CType::Float);
        assert_eq!(x.decl_full_type, Some(FullType::Scalar(CType::Float)));
        let Declaration::VarDecl(y) = &program.declarations[3] else {
            return Err("expected variable declaration".to_string());
        };
        assert_eq!(y.var_type, CType::Float);
        assert_eq!(y.decl_full_type, Some(FullType::Scalar(CType::Float)));
        Ok(())
    }

    #[test]
    fn parses_bfloat16_vector_typedefs() -> Result<(), String> {
        let program =
            parse_source("typedef __bf16 __v16bf __attribute__((vector_size(32)));\n__v16bf x;\n")?;
        let Declaration::VarDecl(var) = &program.declarations[1] else {
            return Err("expected variable declaration".to_string());
        };
        assert_eq!(var.var_type, CType::Double);
        assert_eq!(
            var.decl_full_type,
            Some(FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Double)),
                lanes: 4,
                complex: false,
            })
        );
        Ok(())
    }

    #[test]
    fn parses_builtin_float_complex_type_names() -> Result<(), String> {
        let program = parse_source(
            "extern _Float16 _Complex f(_Complex _Float16 x, _Float32 _Complex y);\n",
        )?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };
        let complex_double = FullType::Vector {
            elem: Box::new(FullType::Scalar(CType::Double)),
            lanes: 2,
            complex: true,
        };
        assert_eq!(func.return_type, CType::Double);
        assert_eq!(func.return_full_type, Some(complex_double.clone()));
        assert_eq!(func.params[0].1, CType::Double);
        assert_eq!(func.params[1].1, CType::Double);
        assert_eq!(func.param_full_types[0], complex_double);
        assert_eq!(func.param_full_types[1], complex_double);
        Ok(())
    }

    #[test]
    fn parses_complex_type_specifiers_as_float_compatibility_aliases() -> Result<(), String> {
        let program = parse_source(
            "extern float _Complex cacosf(float _Complex x);\nextern _Complex double cacos(_Complex z);\n",
        )?;
        let Declaration::FunDecl(cacosf) = &program.declarations[0] else {
            return Err("expected cacosf declaration".to_string());
        };
        let Declaration::FunDecl(cacos) = &program.declarations[1] else {
            return Err("expected cacos declaration".to_string());
        };

        assert_eq!(cacosf.return_type, CType::Float);
        assert_eq!(cacosf.params[0].1, CType::Float);
        assert_eq!(cacos.return_type, CType::Double);
        assert_eq!(cacos.params[0].1, CType::Double);
        Ok(())
    }

    #[test]
    fn parses_complex_type_specifiers_with_rich_type_metadata() -> Result<(), String> {
        let program = parse_source(
            "float _Complex cf;\n_Complex double cd;\n__complex__ float alias;\n_Complex bare;\n",
        )?;
        let Declaration::VarDecl(cf) = &program.declarations[0] else {
            return Err("expected cf declaration".to_string());
        };
        let Declaration::VarDecl(cd) = &program.declarations[1] else {
            return Err("expected cd declaration".to_string());
        };
        let Declaration::VarDecl(alias) = &program.declarations[2] else {
            return Err("expected alias declaration".to_string());
        };
        let Declaration::VarDecl(bare) = &program.declarations[3] else {
            return Err("expected bare declaration".to_string());
        };

        assert_eq!(
            cf.decl_full_type,
            Some(FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Float)),
                lanes: 2,
                complex: true,
            })
        );
        assert_eq!(
            cd.decl_full_type,
            Some(FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Double)),
                lanes: 2,
                complex: true,
            })
        );
        assert_eq!(
            alias.decl_full_type,
            Some(FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Float)),
                lanes: 2,
                complex: true,
            })
        );
        assert_eq!(
            bare.decl_full_type,
            Some(FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Double)),
                lanes: 2,
                complex: true,
            })
        );
        Ok(())
    }

    #[test]
    fn parses_builtin_int128_typedef_names_as_scalar_types() -> Result<(), String> {
        let program =
            parse_source("__uint128_t vector[4];\n__int128_t scalar;\nunsigned __int128 word;\n")?;
        let Declaration::VarDecl(vector) = &program.declarations[0] else {
            return Err("expected vector declaration".to_string());
        };
        let Declaration::VarDecl(scalar) = &program.declarations[1] else {
            return Err("expected scalar declaration".to_string());
        };
        let Declaration::VarDecl(word) = &program.declarations[2] else {
            return Err("expected word declaration".to_string());
        };

        assert_eq!(
            vector.decl_full_type,
            Some(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::UInt128)),
                size: 4,
            })
        );
        assert_eq!(scalar.decl_full_type, Some(FullType::Scalar(CType::Int128)));
        assert_eq!(word.decl_full_type, Some(FullType::Scalar(CType::UInt128)));
        Ok(())
    }

    #[test]
    fn parses_exact_width_bitint_specifiers() -> Result<(), String> {
        let program = parse_source(
            "_BitInt(32) a;\nunsigned _BitInt(32) b;\n_BitInt(64) c;\nunsigned _BitInt(64) d;\n_BitInt(128) e;\nunsigned _BitInt(128) f;\n",
        )?;
        let Declaration::VarDecl(a) = &program.declarations[0] else {
            return Err("expected a declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[1] else {
            return Err("expected b declaration".to_string());
        };
        let Declaration::VarDecl(c) = &program.declarations[2] else {
            return Err("expected c declaration".to_string());
        };
        let Declaration::VarDecl(d) = &program.declarations[3] else {
            return Err("expected d declaration".to_string());
        };
        let Declaration::VarDecl(e) = &program.declarations[4] else {
            return Err("expected e declaration".to_string());
        };
        let Declaration::VarDecl(f) = &program.declarations[5] else {
            return Err("expected f declaration".to_string());
        };

        assert_eq!(a.var_type, CType::Int);
        assert_eq!(a.decl_full_type, Some(FullType::Scalar(CType::Int)));
        assert_eq!(b.var_type, CType::UInt);
        assert_eq!(b.decl_full_type, Some(FullType::Scalar(CType::UInt)));
        assert_eq!(c.var_type, CType::Long);
        assert_eq!(c.decl_full_type, Some(FullType::Scalar(CType::Long)));
        assert_eq!(d.var_type, CType::ULong);
        assert_eq!(d.decl_full_type, Some(FullType::Scalar(CType::ULong)));
        assert_eq!(e.var_type, CType::Int128);
        assert_eq!(e.decl_full_type, Some(FullType::Scalar(CType::Int128)));
        assert_eq!(f.var_type, CType::UInt128);
        assert_eq!(f.decl_full_type, Some(FullType::Scalar(CType::UInt128)));
        Ok(())
    }

    #[test]
    fn bitint_spec_tracks_width_signedness_and_storage() -> Result<(), String> {
        assert_eq!(
            Parser::bitint_spec(32, false)?,
            BitIntSpec {
                width: 32,
                unsigned: false,
                storage: CType::Int,
            }
        );
        assert_eq!(
            Parser::bitint_spec(128, true)?,
            BitIntSpec {
                width: 128,
                unsigned: true,
                storage: CType::UInt128,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_bitint_type_names_in_casts_and_sizeof() -> Result<(), String> {
        let program = parse_source("int a = (_BitInt(32))1;\nint b[sizeof(_BitInt(64))];\n")?;
        let Declaration::VarDecl(a) = &program.declarations[0] else {
            return Err("expected a declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[1] else {
            return Err("expected b declaration".to_string());
        };

        assert!(matches!(a.init, Some(Exp::Cast(CType::Int, _, _))));
        assert_eq!(b.array_dims, Some(vec![8]));
        Ok(())
    }

    #[test]
    fn evaluates_bitint_unsigned_wrap_in_integer_constant_contexts() -> Result<(), String> {
        let program = parse_source(
            "_Static_assert(((unsigned _BitInt(32))0U - 1U) == 4294967295U, \"wrap\");\n\
             enum { N = ((unsigned _BitInt(32))0U - 1U) == 4294967295U ? 5 : -1 };\n\
             int a[N];\n",
        )?;
        let Some(a) = program
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::VarDecl(vd) if vd.name == "a" => Some(vd),
                _ => None,
            })
        else {
            return Err("expected array declaration".to_string());
        };

        assert_eq!(a.array_dims, Some(vec![5]));
        Ok(())
    }

    #[test]
    fn rejects_bitint_widths_without_exact_storage() -> Result<(), String> {
        let err = require_err(parse_source_err("_BitInt(5) x;\n"), "parse should fail")?;
        assert!(
            err.contains("_BitInt width is not supported by an exact storage type"),
            "{err}"
        );

        let err = require_err(parse_source_err("_BitInt(16) x;\n"), "parse should fail")?;
        assert!(
            err.contains("_BitInt width is not supported by an exact storage type"),
            "{err}"
        );

        let err = require_err(parse_source_err("_BitInt(1) x;\n"), "parse should fail")?;
        assert!(
            err.contains("signed _BitInt width must be greater than 1"),
            "{err}"
        );

        let err = require_err(
            parse_source_err("unsigned _BitInt(1) x;\n"),
            "parse should fail",
        )?;
        assert!(
            err.contains("_BitInt width is not supported by an exact storage type"),
            "{err}"
        );

        Ok(())
    }

    #[test]
    fn rejects_malformed_bitint_specifiers() -> Result<(), String> {
        let err = require_err(parse_source_err("_BitInt() x;\n"), "parse should fail")?;
        assert!(
            err.contains("expected integer constant _BitInt width"),
            "{err}"
        );

        let err = require_err(parse_source_err("_BitInt(-1) x;\n"), "parse should fail")?;
        assert!(err.contains("_BitInt width must be positive"), "{err}");

        let err = require_err(parse_source_err("_BitInt(foo) x;\n"), "parse should fail")?;
        assert!(
            err.contains("expected integer constant _BitInt width"),
            "{err}"
        );

        Ok(())
    }

    #[test]
    fn rejects_bitint_with_other_type_specifiers() -> Result<(), String> {
        for src in [
            "_BitInt(32) int x;\n",
            "long _BitInt(64) x;\n",
            "char _BitInt(32) x;\n",
            "float _BitInt(32) x;\n",
            "int x = sizeof(_BitInt(32) int);\n",
        ] {
            let err = require_err(parse_source_err(src), "parse should fail")?;
            assert!(
                err.contains("_BitInt cannot be combined with another type specifier"),
                "{src}: {err}"
            );
        }

        Ok(())
    }

    #[test]
    fn rejects_duplicate_bitint_specifiers() -> Result<(), String> {
        for src in [
            "_BitInt(32) _BitInt(64) x;\n",
            "int x = sizeof(_BitInt(32) _BitInt(64));\n",
        ] {
            let err = require_err(parse_source_err(src), "parse should fail")?;
            assert!(
                err.contains("duplicate _BitInt type specifier"),
                "{src}: {err}"
            );
        }

        Ok(())
    }

    #[test]
    fn preserves_variadic_function_declaration_metadata() -> Result<(), String> {
        let program = parse_source("extern int printf(const char *fmt, ...);\n")?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };
        assert_eq!(func.name, "printf");
        assert_eq!(func.params.len(), 1);
        assert!(func.variadic);
        Ok(())
    }

    #[test]
    fn preserves_initializer_designators_in_ast() -> Result<(), String> {
        let program = parse_source("int a[4] = { [2] = 9 };\n")?;
        let Declaration::VarDecl(var) = &program.declarations[0] else {
            return Err("expected variable declaration".to_string());
        };
        let Some(Exp::ArrayInit(elems)) = &var.init else {
            return Err("expected array initializer".to_string());
        };
        assert!(matches!(
            &elems[0],
            Exp::DesignatedInit(designators, value)
                if matches!(&designators[0], Designator::Index(_))
                    && matches!(value.as_ref(), Exp::Constant(9))
        ));
        Ok(())
    }

    #[test]
    fn parses_constant_expression_array_bounds() -> Result<(), String> {
        let program = parse_source("enum { N = 2 + 1 }; int a[N * 2];\n")?;
        let Declaration::VarDecl(var) = &program.declarations[1] else {
            return Err("expected variable declaration".to_string());
        };
        assert_eq!(var.array_dims, Some(vec![6]));
        Ok(())
    }

    #[test]
    fn infers_unsized_array_bounds_from_initializers() -> Result<(), String> {
        let program = parse_source(
            "char s[] = \"abc\"; int a[] = { 1, 2, 3 }; int b[] = { [4] = 1 };
             struct P { char *s; int *p; }; struct P p[] = { \"\", 0, \"\", 0 };\n",
        )?;
        let Declaration::VarDecl(s) = &program.declarations[0] else {
            return Err("expected string array declaration".to_string());
        };
        let Declaration::VarDecl(a) = &program.declarations[1] else {
            return Err("expected array declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[2] else {
            return Err("expected designated array declaration".to_string());
        };
        let Declaration::VarDecl(p) = &program.declarations[4] else {
            return Err("expected struct array declaration".to_string());
        };
        assert_eq!(s.array_dims, Some(vec![4]));
        assert_eq!(a.array_dims, Some(vec![3]));
        assert_eq!(b.array_dims, Some(vec![5]));
        assert_eq!(p.array_dims, Some(vec![2]));
        Ok(())
    }

    #[test]
    fn parses_trailing_flexible_array_member() -> Result<(), String> {
        let program = parse_source("struct packet { int len; char data[]; };\n")?;
        let Declaration::StructDecl(decl) = &program.declarations[0] else {
            return Err("expected struct declaration".to_string());
        };
        assert_eq!(decl.members.len(), 2);
        assert!(decl.members[1].flexible_array);
        assert!(matches!(
            decl.members[1].member_full_type,
            FullType::Array { size: 0, .. }
        ));
        Ok(())
    }

    #[test]
    fn parses_gnu_zero_length_array_member_without_flexible_rules() -> Result<(), String> {
        let program = parse_source("struct zero { int data[0]; };\n")?;
        let Declaration::StructDecl(decl) = &program.declarations[0] else {
            return Err("expected struct declaration".to_string());
        };
        assert_eq!(decl.members.len(), 1);
        assert!(!decl.members[0].flexible_array);
        assert!(matches!(
            decl.members[0].member_full_type,
            FullType::Array { size: 0, .. }
        ));
        Ok(())
    }

    #[test]
    fn rejects_non_trailing_flexible_array_member() -> Result<(), String> {
        let err = require_err(
            parse_source_err("struct bad { int len; char data[]; int tail; };\n"),
            "parse should fail",
        )?;
        assert!(err.contains("flexible array member must be last"), "{err}");
        Ok(())
    }

    #[test]
    fn parses_unshadowed_true_false_as_c23_constants() -> Result<(), String> {
        let program = parse_source("int a = true;\nint b = false;\n")?;
        let Declaration::VarDecl(a) = &program.declarations[0] else {
            return Err("expected a declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[1] else {
            return Err("expected b declaration".to_string());
        };
        assert!(matches!(a.init, Some(Exp::Constant(1))));
        assert!(matches!(b.init, Some(Exp::Constant(0))));
        Ok(())
    }

    #[test]
    fn true_false_constants_do_not_hide_visible_objects() -> Result<(), String> {
        let program = parse_source("int true;\nint false;\nint a = true;\nint b = false;\n")?;
        let Declaration::VarDecl(a) = &program.declarations[2] else {
            return Err("expected a declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[3] else {
            return Err("expected b declaration".to_string());
        };
        assert!(matches!(a.init, Some(Exp::Var(ref name)) if name == "true"));
        assert!(matches!(b.init, Some(Exp::Var(ref name)) if name == "false"));
        Ok(())
    }

    #[test]
    fn parses_unshadowed_nullptr_as_c23_null_pointer_constant() -> Result<(), String> {
        let program = parse_source("int a = nullptr;\nint b = __nullptr;\n")?;
        let Declaration::VarDecl(a) = &program.declarations[0] else {
            return Err("expected a declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[1] else {
            return Err("expected b declaration".to_string());
        };
        assert!(matches!(
            a.init,
            Some(Exp::Cast(CType::Pointer, Some(FullType::Pointer(_)), _))
        ));
        assert!(matches!(
            b.init,
            Some(Exp::Cast(CType::Pointer, Some(FullType::Pointer(_)), _))
        ));
        Ok(())
    }

    #[test]
    fn nullptr_constant_does_not_hide_visible_objects() -> Result<(), String> {
        let program = parse_source("int nullptr;\nint a = nullptr;\n")?;
        let Declaration::VarDecl(a) = &program.declarations[1] else {
            return Err("expected a declaration".to_string());
        };
        assert!(matches!(a.init, Some(Exp::Var(ref name)) if name == "nullptr"));
        Ok(())
    }

    #[test]
    fn parses_generic_selection_to_selected_expression() -> Result<(), String> {
        let program = parse_source(
            "int x; long y; int a = _Generic(x, int: 1, long: 2, default: 3); int b = _Generic(y, int: 1, long: 2, default: 3);\n",
        )?;
        let Declaration::VarDecl(a) = &program.declarations[2] else {
            return Err("expected variable declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[3] else {
            return Err("expected variable declaration".to_string());
        };
        assert!(matches!(a.init, Some(Exp::Constant(1))));
        assert!(matches!(b.init, Some(Exp::Constant(2))));
        Ok(())
    }

    #[test]
    fn parses_generic_selection_with_controlling_type() -> Result<(), String> {
        let program = parse_source(
            "typedef unsigned long size_type;\n\
             int a = _Generic(int, int: 1, long: 2, default: 3);\n\
             int b = _Generic(size_type, int: 1, unsigned long: 2, default: 3);\n",
        )?;
        let Declaration::VarDecl(a) = &program.declarations[1] else {
            return Err("expected variable declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[2] else {
            return Err("expected variable declaration".to_string());
        };
        assert!(matches!(a.init, Some(Exp::Constant(1))));
        assert!(matches!(b.init, Some(Exp::Constant(2))));
        Ok(())
    }

    #[test]
    fn generic_selection_reports_missing_match() -> Result<(), String> {
        let mut parser = parser_source("_Generic(1, long: 2)")?;
        let err = require_err(parser.parse_expression(), "generic selection should fail")?;
        assert!(err.contains("no matching generic association"), "{err}");
        Ok(())
    }
}
