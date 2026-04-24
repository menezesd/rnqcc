use crate::types::*;
use std::collections::HashMap;

struct TackyGen {
    tmp_counter: usize,
    label_counter: usize,
    string_counter: usize,
    instructions: Vec<TackyInstr>,
    current_function: String,
    static_vars: Vec<TackyStaticVar>,
    static_constants: Vec<TackyStaticConstant>,
    extern_vars: Vec<String>,
    /// CType for each variable/temporary (for codegen output)
    symbol_types: HashMap<String, CType>,
    /// Rich type info (tracks arrays, pointer targets)
    full_types: HashMap<String, FullType>,
    /// Function types: (return_type, param_types, return_ptr_info)
    func_types: HashMap<String, (CType, Vec<CType>, Option<(CType, usize)>)>,
    /// Function return full types
    func_full_types: HashMap<String, FullType>,
    /// Type of variables (from declarations) — kept for backward compat
    var_types: HashMap<String, CType>,
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
            static_vars: Vec::new(),
            static_constants: Vec::new(),
            extern_vars: Vec::new(),
            symbol_types: HashMap::new(),
            full_types: HashMap::new(),
            func_types: HashMap::new(),
            func_full_types: HashMap::new(),
            var_types: HashMap::new(),
            ptr_info: HashMap::new(),
            array_sizes: HashMap::new(),
            struct_defs: HashMap::new(),
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
        // Also populate ptr_info for backward compat
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

        // Also populate ptr_info for backward compat
        if let FullType::Pointer(ref inner) = ft {
            let (base, depth) = Self::ptr_info_from_full(inner);
            self.ptr_info.insert(name.to_string(), (base, depth));
        }

        // Track array sizes
        if ft.is_array() {
            self.array_sizes.insert(name.to_string(), ft.byte_size());
        }
    }

    fn ptr_info_from_full(ft: &FullType) -> (CType, usize) {
        match ft {
            FullType::Scalar(t) => (*t, 1),
            FullType::Pointer(inner) => {
                let (base, depth) = Self::ptr_info_from_full(inner);
                (base, depth + 1)
            }
            FullType::Array { elem, .. } => {
                // Pointer to array: base is element's scalar type
                let base_ct = elem.to_ctype();
                (base_ct, 1)
            }
            FullType::Struct(_) => (CType::Struct, 1),
        }
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

    /// Get the byte size of an expression's type (for sizeof) without evaluating it
    fn sizeof_exp(&self, exp: &Exp) -> usize {
        match exp {
            Exp::Constant(_) => 4, // int
            Exp::LongConstant(_) => 8,
            Exp::UIntConstant(_) => 4,
            Exp::ULongConstant(_) => 8,
            Exp::DoubleConstant(_) => 8,
            Exp::StringLiteral(s) => s.len() + 1, // char array including null
            Exp::Var(name) => {
                // Don't decay arrays for sizeof
                let ft = self.get_full_type(name);
                ft.byte_size_with(&self.struct_defs)
            }
            Exp::Cast(ct, ft, _) => {
                if let Some(ref full) = ft {
                    full.byte_size()
                } else {
                    ct.size() as usize
                }
            }
            Exp::Unary(UnaryOp::Deref, inner) => {
                // *ptr: need to figure out what ptr points to
                let inner_size = self.sizeof_exp(inner);
                // The type of *ptr is the pointee type
                // For sizeof, we need to compute the expression type
                match inner.as_ref() {
                    Exp::Var(name) => {
                        let ft = self.get_full_type(name);
                        match ft {
                            FullType::Pointer(inner_ft) => inner_ft.byte_size(),
                            _ => 4, // fallback
                        }
                    }
                    _ => 8, // fallback to pointer size
                }
            }
            Exp::Subscript(arr, _) => {
                // arr[i]: element type
                match arr.as_ref() {
                    Exp::Var(name) => {
                        let ft = self.get_full_type(name);
                        match ft {
                            FullType::Array { elem, .. } => elem.byte_size(),
                            FullType::Pointer(inner) => inner.byte_size(),
                            _ => 4,
                        }
                    }
                    _ => 4,
                }
            }
            Exp::FunctionCall(name, _) => {
                let ret_type = self.func_types.get(name)
                    .map(|(rt, _, _)| *rt).unwrap_or(CType::Int);
                std::cmp::max(ret_type.size() as usize, 1)
            }
            Exp::SizeOf(_) | Exp::SizeOfType(_, _) => 8, // sizeof returns unsigned long
            Exp::Unary(UnaryOp::AddrOf, _) => 8, // &x is a pointer
            Exp::Unary(UnaryOp::PreIncrement | UnaryOp::PreDecrement | UnaryOp::PostIncrement | UnaryOp::PostDecrement, inner) => {
                self.sizeof_exp(inner)
            }
            Exp::Unary(UnaryOp::Negate | UnaryOp::Complement, inner) => {
                // promoted result
                std::cmp::max(self.sizeof_exp(inner), 4)
            }
            Exp::Unary(UnaryOp::LogicalNot, _) => 4, // !x is always int
            Exp::Binary(op, left, right) => {
                if matches!(op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr |
                    BinaryOp::Equal | BinaryOp::NotEqual |
                    BinaryOp::LessThan | BinaryOp::GreaterThan |
                    BinaryOp::LessEqual | BinaryOp::GreaterEqual) {
                    4 // comparisons return int
                } else if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
                    // Shift result type is promoted left operand type
                    std::cmp::max(self.sizeof_exp(left), 4) // promoted to at least int
                } else {
                    // Common type of both operands (promoted)
                    let l = std::cmp::max(self.sizeof_exp(left), 4);
                    let r = std::cmp::max(self.sizeof_exp(right), 4);
                    std::cmp::max(l, r)
                }
            }
            Exp::Dot(inner, member) => {
                // sizeof(x.member) — get member's type size
                if let Exp::Var(name) = inner.as_ref() {
                    let ft = self.get_full_type(name);
                    if let FullType::Struct(tag) = &ft {
                        if let Some(def) = self.struct_defs.get(tag) {
                            if let Some(mem) = def.find_member(&member) {
                                return mem.member_full_type.byte_size_with(&self.struct_defs);
                            }
                        }
                    }
                }
                4 // fallback
            }
            Exp::Arrow(_, member) => 4, // fallback
            Exp::Assign(left, _) => self.sizeof_exp(left),
            Exp::CompoundAssign(_, left, _) => self.sizeof_exp(left),
            Exp::Conditional(_, then_e, else_e) => {
                // Result type is common type of both branches
                let t_size = self.sizeof_exp(then_e);
                let e_size = self.sizeof_exp(else_e);
                std::cmp::max(t_size, e_size)
            }
            _ => 4, // default to int
        }
    }

    fn static_init_size(v: &StaticInit) -> usize {
        match v {
            StaticInit::CharInit(_) | StaticInit::UCharInit(_) => 1,
            StaticInit::IntInit(_) | StaticInit::UIntInit(_) => 4,
            StaticInit::LongInit(_) | StaticInit::ULongInit(_) | StaticInit::DoubleInit(_) | StaticInit::PointerInit(_) => 8,
            StaticInit::ZeroInit(n) => *n,
            StaticInit::StringInit(s, null_terminated) => s.len() + if *null_terminated { 1 } else { 0 },
        }
    }

    /// Create a constant string in read-only data and return its label name
    fn make_string_constant(&mut self, s: &str) -> String {
        let label = format!("__string_const_{}", self.string_counter);
        self.string_counter += 1;
        let size = s.len() + 1; // including null terminator
        let ft = FullType::Array { elem: Box::new(FullType::Scalar(CType::Char)), size };
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
            if depth <= 1 { base } else { CType::Pointer }
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
            TackyVal::Var(name) => *self.symbol_types.get(name)
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
        let dst = self.fresh_tmp(to);

        // Handle double conversions
        if to == CType::Double && from != CType::Double {
            if from.is_signed() {
                // Signed int/long → double: cvtsi2sd
                self.emit(TackyInstr::IntToDouble { src: val, dst: dst.clone() });
            } else {
                // Unsigned → double: need special handling
                self.emit(TackyInstr::UIntToDouble { src: val, dst: dst.clone() });
            }
            return dst;
        }
        if from == CType::Double && to != CType::Double {
            if to.is_signed() {
                self.emit(TackyInstr::DoubleToInt { src: val, dst: dst.clone() });
            } else {
                self.emit(TackyInstr::DoubleToUInt { src: val, dst: dst.clone() });
            }
            return dst;
        }

        let from_size = from.size();
        let to_size = to.size();

        if from_size == to_size {
            self.emit(TackyInstr::Copy { src: val, dst: dst.clone() });
        } else if from_size < to_size {
            if from.is_signed() {
                self.emit(TackyInstr::SignExtend { src: val, dst: dst.clone() });
            } else {
                self.emit(TackyInstr::ZeroExtend { src: val, dst: dst.clone() });
            }
        } else {
            self.emit(TackyInstr::Truncate { src: val, dst: dst.clone() });
        }
        dst
    }

    // --------------------------------------------------------
    // Expression emission
    // --------------------------------------------------------

    /// Emit an expression. Returns (value, type).
    fn emit_exp(&mut self, exp: Exp) -> (TackyVal, CType) {
        match exp {
            Exp::Constant(val) => (TackyVal::Constant(val), CType::Int),
            Exp::LongConstant(val) => {
                let dst = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Copy { src: TackyVal::Constant(val), dst: dst.clone() });
                (dst, CType::Long)
            }
            Exp::UIntConstant(val) => {
                let dst = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Copy { src: TackyVal::Constant(val), dst: dst.clone() });
                (dst, CType::UInt)
            }
            Exp::ULongConstant(val) => {
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy { src: TackyVal::Constant(val), dst: dst.clone() });
                (dst, CType::ULong)
            }
            Exp::DoubleConstant(val) => {
                let dst = self.fresh_tmp(CType::Double);
                self.emit(TackyInstr::Copy { src: TackyVal::DoubleConstant(val), dst: dst.clone() });
                (dst, CType::Double)
            }
            Exp::SizeOfType(_ct, ft) => {
                let size = ft.byte_size_with(&self.struct_defs) as i64;
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy { src: TackyVal::Constant(size), dst: dst.clone() });
                (dst, CType::ULong)
            }
            Exp::SizeOf(inner) => {
                // sizeof does NOT evaluate the expression — just gets its type's size
                let size = self.sizeof_exp(&inner) as i64;
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy { src: TackyVal::Constant(size), dst: dst.clone() });
                (dst, CType::ULong)
            }
            Exp::StringLiteral(s) => {
                // String literal in expression context: create constant string, decay to pointer
                let label = self.make_string_constant(&s);
                let decayed_ft = FullType::Pointer(Box::new(FullType::Scalar(CType::Char)));
                let ptr = self.fresh_tmp_full(&decayed_ft);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(label),
                    dst: ptr.clone(),
                });
                (ptr, CType::Pointer)
            }
            Exp::Var(name) => {
                let ft = self.get_full_type(&name);
                // Array-to-pointer decay: arrays decay to pointer to first element
                if ft.is_array() {
                    let decayed = ft.decay(); // FullType::Pointer(elem)
                    let ptr = self.fresh_tmp_full(&decayed);
                    self.emit(TackyInstr::GetAddress {
                        src: TackyVal::Var(name),
                        dst: ptr.clone(),
                    });
                    return (ptr, decayed.to_ctype());
                }
                let t = ft.to_ctype();
                (TackyVal::Var(name), t)
            }
            Exp::Cast(target_type, cast_ft, inner) => {
                let (val, from_type) = self.emit_exp(*inner);
                let converted = self.convert_to(val, from_type, target_type);
                // Propagate FullType from cast (e.g. (char *) preserves pointer-to-char info)
                if let Some(ft) = cast_ft {
                    // If convert_to returned the same value (no actual conversion),
                    // create a copy to avoid overwriting the source variable's type
                    let result = if from_type == target_type {
                        let copy = self.fresh_tmp_full(&ft);
                        self.emit(TackyInstr::Copy { src: converted, dst: copy.clone() });
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
                    return (result, target_type);
                }
                (converted, target_type)
            }
            Exp::Unary(op, inner) => self.emit_unary(op, *inner),
            Exp::Binary(BinaryOp::LogicalAnd, left, right) => {
                (self.emit_logical_and(*left, *right), CType::Int)
            }
            Exp::Binary(BinaryOp::LogicalOr, left, right) => {
                (self.emit_logical_or(*left, *right), CType::Int)
            }
            Exp::Binary(op, left, right) => self.emit_binary(op, *left, *right),
            Exp::Assign(left, right) => {
                // Check if LHS is a subscript: a[i] = value
                if let Exp::Subscript(arr, idx) = *left {
                    let (ptr, elem_type, _elem_ft) = self.emit_subscript_addr(*arr, *idx);
                    let (rhs, rhs_type) = self.emit_exp(*right);
                    let rhs_conv = self.convert_to(rhs, rhs_type, elem_type);
                    self.emit(TackyInstr::Store { src: rhs_conv.clone(), dst_ptr: ptr });
                    return (rhs_conv, elem_type);
                }
                // Check if LHS is struct member access: x.member = value (including nested)
                if let Exp::Dot(ref inner_exp, ref member) = *left {
                    let member_addr = self.emit_dot_address(&left);
                    let mem_type = self.lvalue_type(&left);
                    let mem_ft = self.dot_member_full_type(&left);
                    let (rhs, rhs_type) = self.emit_exp(*right);
                    if mem_ft.is_struct() {
                        // Struct-to-struct copy through dot
                        let struct_size = mem_ft.byte_size_with(&self.struct_defs);
                        let src_addr = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress { src: rhs, dst: src_addr.clone() });
                        self.emit_struct_copy_ptr_to_ptr(src_addr, member_addr, struct_size);
                        return (TackyVal::Constant(0), CType::Struct);
                    }
                    let rhs_conv = self.convert_to(rhs, rhs_type, mem_type);
                    self.emit(TackyInstr::Store { src: rhs_conv.clone(), dst_ptr: member_addr });
                    return (rhs_conv, mem_type);
                }
                // Check if LHS is ptr->member = value
                if let Exp::Arrow(ref inner_exp, ref member) = *left {
                    let member_addr = self.emit_dot_address(&left);
                    let mem_type = self.lvalue_type(&left);
                    let mem_ft = self.dot_member_full_type(&left);
                    let (rhs, rhs_type) = self.emit_exp(*right);
                    if mem_ft.is_struct() {
                        let struct_size = mem_ft.byte_size_with(&self.struct_defs);
                        let src_addr = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress { src: rhs, dst: src_addr.clone() });
                        self.emit_struct_copy_ptr_to_ptr(src_addr, member_addr, struct_size);
                        return (TackyVal::Constant(0), CType::Struct);
                    }
                    let rhs_conv = self.convert_to(rhs, rhs_type, mem_type);
                    self.emit(TackyInstr::Store { src: rhs_conv.clone(), dst_ptr: member_addr });
                    return (rhs_conv, mem_type);
                }
                // Check if LHS is a dereference: *ptr = value
                if let Exp::Unary(UnaryOp::Deref, ptr_exp) = *left {
                    let (ptr, _) = self.emit_exp(*ptr_exp);
                    let ptr_ft = self.val_full_type(&ptr);
                    // Check if pointee is a struct — need struct copy
                    if let FullType::Pointer(ref inner) = ptr_ft {
                        if let FullType::Struct(ref tag) = **inner {
                            let struct_size = self.struct_defs.get(tag).map(|d| d.size).unwrap_or(0);
                            let (rhs, _) = self.emit_exp(*right);
                            let src_addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress { src: rhs, dst: src_addr.clone() });
                            self.emit_struct_copy_ptr_to_ptr(src_addr, ptr, struct_size);
                            return (TackyVal::Constant(0), CType::Struct);
                        }
                    }
                    let pointee_type = if let TackyVal::Var(ref name) = ptr {
                        self.deref_type(name)
                    } else { CType::Int };
                    let (rhs, rhs_type) = self.emit_exp(*right);
                    let rhs_conv = self.convert_to(rhs, rhs_type, pointee_type);
                    self.emit(TackyInstr::Store { src: rhs_conv.clone(), dst_ptr: ptr });
                    return (rhs_conv, pointee_type);
                }
                // Check for struct assignment: struct_var = other_struct
                if let Exp::Var(ref lhs_name) = *left {
                    let lhs_ft = self.get_full_type(lhs_name);
                    if let FullType::Struct(ref tag) = lhs_ft {
                        let struct_size = self.struct_defs.get(tag).map(|d| d.size).unwrap_or(0);
                        let (rhs, rhs_type) = self.emit_exp(*right);
                        let src_addr = if rhs_type == CType::Pointer || rhs_type == CType::Struct {
                            // RHS is already a pointer (from Dot/Arrow/Deref) or a struct var
                            let rhs_ft = self.val_full_type(&rhs);
                            if rhs_ft.is_struct() {
                                let addr = self.fresh_tmp(CType::Pointer);
                                self.emit(TackyInstr::GetAddress { src: rhs, dst: addr.clone() });
                                addr
                            } else {
                                // Already a pointer to struct data
                                rhs
                            }
                        } else {
                            let addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress { src: rhs, dst: addr.clone() });
                            addr
                        };
                        self.emit_struct_copy_to(src_addr, lhs_name, struct_size);
                        return (TackyVal::Var(lhs_name.clone()), CType::Struct);
                    }
                }
                let lhs_type = self.lvalue_type(&left);
                let (rhs, rhs_type) = self.emit_exp(*right);
                let rhs_conv = self.convert_to(rhs, rhs_type, lhs_type);
                let lhs = self.emit_lvalue(*left);
                // If assigning a pointer, propagate pointee type
                // Only propagate FullType from RHS if LHS doesn't have a specific array-pointer type
                if lhs_type == CType::Pointer {
                    if let TackyVal::Var(ref lhs_name) = lhs {
                        if let TackyVal::Var(ref rhs_name) = rhs_conv {
                            if let Some(&info) = self.ptr_info.get(rhs_name) {
                                self.ptr_info.insert(lhs_name.clone(), info);
                            }
                            // Only propagate FullType if LHS doesn't have a specific declared type
                            let lhs_has_specific = self.full_types.get(lhs_name)
                                .map(|ft| matches!(ft, FullType::Pointer(inner) if inner.is_array() || inner.is_struct()))
                                .unwrap_or(false);
                            if !lhs_has_specific {
                                if let Some(ft) = self.full_types.get(rhs_name).cloned() {
                                    self.full_types.insert(lhs_name.clone(), ft);
                                }
                            }
                        }
                    }
                }
                self.emit(TackyInstr::Copy { src: rhs_conv, dst: lhs.clone() });
                (lhs, lhs_type)
            }
            Exp::CompoundAssign(op, left, right) => {
                // Handle compound assign through subscript: a[i] += val
                if let Exp::Subscript(arr, idx) = *left {
                    let (ptr, elem_type, elem_full) = self.emit_subscript_addr(*arr, *idx);
                    let cur_val = self.fresh_tmp_full(&elem_full);
                    self.emit(TackyInstr::Load { src_ptr: ptr.clone(), dst: cur_val.clone() });
                    let (rhs, rhs_type) = self.emit_exp(*right);
                    // For pointer compound assign (ptr += n), use ptr_elem_size for scaling
                    if elem_type == CType::Pointer && matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                        let elem_size = match &elem_full {
                            FullType::Pointer(inner) => inner.byte_size() as i64,
                            _ => 1,
                        };
                        let rhs_long = self.convert_to(rhs, rhs_type, CType::Long);
                        let scaled = if elem_size > 1 {
                            let s = self.fresh_tmp(CType::Long);
                            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Mul, left: rhs_long, right: TackyVal::Constant(elem_size), dst: s.clone() });
                            s
                        } else { rhs_long };
                        let result = self.fresh_tmp_full(&elem_full);
                        let tacky_op = Self::convert_binop(op);
                        self.emit(TackyInstr::Binary { op: tacky_op, left: cur_val, right: scaled, dst: result.clone() });
                        self.emit(TackyInstr::Store { src: result.clone(), dst_ptr: ptr });
                        return (result, elem_type);
                    }
                    let common = CType::common(elem_type, rhs_type);
                    let lhs_conv = self.convert_to(cur_val, elem_type, common);
                    let rhs_conv = self.convert_to(rhs, rhs_type, common);
                    let result = self.fresh_tmp(common);
                    let tacky_op = Self::convert_binop(op);
                    self.emit(TackyInstr::Binary { op: tacky_op, left: lhs_conv, right: rhs_conv, dst: result.clone() });
                    let result_conv = self.convert_to(result, common, elem_type);
                    self.emit(TackyInstr::Store { src: result_conv.clone(), dst_ptr: ptr });
                    return (result_conv, elem_type);
                }
                // Handle compound assign through dereference: *ptr += val
                if let Exp::Unary(UnaryOp::Deref, ptr_exp) = *left {
                    let (ptr, _) = self.emit_exp(*ptr_exp);
                    let pointee_type = if let TackyVal::Var(ref name) = ptr {
                        self.deref_type(name)
                    } else { CType::Int };
                    // Load current value through pointer
                    let cur_val = self.fresh_tmp(pointee_type);
                    self.emit(TackyInstr::Load { src_ptr: ptr.clone(), dst: cur_val.clone() });
                    let (rhs, rhs_type) = self.emit_exp(*right);
                    let common = CType::common(pointee_type, rhs_type);
                    let lhs_conv = self.convert_to(cur_val, pointee_type, common);
                    let rhs_conv = self.convert_to(rhs, rhs_type, common);
                    let result = self.fresh_tmp(common);
                    let tacky_op = Self::convert_binop(op);
                    self.emit(TackyInstr::Binary { op: tacky_op, left: lhs_conv, right: rhs_conv, dst: result.clone() });
                    let result_conv = self.convert_to(result, common, pointee_type);
                    self.emit(TackyInstr::Store { src: result_conv.clone(), dst_ptr: ptr });
                    return (result_conv, pointee_type);
                }

                let lhs_type = self.lvalue_type(&left);
                let lhs = self.emit_lvalue(*left);
                let (rhs, rhs_type) = self.emit_exp(*right);

                // Pointer compound assignment: ptr += n → ptr += n * sizeof(*ptr)
                if lhs_type == CType::Pointer && matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                    let elem_size = if let TackyVal::Var(ref n) = lhs {
                        self.ptr_elem_size(n)
                    } else { 1 };
                    let rhs_long = self.convert_to(rhs, rhs_type, CType::Long);
                    let scaled = if elem_size > 1 {
                        let s = self.fresh_tmp(CType::Long);
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Mul, left: rhs_long, right: TackyVal::Constant(elem_size), dst: s.clone()
                        });
                        s
                    } else { rhs_long };
                    let lhs_ft = self.val_full_type(&lhs);
                    let dst = self.fresh_tmp_full(&lhs_ft);
                    let tacky_op = Self::convert_binop(op);
                    self.emit(TackyInstr::Binary { op: tacky_op, left: lhs.clone(), right: scaled, dst: dst.clone() });
                    if let TackyVal::Var(ref vn) = lhs {
                        if let Some(&info) = self.ptr_info.get(vn) {
                            if let TackyVal::Var(ref dn) = dst {
                                self.ptr_info.insert(dn.clone(), info);
                            }
                        }
                    }
                    self.emit(TackyInstr::Copy { src: dst, dst: lhs.clone() });
                    return (lhs, lhs_type);
                }

                let is_shift = matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
                let (lhs_conv, rhs_conv, result_type) = if is_shift {
                    (lhs.clone(), rhs, lhs_type)
                } else {
                    let common = CType::common(lhs_type, rhs_type);
                    let lc = self.convert_to(lhs.clone(), lhs_type, common);
                    let rc = self.convert_to(rhs, rhs_type, common);
                    let rt = if is_comparison_op(&op) { CType::Int } else { common };
                    (lc, rc, rt)
                };

                let dst = self.fresh_tmp(result_type);
                let tacky_op = Self::convert_binop(op);
                self.emit(TackyInstr::Binary { op: tacky_op, left: lhs_conv, right: rhs_conv, dst: dst.clone() });
                let dst_conv = self.convert_to(dst, result_type, lhs_type);
                self.emit(TackyInstr::Copy { src: dst_conv, dst: lhs.clone() });
                (lhs, lhs_type)
            }
            Exp::Conditional(cond, then_exp, else_exp) => {
                let (cond_val, _) = self.emit_exp(*cond);
                let else_label = self.fresh_label("cond_else");
                let end_label = self.fresh_label("cond_end");
                self.emit(TackyInstr::JumpIfZero(cond_val, else_label.clone()));
                let (then_val, then_type) = self.emit_exp(*then_exp);
                // Handle void ternary
                if then_type == CType::Void {
                    self.emit(TackyInstr::Jump(end_label.clone()));
                    self.emit(TackyInstr::Label(else_label));
                    let (_else_val, _else_type) = self.emit_exp(*else_exp);
                    self.emit(TackyInstr::Label(end_label));
                    return (TackyVal::Constant(0), CType::Void);
                }
                let then_tmp = self.fresh_tmp(then_type);
                self.emit(TackyInstr::Copy { src: then_val, dst: then_tmp.clone() });
                self.emit(TackyInstr::Jump(end_label.clone()));
                self.emit(TackyInstr::Label(else_label));
                let (else_val, else_type) = self.emit_exp(*else_exp);
                let common = CType::common(then_type, else_type);
                let result = self.fresh_tmp(common);
                let else_conv = self.convert_to(else_val, else_type, common);
                self.emit(TackyInstr::Copy { src: else_conv, dst: result.clone() });
                let end2_label = self.fresh_label("cond_end2");
                self.emit(TackyInstr::Jump(end2_label.clone()));
                self.emit(TackyInstr::Label(end_label));
                let then_conv = self.convert_to(then_tmp, then_type, common);
                self.emit(TackyInstr::Copy { src: then_conv, dst: result.clone() });
                self.emit(TackyInstr::Label(end2_label));
                (result, common)
            }
            Exp::FunctionCall(name, args) => {
                let (ret_type, param_types, ret_pi) = self.func_types.get(&name)
                    .cloned()
                    .unwrap_or((CType::Int, Vec::new(), None));
                let mut tacky_args = Vec::new();
                for (i, arg) in args.into_iter().enumerate() {
                    let (val, val_type) = self.emit_exp(arg);
                    let expected = param_types.get(i).copied().unwrap_or(val_type);
                    let conv = self.convert_to(val, val_type, expected);
                    tacky_args.push(conv);
                }
                // Use return FullType if available
                let ret_ft = self.func_full_types.get(&name).cloned();
                let dst = if let Some(ref rft) = ret_ft {
                    self.fresh_tmp_full(rft)
                } else {
                    self.fresh_tmp(ret_type)
                };
                self.emit(TackyInstr::FunCall { name, args: tacky_args, dst: dst.clone() });
                // Propagate return pointer info
                if let Some(pi) = ret_pi {
                    if let TackyVal::Var(ref dst_name) = dst {
                        self.ptr_info.insert(dst_name.clone(), pi);
                    }
                }
                (dst, ret_type)
            }
            Exp::Subscript(arr, idx) => {
                // a[i] ≡ *(a + i) ≡ i[a]
                let (first_val, first_ctype) = self.emit_exp(*arr);
                let (second_val, second_type) = self.emit_exp(*idx);

                // Normalize: pointer first, index second
                let first_full = self.val_full_type(&first_val);
                let (arr_val, idx_val, idx_type, arr_full) = if first_full.is_pointer() || first_ctype == CType::Pointer {
                    (first_val, second_val, second_type, first_full)
                } else {
                    let second_full = self.val_full_type(&second_val);
                    (second_val, first_val, first_ctype, second_full)
                };
                let (elem_full, scale) = match &arr_full {
                    FullType::Pointer(inner) => (inner.as_ref().clone(), inner.byte_size() as i64),
                    _ => (FullType::Scalar(CType::Int), 4), // fallback
                };

                // Compute pointer to element using AddPtr
                let idx_long = self.convert_to(idx_val, idx_type, CType::Long);
                let result_ptr_type = FullType::Pointer(Box::new(elem_full.clone()));
                let ptr = self.fresh_tmp_full(&result_ptr_type);
                self.emit(TackyInstr::AddPtr {
                    ptr: arr_val,
                    index: idx_long,
                    scale,
                    dst: ptr.clone(),
                });

                // If element is an array, it decays to a pointer (no load needed)
                if elem_full.is_array() {
                    // The result is a pointer to the sub-array's first element
                    // No Load — the address IS the result after decay
                    let decayed = elem_full.decay();
                    let decayed_ptr = self.fresh_tmp_full(&decayed);
                    // ptr already points to the start of the sub-array
                    // A "decay" here just reinterprets the pointer type
                    self.emit(TackyInstr::Copy { src: ptr, dst: decayed_ptr.clone() });
                    return (decayed_ptr, decayed.to_ctype());
                }

                // For scalar/pointer elements, load the value
                let elem_ctype = elem_full.to_ctype();
                let result = self.fresh_tmp_full(&elem_full);
                self.emit(TackyInstr::Load { src_ptr: ptr, dst: result.clone() });
                (result, elem_ctype)
            }
            Exp::ArrayInit(elems) => {
                // Array initializer — this is handled during variable declaration, not standalone
                panic!("Array initializer not allowed in expression context");
            }
            Exp::Dot(inner, member) => {
                // Determine struct address and tag
                let (struct_addr, tag) = if let Exp::Var(ref n) = *inner {
                    let ft = self.get_full_type(n);
                    let tag = match &ft {
                        FullType::Struct(t) => t.clone(),
                        _ => panic!("Dot on non-struct type: {:?}", ft),
                    };
                    let addr = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::GetAddress { src: TackyVal::Var(n.clone()), dst: addr.clone() });
                    (addr, tag)
                } else if let Exp::Unary(UnaryOp::Deref, ref ptr_exp) = *inner {
                    // (*ptr).member — get the pointer, dereference gives struct
                    let (ptr, _) = self.emit_exp(*ptr_exp.clone());
                    let ptr_ft = self.val_full_type(&ptr);
                    let tag = match &ptr_ft {
                        FullType::Pointer(inner_ft) => match inner_ft.as_ref() {
                            FullType::Struct(t) => t.clone(),
                            _ => panic!("Deref.Dot on non-struct-pointer"),
                        },
                        _ => panic!("Deref.Dot on non-pointer"),
                    };
                    (ptr, tag)
                } else {
                    // Complex expression (e.g., another Dot/Arrow returning struct pointer)
                    let (val, val_type) = self.emit_exp(*inner);
                    let val_ft = self.val_full_type(&val);
                    // If the expression returned a pointer to a struct member
                    let tag = match &val_ft {
                        FullType::Struct(t) => {
                            let addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress { src: val, dst: addr.clone() });
                            return self.access_struct_member(addr, t.clone(), &member);
                        }
                        FullType::Pointer(inner) => match inner.as_ref() {
                            FullType::Struct(t) => t.clone(),
                            _ => panic!("Dot on non-struct result: {:?}", val_ft),
                        },
                        _ => panic!("Dot on non-struct result: {:?}", val_ft),
                    };
                    (val, tag)
                };
                self.access_struct_member(struct_addr, tag, &member)
            }
            Exp::Arrow(inner, member) => {
                let (ptr_val, _) = self.emit_exp(*inner);
                let ptr_ft = self.val_full_type(&ptr_val);
                let tag = match &ptr_ft {
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => panic!("Arrow on non-struct-pointer: {:?}", ptr_ft),
                    },
                    _ => panic!("Arrow on non-pointer: {:?}", ptr_ft),
                };
                self.access_struct_member(ptr_val, tag, &member)
            }
        }
    }

    fn lvalue_type(&self, exp: &Exp) -> CType {
        match exp {
            Exp::Var(name) => self.var_types.get(name).copied()
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
            Exp::Dot(inner, member) => {
                if let Exp::Var(name) = inner.as_ref() {
                    let ft = self.get_full_type(name);
                    if let FullType::Struct(tag) = &ft {
                        if let Some(def) = self.struct_defs.get(tag) {
                            if let Some(mem) = def.find_member(member) {
                                return mem.member_type;
                            }
                        }
                    }
                }
                CType::Int
            }
            Exp::Arrow(inner, member) => {
                if let Exp::Var(name) = inner.as_ref() {
                    let ft = self.get_full_type(name);
                    if let FullType::Pointer(inner_ft) = &ft {
                        if let FullType::Struct(tag) = inner_ft.as_ref() {
                            if let Some(def) = self.struct_defs.get(tag) {
                                if let Some(mem) = def.find_member(member) {
                                    return mem.member_type;
                                }
                            }
                        }
                    }
                }
                CType::Int
            }
            _ => CType::Int,
        }
    }

    fn emit_lvalue(&self, exp: Exp) -> TackyVal {
        match exp {
            Exp::Var(name) => TackyVal::Var(name),
            _ => panic!("Expression is not a simple lvalue"),
        }
    }

    /// Compute the address of a Dot/Arrow lvalue expression
    fn emit_dot_address(&mut self, exp: &Exp) -> TackyVal {
        match exp {
            Exp::Dot(inner, member) => {
                let base_addr = if let Exp::Var(n) = inner.as_ref() {
                    let addr = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::GetAddress { src: TackyVal::Var(n.clone()), dst: addr.clone() });
                    addr
                } else if let Exp::Dot(_, _) = inner.as_ref() {
                    self.emit_dot_address(inner)
                } else if let Exp::Arrow(ptr_exp, mem) = inner.as_ref() {
                    let (ptr, _) = self.emit_exp((**ptr_exp).clone());
                    let ptr_ft = self.val_full_type(&ptr);
                    let tag = match &ptr_ft { FullType::Pointer(inner) => match inner.as_ref() { FullType::Struct(t) => t.clone(), _ => panic!("") }, _ => panic!("") };
                    let def = self.struct_defs.get(&tag).cloned().unwrap();
                    let m = def.find_member(mem).unwrap();
                    let result = self.fresh_tmp(CType::Pointer);
                    if m.offset > 0 {
                        self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: ptr, right: TackyVal::Constant(m.offset as i64), dst: result.clone() });
                    } else {
                        self.emit(TackyInstr::Copy { src: ptr, dst: result.clone() });
                    }
                    result
                } else {
                    let (val, _) = self.emit_exp((**inner).clone());
                    let addr = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::GetAddress { src: val, dst: addr.clone() });
                    addr
                };
                // Get the struct tag from the inner expression's type
                let tag = self.dot_inner_tag(inner);
                let def = self.struct_defs.get(&tag).cloned().unwrap();
                let mem = def.find_member(member).unwrap();
                let result = self.fresh_tmp(CType::Pointer);
                if mem.offset > 0 {
                    self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: base_addr, right: TackyVal::Constant(mem.offset as i64), dst: result.clone() });
                } else {
                    self.emit(TackyInstr::Copy { src: base_addr, dst: result.clone() });
                }
                result
            }
            Exp::Arrow(inner, member) => {
                let (ptr, _) = self.emit_exp((**inner).clone());
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
                            panic!("emit_dot_address Arrow: inner is {:?}, expected Struct", inner)
                        }
                    },
                    _ => panic!("emit_dot_address Arrow: ft is {:?}, expected Pointer. Consider adding cast_ft in the cast expression.", ptr_ft)
                };
                let def = self.struct_defs.get(&tag).cloned().unwrap();
                let mem = def.find_member(member).unwrap();
                let result = self.fresh_tmp(CType::Pointer);
                if mem.offset > 0 {
                    self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: ptr, right: TackyVal::Constant(mem.offset as i64), dst: result.clone() });
                } else {
                    self.emit(TackyInstr::Copy { src: ptr, dst: result.clone() });
                }
                result
            }
            _ => panic!("emit_dot_address called on non-Dot/Arrow expression"),
        }
    }

    fn dot_inner_tag(&self, exp: &Exp) -> String {
        match exp {
            Exp::Var(n) => {
                let ft = self.get_full_type(n);
                match ft { FullType::Struct(t) => t, _ => panic!("Dot on non-struct var") }
            }
            Exp::Dot(inner, member) => {
                let parent_tag = self.dot_inner_tag(inner);
                let def = self.struct_defs.get(&parent_tag).unwrap();
                let mem = def.find_member(member).unwrap();
                match &mem.member_full_type { FullType::Struct(t) => t.clone(), _ => panic!("Nested dot on non-struct member") }
            }
            Exp::Arrow(inner, member) => {
                if let Exp::Var(n) = inner.as_ref() {
                    let ft = self.get_full_type(n);
                    if let FullType::Pointer(inner_ft) = ft {
                        if let FullType::Struct(t) = inner_ft.as_ref() {
                            let def = self.struct_defs.get(t).unwrap();
                            let mem = def.find_member(member).unwrap();
                            match &mem.member_full_type { FullType::Struct(t) => t.clone(), _ => panic!("") }
                        } else { panic!("") }
                    } else { panic!("") }
                } else { panic!("") }
            }
            _ => panic!("dot_inner_tag on non-struct expression"),
        }
    }

    fn dot_member_full_type(&self, exp: &Exp) -> FullType {
        match exp {
            Exp::Dot(inner, member) => {
                let tag = self.dot_inner_tag(inner);
                let def = self.struct_defs.get(&tag).unwrap();
                let mem = def.find_member(member).unwrap();
                mem.member_full_type.clone()
            }
            Exp::Arrow(inner, member) => {
                if let Exp::Var(n) = inner.as_ref() {
                    let ft = self.get_full_type(n);
                    if let FullType::Pointer(inner_ft) = ft {
                        if let FullType::Struct(t) = inner_ft.as_ref() {
                            let def = self.struct_defs.get(t).unwrap();
                            let mem = def.find_member(member).unwrap();
                            return mem.member_full_type.clone();
                        }
                    }
                }
                FullType::Scalar(CType::Int)
            }
            _ => FullType::Scalar(CType::Int),
        }
    }

    /// Access a struct member given the struct's base address
    fn access_struct_member(&mut self, struct_addr: TackyVal, tag: String, member: &str) -> (TackyVal, CType) {
        let def = self.struct_defs.get(&tag).cloned()
            .unwrap_or_else(|| panic!("Undefined struct: {}", tag));
        let mem = def.find_member(member)
            .unwrap_or_else(|| panic!("No member '{}' in struct {}", member, tag));
        let mem_type = mem.member_type;
        let mem_offset = mem.offset;
        let mem_ft = mem.member_full_type.clone();

        let mem_ptr = self.fresh_tmp(CType::Pointer);
        if mem_offset > 0 {
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: struct_addr, right: TackyVal::Constant(mem_offset as i64), dst: mem_ptr.clone() });
        } else {
            self.emit(TackyInstr::Copy { src: struct_addr, dst: mem_ptr.clone() });
        }

        if mem_ft.is_array() {
            // Array member: return pointer (decayed)
            let result_ft = FullType::Pointer(Box::new(match &mem_ft { FullType::Array { elem, .. } => *elem.clone(), _ => mem_ft.clone() }));
            let result = self.fresh_tmp_full(&result_ft);
            self.emit(TackyInstr::Copy { src: mem_ptr, dst: result.clone() });
            (result, CType::Pointer)
        } else if mem_ft.is_struct() {
            // Struct member: return pointer to it (not loaded)
            let result_ft = FullType::Pointer(Box::new(mem_ft));
            let result = self.fresh_tmp_full(&result_ft);
            self.emit(TackyInstr::Copy { src: mem_ptr, dst: result.clone() });
            (result, CType::Pointer)
        } else {
            // Scalar member: load the value
            let result = self.fresh_tmp_full(&mem_ft);
            self.emit(TackyInstr::Load { src_ptr: mem_ptr, dst: result.clone() });
            (result, mem_type)
        }
    }

    /// Emit a word-by-word struct copy from src address to dst name
    fn emit_struct_copy_to(&mut self, src_addr: TackyVal, dst_name: &str, struct_size: usize) {
        let mut off = 0usize;
        while off + 8 <= struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: src_addr.clone(), right: TackyVal::Constant(off as i64), dst: ptr.clone() });
            let tmp = self.fresh_tmp(CType::Long);
            self.emit(TackyInstr::Load { src_ptr: ptr, dst: tmp.clone() });
            self.emit(TackyInstr::CopyToOffset { src: tmp, dst_name: dst_name.to_string(), offset: off as i64 });
            off += 8;
        }
        while off + 4 <= struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: src_addr.clone(), right: TackyVal::Constant(off as i64), dst: ptr.clone() });
            let tmp = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Load { src_ptr: ptr, dst: tmp.clone() });
            self.emit(TackyInstr::CopyToOffset { src: tmp, dst_name: dst_name.to_string(), offset: off as i64 });
            off += 4;
        }
        while off < struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: src_addr.clone(), right: TackyVal::Constant(off as i64), dst: ptr.clone() });
            let tmp = self.fresh_tmp(CType::Char);
            self.emit(TackyInstr::Load { src_ptr: ptr, dst: tmp.clone() });
            self.emit(TackyInstr::CopyToOffset { src: tmp, dst_name: dst_name.to_string(), offset: off as i64 });
            off += 1;
        }
    }

    /// Emit struct copy from src address to dst address (both pointers)
    fn emit_struct_copy_ptr_to_ptr(&mut self, src_addr: TackyVal, dst_addr: TackyVal, struct_size: usize) {
        let mut off = 0usize;
        while off + 8 <= struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: src_addr.clone(), right: TackyVal::Constant(off as i64), dst: src_ptr.clone() });
            let tmp = self.fresh_tmp(CType::Long);
            self.emit(TackyInstr::Load { src_ptr: src_ptr, dst: tmp.clone() });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: dst_addr.clone(), right: TackyVal::Constant(off as i64), dst: dst_ptr.clone() });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr: dst_ptr });
            off += 8;
        }
        while off + 4 <= struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: src_addr.clone(), right: TackyVal::Constant(off as i64), dst: src_ptr.clone() });
            let tmp = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Load { src_ptr: src_ptr, dst: tmp.clone() });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: dst_addr.clone(), right: TackyVal::Constant(off as i64), dst: dst_ptr.clone() });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr: dst_ptr });
            off += 4;
        }
        while off < struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: src_addr.clone(), right: TackyVal::Constant(off as i64), dst: src_ptr.clone() });
            let tmp = self.fresh_tmp(CType::Char);
            self.emit(TackyInstr::Load { src_ptr: src_ptr, dst: tmp.clone() });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: dst_addr.clone(), right: TackyVal::Constant(off as i64), dst: dst_ptr.clone() });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr: dst_ptr });
            off += 1;
        }
    }

    /// Compute the address for a subscript expression a[i].
    /// Returns (pointer_to_element, element_type)
    /// Returns (pointer_to_element, element_ctype, element_full_type)
    fn emit_subscript_addr(&mut self, arr: Exp, idx: Exp) -> (TackyVal, CType, FullType) {
        let (first_val, first_type) = self.emit_exp(arr);
        let (second_val, second_type) = self.emit_exp(idx);

        // Normalize: pointer first, index second
        let first_full = self.val_full_type(&first_val);
        let (arr_val, idx_val, idx_type, arr_full) = if first_full.is_pointer() || first_type == CType::Pointer {
            (first_val, second_val, second_type, first_full)
        } else {
            let second_full = self.val_full_type(&second_val);
            (second_val, first_val, first_type, second_full)
        };
        let (elem_full, scale) = match &arr_full {
            FullType::Pointer(inner) => (inner.as_ref().clone(), inner.byte_size() as i64),
            _ => {
                // Fallback to old approach
                let elem_type = if let TackyVal::Var(ref name) = arr_val {
                    self.deref_type(name)
                } else { CType::Int };
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

        // Propagate pointee info (backward compat)
        if let TackyVal::Var(ref pname) = ptr {
            if let TackyVal::Var(ref aname) = arr_val {
                if let Some(info) = self.deref_info(aname) {
                    self.ptr_info.insert(pname.clone(), info);
                } else {
                    self.ptr_info.insert(pname.clone(), (elem_type, 1));
                }
            }
        }

        (ptr, elem_type, elem_full)
    }

    fn emit_unary(&mut self, op: UnaryOp, inner: Exp) -> (TackyVal, CType) {
        match op {
            UnaryOp::Negate | UnaryOp::Complement => {
                let (src, src_type) = self.emit_exp(inner);
                if src_type == CType::Double && matches!(op, UnaryOp::Negate) {
                    let dst = self.fresh_tmp(CType::Double);
                    self.emit(TackyInstr::Unary { op: TackyUnaryOp::Negate, src, dst: dst.clone() });
                    return (dst, CType::Double);
                }
                // Integer promotion: char types → int
                let promoted = src_type.promote();
                let src_conv = self.convert_to(src, src_type, promoted);
                let dst = self.fresh_tmp(promoted);
                let tacky_op = match op {
                    UnaryOp::Negate => TackyUnaryOp::Negate,
                    UnaryOp::Complement => TackyUnaryOp::Complement,
                    _ => unreachable!(),
                };
                self.emit(TackyInstr::Unary { op: tacky_op, src: src_conv, dst: dst.clone() });
                (dst, promoted)
            }
            UnaryOp::LogicalNot => {
                let (src, _) = self.emit_exp(inner);
                let dst = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Unary { op: TackyUnaryOp::LogicalNot, src, dst: dst.clone() });
                (dst, CType::Int)
            }
            UnaryOp::PreIncrement | UnaryOp::PreDecrement => {
                if let Exp::Subscript(arr, idx) = inner {
                    let (ptr, pt, _pt_ft) = self.emit_subscript_addr(*arr, *idx);
                    let cur = self.fresh_tmp(pt);
                    self.emit(TackyInstr::Load { src_ptr: ptr.clone(), dst: cur.clone() });
                    let one = self.convert_to(TackyVal::Constant(1), CType::Int, pt);
                    let result = self.fresh_tmp(pt);
                    let binop = if matches!(op, UnaryOp::PreIncrement) { TackyBinaryOp::Add } else { TackyBinaryOp::Sub };
                    self.emit(TackyInstr::Binary { op: binop, left: cur, right: one, dst: result.clone() });
                    self.emit(TackyInstr::Store { src: result.clone(), dst_ptr: ptr });
                    return (result, pt);
                }
                if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
                    let (ptr, _) = self.emit_exp(*ptr_exp);
                    let pt = if let TackyVal::Var(ref n) = ptr { self.deref_type(n) } else { CType::Int };
                    let cur = self.fresh_tmp(pt);
                    self.emit(TackyInstr::Load { src_ptr: ptr.clone(), dst: cur.clone() });
                    let one = self.convert_to(TackyVal::Constant(1), CType::Int, pt);
                    let result = self.fresh_tmp(pt);
                    let binop = if matches!(op, UnaryOp::PreIncrement) { TackyBinaryOp::Add } else { TackyBinaryOp::Sub };
                    self.emit(TackyInstr::Binary { op: binop, left: cur, right: one, dst: result.clone() });
                    self.emit(TackyInstr::Store { src: result.clone(), dst_ptr: ptr });
                    return (result, pt);
                }
                let var_type = self.lvalue_type(&inner);
                let var = self.emit_lvalue(inner);
                let increment = if var_type == CType::Pointer {
                    // Pointer increment: add sizeof(element)
                    let elem_size = if let TackyVal::Var(ref n) = var {
                        self.ptr_elem_size(n)
                    } else { 1 };
                    TackyVal::Constant(elem_size)
                } else {
                    let one = TackyVal::Constant(1);
                    self.convert_to(one, CType::Int, var_type)
                };
                let var_ft = self.val_full_type(&var);
                let dst = if var_type == CType::Pointer { self.fresh_tmp_full(&var_ft) } else { self.fresh_tmp(var_type) };
                let binop = if matches!(op, UnaryOp::PreIncrement) { TackyBinaryOp::Add } else { TackyBinaryOp::Sub };
                self.emit(TackyInstr::Binary { op: binop, left: var.clone(), right: increment, dst: dst.clone() });
                if var_type == CType::Pointer {
                    if let TackyVal::Var(ref vn) = var {
                        if let Some(&info) = self.ptr_info.get(vn) {
                            if let TackyVal::Var(ref dn) = dst {
                                self.ptr_info.insert(dn.clone(), info);
                            }
                        }
                    }
                }
                self.emit(TackyInstr::Copy { src: dst.clone(), dst: var });
                (dst, var_type)
            }
            UnaryOp::PostIncrement | UnaryOp::PostDecrement => {
                if let Exp::Subscript(arr, idx) = inner {
                    let (ptr, pt, _pt_ft) = self.emit_subscript_addr(*arr, *idx);
                    let old_val = self.fresh_tmp(pt);
                    self.emit(TackyInstr::Load { src_ptr: ptr.clone(), dst: old_val.clone() });
                    let one = self.convert_to(TackyVal::Constant(1), CType::Int, pt);
                    let new_val = self.fresh_tmp(pt);
                    let binop = if matches!(op, UnaryOp::PostIncrement) { TackyBinaryOp::Add } else { TackyBinaryOp::Sub };
                    self.emit(TackyInstr::Binary { op: binop, left: old_val.clone(), right: one, dst: new_val.clone() });
                    self.emit(TackyInstr::Store { src: new_val, dst_ptr: ptr });
                    return (old_val, pt);
                }
                if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
                    let (ptr, _) = self.emit_exp(*ptr_exp);
                    let pt = if let TackyVal::Var(ref n) = ptr { self.deref_type(n) } else { CType::Int };
                    let old_val = self.fresh_tmp(pt);
                    self.emit(TackyInstr::Load { src_ptr: ptr.clone(), dst: old_val.clone() });
                    let one = self.convert_to(TackyVal::Constant(1), CType::Int, pt);
                    let new_val = self.fresh_tmp(pt);
                    let binop = if matches!(op, UnaryOp::PostIncrement) { TackyBinaryOp::Add } else { TackyBinaryOp::Sub };
                    self.emit(TackyInstr::Binary { op: binop, left: old_val.clone(), right: one, dst: new_val.clone() });
                    self.emit(TackyInstr::Store { src: new_val, dst_ptr: ptr });
                    return (old_val, pt);
                }
                let var_type = self.lvalue_type(&inner);
                let var = self.emit_lvalue(inner);
                let var_ft = self.val_full_type(&var);
                let old_val = if var_type == CType::Pointer { self.fresh_tmp_full(&var_ft) } else { self.fresh_tmp(var_type) };
                self.emit(TackyInstr::Copy { src: var.clone(), dst: old_val.clone() });
                let increment = if var_type == CType::Pointer {
                    let elem_size = if let TackyVal::Var(ref n) = var {
                        self.ptr_elem_size(n)
                    } else { 1 };
                    TackyVal::Constant(elem_size)
                } else {
                    self.convert_to(TackyVal::Constant(1), CType::Int, var_type)
                };
                let new_val = self.fresh_tmp(var_type);
                let binop = if matches!(op, UnaryOp::PostIncrement) { TackyBinaryOp::Add } else { TackyBinaryOp::Sub };
                self.emit(TackyInstr::Binary { op: binop, left: var.clone(), right: increment, dst: new_val.clone() });
                self.emit(TackyInstr::Copy { src: new_val, dst: var });
                (old_val, var_type)
            }
            UnaryOp::AddrOf => {
                // &(*e) is just e
                if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
                    return self.emit_exp(*ptr_exp);
                }
                // &"string" — address of string literal (no decay)
                if let Exp::StringLiteral(s) = inner {
                    let label = self.make_string_constant(&s);
                    let str_size = s.len() + 1; // including null
                    let str_ft = FullType::Array { elem: Box::new(FullType::Scalar(CType::Char)), size: str_size };
                    let addr_ft = FullType::Pointer(Box::new(str_ft));
                    let dst = self.fresh_tmp_full(&addr_ft);
                    self.emit(TackyInstr::GetAddress {
                        src: TackyVal::Var(label),
                        dst: dst.clone(),
                    });
                    return (dst, CType::Pointer);
                }
                // &(s.member) — address of struct member
                if let Exp::Dot(inner_exp, member) = inner {
                    let struct_name = match *inner_exp {
                        Exp::Var(n) => n,
                        _ => { let (v, _) = self.emit_exp(*inner_exp); if let TackyVal::Var(n) = v { n } else { panic!("Dot on non-var") } }
                    };
                    let struct_ft = self.get_full_type(&struct_name);
                    let tag = match &struct_ft { FullType::Struct(t) => t.clone(), _ => panic!("Dot on non-struct") };
                    let def = self.struct_defs.get(&tag).cloned().unwrap();
                    let mem = def.find_member(&member).unwrap();
                    let mem_offset = mem.offset;
                    let struct_addr = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::GetAddress { src: TackyVal::Var(struct_name), dst: struct_addr.clone() });
                    let result = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: struct_addr, right: TackyVal::Constant(mem_offset as i64), dst: result.clone() });
                    return (result, CType::Pointer);
                }
                // &(p->member) — address of member through pointer
                if let Exp::Arrow(inner_exp, member) = inner {
                    let (ptr_val, _) = self.emit_exp(*inner_exp);
                    let ptr_ft = self.val_full_type(&ptr_val);
                    let tag = match &ptr_ft { FullType::Pointer(inner) => match inner.as_ref() { FullType::Struct(t) => t.clone(), _ => panic!("") }, _ => panic!("") };
                    let def = self.struct_defs.get(&tag).cloned().unwrap();
                    let mem = def.find_member(&member).unwrap();
                    let result = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::Binary { op: TackyBinaryOp::Add, left: ptr_val, right: TackyVal::Constant(mem.offset as i64), dst: result.clone() });
                    return (result, CType::Pointer);
                }
                // &(arr[i]) — address of subscripted element (also handles i[arr])
                if let Exp::Subscript(first, second) = inner {
                    // Try first as pointer, then second
                    let (ptr, _elem_type, _elem_ft2) = self.emit_subscript_addr(*first, *second);
                    return (ptr, CType::Pointer);
                }
                // &x — get address of variable
                let pointee_type = self.lvalue_type(&inner);
                let var = self.emit_lvalue(inner);
                // Set FullType: &x has type Pointer(type_of_x)
                let var_ft = self.val_full_type(&var);
                let addr_ft = FullType::Pointer(Box::new(var_ft));
                let dst = self.fresh_tmp_full(&addr_ft);
                if let TackyVal::Var(ref dst_name) = dst {
                    // Figure out the depth: if pointee is a pointer, we're adding one more level
                    if let Exp::Var(ref vname) = *Box::new(Exp::Constant(0)) {
                        // won't match — we lost the inner expression. Use a different approach:
                    }
                    // Simple approach: addr-of a pointer with depth N gives pointer with depth N+1
                    // addr-of a non-pointer gives pointer with depth 1
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
                self.emit(TackyInstr::GetAddress { src: var, dst: dst.clone() });
                (dst, CType::Pointer)
            }
            UnaryOp::Deref => {
                // *ptr — dereference pointer
                let (ptr, _) = self.emit_exp(inner);
                let _dbg_ft = self.val_full_type(&ptr);
                // Check if dereferencing produces an array type
                let ptr_full = self.val_full_type(&ptr);
                if let FullType::Pointer(ref inner_ft) = ptr_full {
                    if inner_ft.is_array() {
                        let decayed = inner_ft.decay();
                        let result = self.fresh_tmp_full(&decayed);
                        self.emit(TackyInstr::Copy { src: ptr, dst: result.clone() });
                        return (result, decayed.to_ctype());
                    }
                    if inner_ft.is_struct() {
                        // Dereferencing a pointer-to-struct: the struct lives at the pointer address
                        // Return the pointer as the "struct value" (similar to array treatment)
                        let result = self.fresh_tmp_full(inner_ft);
                        self.emit(TackyInstr::Copy { src: ptr, dst: result.clone() });
                        // Also register as aggregate for stack allocation
                        if let TackyVal::Var(ref name) = result {
                            if let FullType::Struct(ref tag) = **inner_ft {
                                if let Some(def) = self.struct_defs.get(tag) {
                                    self.array_sizes.insert(name.clone(), def.size);
                                }
                            }
                        }
                        return (result, CType::Struct);
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
                self.emit(TackyInstr::Load { src_ptr: ptr.clone(), dst: dst.clone() });
                // Propagate pointer info for multi-level indirection
                if pointee_type == CType::Pointer {
                    if let TackyVal::Var(ref ptr_name) = ptr {
                        if let Some(info) = self.deref_info(ptr_name) {
                            if let TackyVal::Var(ref dst_name) = dst {
                                self.ptr_info.insert(dst_name.clone(), info);
                            }
                        }
                    }
                }
                (dst, pointee_type)
            }
        }
    }

    fn emit_binary(&mut self, op: BinaryOp, left: Exp, right: Exp) -> (TackyVal, CType) {
        let (l, l_type) = self.emit_exp(left);
        let (r, r_type) = self.emit_exp(right);

        // Pointer arithmetic: ptr + int or int + ptr → scale int by elem size
        if matches!(op, BinaryOp::Add | BinaryOp::Sub) {
            let (is_ptr_arith, ptr_val, int_val, elem_size, int_type) =
                if l_type == CType::Pointer && !r_type.is_pointer() {
                    let es = if let TackyVal::Var(ref n) = l {
                        self.ptr_elem_size(n)
                    } else { 1 };
                    (true, l.clone(), r.clone(), es, r_type)
                } else if r_type == CType::Pointer && !l_type.is_pointer() && matches!(op, BinaryOp::Add) {
                    let es = if let TackyVal::Var(ref n) = r {
                        self.ptr_elem_size(n)
                    } else { 1 };
                    (true, r.clone(), l.clone(), es, l_type)
                } else {
                    (false, l.clone(), r.clone(), 1, r_type)
                };

            if is_ptr_arith && elem_size > 1 {
                let int_long = self.convert_to(int_val, int_type, CType::Long);
                let scaled = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Mul, left: int_long, right: TackyVal::Constant(elem_size), dst: scaled.clone()
                });
                // Propagate FullType from source pointer to result
                let ptr_ft = self.val_full_type(&ptr_val);
                let dst = self.fresh_tmp_full(&ptr_ft);
                let tacky_op = Self::convert_binop(op.clone());
                self.emit(TackyInstr::Binary { op: tacky_op, left: ptr_val.clone(), right: scaled, dst: dst.clone() });
                // Propagate ptr_info
                if let TackyVal::Var(ref pname) = ptr_val {
                    if let Some(&info) = self.ptr_info.get(pname) {
                        if let TackyVal::Var(ref dname) = dst {
                            self.ptr_info.insert(dname.clone(), info);
                        }
                    }
                }
                return (dst, CType::Pointer);
            } else if is_ptr_arith {
                let int_long = self.convert_to(int_val, int_type, CType::Long);
                let ptr_ft = self.val_full_type(&ptr_val);
                let dst = self.fresh_tmp_full(&ptr_ft);
                let tacky_op = Self::convert_binop(op);
                self.emit(TackyInstr::Binary { op: tacky_op, left: ptr_val.clone(), right: int_long, dst: dst.clone() });
                if let TackyVal::Var(ref pname) = ptr_val {
                    if let Some(&info) = self.ptr_info.get(pname) {
                        if let TackyVal::Var(ref dname) = dst {
                            self.ptr_info.insert(dname.clone(), info);
                        }
                    }
                }
                return (dst, CType::Pointer);
            }

            // ptr - ptr → difference divided by elem size
            if l_type == CType::Pointer && r_type == CType::Pointer && matches!(op, BinaryOp::Sub) {
                let raw_diff = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Sub, left: l, right: r, dst: raw_diff.clone()
                });
                let es = if let TackyVal::Var(ref n) = ptr_val {
                    self.ptr_elem_size(n)
                } else { 1 };
                if es > 1 {
                    let result = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Div, left: raw_diff, right: TackyVal::Constant(es), dst: result.clone()
                    });
                    return (result, CType::Long);
                }
                return (raw_diff, CType::Long);
            }
        }

        // For shifts, don't convert to common type, but do promote chars
        let is_shift = matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
        if is_shift {
            let promoted = l_type.promote();
            let l_conv = self.convert_to(l, l_type, promoted);
            let dst = self.fresh_tmp(promoted);
            let tacky_op = Self::convert_binop(op);
            self.emit(TackyInstr::Binary { op: tacky_op, left: l_conv, right: r, dst: dst.clone() });
            return (dst, promoted);
        }

        let common = CType::common(l_type, r_type);
        let l_conv = self.convert_to(l, l_type, common);
        let r_conv = self.convert_to(r, r_type, common);
        let result_type = if is_comparison_op(&op) { CType::Int } else { common };
        let dst = self.fresh_tmp(result_type);
        let tacky_op = Self::convert_binop(op);
        self.emit(TackyInstr::Binary { op: tacky_op, left: l_conv, right: r_conv, dst: dst.clone() });
        (dst, result_type)
    }

    fn emit_logical_and(&mut self, left: Exp, right: Exp) -> TackyVal {
        let false_label = self.fresh_label("and_false");
        let end_label = self.fresh_label("and_end");
        let result = self.fresh_tmp(CType::Int);
        let (l, _) = self.emit_exp(left);
        self.emit(TackyInstr::JumpIfZero(l, false_label.clone()));
        let (r, _) = self.emit_exp(right);
        self.emit(TackyInstr::JumpIfZero(r, false_label.clone()));
        self.emit(TackyInstr::Copy { src: TackyVal::Constant(1), dst: result.clone() });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(false_label));
        self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: result.clone() });
        self.emit(TackyInstr::Label(end_label));
        result
    }

    fn emit_logical_or(&mut self, left: Exp, right: Exp) -> TackyVal {
        let true_label = self.fresh_label("or_true");
        let end_label = self.fresh_label("or_end");
        let result = self.fresh_tmp(CType::Int);
        let (l, _) = self.emit_exp(left);
        self.emit(TackyInstr::JumpIfNotZero(l, true_label.clone()));
        let (r, _) = self.emit_exp(right);
        self.emit(TackyInstr::JumpIfNotZero(r, true_label.clone()));
        self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: result.clone() });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(true_label));
        self.emit(TackyInstr::Copy { src: TackyVal::Constant(1), dst: result.clone() });
        self.emit(TackyInstr::Label(end_label));
        result
    }

    fn convert_binop(op: BinaryOp) -> TackyBinaryOp {
        match op {
            BinaryOp::Add => TackyBinaryOp::Add,
            BinaryOp::Sub => TackyBinaryOp::Sub,
            BinaryOp::Mul => TackyBinaryOp::Mul,
            BinaryOp::Div => TackyBinaryOp::Div,
            BinaryOp::Mod => TackyBinaryOp::Mod,
            BinaryOp::BitwiseAnd => TackyBinaryOp::BitwiseAnd,
            BinaryOp::BitwiseOr => TackyBinaryOp::BitwiseOr,
            BinaryOp::BitwiseXor => TackyBinaryOp::BitwiseXor,
            BinaryOp::ShiftLeft => TackyBinaryOp::ShiftLeft,
            BinaryOp::ShiftRight => TackyBinaryOp::ShiftRight,
            BinaryOp::Equal => TackyBinaryOp::Equal,
            BinaryOp::NotEqual => TackyBinaryOp::NotEqual,
            BinaryOp::LessThan => TackyBinaryOp::LessThan,
            BinaryOp::GreaterThan => TackyBinaryOp::GreaterThan,
            BinaryOp::LessEqual => TackyBinaryOp::LessEqual,
            BinaryOp::GreaterEqual => TackyBinaryOp::GreaterEqual,
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr => unreachable!(),
        }
    }

    // --------------------------------------------------------
    // Statement emission
    // --------------------------------------------------------

    fn emit_statement(&mut self, stmt: Statement) {
        match stmt {
            Statement::Return(exp) => {
                let ret_type = self.func_types.get(&self.current_function)
                    .map(|(rt, _, _)| *rt).unwrap_or(CType::Int);
                if let Some(exp) = exp {
                    let (val, val_type) = self.emit_exp(exp);
                    if ret_type == CType::Void {
                        // void return — just emit return with dummy value
                        self.emit(TackyInstr::Return(TackyVal::Constant(0)));
                    } else {
                        let val_conv = self.convert_to(val, val_type, ret_type);
                        self.emit(TackyInstr::Return(val_conv));
                    }
                } else {
                    // return; (no expression) — for void functions
                    self.emit(TackyInstr::Return(TackyVal::Constant(0)));
                }
            }
            Statement::Expression(exp) => {
                self.emit_exp(exp);
            }
            Statement::If(cond, then_stmt, else_stmt) => {
                let (cond_val, _) = self.emit_exp(cond);
                match else_stmt {
                    None => {
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::JumpIfZero(cond_val, end_label.clone()));
                        self.emit_statement(*then_stmt);
                        self.emit(TackyInstr::Label(end_label));
                    }
                    Some(else_s) => {
                        let else_label = self.fresh_label("if_else");
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::JumpIfZero(cond_val, else_label.clone()));
                        self.emit_statement(*then_stmt);
                        self.emit(TackyInstr::Jump(end_label.clone()));
                        self.emit(TackyInstr::Label(else_label));
                        self.emit_statement(*else_s);
                        self.emit(TackyInstr::Label(end_label));
                    }
                }
            }
            Statement::Block(block) => self.emit_block(block),
            Statement::While { condition, body, label } => {
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                self.emit(TackyInstr::Label(continue_label.clone()));
                let (cond_val, _) = self.emit_exp(condition);
                self.emit(TackyInstr::JumpIfZero(cond_val, break_label.clone()));
                self.emit_statement(*body);
                self.emit(TackyInstr::Jump(continue_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::DoWhile { body, condition, label } => {
                let start_label = format!("start_{}", label);
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                self.emit(TackyInstr::Label(start_label.clone()));
                self.emit_statement(*body);
                self.emit(TackyInstr::Label(continue_label));
                let (cond_val, _) = self.emit_exp(condition);
                self.emit(TackyInstr::JumpIfNotZero(cond_val, start_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::For { init, condition, post, body, label } => {
                let start_label = format!("start_{}", label);
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                match init {
                    ForInit::Declaration(vd) => {
                        // Delegate to emit_var_decl which handles arrays, scalars, etc.
                        self.emit_var_decl(vd);
                    }
                    ForInit::Expression(Some(exp)) => { self.emit_exp(exp); }
                    ForInit::Expression(None) => {}
                }
                self.emit(TackyInstr::Label(start_label.clone()));
                if let Some(cond) = condition {
                    let (cond_val, _) = self.emit_exp(cond);
                    self.emit(TackyInstr::JumpIfZero(cond_val, break_label.clone()));
                }
                self.emit_statement(*body);
                self.emit(TackyInstr::Label(continue_label));
                if let Some(post_exp) = post { self.emit_exp(post_exp); }
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
                self.emit(TackyInstr::Jump(format!("label.{}.{}", self.current_function, label)));
            }
            Statement::Label(name, body) => {
                self.emit(TackyInstr::Label(format!("label.{}.{}", self.current_function, name)));
                self.emit_statement(*body);
            }
            Statement::Switch { control, body, label, cases } => {
                let break_label = format!("break_{}", label);
                let (control_val, ctrl_type) = self.emit_exp(control);
                // Integer promotion for switch control
                let promoted_type = ctrl_type.promote();
                let control_val = self.convert_to(control_val, ctrl_type, promoted_type);
                for case in &cases {
                    if let Some(val) = case.value {
                        let cmp_result = self.fresh_tmp(CType::Int);
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Equal,
                            left: control_val.clone(),
                            right: TackyVal::Constant(val),
                            dst: cmp_result.clone(),
                        });
                        self.emit(TackyInstr::JumpIfNotZero(cmp_result, case.label.clone()));
                    }
                }
                let default_label = cases.iter().find(|c| c.value.is_none()).map(|c| c.label.clone());
                match default_label {
                    Some(dl) => self.emit(TackyInstr::Jump(dl)),
                    None => self.emit(TackyInstr::Jump(break_label.clone())),
                }
                self.emit_statement(*body);
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::Case { body, label, .. } => {
                self.emit(TackyInstr::Label(label));
                self.emit_statement(*body);
            }
            Statement::Default { body, label } => {
                self.emit(TackyInstr::Label(label));
                self.emit_statement(*body);
            }
            Statement::Null => {}
        }
    }

    /// Flatten array initializer and emit CopyToOffset for each scalar value.
    /// `base_offset` is the byte offset from the start of the array.
    /// `elem_sizes`: byte size of each sub-array level.
    /// For `int[4][2][6]`: elem_sizes = [48, 24, 4] (size of [2][6], [6], int)
    fn emit_array_init_flat(&mut self, arr_name: &str, init: &Exp, scalar_type: CType, base_offset: i64, elem_sizes: &[i64]) {
        match init {
            Exp::ArrayInit(elems) => {
                let this_elem_size = if !elem_sizes.is_empty() { elem_sizes[0] } else { scalar_type.size() as i64 };
                let inner_sizes = if elem_sizes.len() > 1 { &elem_sizes[1..] } else { &[] };
                for (i, elem) in elems.iter().enumerate() {
                    let elem_offset = base_offset + (i as i64) * this_elem_size;
                    match elem {
                        Exp::ArrayInit(_) => {
                            self.emit_array_init_flat(arr_name, elem, scalar_type, elem_offset, inner_sizes);
                        }
                        Exp::StringLiteral(s) if scalar_type == CType::Pointer => {
                            // String literal in array of pointers context: decay to pointer
                            let (val, val_type) = self.emit_exp(elem.clone());
                            let val_conv = self.convert_to(val, val_type, scalar_type);
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: arr_name.to_string(),
                                offset: elem_offset,
                            });
                        }
                        Exp::StringLiteral(s) => {
                            // String literal as sub-element of char array compound init
                            let chars_to_copy = std::cmp::min(s.len(), this_elem_size as usize);
                            let char_type = if scalar_type == CType::UChar { CType::UChar } else { CType::Char };
                            for (j, byte) in s.bytes().take(chars_to_copy).enumerate() {
                                let src = self.fresh_tmp(char_type);
                                self.emit(TackyInstr::Copy { src: TackyVal::Constant(byte as i64), dst: src.clone() });
                                self.emit(TackyInstr::CopyToOffset { src, dst_name: arr_name.to_string(), offset: elem_offset + j as i64 });
                            }
                        }
                        _ => {
                            let (val, val_type) = self.emit_exp(elem.clone());
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
                let this_elem_size = if !elem_sizes.is_empty() { elem_sizes[0] } else { s.len() as i64 + 1 };
                let chars_to_copy = std::cmp::min(s.len(), this_elem_size as usize);
                let char_type = if scalar_type == CType::UChar { CType::UChar } else { CType::Char };
                for (i, byte) in s.bytes().take(chars_to_copy).enumerate() {
                    let src = self.fresh_tmp(char_type);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(byte as i64), dst: src.clone() });
                    self.emit(TackyInstr::CopyToOffset { src, dst_name: arr_name.to_string(), offset: base_offset + i as i64 });
                }
            }
            _ => {
                let (val, val_type) = self.emit_exp(init.clone());
                let val_conv = self.convert_to(val, val_type, scalar_type);
                self.emit(TackyInstr::CopyToOffset {
                    src: val_conv,
                    dst_name: arr_name.to_string(),
                    offset: base_offset,
                });
            }
        }
    }

    /// Compute element sizes for each array dimension.
    /// For `int[4][2][6]`: returns [48, 24, 4] (sizes of [2][6], [6], int)
    fn compute_elem_sizes(ft: &FullType) -> Vec<i64> {
        let mut sizes = Vec::new();
        let mut t = ft;
        while let FullType::Array { elem, .. } = t {
            sizes.push(elem.byte_size() as i64);
            t = elem;
        }
        sizes
    }

    /// Flatten a static initializer (possibly nested ArrayInit) into a flat list of StaticInit values.
    /// Uses elem_sizes to properly zero-pad partial initializations.
    fn flatten_static_init(init: &Exp, base_type: CType, elem_sizes: &[i64], out: &mut Vec<StaticInit>) {
        match init {
            Exp::ArrayInit(elems) => {
                let this_elem_size = if !elem_sizes.is_empty() { elem_sizes[0] } else { base_type.size() as i64 };
                let inner_sizes = if elem_sizes.len() > 1 { &elem_sizes[1..] } else { &[] };
                for elem in elems {
                    let start_len: usize = out.iter().map(|v| Self::static_init_size(v)).sum();
                    match elem {
                        Exp::StringLiteral(s) => {
                            // String literal as sub-element: use this_elem_size from parent
                            let null_terminated = (s.len() as i64) < this_elem_size;
                            let str_to_write = if (s.len() as i64) <= this_elem_size { s.clone() } else { s[..this_elem_size as usize].to_string() };
                            out.push(StaticInit::StringInit(str_to_write, null_terminated));
                        }
                        _ => {
                            Self::flatten_static_init(elem, base_type, inner_sizes, out);
                        }
                    }
                    let end_len: usize = out.iter().map(|v| Self::static_init_size(v)).sum();
                    let written = (end_len - start_len) as i64;
                    if written < this_elem_size {
                        out.push(StaticInit::ZeroInit((this_elem_size - written) as usize));
                    }
                }
            }
            Exp::StringLiteral(s) => {
                // String literal at top level (direct array init, not inside ArrayInit)
                let this_elem_size = if !elem_sizes.is_empty() { elem_sizes[0] } else { s.len() as i64 + 1 };
                let null_terminated = (s.len() as i64) < this_elem_size;
                let str_to_write = if (s.len() as i64) <= this_elem_size { s.clone() } else { s[..this_elem_size as usize].to_string() };
                out.push(StaticInit::StringInit(str_to_write, null_terminated));
                let written = s.len() as i64 + if null_terminated { 1 } else { 0 };
                if written < this_elem_size {
                    out.push(StaticInit::ZeroInit((this_elem_size - written) as usize));
                }
            }
            _ => {
                let (v, is_dbl, is_uns) = eval_constant_init(&Some(init.clone()));
                let cv = convert_init_value(v, base_type, is_dbl, is_uns);
                out.push(make_static_init(cv, base_type));
            }
        }
    }

    fn count_scalar_inits(exp: &Exp) -> usize {
        match exp {
            Exp::ArrayInit(elems) => elems.iter().map(|e| Self::count_scalar_inits(e)).sum(),
            _ => 1,
        }
    }

    fn emit_array_init(&mut self, arr_name: &str, init: &Exp, elem_type: CType, _dims: &[usize], _depth: usize) {
        if let Exp::ArrayInit(elems) = init {
            for (i, elem) in elems.iter().enumerate() {
                if let Exp::ArrayInit(_) = elem {
                    // Nested array init — flatten for now
                    // TODO: handle multi-dimensional properly
                    self.emit_array_init(arr_name, elem, elem_type, _dims, _depth + 1);
                } else {
                    let (val, val_type) = self.emit_exp(elem.clone());
                    let val_conv = self.convert_to(val, val_type, elem_type);
                    // ptr = arr_name + i * elem_size
                    let offset_val = TackyVal::Constant((i as i64) * (elem_type.size() as i64));
                    let ptr = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: TackyVal::Var(arr_name.to_string()),
                        right: offset_val,
                        dst: ptr.clone(),
                    });
                    self.emit(TackyInstr::Store { src: val_conv, dst_ptr: ptr });
                }
            }
        }
    }

    /// Handle a variable declaration (arrays, scalars, static, etc.)
    fn emit_var_decl(&mut self, vd: VarDeclaration) {
        // Static arrays
        if vd.array_dims.is_some() && vd.storage_class == Some(StorageClass::Static) {
            let base_type = vd.var_type;
            let dims = vd.array_dims.as_ref().unwrap();
            let full_type = vd.decl_full_type.clone()
                .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
            let total_elems: usize = dims.iter().product();
            let total_bytes = total_elems * base_type.size() as usize;
            let align = if total_bytes >= 16 { 16 } else { std::cmp::max(base_type.size() as usize, 1) };
            self.register_var(&vd.name, full_type.clone());
            let mut init_values: Vec<StaticInit> = Vec::new();

            // String literal initializer for static char array
            if let Some(Exp::StringLiteral(ref s)) = vd.init {
                let null_terminated = s.len() < total_bytes;
                init_values.push(StaticInit::StringInit(
                    if s.len() <= total_bytes { s.clone() } else { s[..total_bytes].to_string() },
                    null_terminated,
                ));
                let string_bytes = if null_terminated { s.len() + 1 } else { s.len() };
                if string_bytes < total_bytes {
                    init_values.push(StaticInit::ZeroInit(total_bytes - string_bytes));
                }
            } else if let Some(ref init_exp) = vd.init {
                let elem_sizes = Self::compute_elem_sizes(&full_type);
                Self::flatten_static_init(init_exp, base_type, &elem_sizes, &mut init_values);
                let initialized_bytes: usize = init_values.iter().map(|v| Self::static_init_size(v)).sum();
                if initialized_bytes < total_bytes {
                    init_values.push(StaticInit::ZeroInit(total_bytes - initialized_bytes));
                }
            } else {
                init_values.push(StaticInit::ZeroInit(total_bytes));
            }
            self.static_vars.push(TackyStaticVar {
                name: vd.name.clone(), global: false, alignment: align, init_values,
            });
            return;
        }

        // Local arrays
        if let Some(ref dims) = vd.array_dims {
            let base_type = vd.var_type;
            let full_type = vd.decl_full_type.clone()
                .unwrap_or_else(|| FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims));
            let total_bytes = full_type.byte_size();
            self.register_var(&vd.name, full_type.clone());
            self.array_sizes.insert(vd.name.clone(), total_bytes);
            let scalar_type = {
                let mut t = &full_type;
                while let FullType::Array { elem, .. } = t { t = elem; }
                t.to_ctype()
            };
            // Zero-fill using long-sized chunks
            let sz = if scalar_type.size() > 0 { scalar_type.size() as usize } else { 1 };
            {
                let mut off = 0usize;
                while off + 8 <= total_bytes {
                    let z = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: z.clone() });
                    self.emit(TackyInstr::CopyToOffset { src: z, dst_name: vd.name.clone(), offset: off as i64 });
                    off += 8;
                }
                while off + 4 <= total_bytes {
                    let z = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: z.clone() });
                    self.emit(TackyInstr::CopyToOffset { src: z, dst_name: vd.name.clone(), offset: off as i64 });
                    off += 4;
                }
                while off < total_bytes {
                    let z = self.fresh_tmp(CType::Char);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: z.clone() });
                    self.emit(TackyInstr::CopyToOffset { src: z, dst_name: vd.name.clone(), offset: off as i64 });
                    off += 1;
                }
            }
            if let Some(Exp::StringLiteral(ref s)) = vd.init {
                // String literal initializer for local char array: emit byte by byte
                let chars_to_copy = std::cmp::min(s.len(), total_bytes);
                for (i, byte) in s.bytes().take(chars_to_copy).enumerate() {
                    let char_type = if base_type == CType::UChar { CType::UChar } else { CType::Char };
                    let src = self.fresh_tmp(char_type);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(byte as i64), dst: src.clone() });
                    self.emit(TackyInstr::CopyToOffset { src, dst_name: vd.name.clone(), offset: i as i64 });
                }
                // Null terminator if there's room (already zero-filled above)
            } else if let Some(init) = vd.init {
                let elem_sizes = Self::compute_elem_sizes(&full_type);
                self.emit_array_init_flat(&vd.name, &init, scalar_type, 0, &elem_sizes);
            }
            return;
        }

        // Struct variable
        if let Some(FullType::Struct(ref tag)) = vd.decl_full_type {
            let tag = tag.clone();
            let def = self.struct_defs.get(&tag).cloned()
                .unwrap_or_else(|| panic!("Undefined struct: {}", tag));
            let struct_size = def.size;
            let struct_align = def.alignment;
            let ft = FullType::Struct(tag.clone());

            // Static struct: emit as static data
            if vd.storage_class == Some(StorageClass::Static) {
                self.register_var(&vd.name, ft);
                let mut init_values: Vec<StaticInit> = Vec::new();
                if let Some(Exp::ArrayInit(ref elems)) = vd.init {
                    // Flatten compound initializer for static struct
                    let mut bytes_written = 0usize;
                    for (i, elem) in elems.iter().enumerate() {
                        if i >= def.members.len() { break; }
                        let mem = &def.members[i];
                        // Pad to member offset
                        if bytes_written < mem.offset {
                            init_values.push(StaticInit::ZeroInit(mem.offset - bytes_written));
                        }
                        if mem.member_full_type.is_array() || mem.member_full_type.is_struct() {
                            if let Exp::StringLiteral(s) = elem {
                                // String literal for char array member
                                let null_term = s.len() < mem.size;
                                init_values.push(StaticInit::StringInit(s.clone(), null_term));
                                let str_bytes = s.len() + if null_term { 1 } else { 0 };
                                if str_bytes < mem.size {
                                    init_values.push(StaticInit::ZeroInit(mem.size - str_bytes));
                                }
                            } else if let Exp::ArrayInit(ref sub_elems) = elem {
                                if mem.member_full_type.is_array() {
                                    let scalar_t = { let mut t = &mem.member_full_type; while let FullType::Array { elem: e, .. } = t { t = e; } t.to_ctype() };
                                    let elem_sizes = Self::compute_elem_sizes(&mem.member_full_type);
                                    Self::flatten_static_init(elem, scalar_t, &elem_sizes, &mut init_values);
                                    // Pad to member size
                                    let written: usize = init_values.iter().map(|v| Self::static_init_size(v)).sum::<usize>() - (mem.offset + (bytes_written.max(mem.offset) - mem.offset));
                                } else if let FullType::Struct(ref inner_tag) = mem.member_full_type {
                                    // Nested struct compound init in static context
                                    let inner_def = self.struct_defs.get(inner_tag).cloned().unwrap();
                                    let mut inner_written = 0usize;
                                    for (j, sub_elem) in sub_elems.iter().enumerate() {
                                        if j >= inner_def.members.len() { break; }
                                        let inner_mem = &inner_def.members[j];
                                        if inner_written < inner_mem.offset {
                                            init_values.push(StaticInit::ZeroInit(inner_mem.offset - inner_written));
                                        }
                                        let (v, is_dbl, is_uns) = eval_constant_init(&Some(sub_elem.clone()));
                                        let cv = convert_init_value(v, inner_mem.member_type, is_dbl, is_uns);
                                        init_values.push(make_static_init(cv, inner_mem.member_type));
                                        inner_written = inner_mem.offset + inner_mem.member_type.size() as usize;
                                    }
                                    if inner_written < inner_def.size {
                                        init_values.push(StaticInit::ZeroInit(inner_def.size - inner_written));
                                    }
                                }
                            }
                            bytes_written = mem.offset + mem.size;
                        } else if let Exp::StringLiteral(ref s) = elem {
                            if mem.member_type == CType::Pointer {
                                let str_label = self.make_string_constant(s);
                                init_values.push(StaticInit::PointerInit(str_label));
                            } else {
                                let null_term = s.len() < mem.size;
                                init_values.push(StaticInit::StringInit(s.clone(), null_term));
                                let str_bytes = s.len() + if null_term { 1 } else { 0 };
                                if str_bytes < mem.size {
                                    init_values.push(StaticInit::ZeroInit(mem.size - str_bytes));
                                }
                            }
                            bytes_written = mem.offset + mem.size;
                        } else {
                            let (v, is_dbl, is_uns) = eval_constant_init(&Some(elem.clone()));
                            let cv = convert_init_value(v, mem.member_type, is_dbl, is_uns);
                            init_values.push(make_static_init(cv, mem.member_type));
                            bytes_written = mem.offset + mem.member_type.size() as usize;
                        }
                    }
                    // Trailing padding
                    if bytes_written < struct_size {
                        init_values.push(StaticInit::ZeroInit(struct_size - bytes_written));
                    }
                } else {
                    init_values.push(StaticInit::ZeroInit(struct_size));
                }
                self.static_vars.push(TackyStaticVar {
                    name: vd.name.clone(), global: false, alignment: struct_align, init_values,
                });
                return;
            }
            if vd.storage_class == Some(StorageClass::Extern) {
                self.register_var(&vd.name, ft);
                self.extern_vars.push(vd.name);
                return;
            }
            self.register_var(&vd.name, ft);
            self.array_sizes.insert(vd.name.clone(), struct_size);
            // Zero-fill using long-sized chunks
            {
                let mut off = 0usize;
                while off + 8 <= struct_size {
                    let z = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: z.clone() });
                    self.emit(TackyInstr::CopyToOffset { src: z, dst_name: vd.name.clone(), offset: off as i64 });
                    off += 8;
                }
                while off + 4 <= struct_size {
                    let z = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: z.clone() });
                    self.emit(TackyInstr::CopyToOffset { src: z, dst_name: vd.name.clone(), offset: off as i64 });
                    off += 4;
                }
                while off < struct_size {
                    let z = self.fresh_tmp(CType::Char);
                    self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: z.clone() });
                    self.emit(TackyInstr::CopyToOffset { src: z, dst_name: vd.name.clone(), offset: off as i64 });
                    off += 1;
                }
            }
            // Handle compound initializer
            if let Some(Exp::ArrayInit(ref elems)) = vd.init {
                for (i, elem) in elems.iter().enumerate() {
                    if i >= def.members.len() { break; }
                    let member = &def.members[i];
                    let mem_ft = &member.member_full_type;
                    // Handle nested struct/array member init
                    if mem_ft.is_array() || mem_ft.is_struct() {
                        // Handle string literal for char array members
                        if let Exp::StringLiteral(ref s) = elem {
                            let chars_to_copy = std::cmp::min(s.len(), member.size);
                            for (j, byte) in s.bytes().take(chars_to_copy).enumerate() {
                                let char_type = CType::Char;
                                let src = self.fresh_tmp(char_type);
                                self.emit(TackyInstr::Copy { src: TackyVal::Constant(byte as i64), dst: src.clone() });
                                self.emit(TackyInstr::CopyToOffset { src, dst_name: vd.name.clone(), offset: (member.offset + j) as i64 });
                            }
                        } else if let Exp::ArrayInit(ref sub_elems) = elem {
                            if mem_ft.is_array() {
                                let elem_sizes = Self::compute_elem_sizes(mem_ft);
                                let scalar_t = { let mut t = mem_ft; while let FullType::Array { elem: e, .. } = t { t = e; } t.to_ctype() };
                                self.emit_array_init_flat(&vd.name, elem, scalar_t, member.offset as i64, &elem_sizes);
                            } else if let FullType::Struct(ref inner_tag) = mem_ft {
                                // Nested struct compound init
                                let inner_def = self.struct_defs.get(inner_tag).cloned()
                                    .unwrap_or_else(|| panic!("Undefined struct: {}", inner_tag));
                                for (j, sub_elem) in sub_elems.iter().enumerate() {
                                    if j >= inner_def.members.len() { break; }
                                    let inner_mem = &inner_def.members[j];
                                    if inner_mem.member_full_type.is_array() {
                                        let elem_sizes = Self::compute_elem_sizes(&inner_mem.member_full_type);
                                        let scalar_t = { let mut t = &inner_mem.member_full_type; while let FullType::Array { elem: e, .. } = t { t = e; } t.to_ctype() };
                                        self.emit_array_init_flat(&vd.name, sub_elem, scalar_t, (member.offset + inner_mem.offset) as i64, &elem_sizes);
                                    } else {
                                        let (val, val_type) = self.emit_exp(sub_elem.clone());
                                        let target_type = inner_mem.member_type;
                                        let val_conv = self.convert_to(val, val_type, target_type);
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
                        let (val, val_type) = self.emit_exp(elem.clone());
                        let target_type = member.member_type;
                        let val_conv = self.convert_to(val, val_type, target_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: val_conv,
                            dst_name: vd.name.clone(),
                            offset: member.offset as i64,
                        });
                    }
                }
            } else if let Some(init) = vd.init {
                // Copy from another struct expression
                let (val, _) = self.emit_exp(init);
                let src_addr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress { src: val, dst: src_addr.clone() });
                self.emit_struct_copy_to(src_addr, &vd.name, struct_size);
            }
            return;
        }

        // Regular scalar/pointer variable
        self.var_types.insert(vd.name.clone(), vd.var_type);
        self.symbol_types.insert(vd.name.clone(), vd.var_type);
        if let Some(pi) = vd.ptr_info { self.ptr_info.insert(vd.name.clone(), pi); }
        // Use decl_full_type if available (preserves pointer-to-array info)
        let ft = if let Some(ref dft) = vd.decl_full_type {
            dft.clone()
        } else {
            FullType::from_decl(vd.var_type, vd.ptr_info, &None)
        };
        self.full_types.insert(vd.name.clone(), ft);

        if vd.storage_class == Some(StorageClass::Static) {
            if let Some(Exp::ArrayInit(ref elems)) = vd.init {
                return;
            }
            // Static pointer initialized with string literal: static char *p = "hello";
            if let Some(Exp::StringLiteral(ref s)) = vd.init {
                let str_label = self.make_string_constant(s);
                let align = std::cmp::max(vd.var_type.size() as usize, 1);
                self.static_vars.push(TackyStaticVar {
                    name: vd.name, global: false, alignment: align,
                    init_values: vec![StaticInit::PointerInit(str_label)],
                });
                return;
            }
            let (raw_val, is_dbl, is_uns) = eval_constant_init(&vd.init);
            let init_val = convert_init_value(raw_val, vd.var_type, is_dbl, is_uns);
            let align = if vd.var_type == CType::Double { 16 } else { std::cmp::max(vd.var_type.size() as usize, 1) };
            let init_v = make_static_init(init_val, vd.var_type);
            self.static_vars.push(TackyStaticVar {
                name: vd.name, global: false, alignment: align, init_values: vec![init_v],
            });
        } else if vd.storage_class == Some(StorageClass::Extern) {
            self.extern_vars.push(vd.name);
        } else if let Some(init) = vd.init {
            let vd_name = vd.name.clone();
            let (val, val_type) = self.emit_exp(init);
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
            self.emit(TackyInstr::Copy { src: val_conv, dst: TackyVal::Var(vd_name) });
        }
    }

    fn emit_block(&mut self, block: Block) {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    self.emit_var_decl(vd);
                    /* OLD CODE - now handled by emit_var_decl:
                    // Handle array declarations
                    // Static arrays must be handled separately (emitted to .data section)
                    if vd.array_dims.is_some() && vd.storage_class == Some(StorageClass::Static) {
                        let base_type = vd.var_type;
                        let dims = vd.array_dims.as_ref().unwrap();
                        let full_type = FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims);
                        let total_elems: usize = dims.iter().product();
                        let total_bytes = total_elems * base_type.size() as usize;
                        let align = if total_bytes >= 16 { 16 } else { base_type.size() as usize };

                        // Register as array type so decay works
                        self.register_var(&vd.name, full_type.clone());

                        let mut init_values: Vec<StaticInit> = Vec::new();
                        if let Some(Exp::ArrayInit(ref elems)) = vd.init {
                            for e in elems.iter() {
                                let (v, is_dbl, is_uns) = eval_constant_init(&Some(e.clone()));
                                let cv = convert_init_value(v, base_type, is_dbl, is_uns);
                                init_values.push(make_static_init(cv, base_type));
                            }
                            // Zero-pad
                            let initialized = init_values.len() * base_type.size() as usize;
                            if initialized < total_bytes {
                                init_values.push(StaticInit::ZeroInit(total_bytes - initialized));
                            }
                        } else {
                            init_values.push(StaticInit::ZeroInit(total_bytes));
                        }

                        self.static_vars.push(TackyStaticVar {
                            name: vd.name.clone(),
                            global: false,
                            alignment: align,
                            init_values,
                        });
                        continue;
                    }

                    if let Some(ref dims) = vd.array_dims {
                        let base_type = vd.var_type;
                        // Build FullType from dims
                        let full_type = FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims);
                        let total_bytes = full_type.byte_size();

                        // Register the array variable directly (no __array_storage_ indirection)
                        // The array IS the storage. It has type Array, not Pointer.
                        self.register_var(&vd.name, full_type.clone());
                        // Override symbol_types to mark as array for stack allocation
                        self.array_sizes.insert(vd.name.clone(), total_bytes);

                        // Zero-fill using CopyToOffset
                        // Get the innermost scalar element type and total count
                        let scalar_type = {
                            let mut t = &full_type;
                            while let FullType::Array { elem, .. } = t { t = elem; }
                            t.to_ctype()
                        };
                        let total_scalar_elems = total_bytes / scalar_type.size() as usize;

                        for i in 0..total_scalar_elems {
                            let offset = (i as i64) * (scalar_type.size() as i64);
                            let zero = if scalar_type == CType::Double {
                                let z = self.fresh_tmp(CType::Double);
                                self.emit(TackyInstr::Copy { src: TackyVal::DoubleConstant(0.0), dst: z.clone() });
                                z
                            } else {
                                let z = self.fresh_tmp(scalar_type);
                                self.emit(TackyInstr::Copy { src: TackyVal::Constant(0), dst: z.clone() });
                                z
                            };
                            self.emit(TackyInstr::CopyToOffset {
                                src: zero,
                                dst_name: vd.name.clone(),
                                offset,
                            });
                        }

                        // Handle initialization
                        if let Some(init) = vd.init {
                            self.emit_array_init_flat(&vd.name, &init, scalar_type, 0);
                        }

                        continue;
                    }

                    self.var_types.insert(vd.name.clone(), vd.var_type);
                    self.symbol_types.insert(vd.name.clone(), vd.var_type);
                    if let Some(pi) = vd.ptr_info { self.ptr_info.insert(vd.name.clone(), pi); }
                    // Register FullType
                    let ft = FullType::from_decl(vd.var_type, vd.ptr_info, &None);
                    self.full_types.insert(vd.name.clone(), ft);
                    if vd.storage_class == Some(StorageClass::Static) {
                        // Check for static array init
                        if let Some(Exp::ArrayInit(ref elems)) = vd.init {
                            let elem_type = vd.var_type;
                            let total_elems = if let Some(ref dims) = vd.array_dims {
                                dims.iter().product()
                            } else {
                                elems.len()
                            };
                            let mut vals = vec![0i64; total_elems];
                            for (i, e) in elems.iter().enumerate() {
                                let (v, is_dbl, is_uns) = eval_constant_init(&Some(e.clone()));
                                vals[i] = convert_init_value(v, elem_type, is_dbl, is_uns);
                            }
                            // Emit the array as a static data entry
                            let total_bytes = total_elems * elem_type.size() as usize;
                            let align = if total_bytes >= 16 { 16 } else { elem_type.size() as usize };
                            let mut init_values: Vec<StaticInit> = vals.iter().map(|&v| {
                                match elem_type {
                                    CType::Int => StaticInit::IntInit(v as i32),
                                    CType::UInt => StaticInit::UIntInit(v as u32),
                                    CType::Long => StaticInit::LongInit(v),
                                    CType::ULong => StaticInit::ULongInit(v as u64),
                                    CType::Double => StaticInit::DoubleInit(f64::from_bits(v as u64)),
                                    _ => StaticInit::LongInit(v),
                                }
                            }).collect();
                            // Zero-pad remaining elements
                            let initialized_bytes = vals.len() * elem_type.size() as usize;
                            if initialized_bytes < total_bytes {
                                init_values.push(StaticInit::ZeroInit(total_bytes - initialized_bytes));
                            }
                            self.static_vars.push(TackyStaticVar {
                                name: vd.name.clone(),
                                global: false,
                                alignment: align,
                                init_values,
                            });
                            // The array name is treated as a pointer to the static data
                            // It's registered as a "global" so codegen uses Data addressing
                            self.ptr_info.insert(vd.name.clone(), (elem_type, 1));
                            // DON'T register in var_types as Pointer — instead,
                            // the array name will be a Data operand that IS the address
                            continue;
                        }
                        let (raw_val, is_dbl, is_uns) = eval_constant_init(&vd.init);
                        let init_val = convert_init_value(raw_val, vd.var_type, is_dbl, is_uns);
                        let align = if vd.var_type == CType::Double { 16 } else { vd.var_type.size() as usize };
                        let init_v = make_static_init(init_val, vd.var_type);
                        self.static_vars.push(TackyStaticVar {
                            name: vd.name,
                            global: false,
                            alignment: align,
                            init_values: vec![init_v],
                        });
                    } else if vd.storage_class == Some(StorageClass::Extern) {
                        self.extern_vars.push(vd.name);
                    } else if let Some(init) = vd.init {
                        let vd_name = vd.name.clone();
                        let (val, val_type) = self.emit_exp(init);
                        let val_conv = self.convert_to(val, val_type, vd.var_type);
                        // Propagate pointee type for pointer assignments
                        if vd.var_type == CType::Pointer {
                            if let TackyVal::Var(ref src_name) = val_conv {
                                if let Some(&info) = self.ptr_info.get(src_name) {
                                    self.ptr_info.insert(vd_name.clone(), info);
                                }
                            }
                        }
                        self.emit(TackyInstr::Copy { src: val_conv, dst: TackyVal::Var(vd_name) });
                    }
                    END OF OLD CODE */
                }
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    // Register function type for block-scope prototypes
                    let param_types: Vec<CType> = fd.params.iter().map(|(_, t, _)| *t).collect();
                    self.func_types.insert(fd.name.clone(), (fd.return_type, param_types, fd.return_ptr_info)); if let Some(ref rft) = fd.return_full_type { self.func_full_types.insert(fd.name.clone(), rft.clone()); }
                }
                BlockItem::Declaration(Declaration::StructDecl(sd)) => {
                    if !sd.members.is_empty() {
                        let def = StructDef::from_members(&sd.tag, &sd.members, &self.struct_defs);
                        self.struct_defs.insert(sd.tag.clone(), def);
                    }
                }
                BlockItem::Statement(stmt) => self.emit_statement(stmt),
            }
        }
    }

    fn emit_function(&mut self, func: FunctionDeclaration) -> Option<TackyFunction> {
        let body = match func.body {
            Some(b) => b,
            None => return None,
        };

        self.current_function = func.name.clone();
        self.instructions.clear();

        // Register params
        for (i, (name, ptype, pi)) in func.params.iter().enumerate() {
            self.var_types.insert(name.clone(), *ptype);
            self.symbol_types.insert(name.clone(), *ptype);
            if let Some(info) = pi {
                self.ptr_info.insert(name.clone(), *info);
            }
            // Register FullType — prefer param_full_types if available
            let ft = if i < func.param_full_types.len() {
                func.param_full_types[i].clone()
            } else {
                FullType::from_decl(*ptype, *pi, &None)
            };
            self.full_types.insert(name.clone(), ft);
        }

        self.emit_block(body);
        self.emit(TackyInstr::Return(TackyVal::Constant(0)));

        Some(TackyFunction {
            name: func.name,
            params: func.params.iter().map(|(n, _, _)| n.clone()).collect(),
            global: true, // overridden by linkage map in generate()
            body: std::mem::take(&mut self.instructions),
        })
    }
}

fn is_comparison_op(op: &BinaryOp) -> bool {
    matches!(op,
        BinaryOp::Equal | BinaryOp::NotEqual |
        BinaryOp::LessThan | BinaryOp::GreaterThan |
        BinaryOp::LessEqual | BinaryOp::GreaterEqual
    )
}

/// Truncate/convert a constant value to the target type's bit width
fn convert_init_value(val: i64, target: CType, source_is_double: bool, source_is_unsigned: bool) -> i64 {
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
            CType::Int => d as i32 as i64,
            CType::UInt => d as u32 as i64,
            CType::Long => d as i64,
            CType::ULong => d as u64 as i64,
            _ => val,
        };
    }
    match target {
        CType::Char | CType::SChar => val as i8 as i64,
        CType::UChar => val as u8 as i64,
        CType::Int => val as i32 as i64,
        CType::UInt => val as u32 as i64,
        CType::Long | CType::ULong | CType::Double | CType::Pointer => val,
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
            CType::Int => StaticInit::IntInit(val as i32),
            CType::UInt => StaticInit::UIntInit(val as u32),
            CType::Long | CType::Pointer => StaticInit::LongInit(val),
            CType::ULong => StaticInit::ULongInit(val as u64),
            CType::Double => StaticInit::DoubleInit(f64::from_bits(val as u64)),
            CType::Void | CType::Struct => StaticInit::ZeroInit(0),
        }
    }
}

fn eval_constant_init(init: &Option<Exp>) -> (i64, bool, bool) {
    match init {
        Some(Exp::Constant(c)) | Some(Exp::LongConstant(c)) => (*c, false, false),
        Some(Exp::UIntConstant(c)) | Some(Exp::ULongConstant(c)) => (*c, false, true),
        Some(Exp::DoubleConstant(d)) => (d.to_bits() as i64, true, false),
        Some(Exp::Unary(UnaryOp::Negate, inner)) => {
            match inner.as_ref() {
                Exp::Constant(c) | Exp::LongConstant(c) => (-c, false, false),
                Exp::UIntConstant(c) | Exp::ULongConstant(c) => (-c, false, true),
                Exp::DoubleConstant(d) => ((-d).to_bits() as i64, true, false),
                _ => panic!("Static variable initializer must be a constant (in negate)"),
            }
        }
        Some(_) => panic!("Static variable initializer must be a constant"),
        None => (0, false, false),
    }
}

pub fn generate(program: Program) -> TackyProgram {
    let mut gen = TackyGen::new();
    let mut top_level = Vec::new();
    let mut global_vars = std::collections::HashSet::new();

    use std::collections::HashMap;

    // Determine linkage
    let mut linkage: HashMap<String, bool> = HashMap::new();
    for decl in &program.declarations {
        let (name, sc) = match decl {
            Declaration::FunDecl(fd) => (fd.name.clone(), &fd.storage_class),
            Declaration::VarDecl(vd) => (vd.name.clone(), &vd.storage_class),
            Declaration::StructDecl(_) => continue,
        };
        if !linkage.contains_key(&name) {
            linkage.insert(name, *sc != Some(StorageClass::Static));
        }
    }

    // Collect function types and file-scope variable types
    for decl in &program.declarations {
        match decl {
            Declaration::FunDecl(fd) => {
                let param_types: Vec<CType> = fd.params.iter().map(|(_, t, _)| *t).collect();
                gen.func_types.insert(fd.name.clone(), (fd.return_type, param_types, fd.return_ptr_info)); if let Some(ref rft) = fd.return_full_type { gen.func_full_types.insert(fd.name.clone(), rft.clone()); }
            }
            Declaration::VarDecl(vd) => {
                gen.var_types.insert(vd.name.clone(), vd.var_type);
                gen.symbol_types.insert(vd.name.clone(), vd.var_type);
                if let Some(pi) = vd.ptr_info { gen.ptr_info.insert(vd.name.clone(), pi); }
                // Register FullType (including for extern arrays)
                if let Some(ref dft) = vd.decl_full_type {
                    gen.full_types.insert(vd.name.clone(), dft.clone());
                    if dft.is_array() {
                        gen.array_sizes.insert(vd.name.clone(), dft.byte_size());
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
                    let def = StructDef::from_members(&sd.tag, &sd.members, &gen.struct_defs);
                    gen.struct_defs.insert(sd.tag.clone(), def);
                }
            }
        }
    }

    // Collect file-scope variables, merging
    let mut file_scope_vars: HashMap<String, (bool, Option<(i64, bool, bool)>, CType)> = HashMap::new();
    let mut file_scope_order: Vec<String> = Vec::new();

    for decl in &program.declarations {
        if let Declaration::VarDecl(vd) = decl {
            global_vars.insert(vd.name.clone());
            if vd.storage_class == Some(StorageClass::Extern) && vd.init.is_none() {
                continue;
            }
            let init_val: Option<(i64, bool, bool)> = match &vd.init {
                Some(Exp::Constant(c)) | Some(Exp::LongConstant(c)) => Some((*c, false, false)),
                Some(Exp::UIntConstant(c)) | Some(Exp::ULongConstant(c)) => Some((*c, false, true)),
                Some(Exp::DoubleConstant(d)) => Some((d.to_bits() as i64, true, false)),
                Some(Exp::Unary(UnaryOp::Negate, inner)) => match inner.as_ref() {
                    Exp::Constant(c) | Exp::LongConstant(c) => Some((-c, false, false)),
                    Exp::UIntConstant(c) | Exp::ULongConstant(c) => Some((-c, false, true)),
                    Exp::DoubleConstant(d) => Some(((-d).to_bits() as i64, true, false)),
                    _ => panic!("Global initializer must be constant"),
                },
                Some(Exp::ArrayInit(_)) => None, // Array init handled separately
                Some(Exp::StringLiteral(_)) => None, // String init handled separately
                Some(_) => panic!("Global initializer must be constant"),
                None => None,
            };
            let is_global = *linkage.get(&vd.name).unwrap_or(&true);
            if let Some(entry) = file_scope_vars.get_mut(&vd.name) {
                if init_val.is_some() { entry.1 = init_val; }
            } else {
                file_scope_vars.insert(vd.name.clone(), (is_global, init_val, vd.var_type));
                file_scope_order.push(vd.name.clone());
            }
        }
    }

    // Handle global arrays (both initialized and uninitialized)
    let mut global_array_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for decl in &program.declarations {
        if let Declaration::VarDecl(vd) = decl {
            // Handle global struct variables
            if let Some(FullType::Struct(ref tag)) = vd.decl_full_type {
                if vd.storage_class != Some(StorageClass::Extern) && !global_array_names.contains(&vd.name) {
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

                        let mut init_values: Vec<StaticInit> = Vec::new();
                        if let Some(Exp::ArrayInit(ref elems)) = vd.init {
                            // Compound initializer for global struct
                            let def = gen.struct_defs.get(&tag).cloned().unwrap();
                            let mut bytes_written = 0usize;
                            for (i, elem) in elems.iter().enumerate() {
                                if i >= def.members.len() { break; }
                                let mem = &def.members[i];
                                if bytes_written < mem.offset {
                                    init_values.push(StaticInit::ZeroInit(mem.offset - bytes_written));
                                }
                                if mem.member_full_type.is_array() {
                                    let scalar_t = { let mut t = &mem.member_full_type; while let FullType::Array { elem: e, .. } = t { t = e; } t.to_ctype() };
                                    let elem_sizes = TackyGen::compute_elem_sizes(&mem.member_full_type);
                                    TackyGen::flatten_static_init(elem, scalar_t, &elem_sizes, &mut init_values);
                                } else if mem.member_full_type.is_struct() {
                                    // Nested struct init — flatten recursively
                                    // TODO: proper nested struct static init
                                    init_values.push(StaticInit::ZeroInit(mem.size));
                                } else if let Exp::StringLiteral(ref s) = elem {
                                    // String literal member → create string constant and pointer
                                    if mem.member_type == CType::Pointer {
                                        let str_label = gen.make_string_constant(s);
                                        init_values.push(StaticInit::PointerInit(str_label));
                                    } else {
                                        // char array member initialized with string
                                        let null_term = s.len() < mem.size;
                                        init_values.push(StaticInit::StringInit(s.clone(), null_term));
                                        let str_bytes = s.len() + if null_term { 1 } else { 0 };
                                        if str_bytes < mem.size {
                                            init_values.push(StaticInit::ZeroInit(mem.size - str_bytes));
                                        }
                                    }
                                } else {
                                    let (v, is_dbl, is_uns) = eval_constant_init(&Some(elem.clone()));
                                    let cv = convert_init_value(v, mem.member_type, is_dbl, is_uns);
                                    init_values.push(make_static_init(cv, mem.member_type));
                                }
                                bytes_written = mem.offset + mem.size;
                            }
                            if bytes_written < struct_size {
                                init_values.push(StaticInit::ZeroInit(struct_size - bytes_written));
                            }
                        } else {
                            init_values.push(StaticInit::ZeroInit(struct_size));
                        }
                        top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                            name: vd.name.clone(), global: is_global, alignment: struct_align, init_values,
                        }));
                    }
                    continue;
                }
            }
            // Handle uninitialized global arrays (skip extern and already-handled)
            if vd.array_dims.is_some() && !matches!(&vd.init, Some(Exp::ArrayInit(_)) | Some(Exp::StringLiteral(_)))
                && vd.storage_class != Some(StorageClass::Extern)
                && !global_array_names.contains(&vd.name) {
                let dims = vd.array_dims.as_ref().unwrap();
                let base_type = vd.var_type;
                let total_elems: usize = dims.iter().product();
                let total_bytes = total_elems * base_type.size() as usize;
                let align = if total_bytes >= 16 { 16 } else { base_type.size() as usize };
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                let ft = FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims);
                gen.register_var(&vd.name, ft);
                global_array_names.insert(vd.name.clone());
                file_scope_vars.remove(&vd.name);
                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(), global: is_global, alignment: align,
                    init_values: vec![StaticInit::ZeroInit(total_bytes)],
                }));
                continue;
            }
            // Global char array initialized with string literal
            if let (Some(ref dims), Some(Exp::StringLiteral(ref s))) = (&vd.array_dims, &vd.init) {
                let base_type = vd.var_type;
                let total_elems: usize = dims.iter().product();
                let total_bytes = total_elems * base_type.size() as usize;
                let align = if total_bytes >= 16 { 16 } else { std::cmp::max(base_type.size() as usize, 1) };
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                let ft = FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims);
                gen.register_var(&vd.name, ft);
                global_array_names.insert(vd.name.clone());
                file_scope_vars.remove(&vd.name);
                let null_terminated = s.len() < total_bytes;
                let mut init_values: Vec<StaticInit> = vec![
                    StaticInit::StringInit(
                        if s.len() <= total_bytes { s.clone() } else { s[..total_bytes].to_string() },
                        null_terminated,
                    ),
                ];
                let string_bytes = if null_terminated { s.len() + 1 } else { s.len() };
                if string_bytes < total_bytes {
                    init_values.push(StaticInit::ZeroInit(total_bytes - string_bytes));
                }
                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(), global: is_global, alignment: align, init_values,
                }));
                continue;
            }
            // Global pointer initialized with string literal: char *p = "hello";
            if let (None, Some(Exp::StringLiteral(ref s))) = (&vd.array_dims, &vd.init) {
                let str_label = gen.make_string_constant(s);
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                let align = std::cmp::max(vd.var_type.size() as usize, 1);
                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(), global: is_global, alignment: align,
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
            if let (Some(ref dims), Some(Exp::ArrayInit(ref elems))) = (&vd.array_dims, &vd.init) {
                let base_type = vd.var_type;
                let total_elems: usize = dims.iter().product();
                let total_bytes = total_elems * base_type.size() as usize;
                let align = if total_bytes >= 16 { 16 } else { base_type.size() as usize };
                let is_global = *linkage.get(&vd.name).unwrap_or(&true);

                // Build init values with proper sub-array zero-padding
                let mut init_values: Vec<StaticInit> = Vec::new();
                let ft = FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims);
                let elem_sizes = TackyGen::compute_elem_sizes(&ft);
                // Simple flatten for global arrays (they only have constant inits)
                fn flatten_init(exp: &Exp, base_type: CType, vals: &mut Vec<StaticInit>) {
                    match exp {
                        Exp::ArrayInit(elems) => {
                            for e in elems { flatten_init(e, base_type, vals); }
                        }
                        _ => {
                            let (v, is_dbl, is_uns) = eval_constant_init(&Some(exp.clone()));
                            let cv = convert_init_value(v, base_type, is_dbl, is_uns);
                            vals.push(make_static_init(cv, base_type));
                        }
                    }
                }
                TackyGen::flatten_static_init(&vd.init.as_ref().unwrap(), base_type, &elem_sizes, &mut init_values);
                let initialized_bytes: usize = init_values.iter().map(|v| TackyGen::static_init_size(v)).sum();
                if initialized_bytes < total_bytes {
                    init_values.push(StaticInit::ZeroInit(total_bytes - initialized_bytes));
                }

                top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
                    name: vd.name.clone(), global: is_global, alignment: align, init_values,
                }));

                // Register FullType
                let ft = FullType::from_decl(base_type, vd.ptr_info, &vd.array_dims);
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
                if let Some(mut tf) = gen.emit_function(fd) {
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
        }
    }

    for name in file_scope_order {
        let entry = file_scope_vars.remove(&name);
        if entry.is_none() { continue; } // already handled (e.g., global array)
        let (is_global, init_val, var_type) = entry.unwrap();
        let (raw_init, is_dbl, is_uns) = init_val.unwrap_or((0, false, false));
        let converted_init = convert_init_value(raw_init, var_type, is_dbl, is_uns);
        let align = if var_type == CType::Double { 16 } else { var_type.size() as usize };
        let init_v = make_static_init(converted_init, var_type);
        top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
            name,
            global: is_global,
            alignment: align,
            init_values: vec![init_v],
        }));
    }

    // Add static local var names too
    for tl in &top_level {
        match tl {
            TackyTopLevel::StaticVar(sv) => { global_vars.insert(sv.name.clone()); }
            TackyTopLevel::StaticConstant(sc) => { global_vars.insert(sc.name.clone()); }
            _ => {}
        }
    }

    TackyProgram {
        top_level,
        global_vars,
        symbol_types: gen.symbol_types,
        array_sizes: gen.array_sizes,
    }
}
