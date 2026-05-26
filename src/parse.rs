#![allow(dead_code)]

use crate::diagnostic::Diagnostic;
use crate::lex;
use crate::types::*;

type ParseResult<T> = Result<T, String>;
const MAX_SUPPORTED_ALIGNMENT: usize = 1 << 30;

#[derive(Debug, Clone, Copy, Default)]
struct AggregateAttributes {
    packed: bool,
    alignment: Option<std::num::NonZeroUsize>,
}

#[derive(Debug, Clone, Copy, Default)]
struct MemberAttributes {
    alignment: Option<std::num::NonZeroUsize>,
    packed: bool,
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
    Function(Vec<ParamDecl>, Vec<FullType>, bool, Box<Declarator>),
}

#[derive(Debug, Clone)]
struct FunctionDeclaratorInfo {
    params: Vec<ParamDecl>,
    param_full_types: Vec<FullType>,
    variadic: bool,
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
}

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    last_struct_tag: Option<String>,
    /// Scoped typedef table: each scope maps typedef names to their resolved type info.
    typedef_scopes: Vec<std::collections::HashMap<String, TypedefInfo>>,
    /// Struct/union definitions encountered during type specifier parsing
    pending_struct_decls: Vec<StructDeclaration>,
    /// Parser-time struct/union layouts for constant-expression helpers like offsetof.
    struct_defs: std::collections::HashMap<String, StructDef>,
    /// Full type from the last typedef used as a type specifier
    last_typedef_full_type: Option<FullType>,
    /// Scoped enum constant table: maps constant names to their integer values.
    enum_scopes: Vec<std::collections::HashMap<String, i64>>,
    /// Scoped object table for parser-time typeof(expr) support.
    value_scopes: Vec<std::collections::HashMap<String, FullType>>,
    /// Extra block items from multi-declarator parsing (e.g., `int x, y;`)
    pending_block_items: Vec<BlockItem>,
    /// Extra top-level declarations from multi-declarator parsing
    pending_declarations: Vec<Declaration>,
    /// Alignment specifier collected while parsing declaration specifiers.
    pending_alignment: Option<std::num::NonZeroUsize>,
    /// True when declaration specifiers used GNU __auto_type.
    pending_auto_type: bool,
    /// True when declaration specifiers/attributes mark a function as noreturn.
    pending_noreturn: bool,
}

impl Parser {
    fn merge_aggregate_attributes(
        prefix: AggregateAttributes,
        suffix: AggregateAttributes,
    ) -> AggregateAttributes {
        AggregateAttributes {
            packed: prefix.packed || suffix.packed,
            alignment: match (prefix.alignment, suffix.alignment) {
                (Some(prefix), Some(suffix)) => Some(prefix.max(suffix)),
                (Some(prefix), None) => Some(prefix),
                (None, Some(suffix)) => Some(suffix),
                (None, None) => None,
            },
        }
    }

    pub fn new(tokens: Vec<Token>) -> Self {
        let mut builtin_typedefs = std::collections::HashMap::new();
        builtin_typedefs.insert(
            "__builtin_va_list".to_string(),
            TypedefInfo {
                base_type: CType::Pointer,
                full_type: FullType::Pointer(Box::new(FullType::Scalar(CType::Char))),
                struct_tag: None,
            },
        );
        builtin_typedefs.insert(
            "__gnuc_va_list".to_string(),
            TypedefInfo {
                base_type: CType::Pointer,
                full_type: FullType::Pointer(Box::new(FullType::Scalar(CType::Char))),
                struct_tag: None,
            },
        );
        for name in ["__int128_t", "__uint128_t"] {
            builtin_typedefs.insert(
                name.to_string(),
                TypedefInfo {
                    base_type: CType::ULong,
                    full_type: FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::ULong)),
                        size: 2,
                    },
                    struct_tag: None,
                },
            );
        }

        Parser {
            tokens,
            pos: 0,
            last_struct_tag: None,
            typedef_scopes: vec![builtin_typedefs],
            pending_struct_decls: Vec::new(),
            struct_defs: std::collections::HashMap::new(),
            last_typedef_full_type: None,
            enum_scopes: vec![std::collections::HashMap::new()],
            value_scopes: vec![std::collections::HashMap::new()],
            pending_block_items: Vec::new(),
            pending_declarations: Vec::new(),
            pending_alignment: None,
            pending_auto_type: false,
            pending_noreturn: false,
        }
    }

    fn push_typedef_scope(&mut self) {
        self.typedef_scopes.push(std::collections::HashMap::new());
        self.enum_scopes.push(std::collections::HashMap::new());
        self.value_scopes.push(std::collections::HashMap::new());
    }

    fn pop_typedef_scope(&mut self) {
        self.typedef_scopes.pop();
        self.enum_scopes.pop();
        self.value_scopes.pop();
    }

    fn add_typedef(&mut self, name: String, info: TypedefInfo) -> ParseResult<()> {
        let Some(scope) = self.typedef_scopes.last_mut() else {
            return Err(self.format_error("parser typedef scope stack is empty"));
        };
        scope.insert(name, info);
        Ok(())
    }

    fn lookup_typedef(&self, name: &str) -> Option<&TypedefInfo> {
        for scope in self.typedef_scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    fn is_typedef_name(&self, name: &str) -> bool {
        self.lookup_typedef(name).is_some()
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
        )
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
        if ctype == CType::Void && array_dims.is_none() {
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
        let var_type = if array_dims.is_some() {
            let mut t = &full_type;
            while let FullType::Array { elem, .. } = t {
                t = elem;
            }
            t.to_ctype()
        } else {
            ctype
        };
        Ok(VarDeclaration {
            name,
            var_type,
            ptr_info: pi,
            array_dims,
            decl_full_type: Some(full_type),
            init,
            storage_class: sc,
            alignment,
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
            init: Some(init),
            storage_class: sc,
            alignment,
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
            Exp::ArrayInit(elems) if elem.to_ctype().is_char() => match elems.as_slice() {
                [Exp::StringLiteral(s)] => c_string_byte_len(s) + 1,
                _ => self.infer_array_init_len(elems)?,
            },
            Exp::ArrayInit(elems) => self.infer_array_init_len(elems)?,
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
        let Some(scope) = self.value_scopes.last_mut() else {
            return Err(self.format_error("parser value scope stack is empty"));
        };
        scope.insert(name, full_type);
        Ok(())
    }

    fn lookup_value_type(&self, name: &str) -> Option<FullType> {
        for scope in self.value_scopes.iter().rev() {
            if let Some(full_type) = scope.get(name) {
                return Some(full_type.clone());
            }
        }
        None
    }

    fn record_struct_definition(&mut self, sd: &StructDeclaration) -> ParseResult<()> {
        if sd.members.is_empty() {
            return Ok(());
        }
        let def = StructDef::from_declaration(sd, &self.struct_defs)
            .map_err(|err| self.format_error(&err))?;
        self.struct_defs.insert(sd.tag.clone(), def);
        Ok(())
    }

    fn offsetof_member_designator(&mut self, base_type: FullType) -> ParseResult<usize> {
        let mut current_type = base_type;
        let mut offset = 0usize;
        let first_member = self.parse_identifier()?;
        offset += self.offsetof_member_step(&mut current_type, &first_member)?;

        loop {
            if self.eat(&Token::Dot) {
                let member = self.parse_identifier()?;
                offset += self.offsetof_member_step(&mut current_type, &member)?;
            } else if self.eat(&Token::OpenBracket) {
                let index_exp = self.parse_expression()?;
                let index = self
                    .eval_integer_constant_exp_with_layout(&index_exp)
                    .ok_or_else(|| {
                        self.format_error(
                            "offsetof array index must be an integer constant expression",
                        )
                    })?;
                if index < 0 {
                    return Err(self.format_error("offsetof array index may not be negative"));
                }
                let FullType::Array { elem, .. } = current_type.clone() else {
                    return Err(self.format_error("offsetof array index requires an array member"));
                };
                let elem_size = elem.byte_size_with(&self.struct_defs);
                offset += index as usize * elem_size;
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
    ) -> ParseResult<usize> {
        let FullType::Struct(tag) = current_type else {
            return Err(self.format_error("offsetof member access requires a struct or union"));
        };
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
        Ok(mem.offset)
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

    fn eval_integer_constant_exp(exp: &Exp) -> Option<i64> {
        Self::eval_integer_constant_exp_with_defs(exp, &std::collections::HashMap::new())
    }

    fn eval_integer_constant_exp_with_layout(&self, exp: &Exp) -> Option<i64> {
        Self::eval_integer_constant_exp_with_defs(exp, &self.struct_defs)
    }

    fn eval_integer_constant_exp_with_defs(
        exp: &Exp,
        struct_defs: &std::collections::HashMap<String, StructDef>,
    ) -> Option<i64> {
        match exp {
            Exp::Constant(c)
            | Exp::LongConstant(c)
            | Exp::UIntConstant(c)
            | Exp::ULongConstant(c) => Some(*c),
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
                    BinaryOp::Add => Some(left + right),
                    BinaryOp::Sub => Some(left - right),
                    BinaryOp::Mul => Some(left * right),
                    BinaryOp::Div => (right != 0).then_some(left / right),
                    BinaryOp::Mod => (right != 0).then_some(left % right),
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
        let base_type = self.parse_type()?;
        let base_struct_tag = if base_type == CType::Struct {
            self.last_struct_tag.clone()
        } else {
            None
        };
        let typedef_full_type = self.last_typedef_full_type.take();
        let tree = self.parse_abstract_decl_tree()?;
        let full_type = if let Some(base_full_type) = typedef_full_type {
            Self::process_abstract_tree(&tree, base_full_type)
        } else {
            Self::process_abstract_tree(&tree, FullType::Scalar(base_type))
        };
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
                _ => break,
            }
        }
        Ok(attrs)
    }

    fn consume_alignment_specifiers(&mut self) -> ParseResult<Option<std::num::NonZeroUsize>> {
        Ok(self.consume_member_attributes()?.alignment)
    }

    fn consume_declaration_attributes(
        &mut self,
    ) -> ParseResult<(Option<std::num::NonZeroUsize>, bool)> {
        let mut alignment = None;
        let mut noreturn = false;
        loop {
            match self.peek().cloned() {
                Some(Token::KWAlignAs) => {
                    let value = self.parse_alignment_specifier()?;
                    alignment = Self::merge_alignment(alignment, value);
                }
                Some(Token::AttributeAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    alignment = Self::merge_alignment(alignment, value);
                }
                Some(Token::AttributeAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    alignment = Self::merge_alignment(alignment, value);
                    noreturn = true;
                }
                Some(Token::AttributeNoreturn) | Some(Token::KWNoreturn) => {
                    self.advance()?;
                    noreturn = true;
                }
                Some(Token::AttributePacked) => {
                    self.advance()?;
                }
                Some(Token::AttributePackedAligned(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    alignment = Self::merge_alignment(alignment, value);
                }
                Some(Token::AttributePackedAlignedNoreturn(expression)) => {
                    let value = self.parse_attribute_alignment(&expression)?;
                    self.advance()?;
                    alignment = Self::merge_alignment(alignment, value);
                    noreturn = true;
                }
                _ => break,
            }
        }
        Ok((alignment, noreturn))
    }

    fn consume_aggregate_attributes(&mut self) -> ParseResult<AggregateAttributes> {
        let mut attrs = AggregateAttributes::default();
        loop {
            match self.peek().cloned() {
                Some(Token::AttributePacked) => {
                    self.advance()?;
                    attrs.packed = true;
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
                _ => break,
            }
        }
        Ok(attrs)
    }

    fn parse_typeof_full_type(&mut self) -> ParseResult<FullType> {
        self.expect_token(Token::KWTypeOf)?;
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
            Exp::UIntConstant(_) => Ok(FullType::Scalar(CType::UInt)),
            Exp::ULongConstant(_) => Ok(FullType::Scalar(CType::ULong)),
            Exp::DoubleConstant(_) => Ok(FullType::Scalar(CType::Double)),
            Exp::StringLiteral(value) => Ok(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size: c_string_byte_len(value) + 1,
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
            Exp::Subscript(array, _) => match self.typeof_expression(array)? {
                FullType::Pointer(inner) | FullType::Array { elem: inner, .. } => Ok(*inner),
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
            return Ok(0);
        }
        if self.eat(&Token::Star) {
            return Ok(0);
        }
        let exp = self.parse_assignment()?;
        let value = self
            .eval_integer_constant_exp_with_layout(&exp)
            .ok_or_else(|| self.format_error("expected constant array size"))?;
        if value < 0 {
            return Err(self.format_error("array size must be non-negative"));
        }
        Ok(value as usize)
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
        let mut sc: Option<StorageClass> = None;
        let mut has_int = false;
        let mut has_long = false;
        let mut has_short = false;
        let mut has_char = false;
        let mut has_unsigned = false;
        let mut has_signed = false;
        let mut has_void = false;

        loop {
            match self.peek().cloned() {
                // Ignored qualifiers/specifiers
                Some(Token::KWConst)
                | Some(Token::KWVolatile)
                | Some(Token::KWRestrict)
                | Some(Token::KWInline) => {
                    self.advance()?;
                    continue;
                }
                Some(Token::KWNoreturn) | Some(Token::AttributeNoreturn) => {
                    self.pending_noreturn = true;
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
                Some(Token::KWAtomic) => {
                    self.advance()?;
                    if self.eat(&Token::OpenParen) {
                        let (_atomic_sc, atomic_type) = self.parse_specifiers()?;
                        self.expect_token(Token::CloseParen)?;
                        return Ok((sc, atomic_type));
                    }
                    continue;
                }
                Some(Token::KWTypeOf)
                    if !has_int
                        && !has_long
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
                Some(Token::KWInt) if !has_int && !has_void && !has_char => {
                    self.advance()?;
                    has_int = true;
                }
                Some(Token::KWLong) if !has_void && !has_char => {
                    self.advance()?;
                    has_long = true; // long long is the same as long (both 64-bit)
                }
                Some(Token::KWChar) if !has_char && !has_int && !has_long && !has_void => {
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
                    return Ok((sc, CType::Double));
                }
                Some(Token::KWFloat)
                    if !has_int
                        && !has_long
                        && !has_void
                        && !has_unsigned
                        && !has_signed
                        && !has_char =>
                {
                    self.advance()?;
                    return Ok((sc, CType::Float));
                }
                Some(Token::KWVoid)
                    if !has_int
                        && !has_long
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
            && !has_short
            && !has_void
            && !has_unsigned
            && !has_signed
            && !has_char
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
                // Optional body
                if self.at(&Token::OpenBrace) {
                    self.parse_enum_body()?;
                }
                return Ok((sc, CType::Int));
            }
            // Check for typedef name
            if let Some(Token::Identifier(name)) = self.peek() {
                if Self::is_builtin_float_type_name(name) {
                    self.advance()?;
                    self.last_typedef_full_type = None;
                    return Ok((sc, CType::Double));
                }
                if let Some(info) = self.lookup_typedef(name) {
                    let ct = info.base_type;
                    let tag = info.struct_tag.clone();
                    let ft = info.full_type.clone();
                    self.advance()?;
                    if let Some(tag) = tag {
                        self.last_struct_tag = Some(tag);
                    }
                    self.last_typedef_full_type = Some(ft);
                    return Ok((sc, ct));
                }
            }
            return Err(self.format_error("expected type specifier"));
        }
        self.last_typedef_full_type = None;

        let ctype = if has_void {
            CType::Void
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
        } else if has_long {
            CType::Long
        } else {
            CType::Int // 'signed', 'signed int', 'int' all map to Int
        };

        Ok((sc, ctype))
    }

    fn parse_type(&mut self) -> ParseResult<CType> {
        // Skip type qualifiers
        while matches!(
            self.peek(),
            Some(Token::KWConst)
                | Some(Token::KWVolatile)
                | Some(Token::KWRestrict)
                | Some(Token::KWThreadLocal)
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
        if self.at(&Token::KWTypeOf) {
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
            return Ok(CType::Double);
        }
        if self.at(&Token::KWFloat) {
            self.advance()?;
            return Ok(CType::Float);
        }
        if self.at(&Token::KWBool) {
            self.advance()?;
            return Ok(CType::Bool);
        }
        if self.at(&Token::KWEnum) {
            self.advance()?;
            if let Some(Token::Identifier(_)) = self.peek() {
                self.advance()?;
            }
            if self.at(&Token::OpenBrace) {
                self.parse_enum_body()?;
            }
            return Ok(CType::Int);
        }
        // Check for typedef name before parsing int/long/etc.
        if let Some(Token::Identifier(name)) = self.peek() {
            if Self::is_builtin_float_type_name(name) {
                self.advance()?;
                self.last_typedef_full_type = None;
                return Ok(CType::Double);
            }
            if let Some(info) = self.lookup_typedef(name) {
                let ct = info.base_type;
                let tag = info.struct_tag.clone();
                let ft = info.full_type.clone();
                self.advance()?;
                if let Some(tag) = tag {
                    self.last_struct_tag = Some(tag);
                }
                self.last_typedef_full_type = Some(ft);
                return Ok(ct);
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
        loop {
            match self.peek() {
                Some(Token::KWConst)
                | Some(Token::KWVolatile)
                | Some(Token::KWRestrict)
                | Some(Token::KWThreadLocal) => {
                    self.advance()?;
                    continue;
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
                Some(Token::KWLong) if !has_char => {
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
        let ctype = if has_char && has_unsigned {
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
        } else if has_double {
            CType::Double
        } else if has_long {
            CType::Long
        } else if has_int || has_signed {
            CType::Int
        } else {
            return Err(self.format_error("expected type specifier"));
        };
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
            | Token::KWAlignOf
            | Token::KWEnum
            | Token::KWConst
            | Token::KWVolatile
            | Token::KWAtomic
            | Token::KWThreadLocal
            | Token::KWStaticAssert
            | Token::KWRestrict
            | Token::KWBool
            | Token::KWShort
            | Token::KWTypeOf
            | Token::KWAutoType
            | Token::AttributePacked
            | Token::AttributePackedAligned(_)
            | Token::AttributePackedAlignedNoreturn(_)
            | Token::AttributeAlignedNoreturn(_)
            | Token::AttributeNoreturn
            | Token::KWNoreturn => true,
            Token::Identifier(name) => {
                self.is_typedef_name(name) || Self::is_builtin_float_type_name(name)
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
            Declarator::Function(params, pfts, variadic, inner) => {
                if let Declarator::Ident(name) = inner.as_ref() {
                    (
                        name.clone(),
                        base_ft,
                        Some(FunctionDeclaratorInfo {
                            params: params.clone(),
                            param_full_types: pfts.clone(),
                            variadic: *variadic,
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
            Declarator::Function(params, pfts, variadic, inner) => {
                if let Declarator::Ident(name) = inner.as_ref() {
                    // Function returning current_type: void *func() or int *func()
                    (
                        name.clone(),
                        current_type,
                        Some(FunctionDeclaratorInfo {
                            params: params.clone(),
                            param_full_types: pfts.clone(),
                            variadic: *variadic,
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
        // Count leading * (skip const/volatile/restrict after each star)
        let mut stars = 0;
        while self.eat(&Token::Star) {
            stars += 1;
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

        // Direct declarator: identifier, (declarator), or abstract (no name)
        let mut decl = if self.eat(&Token::OpenParen) {
            // Check if this is a grouped declarator like (*fp) or just (params)
            if self.at(&Token::Star) || matches!(self.peek(), Some(Token::Identifier(_))) {
                // Could be grouped declarator: (*name) or (name)
                // But only if NOT followed by a type keyword inside (which would indicate params)
                let save = self.pos;
                // Peek ahead: skip stars, check for identifier
                let mut temp_stars = 0;
                while self.eat(&Token::Star) {
                    temp_stars += 1;
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
            let (params, param_fts, variadic) = self.parse_param_list()?;
            self.expect_token(Token::CloseParen)?;
            decl = Declarator::Function(params, param_fts, variadic, Box::new(decl));
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
        let mut stars = 0;
        while self.eat(&Token::Star) {
            stars += 1;
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
            let (_, param_fts, variadic) = self.parse_param_list()?;
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

    fn parse_array_dims(&mut self) -> ParseResult<Option<Vec<usize>>> {
        let mut dims = Vec::new();
        while self.eat(&Token::OpenBracket) {
            dims.push(self.parse_array_size(true)?);
            self.expect_token(Token::CloseBracket)?;
        }
        if dims.is_empty() {
            Ok(None)
        } else {
            Ok(Some(dims))
        }
    }

    fn parse_array_init(&mut self) -> ParseResult<Exp> {
        self.expect_token(Token::OpenBrace)?;
        let mut elems = Vec::new();
        if !self.at(&Token::CloseBrace) {
            elems.push(self.parse_init_element()?);
            while self.eat(&Token::Comma) {
                if self.at(&Token::CloseBrace) {
                    break;
                } // trailing comma
                elems.push(self.parse_init_element()?);
            }
        }
        self.expect_token(Token::CloseBrace)?;
        Ok(Exp::ArrayInit(elems))
    }

    /// Parse one element of an initializer list.
    fn parse_init_element(&mut self) -> ParseResult<Exp> {
        let designators = self.parse_designators()?;
        let value = if self.at(&Token::OpenBrace) {
            self.parse_array_init()?
        } else {
            self.parse_assignment()?
        };
        if designators.is_empty() {
            Ok(value)
        } else {
            Ok(Exp::DesignatedInit(designators, Box::new(value)))
        }
    }

    /// Parse designator sequences like `.field =`, `[index] =`, or `.a.b[0] =`.
    fn parse_designators(&mut self) -> ParseResult<Vec<Designator>> {
        let mut designators = Vec::new();
        loop {
            if self.eat(&Token::Dot) {
                designators.push(Designator::Field(self.parse_identifier()?));
            } else if self.eat(&Token::OpenBracket) {
                let index = self.parse_expression()?;
                self.expect_token(Token::CloseBracket)?;
                designators.push(Designator::Index(Box::new(index)));
            } else {
                break;
            }
        }
        if !designators.is_empty() {
            self.expect_token(Token::Assign)?;
        }
        Ok(designators)
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
            Some(tok) => self.is_type_keyword(tok),
            None => false,
        }
    }

    /// Parse `struct tag` as a type specifier
    fn replace_scalar_struct(ft: &FullType, tag: &str) -> FullType {
        match ft {
            FullType::Scalar(CType::Struct) => FullType::Struct(tag.to_string()),
            FullType::Pointer(inner) => {
                FullType::Pointer(Box::new(Self::replace_scalar_struct(inner, tag)))
            }
            FullType::Function {
                return_type,
                params,
                variadic,
            } => FullType::Function {
                return_type: Box::new(Self::replace_scalar_struct(return_type, tag)),
                params: params
                    .iter()
                    .map(|p| Self::replace_scalar_struct(p, tag))
                    .collect(),
                variadic: *variadic,
            },
            FullType::Array { elem, size } => FullType::Array {
                elem: Box::new(Self::replace_scalar_struct(elem, tag)),
                size: *size,
            },
            other => other.clone(),
        }
    }

    fn is_flexible_array_member(ft: &FullType) -> bool {
        matches!(ft, FullType::Array { size: 0, .. })
    }

    fn validate_flexible_array_members(
        &self,
        members: &[MemberDeclaration],
        is_union: bool,
    ) -> ParseResult<()> {
        for (index, member) in members.iter().enumerate() {
            if !Self::is_flexible_array_member(&member.member_full_type) {
                continue;
            }
            if is_union {
                return Err(self.format_error("flexible array member not allowed in union"));
            }
            if member.name.is_empty() {
                return Err(self.format_error("flexible array member must be named"));
            }
            if member.bit_width.is_some() {
                return Err(self.format_error("flexible array member cannot be a bit-field"));
            }
            if index + 1 != members.len() {
                return Err(self.format_error("flexible array member must be last"));
            }
            if members.len() == 1 {
                return Err(self.format_error("flexible array member requires a previous member"));
            }
        }
        Ok(())
    }

    fn parse_struct_members(&mut self) -> ParseResult<Vec<MemberDeclaration>> {
        self.expect_token(Token::OpenBrace)?;
        let mut members = Vec::new();
        while !self.at(&Token::CloseBrace) {
            if self.peek().is_none() {
                return Err(self.format_error("expected CloseBrace but found end of input"));
            }
            let member_attrs = self.consume_member_attributes()?;
            let base_type = self.parse_type()?;
            let base_typedef_full_type = self.last_typedef_full_type.clone();
            loop {
                self.last_typedef_full_type = base_typedef_full_type.clone();
                let (name, full_type, _) =
                    if base_type == CType::Struct && self.at(&Token::Semicolon) {
                        let Some(tag) = self.last_struct_tag.clone() else {
                            return Err(self.format_error("anonymous aggregate member has no type"));
                        };
                        (String::new(), FullType::Struct(tag), None)
                    } else if self.at(&Token::Colon) {
                        (String::new(), FullType::Scalar(base_type), None)
                    } else {
                        self.parse_declarator_full(base_type)?
                    };
                let member_type = full_type.to_ctype();
                // Replace Scalar(Struct) with FullType::Struct(tag)
                let member_full_type = if base_type == CType::Struct {
                    if let Some(ref tag) = self.last_struct_tag {
                        Self::replace_scalar_struct(&full_type, tag)
                    } else {
                        full_type
                    }
                } else {
                    full_type
                };
                let bit_width = if self.eat(&Token::Colon) {
                    let width_exp = self.parse_assignment()?;
                    let width = self
                        .eval_integer_constant_exp_with_layout(&width_exp)
                        .ok_or_else(|| {
                            self.format_error("expected integer constant bit-field width")
                        })?;
                    if width < 0 {
                        return Err(self.format_error("bit-field width must be non-negative"));
                    }
                    let width = u8::try_from(width)
                        .map_err(|_| self.format_error("bit-field width is too large"))?;
                    Some(width)
                } else {
                    None
                };
                let post_attrs = self.consume_member_attributes()?;
                let member_alignment = match (member_attrs.alignment, post_attrs.alignment) {
                    (Some(current), Some(post)) => Some(current.max(post)),
                    (Some(current), None) => Some(current),
                    (None, Some(post)) => Some(post),
                    (None, None) => None,
                };
                members.push(MemberDeclaration {
                    name,
                    member_type,
                    member_full_type,
                    bit_width,
                    alignment: member_alignment,
                    packed: member_attrs.packed || post_attrs.packed,
                });
                if self.eat(&Token::Comma) {
                    continue;
                }
                self.expect_token(Token::Semicolon)?;
                break;
            }
            self.last_typedef_full_type = None;
        }
        self.expect_token(Token::CloseBrace)?;
        Ok(members)
    }

    fn parse_struct_type_specifier(&mut self) -> ParseResult<(CType, String)> {
        let is_union = self.at(&Token::KWUnion);
        if is_union {
            self.advance()?;
        } else {
            self.expect_token(Token::KWStruct)?;
        }
        let prefix_attrs = self.consume_aggregate_attributes()?;
        // Tag is optional for anonymous structs/unions
        let tag = if let Some(Token::Identifier(_)) = self.peek() {
            self.parse_identifier()?
        } else {
            // Anonymous struct/union — generate a unique tag
            let tag = format!("__anon_{}", self.pos);
            tag
        };
        // If followed by { members }, parse the struct body and record a pending definition
        if self.at(&Token::OpenBrace) {
            let members = self.parse_struct_members()?;
            let suffix_attrs = self.consume_aggregate_attributes()?;
            self.validate_flexible_array_members(&members, is_union)?;
            let attrs = Self::merge_aggregate_attributes(prefix_attrs, suffix_attrs);
            let declaration = StructDeclaration {
                tag: tag.clone(),
                members,
                is_union,
                packed: attrs.packed,
                alignment: attrs.alignment,
            };
            self.record_struct_definition(&declaration)?;
            self.pending_struct_decls.push(declaration);
        }
        self.last_struct_tag = Some(tag.clone());
        Ok((CType::Struct, tag))
    }

    fn parse_static_assert_declaration(&mut self) -> ParseResult<()> {
        self.expect_token(Token::KWStaticAssert)?;
        self.expect_token(Token::OpenParen)?;
        let condition = self.parse_expression()?;
        let value = self
            .eval_integer_constant_exp_with_layout(&condition)
            .unwrap_or(1);
        if self.eat(&Token::Comma) {
            match self.peek() {
                Some(Token::StringLiteral(_)) => {
                    self.advance()?;
                }
                _ => return Err(self.format_error("expected string literal in static assertion")),
            }
        }
        self.expect_token(Token::CloseParen)?;
        self.expect_token(Token::Semicolon)?;
        if value == 0 {
            return Err(self.format_error("static assertion failed"));
        }
        Ok(())
    }

    fn parse_declaration(&mut self) -> ParseResult<Declaration> {
        if self.eat(&Token::Semicolon) {
            return Ok(Declaration::TypedefDecl);
        }
        if self.at(&Token::KWStaticAssert) {
            self.parse_static_assert_declaration()?;
            return Ok(Declaration::TypedefDecl);
        }
        // Check for standalone enum declaration: enum Tag { ... };
        if self.at(&Token::KWEnum) {
            let save_pos = self.pos;
            self.advance()?; // consume 'enum'
                             // Optional tag
            if let Some(Token::Identifier(_)) = self.peek() {
                self.advance()?;
            }
            if self.at(&Token::OpenBrace) {
                self.pos = save_pos;
                // Parse through parse_specifiers which handles enum body
                let (_sc, _base_type) = self.parse_specifiers()?;
                self.last_typedef_full_type = None;
                if self.at(&Token::Semicolon) {
                    self.advance()?;
                    return Ok(Declaration::TypedefDecl); // no-op, constants already registered
                }
                // Not a standalone enum — has a variable name after. Put back and re-parse.
                self.pos = save_pos;
            } else {
                self.pos = save_pos;
            }
        }
        // Check for standalone struct/union declaration: struct/union tag { members };
        if self.at(&Token::KWStruct) || self.at(&Token::KWUnion) {
            let is_union = self.at(&Token::KWUnion);
            let save_pos = self.pos;
            self.advance()?; // consume 'struct' or 'union'
            let prefix_attrs = self.consume_aggregate_attributes()?;
            if let Some(Token::Identifier(_)) = self.peek() {
                let tag = self.parse_identifier()?;
                if self.at(&Token::OpenBrace) {
                    let members = self.parse_struct_members()?;
                    let suffix_attrs = self.consume_aggregate_attributes()?;
                    self.validate_flexible_array_members(&members, is_union)?;
                    self.expect_token(Token::Semicolon)?;
                    let attrs = Self::merge_aggregate_attributes(prefix_attrs, suffix_attrs);
                    let declaration = StructDeclaration {
                        tag,
                        members,
                        is_union,
                        packed: attrs.packed,
                        alignment: attrs.alignment,
                    };
                    self.record_struct_definition(&declaration)?;
                    return Ok(Declaration::StructDecl(declaration));
                } else if self.at(&Token::Semicolon) {
                    self.advance()?;
                    return Ok(Declaration::StructDecl(StructDeclaration {
                        tag,
                        members: vec![],
                        is_union,
                        packed: prefix_attrs.packed,
                        alignment: prefix_attrs.alignment,
                    }));
                }
            }
            // Not a standalone decl — put back and let parse_specifiers handle it
            self.pos = save_pos;
        }
        let (sc, base_type) = self.parse_specifiers()?;
        let is_auto_type = std::mem::take(&mut self.pending_auto_type);
        let spec_noreturn = std::mem::take(&mut self.pending_noreturn);
        let decl_alignment = self.pending_alignment.take();
        // Save struct tag before declarator parsing (params may overwrite last_struct_tag)
        let saved_struct_tag = if base_type == CType::Struct {
            self.last_struct_tag.clone()
        } else {
            None
        };

        let decl_tree = self.parse_declarator_tree()?;
        let (post_alignment, post_noreturn) = self.consume_declaration_attributes()?;
        let first_alignment = match (decl_alignment, post_alignment) {
            (Some(current), Some(post)) => Some(current.max(post)),
            (Some(current), None) => Some(current),
            (None, Some(post)) => Some(post),
            (None, None) => None,
        };
        let first_noreturn = spec_noreturn || post_noreturn;
        let td_ft = self.last_typedef_full_type.take();
        let (name, full_type, decl_params) =
            Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());

        // Replace Scalar(Struct) with FullType::Struct(tag) if applicable
        let full_type = if base_type == CType::Struct {
            if let Some(ref tag) = saved_struct_tag {
                Self::replace_scalar_struct(&full_type, tag)
            } else {
                full_type
            }
        } else {
            full_type
        };

        if is_auto_type {
            if sc.as_ref().is_some_and(StorageClass::is_typedef)
                || decl_params.is_some()
                || full_type != FullType::Scalar(CType::Void)
            {
                return Err(self.format_error("__auto_type requires a single object declarator"));
            }
            let decl = self.make_auto_var_decl(name, sc, first_alignment)?;
            self.expect_token(Token::Semicolon)?;
            self.add_value_type(decl.name.clone(), self.var_decl_full_type(&decl)?)?;
            return Ok(Declaration::VarDecl(decl));
        }

        // Handle typedef declarations
        if sc.as_ref().is_some_and(StorageClass::is_typedef) {
            self.add_typedef(
                name,
                TypedefInfo {
                    base_type,
                    full_type: full_type.clone(),
                    struct_tag: saved_struct_tag.clone(),
                },
            )?;
            while self.eat(&Token::Comma) {
                self.last_typedef_full_type = td_ft.clone();
                let decl_tree = self.parse_declarator_tree()?;
                let (name2, full_type2, _) =
                    Self::process_declarator(&decl_tree, base_type, td_ft.as_ref());
                let full_type2 = if base_type == CType::Struct {
                    if let Some(ref tag) = saved_struct_tag {
                        Self::replace_scalar_struct(&full_type2, tag)
                    } else {
                        full_type2
                    }
                } else {
                    full_type2
                };
                self.add_typedef(
                    name2,
                    TypedefInfo {
                        base_type,
                        full_type: full_type2,
                        struct_tag: saved_struct_tag.clone(),
                    },
                )?;
            }
            self.expect_token(Token::Semicolon)?;
            return Ok(Declaration::TypedefDecl);
        }

        // Derive scalar metadata from the canonical FullType.
        let ctype = full_type.to_ctype();
        let pi = match &full_type {
            FullType::Pointer(inner) => {
                let (base, depth) = ptr_info_from_full(inner);
                Some((base, depth))
            }
            _ => None,
        };
        // Is it a function?
        if let Some(func_info) = decl_params {
            self.add_value_type(
                name.clone(),
                Self::function_full_type(full_type.clone(), &func_info),
            )?;
            let param_value_types =
                Self::param_value_types(&func_info.params, &func_info.param_full_types);
            let body = if self.at(&Token::OpenBrace) {
                Some(self.parse_block_with_values(&param_value_types)?)
            } else {
                self.expect_token(Token::Semicolon)?;
                None
            };
            return Ok(Declaration::FunDecl(FunctionDeclaration {
                name,
                return_type: ctype,
                return_ptr_info: pi,
                return_full_type: Some(full_type.clone()),
                params: func_info.params,
                body,
                storage_class: sc,
                param_full_types: func_info.param_full_types,
                variadic: func_info.variadic,
                noreturn: first_noreturn,
            }));
        }

        // Check for function (in case declarator didn't catch params)
        if self.at(&Token::OpenParen) {
            self.expect_token(Token::OpenParen)?;
            let (params, param_fts, variadic) = self.parse_param_list()?;
            self.expect_token(Token::CloseParen)?;
            let func_info = FunctionDeclaratorInfo {
                params,
                param_full_types: param_fts,
                variadic,
            };
            self.add_value_type(
                name.clone(),
                Self::function_full_type(full_type.clone(), &func_info),
            )?;
            let param_value_types =
                Self::param_value_types(&func_info.params, &func_info.param_full_types);
            let body = if self.at(&Token::OpenBrace) {
                Some(self.parse_block_with_values(&param_value_types)?)
            } else {
                self.expect_token(Token::Semicolon)?;
                None
            };
            return Ok(Declaration::FunDecl(FunctionDeclaration {
                name,
                return_type: ctype,
                return_ptr_info: pi,
                return_full_type: Some(full_type.clone()),
                params: func_info.params,
                body,
                storage_class: sc,
                param_full_types: func_info.param_full_types,
                variadic: func_info.variadic,
                noreturn: first_noreturn,
            }));
        }

        let first = self.make_var_decl(name, &full_type, ctype, pi, sc.clone(), first_alignment)?;
        self.add_value_type(first.name.clone(), full_type.clone())?;
        // Check for multiple declarators
        if self.eat(&Token::Comma) {
            let mut extra = Vec::new();
            loop {
                let (name2, full_type2, _) = self.parse_declarator_full(base_type)?;
                let post_alignment = self.consume_alignment_specifiers()?;
                let declarator_alignment = match (decl_alignment, post_alignment) {
                    (Some(current), Some(post)) => Some(current.max(post)),
                    (Some(current), None) => Some(current),
                    (None, Some(post)) => Some(post),
                    (None, None) => None,
                };
                let full_type2 = if base_type == CType::Struct {
                    if let Some(ref tag) = saved_struct_tag {
                        Self::replace_scalar_struct(&full_type2, tag)
                    } else {
                        full_type2
                    }
                } else {
                    full_type2
                };
                let ctype2 = full_type2.to_ctype();
                let pi2 = match &full_type2 {
                    FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                    _ => None,
                };
                let decl = self.make_var_decl(
                    name2,
                    &full_type2,
                    ctype2,
                    pi2,
                    sc.clone(),
                    declarator_alignment,
                )?;
                self.add_value_type(decl.name.clone(), full_type2.clone())?;
                extra.push(Declaration::VarDecl(decl));
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
            self.expect_token(Token::Semicolon)?;
            self.pending_declarations.extend(extra);
        } else {
            self.expect_token(Token::Semicolon)?;
        }
        Ok(Declaration::VarDecl(first))
    }

    fn parse_var_declaration(&mut self) -> ParseResult<VarDeclaration> {
        let (sc, base_type) = self.parse_specifiers()?;
        let is_auto_type = std::mem::take(&mut self.pending_auto_type);
        let _spec_noreturn = std::mem::take(&mut self.pending_noreturn);
        let decl_alignment = self.pending_alignment.take();
        let (name, full_type, _) = self.parse_declarator_full(base_type)?;
        let post_alignment = self.consume_alignment_specifiers()?;
        let decl_alignment = match (decl_alignment, post_alignment) {
            (Some(current), Some(post)) => Some(current.max(post)),
            (Some(current), None) => Some(current),
            (None, Some(post)) => Some(post),
            (None, None) => None,
        };
        let full_type = if base_type == CType::Struct {
            if let Some(ref tag) = self.last_struct_tag {
                Self::replace_scalar_struct(&full_type, tag)
            } else {
                full_type
            }
        } else {
            full_type
        };
        if is_auto_type {
            if full_type != FullType::Scalar(CType::Void) {
                return Err(self.format_error("__auto_type requires a single object declarator"));
            }
            let decl = self.make_auto_var_decl(name, sc, decl_alignment)?;
            self.expect_token(Token::Semicolon)?;
            self.add_value_type(decl.name.clone(), self.var_decl_full_type(&decl)?)?;
            return Ok(decl);
        }
        let ctype = full_type.to_ctype();
        let pi = match &full_type {
            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
            _ => None,
        };
        let init = if self.eat(&Token::Assign) {
            if self.at(&Token::OpenBrace) {
                Some(self.parse_array_init()?)
            } else {
                Some(self.parse_expression()?)
            }
        } else {
            None
        };
        let full_type = self.infer_unsized_array_type(full_type, init.as_ref())?;
        let array_dims = Self::extract_array_dims(&full_type);
        if ctype == CType::Void && array_dims.is_none() {
            return Err(self.format_error("cannot declare variable with void type"));
        }
        self.expect_token(Token::Semicolon)?;
        self.add_value_type(name.clone(), full_type.clone())?;
        Ok(VarDeclaration {
            name,
            var_type: if array_dims.is_some() {
                let mut t = &full_type;
                while let FullType::Array { elem, .. } = t {
                    t = elem;
                }
                t.to_ctype()
            } else {
                ctype
            },
            ptr_info: pi,
            array_dims,
            decl_full_type: Some(full_type.clone()),
            init,
            storage_class: sc,
            alignment: decl_alignment,
        })
    }

    fn parse_param_list(&mut self) -> ParseResult<(Vec<ParamDecl>, Vec<FullType>, bool)> {
        // "void" or empty or "int x, long y, ..."
        if self.at(&Token::KWVoid)
            && self.pos + 1 < self.tokens.len()
            && self.tokens[self.pos + 1] == Token::CloseParen
        {
            self.advance()?;
            return Ok((Vec::new(), Vec::new(), false));
        }
        if self.at(&Token::CloseParen) {
            return Ok((Vec::new(), Vec::new(), false));
        }
        let mut params = Vec::new();
        let mut param_fts = Vec::new();
        let parse_one_param = |s: &mut Self, fts: &mut Vec<FullType>| -> ParseResult<ParamDecl> {
            let base = s.parse_type()?;
            // Use abstract declarator parsing (name optional) for params
            let tree = s.parse_declarator_tree_inner(true)?;
            let td_ft = s.last_typedef_full_type.take();
            let (name, full_type, _) = Self::process_declarator(&tree, base, td_ft.as_ref());
            // Generate a dummy name for unnamed params
            let name = if name.is_empty() {
                format!("__unnamed_{}", s.pos)
            } else {
                name
            };
            // Replace Scalar(Struct) with FullType::Struct(tag)
            let full_type = if base == CType::Struct {
                if let Some(ref tag) = s.last_struct_tag {
                    Self::replace_scalar_struct(&full_type, tag)
                } else {
                    full_type
                }
            } else {
                full_type
            };
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
            Ok((name, t, pi))
        };
        params.push(parse_one_param(self, &mut param_fts)?);
        let mut variadic = false;
        while self.eat(&Token::Comma) {
            // Check for ... (variadic)
            if self.eat(&Token::Ellipsis) {
                variadic = true;
                break;
            }
            params.push(parse_one_param(self, &mut param_fts)?);
        }
        Ok((params, param_fts, variadic))
    }

    fn parse_identifier(&mut self) -> ParseResult<String> {
        match self.advance()? {
            Token::Identifier(name) => Ok(name),
            other => {
                self.pos -= 1;
                Err(self.format_error(&format!("expected identifier, found {:?}", other)))
            }
        }
    }

    // --------------------------------------------------------
    // Blocks and block items
    // --------------------------------------------------------

    fn parse_block(&mut self) -> ParseResult<Block> {
        self.parse_block_with_values(&[])
    }

    fn parse_scoped_block_items(&mut self) -> ParseResult<Block> {
        let mut items = Vec::new();
        while !self.at(&Token::CloseBrace) {
            if self.peek().is_none() {
                return Err(self.format_error("expected CloseBrace but found end of input"));
            }
            let item = self.parse_block_item()?;
            // Emit any pending struct/union definitions from type specifier parsing
            for sd in self.pending_struct_decls.drain(..) {
                items.push(BlockItem::Declaration(Declaration::StructDecl(sd)));
            }
            items.push(item);
            // Emit extra items from multi-declarator parsing
            items.append(&mut self.pending_block_items);
        }
        Ok(items)
    }

    fn parse_block_with_values(
        &mut self,
        initial_values: &[(String, FullType)],
    ) -> ParseResult<Block> {
        self.expect_token(Token::OpenBrace)?;
        self.push_typedef_scope();
        for (name, full_type) in initial_values {
            self.add_value_type(name.clone(), full_type.clone())?;
        }
        let items = match self.parse_scoped_block_items() {
            Ok(items) => items,
            Err(err) => {
                self.pop_typedef_scope();
                return Err(err);
            }
        };
        self.expect_token(Token::CloseBrace)?;
        self.pop_typedef_scope();
        Ok(items)
    }

    fn parse_statement_expression(&mut self) -> ParseResult<Exp> {
        self.expect_token(Token::OpenParen)?;
        self.expect_token(Token::OpenBrace)?;
        self.push_typedef_scope();
        let mut block = match self.parse_scoped_block_items() {
            Ok(items) => items,
            Err(err) => {
                self.pop_typedef_scope();
                return Err(err);
            }
        };
        self.expect_token(Token::CloseBrace)?;
        let (result, result_type) = match block.pop() {
            Some(BlockItem::Statement(Statement::Expression(exp))) => {
                let full_type = self.typeof_expression(&exp)?;
                (Some(Box::new(exp)), Some(full_type))
            }
            Some(item) => {
                block.push(item);
                (None, None)
            }
            None => (None, None),
        };
        self.pop_typedef_scope();
        self.expect_token(Token::CloseParen)?;
        Ok(Exp::StatementExpr(block, result, result_type))
    }

    fn is_declaration_start(&self) -> bool {
        match self.peek() {
            Some(Token::KWInt)
            | Some(Token::KWLong)
            | Some(Token::KWUnsigned)
            | Some(Token::KWSigned)
            | Some(Token::KWDouble)
            | Some(Token::KWFloat)
            | Some(Token::KWVoid)
            | Some(Token::KWChar)
            | Some(Token::KWStruct)
            | Some(Token::KWUnion)
            | Some(Token::KWAlignOf)
            | Some(Token::KWEnum)
            | Some(Token::KWStatic)
            | Some(Token::KWExtern)
            | Some(Token::KWTypedef)
            | Some(Token::KWConst)
            | Some(Token::KWVolatile)
            | Some(Token::KWAtomic)
            | Some(Token::KWThreadLocal)
            | Some(Token::KWStaticAssert)
            | Some(Token::KWInline)
            | Some(Token::KWRegister)
            | Some(Token::KWRestrict)
            | Some(Token::KWBool)
            | Some(Token::KWShort)
            | Some(Token::KWTypeOf)
            | Some(Token::KWAutoType)
            | Some(Token::AttributeAligned(_))
            | Some(Token::AttributeAlignedNoreturn(_))
            | Some(Token::AttributePacked)
            | Some(Token::AttributePackedAligned(_))
            | Some(Token::AttributePackedAlignedNoreturn(_))
            | Some(Token::AttributeNoreturn)
            | Some(Token::KWNoreturn) => true,
            Some(Token::Identifier(name)) => self.is_typedef_name(name),
            _ => false,
        }
    }

    fn parse_block_item(&mut self) -> ParseResult<BlockItem> {
        if self.is_declaration_start() {
            if self.at(&Token::KWStaticAssert) {
                self.parse_static_assert_declaration()?;
                return Ok(BlockItem::Declaration(Declaration::TypedefDecl));
            }
            // Check for standalone enum declaration: enum Tag { ... };
            if self.at(&Token::KWEnum) {
                let save_pos = self.pos;
                self.advance()?;
                if let Some(Token::Identifier(_)) = self.peek() {
                    self.advance()?;
                }
                if self.at(&Token::OpenBrace) {
                    self.pos = save_pos;
                    let (_sc, _base_type) = self.parse_specifiers()?;
                    self.last_typedef_full_type = None;
                    if self.at(&Token::Semicolon) {
                        self.advance()?;
                        return Ok(BlockItem::Declaration(Declaration::TypedefDecl));
                    }
                    self.pos = save_pos;
                } else {
                    self.pos = save_pos;
                }
            }
            // Check for standalone struct/union declaration: struct tag { ... };
            if self.at(&Token::KWStruct) || self.at(&Token::KWUnion) {
                let is_union = self.at(&Token::KWUnion);
                let save_pos = self.pos;
                self.advance()?; // consume 'struct' or 'union'
                                 // Only check for standalone decl if next token is an identifier (not anonymous)
                let prefix_attrs = self.consume_aggregate_attributes()?;
                if let Some(Token::Identifier(_)) = self.peek() {
                    let tag = self.parse_identifier()?;
                    if self.at(&Token::OpenBrace) {
                        let members = self.parse_struct_members()?;
                        let suffix_attrs = self.consume_aggregate_attributes()?;
                        self.validate_flexible_array_members(&members, is_union)?;
                        self.expect_token(Token::Semicolon)?;
                        let attrs = Self::merge_aggregate_attributes(prefix_attrs, suffix_attrs);
                        return Ok(BlockItem::Declaration(Declaration::StructDecl(
                            StructDeclaration {
                                tag,
                                members,
                                is_union,
                                packed: attrs.packed,
                                alignment: attrs.alignment,
                            },
                        )));
                    } else if self.at(&Token::Semicolon) {
                        self.advance()?;
                        return Ok(BlockItem::Declaration(Declaration::StructDecl(
                            StructDeclaration {
                                tag,
                                members: vec![],
                                is_union,
                                packed: prefix_attrs.packed,
                                alignment: prefix_attrs.alignment,
                            },
                        )));
                    }
                }
                // Not a standalone decl — put back and let parse_specifiers handle it
                self.pos = save_pos;
            }
            let (sc, base_type) = self.parse_specifiers()?;
            let is_auto_type = std::mem::take(&mut self.pending_auto_type);
            let spec_noreturn = std::mem::take(&mut self.pending_noreturn);
            let decl_alignment = self.pending_alignment.take();
            let saved_struct_tag = if base_type == CType::Struct {
                self.last_struct_tag.clone()
            } else {
                None
            };
            let (name, full_type, decl_params) = self.parse_declarator_full(base_type)?;
            let (post_alignment, post_noreturn) = self.consume_declaration_attributes()?;
            let decl_alignment = match (decl_alignment, post_alignment) {
                (Some(current), Some(post)) => Some(current.max(post)),
                (Some(current), None) => Some(current),
                (None, Some(post)) => Some(post),
                (None, None) => None,
            };
            let decl_noreturn = spec_noreturn || post_noreturn;
            // Replace Scalar(Struct) with FullType::Struct(tag)
            let full_type = if base_type == CType::Struct {
                if let Some(ref tag) = saved_struct_tag {
                    Self::replace_scalar_struct(&full_type, tag)
                } else {
                    full_type
                }
            } else {
                full_type
            };

            if is_auto_type {
                if sc.as_ref().is_some_and(StorageClass::is_typedef)
                    || decl_params.is_some()
                    || full_type != FullType::Scalar(CType::Void)
                {
                    return Err(
                        self.format_error("__auto_type requires a single object declarator")
                    );
                }
                let decl = self.make_auto_var_decl(name, sc, decl_alignment)?;
                self.expect_token(Token::Semicolon)?;
                self.add_value_type(decl.name.clone(), self.var_decl_full_type(&decl)?)?;
                return Ok(BlockItem::Declaration(Declaration::VarDecl(decl)));
            }

            // Handle typedef declarations
            if sc.as_ref().is_some_and(StorageClass::is_typedef) {
                self.expect_token(Token::Semicolon)?;
                self.add_typedef(
                    name,
                    TypedefInfo {
                        base_type,
                        full_type: full_type.clone(),
                        struct_tag: saved_struct_tag,
                    },
                )?;
                return Ok(BlockItem::Declaration(Declaration::TypedefDecl));
            }

            let ctype = full_type.to_ctype();
            let pi = match &full_type {
                FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                _ => None,
            };

            if decl_params.is_some() || self.at(&Token::OpenParen) {
                let func_info = if let Some(info) = decl_params {
                    info
                } else {
                    self.expect_token(Token::OpenParen)?;
                    let (params, param_full_types, variadic) = self.parse_param_list()?;
                    self.expect_token(Token::CloseParen)?;
                    FunctionDeclaratorInfo {
                        params,
                        param_full_types,
                        variadic,
                    }
                };

                let body = if self.at(&Token::OpenBrace) {
                    Some(self.parse_block()?)
                } else {
                    self.expect_token(Token::Semicolon)?;
                    None
                };

                if body.is_some() {
                    return Err(self.format_error("function definitions not allowed inside blocks"));
                }

                self.add_value_type(
                    name.clone(),
                    Self::function_full_type(full_type.clone(), &func_info),
                )?;

                Ok(BlockItem::Declaration(Declaration::FunDecl(
                    FunctionDeclaration {
                        name,
                        return_type: ctype,
                        return_ptr_info: pi,
                        return_full_type: Some(full_type.clone()),
                        params: func_info.params,
                        body,
                        storage_class: sc,
                        param_full_types: func_info.param_full_types,
                        variadic: func_info.variadic,
                        noreturn: decl_noreturn,
                    },
                )))
            } else {
                let first =
                    self.make_var_decl(name, &full_type, ctype, pi, sc.clone(), decl_alignment)?;
                self.add_value_type(first.name.clone(), full_type.clone())?;
                // Check for multiple declarators: int x = 1, y, *z;
                if self.eat(&Token::Comma) {
                    let mut extra = Vec::new();
                    loop {
                        let (name2, full_type2, _) = self.parse_declarator_full(base_type)?;
                        let full_type2 = if base_type == CType::Struct {
                            if let Some(ref tag) = saved_struct_tag {
                                Self::replace_scalar_struct(&full_type2, tag)
                            } else {
                                full_type2
                            }
                        } else {
                            full_type2
                        };
                        let ctype2 = full_type2.to_ctype();
                        let pi2 = match &full_type2 {
                            FullType::Pointer(inner) => Some(ptr_info_from_full(inner)),
                            _ => None,
                        };
                        let decl = self.make_var_decl(
                            name2,
                            &full_type2,
                            ctype2,
                            pi2,
                            sc.clone(),
                            decl_alignment,
                        )?;
                        self.add_value_type(decl.name.clone(), full_type2.clone())?;
                        extra.push(BlockItem::Declaration(Declaration::VarDecl(decl)));
                        if !self.eat(&Token::Comma) {
                            break;
                        }
                    }
                    self.expect_token(Token::Semicolon)?;
                    // Stash extras for parse_block to collect
                    self.pending_block_items.extend(extra);
                } else {
                    self.expect_token(Token::Semicolon)?;
                }
                Ok(BlockItem::Declaration(Declaration::VarDecl(first)))
            }
        } else {
            Ok(BlockItem::Statement(self.parse_statement()?))
        }
    }

    // --------------------------------------------------------
    // Statements
    // --------------------------------------------------------

    fn parse_statement(&mut self) -> ParseResult<Statement> {
        match self.peek() {
            Some(Token::KWReturn) => {
                self.advance()?;
                if self.at(&Token::Semicolon) {
                    self.advance()?;
                    Ok(Statement::Return(None))
                } else {
                    let exp = self.parse_expression()?;
                    self.expect_token(Token::Semicolon)?;
                    Ok(Statement::Return(Some(exp)))
                }
            }
            Some(Token::KWIf) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let condition = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                let then_stmt = Box::new(self.parse_statement()?);
                let else_stmt = if self.eat(&Token::KWElse) {
                    Some(Box::new(self.parse_statement()?))
                } else {
                    None
                };
                Ok(Statement::If(condition, then_stmt, else_stmt))
            }
            Some(Token::KWWhile) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let condition = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::While {
                    condition,
                    body,
                    label: String::new(), // filled by resolve pass
                })
            }
            Some(Token::KWDo) => {
                self.advance()?;
                let body = Box::new(self.parse_statement()?);
                self.expect_token(Token::KWWhile)?;
                self.expect_token(Token::OpenParen)?;
                let condition = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::DoWhile {
                    body,
                    condition,
                    label: String::new(),
                })
            }
            Some(Token::KWFor) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let init = if self.is_declaration_start() {
                    ForInit::Declaration(self.parse_var_declaration()?)
                } else if self.eat(&Token::Semicolon) {
                    ForInit::Expression(None)
                } else {
                    let exp = self.parse_expression()?;
                    self.expect_token(Token::Semicolon)?;
                    ForInit::Expression(Some(exp))
                };
                let condition = if self.at(&Token::Semicolon) {
                    None
                } else {
                    Some(self.parse_expression()?)
                };
                self.expect_token(Token::Semicolon)?;
                let post = if self.at(&Token::CloseParen) {
                    None
                } else {
                    Some(self.parse_expression()?)
                };
                self.expect_token(Token::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::For {
                    init: Box::new(init),
                    condition,
                    post,
                    body,
                    label: String::new(),
                })
            }
            Some(Token::KWBreak) => {
                self.advance()?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Break(String::new())) // filled by resolve pass
            }
            Some(Token::KWContinue) => {
                self.advance()?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Continue(String::new()))
            }
            Some(Token::KWGoto) => {
                self.advance()?;
                let label = self.parse_identifier()?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Goto(label))
            }
            Some(Token::KWSwitch) => {
                self.advance()?;
                self.expect_token(Token::OpenParen)?;
                let control = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Switch {
                    control,
                    body,
                    label: String::new(),
                    cases: Vec::new(),
                })
            }
            Some(Token::KWCase) => {
                self.advance()?;
                let value = self.parse_expression()?;
                self.expect_token(Token::Colon)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Case {
                    value,
                    body,
                    label: String::new(),
                })
            }
            Some(Token::KWDefault) => {
                self.advance()?;
                self.expect_token(Token::Colon)?;
                let body = Box::new(self.parse_statement()?);
                Ok(Statement::Default {
                    body,
                    label: String::new(),
                })
            }
            Some(Token::OpenBrace) => Ok(Statement::Block(self.parse_block()?)),
            Some(Token::Semicolon) => {
                self.advance()?;
                Ok(Statement::Null)
            }
            // Check for labeled statement: identifier ':'
            Some(Token::Identifier(_))
                if self.pos + 1 < self.tokens.len()
                    && self.tokens[self.pos + 1] == Token::Colon =>
            {
                let name = self.parse_identifier()?;
                self.expect_token(Token::Colon)?;
                let stmt = Box::new(self.parse_statement()?);
                Ok(Statement::Label(name, stmt))
            }
            _ => {
                let exp = self.parse_expression()?;
                self.expect_token(Token::Semicolon)?;
                Ok(Statement::Expression(exp))
            }
        }
    }

    // --------------------------------------------------------
    // Expressions (precedence climbing)
    // --------------------------------------------------------

    fn parse_expression(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_assignment()?;
        while self.eat(&Token::Comma) {
            let right = self.parse_assignment()?;
            left = Exp::Comma(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_assignment(&mut self) -> ParseResult<Exp> {
        let left = self.parse_conditional()?;

        match self.peek().cloned() {
            Some(Token::Assign) => {
                self.advance()?;
                let right = self.parse_assignment()?; // right-associative
                Ok(Exp::Assign(Box::new(left), Box::new(right)))
            }
            Some(tok) => {
                if let Some(op) = Self::compound_assign_op(&tok) {
                    self.advance()?;
                    let right = self.parse_assignment()?;
                    Ok(Exp::CompoundAssign(op, Box::new(left), Box::new(right)))
                } else {
                    Ok(left)
                }
            }
            None => Ok(left),
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

    fn parse_conditional(&mut self) -> ParseResult<Exp> {
        let cond = self.parse_logical_or()?;
        if self.eat(&Token::Question) {
            let then_exp = self.parse_expression()?;
            self.expect_token(Token::Colon)?;
            let else_exp = self.parse_conditional()?; // right-associative
            Ok(Exp::Conditional(
                Box::new(cond),
                Box::new(then_exp),
                Box::new(else_exp),
            ))
        } else {
            Ok(cond)
        }
    }

    fn parse_logical_or(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_logical_and()?;
        while self.eat(&Token::LogicalOr) {
            let right = self.parse_logical_and()?;
            left = Exp::Binary(BinaryOp::LogicalOr, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_logical_and(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_bitwise_or()?;
        while self.eat(&Token::LogicalAnd) {
            let right = self.parse_bitwise_or()?;
            left = Exp::Binary(BinaryOp::LogicalAnd, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_or(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_bitwise_xor()?;
        while self.eat(&Token::Pipe) {
            let right = self.parse_bitwise_xor()?;
            left = Exp::Binary(BinaryOp::BitwiseOr, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_xor(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_bitwise_and()?;
        while self.eat(&Token::Caret) {
            let right = self.parse_bitwise_and()?;
            left = Exp::Binary(BinaryOp::BitwiseXor, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_bitwise_and(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_equality()?;
        while self.eat(&Token::Ampersand) {
            let right = self.parse_equality()?;
            left = Exp::Binary(BinaryOp::BitwiseAnd, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_relational()?;
        loop {
            match self.peek().cloned() {
                Some(Token::EqualEqual) => {
                    self.advance()?;
                    let right = self.parse_relational()?;
                    left = Exp::Binary(BinaryOp::Equal, Box::new(left), Box::new(right));
                }
                Some(Token::NotEqual) => {
                    self.advance()?;
                    let right = self.parse_relational()?;
                    left = Exp::Binary(BinaryOp::NotEqual, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_relational(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_shift()?;
        loop {
            match self.peek().cloned() {
                Some(Token::LessThan) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::LessThan, Box::new(left), Box::new(right));
                }
                Some(Token::GreaterThan) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::GreaterThan, Box::new(left), Box::new(right));
                }
                Some(Token::LessEqual) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::LessEqual, Box::new(left), Box::new(right));
                }
                Some(Token::GreaterEqual) => {
                    self.advance()?;
                    let right = self.parse_shift()?;
                    left = Exp::Binary(BinaryOp::GreaterEqual, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_additive()?;
        loop {
            match self.peek().cloned() {
                Some(Token::ShiftLeft) => {
                    self.advance()?;
                    let right = self.parse_additive()?;
                    left = Exp::Binary(BinaryOp::ShiftLeft, Box::new(left), Box::new(right));
                }
                Some(Token::ShiftRight) => {
                    self.advance()?;
                    let right = self.parse_additive()?;
                    left = Exp::Binary(BinaryOp::ShiftRight, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_multiplicative()?;
        loop {
            match self.peek().cloned() {
                Some(Token::Plus) => {
                    self.advance()?;
                    let right = self.parse_multiplicative()?;
                    left = Exp::Binary(BinaryOp::Add, Box::new(left), Box::new(right));
                }
                Some(Token::Minus) => {
                    self.advance()?;
                    let right = self.parse_multiplicative()?;
                    left = Exp::Binary(BinaryOp::Sub, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> ParseResult<Exp> {
        let mut left = self.parse_unary()?;
        loop {
            match self.peek().cloned() {
                Some(Token::Star) => {
                    self.advance()?;
                    let right = self.parse_unary()?;
                    left = Exp::Binary(BinaryOp::Mul, Box::new(left), Box::new(right));
                }
                Some(Token::Slash) => {
                    self.advance()?;
                    let right = self.parse_unary()?;
                    left = Exp::Binary(BinaryOp::Div, Box::new(left), Box::new(right));
                }
                Some(Token::Percent) => {
                    self.advance()?;
                    let right = self.parse_unary()?;
                    left = Exp::Binary(BinaryOp::Mod, Box::new(left), Box::new(right));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> ParseResult<Exp> {
        match self.peek().cloned() {
            Some(Token::Minus) => {
                self.advance()?;
                Ok(Exp::Unary(UnaryOp::Negate, Box::new(self.parse_unary()?)))
            }
            Some(Token::Tilde) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::Complement,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Bang) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::LogicalNot,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Increment) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::PreIncrement,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Decrement) => {
                self.advance()?;
                Ok(Exp::Unary(
                    UnaryOp::PreDecrement,
                    Box::new(self.parse_unary()?),
                ))
            }
            Some(Token::Star) => {
                self.advance()?;
                Ok(Exp::Unary(UnaryOp::Deref, Box::new(self.parse_unary()?)))
            }
            Some(Token::Ampersand) => {
                self.advance()?;
                Ok(Exp::Unary(UnaryOp::AddrOf, Box::new(self.parse_unary()?)))
            }
            // sizeof expression or sizeof(type)
            Some(Token::KWSizeOf) => {
                self.advance()?;
                // sizeof(type) — check for ( followed by type keyword
                if self.at(&Token::OpenParen)
                    && self.pos + 1 < self.tokens.len()
                    && self.is_type_keyword(&self.tokens[self.pos + 1])
                {
                    self.advance()?; // consume '('
                    let base_type = self.parse_type()?;
                    let full_type = self.parse_abstract_declarator_type(base_type)?;
                    let full_type = if base_type == CType::Struct {
                        if let Some(ref tag) = self.last_struct_tag {
                            Self::replace_scalar_struct(&full_type, tag)
                        } else {
                            full_type
                        }
                    } else {
                        full_type
                    };
                    self.expect_token(Token::CloseParen)?;
                    let ctype = full_type.to_ctype();
                    Ok(Exp::SizeOfType(ctype, full_type))
                } else {
                    // sizeof <unary-exp> (not a cast expression)
                    let operand = self.parse_unary()?;
                    Ok(Exp::SizeOf(Box::new(operand)))
                }
            }
            Some(Token::KWAlignOf) => {
                self.advance()?;
                if self.at(&Token::OpenParen)
                    && self.pos + 1 < self.tokens.len()
                    && self.is_type_keyword(&self.tokens[self.pos + 1])
                {
                    self.advance()?;
                    let base_type = self.parse_type()?;
                    let full_type = self.parse_abstract_declarator_type(base_type)?;
                    let full_type = if base_type == CType::Struct {
                        if let Some(ref tag) = self.last_struct_tag {
                            Self::replace_scalar_struct(&full_type, tag)
                        } else {
                            full_type
                        }
                    } else {
                        full_type
                    };
                    self.expect_token(Token::CloseParen)?;
                    Ok(Exp::AlignOfType(full_type))
                } else {
                    let operand = self.parse_unary()?;
                    let full_type = self.typeof_expression(&operand)?;
                    Ok(Exp::AlignOfType(full_type))
                }
            }
            // Cast expression or compound literal: (type) unary or (type){init}
            Some(Token::OpenParen)
                if self.pos + 1 < self.tokens.len()
                    && self.is_type_keyword(&self.tokens[self.pos + 1]) =>
            {
                self.advance()?; // consume '('
                let base_type = self.parse_type()?;
                let full_type = self.parse_abstract_declarator_type(base_type)?;
                let full_type = if base_type == CType::Struct {
                    if let Some(ref tag) = self.last_struct_tag {
                        Self::replace_scalar_struct(&full_type, tag)
                    } else {
                        full_type
                    }
                } else {
                    full_type
                };
                self.expect_token(Token::CloseParen)?;
                if self.at(&Token::OpenBrace) {
                    // Compound literal: (Type){init}
                    let init = self.parse_array_init()?;
                    // Treat as a cast of the initializer to the target type
                    let target_type = full_type.to_ctype();
                    let cast_ft = if target_type == CType::Pointer || target_type == CType::Struct {
                        Some(full_type)
                    } else {
                        None
                    };
                    Ok(Exp::Cast(target_type, cast_ft, Box::new(init)))
                } else {
                    let target_type = full_type.to_ctype();
                    let operand = self.parse_unary()?;
                    let cast_ft = if target_type == CType::Pointer || target_type == CType::Struct {
                        Some(full_type)
                    } else {
                        None
                    };
                    Ok(Exp::Cast(target_type, cast_ft, Box::new(operand)))
                }
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> ParseResult<Exp> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().cloned() {
                Some(Token::Increment) => {
                    self.advance()?;
                    expr = Exp::Unary(UnaryOp::PostIncrement, Box::new(expr));
                }
                Some(Token::Decrement) => {
                    self.advance()?;
                    expr = Exp::Unary(UnaryOp::PostDecrement, Box::new(expr));
                }
                Some(Token::OpenBracket) => {
                    self.advance()?;
                    let index = self.parse_expression()?;
                    self.expect_token(Token::CloseBracket)?;
                    expr = Exp::Subscript(Box::new(expr), Box::new(index));
                }
                Some(Token::Dot) => {
                    self.advance()?;
                    let member = self.parse_identifier()?;
                    expr = Exp::Dot(Box::new(expr), member);
                }
                Some(Token::Arrow) => {
                    self.advance()?;
                    let member = self.parse_identifier()?;
                    expr = Exp::Arrow(Box::new(expr), member);
                }
                Some(Token::OpenParen)
                    if !matches!(expr, Exp::Var(_) | Exp::FunctionCall(_, _)) =>
                {
                    // Indirect call through expression: expr(args)
                    // e.g., ops[0](1,2) or get_func()(args)
                    self.advance()?;
                    let args = self.parse_arg_list()?;
                    self.expect_token(Token::CloseParen)?;
                    expr = Exp::IndirectCall(Box::new(expr), args);
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn generic_types_match(control: &FullType, association: &FullType) -> bool {
        let control = Self::generic_decay_type(control);
        let association = Self::generic_decay_type(association);
        control == association
            || matches!(
                (&control, &association),
                (FullType::Scalar(left), FullType::Scalar(right)) if left == right
            )
    }

    fn generic_decay_type(full_type: &FullType) -> FullType {
        match full_type {
            FullType::Function { .. } => FullType::Pointer(Box::new(full_type.clone())),
            _ => full_type.decay(),
        }
    }

    fn parse_generic_selection(&mut self) -> ParseResult<Exp> {
        self.expect_token(Token::KWGeneric)?;
        self.expect_token(Token::OpenParen)?;
        let control = self.parse_assignment()?;
        let control_type = self.typeof_expression(&control)?;
        self.expect_token(Token::Comma)?;

        let mut selected = None;
        let mut default = None;
        loop {
            if self.eat(&Token::KWDefault) {
                if default.is_some() {
                    return Err(self.format_error("duplicate default generic association"));
                }
                self.expect_token(Token::Colon)?;
                let expr = self.parse_assignment()?;
                default = Some(expr);
            } else {
                let assoc_type = self.parse_type_name_full()?;
                self.expect_token(Token::Colon)?;
                let expr = self.parse_assignment()?;
                if selected.is_none() && Self::generic_types_match(&control_type, &assoc_type) {
                    selected = Some(expr);
                }
            }

            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect_token(Token::CloseParen)?;
        selected
            .or(default)
            .ok_or_else(|| self.format_error("no matching generic association"))
    }

    fn parse_primary(&mut self) -> ParseResult<Exp> {
        match self.peek().cloned() {
            Some(Token::KWGeneric) => self.parse_generic_selection(),
            Some(Token::IntLiteral(val)) => {
                self.advance()?;
                if val >= i32::MIN as i64 && val <= i32::MAX as i64 {
                    Ok(Exp::Constant(val))
                } else {
                    Ok(Exp::LongConstant(val))
                }
            }
            Some(Token::LongLiteral(val)) => {
                self.advance()?;
                Ok(Exp::LongConstant(val))
            }
            Some(Token::UIntLiteral(val)) => {
                self.advance()?;
                // UInt constants > UINT_MAX are promoted to ulong
                if val > u32::MAX as i64 {
                    Ok(Exp::ULongConstant(val))
                } else {
                    Ok(Exp::UIntConstant(val))
                }
            }
            Some(Token::ULongLiteral(val)) => {
                self.advance()?;
                Ok(Exp::ULongConstant(val))
            }
            Some(Token::DoubleLiteral(val)) => {
                self.advance()?;
                Ok(Exp::DoubleConstant(val))
            }
            Some(Token::CharLiteral(val)) => {
                self.advance()?;
                Ok(Exp::Constant(val)) // char constants have type int
            }
            Some(Token::StringLiteral(_)) => {
                // Concatenate adjacent string literals
                let mut s = String::new();
                while let Some(Token::StringLiteral(part)) = self.peek().cloned() {
                    self.advance()?;
                    s.push_str(&part);
                }
                Ok(Exp::StringLiteral(s))
            }
            Some(Token::Identifier(name)) => {
                self.advance()?;
                // Check for function call
                if self.eat(&Token::OpenParen) {
                    if name == "__builtin_types_compatible_p" {
                        let left = self.parse_type_name_full()?;
                        self.expect_token(Token::Comma)?;
                        let right = self.parse_type_name_full()?;
                        self.expect_token(Token::CloseParen)?;
                        return Ok(Exp::Constant(
                            Self::gnu_types_compatible(&left, &right) as i64
                        ));
                    }
                    if name == "__builtin_offsetof" {
                        let base_type = self.parse_type()?;
                        let full_type = self.parse_abstract_declarator_type(base_type)?;
                        let full_type = if base_type == CType::Struct {
                            if let Some(ref tag) = self.last_struct_tag {
                                Self::replace_scalar_struct(&full_type, tag)
                            } else {
                                full_type
                            }
                        } else {
                            full_type
                        };
                        self.expect_token(Token::Comma)?;
                        let offset = self.offsetof_member_designator(full_type)?;
                        self.expect_token(Token::CloseParen)?;
                        return Ok(Exp::ULongConstant(offset as i64));
                    }
                    let args = self.parse_arg_list()?;
                    self.expect_token(Token::CloseParen)?;
                    if name == "__builtin_expect" || name == "__builtin_expect_with_probability" {
                        return args.into_iter().next().ok_or_else(|| {
                            self.format_error(&format!("{} requires an argument", name))
                        });
                    }
                    if name == "__builtin_constant_p" {
                        let Some(arg) = args.first() else {
                            return Err(
                                self.format_error("__builtin_constant_p requires an argument")
                            );
                        };
                        let is_constant = self.eval_integer_constant_exp_with_layout(arg).is_some()
                            || matches!(arg, Exp::DoubleConstant(_) | Exp::StringLiteral(_));
                        return Ok(Exp::Constant(is_constant as i64));
                    }
                    if name == "__builtin_strlen" {
                        let Some(arg) = args.first() else {
                            return Err(self.format_error("__builtin_strlen requires an argument"));
                        };
                        if let Exp::StringLiteral(s) = arg {
                            return Ok(Exp::ULongConstant(c_string_byte_len(s) as i64));
                        }
                    }
                    if let Some(fallback) = Self::fortified_builtin_fallback(&name, &args) {
                        return fallback.map_err(|err| self.format_error(&err));
                    }
                    if matches!(
                        name.as_str(),
                        "__builtin_object_size" | "__builtin_dynamic_object_size"
                    ) {
                        if args.len() < 2 {
                            return Err(
                                self.format_error(&format!("{} requires two arguments", name))
                            );
                        }
                        let mode = self
                            .eval_integer_constant_exp_with_layout(&args[1])
                            .unwrap_or(0);
                        return Ok(if mode >= 2 {
                            Exp::ULongConstant(0)
                        } else {
                            Exp::ULongConstant(-1)
                        });
                    }
                    if name == "__builtin_assume_aligned" {
                        return args.into_iter().next().ok_or_else(|| {
                            self.format_error("__builtin_assume_aligned requires an argument")
                        });
                    }
                    if name == "__builtin_prefetch" {
                        return Ok(Exp::Constant(0));
                    }
                    if matches!(
                        name.as_str(),
                        "__atomic_thread_fence" | "__atomic_signal_fence" | "__sync_synchronize"
                    ) {
                        return Ok(Exp::AtomicFence);
                    }
                    if name == "__builtin_bswap32" {
                        let Some(arg) = args.first() else {
                            return Err(self.format_error("__builtin_bswap32 requires an argument"));
                        };
                        return Ok(Self::bswap_exp(arg.clone(), 32));
                    }
                    if name == "__builtin_bswap64" {
                        let Some(arg) = args.first() else {
                            return Err(self.format_error("__builtin_bswap64 requires an argument"));
                        };
                        return Ok(Self::bswap_exp(arg.clone(), 64));
                    }
                    if name == "__atomic_load_n" && args.len() >= 2 {
                        return Ok(Self::ordered_atomic_builtin_exp(Exp::Unary(
                            UnaryOp::Deref,
                            Box::new(args[0].clone()),
                        )));
                    }
                    if name == "__atomic_store_n" && args.len() >= 3 {
                        return Ok(Self::ordered_atomic_builtin_exp(Exp::Assign(
                            Box::new(Exp::Unary(UnaryOp::Deref, Box::new(args[0].clone()))),
                            Box::new(args[1].clone()),
                        )));
                    }
                    if name == "__atomic_exchange_n" && args.len() >= 3 {
                        return Ok(Exp::AtomicExchange {
                            ptr: Box::new(args[0].clone()),
                            value: Box::new(args[1].clone()),
                        });
                    }
                    if name == "__atomic_compare_exchange_n" && args.len() >= 6 {
                        return Ok(Exp::AtomicCompareExchange {
                            ptr: Box::new(args[0].clone()),
                            expected: Box::new(args[1].clone()),
                            desired: Box::new(args[2].clone()),
                        });
                    }
                    if matches!(
                        name.as_str(),
                        "__sync_bool_compare_and_swap" | "__sync_val_compare_and_swap"
                    ) && args.len() >= 3
                    {
                        return Ok(Exp::AtomicCompareSwap {
                            ptr: Box::new(args[0].clone()),
                            expected: Box::new(args[1].clone()),
                            desired: Box::new(args[2].clone()),
                            return_old: name == "__sync_val_compare_and_swap",
                        });
                    }
                    if let Some(op) = Self::atomic_fetch_op(&name) {
                        let min_args = if name.starts_with("__sync_") { 2 } else { 3 };
                        if args.len() >= min_args {
                            return Ok(Exp::AtomicFetch {
                                op,
                                ptr: Box::new(args[0].clone()),
                                arg: Box::new(args[1].clone()),
                                return_old: Self::atomic_fetch_returns_old(&name),
                            });
                        }
                    }
                    if name == "__builtin_choose_expr" && args.len() == 3 {
                        let condition = self
                            .eval_integer_constant_exp_with_layout(&args[0])
                            .ok_or_else(|| {
                                self.format_error(
                                    "__builtin_choose_expr requires an integer constant condition",
                                )
                            })?;
                        if condition != 0 {
                            return Ok(args[1].clone());
                        }
                        return Ok(args[2].clone());
                    }
                    if matches!(name.as_str(), "__builtin_unreachable" | "__builtin_trap") {
                        return Ok(Exp::Unreachable);
                    }
                    Ok(Exp::FunctionCall(name, args))
                } else if let Some(val) = self.lookup_enum_constant(&name) {
                    // Enum constant — resolve to integer literal
                    Ok(Exp::Constant(val))
                } else {
                    Ok(Exp::Var(name))
                }
            }
            Some(Token::OpenParen) => {
                if self.pos + 1 < self.tokens.len() && self.tokens[self.pos + 1] == Token::OpenBrace
                {
                    return self.parse_statement_expression();
                }
                self.advance()?;
                let exp = self.parse_expression()?;
                self.expect_token(Token::CloseParen)?;
                Ok(exp)
            }
            other => Err(self.format_error(&format!("expected expression, found {:?}", other))),
        }
    }

    fn parse_arg_list(&mut self) -> ParseResult<Vec<Exp>> {
        if self.at(&Token::CloseParen) {
            return Ok(Vec::new());
        }
        let mut args = vec![self.parse_assignment()?];
        while self.eat(&Token::Comma) {
            args.push(self.parse_assignment()?);
        }
        Ok(args)
    }
}

pub fn parse(tokens: Vec<Token>) -> ParseResult<Program> {
    let mut parser = Parser::new(tokens);
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
    let plain_tokens: Vec<Token> = tokens.iter().map(|spanned| spanned.token.clone()).collect();
    parse(plain_tokens).map_err(|message| {
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
    fn parse_reports_nonconstant_array_size() -> Result<(), String> {
        let err = require_err(parse_source_err("int x; int a[x];\n"), "parse should fail")?;
        assert!(err.contains("expected constant array size"), "{err}");
        assert!(err.contains("Parse error at token"), "{err}");
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
    fn parse_array_dims_reports_missing_close_bracket() -> Result<(), String> {
        let mut parser = parser_source("[2")?;
        let err = require_err(parser.parse_array_dims(), "array dimension should fail")?;
        assert!(err.contains("expected CloseBracket"), "{err}");
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
    fn parse_abstract_declarator_reports_nonconstant_array_size() -> Result<(), String> {
        let mut parser = parser_source("[x]")?;
        let err = require_err(
            parser.parse_abstract_declarator_type(CType::Int),
            "abstract declarator should fail",
        )?;
        assert!(err.contains("expected constant array size"), "{err}");
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
    fn parse_block_item_reports_nested_function_definition() -> Result<(), String> {
        let mut parser = parser_source("int f() { return 1; }")?;
        let err = require_err(parser.parse_block_item(), "block item should fail")?;
        assert!(
            err.contains("function definitions not allowed inside blocks"),
            "{err}"
        );
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
    fn parse_struct_members_reports_missing_member_semicolon() -> Result<(), String> {
        let mut parser = parser_source("{ int x }")?;
        let err = require_err(parser.parse_struct_members(), "struct members should fail")?;
        assert!(err.contains("expected Semicolon"), "{err}");
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
    fn parses_long_double_parameters_as_double() -> Result<(), String> {
        let program = parse_source("extern long double f(long double x);\n")?;
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
    fn parses_builtin_float_type_names_as_double() -> Result<(), String> {
        let program = parse_source("extern _Float16 f(_Float64 x);\n")?;
        let Declaration::FunDecl(func) = &program.declarations[0] else {
            return Err("expected function declaration".to_string());
        };
        assert_eq!(func.return_type, CType::Double);
        assert_eq!(func.params[0].1, CType::Double);
        Ok(())
    }

    #[test]
    fn parses_builtin_int128_typedef_names_as_opaque_two_word_arrays() -> Result<(), String> {
        let program = parse_source("__uint128_t vector[4];\n__int128_t scalar;\n")?;
        let Declaration::VarDecl(vector) = &program.declarations[0] else {
            return Err("expected vector declaration".to_string());
        };
        let Declaration::VarDecl(scalar) = &program.declarations[1] else {
            return Err("expected scalar declaration".to_string());
        };

        assert_eq!(
            vector.decl_full_type,
            Some(FullType::Array {
                elem: Box::new(FullType::Array {
                    elem: Box::new(FullType::Scalar(CType::ULong)),
                    size: 2,
                }),
                size: 4,
            })
        );
        assert_eq!(
            scalar.decl_full_type,
            Some(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::ULong)),
                size: 2,
            })
        );
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
        let program =
            parse_source("char s[] = \"abc\"; int a[] = { 1, 2, 3 }; int b[] = { [4] = 1 };\n")?;
        let Declaration::VarDecl(s) = &program.declarations[0] else {
            return Err("expected string array declaration".to_string());
        };
        let Declaration::VarDecl(a) = &program.declarations[1] else {
            return Err("expected array declaration".to_string());
        };
        let Declaration::VarDecl(b) = &program.declarations[2] else {
            return Err("expected designated array declaration".to_string());
        };
        assert_eq!(s.array_dims, Some(vec![4]));
        assert_eq!(a.array_dims, Some(vec![3]));
        assert_eq!(b.array_dims, Some(vec![5]));
        Ok(())
    }

    #[test]
    fn parses_trailing_flexible_array_member() -> Result<(), String> {
        let program = parse_source("struct packet { int len; char data[]; };\n")?;
        let Declaration::StructDecl(decl) = &program.declarations[0] else {
            return Err("expected struct declaration".to_string());
        };
        assert_eq!(decl.members.len(), 2);
        assert!(matches!(
            decl.members[1].member_full_type,
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
    fn generic_selection_reports_missing_match() -> Result<(), String> {
        let mut parser = parser_source("_Generic(1, long: 2)")?;
        let err = require_err(parser.parse_expression(), "generic selection should fail")?;
        assert!(err.contains("no matching generic association"), "{err}");
        Ok(())
    }
}
