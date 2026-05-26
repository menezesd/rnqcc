#![allow(dead_code)]

use crate::types::*;
use std::collections::HashMap;

type FileScopeVarInfo = (bool, bool, Option<(i64, bool, bool)>, CType);
type BuiltinFunctionInfo = (
    &'static str,
    CType,
    FullType,
    Vec<CType>,
    Option<(CType, usize)>,
);
pub type TackyResult<T> = Result<T, String>;

struct StaticInitBuilder {
    pieces: Vec<(usize, StaticInit)>,
}

impl StaticInitBuilder {
    fn new() -> Self {
        Self { pieces: Vec::new() }
    }

    fn put(&mut self, offset: usize, init: StaticInit) -> TackyResult<()> {
        let size = TackyGen::static_init_size(&init);
        if size == 0 {
            return Ok(());
        }
        let end = offset + size;
        if let Some(pos) = self.pieces.iter().position(|(existing_offset, existing)| {
            *existing_offset == offset && TackyGen::static_init_size(existing) == size
        }) {
            self.pieces[pos] = (offset, init);
            return Ok(());
        }
        if self.pieces.iter().any(|(existing_offset, existing)| {
            let existing_end = *existing_offset + TackyGen::static_init_size(existing);
            offset < existing_end && *existing_offset < end
        }) {
            return Err("overlapping static initializer designators".to_string());
        }
        self.pieces.push((offset, init));
        Ok(())
    }

    fn finish(mut self, total_bytes: usize) -> TackyResult<Vec<StaticInit>> {
        self.pieces.sort_by_key(|(offset, _)| *offset);
        let mut out = Vec::new();
        let mut cursor = 0usize;
        for (offset, init) in self.pieces {
            if offset > total_bytes {
                break;
            }
            if offset > cursor {
                out.push(StaticInit::ZeroInit(offset - cursor));
                cursor = offset;
            }
            let size = TackyGen::static_init_size(&init);
            if cursor + size > total_bytes {
                return Err("static initializer exceeds object size".to_string());
            }
            out.push(init);
            cursor += size;
        }
        if cursor < total_bytes {
            out.push(StaticInit::ZeroInit(total_bytes - cursor));
        }
        Ok(out)
    }
}

struct TackyGen {
    tmp_counter: usize,
    label_counter: usize,
    string_counter: usize,
    instructions: Vec<TackyInstr>,
    current_function: String,
    /// Hidden return pointer name for functions returning large structs
    hidden_ret_ptr: Option<String>,
    static_vars: Vec<TackyStaticVar>,
    static_constants: Vec<TackyStaticConstant>,
    extern_vars: Vec<String>,
    /// CType for each variable/temporary (for codegen output)
    symbol_types: HashMap<String, CType>,
    /// Rich type info (tracks arrays, pointer targets)
    full_types: HashMap<String, FullType>,
    /// Function types: (return_type, param_types, return_ptr_info)
    func_types: HashMap<String, FunctionTypeInfo>,
    /// Function return full types
    func_full_types: HashMap<String, FullType>,
    /// Function parameter full types.
    func_param_full_types: HashMap<String, Vec<FullType>>,
    /// Scalar type cache for variables and temporaries.
    var_types: HashMap<String, CType>,
    symbol_alignments: HashMap<String, usize>,
    ptr_info: HashMap<String, (CType, usize)>,
    /// Array storage sizes for stack allocation
    array_sizes: HashMap<String, usize>,
    /// Struct definitions
    struct_defs: HashMap<String, StructDef>,
}

impl TackyGen {
    fn new() -> Self {
        TackyGen {
            tmp_counter: 0,
            label_counter: 0,
            string_counter: 0,
            instructions: Vec::new(),
            current_function: String::new(),
            hidden_ret_ptr: None,
            static_vars: Vec::new(),
            static_constants: Vec::new(),
            extern_vars: Vec::new(),
            symbol_types: HashMap::new(),
            full_types: HashMap::new(),
            func_types: HashMap::new(),
            func_full_types: HashMap::new(),
            func_param_full_types: HashMap::new(),
            var_types: HashMap::new(),
            symbol_alignments: HashMap::new(),
            ptr_info: HashMap::new(),
            array_sizes: HashMap::new(),
            struct_defs: HashMap::new(),
        }
    }

    fn fresh_var_name(&mut self) -> String {
        let name = format!("tmp.{}", self.tmp_counter);
        self.tmp_counter += 1;
        name
    }

    fn zero_init_local(&mut self, name: &str, total_bytes: usize) {
        let mut off = 0usize;
        while off + 8 <= total_bytes {
            let z = self.fresh_tmp(CType::Long);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: z.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: z,
                dst_name: name.to_string(),
                offset: off as i64,
            });
            off += 8;
        }
        while off + 4 <= total_bytes {
            let z = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: z.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: z,
                dst_name: name.to_string(),
                offset: off as i64,
            });
            off += 4;
        }
        while off < total_bytes {
            let z = self.fresh_tmp(CType::Char);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: z.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: z,
                dst_name: name.to_string(),
                offset: off as i64,
            });
            off += 1;
        }
    }

    fn fresh_tmp(&mut self, t: CType) -> TackyVal {
        let name = format!("tmp.{}", self.tmp_counter);
        self.tmp_counter += 1;
        self.symbol_types.insert(name.clone(), t);
        TackyVal::Var(name)
    }

    fn fresh_tmp_full(&mut self, ft: &FullType) -> TackyVal {
        let name = format!("tmp.{}", self.tmp_counter);
        self.tmp_counter += 1;
        let ct = ft.to_ctype();
        self.symbol_types.insert(name.clone(), ct);
        self.var_types.insert(name.clone(), ct);
        self.full_types.insert(name.clone(), ft.clone());
        // Keep pointer-depth metadata in sync with the canonical FullType.
        if let FullType::Pointer(ref inner) = ft {
            let (base, depth) = Self::ptr_info_from_full(inner);
            self.ptr_info.insert(name.clone(), (base, depth));
        }
        TackyVal::Var(name)
    }

    /// Register a variable with its full type
    fn register_var(&mut self, name: &str, ft: FullType) {
        let ct = ft.to_ctype();
        self.symbol_types.insert(name.to_string(), ct);
        self.var_types.insert(name.to_string(), ct);
        self.full_types.insert(name.to_string(), ft.clone());

        // Keep pointer-depth metadata in sync with the canonical FullType.
        if let FullType::Pointer(ref inner) = ft {
            let (base, depth) = Self::ptr_info_from_full(inner);
            self.ptr_info.insert(name.to_string(), (base, depth));
        }

        // Track array/struct sizes
        if ft.is_array() {
            self.array_sizes
                .insert(name.to_string(), ft.byte_size_with(&self.struct_defs));
        }
        if let FullType::Struct(ref tag) = ft {
            if let Some(def) = self.struct_defs.get(tag) {
                self.array_sizes.insert(name.to_string(), def.size);
            }
        }
    }

    fn ptr_info_from_full(ft: &FullType) -> (CType, usize) {
        match ft {
            FullType::Scalar(t) => (*t, 1),
            FullType::Pointer(inner) => {
                let (base, depth) = Self::ptr_info_from_full(inner);
                (base, depth + 1)
            }
            FullType::Function { return_type, .. } => Self::ptr_info_from_full(return_type),
            FullType::Array { elem, .. } => {
                // Pointer to array: base is element's scalar type
                let base_ct = elem.to_ctype();
                (base_ct, 1)
            }
            FullType::Struct(_) => (CType::Struct, 1),
        }
    }

    fn function_signature_from_full(ft: &FullType) -> Option<(FullType, Vec<FullType>, bool)> {
        match ft {
            FullType::Function {
                return_type,
                params,
                variadic,
            } => Some((return_type.as_ref().clone(), params.clone(), *variadic)),
            FullType::Pointer(inner) => Self::function_signature_from_full(inner),
            _ => None,
        }
    }

    fn void_pointer_type() -> FullType {
        FullType::Pointer(Box::new(FullType::Scalar(CType::Void)))
    }

    fn char_pointer_type() -> FullType {
        FullType::Pointer(Box::new(FullType::Scalar(CType::Char)))
    }

    fn builtin_function_info(name: &str) -> Option<BuiltinFunctionInfo> {
        let void_ptr = Self::void_pointer_type();
        let char_ptr = Self::char_pointer_type();
        match name {
            "__builtin_memcpy" => Some((
                "memcpy",
                CType::Pointer,
                void_ptr,
                vec![CType::Pointer, CType::Pointer, CType::ULong],
                Some((CType::Void, 1)),
            )),
            "__builtin_memmove" => Some((
                "memmove",
                CType::Pointer,
                void_ptr,
                vec![CType::Pointer, CType::Pointer, CType::ULong],
                Some((CType::Void, 1)),
            )),
            "__builtin_memset" => Some((
                "memset",
                CType::Pointer,
                void_ptr,
                vec![CType::Pointer, CType::Int, CType::ULong],
                Some((CType::Void, 1)),
            )),
            "__builtin_memcmp" => Some((
                "memcmp",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer, CType::Pointer, CType::ULong],
                None,
            )),
            "__builtin_memchr" => Some((
                "memchr",
                CType::Pointer,
                void_ptr,
                vec![CType::Pointer, CType::Int, CType::ULong],
                Some((CType::Void, 1)),
            )),
            "__builtin_strlen" => Some((
                "strlen",
                CType::ULong,
                FullType::Scalar(CType::ULong),
                vec![CType::Pointer],
                None,
            )),
            "__builtin_strcmp" => Some((
                "strcmp",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer, CType::Pointer],
                None,
            )),
            "__builtin_strncmp" => Some((
                "strncmp",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer, CType::Pointer, CType::ULong],
                None,
            )),
            "__builtin_strchr" => Some((
                "strchr",
                CType::Pointer,
                char_ptr.clone(),
                vec![CType::Pointer, CType::Int],
                Some((CType::Char, 1)),
            )),
            "__builtin_strrchr" => Some((
                "strrchr",
                CType::Pointer,
                char_ptr.clone(),
                vec![CType::Pointer, CType::Int],
                Some((CType::Char, 1)),
            )),
            "__builtin_strstr" => Some((
                "strstr",
                CType::Pointer,
                char_ptr.clone(),
                vec![CType::Pointer, CType::Pointer],
                Some((CType::Char, 1)),
            )),
            "__builtin_strspn" => Some((
                "strspn",
                CType::ULong,
                FullType::Scalar(CType::ULong),
                vec![CType::Pointer, CType::Pointer],
                None,
            )),
            "__builtin_strcspn" => Some((
                "strcspn",
                CType::ULong,
                FullType::Scalar(CType::ULong),
                vec![CType::Pointer, CType::Pointer],
                None,
            )),
            "__builtin_strcpy" => Some((
                "strcpy",
                CType::Pointer,
                char_ptr.clone(),
                vec![CType::Pointer, CType::Pointer],
                Some((CType::Char, 1)),
            )),
            "__builtin_strncpy" => Some((
                "strncpy",
                CType::Pointer,
                char_ptr.clone(),
                vec![CType::Pointer, CType::Pointer, CType::ULong],
                Some((CType::Char, 1)),
            )),
            "__builtin_strcat" => Some((
                "strcat",
                CType::Pointer,
                char_ptr.clone(),
                vec![CType::Pointer, CType::Pointer],
                Some((CType::Char, 1)),
            )),
            "__builtin_strncat" => Some((
                "strncat",
                CType::Pointer,
                char_ptr,
                vec![CType::Pointer, CType::Pointer, CType::ULong],
                Some((CType::Char, 1)),
            )),
            _ => None,
        }
    }

    fn is_void_pointer(ft: &FullType) -> bool {
        matches!(
            ft,
            FullType::Pointer(inner) if matches!(inner.as_ref(), FullType::Scalar(CType::Void))
        )
    }

    fn compatible_full_types(&self, dst: &FullType, src: &FullType) -> bool {
        if dst == src {
            return true;
        }
        match (dst, src) {
            (FullType::Scalar(CType::Bool), FullType::Pointer(_))
            | (FullType::Scalar(CType::Bool), FullType::Scalar(CType::Pointer)) => true,
            (FullType::Scalar(a), FullType::Scalar(b)) => {
                *a != CType::Struct && *b != CType::Struct && *a != CType::Void && *b != CType::Void
            }
            (FullType::Pointer(_), FullType::Scalar(CType::Pointer))
            | (FullType::Scalar(CType::Pointer), FullType::Pointer(_)) => true,
            (FullType::Pointer(_), FullType::Pointer(_)) => {
                Self::is_void_pointer(dst) || Self::is_void_pointer(src) || dst == src
            }
            (FullType::Pointer(pointee), FullType::Array { elem, .. }) => {
                Self::is_void_pointer(dst) || self.compatible_full_types(pointee, elem)
            }
            (FullType::Struct(a), FullType::Struct(b)) => a == b,
            _ => false,
        }
    }

    fn assert_assignable_full_type(
        &self,
        dst: &FullType,
        src: &FullType,
        context: &str,
    ) -> TackyResult<()> {
        if !self.compatible_full_types(dst, src) {
            return Err(format!(
                "incompatible types in {}: cannot convert {:?} to {:?}",
                context, src, dst
            ));
        }
        Ok(())
    }

    fn is_null_pointer_constant(exp: &Exp) -> bool {
        matches!(
            exp,
            Exp::Constant(0) | Exp::LongConstant(0) | Exp::UIntConstant(0) | Exp::ULongConstant(0)
        ) || matches!(exp, Exp::Cast(_, _, inner) if Self::is_null_pointer_constant(inner))
    }

    fn assert_assignable_exp_full_type(
        &self,
        dst: &FullType,
        src: &FullType,
        src_exp: &Exp,
        context: &str,
    ) -> TackyResult<()> {
        if matches!(dst, FullType::Pointer(_)) && Self::is_null_pointer_constant(src_exp) {
            return Ok(());
        }
        if let (FullType::Struct(dst_tag), FullType::Pointer(src_inner)) = (dst, src) {
            if matches!(src_inner.as_ref(), FullType::Struct(src_tag) if src_tag == dst_tag)
                && matches!(
                    src_exp,
                    Exp::Dot(_, _)
                        | Exp::Arrow(_, _)
                        | Exp::Subscript(_, _)
                        | Exp::Unary(UnaryOp::Deref, _)
                )
            {
                return Ok(());
            }
        }
        self.assert_assignable_full_type(dst, src, context)
    }

    /// Get the FullType for a variable (with fallback)
    fn get_full_type(&self, name: &str) -> FullType {
        if let Some(ft) = self.full_types.get(name) {
            ft.clone()
        } else if let Some(&ct) = self.var_types.get(name) {
            FullType::Scalar(ct)
        } else if let Some(&ct) = self.symbol_types.get(name) {
            FullType::Scalar(ct)
        } else {
            FullType::Scalar(CType::Int)
        }
    }

    /// Get the byte size of the element a pointer points to (using FullType)
    fn ptr_elem_size(&self, name: &str) -> i64 {
        if let Some(ft) = self.full_types.get(name) {
            match ft {
                FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                _ => self.deref_type(name).size() as i64,
            }
        } else {
            self.deref_type(name).size() as i64
        }
    }

    /// Get the FullType of a TackyVal expression result
    fn val_full_type(&self, val: &TackyVal) -> FullType {
        match val {
            TackyVal::Constant(_) => FullType::Scalar(CType::Int),
            TackyVal::DoubleConstant(_) => FullType::Scalar(CType::Double),
            TackyVal::Var(name) => self.get_full_type(name),
        }
    }

    fn fresh_label(&mut self, prefix: &str) -> String {
        let label = format!("{}.{}", prefix, self.label_counter);
        self.label_counter += 1;
        label
    }

    fn emit(&mut self, instr: TackyInstr) {
        self.instructions.push(instr);
    }

    /// Compute the FullType of an expression without evaluating it (for sizeof, etc.)
    fn typeof_exp(&self, exp: &Exp) -> FullType {
        match exp {
            Exp::Constant(_) => FullType::Scalar(CType::Int),
            Exp::LongConstant(_) => FullType::Scalar(CType::Long),
            Exp::UIntConstant(_) => FullType::Scalar(CType::UInt),
            Exp::ULongConstant(_) => FullType::Scalar(CType::ULong),
            Exp::DoubleConstant(_) => FullType::Scalar(CType::Double),
            Exp::StringLiteral(s) => FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size: c_string_byte_len(s) + 1,
            },
            Exp::Var(name) => self.get_full_type(name),
            Exp::Cast(ct, ft, _) => {
                if let Some(ref full) = ft {
                    full.clone()
                } else {
                    FullType::Scalar(*ct)
                }
            }
            Exp::Unary(UnaryOp::Deref, inner) => {
                let inner_ft = self.typeof_exp(inner);
                match inner_ft {
                    FullType::Pointer(pointee) => *pointee,
                    FullType::Array { elem, .. } => *elem,
                    _ => FullType::Scalar(CType::Int),
                }
            }
            Exp::Unary(UnaryOp::AddrOf, inner) => {
                FullType::Pointer(Box::new(self.typeof_exp(inner)))
            }
            Exp::Subscript(arr, _) => {
                let arr_ft = self.typeof_exp(arr);
                match arr_ft {
                    FullType::Array { elem, .. } => *elem,
                    FullType::Pointer(inner) => *inner,
                    _ => FullType::Scalar(CType::Int),
                }
            }
            Exp::Dot(inner, member) => {
                let inner_ft = self.typeof_exp(inner);
                if let FullType::Struct(tag) = &inner_ft {
                    if let Some(def) = self.struct_defs.get(tag) {
                        if let Some(mem) = def.find_member(member) {
                            return mem.member_full_type.clone();
                        }
                    }
                }
                FullType::Scalar(CType::Int)
            }
            Exp::Arrow(inner, member) => {
                let inner_ft = self.typeof_exp(inner);
                let pointee = match inner_ft {
                    FullType::Pointer(p) => *p,
                    _ => inner_ft,
                };
                if let FullType::Struct(tag) = &pointee {
                    if let Some(def) = self.struct_defs.get(tag) {
                        if let Some(mem) = def.find_member(member) {
                            return mem.member_full_type.clone();
                        }
                    }
                }
                FullType::Scalar(CType::Int)
            }
            Exp::FunctionCall(name, _) => {
                if let Some(ft) = self.func_full_types.get(name) {
                    ft.clone()
                } else {
                    let ret_type = self
                        .func_types
                        .get(name)
                        .map(|(rt, _, _, _)| *rt)
                        .unwrap_or(CType::Int);
                    FullType::Scalar(ret_type)
                }
            }
            Exp::SizeOf(_) | Exp::SizeOfType(_, _) | Exp::AlignOfType(_) => {
                FullType::Scalar(CType::ULong)
            }
            Exp::Unary(
                UnaryOp::PreIncrement
                | UnaryOp::PreDecrement
                | UnaryOp::PostIncrement
                | UnaryOp::PostDecrement,
                inner,
            ) => self.typeof_exp(inner),
            Exp::Unary(UnaryOp::Negate | UnaryOp::Complement, inner) => {
                let ft = self.typeof_exp(inner);
                match ft {
                    FullType::Scalar(ct) if ct.size() < 4 => FullType::Scalar(CType::Int),
                    _ => ft,
                }
            }
            Exp::Unary(UnaryOp::LogicalNot, _) => FullType::Scalar(CType::Int),
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
                    FullType::Scalar(CType::Int)
                } else if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
                    let ft = self.typeof_exp(left);
                    match ft {
                        FullType::Scalar(ct) if ct.size() < 4 => FullType::Scalar(CType::Int),
                        _ => ft,
                    }
                } else {
                    let l = self.typeof_exp(left);
                    let r = self.typeof_exp(right);
                    if l.byte_size_with(&self.struct_defs) >= r.byte_size_with(&self.struct_defs) {
                        l
                    } else {
                        r
                    }
                }
            }
            Exp::Assign(left, _) => self.typeof_exp(left),
            Exp::CompoundAssign(_, left, _) => self.typeof_exp(left),
            Exp::Conditional(_, then_e, else_e) => {
                let t = self.typeof_exp(then_e);
                let e = self.typeof_exp(else_e);
                if t.byte_size_with(&self.struct_defs) >= e.byte_size_with(&self.struct_defs) {
                    t
                } else {
                    e
                }
            }
            Exp::Comma(_, right) => self.typeof_exp(right),
            Exp::StatementExpr(_, _, Some(full_type)) => full_type.clone(),
            Exp::StatementExpr(_, _, None) => FullType::Scalar(CType::Void),
            Exp::AtomicFence => FullType::Scalar(CType::Int),
            Exp::Unreachable => FullType::Scalar(CType::Void),
            Exp::AtomicFetch { ptr, .. } => match self.typeof_exp(ptr) {
                FullType::Pointer(inner) => inner.as_ref().clone(),
                _ => FullType::Scalar(CType::Int),
            },
            Exp::AtomicExchange { ptr, .. } => match self.typeof_exp(ptr) {
                FullType::Pointer(inner) => inner.as_ref().clone(),
                _ => FullType::Scalar(CType::Int),
            },
            Exp::AtomicCompareExchange { .. } => FullType::Scalar(CType::Bool),
            Exp::AtomicCompareSwap {
                ptr, return_old, ..
            } => {
                if *return_old {
                    match self.typeof_exp(ptr) {
                        FullType::Pointer(inner) => inner.as_ref().clone(),
                        _ => FullType::Scalar(CType::Int),
                    }
                } else {
                    FullType::Scalar(CType::Bool)
                }
            }
            _ => FullType::Scalar(CType::Int),
        }
    }

    /// Get the byte size of an expression's type (for sizeof) without evaluating it
    fn sizeof_exp(&self, exp: &Exp) -> usize {
        self.typeof_exp(exp).byte_size_with(&self.struct_defs)
    }

    fn static_init_size(v: &StaticInit) -> usize {
        match v {
            StaticInit::CharInit(_) | StaticInit::UCharInit(_) => 1,
            StaticInit::ShortInit(_) | StaticInit::UShortInit(_) => 2,
            StaticInit::IntInit(_) | StaticInit::UIntInit(_) => 4,
            StaticInit::LongInit(_)
            | StaticInit::ULongInit(_)
            | StaticInit::DoubleInit(_)
            | StaticInit::PointerInit(_) => 8,
            StaticInit::FloatInit(_) => 4,
            StaticInit::ZeroInit(n) => *n,
            StaticInit::StringInit(s, null_terminated) => {
                c_string_byte_len(s) + if *null_terminated { 1 } else { 0 }
            }
        }
    }

    /// Create a constant string in read-only data and return its label name
    fn make_string_constant(&mut self, s: &str) -> String {
        let label = format!("__string_const_{}", self.string_counter);
        self.string_counter += 1;
        let size = c_string_byte_len(s) + 1; // including null terminator
        let ft = FullType::Array {
            elem: Box::new(FullType::Scalar(CType::Char)),
            size,
        };
        self.register_var(&label, ft);
        self.static_constants.push(TackyStaticConstant {
            name: label.clone(),
            alignment: 1,
            init: StaticInit::StringInit(s.to_string(), true),
        });
        label
    }

    /// Get the type you get when dereferencing a pointer variable
    fn deref_type(&self, name: &str) -> CType {
        if let Some(&(base, depth)) = self.ptr_info.get(name) {
            if depth <= 1 {
                base
            } else {
                CType::Pointer
            }
        } else {
            CType::Int // fallback
        }
    }

    /// Get the deref info for a result of dereferencing (for propagation)
    fn deref_info(&self, name: &str) -> Option<(CType, usize)> {
        if let Some(&(base, depth)) = self.ptr_info.get(name) {
            if depth > 1 {
                Some((base, depth - 1))
            } else {
                None // fully dereferenced, no longer a pointer
            }
        } else {
            None
        }
    }

    fn type_of(&self, val: &TackyVal) -> CType {
        match val {
            TackyVal::Constant(_) => CType::Int,
            TackyVal::DoubleConstant(_) => CType::Double,
            TackyVal::Var(name) => *self
                .symbol_types
                .get(name)
                .or_else(|| self.var_types.get(name))
                .unwrap_or(&CType::Int),
        }
    }

    /// Insert a cast if needed, returns the (possibly converted) value and its type
    fn convert_to(&mut self, val: TackyVal, from: CType, to: CType) -> TackyVal {
        if from == to {
            return val;
        }
        // Cast to void: no conversion needed, just discard
        if to == CType::Void {
            return val;
        }
        // Cast from void: shouldn't happen in valid code, but treat as no-op
        if from == CType::Void {
            return val;
        }
        if to == CType::Bool {
            let dst = self.fresh_tmp(CType::Bool);
            if from.is_floating() {
                let zero = self.fresh_tmp(from);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(0.0),
                    dst: zero.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::NotEqual,
                    left: val,
                    right: zero,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::NotEqual,
                    left: val,
                    right: TackyVal::Constant(0),
                    dst: dst.clone(),
                });
            }
            return dst;
        }
        let dst = self.fresh_tmp(to);

        // Handle floating-point conversions.
        if to == CType::Double && from == CType::Float {
            self.emit(TackyInstr::FloatToDouble {
                src: val,
                dst: dst.clone(),
            });
            return dst;
        }
        if to == CType::Float && from == CType::Double {
            self.emit(TackyInstr::DoubleToFloat {
                src: val,
                dst: dst.clone(),
            });
            return dst;
        }
        if to == CType::Double && !from.is_floating() {
            if from.is_signed() {
                self.emit(TackyInstr::IntToDouble {
                    src: val,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::UIntToDouble {
                    src: val,
                    dst: dst.clone(),
                });
            }
            return dst;
        }
        if to == CType::Float && !from.is_floating() {
            if from.is_signed() {
                self.emit(TackyInstr::IntToFloat {
                    src: val,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::UIntToFloat {
                    src: val,
                    dst: dst.clone(),
                });
            }
            return dst;
        }
        if from == CType::Double && !to.is_floating() {
            if to.is_signed() {
                self.emit(TackyInstr::DoubleToInt {
                    src: val,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::DoubleToUInt {
                    src: val,
                    dst: dst.clone(),
                });
            }
            return dst;
        }
        if from == CType::Float && !to.is_floating() {
            if to.is_signed() {
                self.emit(TackyInstr::FloatToInt {
                    src: val,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::FloatToUInt {
                    src: val,
                    dst: dst.clone(),
                });
            }
            return dst;
        }

        let from_size = from.size();
        let to_size = to.size();

        if from_size == to_size {
            self.emit(TackyInstr::Copy {
                src: val,
                dst: dst.clone(),
            });
        } else if from_size < to_size {
            if from.is_signed() {
                self.emit(TackyInstr::SignExtend {
                    src: val,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::ZeroExtend {
                    src: val,
                    dst: dst.clone(),
                });
            }
        } else {
            self.emit(TackyInstr::Truncate {
                src: val,
                dst: dst.clone(),
            });
        }
        dst
    }

    // --------------------------------------------------------
    // Expression emission
    // --------------------------------------------------------

    fn emit_exp(&mut self, exp: Exp) -> TackyResult<(TackyVal, CType)> {
        match exp {
            Exp::Unreachable => {
                self.emit(TackyInstr::Unreachable);
                Ok((TackyVal::Constant(0), CType::Void))
            }
            Exp::AtomicFence => {
                self.emit(TackyInstr::AtomicFence);
                Ok((TackyVal::Constant(0), CType::Int))
            }
            Exp::AtomicFetch {
                op,
                ptr,
                arg,
                return_old,
            } => {
                let (ptr_val, _) = self.emit_exp(*ptr)?;
                let pointee_type = if let TackyVal::Var(ref name) = ptr_val {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                if pointee_type.is_floating() || pointee_type == CType::Pointer {
                    return Err(
                        "atomic fetch builtins currently require integer objects".to_string()
                    );
                }
                let (arg_val, arg_type) = self.emit_exp(*arg)?;
                let arg_val = self.convert_to(arg_val, arg_type, pointee_type);
                let dst = self.fresh_tmp(pointee_type);
                self.emit(TackyInstr::AtomicFetch {
                    op: Self::convert_binop(op)?,
                    ptr: ptr_val,
                    arg: arg_val,
                    return_old,
                    dst: dst.clone(),
                });
                Ok((dst, pointee_type))
            }
            Exp::AtomicExchange { ptr, value } => {
                let (ptr_val, _) = self.emit_exp(*ptr)?;
                let pointee_type = if let TackyVal::Var(ref name) = ptr_val {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                if pointee_type.is_floating() {
                    return Err(
                        "atomic exchange builtins currently require integer or pointer objects"
                            .to_string(),
                    );
                }
                let (value_val, value_type) = self.emit_exp(*value)?;
                let value_val = self.convert_to(value_val, value_type, pointee_type);
                let dst = self.fresh_tmp(pointee_type);
                self.emit(TackyInstr::AtomicExchange {
                    ptr: ptr_val,
                    value: value_val,
                    dst: dst.clone(),
                });
                Ok((dst, pointee_type))
            }
            Exp::AtomicCompareExchange {
                ptr,
                expected,
                desired,
            } => {
                let (ptr_val, _) = self.emit_exp(*ptr)?;
                let pointee_type = if let TackyVal::Var(ref name) = ptr_val {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                if pointee_type.is_floating() {
                    return Err(
                        "atomic compare-exchange builtins currently require integer or pointer objects"
                            .to_string(),
                    );
                }
                let (expected_val, _) = self.emit_exp(*expected)?;
                let (desired_val, desired_type) = self.emit_exp(*desired)?;
                let desired_val = self.convert_to(desired_val, desired_type, pointee_type);
                let dst = self.fresh_tmp(CType::Bool);
                self.emit(TackyInstr::AtomicCompareExchange {
                    ptr: ptr_val,
                    expected: expected_val,
                    desired: desired_val,
                    dst: dst.clone(),
                });
                Ok((dst, CType::Bool))
            }
            Exp::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                return_old,
            } => {
                let (ptr_val, _) = self.emit_exp(*ptr)?;
                let pointee_type = if let TackyVal::Var(ref name) = ptr_val {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                if pointee_type.is_floating() {
                    return Err(
                        "sync compare-and-swap builtins currently require integer or pointer objects"
                            .to_string(),
                    );
                }
                let (expected_val, expected_type) = self.emit_exp(*expected)?;
                let expected_val = self.convert_to(expected_val, expected_type, pointee_type);
                let (desired_val, desired_type) = self.emit_exp(*desired)?;
                let desired_val = self.convert_to(desired_val, desired_type, pointee_type);
                let dst_type = if return_old {
                    pointee_type
                } else {
                    CType::Bool
                };
                let dst = self.fresh_tmp(dst_type);
                self.emit(TackyInstr::AtomicCompareSwap {
                    ptr: ptr_val,
                    expected: expected_val,
                    desired: desired_val,
                    return_old,
                    dst: dst.clone(),
                });
                Ok((dst, dst_type))
            }
            Exp::Constant(val) => Ok((TackyVal::Constant(val), CType::Int)),
            Exp::LongConstant(val) => {
                let dst = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::Long))
            }
            Exp::UIntConstant(val) => {
                let dst = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::UInt))
            }
            Exp::ULongConstant(val) => {
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::ULong))
            }
            Exp::DoubleConstant(val) => {
                let dst = self.fresh_tmp(CType::Double);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::Double))
            }
            Exp::SizeOfType(_ct, ft) => {
                let size = ft.byte_size_with(&self.struct_defs) as i64;
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(size),
                    dst: dst.clone(),
                });
                Ok((dst, CType::ULong))
            }
            Exp::AlignOfType(ft) => {
                let alignment = ft.alignment_with(&self.struct_defs) as i64;
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(alignment),
                    dst: dst.clone(),
                });
                Ok((dst, CType::ULong))
            }
            Exp::SizeOf(inner) => {
                let size = self.sizeof_exp(&inner) as i64;
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(size),
                    dst: dst.clone(),
                });
                Ok((dst, CType::ULong))
            }
            Exp::StringLiteral(s) => {
                let label = self.make_string_constant(&s);
                let decayed_ft = FullType::Pointer(Box::new(FullType::Scalar(CType::Char)));
                let ptr = self.fresh_tmp_full(&decayed_ft);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(label),
                    dst: ptr.clone(),
                });
                Ok((ptr, CType::Pointer))
            }
            Exp::Var(name) => self.emit_var(name),
            Exp::Cast(target_type, cast_ft, inner) if !matches!(*inner, Exp::ArrayInit(_)) => {
                self.emit_cast(target_type, cast_ft, *inner)
            }
            Exp::Cast(target_type, cast_ft, inner) => {
                self.emit_compound_literal_cast(target_type, cast_ft, *inner)
            }
            Exp::ArrayInit(_) => {
                Err("Array initializer not allowed in expression context".to_string())
            }
            Exp::DesignatedInit(_, value) => self.emit_exp(*value),
            Exp::StatementExpr(block, result, result_type) => {
                self.emit_block(block)?;
                match result {
                    Some(exp) => self.emit_exp(*exp),
                    None => Ok((
                        TackyVal::Constant(0),
                        result_type
                            .as_ref()
                            .map(FullType::to_ctype)
                            .unwrap_or(CType::Void),
                    )),
                }
            }
            Exp::Assign(left, right) if matches!(left.as_ref(), Exp::Subscript(_, _)) => {
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() {
                    let struct_size = lhs_ft.byte_size_with(&self.struct_defs);
                    let Exp::Subscript(arr, idx) = *left else {
                        return Err("Expression is not a subscript lvalue".to_string());
                    };
                    let right_for_type = (*right).clone();
                    let (ptr, _elem_type, elem_ft) = self.emit_subscript_addr(*arr, *idx)?;
                    let (rhs, rhs_type) = self.emit_exp(*right)?;
                    let rhs_ft = self.val_full_type(&rhs);
                    self.assert_assignable_exp_full_type(
                        &elem_ft,
                        &rhs_ft,
                        &right_for_type,
                        "assignment",
                    )?;
                    let src_addr = if rhs_type == CType::Pointer {
                        rhs
                    } else {
                        self.get_struct_addr(rhs)
                    };
                    self.emit_struct_copy_ptr_to_ptr(src_addr, ptr.clone(), struct_size);
                    return Ok((ptr, CType::Pointer));
                }
                let Exp::Subscript(arr, idx) = *left else {
                    return Err("Expression is not a subscript lvalue".to_string());
                };
                let right_for_type = (*right).clone();
                let (ptr, elem_type, elem_ft) = self.emit_subscript_addr(*arr, *idx)?;
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let rhs_ft = self.val_full_type(&rhs);
                self.assert_assignable_exp_full_type(
                    &elem_ft,
                    &rhs_ft,
                    &right_for_type,
                    "assignment",
                )?;
                let rhs_conv = self.convert_to(rhs, rhs_type, elem_type);
                self.emit(TackyInstr::Store {
                    src: rhs_conv.clone(),
                    dst_ptr: ptr,
                });
                Ok((rhs_conv, elem_type))
            }
            Exp::Assign(left, right) if matches!(left.as_ref(), Exp::Unary(UnaryOp::Deref, _)) => {
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() {
                    let struct_size = lhs_ft.byte_size_with(&self.struct_defs);
                    let Exp::Unary(UnaryOp::Deref, ptr_exp) = *left else {
                        return Err("Expression is not a dereference lvalue".to_string());
                    };
                    let (ptr, _) = self.emit_exp(*ptr_exp)?;
                    let ptr_ft = self.val_full_type(&ptr);
                    let pointee_ft = match ptr_ft {
                        FullType::Pointer(inner) => *inner,
                        _ => lhs_ft,
                    };
                    let right_for_type = (*right).clone();
                    let (rhs, _rhs_type) = self.emit_exp(*right)?;
                    let rhs_ft = self.val_full_type(&rhs);
                    self.assert_assignable_exp_full_type(
                        &pointee_ft,
                        &rhs_ft,
                        &right_for_type,
                        "assignment",
                    )?;
                    let src_addr = if let TackyVal::Var(ref n) = rhs {
                        if self.array_sizes.contains_key(n) {
                            let a = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: rhs,
                                dst: a.clone(),
                            });
                            a
                        } else {
                            rhs
                        }
                    } else {
                        let a = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: rhs,
                            dst: a.clone(),
                        });
                        a
                    };
                    self.emit_struct_copy_ptr_to_ptr(src_addr, ptr, struct_size);
                    return Ok((TackyVal::Constant(0), CType::Struct));
                }
                let Exp::Unary(UnaryOp::Deref, ptr_exp) = *left else {
                    return Err("Expression is not a dereference lvalue".to_string());
                };
                let (ptr, _) = self.emit_exp(*ptr_exp)?;
                let ptr_ft = self.val_full_type(&ptr);
                let pointee_type = if let TackyVal::Var(ref name) = ptr {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                let pointee_ft = match ptr_ft {
                    FullType::Pointer(inner) => *inner,
                    _ => FullType::Scalar(pointee_type),
                };
                let right_for_type = (*right).clone();
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let rhs_ft = self.val_full_type(&rhs);
                self.assert_assignable_exp_full_type(
                    &pointee_ft,
                    &rhs_ft,
                    &right_for_type,
                    "assignment",
                )?;
                let rhs_conv = self.convert_to(rhs, rhs_type, pointee_type);
                self.emit(TackyInstr::Store {
                    src: rhs_conv.clone(),
                    dst_ptr: ptr,
                });
                Ok((rhs_conv, pointee_type))
            }
            Exp::Assign(left, right) if matches!(left.as_ref(), Exp::Dot(_, _)) => {
                let Exp::Dot(inner, member) = *left else {
                    return Err("Expression is not a dot lvalue".to_string());
                };
                if let Exp::Var(struct_name) = *inner {
                    let ft = self.get_full_type(&struct_name);
                    let tag = match &ft {
                        FullType::Struct(tag) => tag.clone(),
                        _ => return Err(format!("Dot on non-struct: {:?}", ft)),
                    };
                    let mem = self.struct_member(&tag, &member)?;
                    let mem_ft = mem.member_full_type.clone();
                    if mem.bit_width.is_some() {
                        let right_for_type = (*right).clone();
                        let (rhs, rhs_type) = self.emit_exp(*right)?;
                        let rhs_ft = self.val_full_type(&rhs);
                        self.assert_assignable_exp_full_type(
                            &mem_ft,
                            &rhs_ft,
                            &right_for_type,
                            "assignment",
                        )?;
                        let rhs_conv = self.convert_to(rhs, rhs_type, mem.member_type);
                        let value = self.store_bit_field_to_offset(struct_name, &mem, rhs_conv)?;
                        return Ok((value, mem.member_type));
                    }
                    if mem_ft.is_struct() {
                        let left_exp = Exp::Dot(Box::new(Exp::Var(struct_name)), member);
                        let dst_addr = self.emit_dot_address(&left_exp)?;
                        let right_for_type = (*right).clone();
                        let (rhs, rhs_type) = self.emit_exp(*right)?;
                        let rhs_ft = self.val_full_type(&rhs);
                        self.assert_assignable_exp_full_type(
                            &mem_ft,
                            &rhs_ft,
                            &right_for_type,
                            "assignment",
                        )?;
                        let src_addr = if rhs_type == CType::Pointer {
                            rhs
                        } else {
                            self.get_struct_addr(rhs)
                        };
                        self.emit_struct_copy_ptr_to_ptr(
                            src_addr,
                            dst_addr,
                            mem_ft.byte_size_with(&self.struct_defs),
                        );
                        return Ok((TackyVal::Constant(0), CType::Struct));
                    }
                    let right_for_type = (*right).clone();
                    let (rhs, rhs_type) = self.emit_exp(*right)?;
                    let rhs_ft = self.val_full_type(&rhs);
                    self.assert_assignable_exp_full_type(
                        &mem_ft,
                        &rhs_ft,
                        &right_for_type,
                        "assignment",
                    )?;
                    let rhs_conv = self.convert_to(rhs, rhs_type, mem.member_type);
                    self.emit(TackyInstr::CopyToOffset {
                        src: rhs_conv.clone(),
                        dst_name: struct_name,
                        offset: mem.offset as i64,
                    });
                    Ok((rhs_conv, mem.member_type))
                } else {
                    let left_exp = Exp::Dot(inner, member);
                    let mem_ft = self.typeof_exp(&left_exp);
                    let mem = {
                        if let Exp::Dot(ref inner, ref member) = left_exp {
                            let tag = self.dot_inner_tag(inner)?;
                            self.struct_member(&tag, member)?
                        } else {
                            unreachable!()
                        }
                    };
                    if mem.bit_width.is_some() {
                        let member_addr = self.emit_dot_address(&left_exp)?;
                        let right_for_type = (*right).clone();
                        let (rhs, rhs_type) = self.emit_exp(*right)?;
                        let rhs_ft = self.val_full_type(&rhs);
                        self.assert_assignable_exp_full_type(
                            &mem.member_full_type,
                            &rhs_ft,
                            &right_for_type,
                            "assignment",
                        )?;
                        let rhs_conv = self.convert_to(rhs, rhs_type, mem.member_type);
                        let value = self.store_bit_field_to_ptr(member_addr, &mem, rhs_conv)?;
                        return Ok((value, mem.member_type));
                    }
                    if mem_ft.is_struct() {
                        let dst_addr = self.emit_dot_address(&left_exp)?;
                        let right_for_type = (*right).clone();
                        let (rhs, rhs_type) = self.emit_exp(*right)?;
                        let rhs_ft = self.val_full_type(&rhs);
                        self.assert_assignable_exp_full_type(
                            &mem_ft,
                            &rhs_ft,
                            &right_for_type,
                            "assignment",
                        )?;
                        let src_addr = if rhs_type == CType::Pointer {
                            rhs
                        } else {
                            self.get_struct_addr(rhs)
                        };
                        self.emit_struct_copy_ptr_to_ptr(
                            src_addr,
                            dst_addr,
                            mem_ft.byte_size_with(&self.struct_defs),
                        );
                        return Ok((TackyVal::Constant(0), CType::Struct));
                    }
                    let member_addr = self.emit_dot_address(&left_exp)?;
                    let mem_type = mem_ft.to_ctype();
                    let right_for_type = (*right).clone();
                    let (rhs, rhs_type) = self.emit_exp(*right)?;
                    let rhs_ft = self.val_full_type(&rhs);
                    self.assert_assignable_exp_full_type(
                        &mem_ft,
                        &rhs_ft,
                        &right_for_type,
                        "assignment",
                    )?;
                    let rhs_conv = self.convert_to(rhs, rhs_type, mem_type);
                    self.emit(TackyInstr::Store {
                        src: rhs_conv.clone(),
                        dst_ptr: member_addr,
                    });
                    Ok((rhs_conv, mem_type))
                }
            }
            Exp::Assign(left, right) if matches!(left.as_ref(), Exp::Arrow(_, _)) => {
                let Exp::Arrow(inner, member) = *left else {
                    return Err("Expression is not an arrow lvalue".to_string());
                };
                let (ptr_val, _) = self.emit_exp(*inner)?;
                let ptr_ft = self.val_full_type(&ptr_val);
                let mem = match &ptr_ft {
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(tag) => self.struct_member(tag, &member)?,
                        _ => self
                            .struct_defs
                            .values()
                            .find_map(|def| def.find_member(&member).cloned())
                            .ok_or_else(|| {
                                format!(
                                    "Arrow assignment: cannot find struct for member {}",
                                    member
                                )
                            })?,
                    },
                    FullType::Scalar(CType::Pointer) => self
                        .struct_defs
                        .values()
                        .find_map(|def| def.find_member(&member).cloned())
                        .ok_or_else(|| {
                            format!("Arrow assignment: cannot find struct for member {}", member)
                        })?,
                    _ => return Err(format!("Arrow on non-pointer: {:?}", ptr_ft)),
                };
                let mem_type = mem.member_type;
                let mem_offset = mem.offset;
                let mem_ft = mem.member_full_type.clone();
                let mem_ptr = self.fresh_tmp(CType::Pointer);
                if mem_offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ptr_val,
                        right: TackyVal::Constant(mem_offset as i64),
                        dst: mem_ptr.clone(),
                    });
                } else {
                    self.emit(TackyInstr::Copy {
                        src: ptr_val,
                        dst: mem_ptr.clone(),
                    });
                }
                if mem_ft.is_struct() {
                    let right_for_type = (*right).clone();
                    let (rhs, rhs_type) = self.emit_exp(*right)?;
                    let rhs_ft = self.val_full_type(&rhs);
                    self.assert_assignable_exp_full_type(
                        &mem_ft,
                        &rhs_ft,
                        &right_for_type,
                        "assignment",
                    )?;
                    let src_addr = if rhs_type == CType::Pointer {
                        rhs
                    } else {
                        self.get_struct_addr(rhs)
                    };
                    self.emit_struct_copy_ptr_to_ptr(
                        src_addr,
                        mem_ptr,
                        mem_ft.byte_size_with(&self.struct_defs),
                    );
                    return Ok((TackyVal::Constant(0), CType::Struct));
                }
                let right_for_type = (*right).clone();
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let rhs_ft = self.val_full_type(&rhs);
                self.assert_assignable_exp_full_type(
                    &mem_ft,
                    &rhs_ft,
                    &right_for_type,
                    "assignment",
                )?;
                let rhs_conv = self.convert_to(rhs, rhs_type, mem_type);
                if mem.bit_width.is_some() {
                    let value = self.store_bit_field_to_ptr(mem_ptr, &mem, rhs_conv)?;
                    return Ok((value, mem_type));
                }
                self.emit(TackyInstr::Store {
                    src: rhs_conv.clone(),
                    dst_ptr: mem_ptr,
                });
                Ok((rhs_conv, mem_type))
            }
            Exp::Assign(left, right) if matches!(left.as_ref(), Exp::Var(_)) => {
                let lhs_type = self.lvalue_type(&left);
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() {
                    let Exp::Var(lhs_name) = *left else {
                        return Err("Expression is not a variable lvalue".to_string());
                    };
                    let struct_size = lhs_ft.byte_size_with(&self.struct_defs);
                    let right_for_type = (*right).clone();
                    let (rhs, rhs_type) = self.emit_exp(*right)?;
                    let rhs_ft = self.val_full_type(&rhs);
                    self.assert_assignable_exp_full_type(
                        &lhs_ft,
                        &rhs_ft,
                        &right_for_type,
                        "assignment",
                    )?;
                    let rhs_struct_name = if rhs_type == CType::Struct {
                        if let TackyVal::Var(ref n) = rhs {
                            Some(n.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let src_addr = if rhs_type == CType::Pointer {
                        rhs
                    } else {
                        self.get_struct_addr(rhs)
                    };
                    if let Some(src_name) = rhs_struct_name {
                        self.emit(TackyInstr::CopyStruct {
                            src_name,
                            dst_name: lhs_name.clone(),
                        });
                    } else {
                        self.emit_struct_copy_to(src_addr, &lhs_name, struct_size);
                    }
                    return Ok((TackyVal::Var(lhs_name), CType::Struct));
                }
                let right_for_type = (*right).clone();
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let rhs_ft = self.val_full_type(&rhs);
                self.assert_assignable_exp_full_type(
                    &lhs_ft,
                    &rhs_ft,
                    &right_for_type,
                    "assignment",
                )?;
                let rhs_conv = self.convert_to(rhs, rhs_type, lhs_type);
                let lhs = self.emit_lvalue(*left)?;
                if lhs_type == CType::Pointer {
                    if let TackyVal::Var(ref lhs_name) = lhs {
                        if let TackyVal::Var(ref rhs_name) = rhs_conv {
                            if let Some(&info) = self.ptr_info.get(rhs_name) {
                                self.ptr_info.insert(lhs_name.clone(), info);
                            }
                            let lhs_has_specific = self
                                .full_types
                                .get(lhs_name)
                                .map(|ft| {
                                    matches!(ft, FullType::Pointer(inner) if inner.is_array() || inner.is_struct())
                                })
                                .unwrap_or(false);
                            if !lhs_has_specific {
                                if let Some(ft) = self.full_types.get(rhs_name).cloned() {
                                    self.full_types.insert(lhs_name.clone(), ft);
                                }
                            }
                        }
                    }
                }
                self.emit(TackyInstr::Copy {
                    src: rhs_conv,
                    dst: lhs.clone(),
                });
                Ok((lhs, lhs_type))
            }
            Exp::Assign(left, _)
                if !matches!(
                    left.as_ref(),
                    Exp::Subscript(_, _)
                        | Exp::Unary(UnaryOp::Deref, _)
                        | Exp::Dot(_, _)
                        | Exp::Arrow(_, _)
                ) =>
            {
                Err("Expression is not a simple lvalue".to_string())
            }
            Exp::CompoundAssign(op, left, right)
                if matches!(left.as_ref(), Exp::Subscript(_, _)) =>
            {
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() {
                    return Err("compound assignment to non-scalar lvalue".to_string());
                }
                let Exp::Subscript(arr, idx) = *left else {
                    return Err("Expression is not a subscript lvalue".to_string());
                };
                let (ptr, elem_type, elem_full) = self.emit_subscript_addr(*arr, *idx)?;
                let cur_val = self.fresh_tmp_full(&elem_full);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: cur_val.clone(),
                });
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                if elem_type == CType::Pointer && matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                    let elem_size = match &elem_full {
                        FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                        _ => 1,
                    };
                    let rhs_long = self.convert_to(rhs, rhs_type, CType::Long);
                    let scaled = if elem_size > 1 {
                        let s = self.fresh_tmp(CType::Long);
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Mul,
                            left: rhs_long,
                            right: TackyVal::Constant(elem_size),
                            dst: s.clone(),
                        });
                        s
                    } else {
                        rhs_long
                    };
                    let result = self.fresh_tmp_full(&elem_full);
                    let tacky_op = Self::convert_binop(op)?;
                    self.emit(TackyInstr::Binary {
                        op: tacky_op,
                        left: cur_val,
                        right: scaled,
                        dst: result.clone(),
                    });
                    self.emit(TackyInstr::Store {
                        src: result.clone(),
                        dst_ptr: ptr,
                    });
                    return Ok((result, elem_type));
                }
                let common = CType::common(elem_type, rhs_type);
                let lhs_conv = self.convert_to(cur_val, elem_type, common);
                let rhs_conv = self.convert_to(rhs, rhs_type, common);
                let result = self.fresh_tmp(common);
                let tacky_op = Self::convert_binop(op)?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: lhs_conv,
                    right: rhs_conv,
                    dst: result.clone(),
                });
                let result_conv = self.convert_to(result, common, elem_type);
                self.emit(TackyInstr::Store {
                    src: result_conv.clone(),
                    dst_ptr: ptr,
                });
                Ok((result_conv, elem_type))
            }
            Exp::CompoundAssign(op, left, right)
                if matches!(left.as_ref(), Exp::Unary(UnaryOp::Deref, _)) =>
            {
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() {
                    return Err("compound assignment to non-scalar lvalue".to_string());
                }
                let Exp::Unary(UnaryOp::Deref, ptr_exp) = *left else {
                    return Err("Expression is not a dereference lvalue".to_string());
                };
                let (ptr, _) = self.emit_exp(*ptr_exp)?;
                let pointee_type = if let TackyVal::Var(ref name) = ptr {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                let cur_val = self.fresh_tmp(pointee_type);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: cur_val.clone(),
                });
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let common = CType::common(pointee_type, rhs_type);
                let lhs_conv = self.convert_to(cur_val, pointee_type, common);
                let rhs_conv = self.convert_to(rhs, rhs_type, common);
                let result = self.fresh_tmp(common);
                let tacky_op = Self::convert_binop(op)?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: lhs_conv,
                    right: rhs_conv,
                    dst: result.clone(),
                });
                let result_conv = self.convert_to(result, common, pointee_type);
                self.emit(TackyInstr::Store {
                    src: result_conv.clone(),
                    dst_ptr: ptr,
                });
                Ok((result_conv, pointee_type))
            }
            Exp::CompoundAssign(op, left, right)
                if matches!(left.as_ref(), Exp::Dot(_, _) | Exp::Arrow(_, _)) =>
            {
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() {
                    return Err("compound assignment to non-scalar lvalue".to_string());
                }
                let (ptr, lhs_type, lhs_ft) = self
                    .scalar_lvalue_address(*left)?
                    .ok_or_else(|| "Expression is not a simple lvalue".to_string())?;
                let cur_val = self.fresh_tmp_full(&lhs_ft);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: cur_val.clone(),
                });
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                if lhs_type == CType::Pointer && matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                    let elem_size = match &lhs_ft {
                        FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                        _ => 1,
                    };
                    let rhs_long = self.convert_to(rhs, rhs_type, CType::Long);
                    let scaled = if elem_size > 1 {
                        let s = self.fresh_tmp(CType::Long);
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Mul,
                            left: rhs_long,
                            right: TackyVal::Constant(elem_size),
                            dst: s.clone(),
                        });
                        s
                    } else {
                        rhs_long
                    };
                    let result = self.fresh_tmp_full(&lhs_ft);
                    let tacky_op = Self::convert_binop(op)?;
                    self.emit(TackyInstr::Binary {
                        op: tacky_op,
                        left: cur_val,
                        right: scaled,
                        dst: result.clone(),
                    });
                    self.emit(TackyInstr::Store {
                        src: result.clone(),
                        dst_ptr: ptr,
                    });
                    return Ok((result, lhs_type));
                }
                let common = CType::common(lhs_type, rhs_type);
                let lhs_conv = self.convert_to(cur_val, lhs_type, common);
                let rhs_conv = self.convert_to(rhs, rhs_type, common);
                let result = self.fresh_tmp(common);
                let tacky_op = Self::convert_binop(op)?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: lhs_conv,
                    right: rhs_conv,
                    dst: result.clone(),
                });
                let result_conv = self.convert_to(result, common, lhs_type);
                self.emit(TackyInstr::Store {
                    src: result_conv.clone(),
                    dst_ptr: ptr,
                });
                Ok((result_conv, lhs_type))
            }
            Exp::CompoundAssign(op, left, right) if matches!(left.as_ref(), Exp::Var(_)) => {
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() {
                    return Err("compound assignment to non-scalar lvalue".to_string());
                }
                let lhs_type = self.lvalue_type(&left);
                let lhs = self.emit_lvalue(*left)?;
                let (rhs, rhs_type) = self.emit_exp(*right)?;

                if lhs_type == CType::Pointer && matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                    let elem_size = if let TackyVal::Var(ref n) = lhs {
                        self.ptr_elem_size(n)
                    } else {
                        1
                    };
                    let rhs_long = self.convert_to(rhs, rhs_type, CType::Long);
                    let scaled = if elem_size > 1 {
                        let s = self.fresh_tmp(CType::Long);
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Mul,
                            left: rhs_long,
                            right: TackyVal::Constant(elem_size),
                            dst: s.clone(),
                        });
                        s
                    } else {
                        rhs_long
                    };
                    let lhs_ft = self.val_full_type(&lhs);
                    let dst = self.fresh_tmp_full(&lhs_ft);
                    let tacky_op = Self::convert_binop(op)?;
                    self.emit(TackyInstr::Binary {
                        op: tacky_op,
                        left: lhs.clone(),
                        right: scaled,
                        dst: dst.clone(),
                    });
                    if let TackyVal::Var(ref vn) = lhs {
                        if let Some(&info) = self.ptr_info.get(vn) {
                            if let TackyVal::Var(ref dn) = dst {
                                self.ptr_info.insert(dn.clone(), info);
                            }
                        }
                    }
                    self.emit(TackyInstr::Copy {
                        src: dst,
                        dst: lhs.clone(),
                    });
                    return Ok((lhs, lhs_type));
                }

                let is_shift = matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
                let (lhs_conv, rhs_conv, result_type) = if is_shift {
                    (lhs.clone(), rhs, lhs_type)
                } else {
                    let common = CType::common(lhs_type, rhs_type);
                    let lc = self.convert_to(lhs.clone(), lhs_type, common);
                    let rc = self.convert_to(rhs, rhs_type, common);
                    let rt = if is_comparison_op(&op) {
                        CType::Int
                    } else {
                        common
                    };
                    (lc, rc, rt)
                };

                let dst = self.fresh_tmp(result_type);
                let tacky_op = Self::convert_binop(op)?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: lhs_conv,
                    right: rhs_conv,
                    dst: dst.clone(),
                });
                let dst_conv = self.convert_to(dst, result_type, lhs_type);
                self.emit(TackyInstr::Copy {
                    src: dst_conv,
                    dst: lhs.clone(),
                });
                Ok((lhs, lhs_type))
            }
            Exp::CompoundAssign(_, left, _)
                if !matches!(
                    left.as_ref(),
                    Exp::Subscript(_, _)
                        | Exp::Unary(UnaryOp::Deref, _)
                        | Exp::Dot(_, _)
                        | Exp::Arrow(_, _)
                ) =>
            {
                Err("Expression is not a simple lvalue".to_string())
            }
            Exp::Dot(inner, member) => {
                if let Exp::Var(ref n) = *inner {
                    let ft = self.get_full_type(n);
                    let tag = match &ft {
                        FullType::Struct(t) => t.clone(),
                        _ => return Err(format!("Dot on non-struct: {:?}", ft)),
                    };
                    let mem = self.struct_member(&tag, &member)?;
                    let mem_ft = mem.member_full_type.clone();
                    if mem_ft.is_array() || mem_ft.is_struct() {
                        let ptr_ft = FullType::Pointer(Box::new(if mem_ft.is_array() {
                            match &mem_ft {
                                FullType::Array { elem, .. } => *elem.clone(),
                                _ => mem_ft.clone(),
                            }
                        } else {
                            mem_ft.clone()
                        }));
                        let ptr = self.fresh_tmp_full(&if mem_ft.is_struct() {
                            FullType::Pointer(Box::new(mem_ft.clone()))
                        } else {
                            ptr_ft
                        });
                        let addr = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: TackyVal::Var(n.clone()),
                            dst: addr.clone(),
                        });
                        if mem.offset > 0 {
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::Add,
                                left: addr,
                                right: TackyVal::Constant(mem.offset as i64),
                                dst: ptr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: addr,
                                dst: ptr.clone(),
                            });
                        }
                        return Ok((ptr, CType::Pointer));
                    }
                    let result = self.fresh_tmp_full(&mem_ft);
                    self.emit(TackyInstr::CopyFromOffset {
                        src_name: n.clone(),
                        offset: mem.offset as i64,
                        dst: result.clone(),
                    });
                    let result = self.extract_bit_field(result, &mem)?;
                    Ok((result, mem.member_type))
                } else {
                    let (val, _) = self.emit_exp(*inner)?;
                    let val_ft = self.val_full_type(&val);
                    let (struct_addr, tag) = match &val_ft {
                        FullType::Struct(t) => {
                            let addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: addr.clone(),
                            });
                            (addr, t.clone())
                        }
                        FullType::Pointer(inner_ft) => match inner_ft.as_ref() {
                            FullType::Struct(t) => (val, t.clone()),
                            _ => return Err(format!("Dot on non-struct result: {:?}", val_ft)),
                        },
                        _ => return Err(format!("Dot on non-struct result: {:?}", val_ft)),
                    };
                    self.access_struct_member(struct_addr, tag, &member)
                }
            }
            Exp::Arrow(inner, member) => {
                let (ptr_val, _) = self.emit_exp(*inner)?;
                let ptr_ft = self.val_full_type(&ptr_val);
                let tag = match &ptr_ft {
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => return Err(format!("Arrow on non-struct-pointer: {:?}", ptr_ft)),
                    },
                    _ => return Err(format!("Arrow on non-pointer: {:?}", ptr_ft)),
                };
                self.access_struct_member(ptr_val, tag, &member)
            }
            Exp::Comma(left, right) => {
                self.emit_exp(*left)?;
                self.emit_exp(*right)
            }
            Exp::Subscript(arr, idx) => self.emit_subscript(*arr, *idx),
            Exp::Conditional(cond, then_exp, else_exp) => {
                self.emit_conditional(*cond, *then_exp, *else_exp)
            }
            Exp::FunctionCall(name, args) => self.emit_function_call(name, args),
            Exp::IndirectCall(callee, args) => self.emit_indirect_call(*callee, args),
            Exp::Binary(BinaryOp::LogicalAnd, left, right) => {
                Ok((self.emit_logical_and(*left, *right)?, CType::Int))
            }
            Exp::Binary(BinaryOp::LogicalOr, left, right) => {
                Ok((self.emit_logical_or(*left, *right)?, CType::Int))
            }
            Exp::Binary(op, left, right) => self.emit_binary(op, *left, *right),
            Exp::Unary(op @ (UnaryOp::Negate | UnaryOp::Complement), inner) => {
                self.emit_scalar_unary(op, *inner)
            }
            Exp::Unary(UnaryOp::LogicalNot, inner) => {
                let (src, _) = self.emit_exp(*inner)?;
                let dst = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Unary {
                    op: TackyUnaryOp::LogicalNot,
                    src,
                    dst: dst.clone(),
                });
                Ok((dst, CType::Int))
            }
            Exp::Unary(UnaryOp::AddrOf, inner) => self.emit_addr_of(*inner),
            Exp::Unary(UnaryOp::Deref, inner) => self.emit_deref(*inner),
            Exp::Unary(
                op @ (UnaryOp::PreIncrement
                | UnaryOp::PreDecrement
                | UnaryOp::PostIncrement
                | UnaryOp::PostDecrement),
                inner,
            ) => self.emit_inc_dec(op, *inner),
            Exp::Assign(_, _) => Err("Expression is not a simple lvalue".to_string()),
            Exp::CompoundAssign(_, _, _) => Err("Expression is not a simple lvalue".to_string()),
        }
    }

    fn emit_var(&mut self, name: String) -> TackyResult<(TackyVal, CType)> {
        if self.func_types.contains_key(&name) {
            let (return_type, _, _, variadic) = self
                .func_types
                .get(&name)
                .cloned()
                .ok_or_else(|| format!("unknown function '{}'", name))?;
            let return_full_type = self
                .func_full_types
                .get(&name)
                .cloned()
                .unwrap_or(FullType::Scalar(return_type));
            let param_full_types = self
                .func_param_full_types
                .get(&name)
                .cloned()
                .unwrap_or_default();
            let fn_ptr_type = FullType::Pointer(Box::new(FullType::Function {
                return_type: Box::new(return_full_type),
                params: param_full_types,
                variadic,
            }));
            let ptr = self.fresh_tmp_full(&fn_ptr_type);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(name.clone()),
                dst: ptr.clone(),
            });
            self.extern_vars.push(name);
            return Ok((ptr, CType::Pointer));
        }

        let ft = self.get_full_type(&name);
        if ft.is_array() {
            let decayed = ft.decay();
            let ptr = self.fresh_tmp_full(&decayed);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(name),
                dst: ptr.clone(),
            });
            return Ok((ptr, decayed.to_ctype()));
        }
        let t = ft.to_ctype();
        Ok((TackyVal::Var(name), t))
    }

    fn emit_cast(
        &mut self,
        target_type: CType,
        cast_ft: Option<FullType>,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let (val, from_type) = self.emit_exp(inner)?;
        let converted = self.convert_to(val, from_type, target_type);
        if let Some(ft) = cast_ft {
            let result = if from_type == target_type {
                let copy = self.fresh_tmp_full(&ft);
                self.emit(TackyInstr::Copy {
                    src: converted,
                    dst: copy.clone(),
                });
                copy
            } else {
                if let TackyVal::Var(ref name) = converted {
                    self.full_types.insert(name.clone(), ft.clone());
                    if let FullType::Pointer(ref inner_ft) = ft {
                        let (base, depth) = Self::ptr_info_from_full(inner_ft);
                        self.ptr_info.insert(name.clone(), (base, depth));
                    }
                }
                converted
            };
            return Ok((result, target_type));
        }
        Ok((converted, target_type))
    }

    fn emit_compound_literal_cast(
        &mut self,
        target_type: CType,
        cast_ft: Option<FullType>,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let Some(ft) = cast_ft else {
            if let Exp::ArrayInit(elems) = inner {
                if let Some(first) = elems.into_iter().next() {
                    let (val, from_type) = self.emit_exp(first)?;
                    let converted = self.convert_to(val, from_type, target_type);
                    return Ok((converted, target_type));
                }
            }
            return Ok((TackyVal::Constant(0), target_type));
        };

        if let FullType::Struct(ref tag) = ft {
            let tmp_name = self.fresh_var_name();
            self.register_var(&tmp_name, ft.clone());
            let size = self
                .struct_defs
                .get(tag)
                .map(|def| def.size)
                .unwrap_or_else(|| ft.byte_size_with(&self.struct_defs));
            self.zero_init_local(&tmp_name, size);
            self.emit_struct_init_at(&tmp_name, &inner, tag, 0)?;
            return Ok((TackyVal::Var(tmp_name), target_type));
        }

        if ft.is_array() {
            let tmp_name = self.fresh_var_name();
            self.register_var(&tmp_name, ft.clone());
            let size = ft.byte_size_with(&self.struct_defs);
            self.zero_init_local(&tmp_name, size);
            let elem_sizes = Self::compute_elem_sizes(&ft, &self.struct_defs);
            let inner_scalar = {
                let mut t = &ft;
                while let FullType::Array { elem, .. } = t {
                    t = elem;
                }
                t.to_ctype()
            };
            self.emit_array_init_flat(&tmp_name, &inner, inner_scalar, 0, &elem_sizes)?;
            let decayed = ft.decay();
            let ptr = self.fresh_tmp_full(&decayed);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(tmp_name),
                dst: ptr.clone(),
            });
            return Ok((ptr, decayed.to_ctype()));
        }

        if let Exp::ArrayInit(elems) = inner {
            if let Some(first) = elems.into_iter().next() {
                let (val, from_type) = self.emit_exp(first)?;
                let converted = self.convert_to(val, from_type, target_type);
                return Ok((converted, target_type));
            }
        }
        Ok((TackyVal::Constant(0), target_type))
    }

    fn emit_function_call(
        &mut self,
        name: String,
        args: Vec<Exp>,
    ) -> TackyResult<(TackyVal, CType)> {
        let builtin_info = Self::builtin_function_info(&name);
        let call_name = builtin_info
            .as_ref()
            .map(|(call_name, _, _, _, _)| (*call_name).to_string())
            .unwrap_or_else(|| name.clone());
        let pointer_sig = self
            .full_types
            .get(&name)
            .and_then(Self::function_signature_from_full);
        let (ret_type, param_types, ret_pi, variadic) =
            if let Some((_, ret_type, _, param_types, ret_pi)) = builtin_info.as_ref() {
                (*ret_type, param_types.clone(), *ret_pi, false)
            } else {
                self.func_types
                    .get(&name)
                    .cloned()
                    .or_else(|| {
                        pointer_sig.as_ref().map(|(ret_ft, param_fts, variadic)| {
                            let ret_pi = match ret_ft {
                                FullType::Pointer(inner) => Some(Self::ptr_info_from_full(inner)),
                                _ => None,
                            };
                            (
                                ret_ft.to_ctype(),
                                param_fts.iter().map(FullType::to_ctype).collect(),
                                ret_pi,
                                *variadic,
                            )
                        })
                    })
                    .unwrap_or((CType::Int, Vec::new(), None, false))
            };
        let has_prototype =
            builtin_info.is_some() || self.func_types.contains_key(&name) || pointer_sig.is_some();
        if has_prototype && !variadic && args.len() != param_types.len() {
            return Err(format!(
                "function '{}' called with {} argument(s), but prototype expects {}",
                name,
                args.len(),
                param_types.len()
            ));
        }
        if has_prototype && variadic && args.len() < param_types.len() {
            return Err(format!(
                "variadic function '{}' called with {} argument(s), but prototype requires at least {}",
                name,
                args.len(),
                param_types.len()
            ));
        }

        let ret_ft = builtin_info
            .as_ref()
            .map(|(_, _, ret_ft, _, _)| ret_ft.clone())
            .or_else(|| {
                self.func_full_types
                    .get(&name)
                    .cloned()
                    .or_else(|| pointer_sig.as_ref().map(|(ret_ft, _, _)| ret_ft.clone()))
            });
        let param_full_types: Vec<FullType> = self
            .func_param_full_types
            .get(&name)
            .cloned()
            .or_else(|| {
                pointer_sig
                    .as_ref()
                    .map(|(_, param_fts, _)| param_fts.clone())
            })
            .unwrap_or_default();

        let mut tacky_args = Vec::new();
        let mut stack_arg_indices = std::collections::HashSet::new();
        let mut struct_arg_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut fixed_flat_arg_count = 0usize;
        for (i, arg) in args.into_iter().enumerate() {
            let arg_for_type = arg.clone();
            let (val, val_type) = self.emit_exp(arg)?;
            let val_ft = self.val_full_type(&val);
            let expected = param_types
                .get(i)
                .copied()
                .unwrap_or_else(|| val_type.promote());
            if let Some(expected_ft) = param_full_types.get(i) {
                self.assert_assignable_exp_full_type(
                    expected_ft,
                    &val_ft,
                    &arg_for_type,
                    "function call",
                )?;
            }

            let is_struct_arg = val_ft.is_struct() || val_type == CType::Struct;
            if is_struct_arg || expected == CType::Struct {
                let tag = match &val_ft {
                    FullType::Struct(t) => t.clone(),
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => {
                            tacky_args.push(val);
                            if i + 1 == param_types.len() {
                                fixed_flat_arg_count = tacky_args.len();
                            }
                            continue;
                        }
                    },
                    _ => {
                        tacky_args.push(val);
                        if i + 1 == param_types.len() {
                            fixed_flat_arg_count = tacky_args.len();
                        }
                        continue;
                    }
                };

                if let Some(def) = self.struct_defs.get(&tag).cloned() {
                    let classes = def.classify_with(&self.struct_defs);
                    if classes.len() == 1 && classes[0] == ParamClass::Memory {
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let num_eightbytes = def.size.div_ceil(8);
                        let start_idx = tacky_args.len();
                        for eb in 0..num_eightbytes {
                            let eb_offset = (eb * 8) as i64;
                            let tmp = self.fresh_tmp(CType::Long);
                            stack_arg_indices.insert(start_idx + eb);
                            let ptr = self.fresh_tmp(CType::Pointer);
                            if eb_offset > 0 {
                                self.emit(TackyInstr::Binary {
                                    op: TackyBinaryOp::Add,
                                    left: struct_addr.clone(),
                                    right: TackyVal::Constant(eb_offset),
                                    dst: ptr.clone(),
                                });
                            } else {
                                self.emit(TackyInstr::Copy {
                                    src: struct_addr.clone(),
                                    dst: ptr.clone(),
                                });
                            }
                            self.emit(TackyInstr::Load {
                                src_ptr: ptr,
                                dst: tmp.clone(),
                            });
                            tacky_args.push(tmp);
                        }
                    } else {
                        let struct_var_name = if val_ft.is_struct() {
                            if let TackyVal::Var(ref n) = val {
                                Some(n.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let group_start = tacky_args.len();
                        let is_sse_vec: Vec<bool> =
                            classes.iter().map(|c| *c == ParamClass::Sse).collect();
                        struct_arg_groups.push((group_start, classes.len(), is_sse_vec));
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i64;
                            match class {
                                ParamClass::Sse => {
                                    let tmp = self.fresh_tmp(CType::Double);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                ParamClass::Integer => {
                                    let tmp = self.fresh_tmp(CType::Long);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                _ => tacky_args.push(TackyVal::Constant(0)),
                            }
                        }
                    }
                } else {
                    tacky_args.push(self.convert_to(val, val_type, expected));
                }
            } else {
                tacky_args.push(self.convert_to(val, val_type, expected));
            }

            if i + 1 == param_types.len() {
                fixed_flat_arg_count = tacky_args.len();
            }
        }
        if param_types.is_empty() {
            fixed_flat_arg_count = 0;
        }

        let uses_hidden_ptr = if let Some(FullType::Struct(ref tag)) = ret_ft {
            self.struct_defs
                .get(tag)
                .map(|d| d.size > 16)
                .unwrap_or(false)
        } else {
            false
        };
        let is_indirect = builtin_info.is_none()
            && !self.func_types.contains_key(&name)
            && !name.starts_with("__builtin_");
        if uses_hidden_ptr {
            let rft = ret_ft
                .as_ref()
                .ok_or_else(|| "missing struct return type".to_string())?;
            let tmp = self.fresh_tmp_full(rft);
            if let FullType::Struct(ref tag) = rft {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    if let Some(def) = self.struct_defs.get(tag) {
                        self.array_sizes.insert(tmp_name.clone(), def.size);
                    }
                }
            }
            let ret_addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: tmp.clone(),
                dst: ret_addr.clone(),
            });
            tacky_args.insert(0, ret_addr);
            let shifted_stack = stack_arg_indices.iter().map(|&i| i + 1).collect();
            let shifted_groups: Vec<(usize, usize, Vec<bool>)> = struct_arg_groups
                .iter()
                .map(|(start, count, classes)| (start + 1, *count, classes.clone()))
                .collect();
            self.emit(TackyInstr::FunCall {
                name: call_name,
                args: tacky_args,
                dst: tmp.clone(),
                stack_arg_indices: shifted_stack,
                struct_arg_groups: shifted_groups,
                variadic,
                fixed_flat_arg_count: fixed_flat_arg_count + 1,
                indirect: is_indirect,
            });
            if let Some(pi) = ret_pi {
                if let TackyVal::Var(ref dst_name) = tmp {
                    self.ptr_info.insert(dst_name.clone(), pi);
                }
            }
            return Ok((tmp, CType::Struct));
        }

        let dst = if let Some(ref rft) = ret_ft {
            let tmp = self.fresh_tmp_full(rft);
            if let FullType::Struct(ref tag) = rft {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    if let Some(def) = self.struct_defs.get(tag) {
                        let classes = def.classify_with(&self.struct_defs);
                        let alloc_size = std::cmp::max(def.size, classes.len() * 8);
                        self.array_sizes.insert(tmp_name.clone(), alloc_size);
                    }
                }
            }
            tmp
        } else {
            self.fresh_tmp(ret_type)
        };
        self.emit(TackyInstr::FunCall {
            name: call_name,
            args: tacky_args,
            dst: dst.clone(),
            stack_arg_indices,
            struct_arg_groups,
            variadic,
            fixed_flat_arg_count,
            indirect: is_indirect,
        });
        if let Some(pi) = ret_pi {
            if let TackyVal::Var(ref dst_name) = dst {
                self.ptr_info.insert(dst_name.clone(), pi);
            }
        }
        Ok((dst, ret_type))
    }

    fn emit_indirect_call(
        &mut self,
        callee: Exp,
        args: Vec<Exp>,
    ) -> TackyResult<(TackyVal, CType)> {
        let callee_ft = self.typeof_exp(&callee);
        let pointer_sig = Self::function_signature_from_full(&callee_ft);
        let (ret_ft, param_types, variadic) = pointer_sig
            .as_ref()
            .map(|(ret_ft, param_fts, variadic)| {
                (
                    ret_ft.clone(),
                    param_fts.iter().map(FullType::to_ctype).collect::<Vec<_>>(),
                    *variadic,
                )
            })
            .unwrap_or((FullType::Scalar(CType::Int), Vec::new(), false));
        let has_prototype = pointer_sig.is_some();
        if has_prototype && !variadic && args.len() != param_types.len() {
            return Err(format!(
                "function pointer called with {} argument(s), but prototype expects {}",
                args.len(),
                param_types.len()
            ));
        }
        if has_prototype && variadic && args.len() < param_types.len() {
            return Err(format!(
                "variadic function pointer called with {} argument(s), but prototype requires at least {}",
                args.len(),
                param_types.len()
            ));
        }
        let (ptr_val, _ptr_type) = self.emit_exp(callee)?;
        let ptr_name = if let TackyVal::Var(ref n) = ptr_val {
            n.clone()
        } else {
            let tmp = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Copy {
                src: ptr_val,
                dst: tmp.clone(),
            });
            match tmp {
                TackyVal::Var(n) => n,
                _ => return Err("indirect call callee did not lower to a pointer".to_string()),
            }
        };

        let mut tacky_args = Vec::new();
        let mut stack_arg_indices = std::collections::HashSet::new();
        let mut struct_arg_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut fixed_flat_arg_count = 0usize;
        for (i, arg) in args.into_iter().enumerate() {
            let arg_for_type = arg.clone();
            let (val, val_type) = self.emit_exp(arg)?;
            let val_ft = self.val_full_type(&val);
            let expected = param_types
                .get(i)
                .copied()
                .unwrap_or_else(|| val_type.promote());
            if let Some((_, param_fts, _)) = pointer_sig.as_ref() {
                if let Some(expected_ft) = param_fts.get(i) {
                    self.assert_assignable_exp_full_type(
                        expected_ft,
                        &val_ft,
                        &arg_for_type,
                        "function pointer call",
                    )?;
                }
            }

            let is_struct_arg = val_ft.is_struct() || val_type == CType::Struct;
            if is_struct_arg || expected == CType::Struct {
                let tag = match &val_ft {
                    FullType::Struct(t) => t.clone(),
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => {
                            tacky_args.push(val);
                            if i + 1 == param_types.len() {
                                fixed_flat_arg_count = tacky_args.len();
                            }
                            continue;
                        }
                    },
                    _ => {
                        tacky_args.push(val);
                        if i + 1 == param_types.len() {
                            fixed_flat_arg_count = tacky_args.len();
                        }
                        continue;
                    }
                };

                if let Some(def) = self.struct_defs.get(&tag).cloned() {
                    let classes = def.classify_with(&self.struct_defs);
                    if classes.len() == 1 && classes[0] == ParamClass::Memory {
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let num_eightbytes = def.size.div_ceil(8);
                        let start_idx = tacky_args.len();
                        for eb in 0..num_eightbytes {
                            let eb_offset = (eb * 8) as i64;
                            let tmp = self.fresh_tmp(CType::Long);
                            stack_arg_indices.insert(start_idx + eb);
                            let ptr = self.fresh_tmp(CType::Pointer);
                            if eb_offset > 0 {
                                self.emit(TackyInstr::Binary {
                                    op: TackyBinaryOp::Add,
                                    left: struct_addr.clone(),
                                    right: TackyVal::Constant(eb_offset),
                                    dst: ptr.clone(),
                                });
                            } else {
                                self.emit(TackyInstr::Copy {
                                    src: struct_addr.clone(),
                                    dst: ptr.clone(),
                                });
                            }
                            self.emit(TackyInstr::Load {
                                src_ptr: ptr,
                                dst: tmp.clone(),
                            });
                            tacky_args.push(tmp);
                        }
                    } else {
                        let struct_var_name = if val_ft.is_struct() {
                            if let TackyVal::Var(ref n) = val {
                                Some(n.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let group_start = tacky_args.len();
                        let is_sse_vec: Vec<bool> =
                            classes.iter().map(|c| *c == ParamClass::Sse).collect();
                        struct_arg_groups.push((group_start, classes.len(), is_sse_vec));
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i64;
                            match class {
                                ParamClass::Sse => {
                                    let tmp = self.fresh_tmp(CType::Double);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                ParamClass::Integer => {
                                    let tmp = self.fresh_tmp(CType::Long);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                _ => tacky_args.push(TackyVal::Constant(0)),
                            }
                        }
                    }
                } else {
                    tacky_args.push(self.convert_to(val, val_type, expected));
                }
            } else {
                tacky_args.push(self.convert_to(val, val_type, expected));
            }

            if i + 1 == param_types.len() {
                fixed_flat_arg_count = tacky_args.len();
            }
        }
        if param_types.is_empty() {
            fixed_flat_arg_count = 0;
        }
        let ret_type = ret_ft.to_ctype();
        let uses_hidden_ptr = if let FullType::Struct(ref tag) = ret_ft {
            self.struct_defs
                .get(tag)
                .map(|def| def.size > 16)
                .unwrap_or(false)
        } else {
            false
        };
        if uses_hidden_ptr {
            let dst = self.fresh_tmp_full(&ret_ft);
            if let FullType::Struct(ref tag) = ret_ft {
                if let TackyVal::Var(ref dst_name) = dst {
                    if let Some(def) = self.struct_defs.get(tag) {
                        self.array_sizes.insert(dst_name.clone(), def.size);
                    }
                }
            }
            let ret_addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: dst.clone(),
                dst: ret_addr.clone(),
            });
            tacky_args.insert(0, ret_addr);
            let shifted_stack = stack_arg_indices.iter().map(|&i| i + 1).collect();
            let shifted_groups: Vec<(usize, usize, Vec<bool>)> = struct_arg_groups
                .iter()
                .map(|(start, count, classes)| (start + 1, *count, classes.clone()))
                .collect();
            self.emit(TackyInstr::FunCall {
                name: ptr_name,
                fixed_flat_arg_count: fixed_flat_arg_count + 1,
                args: tacky_args,
                dst: dst.clone(),
                stack_arg_indices: shifted_stack,
                struct_arg_groups: shifted_groups,
                variadic,
                indirect: true,
            });
            return Ok((dst, CType::Struct));
        }

        let dst = self.fresh_tmp_full(&ret_ft);
        if let FullType::Struct(ref tag) = ret_ft {
            if let TackyVal::Var(ref dst_name) = dst {
                if let Some(def) = self.struct_defs.get(tag) {
                    let classes = def.classify_with(&self.struct_defs);
                    let alloc_size = std::cmp::max(def.size, classes.len() * 8);
                    self.array_sizes.insert(dst_name.clone(), alloc_size);
                }
            }
        }
        self.emit(TackyInstr::FunCall {
            name: ptr_name,
            fixed_flat_arg_count,
            args: tacky_args,
            dst: dst.clone(),
            stack_arg_indices,
            struct_arg_groups,
            variadic,
            indirect: true,
        });
        Ok((dst, ret_type))
    }

    fn emit_addr_of(&mut self, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
            return self.emit_exp(*ptr_exp);
        }

        if let Exp::StringLiteral(s) = inner {
            let label = self.make_string_constant(&s);
            let str_size = c_string_byte_len(&s) + 1;
            let str_ft = FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size: str_size,
            };
            let addr_ft = FullType::Pointer(Box::new(str_ft));
            let dst = self.fresh_tmp_full(&addr_ft);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(label),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }

        if matches!(inner, Exp::Dot(_, _) | Exp::Arrow(_, _)) {
            let pointee_ft = self.typeof_exp(&inner);
            let addr = self.emit_dot_address(&inner)?;
            let addr_ft = FullType::Pointer(Box::new(pointee_ft));
            let dst = self.fresh_tmp_full(&addr_ft);
            self.emit(TackyInstr::Copy {
                src: addr,
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }

        if let Exp::Subscript(first, second) = inner {
            let (ptr, _elem_type, _elem_ft) = self.emit_subscript_addr(*first, *second)?;
            return Ok((ptr, CType::Pointer));
        }

        if let Exp::Var(ref name) = inner {
            if self.func_types.contains_key(name) {
                let dst = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(name.clone()),
                    dst: dst.clone(),
                });
                self.extern_vars.push(name.clone());
                return Ok((dst, CType::Pointer));
            }
        }

        let pointee_type = self.lvalue_type(&inner);
        let var = self.emit_lvalue(inner)?;
        let var_ft = self.val_full_type(&var);
        let addr_ft = FullType::Pointer(Box::new(var_ft));
        let dst = self.fresh_tmp_full(&addr_ft);
        if let TackyVal::Var(ref dst_name) = dst {
            let info = match &var {
                TackyVal::Var(vname) => {
                    if let Some(&(base, depth)) = self.ptr_info.get(vname) {
                        (base, depth + 1)
                    } else {
                        (pointee_type, 1)
                    }
                }
                _ => (pointee_type, 1),
            };
            self.ptr_info.insert(dst_name.clone(), info);
        }
        self.emit(TackyInstr::GetAddress {
            src: var,
            dst: dst.clone(),
        });
        Ok((dst, CType::Pointer))
    }

    fn emit_deref(&mut self, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        let (ptr, _) = self.emit_exp(inner)?;
        let ptr_full = self.val_full_type(&ptr);
        if let FullType::Pointer(ref inner_ft) = ptr_full {
            if matches!(inner_ft.as_ref(), FullType::Function { .. }) {
                return Ok((ptr, CType::Pointer));
            }
            if inner_ft.is_array() {
                let decayed = inner_ft.decay();
                let result = self.fresh_tmp_full(&decayed);
                self.emit(TackyInstr::Copy {
                    src: ptr,
                    dst: result.clone(),
                });
                return Ok((result, decayed.to_ctype()));
            }
            if inner_ft.is_struct() {
                let ptr_ft = FullType::Pointer(Box::new(inner_ft.as_ref().clone()));
                let result = self.fresh_tmp_full(&ptr_ft);
                self.emit(TackyInstr::Copy {
                    src: ptr,
                    dst: result.clone(),
                });
                return Ok((result, CType::Pointer));
            }
        }
        let pointee_type = if let TackyVal::Var(ref name) = ptr {
            self.deref_type(name)
        } else {
            CType::Int
        };
        let pointee_full = if let FullType::Pointer(ref inner_ft) = ptr_full {
            inner_ft.as_ref().clone()
        } else {
            FullType::Scalar(pointee_type)
        };
        let dst = self.fresh_tmp_full(&pointee_full);
        self.emit(TackyInstr::Load {
            src_ptr: ptr.clone(),
            dst: dst.clone(),
        });
        if pointee_type == CType::Pointer {
            if let TackyVal::Var(ref ptr_name) = ptr {
                if let Some(info) = self.deref_info(ptr_name) {
                    if let TackyVal::Var(ref dst_name) = dst {
                        self.ptr_info.insert(dst_name.clone(), info);
                    }
                }
            }
        }
        Ok((dst, pointee_type))
    }

    fn emit_subscript(&mut self, arr: Exp, idx: Exp) -> TackyResult<(TackyVal, CType)> {
        let (ptr, _elem_type, elem_full) = self.emit_subscript_addr(arr, idx)?;

        if elem_full.is_array() {
            let decayed = elem_full.decay();
            let decayed_ptr = self.fresh_tmp_full(&decayed);
            self.emit(TackyInstr::Copy {
                src: ptr,
                dst: decayed_ptr.clone(),
            });
            return Ok((decayed_ptr, decayed.to_ctype()));
        }

        if elem_full.is_struct() {
            let ptr_ft = FullType::Pointer(Box::new(elem_full));
            let result = self.fresh_tmp_full(&ptr_ft);
            self.emit(TackyInstr::Copy {
                src: ptr,
                dst: result.clone(),
            });
            return Ok((result, CType::Pointer));
        }

        let elem_ctype = elem_full.to_ctype();
        let result = self.fresh_tmp_full(&elem_full);
        self.emit(TackyInstr::Load {
            src_ptr: ptr,
            dst: result.clone(),
        });
        Ok((result, elem_ctype))
    }

    fn emit_conditional(
        &mut self,
        cond: Exp,
        then_exp: Exp,
        else_exp: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let (cond_val, _) = self.emit_exp(cond)?;
        let else_label = self.fresh_label("cond_else");
        let end_label = self.fresh_label("cond_end");
        self.emit(TackyInstr::JumpIfZero(cond_val, else_label.clone()));
        let (then_val, then_type) = self.emit_exp(then_exp)?;

        if then_type == CType::Void {
            self.emit(TackyInstr::Jump(end_label.clone()));
            self.emit(TackyInstr::Label(else_label));
            self.emit_exp(else_exp)?;
            self.emit(TackyInstr::Label(end_label));
            return Ok((TackyVal::Constant(0), CType::Void));
        }

        if then_type == CType::Struct {
            let then_ft = self.val_full_type(&then_val);
            let tag = match &then_ft {
                FullType::Struct(t) => t.clone(),
                FullType::Pointer(inner) => match inner.as_ref() {
                    FullType::Struct(t) => t.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            let struct_size = self.struct_defs.get(&tag).map(|d| d.size).unwrap_or(0);
            let result = self.fresh_tmp_full(&FullType::Struct(tag.clone()));
            if let TackyVal::Var(ref rn) = result {
                self.array_sizes.insert(rn.clone(), struct_size);
            }
            let then_addr = if then_ft.is_struct() {
                if let TackyVal::Var(ref n) = then_val {
                    if self.array_sizes.contains_key(n) {
                        let a = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: then_val,
                            dst: a.clone(),
                        });
                        a
                    } else {
                        then_val
                    }
                } else {
                    then_val
                }
            } else {
                then_val
            };
            if let TackyVal::Var(ref rn) = result {
                self.emit_struct_copy_to(then_addr, rn, struct_size);
            }
            self.emit(TackyInstr::Jump(end_label.clone()));
            self.emit(TackyInstr::Label(else_label));
            let (else_val, _) = self.emit_exp(else_exp)?;
            let else_ft = self.val_full_type(&else_val);
            let else_addr = if else_ft.is_struct() {
                if let TackyVal::Var(ref n) = else_val {
                    if self.array_sizes.contains_key(n) {
                        let a = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: else_val,
                            dst: a.clone(),
                        });
                        a
                    } else {
                        else_val
                    }
                } else {
                    else_val
                }
            } else {
                else_val
            };
            if let TackyVal::Var(ref rn) = result {
                self.emit_struct_copy_to(else_addr, rn, struct_size);
            }
            self.emit(TackyInstr::Label(end_label));
            return Ok((result, CType::Struct));
        }

        let then_tmp = self.fresh_tmp(then_type);
        self.emit(TackyInstr::Copy {
            src: then_val,
            dst: then_tmp.clone(),
        });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(else_label));
        let (else_val, else_type) = self.emit_exp(else_exp)?;
        let common = CType::common(then_type, else_type);
        let result = self.fresh_tmp(common);
        let else_conv = self.convert_to(else_val, else_type, common);
        self.emit(TackyInstr::Copy {
            src: else_conv,
            dst: result.clone(),
        });
        let end2_label = self.fresh_label("cond_end2");
        self.emit(TackyInstr::Jump(end2_label.clone()));
        self.emit(TackyInstr::Label(end_label));
        let then_conv = self.convert_to(then_tmp, then_type, common);
        self.emit(TackyInstr::Copy {
            src: then_conv,
            dst: result.clone(),
        });
        self.emit(TackyInstr::Label(end2_label));
        Ok((result, common))
    }

    fn emit_scalar_unary(&mut self, op: UnaryOp, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        let (src, src_type) = self.emit_exp(inner)?;
        if src_type.is_floating() && matches!(op, UnaryOp::Negate) {
            let dst = self.fresh_tmp(src_type);
            self.emit(TackyInstr::Unary {
                op: TackyUnaryOp::Negate,
                src,
                dst: dst.clone(),
            });
            return Ok((dst, src_type));
        }

        let promoted = src_type.promote();
        let src_conv = self.convert_to(src, src_type, promoted);
        let dst = self.fresh_tmp(promoted);
        let tacky_op = match op {
            UnaryOp::Negate => TackyUnaryOp::Negate,
            UnaryOp::Complement => TackyUnaryOp::Complement,
            _ => return Err(format!("invalid scalar unary operator: {:?}", op)),
        };
        self.emit(TackyInstr::Unary {
            op: tacky_op,
            src: src_conv,
            dst: dst.clone(),
        });
        Ok((dst, promoted))
    }

    fn lvalue_type(&self, exp: &Exp) -> CType {
        match exp {
            Exp::Var(name) => self
                .var_types
                .get(name)
                .copied()
                .or_else(|| self.symbol_types.get(name).copied())
                .unwrap_or(CType::Int),
            Exp::Unary(UnaryOp::Deref, inner) => {
                if let Exp::Var(name) = inner.as_ref() {
                    self.deref_type(name)
                } else {
                    CType::Int
                }
            }
            Exp::Subscript(arr, _) => {
                if let Exp::Var(name) = arr.as_ref() {
                    self.deref_type(name)
                } else {
                    CType::Int
                }
            }
            Exp::Dot(_, _) => self.typeof_exp(exp).to_ctype(),
            Exp::Arrow(_, _) => self.typeof_exp(exp).to_ctype(),
            _ => CType::Int,
        }
    }

    fn emit_lvalue(&self, exp: Exp) -> TackyResult<TackyVal> {
        match exp {
            Exp::Var(name) => Ok(TackyVal::Var(name)),
            _ => Err("Expression is not a simple lvalue".to_string()),
        }
    }

    fn scalar_lvalue_address(
        &mut self,
        exp: Exp,
    ) -> TackyResult<Option<(TackyVal, CType, FullType)>> {
        match exp {
            Exp::Subscript(arr, idx) => {
                let (ptr, elem_type, elem_ft) = self.emit_subscript_addr(*arr, *idx)?;
                Ok(Some((ptr, elem_type, elem_ft)))
            }
            Exp::Unary(UnaryOp::Deref, ptr_exp) => {
                let (ptr, _) = self.emit_exp(*ptr_exp)?;
                let ptr_ft = self.val_full_type(&ptr);
                let elem_ft = match ptr_ft {
                    FullType::Pointer(inner) => *inner,
                    _ => FullType::Scalar(CType::Int),
                };
                Ok(Some((ptr, elem_ft.to_ctype(), elem_ft)))
            }
            Exp::Dot(_, _) | Exp::Arrow(_, _) => {
                let ft = self.typeof_exp(&exp);
                let ptr = self.emit_dot_address(&exp)?;
                Ok(Some((ptr, ft.to_ctype(), ft)))
            }
            _ => Ok(None),
        }
    }

    fn emit_inc_dec(&mut self, op: UnaryOp, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        let is_pre = matches!(op, UnaryOp::PreIncrement | UnaryOp::PreDecrement);
        let binop = if matches!(op, UnaryOp::PreIncrement | UnaryOp::PostIncrement) {
            TackyBinaryOp::Add
        } else {
            TackyBinaryOp::Sub
        };

        if let Exp::Subscript(arr, idx) = inner {
            let (ptr, pt, _pt_ft) = self.emit_subscript_addr(*arr, *idx)?;
            let current = self.fresh_tmp(pt);
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let one = self.convert_to(TackyVal::Constant(1), CType::Int, pt);
            let result = self.fresh_tmp(pt);
            self.emit(TackyInstr::Binary {
                op: binop,
                left: current.clone(),
                right: one,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Store {
                src: result.clone(),
                dst_ptr: ptr,
            });
            return Ok((if is_pre { result } else { current }, pt));
        }

        if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
            let (ptr, _) = self.emit_exp(*ptr_exp)?;
            let pt = if let TackyVal::Var(ref n) = ptr {
                self.deref_type(n)
            } else {
                CType::Int
            };
            let current = self.fresh_tmp(pt);
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let one = self.convert_to(TackyVal::Constant(1), CType::Int, pt);
            let result = self.fresh_tmp(pt);
            self.emit(TackyInstr::Binary {
                op: binop,
                left: current.clone(),
                right: one,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Store {
                src: result.clone(),
                dst_ptr: ptr,
            });
            return Ok((if is_pre { result } else { current }, pt));
        }

        if let Some((ptr, var_type, var_ft)) = self.scalar_lvalue_address(inner.clone())? {
            let current = self.fresh_tmp_full(&var_ft);
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let increment = if var_type == CType::Pointer {
                let elem_size = match &var_ft {
                    FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                    _ => 1,
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, var_type)
            };
            let result = self.fresh_tmp_full(&var_ft);
            self.emit(TackyInstr::Binary {
                op: binop,
                left: current.clone(),
                right: increment,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Store {
                src: result.clone(),
                dst_ptr: ptr,
            });
            return Ok((if is_pre { result } else { current }, var_type));
        }

        let var_type = self.lvalue_type(&inner);
        let var = self.emit_lvalue(inner)?;
        let var_ft = self.val_full_type(&var);

        if is_pre {
            let increment = if var_type == CType::Pointer {
                let elem_size = if let TackyVal::Var(ref n) = var {
                    self.ptr_elem_size(n)
                } else {
                    1
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, var_type)
            };
            let dst = if var_type == CType::Pointer {
                self.fresh_tmp_full(&var_ft)
            } else {
                self.fresh_tmp(var_type)
            };
            self.emit(TackyInstr::Binary {
                op: binop,
                left: var.clone(),
                right: increment,
                dst: dst.clone(),
            });
            if var_type == CType::Pointer {
                if let TackyVal::Var(ref vn) = var {
                    if let Some(&info) = self.ptr_info.get(vn) {
                        if let TackyVal::Var(ref dn) = dst {
                            self.ptr_info.insert(dn.clone(), info);
                        }
                    }
                }
            }
            self.emit(TackyInstr::Copy {
                src: dst.clone(),
                dst: var,
            });
            return Ok((dst, var_type));
        }

        let old_val = if var_type == CType::Pointer {
            self.fresh_tmp_full(&var_ft)
        } else {
            self.fresh_tmp(var_type)
        };
        self.emit(TackyInstr::Copy {
            src: var.clone(),
            dst: old_val.clone(),
        });
        let increment = if var_type == CType::Pointer {
            let elem_size = if let TackyVal::Var(ref n) = var {
                self.ptr_elem_size(n)
            } else {
                1
            };
            TackyVal::Constant(elem_size)
        } else {
            self.convert_to(TackyVal::Constant(1), CType::Int, var_type)
        };
        let new_val = self.fresh_tmp(var_type);
        self.emit(TackyInstr::Binary {
            op: binop,
            left: var.clone(),
            right: increment,
            dst: new_val.clone(),
        });
        self.emit(TackyInstr::Copy {
            src: new_val,
            dst: var,
        });
        Ok((old_val, var_type))
    }

    /// Compute the address of a Dot/Arrow lvalue expression
    fn emit_dot_address(&mut self, exp: &Exp) -> TackyResult<TackyVal> {
        match exp {
            Exp::Dot(inner, member) => {
                let base_addr = if let Exp::Var(n) = inner.as_ref() {
                    let addr = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::GetAddress {
                        src: TackyVal::Var(n.clone()),
                        dst: addr.clone(),
                    });
                    addr
                } else if let Exp::Dot(_, _) = inner.as_ref() {
                    self.emit_dot_address(inner)?
                } else if let Exp::Arrow(ptr_exp, mem) = inner.as_ref() {
                    let (ptr, _) = self.emit_exp((**ptr_exp).clone())?;
                    let ptr_ft = self.val_full_type(&ptr);
                    let tag = match &ptr_ft {
                        FullType::Pointer(inner) => match inner.as_ref() {
                            FullType::Struct(t) => t.clone(),
                            _ => {
                                return Err(format!(
                                    "emit_dot_address Arrow: inner is {:?}, expected Struct",
                                    inner
                                ))
                            }
                        },
                        _ => {
                            return Err(format!(
                                "emit_dot_address Arrow: ft is {:?}, expected Pointer",
                                ptr_ft
                            ))
                        }
                    };
                    let m = self.struct_member(&tag, mem)?;
                    let result = self.fresh_tmp(CType::Pointer);
                    if m.offset > 0 {
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Add,
                            left: ptr,
                            right: TackyVal::Constant(m.offset as i64),
                            dst: result.clone(),
                        });
                    } else {
                        self.emit(TackyInstr::Copy {
                            src: ptr,
                            dst: result.clone(),
                        });
                    }
                    result
                } else {
                    let (val, _) = self.emit_exp((**inner).clone())?;
                    let val_ft = self.val_full_type(&val);
                    match &val_ft {
                        FullType::Struct(_) => {
                            let addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: addr.clone(),
                            });
                            addr
                        }
                        FullType::Pointer(_) => val,
                        _ => {
                            let addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: addr.clone(),
                            });
                            addr
                        }
                    }
                };
                // Get the struct tag using typeof_exp
                let tag = self.dot_inner_tag(inner)?;
                let mem = self.struct_member(&tag, member)?;
                let result = self.fresh_tmp(CType::Pointer);
                if mem.offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: base_addr,
                        right: TackyVal::Constant(mem.offset as i64),
                        dst: result.clone(),
                    });
                } else {
                    self.emit(TackyInstr::Copy {
                        src: base_addr,
                        dst: result.clone(),
                    });
                }
                Ok(result)
            }
            Exp::Arrow(inner, member) => {
                let (ptr, _) = self.emit_exp((**inner).clone())?;
                let ptr_ft = self.val_full_type(&ptr);
                // Try to get struct tag from FullType; fall back to looking up ptr_info
                let tag = match &ptr_ft {
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => {
                            // Fallback: try ptr_info
                            if let TackyVal::Var(ref name) = ptr {
                                if let Some(&(base_t, _)) = self.ptr_info.get(name) {
                                    if base_t == CType::Struct {
                                        // Can't determine tag from ptr_info alone
                                    }
                                }
                            }
                            return Err(format!(
                                "emit_dot_address Arrow: inner is {:?}, expected Struct",
                                inner
                            ));
                        }
                    },
                    _ => {
                        return Err(format!(
                            "emit_dot_address Arrow: ft is {:?}, expected Pointer",
                            ptr_ft
                        ))
                    }
                };
                let mem = self.struct_member(&tag, member)?;
                let result = self.fresh_tmp(CType::Pointer);
                if mem.offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ptr,
                        right: TackyVal::Constant(mem.offset as i64),
                        dst: result.clone(),
                    });
                } else {
                    self.emit(TackyInstr::Copy {
                        src: ptr,
                        dst: result.clone(),
                    });
                }
                Ok(result)
            }
            _ => Err("emit_dot_address called on non-Dot/Arrow expression".to_string()),
        }
    }

    fn try_dot_inner_tag(&self, exp: &Exp) -> Option<String> {
        match exp {
            Exp::Var(n) => {
                let ft = self.get_full_type(n);
                match ft {
                    FullType::Struct(t) => Some(t),
                    FullType::Pointer(inner) => match *inner {
                        FullType::Struct(t) => Some(t),
                        _ => None,
                    },
                    _ => None,
                }
            }
            Exp::Dot(inner, member) => {
                let parent_tag = self.try_dot_inner_tag(inner)?;
                let def = self.struct_defs.get(&parent_tag)?;
                let mem = def.find_member(member)?;
                match &mem.member_full_type {
                    FullType::Struct(t) => Some(t.clone()),
                    _ => None,
                }
            }
            Exp::Arrow(inner, member) => {
                if let Exp::Var(n) = inner.as_ref() {
                    let ft = self.get_full_type(n);
                    if let FullType::Pointer(inner_ft) = ft {
                        if let FullType::Struct(t) = inner_ft.as_ref() {
                            let def = self.struct_defs.get(t)?;
                            let mem = def.find_member(member)?;
                            match &mem.member_full_type {
                                FullType::Struct(t) => Some(t.clone()),
                                _ => None,
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn dot_inner_tag(&self, exp: &Exp) -> TackyResult<String> {
        let ft = self.typeof_exp(exp);
        match ft {
            FullType::Struct(t) => Ok(t),
            FullType::Pointer(inner) => match *inner {
                FullType::Struct(t) => Ok(t),
                _ => Err(format!("dot_inner_tag: pointer to non-struct: {:?}", inner)),
            },
            _ => Err(format!(
                "dot_inner_tag: non-struct type {:?} for {:?}",
                ft, exp
            )),
        }
    }

    fn dot_member_full_type(&self, exp: &Exp) -> FullType {
        self.typeof_exp(exp)
    }

    fn struct_member(&self, tag: &str, member: &str) -> TackyResult<StructMember> {
        let def = self
            .struct_defs
            .get(tag)
            .cloned()
            .ok_or_else(|| format!("Undefined struct: {}", tag))?;
        def.find_member(member)
            .cloned()
            .ok_or_else(|| format!("No member '{}' in struct {}", member, tag))
    }

    fn bit_mask(width: u8) -> i64 {
        if width >= 63 {
            i64::MAX
        } else {
            (1_i64 << width) - 1
        }
    }

    fn sign_extend_bit_field_value(
        &mut self,
        value: TackyVal,
        mem: &StructMember,
        width: u8,
    ) -> TackyVal {
        if !mem.member_type.is_signed() || width == 0 || width as i32 >= mem.member_type.size() * 8
        {
            return value;
        }
        let sign_bit = 1_i64 << (width - 1);
        let xored = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseXor,
            left: value,
            right: TackyVal::Constant(sign_bit),
            dst: xored.clone(),
        });
        let extended = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::Sub,
            left: xored,
            right: TackyVal::Constant(sign_bit),
            dst: extended.clone(),
        });
        extended
    }

    fn extract_bit_field(&mut self, unit: TackyVal, mem: &StructMember) -> TackyResult<TackyVal> {
        let Some(width) = mem.bit_width else {
            return Ok(unit);
        };
        let mut value = unit;
        if mem.bit_offset > 0 {
            let shifted = self.fresh_tmp(mem.member_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftRight,
                left: value,
                right: TackyVal::Constant(mem.bit_offset as i64),
                dst: shifted.clone(),
            });
            value = shifted;
        }
        if width as i32 == mem.member_type.size() * 8 {
            return Ok(value);
        }
        let masked = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: value,
            right: TackyVal::Constant(Self::bit_mask(width)),
            dst: masked.clone(),
        });
        Ok(self.sign_extend_bit_field_value(masked, mem, width))
    }

    fn store_bit_field_to_offset(
        &mut self,
        dst_name: String,
        mem: &StructMember,
        rhs: TackyVal,
    ) -> TackyResult<TackyVal> {
        let Some(width) = mem.bit_width else {
            return Err("store_bit_field_to_offset called for non-bit-field".to_string());
        };
        let unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::CopyFromOffset {
            src_name: dst_name.clone(),
            offset: mem.offset as i64,
            dst: unit.clone(),
        });
        let rhs_masked = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: rhs,
            right: TackyVal::Constant(Self::bit_mask(width)),
            dst: rhs_masked.clone(),
        });
        let inserted = if mem.bit_offset > 0 {
            let shifted = self.fresh_tmp(mem.member_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: rhs_masked.clone(),
                right: TackyVal::Constant(mem.bit_offset as i64),
                dst: shifted.clone(),
            });
            shifted
        } else {
            rhs_masked.clone()
        };
        let field_mask = Self::bit_mask(width) << mem.bit_offset;
        let cleared = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: unit,
            right: TackyVal::Constant(!field_mask),
            dst: cleared.clone(),
        });
        let new_unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left: cleared,
            right: inserted,
            dst: new_unit.clone(),
        });
        self.emit(TackyInstr::CopyToOffset {
            src: new_unit,
            dst_name,
            offset: mem.offset as i64,
        });
        Ok(self.sign_extend_bit_field_value(rhs_masked, mem, width))
    }

    fn store_bit_field_to_ptr(
        &mut self,
        dst_ptr: TackyVal,
        mem: &StructMember,
        rhs: TackyVal,
    ) -> TackyResult<TackyVal> {
        let Some(width) = mem.bit_width else {
            return Err("store_bit_field_to_ptr called for non-bit-field".to_string());
        };
        let unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Load {
            src_ptr: dst_ptr.clone(),
            dst: unit.clone(),
        });
        let rhs_masked = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: rhs,
            right: TackyVal::Constant(Self::bit_mask(width)),
            dst: rhs_masked.clone(),
        });
        let inserted = if mem.bit_offset > 0 {
            let shifted = self.fresh_tmp(mem.member_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: rhs_masked.clone(),
                right: TackyVal::Constant(mem.bit_offset as i64),
                dst: shifted.clone(),
            });
            shifted
        } else {
            rhs_masked.clone()
        };
        let field_mask = Self::bit_mask(width) << mem.bit_offset;
        let cleared = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: unit,
            right: TackyVal::Constant(!field_mask),
            dst: cleared.clone(),
        });
        let new_unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left: cleared,
            right: inserted,
            dst: new_unit.clone(),
        });
        self.emit(TackyInstr::Store {
            src: new_unit,
            dst_ptr,
        });
        Ok(self.sign_extend_bit_field_value(rhs_masked, mem, width))
    }

    fn access_struct_member(
        &mut self,
        struct_addr: TackyVal,
        tag: String,
        member: &str,
    ) -> TackyResult<(TackyVal, CType)> {
        let mem = self.struct_member(&tag, member)?;
        let mem_type = mem.member_type;
        let mem_offset = mem.offset;
        let mem_ft = mem.member_full_type.clone();

        let mem_ptr = self.fresh_tmp(CType::Pointer);
        if mem_offset > 0 {
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: struct_addr,
                right: TackyVal::Constant(mem_offset as i64),
                dst: mem_ptr.clone(),
            });
        } else {
            self.emit(TackyInstr::Copy {
                src: struct_addr,
                dst: mem_ptr.clone(),
            });
        }

        if mem_ft.is_array() {
            // Array member: return pointer (decayed)
            let result_ft = FullType::Pointer(Box::new(match &mem_ft {
                FullType::Array { elem, .. } => *elem.clone(),
                _ => mem_ft.clone(),
            }));
            let result = self.fresh_tmp_full(&result_ft);
            self.emit(TackyInstr::Copy {
                src: mem_ptr,
                dst: result.clone(),
            });
            Ok((result, CType::Pointer))
        } else if mem_ft.is_struct() {
            // Struct member: return pointer to it (not loaded)
            let result_ft = FullType::Pointer(Box::new(mem_ft));
            let result = self.fresh_tmp_full(&result_ft);
            self.emit(TackyInstr::Copy {
                src: mem_ptr,
                dst: result.clone(),
            });
            Ok((result, CType::Pointer))
        } else {
            // Scalar member: load the value
            let result = self.fresh_tmp_full(&mem_ft);
            self.emit(TackyInstr::Load {
                src_ptr: mem_ptr,
                dst: result.clone(),
            });
            let result = self.extract_bit_field(result, &mem)?;
            Ok((result, mem_type))
        }
    }

    /// Get the address of a struct value, handling deref temps correctly
    fn get_struct_addr(&mut self, val: TackyVal) -> TackyVal {
        if let TackyVal::Var(ref n) = val {
            if self.array_sizes.contains_key(n) {
                // Proper struct variable — take its address
                let a = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: val,
                    dst: a.clone(),
                });
                return a;
            }
        }
        // Deref temp or pointer — use directly
        val
    }

    /// Emit a word-by-word struct copy from src address to dst name
    fn emit_struct_copy_to(&mut self, src_addr: TackyVal, dst_name: &str, struct_size: usize) {
        let mut off = 0usize;
        while off + 8 <= struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Long);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: tmp.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: tmp,
                dst_name: dst_name.to_string(),
                offset: off as i64,
            });
            off += 8;
        }
        while off + 4 <= struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: tmp.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: tmp,
                dst_name: dst_name.to_string(),
                offset: off as i64,
            });
            off += 4;
        }
        while off < struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Char);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: tmp.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: tmp,
                dst_name: dst_name.to_string(),
                offset: off as i64,
            });
            off += 1;
        }
    }

    /// Emit struct copy from src address to dst address (both pointers)
    fn emit_struct_copy_ptr_to_ptr(
        &mut self,
        src_addr: TackyVal,
        dst_addr: TackyVal,
        struct_size: usize,
    ) {
        let mut off = 0usize;
        while off + 8 <= struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: src_ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Long);
            self.emit(TackyInstr::Load {
                src_ptr,
                dst: tmp.clone(),
            });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: dst_ptr.clone(),
            });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr });
            off += 8;
        }
        while off + 4 <= struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: src_ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Load {
                src_ptr,
                dst: tmp.clone(),
            });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: dst_ptr.clone(),
            });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr });
            off += 4;
        }
        while off < struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: src_ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Char);
            self.emit(TackyInstr::Load {
                src_ptr,
                dst: tmp.clone(),
            });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: dst_ptr.clone(),
            });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr });
            off += 1;
        }
    }

    /// Compute the address for a subscript expression a[i].
    /// Returns (pointer_to_element, element_type)
    /// Returns (pointer_to_element, element_ctype, element_full_type)
    fn emit_subscript_addr(
        &mut self,
        arr: Exp,
        idx: Exp,
    ) -> TackyResult<(TackyVal, CType, FullType)> {
        let (first_val, first_type) = self.emit_exp(arr)?;
        let (second_val, second_type) = self.emit_exp(idx)?;

        // Normalize: pointer first, index second
        let first_full = self.val_full_type(&first_val);
        let (arr_val, idx_val, idx_type, arr_full) =
            if first_full.is_pointer() || first_type == CType::Pointer {
                (first_val, second_val, second_type, first_full)
            } else {
                let second_full = self.val_full_type(&second_val);
                (second_val, first_val, first_type, second_full)
            };
        let (elem_full, scale) = match &arr_full {
            FullType::Pointer(inner) => (
                inner.as_ref().clone(),
                inner.byte_size_with(&self.struct_defs) as i64,
            ),
            _ => {
                // Fallback to old approach
                let elem_type = if let TackyVal::Var(ref name) = arr_val {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                (FullType::Scalar(elem_type), elem_type.size() as i64)
            }
        };
        let elem_type = elem_full.to_ctype();

        let idx_long = self.convert_to(idx_val, idx_type, CType::Long);
        let result_ptr_type = FullType::Pointer(Box::new(elem_full.clone()));
        let ptr = self.fresh_tmp_full(&result_ptr_type);
        self.emit(TackyInstr::AddPtr {
            ptr: arr_val.clone(),
            index: idx_long,
            scale,
            dst: ptr.clone(),
        });

        // Propagate pointee metadata to the derived pointer value.
        if let TackyVal::Var(ref pname) = ptr {
            if let TackyVal::Var(ref aname) = arr_val {
                if let Some(info) = self.deref_info(aname) {
                    self.ptr_info.insert(pname.clone(), info);
                } else {
                    self.ptr_info.insert(pname.clone(), (elem_type, 1));
                }
            }
        }

        Ok((ptr, elem_type, elem_full))
    }

    fn emit_binary(
        &mut self,
        op: BinaryOp,
        left: Exp,
        right: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let (l, l_type) = self.emit_exp(left)?;
        let (r, r_type) = self.emit_exp(right)?;

        if matches!(op, BinaryOp::Add | BinaryOp::Sub) {
            let (is_ptr_arith, ptr_val, int_val, elem_size, int_type) =
                if l_type == CType::Pointer && !r_type.is_pointer() {
                    let es = if let TackyVal::Var(ref n) = l {
                        self.ptr_elem_size(n)
                    } else {
                        1
                    };
                    (true, l.clone(), r.clone(), es, r_type)
                } else if r_type == CType::Pointer
                    && !l_type.is_pointer()
                    && matches!(op, BinaryOp::Add)
                {
                    let es = if let TackyVal::Var(ref n) = r {
                        self.ptr_elem_size(n)
                    } else {
                        1
                    };
                    (true, r.clone(), l.clone(), es, l_type)
                } else {
                    (false, l.clone(), r.clone(), 1, r_type)
                };

            if is_ptr_arith && elem_size > 1 {
                let int_long = self.convert_to(int_val, int_type, CType::Long);
                let scaled = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Mul,
                    left: int_long,
                    right: TackyVal::Constant(elem_size),
                    dst: scaled.clone(),
                });
                let ptr_ft = self.val_full_type(&ptr_val);
                let dst = self.fresh_tmp_full(&ptr_ft);
                let tacky_op = Self::convert_binop(op.clone())?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: ptr_val.clone(),
                    right: scaled,
                    dst: dst.clone(),
                });
                if let TackyVal::Var(ref pname) = ptr_val {
                    if let Some(&info) = self.ptr_info.get(pname) {
                        if let TackyVal::Var(ref dname) = dst {
                            self.ptr_info.insert(dname.clone(), info);
                        }
                    }
                }
                return Ok((dst, CType::Pointer));
            } else if is_ptr_arith {
                let int_long = self.convert_to(int_val, int_type, CType::Long);
                let ptr_ft = self.val_full_type(&ptr_val);
                let dst = self.fresh_tmp_full(&ptr_ft);
                let tacky_op = Self::convert_binop(op)?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: ptr_val.clone(),
                    right: int_long,
                    dst: dst.clone(),
                });
                if let TackyVal::Var(ref pname) = ptr_val {
                    if let Some(&info) = self.ptr_info.get(pname) {
                        if let TackyVal::Var(ref dname) = dst {
                            self.ptr_info.insert(dname.clone(), info);
                        }
                    }
                }
                return Ok((dst, CType::Pointer));
            }

            if l_type == CType::Pointer && r_type == CType::Pointer && matches!(op, BinaryOp::Sub) {
                let raw_diff = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Sub,
                    left: l,
                    right: r,
                    dst: raw_diff.clone(),
                });
                let es = if let TackyVal::Var(ref n) = ptr_val {
                    self.ptr_elem_size(n)
                } else {
                    1
                };
                if es > 1 {
                    let result = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Div,
                        left: raw_diff,
                        right: TackyVal::Constant(es),
                        dst: result.clone(),
                    });
                    return Ok((result, CType::Long));
                }
                return Ok((raw_diff, CType::Long));
            }
        }

        let is_shift = matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
        if is_shift {
            let promoted = l_type.promote();
            let l_conv = self.convert_to(l, l_type, promoted);
            let dst = self.fresh_tmp(promoted);
            let tacky_op = Self::convert_binop(op)?;
            self.emit(TackyInstr::Binary {
                op: tacky_op,
                left: l_conv,
                right: r,
                dst: dst.clone(),
            });
            return Ok((dst, promoted));
        }

        let common = CType::common(l_type, r_type);
        let l_conv = self.convert_to(l, l_type, common);
        let r_conv = self.convert_to(r, r_type, common);
        let result_type = if is_comparison_op(&op) {
            CType::Int
        } else {
            common
        };
        let dst = self.fresh_tmp(result_type);
        let tacky_op = Self::convert_binop(op)?;
        self.emit(TackyInstr::Binary {
            op: tacky_op,
            left: l_conv,
            right: r_conv,
            dst: dst.clone(),
        });
        Ok((dst, result_type))
    }

    fn emit_logical_and(&mut self, left: Exp, right: Exp) -> TackyResult<TackyVal> {
        let false_label = self.fresh_label("and_false");
        let end_label = self.fresh_label("and_end");
        let result = self.fresh_tmp(CType::Int);
        let (l, _) = self.emit_exp(left)?;
        self.emit(TackyInstr::JumpIfZero(l, false_label.clone()));
        let (r, _) = self.emit_exp(right)?;
        self.emit(TackyInstr::JumpIfZero(r, false_label.clone()));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(false_label));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Label(end_label));
        Ok(result)
    }

    fn emit_logical_or(&mut self, left: Exp, right: Exp) -> TackyResult<TackyVal> {
        let true_label = self.fresh_label("or_true");
        let end_label = self.fresh_label("or_end");
        let result = self.fresh_tmp(CType::Int);
        let (l, _) = self.emit_exp(left)?;
        self.emit(TackyInstr::JumpIfNotZero(l, true_label.clone()));
        let (r, _) = self.emit_exp(right)?;
        self.emit(TackyInstr::JumpIfNotZero(r, true_label.clone()));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(true_label));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Label(end_label));
        Ok(result)
    }

    fn convert_binop(op: BinaryOp) -> TackyResult<TackyBinaryOp> {
        match op {
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                Err(format!("invalid scalar binary operator: {:?}", op))
            }
            BinaryOp::Add => Ok(TackyBinaryOp::Add),
            BinaryOp::Sub => Ok(TackyBinaryOp::Sub),
            BinaryOp::Mul => Ok(TackyBinaryOp::Mul),
            BinaryOp::Div => Ok(TackyBinaryOp::Div),
            BinaryOp::Mod => Ok(TackyBinaryOp::Mod),
            BinaryOp::BitwiseAnd => Ok(TackyBinaryOp::BitwiseAnd),
            BinaryOp::BitwiseNand => Ok(TackyBinaryOp::BitwiseNand),
            BinaryOp::BitwiseOr => Ok(TackyBinaryOp::BitwiseOr),
            BinaryOp::BitwiseXor => Ok(TackyBinaryOp::BitwiseXor),
            BinaryOp::ShiftLeft => Ok(TackyBinaryOp::ShiftLeft),
            BinaryOp::ShiftRight => Ok(TackyBinaryOp::ShiftRight),
            BinaryOp::Equal => Ok(TackyBinaryOp::Equal),
            BinaryOp::NotEqual => Ok(TackyBinaryOp::NotEqual),
            BinaryOp::LessThan => Ok(TackyBinaryOp::LessThan),
            BinaryOp::GreaterThan => Ok(TackyBinaryOp::GreaterThan),
            BinaryOp::LessEqual => Ok(TackyBinaryOp::LessEqual),
            BinaryOp::GreaterEqual => Ok(TackyBinaryOp::GreaterEqual),
        }
    }

    // --------------------------------------------------------
    // Statement emission
    // --------------------------------------------------------

    fn emit_statement(&mut self, stmt: Statement) -> TackyResult<()> {
        match stmt {
            Statement::Return(exp) => {
                let ret_type = self
                    .func_types
                    .get(&self.current_function)
                    .map(|(rt, _, _, _)| *rt)
                    .unwrap_or(CType::Int);
                if let Some(exp) = exp {
                    let exp_for_type = exp.clone();
                    let (val, val_type) = self.emit_exp(exp)?;
                    if ret_type == CType::Void {
                        self.emit(TackyInstr::Return(TackyVal::Constant(0)));
                    } else if let Some(ref ret_ptr) = self.hidden_ret_ptr.clone() {
                        // Large struct return via hidden pointer
                        let ret_ptr_val = TackyVal::Var(ret_ptr.clone());
                        let src_addr = if val_type == CType::Struct {
                            if let TackyVal::Var(ref name) = val {
                                if self.array_sizes.contains_key(name) {
                                    let a = self.fresh_tmp(CType::Pointer);
                                    self.emit(TackyInstr::GetAddress {
                                        src: val,
                                        dst: a.clone(),
                                    });
                                    a
                                } else {
                                    val
                                }
                            } else {
                                val
                            }
                        } else {
                            let a = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: a.clone(),
                            });
                            a
                        };
                        // Copy struct to hidden return pointer location
                        let ret_ft = self.func_full_types.get(&self.current_function).cloned();
                        let struct_size = if let Some(FullType::Struct(ref tag)) = ret_ft {
                            self.struct_defs.get(tag).map(|d| d.size).unwrap_or(0)
                        } else {
                            0
                        };
                        self.emit_struct_copy_ptr_to_ptr(
                            src_addr,
                            ret_ptr_val.clone(),
                            struct_size,
                        );
                        self.emit(TackyInstr::Return(ret_ptr_val));
                    } else {
                        let ret_ft = self
                            .func_full_types
                            .get(&self.current_function)
                            .cloned()
                            .unwrap_or(FullType::Scalar(ret_type));
                        let val_ft = self.val_full_type(&val);
                        self.assert_assignable_exp_full_type(
                            &ret_ft,
                            &val_ft,
                            &exp_for_type,
                            "return",
                        )?;
                        let val_conv = self.convert_to(val, val_type, ret_type);
                        self.emit(TackyInstr::Return(val_conv));
                    }
                } else {
                    self.emit(TackyInstr::Return(TackyVal::Constant(0)));
                }
            }
            Statement::Expression(exp) => {
                self.emit_exp(exp)?;
            }
            Statement::If(cond, then_stmt, else_stmt) => {
                let (cond_val, _) = self.emit_exp(cond)?;
                match else_stmt {
                    None => {
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::JumpIfZero(cond_val, end_label.clone()));
                        self.emit_statement(*then_stmt)?;
                        self.emit(TackyInstr::Label(end_label));
                    }
                    Some(else_s) => {
                        let else_label = self.fresh_label("if_else");
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::JumpIfZero(cond_val, else_label.clone()));
                        self.emit_statement(*then_stmt)?;
                        self.emit(TackyInstr::Jump(end_label.clone()));
                        self.emit(TackyInstr::Label(else_label));
                        self.emit_statement(*else_s)?;
                        self.emit(TackyInstr::Label(end_label));
                    }
                }
            }
            Statement::Block(block) => {
                self.emit_block(block)?;
            }
            Statement::While {
                condition,
                body,
                label,
            } => {
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                self.emit(TackyInstr::Label(continue_label.clone()));
                let (cond_val, _) = self.emit_exp(condition)?;
                self.emit(TackyInstr::JumpIfZero(cond_val, break_label.clone()));
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Jump(continue_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::DoWhile {
                body,
                condition,
                label,
            } => {
                let start_label = format!("start_{}", label);
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                self.emit(TackyInstr::Label(start_label.clone()));
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Label(continue_label));
                let (cond_val, _) = self.emit_exp(condition)?;
                self.emit(TackyInstr::JumpIfNotZero(cond_val, start_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                label,
            } => {
                let start_label = format!("start_{}", label);
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                match *init {
                    ForInit::Declaration(vd) => {
                        // Delegate to emit_var_decl which handles arrays, scalars, etc.
                        self.emit_var_decl(vd)?;
                    }
                    ForInit::Expression(Some(exp)) => {
                        self.emit_exp(exp)?;
                    }
                    ForInit::Expression(None) => {}
                }
                self.emit(TackyInstr::Label(start_label.clone()));
                if let Some(cond) = condition {
                    let (cond_val, _) = self.emit_exp(cond)?;
                    self.emit(TackyInstr::JumpIfZero(cond_val, break_label.clone()));
                }
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Label(continue_label));
                if let Some(post_exp) = post {
                    self.emit_exp(post_exp)?;
                }
                self.emit(TackyInstr::Jump(start_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::Break(label) => {
                self.emit(TackyInstr::Jump(format!("break_{}", label)));
            }
            Statement::Continue(label) => {
                self.emit(TackyInstr::Jump(format!("continue_{}", label)));
            }
            Statement::Goto(label) => {
                self.emit(TackyInstr::Jump(format!(
                    "label.{}.{}",
                    self.current_function, label
                )));
            }
            Statement::Label(name, body) => {
                self.emit(TackyInstr::Label(format!(
                    "label.{}.{}",
                    self.current_function, name
                )));
                self.emit_statement(*body)?;
            }
            Statement::Switch {
                control,
                body,
                label,
                cases,
            } => {
                let break_label = format!("break_{}", label);
                let (control_val, ctrl_type) = self.emit_exp(control)?;
                // Integer promotion for switch control
                let promoted_type = ctrl_type.promote();
                let control_val = self.convert_to(control_val, ctrl_type, promoted_type);
                for case in &cases {
                    if let Some(val) = case.value {
                        let cmp_value = self.fresh_tmp(CType::Int);
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Equal,
                            left: control_val.clone(),
                            right: TackyVal::Constant(val),
                            dst: cmp_value.clone(),
                        });
                        self.emit(TackyInstr::JumpIfNotZero(cmp_value, case.label.clone()));
                    }
                }
                let default_label = cases
                    .iter()
                    .find(|c| c.value.is_none())
                    .map(|c| c.label.clone());
                match default_label {
                    Some(dl) => self.emit(TackyInstr::Jump(dl)),
                    None => self.emit(TackyInstr::Jump(break_label.clone())),
                }
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::Case { body, label, .. } => {
                self.emit(TackyInstr::Label(label));
                self.emit_statement(*body)?;
            }
            Statement::Default { body, label } => {
                self.emit(TackyInstr::Label(label));
                self.emit_statement(*body)?;
            }
            Statement::Null => {}
        }
        Ok(())
    }

    /// Flatten array initializer and emit CopyToOffset for each scalar value.
    /// `base_offset` is the byte offset from the start of the array.
    /// `elem_sizes`: byte size of each sub-array level.
    /// For `int[4][2][6]`: elem_sizes = [48, 24, 4] (size of [2][6], [6], int)
    fn emit_struct_init_at(
        &mut self,
        arr_name: &str,
        init: &Exp,
        tag: &str,
        base_offset: i64,
    ) -> TackyResult<()> {
        if let Exp::ArrayInit(elems) = init {
            let def = self
                .struct_defs
                .get(tag)
                .cloned()
                .ok_or_else(|| format!("Undefined struct: {}", tag))?;
            let has_designators = elems
                .iter()
                .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)));
            // For unions, the compound init initializes the first member only.
            // If the first member is an array/struct, treat the whole init as its initializer.
            if def.is_union && !def.members.is_empty() && !has_designators {
                let mem = &def.members[0];
                let mem_offset = base_offset + mem.offset as i64;
                if mem.member_full_type.is_array() {
                    // For union with array first member, check for string literal init
                    if elems.len() == 1 {
                        if let Exp::StringLiteral(ref s) = elems[0] {
                            let bytes = c_string_bytes(s);
                            let chars_to_copy = std::cmp::min(bytes.len(), mem.size);
                            for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                let src = self.fresh_tmp(CType::Char);
                                self.emit(TackyInstr::Copy {
                                    src: TackyVal::Constant(byte as i64),
                                    dst: src.clone(),
                                });
                                self.emit(TackyInstr::CopyToOffset {
                                    src,
                                    dst_name: arr_name.to_string(),
                                    offset: mem_offset + j as i64,
                                });
                            }
                            return Ok(());
                        }
                    }
                    // For compound array init, pass the elements
                    let mem_elem_sizes =
                        Self::compute_elem_sizes(&mem.member_full_type, &self.struct_defs);
                    let inner_scalar = {
                        let mut t = &mem.member_full_type;
                        while let FullType::Array { elem: e, .. } = t {
                            t = e;
                        }
                        t.to_ctype()
                    };
                    self.emit_array_init_flat(
                        arr_name,
                        init,
                        inner_scalar,
                        mem_offset,
                        &mem_elem_sizes,
                    )?;
                    return Ok(());
                } else if mem.member_full_type.is_struct() {
                    if let FullType::Struct(ref inner_tag) = mem.member_full_type {
                        self.emit_struct_init_at(arr_name, init, inner_tag, mem_offset)?;
                        return Ok(());
                    }
                }
                // For scalar first member, just use the first element
            }
            let max_members = if def.is_union { 1 } else { def.members.len() };
            let mut positional_index = 0usize;
            for elem in elems {
                if let Exp::DesignatedInit(designators, value) = elem {
                    self.emit_designated_init_at(
                        arr_name,
                        &FullType::Struct(tag.to_string()),
                        designators,
                        value,
                        base_offset,
                    )?;
                    continue;
                }
                if positional_index >= max_members {
                    break;
                }
                let mem = &def.members[positional_index];
                positional_index += 1;
                let mem_offset = base_offset + mem.offset as i64;
                match elem {
                    Exp::ArrayInit(_) if mem.member_full_type.is_array() => {
                        let mem_elem_sizes =
                            Self::compute_elem_sizes(&mem.member_full_type, &self.struct_defs);
                        let inner_scalar = {
                            let mut t = &mem.member_full_type;
                            while let FullType::Array { elem: e, .. } = t {
                                t = e;
                            }
                            t.to_ctype()
                        };
                        self.emit_array_init_flat(
                            arr_name,
                            elem,
                            inner_scalar,
                            mem_offset,
                            &mem_elem_sizes,
                        )?;
                    }
                    Exp::ArrayInit(_) if mem.member_full_type.is_struct() => {
                        if let FullType::Struct(ref inner_tag) = mem.member_full_type {
                            self.emit_struct_init_at(arr_name, elem, inner_tag, mem_offset)?;
                        }
                    }
                    Exp::StringLiteral(s) if mem.member_full_type.is_array() => {
                        let bytes = c_string_bytes(s);
                        let chars_to_copy = std::cmp::min(bytes.len(), mem.size);
                        for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                            let src = self.fresh_tmp(CType::Char);
                            self.emit(TackyInstr::Copy {
                                src: TackyVal::Constant(byte as i64),
                                dst: src.clone(),
                            });
                            self.emit(TackyInstr::CopyToOffset {
                                src,
                                dst_name: arr_name.to_string(),
                                offset: mem_offset + j as i64,
                            });
                        }
                    }
                    _ if mem.member_full_type.is_struct() => {
                        // Struct member initialized from a struct-valued expression
                        let struct_size = mem.member_full_type.byte_size_with(&self.struct_defs);
                        let (val, val_type) = self.emit_exp(elem.clone())?;
                        let src_addr = if val_type == CType::Pointer {
                            val
                        } else {
                            self.get_struct_addr(val)
                        };
                        let dst_addr = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: TackyVal::Var(arr_name.to_string()),
                            dst: dst_addr.clone(),
                        });
                        let member_addr = self.fresh_tmp(CType::Pointer);
                        if mem_offset > 0 {
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::Add,
                                left: dst_addr,
                                right: TackyVal::Constant(mem_offset),
                                dst: member_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: dst_addr,
                                dst: member_addr.clone(),
                            });
                        }
                        self.emit_struct_copy_ptr_to_ptr(src_addr, member_addr, struct_size);
                    }
                    _ => {
                        let (val, val_type) = self.emit_exp(elem.clone())?;
                        let val_conv = self.convert_to(val, val_type, mem.member_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: val_conv,
                            dst_name: arr_name.to_string(),
                            offset: mem_offset,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn emit_array_init_flat(
        &mut self,
        arr_name: &str,
        init: &Exp,
        scalar_type: CType,
        base_offset: i64,
        elem_sizes: &[i64],
    ) -> TackyResult<()> {
        match init {
            Exp::ArrayInit(elems) => {
                let this_elem_size = if !elem_sizes.is_empty() {
                    elem_sizes[0]
                } else {
                    scalar_type.size() as i64
                };
                let inner_sizes = if elem_sizes.len() > 1 {
                    &elem_sizes[1..]
                } else {
                    &[]
                };
                let array_ft = FullType::Array {
                    elem: Box::new(if inner_sizes.is_empty() {
                        FullType::Scalar(scalar_type)
                    } else {
                        FullType::Array {
                            elem: Box::new(FullType::Scalar(scalar_type)),
                            size: (this_elem_size / inner_sizes[0]) as usize,
                        }
                    }),
                    size: elems.len(),
                };
                let mut positional_index = 0usize;
                for elem in elems {
                    if let Exp::DesignatedInit(designators, value) = elem {
                        self.emit_designated_init_at(
                            arr_name,
                            &array_ft,
                            designators,
                            value,
                            base_offset,
                        )?;
                        continue;
                    }
                    let elem_offset = base_offset + (positional_index as i64) * this_elem_size;
                    positional_index += 1;
                    match elem {
                        Exp::ArrayInit(_)
                            if inner_sizes.is_empty() && scalar_type == CType::Struct =>
                        {
                            // Compound initializer for struct/union element in array
                            // Find the struct tag for the array element type
                            let arr_ft = self.get_full_type(arr_name);
                            let struct_tag = {
                                let mut t = &arr_ft;
                                // Peel arrays AND drill into struct first members if the
                                // outermost type is a struct/union (happens for nested unions)
                                while let FullType::Array { elem: e, .. } = t {
                                    t = e;
                                }
                                if let FullType::Struct(tag) = t {
                                    // Check if this is a union whose first member is an array of structs
                                    if let Some(def) = self.struct_defs.get(tag) {
                                        if def.is_union {
                                            if let Some(mem) = def.members.first() {
                                                let mut mt = &mem.member_full_type;
                                                while let FullType::Array { elem: e, .. } = mt {
                                                    mt = e;
                                                }
                                                if let FullType::Struct(inner_tag) = mt {
                                                    inner_tag.clone()
                                                } else {
                                                    tag.clone()
                                                }
                                            } else {
                                                tag.clone()
                                            }
                                        } else {
                                            tag.clone()
                                        }
                                    } else {
                                        tag.clone()
                                    }
                                } else {
                                    return Err("Expected struct in array".to_string());
                                }
                            };
                            self.emit_struct_init_at(arr_name, elem, &struct_tag, elem_offset)?;
                        }
                        _ if inner_sizes.is_empty()
                            && scalar_type == CType::Struct
                            && self.typeof_exp(elem).is_struct() =>
                        {
                            // Struct-valued expression in struct/union array
                            let arr_ft = self.get_full_type(arr_name);
                            let struct_tag = {
                                let mut t = &arr_ft;
                                while let FullType::Array { elem: e, .. } = t {
                                    t = e;
                                }
                                match t {
                                    FullType::Struct(tag) => tag.clone(),
                                    _ => return Err("Expected struct in array".to_string()),
                                }
                            };
                            let struct_size = self
                                .struct_defs
                                .get(&struct_tag)
                                .map(|d| d.size)
                                .unwrap_or(0);
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let src_addr = if val_type == CType::Pointer {
                                val
                            } else {
                                self.get_struct_addr(val)
                            };
                            let dst_addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: TackyVal::Var(arr_name.to_string()),
                                dst: dst_addr.clone(),
                            });
                            let elem_addr = self.fresh_tmp(CType::Pointer);
                            if elem_offset > 0 {
                                self.emit(TackyInstr::Binary {
                                    op: TackyBinaryOp::Add,
                                    left: dst_addr,
                                    right: TackyVal::Constant(elem_offset),
                                    dst: elem_addr.clone(),
                                });
                            } else {
                                self.emit(TackyInstr::Copy {
                                    src: dst_addr,
                                    dst: elem_addr.clone(),
                                });
                            }
                            self.emit_struct_copy_ptr_to_ptr(src_addr, elem_addr, struct_size);
                        }
                        Exp::ArrayInit(_) => {
                            self.emit_array_init_flat(
                                arr_name,
                                elem,
                                scalar_type,
                                elem_offset,
                                inner_sizes,
                            )?;
                        }
                        Exp::StringLiteral(s) if scalar_type == CType::Pointer => {
                            // String literal in array of pointers context: decay to pointer
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let val_conv = self.convert_to(val, val_type, scalar_type);
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: arr_name.to_string(),
                                offset: elem_offset,
                            });
                        }
                        Exp::StringLiteral(s) => {
                            // String literal as sub-element of char array compound init
                            let bytes = c_string_bytes(s);
                            let chars_to_copy = std::cmp::min(bytes.len(), this_elem_size as usize);
                            let char_type = if scalar_type == CType::UChar {
                                CType::UChar
                            } else {
                                CType::Char
                            };
                            for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                let src = self.fresh_tmp(char_type);
                                self.emit(TackyInstr::Copy {
                                    src: TackyVal::Constant(byte as i64),
                                    dst: src.clone(),
                                });
                                self.emit(TackyInstr::CopyToOffset {
                                    src,
                                    dst_name: arr_name.to_string(),
                                    offset: elem_offset + j as i64,
                                });
                            }
                        }
                        _ => {
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let val_conv = self.convert_to(val, val_type, scalar_type);
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: arr_name.to_string(),
                                offset: elem_offset,
                            });
                        }
                    }
                }
            }
            Exp::StringLiteral(s) => {
                // String literal fills the array; use its own length, not per-element size
                let bytes = c_string_bytes(s);
                let chars_to_copy = bytes.len();
                let char_type = if scalar_type == CType::UChar {
                    CType::UChar
                } else {
                    CType::Char
                };
                for (i, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                    let src = self.fresh_tmp(char_type);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(byte as i64),
                        dst: src.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src,
                        dst_name: arr_name.to_string(),
                        offset: base_offset + i as i64,
                    });
                }
            }
            _ => {
                let (val, val_type) = self.emit_exp(init.clone())?;
                let val_conv = self.convert_to(val, val_type, scalar_type);
                self.emit(TackyInstr::CopyToOffset {
                    src: val_conv,
                    dst_name: arr_name.to_string(),
                    offset: base_offset,
                });
            }
        }
        Ok(())
    }

    /// Compute element sizes for each array dimension.
    /// For `int[4][2][6]`: returns [48, 24, 4] (sizes of [2][6], [6], int)
    fn compute_elem_sizes(ft: &FullType, struct_defs: &HashMap<String, StructDef>) -> Vec<i64> {
        let mut sizes = Vec::new();
        let mut t = ft;
        while let FullType::Array { elem, .. } = t {
            sizes.push(elem.byte_size_with(struct_defs) as i64);
            t = elem;
        }
        sizes
    }

    fn eval_designator_index(exp: &Exp) -> Option<i64> {
        match exp {
            Exp::Constant(c)
            | Exp::LongConstant(c)
            | Exp::UIntConstant(c)
            | Exp::ULongConstant(c) => Some(*c),
            Exp::Unary(UnaryOp::Negate, inner) => Self::eval_designator_index(inner).map(|v| -v),
            Exp::Unary(UnaryOp::Complement, inner) => {
                Self::eval_designator_index(inner).map(|v| !v)
            }
            Exp::Unary(UnaryOp::LogicalNot, inner) => {
                Self::eval_designator_index(inner).map(|v| (v == 0) as i64)
            }
            Exp::Binary(op, left, right) => {
                let left = Self::eval_designator_index(left)?;
                let right = Self::eval_designator_index(right)?;
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
            _ => None,
        }
    }

    fn array_scalar_type(ft: &FullType) -> CType {
        let mut t = ft;
        while let FullType::Array { elem, .. } = t {
            t = elem;
        }
        t.to_ctype()
    }

    fn emit_initializer_value_at(
        &mut self,
        arr_name: &str,
        target_ft: &FullType,
        value: &Exp,
        offset: i64,
    ) -> TackyResult<()> {
        match (target_ft, value) {
            (FullType::Array { .. }, Exp::ArrayInit(_) | Exp::StringLiteral(_)) => {
                let elem_sizes = Self::compute_elem_sizes(target_ft, &self.struct_defs);
                self.emit_array_init_flat(
                    arr_name,
                    value,
                    Self::array_scalar_type(target_ft),
                    offset,
                    &elem_sizes,
                )?;
            }
            (FullType::Struct(tag), Exp::ArrayInit(_)) => {
                self.emit_struct_init_at(arr_name, value, tag, offset)?;
            }
            (FullType::Struct(_), _) => {
                let struct_size = target_ft.byte_size_with(&self.struct_defs);
                let (val, val_type) = self.emit_exp(value.clone())?;
                let src_addr = if val_type == CType::Pointer {
                    val
                } else {
                    self.get_struct_addr(val)
                };
                let dst_addr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(arr_name.to_string()),
                    dst: dst_addr.clone(),
                });
                let target_addr = self.fresh_tmp(CType::Pointer);
                if offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: dst_addr,
                        right: TackyVal::Constant(offset),
                        dst: target_addr.clone(),
                    });
                } else {
                    self.emit(TackyInstr::Copy {
                        src: dst_addr,
                        dst: target_addr.clone(),
                    });
                }
                self.emit_struct_copy_ptr_to_ptr(src_addr, target_addr, struct_size);
            }
            _ => {
                let target_type = target_ft.to_ctype();
                let (val, val_type) = self.emit_exp(value.clone())?;
                let val_conv = self.convert_to(val, val_type, target_type);
                self.emit(TackyInstr::CopyToOffset {
                    src: val_conv,
                    dst_name: arr_name.to_string(),
                    offset,
                });
            }
        }
        Ok(())
    }

    fn emit_designated_init_at(
        &mut self,
        arr_name: &str,
        base_ft: &FullType,
        designators: &[Designator],
        value: &Exp,
        base_offset: i64,
    ) -> TackyResult<()> {
        if designators.is_empty() {
            return self.emit_initializer_value_at(arr_name, base_ft, value, base_offset);
        }

        match (&designators[0], base_ft) {
            (Designator::Index(index), FullType::Array { elem, size: _ }) => {
                let index = Self::eval_designator_index(index)
                    .ok_or_else(|| "array designator index must be constant".to_string())?;
                if index < 0 {
                    return Err(format!("array designator index {} out of bounds", index));
                }
                let elem_size = elem.byte_size_with(&self.struct_defs) as i64;
                self.emit_designated_init_at(
                    arr_name,
                    elem,
                    &designators[1..],
                    value,
                    base_offset + index * elem_size,
                )?;
            }
            (Designator::Field(name), FullType::Struct(tag)) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let mem = def
                    .find_member(name)
                    .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?
                    .clone();
                self.emit_designated_init_at(
                    arr_name,
                    &mem.member_full_type,
                    &designators[1..],
                    value,
                    base_offset + mem.offset as i64,
                )?;
            }
            _ => {
                return Err(format!(
                    "invalid initializer designator for type {:?}",
                    base_ft
                ))
            }
        }
        Ok(())
    }

    fn put_static_initializer(
        &mut self,
        builder: &mut StaticInitBuilder,
        base_ft: &FullType,
        init: &Exp,
        base_offset: usize,
    ) -> TackyResult<()> {
        match (base_ft, init) {
            (FullType::Array { elem, size }, Exp::ArrayInit(elems)) => {
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let mut positional_index = 0usize;
                for elem_init in elems {
                    let (index, value) = if let Exp::DesignatedInit(designators, value) = elem_init
                    {
                        let (Designator::Index(index), rest) = designators
                            .split_first()
                            .ok_or_else(|| "empty initializer designator".to_string())?
                        else {
                            return Err("invalid array initializer designator".to_string());
                        };
                        let index = Self::eval_designator_index(index)
                            .ok_or_else(|| "array designator index must be constant".to_string())?;
                        if index < 0 || index as usize >= *size {
                            return Err(format!("array designator index {} out of bounds", index));
                        }
                        let value = if rest.is_empty() {
                            value.as_ref().clone()
                        } else {
                            Exp::DesignatedInit(rest.to_vec(), value.clone())
                        };
                        (index as usize, value)
                    } else {
                        let index = positional_index;
                        positional_index += 1;
                        if index >= *size {
                            break;
                        }
                        (index, elem_init.clone())
                    };
                    self.put_static_initializer(
                        builder,
                        elem,
                        &value,
                        base_offset + index * elem_size,
                    )?;
                }
            }
            (FullType::Array { elem, size }, Exp::StringLiteral(s)) => {
                let total_bytes = elem.byte_size_with(&self.struct_defs) * size;
                let string_bytes = c_string_byte_len(s);
                let null_terminated = string_bytes < total_bytes;
                let str_to_write = if string_bytes <= total_bytes {
                    s.clone()
                } else {
                    c_string_truncate_bytes(s, total_bytes)
                };
                builder.put(
                    base_offset,
                    StaticInit::StringInit(str_to_write, null_terminated),
                )?;
            }
            (FullType::Struct(tag), Exp::ArrayInit(elems)) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .cloned()
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let max_members = if def.is_union { 1 } else { def.members.len() };
                let mut positional_index = 0usize;
                for elem_init in elems {
                    let (member, value) = if let Exp::DesignatedInit(designators, value) = elem_init
                    {
                        let (Designator::Field(name), rest) = designators
                            .split_first()
                            .ok_or_else(|| "empty initializer designator".to_string())?
                        else {
                            return Err("invalid struct initializer designator".to_string());
                        };
                        let member = def
                            .find_member(name)
                            .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?
                            .clone();
                        let value = if rest.is_empty() {
                            value.as_ref().clone()
                        } else {
                            Exp::DesignatedInit(rest.to_vec(), value.clone())
                        };
                        (member, value)
                    } else {
                        if positional_index >= max_members {
                            break;
                        }
                        let member = def.members[positional_index].clone();
                        positional_index += 1;
                        (member, elem_init.clone())
                    };
                    self.put_static_initializer(
                        builder,
                        &member.member_full_type,
                        &value,
                        base_offset + member.offset,
                    )?;
                }
            }
            (FullType::Pointer(_), Exp::StringLiteral(s)) => {
                let label = self.make_string_constant(s);
                builder.put(base_offset, StaticInit::PointerInit(label))?;
            }
            (FullType::Scalar(ctype), _) => {
                let (v, is_dbl, is_uns) = eval_constant_init(&Some(init.clone()))?;
                let cv = convert_init_value(v, *ctype, is_dbl, is_uns);
                builder.put(base_offset, make_static_init(cv, *ctype))?;
            }
            (FullType::Pointer(_), _) => {
                let (v, is_dbl, is_uns) = eval_constant_init(&Some(init.clone()))?;
                let cv = convert_init_value(v, CType::Pointer, is_dbl, is_uns);
                builder.put(base_offset, make_static_init(cv, CType::Pointer))?;
            }
            _ => {
                return Err(format!(
                    "unsupported static aggregate initializer for {:?}",
                    base_ft
                ))
            }
        }
        Ok(())
    }

    fn build_static_initializer(
        &mut self,
        ft: &FullType,
        init: &Exp,
    ) -> TackyResult<Vec<StaticInit>> {
        let total_bytes = ft.byte_size_with(&self.struct_defs);
        let mut builder = StaticInitBuilder::new();
        self.put_static_initializer(&mut builder, ft, init, 0)?;
        builder.finish(total_bytes)
    }

    /// Handle a variable declaration (arrays, scalars, static, etc.)
    fn emit_var_decl(&mut self, vd: VarDeclaration) -> TackyResult<()> {
        if let Some(alignment) = vd.alignment {
            self.symbol_alignments
                .insert(vd.name.clone(), alignment.get());
        }
        let is_thread_local = vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_thread_local);
        // Static arrays
        if vd.array_dims.is_some()
            && vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_static)
        {
            let base_type = vd.var_type;
            let full_type = vd
                .decl_full_type
                .clone()
                .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
            let total_bytes = full_type.byte_size_with(&self.struct_defs);
            let align = {
                let elem_align = {
                    let mut t = &full_type;
                    while let FullType::Array { elem, .. } = t {
                        t = elem;
                    }
                    if let FullType::Struct(tag) = t {
                        self.struct_defs.get(tag).map(|d| d.alignment).unwrap_or(1)
                    } else {
                        std::cmp::max(base_type.size() as usize, 1)
                    }
                };
                if total_bytes >= 16 {
                    std::cmp::max(elem_align, 16)
                } else {
                    std::cmp::max(elem_align, 1)
                }
            };
            let align = vd.alignment.map_or(align, |a| a.get().max(align));
            self.register_var(&vd.name, full_type.clone());
            let init_values = if let Some(ref init_exp) = vd.init {
                self.build_static_initializer(&full_type, init_exp)?
            } else {
                vec![StaticInit::ZeroInit(total_bytes)]
            };
            self.static_vars.push(TackyStaticVar {
                name: vd.name.clone(),
                global: false,
                thread_local: is_thread_local,
                alignment: align,
                init_values,
            });
            return Ok(());
        }

        // Local arrays
        if vd.array_dims.is_some() {
            let base_type = vd.var_type;
            let full_type = vd
                .decl_full_type
                .clone()
                .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
            let total_bytes = full_type.byte_size_with(&self.struct_defs);
            self.register_var(&vd.name, full_type.clone());
            self.array_sizes.insert(vd.name.clone(), total_bytes);
            let scalar_type = {
                let mut t = &full_type;
                while let FullType::Array { elem, .. } = t {
                    t = elem;
                }
                t.to_ctype()
            };
            // Zero-fill using long-sized chunks
            {
                let mut off = 0usize;
                while off + 8 <= total_bytes {
                    let z = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: z.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: z,
                        dst_name: vd.name.clone(),
                        offset: off as i64,
                    });
                    off += 8;
                }
                while off + 4 <= total_bytes {
                    let z = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: z.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: z,
                        dst_name: vd.name.clone(),
                        offset: off as i64,
                    });
                    off += 4;
                }
                while off < total_bytes {
                    let z = self.fresh_tmp(CType::Char);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: z.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: z,
                        dst_name: vd.name.clone(),
                        offset: off as i64,
                    });
                    off += 1;
                }
            }
            if let Some(Exp::StringLiteral(ref s)) = vd.init {
                // String literal initializer for local char array: emit byte by byte
                let bytes = c_string_bytes(s);
                let chars_to_copy = std::cmp::min(bytes.len(), total_bytes);
                for (i, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                    let char_type = if base_type == CType::UChar {
                        CType::UChar
                    } else {
                        CType::Char
                    };
                    let src = self.fresh_tmp(char_type);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(byte as i64),
                        dst: src.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src,
                        dst_name: vd.name.clone(),
                        offset: i as i64,
                    });
                }
                // Null terminator if there's room (already zero-filled above)
            } else if let Some(init) = vd.init {
                let elem_sizes = Self::compute_elem_sizes(&full_type, &self.struct_defs);
                self.emit_array_init_flat(&vd.name, &init, scalar_type, 0, &elem_sizes)?;
            }
            return Ok(());
        }

        // Struct variable
        if let Some(FullType::Struct(ref tag)) = vd.decl_full_type {
            let tag = tag.clone();
            let def = self
                .struct_defs
                .get(&tag)
                .cloned()
                .ok_or_else(|| format!("Undefined struct: {}", tag))?;
            let struct_size = def.size;
            let struct_align = def.alignment;
            let ft = FullType::Struct(tag.clone());

            if vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_static)
            {
                self.register_var(&vd.name, ft.clone());
                self.array_sizes.insert(vd.name.clone(), struct_size);
                let init_values = if let Some(ref init) = vd.init {
                    self.build_static_initializer(&ft, init)?
                } else {
                    vec![StaticInit::ZeroInit(struct_size)]
                };
                self.static_vars.push(TackyStaticVar {
                    name: vd.name.clone(),
                    global: false,
                    thread_local: is_thread_local,
                    alignment: vd
                        .alignment
                        .map_or(struct_align, |a| a.get().max(struct_align)),
                    init_values,
                });
                return Ok(());
            }

            if vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_extern)
            {
                self.register_var(&vd.name, ft);
                self.extern_vars.push(vd.name);
                return Ok(());
            }
            self.register_var(&vd.name, ft);
            self.array_sizes.insert(vd.name.clone(), struct_size);
            // Zero-fill using long-sized chunks
            {
                let mut off = 0usize;
                while off + 8 <= struct_size {
                    let z = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: z.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: z,
                        dst_name: vd.name.clone(),
                        offset: off as i64,
                    });
                    off += 8;
                }
                while off + 4 <= struct_size {
                    let z = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: z.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: z,
                        dst_name: vd.name.clone(),
                        offset: off as i64,
                    });
                    off += 4;
                }
                while off < struct_size {
                    let z = self.fresh_tmp(CType::Char);
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: z.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: z,
                        dst_name: vd.name.clone(),
                        offset: off as i64,
                    });
                    off += 1;
                }
            }
            // Handle compound initializer
            if let Some(init_ref @ Exp::ArrayInit(elems)) = vd.init.as_ref() {
                if elems
                    .iter()
                    .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)))
                {
                    self.emit_struct_init_at(&vd.name, init_ref, &tag, 0)?;
                    return Ok(());
                }
                // For unions, if first member is array/struct, delegate the whole init
                if def.is_union && !def.members.is_empty() {
                    // For unions, the whole compound init {x, y, ...} initializes
                    // the FIRST MEMBER only. All elements go to the first member.
                    let mem = &def.members[0];
                    if mem.member_full_type.is_array() {
                        // Check if the first (and only) element is a string literal
                        if elems.len() == 1 {
                            if let Exp::StringLiteral(ref s) = elems[0] {
                                let bytes = c_string_bytes(s);
                                let chars_to_copy = std::cmp::min(bytes.len(), mem.size);
                                for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                    let src = self.fresh_tmp(CType::Char);
                                    self.emit(TackyInstr::Copy {
                                        src: TackyVal::Constant(byte as i64),
                                        dst: src.clone(),
                                    });
                                    self.emit(TackyInstr::CopyToOffset {
                                        src,
                                        dst_name: vd.name.clone(),
                                        offset: j as i64,
                                    });
                                }
                            } else {
                                // Single ArrayInit element → pass to array init for the array member
                                let mem_elem_sizes = Self::compute_elem_sizes(
                                    &mem.member_full_type,
                                    &self.struct_defs,
                                );
                                let inner_scalar = {
                                    let mut t = &mem.member_full_type;
                                    while let FullType::Array { elem: e, .. } = t {
                                        t = e;
                                    }
                                    t.to_ctype()
                                };
                                self.emit_array_init_flat(
                                    &vd.name,
                                    &elems[0],
                                    inner_scalar,
                                    0,
                                    &mem_elem_sizes,
                                )?;
                            }
                        } else {
                            // Multiple elements → they're all array element inits
                            let mem_elem_sizes =
                                Self::compute_elem_sizes(&mem.member_full_type, &self.struct_defs);
                            let inner_scalar = {
                                let mut t = &mem.member_full_type;
                                while let FullType::Array { elem: e, .. } = t {
                                    t = e;
                                }
                                t.to_ctype()
                            };
                            self.emit_array_init_flat(
                                &vd.name,
                                init_ref,
                                inner_scalar,
                                0,
                                &mem_elem_sizes,
                            )?;
                        }
                    } else if mem.member_full_type.is_struct() {
                        if let FullType::Struct(ref inner_tag) = mem.member_full_type {
                            let first = &elems[0];
                            if let Exp::ArrayInit(_) = first {
                                // Compound init for the struct member
                                if elems.len() == 1 {
                                    self.emit_struct_init_at(&vd.name, first, inner_tag, 0)?;
                                } else {
                                    self.emit_struct_init_at(&vd.name, init_ref, inner_tag, 0)?;
                                }
                            } else {
                                // Struct-valued expression (variable, etc.) — struct copy
                                let struct_size =
                                    mem.member_full_type.byte_size_with(&self.struct_defs);
                                let (val, val_type) = self.emit_exp(first.clone())?;
                                let src_addr = if val_type == CType::Pointer {
                                    val
                                } else {
                                    self.get_struct_addr(val)
                                };
                                self.emit_struct_copy_to(src_addr, &vd.name, struct_size);
                            }
                        }
                    } else {
                        // Scalar first member — use first element
                        let (val, val_type) = self.emit_exp(elems[0].clone())?;
                        let val_conv = self.convert_to(val, val_type, mem.member_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: val_conv,
                            dst_name: vd.name.clone(),
                            offset: 0,
                        });
                    }
                } else {
                    let max_members = def.members.len();
                    for (i, elem) in elems.iter().enumerate() {
                        if i >= max_members {
                            break;
                        }
                        let member = &def.members[i];
                        let mem_ft = &member.member_full_type;
                        // Handle nested struct/array member init
                        if mem_ft.is_array() || mem_ft.is_struct() {
                            // Handle string literal for char array members
                            if let Exp::StringLiteral(ref s) = elem {
                                let bytes = c_string_bytes(s);
                                let chars_to_copy = std::cmp::min(bytes.len(), member.size);
                                for (j, byte) in bytes.into_iter().take(chars_to_copy).enumerate() {
                                    let char_type = CType::Char;
                                    let src = self.fresh_tmp(char_type);
                                    self.emit(TackyInstr::Copy {
                                        src: TackyVal::Constant(byte as i64),
                                        dst: src.clone(),
                                    });
                                    self.emit(TackyInstr::CopyToOffset {
                                        src,
                                        dst_name: vd.name.clone(),
                                        offset: (member.offset + j) as i64,
                                    });
                                }
                            } else if mem_ft.is_struct()
                                && !matches!(elem, Exp::ArrayInit(_) | Exp::StringLiteral(_))
                            {
                                // Struct member initialized with a struct-valued expression (e.g., a variable)
                                let struct_size = mem_ft.byte_size_with(&self.struct_defs);
                                let (val, val_type) = self.emit_exp(elem.clone())?;
                                let src_addr = if val_type == CType::Pointer {
                                    val
                                } else {
                                    self.get_struct_addr(val)
                                };
                                // Copy struct data to the member offset
                                let dst_addr = self.fresh_tmp(CType::Pointer);
                                self.emit(TackyInstr::GetAddress {
                                    src: TackyVal::Var(vd.name.clone()),
                                    dst: dst_addr.clone(),
                                });
                                let member_addr = self.fresh_tmp(CType::Pointer);
                                if member.offset > 0 {
                                    self.emit(TackyInstr::Binary {
                                        op: TackyBinaryOp::Add,
                                        left: dst_addr,
                                        right: TackyVal::Constant(member.offset as i64),
                                        dst: member_addr.clone(),
                                    });
                                } else {
                                    self.emit(TackyInstr::Copy {
                                        src: dst_addr,
                                        dst: member_addr.clone(),
                                    });
                                }
                                self.emit_struct_copy_ptr_to_ptr(
                                    src_addr,
                                    member_addr,
                                    struct_size,
                                );
                            } else if let Exp::ArrayInit(ref sub_elems) = elem {
                                if mem_ft.is_array() {
                                    let elem_sizes =
                                        Self::compute_elem_sizes(mem_ft, &self.struct_defs);
                                    let scalar_t = {
                                        let mut t = mem_ft;
                                        while let FullType::Array { elem: e, .. } = t {
                                            t = e;
                                        }
                                        t.to_ctype()
                                    };
                                    self.emit_array_init_flat(
                                        &vd.name,
                                        elem,
                                        scalar_t,
                                        member.offset as i64,
                                        &elem_sizes,
                                    )?;
                                } else if let FullType::Struct(ref inner_tag) = mem_ft {
                                    // Nested struct compound init
                                    let inner_def =
                                        self.struct_defs.get(inner_tag).cloned().ok_or_else(
                                            || format!("Undefined struct: {}", inner_tag),
                                        )?;
                                    for (j, sub_elem) in sub_elems.iter().enumerate() {
                                        if j >= inner_def.members.len() {
                                            break;
                                        }
                                        let inner_mem = &inner_def.members[j];
                                        if inner_mem.member_full_type.is_array() {
                                            let elem_sizes = Self::compute_elem_sizes(
                                                &inner_mem.member_full_type,
                                                &self.struct_defs,
                                            );
                                            let scalar_t = {
                                                let mut t = &inner_mem.member_full_type;
                                                while let FullType::Array { elem: e, .. } = t {
                                                    t = e;
                                                }
                                                t.to_ctype()
                                            };
                                            self.emit_array_init_flat(
                                                &vd.name,
                                                sub_elem,
                                                scalar_t,
                                                (member.offset + inner_mem.offset) as i64,
                                                &elem_sizes,
                                            )?;
                                        } else if inner_mem.member_full_type.is_struct() {
                                            if let (
                                                Exp::ArrayInit(_),
                                                FullType::Struct(ref nested_tag),
                                            ) = (sub_elem, &inner_mem.member_full_type)
                                            {
                                                self.emit_struct_init_at(
                                                    &vd.name,
                                                    sub_elem,
                                                    nested_tag,
                                                    (member.offset + inner_mem.offset) as i64,
                                                )?;
                                            } else {
                                                let (val, val_type) =
                                                    self.emit_exp(sub_elem.clone())?;
                                                let target_type = inner_mem.member_type;
                                                let val_conv =
                                                    self.convert_to(val, val_type, target_type);
                                                self.emit(TackyInstr::CopyToOffset {
                                                    src: val_conv,
                                                    dst_name: vd.name.clone(),
                                                    offset: (member.offset + inner_mem.offset)
                                                        as i64,
                                                });
                                            }
                                        } else {
                                            let (val, val_type) =
                                                self.emit_exp(sub_elem.clone())?;
                                            let target_type = inner_mem.member_type;
                                            let val_conv =
                                                self.convert_to(val, val_type, target_type);
                                            self.emit(TackyInstr::CopyToOffset {
                                                src: val_conv,
                                                dst_name: vd.name.clone(),
                                                offset: (member.offset + inner_mem.offset) as i64,
                                            });
                                        }
                                    }
                                }
                            }
                        } else {
                            let (val, val_type) = self.emit_exp(elem.clone())?;
                            let target_type = member.member_type;
                            let val_conv = self.convert_to(val, val_type, target_type);
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: vd.name.clone(),
                                offset: member.offset as i64,
                            });
                        }
                    }
                } // end else (non-union compound init)
            } else if let Some(init) = vd.init {
                // Copy from another struct expression
                let (val, val_type) = self.emit_exp(init)?;
                let rhs_struct_name = if val_type == CType::Struct {
                    if let TackyVal::Var(ref n) = val {
                        Some(n.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(src_name) = rhs_struct_name {
                    self.emit(TackyInstr::CopyStruct {
                        src_name,
                        dst_name: vd.name.clone(),
                    });
                } else {
                    let src_addr = if val_type == CType::Pointer {
                        val
                    } else {
                        self.get_struct_addr(val)
                    };
                    self.emit_struct_copy_to(src_addr, &vd.name, struct_size);
                }
            }
            return Ok(());
        }

        // Regular scalar/pointer variable
        self.var_types.insert(vd.name.clone(), vd.var_type);
        self.symbol_types.insert(vd.name.clone(), vd.var_type);
        if let Some(pi) = vd.ptr_info {
            self.ptr_info.insert(vd.name.clone(), pi);
        }
        // Use decl_full_type if available (preserves pointer-to-array info)
        let ft = if let Some(ref dft) = vd.decl_full_type {
            dft.clone()
        } else {
            FullType::from_decl(vd.var_type, vd.ptr_info, &None)
        };
        self.full_types.insert(vd.name.clone(), ft.clone());

        if vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_static)
        {
            if let Some(Exp::ArrayInit(_)) = vd.init {
                return Ok(());
            }
            // Static pointer initialized with string literal: static char *p = "hello";
            if let Some(Exp::StringLiteral(ref s)) = vd.init {
                let str_label = self.make_string_constant(s);
                let align = std::cmp::max(vd.var_type.size() as usize, 1);
                let align = vd.alignment.map_or(align, |a| a.get().max(align));
                self.static_vars.push(TackyStaticVar {
                    name: vd.name,
                    global: false,
                    thread_local: is_thread_local,
                    alignment: align,
                    init_values: vec![StaticInit::PointerInit(str_label)],
                });
                return Ok(());
            }
            let (raw_val, is_dbl, is_uns) = eval_constant_init(&vd.init)?;
            let init_val = convert_init_value(raw_val, vd.var_type, is_dbl, is_uns);
            let align = if vd.var_type == CType::Double {
                16
            } else {
                std::cmp::max(vd.var_type.size() as usize, 1)
            };
            let align = vd.alignment.map_or(align, |a| a.get().max(align));
            let init_v = make_static_init(init_val, vd.var_type);
            self.static_vars.push(TackyStaticVar {
                name: vd.name,
                global: false,
                thread_local: is_thread_local,
                alignment: align,
                init_values: vec![init_v],
            });
        } else if vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_extern)
        {
            self.extern_vars.push(vd.name);
        } else if let Some(init) = vd.init {
            let vd_name = vd.name.clone();
            let init_for_type = init.clone();
            let (val, val_type) = self.emit_exp(init)?;
            let val_ft = self.val_full_type(&val);
            self.assert_assignable_exp_full_type(&ft, &val_ft, &init_for_type, "initializer")?;
            let val_conv = self.convert_to(val, val_type, vd.var_type);
            if vd.var_type == CType::Pointer {
                if let TackyVal::Var(ref src_name) = val_conv {
                    // Only propagate ptr_info if LHS doesn't have specific decl info
                    if vd.decl_full_type.is_none() {
                        if let Some(&info) = self.ptr_info.get(src_name) {
                            self.ptr_info.insert(vd_name.clone(), info);
                        }
                        if let Some(ft) = self.full_types.get(src_name).cloned() {
                            self.full_types.insert(vd_name.clone(), ft);
                        }
                    }
                }
            }
            self.emit(TackyInstr::Copy {
                src: val_conv,
                dst: TackyVal::Var(vd_name),
            });
        }
        Ok(())
    }

    fn emit_block(&mut self, block: Block) -> TackyResult<()> {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    self.emit_var_decl(vd)?;
                }
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    // Register function type for block-scope prototypes
                    let param_types: Vec<CType> = fd.params.iter().map(|(_, t, _)| *t).collect();
                    self.func_types.insert(
                        fd.name.clone(),
                        (fd.return_type, param_types, fd.return_ptr_info, fd.variadic),
                    );
                    self.func_param_full_types
                        .insert(fd.name.clone(), fd.param_full_types.clone());
                    if let Some(ref rft) = fd.return_full_type {
                        self.func_full_types.insert(fd.name.clone(), rft.clone());
                    }
                }
                BlockItem::Declaration(Declaration::StructDecl(sd)) => {
                    if !sd.members.is_empty() {
                        let def = StructDef::from_declaration(&sd, &self.struct_defs)?;
                        self.struct_defs.insert(sd.tag.clone(), def);
                    }
                }
                BlockItem::Declaration(Declaration::TypedefDecl) => {}
                BlockItem::Statement(stmt) => self.emit_statement(stmt)?,
            }
        }
        Ok(())
    }

    fn emit_function(&mut self, func: FunctionDeclaration) -> TackyResult<Option<TackyFunction>> {
        let Some(body) = func.body else {
            return Ok(None);
        };

        self.current_function = func.name.clone();
        self.instructions.clear();

        // Check if return type requires hidden pointer
        let ret_needs_hidden_ptr = if let Some(FullType::Struct(ref tag)) = func.return_full_type {
            self.struct_defs
                .get(tag)
                .map(|d| d.size > 16)
                .unwrap_or(false)
        } else {
            false
        };
        self.hidden_ret_ptr = if ret_needs_hidden_ptr {
            let name = format!("__ret_ptr_{}", func.name);
            self.var_types.insert(name.clone(), CType::Pointer);
            self.symbol_types.insert(name.clone(), CType::Pointer);
            Some(name)
        } else {
            None
        };
        let hidden_ret_ptr_name = self.hidden_ret_ptr.clone();

        // Register params — decompose struct params into eightbytes
        let mut tacky_params = Vec::new();
        let mut stack_params = std::collections::HashSet::new();
        let mut struct_param_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        if let Some(ref ret_ptr) = hidden_ret_ptr_name {
            tacky_params.push(ret_ptr.clone());
        }
        let mut struct_param_fixups: Vec<(String, String, StructDef)> = Vec::new(); // (original_name, tag, def)
        for (i, (name, ptype, pi)) in func.params.iter().enumerate() {
            let ft = if i < func.param_full_types.len() {
                func.param_full_types[i].clone()
            } else {
                FullType::from_decl(*ptype, *pi, &None)
            };

            if let FullType::Struct(ref tag) = ft {
                if let Some(def) = self.struct_defs.get(tag).cloned() {
                    let classes = def.classify_with(&self.struct_defs);
                    if classes.len() == 1 && classes[0] == ParamClass::Memory {
                        // Large struct: decompose into 8-byte eightbyte params (all on stack)
                        let num_eightbytes = def.size.div_ceil(8);
                        for eb_idx in 0..num_eightbytes {
                            let param_name = format!("{}_eb{}", name, eb_idx);
                            self.var_types.insert(param_name.clone(), CType::Long);
                            self.symbol_types.insert(param_name.clone(), CType::Long);
                            tacky_params.push(param_name.clone());
                            stack_params.insert(param_name);
                        }
                    } else {
                        // Decompose into eightbyte params
                        let group_start = tacky_params.len();
                        let is_sse_vec: Vec<bool> =
                            classes.iter().map(|c| *c == ParamClass::Sse).collect();
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let param_name = format!("{}_eb{}", name, eb_idx);
                            let param_type = match class {
                                ParamClass::Sse => CType::Double,
                                _ => CType::Long, // eightbytes always use full 64-bit register
                            };
                            self.var_types.insert(param_name.clone(), param_type);
                            self.symbol_types.insert(param_name.clone(), param_type);
                            tacky_params.push(param_name);
                        }
                        struct_param_groups.push((group_start, classes.len(), is_sse_vec));
                    }
                    // Register the original struct var — allocate enough for eightbyte storage
                    let classes = def.classify_with(&self.struct_defs);
                    let alloc_size = std::cmp::max(def.size, classes.len() * 8);
                    struct_param_fixups.push((name.clone(), tag.clone(), def));
                    self.register_var(name, ft.clone());
                    self.array_sizes.insert(name.clone(), alloc_size);
                    continue;
                }
            }

            self.var_types.insert(name.clone(), *ptype);
            self.symbol_types.insert(name.clone(), *ptype);
            if let Some(info) = pi {
                self.ptr_info.insert(name.clone(), *info);
            }
            self.full_types.insert(name.clone(), ft);
            tacky_params.push(name.clone());
        }

        // Reassemble struct params from eightbytes
        for (name, _tag, def) in &struct_param_fixups {
            let classes = def.classify_with(&self.struct_defs);
            let num_ebs = if classes.len() == 1 && classes[0] == ParamClass::Memory {
                def.size.div_ceil(8)
            } else {
                classes.len()
            };
            for eb_idx in 0..num_ebs {
                let param_name = format!("{}_eb{}", name, eb_idx);
                let eb_offset = (eb_idx * 8) as i64;
                self.emit(TackyInstr::CopyToOffset {
                    src: TackyVal::Var(param_name),
                    dst_name: name.clone(),
                    offset: eb_offset,
                });
            }
        }

        self.emit_block(body)?;
        self.emit(TackyInstr::Return(TackyVal::Constant(0)));

        Ok(Some(TackyFunction {
            name: func.name,
            params: tacky_params,
            global: true, // overridden by linkage map in generate()
            body: std::mem::take(&mut self.instructions),
            stack_params,
            struct_param_groups,
        }))
    }
}

fn is_comparison_op(op: &BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::LessThan
            | BinaryOp::GreaterThan
            | BinaryOp::LessEqual
            | BinaryOp::GreaterEqual
    )
}

/// Truncate/convert a constant value to the target type's bit width
fn convert_init_value(
    val: i64,
    target: CType,
    source_is_double: bool,
    source_is_unsigned: bool,
) -> i64 {
    if target == CType::Float && source_is_double {
        let d = f64::from_bits(val as u64) as f32;
        return d.to_bits() as i64;
    }
    if target == CType::Float && !source_is_double {
        let d = if source_is_unsigned {
            (val as u64) as f32
        } else {
            val as f32
        };
        return d.to_bits() as i64;
    }
    if target == CType::Double && !source_is_double {
        let d = if source_is_unsigned {
            (val as u64) as f64
        } else {
            val as f64
        };
        return (d.to_bits()) as i64;
    }
    if target != CType::Double && source_is_double {
        let d = f64::from_bits(val as u64);
        return match target {
            CType::Char | CType::SChar => d as i8 as i64,
            CType::UChar => d as u8 as i64,
            CType::Short => d as i16 as i64,
            CType::UShort => d as u16 as i64,
            CType::Bool => (d != 0.0) as i64,
            CType::Int => d as i32 as i64,
            CType::UInt => d as u32 as i64,
            CType::Long => d as i64,
            CType::ULong => d as u64 as i64,
            CType::Float => (d as f32).to_bits() as i64,
            _ => val,
        };
    }
    match target {
        CType::Char | CType::SChar => val as i8 as i64,
        CType::UChar => val as u8 as i64,
        CType::Short => val as i16 as i64,
        CType::UShort => val as u16 as i64,
        CType::Bool => (val != 0) as i64,
        CType::Int => val as i32 as i64,
        CType::UInt => val as u32 as i64,
        CType::Long | CType::ULong | CType::Double | CType::Pointer => val,
        CType::Float => (val as f32).to_bits() as i64,
        CType::Void | CType::Struct => val,
    }
}

fn make_static_init(val: i64, t: CType) -> StaticInit {
    if val == 0 {
        StaticInit::ZeroInit(t.size() as usize)
    } else {
        match t {
            CType::Char | CType::SChar => StaticInit::CharInit(val as i8),
            CType::UChar => StaticInit::UCharInit(val as u8),
            CType::Short => StaticInit::ShortInit(val as i16),
            CType::UShort => StaticInit::UShortInit(val as u16),
            CType::Bool => StaticInit::UCharInit((val != 0) as u8),
            CType::Int => StaticInit::IntInit(val as i32),
            CType::UInt => StaticInit::UIntInit(val as u32),
            CType::Long | CType::Pointer => StaticInit::LongInit(val),
            CType::ULong => StaticInit::ULongInit(val as u64),
            CType::Float => StaticInit::FloatInit(f32::from_bits(val as u32)),
            CType::Double => StaticInit::DoubleInit(f64::from_bits(val as u64)),
            CType::Void | CType::Struct => StaticInit::ZeroInit(0),
        }
    }
}

fn eval_static_integer_constant_exp(exp: &Exp) -> Option<(i64, bool, bool)> {
    match exp {
        Exp::Constant(c) | Exp::LongConstant(c) => Some((*c, false, false)),
        Exp::UIntConstant(c) | Exp::ULongConstant(c) => Some((*c, false, true)),
        Exp::DoubleConstant(d) => Some((d.to_bits() as i64, true, false)),
        Exp::Cast(_, _, inner) => eval_static_integer_constant_exp(inner),
        Exp::Unary(op, inner) => {
            let (value, is_double, is_unsigned) = eval_static_integer_constant_exp(inner)?;
            match op {
                UnaryOp::Negate if is_double => {
                    let d = -f64::from_bits(value as u64);
                    Some((d.to_bits() as i64, true, false))
                }
                UnaryOp::Negate => Some((-value, false, is_unsigned)),
                UnaryOp::Complement if !is_double => Some((!value, false, is_unsigned)),
                UnaryOp::LogicalNot if !is_double => Some(((value == 0) as i64, false, false)),
                _ => None,
            }
        }
        Exp::Binary(op, left, right) => {
            let (left, left_double, left_unsigned) = eval_static_integer_constant_exp(left)?;
            let (right, right_double, right_unsigned) = eval_static_integer_constant_exp(right)?;
            if left_double || right_double {
                return None;
            }
            let is_unsigned = left_unsigned || right_unsigned;
            let value = match op {
                BinaryOp::Add => left + right,
                BinaryOp::Sub => left - right,
                BinaryOp::Mul => left * right,
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
            Some((value, false, is_unsigned))
        }
        Exp::Conditional(cond, then_exp, else_exp) => {
            let (cond, is_double, _) = eval_static_integer_constant_exp(cond)?;
            if is_double {
                return None;
            }
            if cond != 0 {
                eval_static_integer_constant_exp(then_exp)
            } else {
                eval_static_integer_constant_exp(else_exp)
            }
        }
        _ => None,
    }
}

fn eval_constant_init(init: &Option<Exp>) -> TackyResult<(i64, bool, bool)> {
    if let Some(exp) = init {
        eval_static_integer_constant_exp(exp)
            .ok_or_else(|| "Static variable initializer must be a constant".to_string())
    } else {
        Ok((0, false, false))
    }
}

pub fn generate(program: Program) -> TackyResult<TackyProgram> {
    let mut gen = TackyGen::new();
    let mut top_level = Vec::new();
    let mut global_vars = std::collections::HashSet::new();
    let mut thread_local_vars = std::collections::HashSet::new();

    use std::collections::HashMap;

    // Determine linkage
    let mut linkage: HashMap<String, bool> = HashMap::new();
    for decl in &program.declarations {
        let (name, sc) = match decl {
            Declaration::FunDecl(fd) => (fd.name.clone(), &fd.storage_class),
            Declaration::VarDecl(vd) => (vd.name.clone(), &vd.storage_class),
            Declaration::StructDecl(_) | Declaration::TypedefDecl => continue,
        };
        linkage
            .entry(name)
            .or_insert(!sc.as_ref().is_some_and(StorageClass::is_static));
    }

    // Collect function types and file-scope variable types
    for decl in &program.declarations {
        match decl {
            Declaration::FunDecl(fd) => {
                let param_types: Vec<CType> = fd.params.iter().map(|(_, t, _)| *t).collect();
                gen.func_types.insert(
                    fd.name.clone(),
                    (fd.return_type, param_types, fd.return_ptr_info, fd.variadic),
                );
                gen.func_param_full_types
                    .insert(fd.name.clone(), fd.param_full_types.clone());
                if let Some(ref rft) = fd.return_full_type {
                    gen.func_full_types.insert(fd.name.clone(), rft.clone());
                }
            }
            Declaration::VarDecl(vd) => {
                gen.var_types.insert(vd.name.clone(), vd.var_type);
                gen.symbol_types.insert(vd.name.clone(), vd.var_type);
                if vd
                    .storage_class
                    .as_ref()
                    .is_some_and(StorageClass::is_thread_local)
                {
                    thread_local_vars.insert(vd.name.clone());
                }
                if let Some(alignment) = vd.alignment {
                    gen.symbol_alignments
                        .insert(vd.name.clone(), alignment.get());
                }
                if let Some(pi) = vd.ptr_info {
                    gen.ptr_info.insert(vd.name.clone(), pi);
                }
                // Register FullType (including for extern arrays)
                if let Some(ref dft) = vd.decl_full_type {
                    gen.full_types.insert(vd.name.clone(), dft.clone());
                    if dft.is_array() {
                        gen.array_sizes
                            .insert(vd.name.clone(), dft.byte_size_with(&gen.struct_defs));
                    }
                    if let FullType::Struct(ref tag) = dft {
                        if let Some(def) = gen.struct_defs.get(tag) {
                            gen.array_sizes.insert(vd.name.clone(), def.size);
                        }
                    }
                }
                global_vars.insert(vd.name.clone());
            }
            Declaration::StructDecl(sd) => {
                if !sd.members.is_empty() {
                    let def = StructDef::from_declaration(sd, &gen.struct_defs)?;
                    gen.struct_defs.insert(sd.tag.clone(), def);
                }
            }
            Declaration::TypedefDecl => {}
        }
    }

    // Collect file-scope variables, merging
    let mut file_scope_vars: HashMap<String, FileScopeVarInfo> = HashMap::new();
    let mut file_scope_alignments: HashMap<String, usize> = HashMap::new();
    let mut file_scope_order: Vec<String> = Vec::new();

    for decl in &program.declarations {
        if let Declaration::VarDecl(vd) = decl {
            global_vars.insert(vd.name.clone());
            let is_thread_local = vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_thread_local);
            if vd
                .storage_class
                .as_ref()
                .is_some_and(StorageClass::is_extern)
                && vd.init.is_none()
            {
                continue;
            }
            let init_val: Option<(i64, bool, bool)> = match &vd.init {
                Some(Exp::ArrayInit(_)) => None,     // Array init handled separately
                Some(Exp::StringLiteral(_)) => None, // String init handled separately
                Some(exp) => Some(
                    eval_static_integer_constant_exp(exp)
                        .ok_or_else(|| "Global initializer must be constant".to_string())?,
                ),
                None => None,
            };
            let is_global = *linkage.get(&vd.name).unwrap_or(&true);
            if let Some(alignment) = vd.alignment {
                file_scope_alignments.insert(vd.name.clone(), alignment.get());
            }
            if let Some(entry) = file_scope_vars.get_mut(&vd.name) {
                if init_val.is_some() {
                    entry.2 = init_val;
                }
                entry.1 |= is_thread_local;
            } else {
                file_scope_vars.insert(
                    vd.name.clone(),
                    (is_global, is_thread_local, init_val, vd.var_type),
                );
                file_scope_order.push(vd.name.clone());
            }
        }
    }

    // Handle global arrays (both initialized and uninitialized)
    let mut global_array_names: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for decl in &program.declarations {
        if let Declaration::VarDecl(vd) = decl {
            // Handle global struct variables
            if let Some(FullType::Struct(ref tag)) = vd.decl_full_type {
                if !vd
                    .storage_class
                    .as_ref()
                    .is_some_and(StorageClass::is_extern)
                    && !global_array_names.contains(&vd.name)
                {
                    let tag = tag.clone();
                    let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                    if let Some(def) = gen.struct_defs.get(&tag) {
                        let struct_size = def.size;
                        let struct_align = def.alignment;
                        let ft = FullType::Struct(tag.clone());
                        gen.register_var(&vd.name, ft);
                        gen.array_sizes.insert(vd.name.clone(), struct_size);
                        global_array_names.insert(vd.name.clone());
                        file_scope_vars.remove(&vd.name);

                        let init_values = if let Some(init_exp) = vd.init.as_ref() {
                            gen.build_static_initializer(&FullType::Struct(tag.clone()), init_exp)?
                        } else {
                            vec![StaticInit::ZeroInit(struct_size)]
                        };
                        top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                            name: vd.name.clone(),
                            global: is_global,
                            thread_local: vd
                                .storage_class
                                .as_ref()
                                .is_some_and(StorageClass::is_thread_local),
                            alignment: vd
                                .alignment
                                .map_or(struct_align, |a| a.get().max(struct_align)),
                            init_values,
                        }));
                    }
                    continue;
                }
            }
            // Handle uninitialized global arrays (skip extern and already-handled)
            if vd.array_dims.is_some()
                && !matches!(
                    &vd.init,
                    Some(Exp::ArrayInit(_)) | Some(Exp::StringLiteral(_))
                )
                && !vd
                    .storage_class
                    .as_ref()
                    .is_some_and(StorageClass::is_extern)
                && !global_array_names.contains(&vd.name)
            {
                let base_type = vd.var_type;
                let ft = vd
                    .decl_full_type
                    .clone()
                    .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
                let total_bytes = ft.byte_size_with(&gen.struct_defs);
                let align = if total_bytes >= 16 {
                    16
                } else {
                    std::cmp::max(base_type.size() as usize, 1)
                };
                let align = vd.alignment.map_or(align, |a| a.get().max(align));
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                gen.register_var(&vd.name, ft);
                global_array_names.insert(vd.name.clone());
                file_scope_vars.remove(&vd.name);
                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(),
                    global: is_global,
                    thread_local: vd
                        .storage_class
                        .as_ref()
                        .is_some_and(StorageClass::is_thread_local),
                    alignment: align,
                    init_values: vec![StaticInit::ZeroInit(total_bytes)],
                }));
                continue;
            }
            // Global char array initialized with string literal
            if let (Some(ref dims), Some(Exp::StringLiteral(ref s))) = (&vd.array_dims, &vd.init) {
                let base_type = vd.var_type;
                let total_elems: usize = dims.iter().product();
                let total_bytes = total_elems * base_type.size() as usize;
                let align = if total_bytes >= 16 {
                    16
                } else {
                    std::cmp::max(base_type.size() as usize, 1)
                };
                let align = vd.alignment.map_or(align, |a| a.get().max(align));
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                let ft = FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims);
                gen.register_var(&vd.name, ft);
                global_array_names.insert(vd.name.clone());
                file_scope_vars.remove(&vd.name);
                let string_bytes = c_string_byte_len(s);
                let null_terminated = string_bytes < total_bytes;
                let mut init_values: Vec<StaticInit> = vec![StaticInit::StringInit(
                    if string_bytes <= total_bytes {
                        s.clone()
                    } else {
                        c_string_truncate_bytes(s, total_bytes)
                    },
                    null_terminated,
                )];
                let written_bytes = if null_terminated {
                    string_bytes + 1
                } else {
                    string_bytes
                };
                if written_bytes < total_bytes {
                    init_values.push(StaticInit::ZeroInit(total_bytes - written_bytes));
                }
                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(),
                    global: is_global,
                    thread_local: vd
                        .storage_class
                        .as_ref()
                        .is_some_and(StorageClass::is_thread_local),
                    alignment: align,
                    init_values,
                }));
                continue;
            }
            // Global pointer initialized with string literal: char *p = "hello";
            if let (None, Some(Exp::StringLiteral(ref s))) = (&vd.array_dims, &vd.init) {
                let str_label = gen.make_string_constant(s);
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                let align = std::cmp::max(vd.var_type.size() as usize, 1);
                let align = vd.alignment.map_or(align, |a| a.get().max(align));
                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(),
                    global: is_global,
                    thread_local: vd
                        .storage_class
                        .as_ref()
                        .is_some_and(StorageClass::is_thread_local),
                    alignment: align,
                    init_values: vec![StaticInit::PointerInit(str_label)],
                }));
                file_scope_vars.remove(&vd.name);
                global_array_names.insert(vd.name.clone());
                // Also emit the string constants collected so far
                for sc in gen.static_constants.drain(..) {
                    global_vars.insert(sc.name.clone());
                    top_level.push(TackyTopLevel::StaticConstant(sc));
                }
                continue;
            }
            if let (Some(_), Some(init_exp @ Exp::ArrayInit(_))) = (&vd.array_dims, &vd.init) {
                let base_type = vd.var_type;
                let ft = vd
                    .decl_full_type
                    .clone()
                    .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
                let total_bytes = ft.byte_size_with(&gen.struct_defs);
                let align = if total_bytes >= 16 {
                    16
                } else {
                    std::cmp::max(base_type.size() as usize, 1)
                };
                let align = vd.alignment.map_or(align, |a| a.get().max(align));
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);

                let mut init_values = gen.build_static_initializer(&ft, init_exp)?;
                let initialized_bytes: usize =
                    init_values.iter().map(TackyGen::static_init_size).sum();
                if initialized_bytes < total_bytes {
                    init_values.push(StaticInit::ZeroInit(total_bytes - initialized_bytes));
                }

                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(),
                    global: is_global,
                    thread_local: vd
                        .storage_class
                        .as_ref()
                        .is_some_and(StorageClass::is_thread_local),
                    alignment: align,
                    init_values,
                }));

                // Register FullType
                gen.register_var(&vd.name, ft);
                global_array_names.insert(vd.name.clone());

                // Remove from file_scope_vars so it's not emitted twice
                file_scope_vars.remove(&vd.name);
            }
        }
    }

    for decl in program.declarations {
        match decl {
            Declaration::FunDecl(fd) => {
                let fname = fd.name.clone();
                if let Some(mut tf) = gen.emit_function(fd)? {
                    tf.global = *linkage.get(&fname).unwrap_or(&true);
                    top_level.push(TackyTopLevel::Function(tf));
                }
                for sv in gen.static_vars.drain(..) {
                    global_vars.insert(sv.name.clone());
                    top_level.push(TackyTopLevel::StaticVar(sv));
                }
                for sc in gen.static_constants.drain(..) {
                    global_vars.insert(sc.name.clone());
                    top_level.push(TackyTopLevel::StaticConstant(sc));
                }
                for ev in gen.extern_vars.drain(..) {
                    global_vars.insert(ev);
                }
            }
            Declaration::VarDecl(_) => {}
            Declaration::StructDecl(_) => {}
            Declaration::TypedefDecl => {}
        }
    }

    for name in file_scope_order {
        let Some((is_global, thread_local, init_val, var_type)) = file_scope_vars.remove(&name)
        else {
            continue;
        };
        let (raw_init, is_dbl, is_uns) = init_val.unwrap_or((0, false, false));
        let converted_init = convert_init_value(raw_init, var_type, is_dbl, is_uns);
        let align = if var_type == CType::Double {
            16
        } else {
            var_type.size() as usize
        };
        let align = file_scope_alignments
            .remove(&name)
            .map_or(align, |a| a.max(align));
        let init_v = make_static_init(converted_init, var_type);
        top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
            name,
            global: is_global,
            thread_local,
            alignment: align,
            init_values: vec![init_v],
        }));
    }

    // Add static local var names too
    for tl in &top_level {
        match tl {
            TackyTopLevel::StaticVar(sv) => {
                global_vars.insert(sv.name.clone());
                if sv.thread_local {
                    thread_local_vars.insert(sv.name.clone());
                }
            }
            TackyTopLevel::StaticConstant(sc) => {
                global_vars.insert(sc.name.clone());
            }
            _ => {}
        }
    }

    // Build var_struct_tags map
    let mut var_struct_tags = std::collections::HashMap::new();
    for (name, ft) in &gen.full_types {
        if let FullType::Struct(tag) = ft {
            var_struct_tags.insert(name.clone(), tag.clone());
        }
    }

    Ok(TackyProgram {
        top_level,
        global_vars,
        thread_local_vars,
        symbol_types: gen.symbol_types,
        symbol_alignments: gen.symbol_alignments,
        array_sizes: gen.array_sizes,
        struct_defs: gen.struct_defs,
        var_struct_tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lex, parse, resolve};

    fn lower(source: &str) -> TackyResult<TackyProgram> {
        let tokens = lex::lex(source)?;
        let ast = parse::parse(tokens)?;
        let resolved = resolve::resolve(ast).map_err(|err| err.render())?.program;
        generate(resolved)
    }

    fn require_err<T>(result: TackyResult<T>, context: &str) -> TackyResult<String> {
        match result {
            Ok(_) => Err(format!("{context} unexpectedly succeeded")),
            Err(err) => Ok(err),
        }
    }

    fn require_some<T>(value: Option<T>, context: &str) -> TackyResult<T> {
        value.ok_or_else(|| context.to_string())
    }

    #[test]
    fn function_designator_argument_lowers_to_address() -> Result<(), String> {
        let program = lower(
            "int f(int (*fp)(int), int x) { return fp(x); }\n\
             int inc(int x) { return x + 1; }\n\
             int main(void) { return f(inc, 9); }\n",
        );
        let debug = format!("{:#?}", program);
        assert!(debug.contains("GetAddress"));
        assert!(debug.contains("\"inc\""));
        assert!(debug.contains("name: \"f\""));
        Ok(())
    }

    #[test]
    fn compound_assign_to_struct_member_lowers_without_simple_lvalue_panic() -> Result<(), String> {
        let program = lower(
            "struct box { int value; };\n\
             int main(void) { struct box b = {4}; b.value += 3; return b.value; }\n",
        );
        let debug = format!("{:#?}", program);
        assert!(debug.contains("Load"));
        assert!(debug.contains("Binary"));
        assert!(debug.contains("Store"));
        Ok(())
    }

    #[test]
    fn static_designated_array_uses_sparse_initializer_offsets() -> Result<(), String> {
        let program = lower("int a[4] = { [2] = 9 };\n")?;
        let static_var = require_some(
            program.top_level.iter().find_map(|item| match item {
                TackyTopLevel::StaticVar(var) if var.name == "a" => Some(var),
                _ => None,
            }),
            "expected static variable",
        )?;
        assert!(matches!(static_var.init_values[0], StaticInit::ZeroInit(8)));
        assert!(matches!(static_var.init_values[1], StaticInit::IntInit(9)));
        assert!(matches!(static_var.init_values[2], StaticInit::ZeroInit(4)));
        Ok(())
    }

    #[test]
    fn static_initializer_reports_overlapping_designators() -> Result<(), String> {
        let mut builder = StaticInitBuilder::new();
        builder.put(0, StaticInit::LongInit(1))?;

        let err = require_err(
            builder.put(4, StaticInit::LongInit(2)),
            "overlapping initializer should fail",
        )?;

        assert_eq!(err, "overlapping static initializer designators");
        Ok(())
    }

    #[test]
    fn static_initializer_reports_array_designator_out_of_bounds() -> Result<(), String> {
        let mut gen = TackyGen::new();
        let init = Exp::ArrayInit(vec![Exp::DesignatedInit(
            vec![Designator::Index(Box::new(Exp::Constant(4)))],
            Box::new(Exp::Constant(1)),
        )]);

        let err = require_err(
            gen.build_static_initializer(
                &FullType::Array {
                    elem: Box::new(FullType::Scalar(CType::Int)),
                    size: 2,
                },
                &init,
            ),
            "out-of-bounds designator should fail",
        )?;

        assert_eq!(err, "array designator index 4 out of bounds");
        Ok(())
    }

    #[test]
    fn static_initializer_reports_nonconstant_scalar_init() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.build_static_initializer(
                &FullType::Scalar(CType::Int),
                &Exp::Var("runtime_value".to_string()),
            ),
            "nonconstant static initializer should fail",
        )?;

        assert_eq!(err, "Static variable initializer must be a constant");
        Ok(())
    }

    #[test]
    fn designated_init_reports_invalid_field() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_designated_init_at(
                "x",
                &FullType::Scalar(CType::Int),
                &[Designator::Field("member".to_string())],
                &Exp::Constant(1),
                0,
            ),
            "field designator on scalar should fail",
        )?;

        assert!(err.contains("invalid initializer designator"), "{err}");
        Ok(())
    }

    #[test]
    fn struct_init_reports_undefined_struct() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_struct_init_at("x", &Exp::ArrayInit(vec![Exp::Constant(1)]), "missing", 0),
            "undefined struct initializer should fail",
        )?;

        assert_eq!(err, "Undefined struct: missing");
        Ok(())
    }

    #[test]
    fn array_init_reports_missing_struct_context() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("items", FullType::Scalar(CType::Int));

        let err = require_err(
            gen.emit_array_init_flat(
                "items",
                &Exp::ArrayInit(vec![Exp::ArrayInit(vec![Exp::Constant(1)])]),
                CType::Struct,
                0,
                &[4],
            ),
            "struct array initializer without struct context should fail",
        )?;

        assert_eq!(err, "Expected struct in array");
        Ok(())
    }

    #[test]
    fn array_init_reports_bad_scalar_element_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "items",
            FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Int)),
                size: 1,
            },
        );

        let err = require_err(
            gen.emit_array_init_flat(
                "items",
                &Exp::Binary(
                    BinaryOp::Add,
                    Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                    Box::new(Exp::Constant(2)),
                ),
                CType::Int,
                0,
                &[4],
            ),
            "bad scalar element expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn local_struct_init_reports_bad_scalar_member_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );

        let err = require_err(
            gen.emit_var_decl(VarDeclaration {
                name: "b".to_string(),
                var_type: CType::Struct,
                ptr_info: None,
                array_dims: None,
                decl_full_type: Some(FullType::Struct("box".to_string())),
                init: Some(Exp::ArrayInit(vec![Exp::Binary(
                    BinaryOp::Add,
                    Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                    Box::new(Exp::Constant(2)),
                )])),
                storage_class: None,
                alignment: None,
            }),
            "bad struct member initializer should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn assignable_reports_incompatible_types() -> Result<(), String> {
        let gen = TackyGen::new();

        let err = require_err(
            gen.assert_assignable_full_type(
                &FullType::Struct("left".to_string()),
                &FullType::Struct("right".to_string()),
                "return",
            ),
            "incompatible struct types should fail",
        )?;

        assert_eq!(
            err,
            "incompatible types in return: cannot convert Struct(\"right\") to Struct(\"left\")"
        );
        Ok(())
    }

    #[test]
    fn struct_member_reports_missing_member() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );

        let err = require_err(
            gen.struct_member("box", "missing"),
            "missing member should fail",
        )?;

        assert_eq!(err, "No member 'missing' in struct box");
        Ok(())
    }

    #[test]
    fn dot_address_reports_non_member_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_dot_address(&Exp::Constant(1)),
            "non-member expression should fail",
        )?;

        assert_eq!(err, "emit_dot_address called on non-Dot/Arrow expression");
        Ok(())
    }

    #[test]
    fn dot_address_reports_bad_dot_base_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_dot_address(&Exp::Dot(
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                "member".to_string(),
            )),
            "bad dot base expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn dot_address_reports_bad_arrow_base_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_dot_address(&Exp::Arrow(
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                "member".to_string(),
            )),
            "bad arrow base expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_reports_array_init_in_expression_context() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::ArrayInit(vec![Exp::Constant(1)])),
            "array initializer expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_sizeof_and_alignof() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (sizeof_val, sizeof_ty) =
            gen.emit_exp(Exp::SizeOfType(CType::Int, FullType::Scalar(CType::Int)))?;
        let (align_val, align_ty) =
            gen.emit_exp(Exp::AlignOfType(FullType::Scalar(CType::Long)))?;

        assert_eq!(sizeof_ty, CType::ULong);
        assert_eq!(align_ty, CType::ULong);
        assert!(matches!(sizeof_val, TackyVal::Var(_)));
        assert!(matches!(align_val, TackyVal::Var(_)));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_array_var_decay() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "a",
            FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Int)),
                size: 4,
            },
        );

        let (val, ty) = gen.emit_exp(Exp::Var("a".to_string()))?;

        assert_eq!(ty, CType::Pointer);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::GetAddress { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_string_literal_decay() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::StringLiteral("hi".to_string()))?;

        assert_eq!(ty, CType::Pointer);
        assert!(matches!(val, TackyVal::Var(_)));
        assert_eq!(gen.static_constants.len(), 1);
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_cast_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Cast(
            CType::Long,
            Some(FullType::Scalar(CType::Long)),
            Box::new(Exp::Constant(7)),
        ))?;

        assert_eq!(ty, CType::Long);
        assert!(matches!(val, TackyVal::Var(_)));
        Ok(())
    }

    #[test]
    fn emit_exp_cast_reports_bad_operand_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Cast(
                CType::Int,
                Some(FullType::Scalar(CType::Int)),
                Box::new(Exp::Binary(
                    BinaryOp::Add,
                    Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                    Box::new(Exp::Constant(2)),
                )),
            )),
            "bad cast operand should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_scalar_compound_literal_cast() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Cast(
            CType::Int,
            None,
            Box::new(Exp::ArrayInit(vec![Exp::LongConstant(9)])),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_struct_compound_literal_cast() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "pair".to_string(),
            StructDef {
                tag: "pair".to_string(),
                members: vec![
                    StructMember {
                        name: "a".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 0,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 4,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                    },
                ],
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );

        let (val, ty) = gen.emit_exp(Exp::Cast(
            CType::Struct,
            Some(FullType::Struct("pair".to_string())),
            Box::new(Exp::ArrayInit(vec![Exp::Constant(1), Exp::Constant(2)])),
        ))?;

        assert_eq!(ty, CType::Struct);
        assert!(
            matches!(val, TackyVal::Var(ref name) if gen.full_types.get(name) == Some(&FullType::Struct("pair".to_string())))
        );
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::CopyToOffset { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_array_compound_literal_cast() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Cast(
            CType::Pointer,
            Some(FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Int)),
                size: 2,
            }),
            Box::new(Exp::ArrayInit(vec![Exp::Constant(1), Exp::Constant(2)])),
        ))?;

        assert_eq!(ty, CType::Pointer);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::GetAddress { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_struct_compound_literal_cast_reports_bad_init() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );

        let err = require_err(
            gen.emit_exp(Exp::Cast(
                CType::Struct,
                Some(FullType::Struct("missing".to_string())),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "undefined struct compound literal should fail",
        )?;

        assert_eq!(err, "Undefined struct: missing");
        Ok(())
    }

    #[test]
    fn emit_exp_reports_dot_on_non_struct() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("x", FullType::Scalar(CType::Int));

        let err = require_err(
            gen.emit_exp(Exp::Dot(
                Box::new(Exp::Var("x".to_string())),
                "member".to_string(),
            )),
            "dot on scalar should fail",
        )?;

        assert_eq!(err, "Dot on non-struct: Scalar(Int)");
        Ok(())
    }

    #[test]
    fn emit_exp_reports_arrow_on_non_pointer() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("x", FullType::Scalar(CType::Int));

        let err = require_err(
            gen.emit_exp(Exp::Arrow(
                Box::new(Exp::Var("x".to_string())),
                "member".to_string(),
            )),
            "arrow on scalar should fail",
        )?;

        assert_eq!(err, "Arrow on non-pointer: Scalar(Int)");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_unary_negation() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Unary(UnaryOp::Negate, Box::new(Exp::Constant(3))))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::Unary {
                op: TackyUnaryOp::Negate,
                ..
            }
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_logical_not() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) =
            gen.emit_exp(Exp::Unary(UnaryOp::LogicalNot, Box::new(Exp::Constant(0))))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::Unary {
                op: TackyUnaryOp::LogicalNot,
                ..
            }
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_unary_reports_bad_operand_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Unary(
                UnaryOp::Complement,
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "bad unary operand should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_scalar_unary_reports_invalid_operator() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_scalar_unary(UnaryOp::LogicalNot, Exp::Constant(0)),
            "logical-not should be handled outside scalar unary lowering",
        )?;

        assert_eq!(err, "invalid scalar unary operator: LogicalNot");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_binary_addition() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Binary(
            BinaryOp::Add,
            Box::new(Exp::Constant(2)),
            Box::new(Exp::Constant(3)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                ..
            }
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_logical_and() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Binary(
            BinaryOp::LogicalAnd,
            Box::new(Exp::Constant(1)),
            Box::new(Exp::Constant(0)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::JumpIfZero(_, _))));
        Ok(())
    }

    #[test]
    fn emit_exp_binary_reports_bad_left_operand() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Binary(
                BinaryOp::Add,
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                Box::new(Exp::Constant(3)),
            )),
            "bad binary left operand should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_binary_reports_invalid_logical_operator() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_binary(BinaryOp::LogicalAnd, Exp::Constant(1), Exp::Constant(2)),
            "logical operators should be handled outside scalar binary lowering",
        )?;

        assert_eq!(err, "invalid scalar binary operator: LogicalAnd");
        Ok(())
    }

    #[test]
    fn emit_exp_logical_reports_bad_right_operand() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Binary(
                BinaryOp::LogicalOr,
                Box::new(Exp::Constant(0)),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "bad logical right operand should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_comma_expression_to_right_value() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Comma(
            Box::new(Exp::Constant(1)),
            Box::new(Exp::LongConstant(2)),
        ))?;

        assert_eq!(ty, CType::Long);
        assert!(matches!(val, TackyVal::Var(_)));
        Ok(())
    }

    #[test]
    fn emit_exp_comma_reports_bad_left_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Comma(
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                Box::new(Exp::Constant(2)),
            )),
            "bad comma left expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_comma_reports_bad_right_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Comma(
                Box::new(Exp::Constant(1)),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(2)])),
            )),
            "bad comma right expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_conditional_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let (val, ty) = gen.emit_exp(Exp::Conditional(
            Box::new(Exp::Constant(1)),
            Box::new(Exp::Constant(2)),
            Box::new(Exp::LongConstant(3)),
        ))?;

        assert_eq!(ty, CType::Long);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::JumpIfZero(_, _))));
        Ok(())
    }

    #[test]
    fn emit_exp_conditional_reports_bad_condition() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Conditional(
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                Box::new(Exp::Constant(2)),
                Box::new(Exp::Constant(3)),
            )),
            "bad conditional condition should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_conditional_reports_bad_then_branch() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Conditional(
                Box::new(Exp::Constant(1)),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(2)])),
                Box::new(Exp::Constant(3)),
            )),
            "bad conditional then branch should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_conditional_reports_bad_else_branch() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Conditional(
                Box::new(Exp::Constant(1)),
                Box::new(Exp::Constant(2)),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(3)])),
            )),
            "bad conditional else branch should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_subscript_load() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let (val, ty) = gen.emit_exp(Exp::Subscript(
            Box::new(Exp::Var("p".to_string())),
            Box::new(Exp::Constant(0)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Load { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_subscript_reports_bad_array_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Subscript(
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                Box::new(Exp::Constant(0)),
            )),
            "bad subscript array expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_address_of_variable() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("x", FullType::Scalar(CType::Int));

        let (val, ty) = gen.emit_exp(Exp::Unary(
            UnaryOp::AddrOf,
            Box::new(Exp::Var("x".to_string())),
        ))?;

        assert_eq!(ty, CType::Pointer);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::GetAddress { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_deref_load() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let (val, ty) = gen.emit_exp(Exp::Unary(
            UnaryOp::Deref,
            Box::new(Exp::Var("p".to_string())),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Load { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_address_of_reports_non_lvalue() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Unary(UnaryOp::AddrOf, Box::new(Exp::Constant(1)))),
            "address-of non-lvalue should fail",
        )?;

        assert_eq!(err, "Expression is not a simple lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_deref_reports_bad_operand_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Unary(
                UnaryOp::Deref,
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "bad deref operand should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_scalar_function_call() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.func_types
            .insert("f".to_string(), (CType::Int, vec![CType::Int], None, false));
        gen.func_full_types
            .insert("f".to_string(), FullType::Scalar(CType::Int));
        gen.func_param_full_types
            .insert("f".to_string(), vec![FullType::Scalar(CType::Int)]);

        let (val, ty) = gen.emit_exp(Exp::FunctionCall("f".to_string(), vec![Exp::Constant(7)]))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::FunCall {
                name,
                args,
                indirect: false,
                ..
            } if name == "f" && args.len() == 1
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_small_struct_function_argument() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "pair".to_string(),
            StructDef {
                tag: "pair".to_string(),
                members: vec![
                    StructMember {
                        name: "a".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 0,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 4,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                    },
                ],
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("p", FullType::Struct("pair".to_string()));
        gen.func_types.insert(
            "take".to_string(),
            (CType::Int, vec![CType::Struct], None, false),
        );
        gen.func_full_types
            .insert("take".to_string(), FullType::Scalar(CType::Int));
        gen.func_param_full_types.insert(
            "take".to_string(),
            vec![FullType::Struct("pair".to_string())],
        );

        let (_val, ty) = gen.emit_exp(Exp::FunctionCall(
            "take".to_string(),
            vec![Exp::Var("p".to_string())],
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::FunCall {
                name,
                args,
                struct_arg_groups,
                ..
            } if name == "take" && args.len() == 1 && !struct_arg_groups.is_empty()
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_large_struct_function_return() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "big".to_string(),
            StructDef {
                tag: "big".to_string(),
                members: vec![
                    StructMember {
                        name: "a".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 0,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 8,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                    },
                    StructMember {
                        name: "c".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 16,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                    },
                ],
                size: 24,
                alignment: 8,
                is_union: false,
            },
        );
        gen.func_types
            .insert("make".to_string(), (CType::Struct, Vec::new(), None, false));
        gen.func_full_types
            .insert("make".to_string(), FullType::Struct("big".to_string()));
        gen.func_param_full_types
            .insert("make".to_string(), Vec::new());

        let (val, ty) = gen.emit_exp(Exp::FunctionCall("make".to_string(), Vec::new()))?;

        assert_eq!(ty, CType::Struct);
        assert!(
            matches!(val, TackyVal::Var(ref name) if gen.full_types.get(name) == Some(&FullType::Struct("big".to_string())))
        );
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::FunCall {
                name,
                args,
                fixed_flat_arg_count,
                ..
            } if name == "make" && args.len() == 1 && *fixed_flat_arg_count == 1
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_function_call_reports_wrong_argument_count() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.func_types
            .insert("f".to_string(), (CType::Int, vec![CType::Int], None, false));

        let err = require_err(
            gen.emit_exp(Exp::FunctionCall(
                "f".to_string(),
                vec![Exp::Constant(1), Exp::Constant(2)],
            )),
            "wrong argument count should fail",
        )?;

        assert_eq!(
            err,
            "function 'f' called with 2 argument(s), but prototype expects 1"
        );
        Ok(())
    }

    #[test]
    fn emit_exp_function_call_reports_bad_argument_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.func_types
            .insert("f".to_string(), (CType::Int, vec![CType::Int], None, false));

        let err = require_err(
            gen.emit_exp(Exp::FunctionCall(
                "f".to_string(),
                vec![Exp::ArrayInit(vec![Exp::Constant(1)])],
            )),
            "bad argument expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_scalar_indirect_call() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "fp",
            FullType::Pointer(Box::new(FullType::Function {
                return_type: Box::new(FullType::Scalar(CType::Int)),
                params: vec![FullType::Scalar(CType::Int)],
                variadic: false,
            })),
        );

        let (val, ty) = gen.emit_exp(Exp::IndirectCall(
            Box::new(Exp::Var("fp".to_string())),
            vec![Exp::Constant(7)],
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::FunCall {
                name,
                args,
                indirect: true,
                ..
            } if name == "fp" && args.len() == 1
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_small_struct_indirect_call_argument() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "pair".to_string(),
            StructDef {
                tag: "pair".to_string(),
                members: vec![
                    StructMember {
                        name: "a".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 0,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 4,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                    },
                ],
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("p", FullType::Struct("pair".to_string()));
        gen.register_var(
            "fp",
            FullType::Pointer(Box::new(FullType::Function {
                return_type: Box::new(FullType::Scalar(CType::Int)),
                params: vec![FullType::Struct("pair".to_string())],
                variadic: false,
            })),
        );

        let (_val, ty) = gen.emit_exp(Exp::IndirectCall(
            Box::new(Exp::Var("fp".to_string())),
            vec![Exp::Var("p".to_string())],
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::FunCall {
                name,
                indirect: true,
                struct_arg_groups,
                ..
            } if name == "fp" && !struct_arg_groups.is_empty()
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_large_struct_indirect_call_return() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "big".to_string(),
            StructDef {
                tag: "big".to_string(),
                members: vec![
                    StructMember {
                        name: "a".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 0,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 8,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                    },
                    StructMember {
                        name: "c".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 16,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                    },
                ],
                size: 24,
                alignment: 8,
                is_union: false,
            },
        );
        gen.register_var(
            "fp",
            FullType::Pointer(Box::new(FullType::Function {
                return_type: Box::new(FullType::Struct("big".to_string())),
                params: Vec::new(),
                variadic: false,
            })),
        );

        let (val, ty) = gen.emit_exp(Exp::IndirectCall(
            Box::new(Exp::Var("fp".to_string())),
            Vec::new(),
        ))?;

        assert_eq!(ty, CType::Struct);
        assert!(
            matches!(val, TackyVal::Var(ref name) if gen.full_types.get(name) == Some(&FullType::Struct("big".to_string())))
        );
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::FunCall {
                name,
                indirect: true,
                args,
                fixed_flat_arg_count,
                ..
            } if name == "fp" && args.len() == 1 && *fixed_flat_arg_count == 1
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_indirect_call_reports_wrong_argument_count() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "fp",
            FullType::Pointer(Box::new(FullType::Function {
                return_type: Box::new(FullType::Scalar(CType::Int)),
                params: vec![FullType::Scalar(CType::Int)],
                variadic: false,
            })),
        );

        let err = require_err(
            gen.emit_exp(Exp::IndirectCall(
                Box::new(Exp::Var("fp".to_string())),
                vec![Exp::Constant(1), Exp::Constant(2)],
            )),
            "wrong argument count should fail",
        )?;

        assert_eq!(
            err,
            "function pointer called with 2 argument(s), but prototype expects 1"
        );
        Ok(())
    }

    #[test]
    fn emit_exp_indirect_call_reports_bad_argument_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "fp",
            FullType::Pointer(Box::new(FullType::Function {
                return_type: Box::new(FullType::Scalar(CType::Int)),
                params: vec![FullType::Scalar(CType::Int)],
                variadic: false,
            })),
        );

        let err = require_err(
            gen.emit_exp(Exp::IndirectCall(
                Box::new(Exp::Var("fp".to_string())),
                vec![Exp::ArrayInit(vec![Exp::Constant(1)])],
            )),
            "bad argument expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_lvalue_reports_non_lvalue() -> Result<(), String> {
        let gen = TackyGen::new();

        let err = require_err(
            gen.emit_lvalue(Exp::Constant(1)),
            "constant is not an lvalue",
        )?;

        assert_eq!(err, "Expression is not a simple lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_reports_assignment_to_non_lvalue() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Assign(
                Box::new(Exp::Constant(1)),
                Box::new(Exp::Constant(2)),
            )),
            "assignment target should be an lvalue",
        )?;

        assert_eq!(err, "Expression is not a simple lvalue");
        Ok(())
    }

    #[test]
    fn scalar_lvalue_address_reports_bad_dot_address() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("x", FullType::Scalar(CType::Int));

        let err = require_err(
            gen.scalar_lvalue_address(Exp::Dot(
                Box::new(Exp::Var("x".to_string())),
                "member".to_string(),
            )),
            "dot on scalar should fail",
        )?;

        assert_eq!(
            err,
            "dot_inner_tag: non-struct type Scalar(Int) for Var(\"x\")"
        );
        Ok(())
    }

    #[test]
    fn emit_subscript_addr_reports_bad_index_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let err = require_err(
            gen.emit_subscript_addr(
                Exp::Var("p".to_string()),
                Exp::ArrayInit(vec![Exp::Constant(0)]),
            ),
            "bad index expression should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_subscript_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Subscript(
                Box::new(Exp::Var("p".to_string())),
                Box::new(Exp::Constant(0)),
            )),
            Box::new(Exp::Constant(7)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Constant(7) | TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_deref_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Unary(
                UnaryOp::Deref,
                Box::new(Exp::Var("p".to_string())),
            )),
            Box::new(Exp::Constant(11)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Constant(11) | TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_deref_assignment_reports_bad_rhs() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let err = require_err(
            gen.emit_exp(Exp::Assign(
                Box::new(Exp::Unary(
                    UnaryOp::Deref,
                    Box::new(Exp::Var("p".to_string())),
                )),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "bad rhs should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_simple_dot_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 4,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("b", FullType::Struct("box".to_string()));

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Dot(
                Box::new(Exp::Var("b".to_string())),
                "value".to_string(),
            )),
            Box::new(Exp::Constant(9)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Constant(9) | TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::CopyToOffset {
                dst_name,
                offset: 4,
                ..
            } if dst_name == "b"
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_nested_dot_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "inner".to_string(),
            StructDef {
                tag: "inner".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.struct_defs.insert(
            "outer".to_string(),
            StructDef {
                tag: "outer".to_string(),
                members: vec![StructMember {
                    name: "inner".to_string(),
                    member_type: CType::Struct,
                    member_full_type: FullType::Struct("inner".to_string()),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("o", FullType::Struct("outer".to_string()));

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Dot(
                Box::new(Exp::Dot(
                    Box::new(Exp::Var("o".to_string())),
                    "inner".to_string(),
                )),
                "value".to_string(),
            )),
            Box::new(Exp::Constant(13)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Constant(13) | TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_struct_dot_member_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "inner".to_string(),
            StructDef {
                tag: "inner".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.struct_defs.insert(
            "outer".to_string(),
            StructDef {
                tag: "outer".to_string(),
                members: vec![StructMember {
                    name: "inner".to_string(),
                    member_type: CType::Struct,
                    member_full_type: FullType::Struct("inner".to_string()),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("o", FullType::Struct("outer".to_string()));
        gen.register_var("src", FullType::Struct("inner".to_string()));

        let (_val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Dot(
                Box::new(Exp::Var("o".to_string())),
                "inner".to_string(),
            )),
            Box::new(Exp::Var("src".to_string())),
        ))?;

        assert_eq!(ty, CType::Struct);
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_struct_arrow_member_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "inner".to_string(),
            StructDef {
                tag: "inner".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.struct_defs.insert(
            "outer".to_string(),
            StructDef {
                tag: "outer".to_string(),
                members: vec![StructMember {
                    name: "inner".to_string(),
                    member_type: CType::Struct,
                    member_full_type: FullType::Struct("inner".to_string()),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("outer".to_string()))),
        );
        gen.register_var("src", FullType::Struct("inner".to_string()));

        let (_val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Arrow(
                Box::new(Exp::Var("p".to_string())),
                "inner".to_string(),
            )),
            Box::new(Exp::Var("src".to_string())),
        ))?;

        assert_eq!(ty, CType::Struct);
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_struct_variable_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("dst", FullType::Struct("box".to_string()));
        gen.register_var("src", FullType::Struct("box".to_string()));

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Var("dst".to_string())),
            Box::new(Exp::Var("src".to_string())),
        ))?;

        assert_eq!(ty, CType::Struct);
        assert!(matches!(val, TackyVal::Var(ref name) if name == "dst"));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::CopyStruct { src_name, dst_name }
                if src_name == "src" && dst_name == "dst"
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_struct_variable_assignment_from_member_lvalue() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "inner".to_string(),
            StructDef {
                tag: "inner".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.struct_defs.insert(
            "outer".to_string(),
            StructDef {
                tag: "outer".to_string(),
                members: vec![StructMember {
                    name: "field".to_string(),
                    member_type: CType::Struct,
                    member_full_type: FullType::Struct("inner".to_string()),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("dst", FullType::Struct("inner".to_string()));
        gen.register_var("src", FullType::Struct("outer".to_string()));

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Var("dst".to_string())),
            Box::new(Exp::Dot(
                Box::new(Exp::Var("src".to_string())),
                "field".to_string(),
            )),
        ))?;

        assert_eq!(ty, CType::Struct);
        assert!(matches!(val, TackyVal::Var(ref name) if name == "dst"));
        assert!(gen.instructions.iter().any(
            |instr| matches!(instr, TackyInstr::CopyToOffset { dst_name, .. } if dst_name == "dst")
        ));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_subscript_struct_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("box".to_string()))),
        );
        gen.register_var("src", FullType::Struct("box".to_string()));

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Subscript(
                Box::new(Exp::Var("p".to_string())),
                Box::new(Exp::Constant(0)),
            )),
            Box::new(Exp::Var("src".to_string())),
        ))?;

        assert_eq!(ty, CType::Pointer);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_deref_struct_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("box".to_string()))),
        );
        gen.register_var("src", FullType::Struct("box".to_string()));

        let (_val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Unary(
                UnaryOp::Deref,
                Box::new(Exp::Var("p".to_string())),
            )),
            Box::new(Exp::Var("src".to_string())),
        ))?;

        assert_eq!(ty, CType::Struct);
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_deref_struct_assignment_reports_bad_rhs() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: Vec::new(),
                size: 0,
                alignment: 1,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("box".to_string()))),
        );

        let err = require_err(
            gen.emit_exp(Exp::Assign(
                Box::new(Exp::Unary(
                    UnaryOp::Deref,
                    Box::new(Exp::Var("p".to_string())),
                )),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "bad deref struct assignment rhs should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_struct_variable_assignment_reports_bad_rhs() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: Vec::new(),
                size: 0,
                alignment: 1,
                is_union: false,
            },
        );
        gen.register_var("dst", FullType::Struct("box".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::Assign(
                Box::new(Exp::Var("dst".to_string())),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "bad struct assignment rhs should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_dot_assignment_reports_missing_member() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: Vec::new(),
                size: 0,
                alignment: 1,
                is_union: false,
            },
        );
        gen.register_var("b", FullType::Struct("box".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::Assign(
                Box::new(Exp::Dot(
                    Box::new(Exp::Var("b".to_string())),
                    "missing".to_string(),
                )),
                Box::new(Exp::Constant(9)),
            )),
            "missing member should fail",
        )?;

        assert_eq!(err, "No member 'missing' in struct box");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_simple_arrow_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 4,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("box".to_string()))),
        );

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Arrow(
                Box::new(Exp::Var("p".to_string())),
                "value".to_string(),
            )),
            Box::new(Exp::Constant(9)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Constant(9) | TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_arrow_assignment_reports_missing_member() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: Vec::new(),
                size: 0,
                alignment: 1,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("box".to_string()))),
        );

        let err = require_err(
            gen.emit_exp(Exp::Assign(
                Box::new(Exp::Arrow(
                    Box::new(Exp::Var("p".to_string())),
                    "missing".to_string(),
                )),
                Box::new(Exp::Constant(9)),
            )),
            "missing member should fail",
        )?;

        assert_eq!(err, "No member 'missing' in struct box");
        Ok(())
    }

    #[test]
    fn emit_exp_arrow_assignment_falls_back_for_scalar_pointer() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("p", FullType::Scalar(CType::Pointer));

        let (val, ty) = gen.emit_exp(Exp::Assign(
            Box::new(Exp::Arrow(
                Box::new(Exp::Var("p".to_string())),
                "value".to_string(),
            )),
            Box::new(Exp::Constant(5)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Constant(5) | TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_simple_compound_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("x", FullType::Scalar(CType::Int));

        let (val, ty) = gen.emit_exp(Exp::CompoundAssign(
            BinaryOp::Add,
            Box::new(Exp::Var("x".to_string())),
            Box::new(Exp::Constant(3)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(ref name) if name == "x"));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Binary { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_compound_assignment_reports_non_lvalue() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Constant(1)),
                Box::new(Exp::Constant(3)),
            )),
            "compound assignment target should be an lvalue",
        )?;

        assert_eq!(err, "Expression is not a simple lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_subscript_compound_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let (val, ty) = gen.emit_exp(Exp::CompoundAssign(
            BinaryOp::Add,
            Box::new(Exp::Subscript(
                Box::new(Exp::Var("p".to_string())),
                Box::new(Exp::Constant(0)),
            )),
            Box::new(Exp::Constant(2)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_deref_compound_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let (val, ty) = gen.emit_exp(Exp::CompoundAssign(
            BinaryOp::Add,
            Box::new(Exp::Unary(
                UnaryOp::Deref,
                Box::new(Exp::Var("p".to_string())),
            )),
            Box::new(Exp::Constant(2)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_dot_compound_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("b", FullType::Struct("box".to_string()));

        let (val, ty) = gen.emit_exp(Exp::CompoundAssign(
            BinaryOp::Add,
            Box::new(Exp::Dot(
                Box::new(Exp::Var("b".to_string())),
                "value".to_string(),
            )),
            Box::new(Exp::Constant(2)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_arrow_compound_assignment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: vec![StructMember {
                    name: "value".to_string(),
                    member_type: CType::Int,
                    member_full_type: FullType::Scalar(CType::Int),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("box".to_string()))),
        );

        let (val, ty) = gen.emit_exp(Exp::CompoundAssign(
            BinaryOp::Add,
            Box::new(Exp::Arrow(
                Box::new(Exp::Var("p".to_string())),
                "value".to_string(),
            )),
            Box::new(Exp::Constant(2)),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_dot_compound_assignment_reports_missing_member() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "box".to_string(),
            StructDef {
                tag: "box".to_string(),
                members: Vec::new(),
                size: 0,
                alignment: 1,
                is_union: false,
            },
        );
        gen.register_var("b", FullType::Struct("box".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Dot(
                    Box::new(Exp::Var("b".to_string())),
                    "missing".to_string(),
                )),
                Box::new(Exp::Constant(2)),
            )),
            "missing member should fail",
        )?;

        assert_eq!(err, "No member 'missing' in struct box");
        Ok(())
    }

    #[test]
    fn emit_exp_subscript_compound_assignment_reports_bad_rhs() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Subscript(
                    Box::new(Exp::Var("p".to_string())),
                    Box::new(Exp::Constant(0)),
                )),
                Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
            )),
            "bad rhs should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }

    #[test]
    fn emit_exp_struct_variable_compound_assignment_reports_non_scalar() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "pair".to_string(),
            StructDef {
                tag: "pair".to_string(),
                members: Vec::new(),
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("lhs", FullType::Struct("pair".to_string()));
        gen.register_var("rhs", FullType::Struct("pair".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Var("lhs".to_string())),
                Box::new(Exp::Var("rhs".to_string())),
            )),
            "struct compound assignment should fail",
        )?;

        assert_eq!(err, "compound assignment to non-scalar lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_subscript_struct_compound_assignment_reports_non_scalar() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "pair".to_string(),
            StructDef {
                tag: "pair".to_string(),
                members: Vec::new(),
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("pair".to_string()))),
        );
        gen.register_var("rhs", FullType::Struct("pair".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Subscript(
                    Box::new(Exp::Var("p".to_string())),
                    Box::new(Exp::Constant(0)),
                )),
                Box::new(Exp::Var("rhs".to_string())),
            )),
            "subscript struct compound assignment should fail",
        )?;

        assert_eq!(err, "compound assignment to non-scalar lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_deref_struct_compound_assignment_reports_non_scalar() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "pair".to_string(),
            StructDef {
                tag: "pair".to_string(),
                members: Vec::new(),
                size: 8,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("pair".to_string()))),
        );
        gen.register_var("rhs", FullType::Struct("pair".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Unary(
                    UnaryOp::Deref,
                    Box::new(Exp::Var("p".to_string())),
                )),
                Box::new(Exp::Var("rhs".to_string())),
            )),
            "deref struct compound assignment should fail",
        )?;

        assert_eq!(err, "compound assignment to non-scalar lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_dot_struct_compound_assignment_reports_non_scalar() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "inner".to_string(),
            StructDef {
                tag: "inner".to_string(),
                members: Vec::new(),
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.struct_defs.insert(
            "outer".to_string(),
            StructDef {
                tag: "outer".to_string(),
                members: vec![StructMember {
                    name: "field".to_string(),
                    member_type: CType::Struct,
                    member_full_type: FullType::Struct("inner".to_string()),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var("o", FullType::Struct("outer".to_string()));
        gen.register_var("rhs", FullType::Struct("inner".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Dot(
                    Box::new(Exp::Var("o".to_string())),
                    "field".to_string(),
                )),
                Box::new(Exp::Var("rhs".to_string())),
            )),
            "dot struct compound assignment should fail",
        )?;

        assert_eq!(err, "compound assignment to non-scalar lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_arrow_struct_compound_assignment_reports_non_scalar() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.struct_defs.insert(
            "inner".to_string(),
            StructDef {
                tag: "inner".to_string(),
                members: Vec::new(),
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.struct_defs.insert(
            "outer".to_string(),
            StructDef {
                tag: "outer".to_string(),
                members: vec![StructMember {
                    name: "field".to_string(),
                    member_type: CType::Struct,
                    member_full_type: FullType::Struct("inner".to_string()),
                    offset: 0,
                    size: 4,
                    bit_width: None,
                    bit_offset: 0,
                }],
                size: 4,
                alignment: 4,
                is_union: false,
            },
        );
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Struct("outer".to_string()))),
        );
        gen.register_var("rhs", FullType::Struct("inner".to_string()));

        let err = require_err(
            gen.emit_exp(Exp::CompoundAssign(
                BinaryOp::Add,
                Box::new(Exp::Arrow(
                    Box::new(Exp::Var("p".to_string())),
                    "field".to_string(),
                )),
                Box::new(Exp::Var("rhs".to_string())),
            )),
            "arrow struct compound assignment should fail",
        )?;

        assert_eq!(err, "compound assignment to non-scalar lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_pre_increment_variable() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("x", FullType::Scalar(CType::Int));

        let (val, ty) = gen.emit_exp(Exp::Unary(
            UnaryOp::PreIncrement,
            Box::new(Exp::Var("x".to_string())),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                ..
            }
        )));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::Copy {
                dst: TackyVal::Var(name),
                ..
            } if name == "x"
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_post_decrement_variable() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var("x", FullType::Scalar(CType::Int));

        let (val, ty) = gen.emit_exp(Exp::Unary(
            UnaryOp::PostDecrement,
            Box::new(Exp::Var("x".to_string())),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::Binary {
                op: TackyBinaryOp::Sub,
                ..
            }
        )));
        assert!(gen.instructions.iter().any(|instr| matches!(
            instr,
            TackyInstr::Copy {
                src: TackyVal::Var(_),
                dst: TackyVal::Var(name),
            } if name == "x"
        )));
        Ok(())
    }

    #[test]
    fn emit_exp_lowers_deref_pre_increment() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let (val, ty) = gen.emit_exp(Exp::Unary(
            UnaryOp::PreIncrement,
            Box::new(Exp::Unary(
                UnaryOp::Deref,
                Box::new(Exp::Var("p".to_string())),
            )),
        ))?;

        assert_eq!(ty, CType::Int);
        assert!(matches!(val, TackyVal::Var(_)));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Load { .. })));
        assert!(gen
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::Store { .. })));
        Ok(())
    }

    #[test]
    fn emit_exp_increment_reports_non_lvalue() -> Result<(), String> {
        let mut gen = TackyGen::new();

        let err = require_err(
            gen.emit_exp(Exp::Unary(
                UnaryOp::PreIncrement,
                Box::new(Exp::Constant(1)),
            )),
            "increment target should be an lvalue",
        )?;

        assert_eq!(err, "Expression is not a simple lvalue");
        Ok(())
    }

    #[test]
    fn emit_exp_subscript_increment_reports_bad_index_expression() -> Result<(), String> {
        let mut gen = TackyGen::new();
        gen.register_var(
            "p",
            FullType::Pointer(Box::new(FullType::Scalar(CType::Int))),
        );

        let err = require_err(
            gen.emit_exp(Exp::Unary(
                UnaryOp::PostIncrement,
                Box::new(Exp::Subscript(
                    Box::new(Exp::Var("p".to_string())),
                    Box::new(Exp::ArrayInit(vec![Exp::Constant(0)])),
                )),
            )),
            "bad subscript index should fail",
        )?;

        assert_eq!(err, "Array initializer not allowed in expression context");
        Ok(())
    }
}
