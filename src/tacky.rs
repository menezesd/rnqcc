#![allow(dead_code)]

use crate::types::*;
use std::collections::{HashMap, HashSet};

type FileScopeVarInfo = (bool, bool, Option<(i64, bool, bool)>, CType);
type StaticScalarValue = (i64, bool, bool);
type StaticComplexValue = (StaticScalarValue, StaticScalarValue);
type BuiltinFunctionInfo = (
    &'static str,
    CType,
    FullType,
    Vec<CType>,
    Option<(CType, usize)>,
);
pub type TackyResult<T> = Result<T, String>;

const VLA_STATIC_SCALE_FALLBACK: usize = 16;
const INLINE_ZERO_INIT_LIMIT: usize = 128;

#[derive(Copy, Clone)]
enum BitBuiltinKind {
    Ffs,
    Clz,
    Ctz,
    Clrsb,
    Popcount,
    Parity,
}

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

    fn required_bytes(&self) -> usize {
        self.pieces
            .iter()
            .map(|(offset, init)| offset + TackyGen::static_init_size(init))
            .max()
            .unwrap_or(0)
    }

    fn init_to_u64(init: &StaticInit) -> Option<u64> {
        match init {
            StaticInit::IntInit(v) => Some(*v as u32 as u64),
            StaticInit::LongInit(v) => Some(*v as u64),
            StaticInit::UIntInit(v) => Some(*v as u64),
            StaticInit::ULongInit(v) => Some(*v),
            StaticInit::ShortInit(v) => Some(*v as u16 as u64),
            StaticInit::UShortInit(v) => Some(*v as u64),
            StaticInit::CharInit(v) => Some(*v as u8 as u64),
            StaticInit::UCharInit(v) => Some(*v as u64),
            StaticInit::ZeroInit(_) => Some(0),
            _ => None,
        }
    }

    fn init_to_u8(init: &StaticInit) -> Option<u8> {
        match init {
            StaticInit::CharInit(v) => Some(*v as u8),
            StaticInit::UCharInit(v) => Some(*v),
            StaticInit::ZeroInit(_) => Some(0),
            _ => None,
        }
    }

    fn put_byte_masked(&mut self, offset: usize, value: u8, mask: u8) -> TackyResult<()> {
        if mask == 0 {
            return Ok(());
        }
        if let Some(pos) = self.pieces.iter().position(|(existing_offset, existing)| {
            *existing_offset == offset && TackyGen::static_init_size(existing) == 1
        }) {
            let current = Self::init_to_u8(&self.pieces[pos].1)
                .ok_or_else(|| "cannot merge byte static initializer".to_string())?;
            self.pieces[pos].1 = StaticInit::UCharInit((current & !mask) | (value & mask));
            return Ok(());
        }
        self.put(offset, StaticInit::UCharInit(value & mask))
    }

    fn put_bit_field(
        &mut self,
        offset: usize,
        storage_type: CType,
        value: i64,
        bit_offset: u8,
        bit_width: u8,
    ) -> TackyResult<()> {
        let storage_bits = (storage_type.size() as usize).saturating_mul(8);
        let field_end = (bit_offset as usize)
            .saturating_add(bit_width as usize)
            .min(storage_bits);
        if field_end <= bit_offset as usize {
            return Ok(());
        }
        let value_mask = if bit_width as usize >= 128 {
            u128::MAX
        } else {
            (1_u128 << bit_width) - 1
        };
        let shifted = ((value as u128) & value_mask) << bit_offset;
        let first_byte = bit_offset as usize / 8;
        let last_byte = (field_end - 1) / 8;
        for byte_index in first_byte..=last_byte {
            let byte_start_bit = byte_index * 8;
            let field_start_in_byte = (bit_offset as usize).saturating_sub(byte_start_bit);
            let field_end_in_byte = field_end.saturating_sub(byte_start_bit).min(8);
            let width = field_end_in_byte - field_start_in_byte;
            let mask = (((1u16 << width) - 1) << field_start_in_byte) as u8;
            let byte_value = ((shifted >> byte_start_bit) & 0xff) as u8;
            self.put_byte_masked(offset + byte_index, byte_value, mask)?;
        }
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
    current_function_params: Vec<String>,
    label_address_function: Option<String>,
    local_label_stack: Vec<HashSet<String>>,
    current_label_bodies: HashMap<String, Statement>,
    current_escaped_functions: HashSet<String>,
    /// Hidden return pointer name for functions returning large structs
    hidden_ret_ptr: Option<String>,
    static_vars: Vec<TackyStaticVar>,
    static_constants: Vec<TackyStaticConstant>,
    static_const_values: HashMap<String, (i64, bool, bool)>,
    extern_vars: Vec<String>,
    /// CType for each variable/temporary (for codegen output)
    symbol_types: HashMap<String, CType>,
    /// Rich type info (tracks arrays, pointer targets)
    full_types: HashMap<String, FullType>,
    /// Function types: (return_type, param_types, return_ptr_info)
    func_types: HashMap<String, FunctionTypeInfo>,
    /// Names that are functions, even when their prototype is not visible at
    /// the current source position.
    function_symbols: HashSet<String>,
    /// Function return full types
    func_full_types: HashMap<String, FullType>,
    /// Function parameter full types.
    func_param_full_types: HashMap<String, Vec<FullType>>,
    old_style_functions: HashSet<String>,
    zero_fixed_variadic_functions: HashSet<String>,
    vla_param_bounds: HashMap<String, Exp>,
    dynamic_sizes: HashMap<String, Exp>,
    /// Scalar type cache for variables and temporaries.
    var_types: HashMap<String, CType>,
    symbol_alignments: HashMap<String, usize>,
    ptr_info: HashMap<String, (CType, usize)>,
    bit_precisions: HashMap<String, u8>,
    /// Array storage sizes for stack allocation
    array_sizes: HashMap<String, usize>,
    /// Struct definitions
    struct_defs: HashMap<String, StructDef>,
    transparent_unions: HashMap<String, FullType>,
    nested_functions: Vec<TackyFunction>,
    instrument_functions: bool,
    permissive: bool,
    no_instrument_functions: std::collections::HashSet<String>,
    inline_va_arg_pack_functions: HashMap<String, FunctionDeclaration>,
    nested_capture_slots: HashMap<String, Vec<(String, String)>>,
}

impl TackyGen {
    fn new() -> Self {
        TackyGen {
            tmp_counter: 0,
            label_counter: 0,
            string_counter: 0,
            instructions: Vec::new(),
            current_function: String::new(),
            current_function_params: Vec::new(),
            label_address_function: None,
            local_label_stack: Vec::new(),
            current_label_bodies: HashMap::new(),
            current_escaped_functions: HashSet::new(),
            hidden_ret_ptr: None,
            static_vars: Vec::new(),
            static_constants: Vec::new(),
            static_const_values: HashMap::new(),
            extern_vars: Vec::new(),
            symbol_types: HashMap::new(),
            full_types: HashMap::new(),
            func_types: HashMap::new(),
            function_symbols: HashSet::new(),
            func_full_types: HashMap::new(),
            func_param_full_types: HashMap::new(),
            old_style_functions: HashSet::new(),
            zero_fixed_variadic_functions: HashSet::new(),
            vla_param_bounds: HashMap::new(),
            dynamic_sizes: HashMap::new(),
            var_types: HashMap::new(),
            symbol_alignments: HashMap::new(),
            ptr_info: HashMap::new(),
            bit_precisions: HashMap::new(),
            array_sizes: HashMap::new(),
            struct_defs: HashMap::new(),
            transparent_unions: HashMap::new(),
            nested_functions: Vec::new(),
            instrument_functions: false,
            permissive: false,
            no_instrument_functions: std::collections::HashSet::new(),
            inline_va_arg_pack_functions: HashMap::new(),
            nested_capture_slots: HashMap::new(),
        }
    }

    fn instrumentation_call(&mut self, hook: &str) -> Vec<TackyInstr> {
        self.extern_vars.push(self.current_function.clone());
        let fn_addr = self.fresh_tmp(CType::Pointer);
        let dst = self.fresh_tmp(CType::Void);
        vec![
            TackyInstr::GetAddress {
                src: TackyVal::Var(self.current_function.clone()),
                dst: fn_addr.clone(),
            },
            TackyInstr::FunCall {
                name: hook.to_string(),
                args: vec![fn_addr, TackyVal::Constant(0)],
                dst,
                stack_arg_indices: std::collections::HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 2,
                hidden_return: false,
                indirect: false,
            },
        ]
    }

    fn apply_function_instrumentation(&mut self, no_instrument_function: bool) {
        if !self.instrument_functions
            || no_instrument_function
            || self
                .no_instrument_functions
                .contains(&self.current_function)
            || matches!(
                self.current_function.as_str(),
                "__cyg_profile_func_enter" | "__cyg_profile_func_exit"
            )
        {
            return;
        }

        let mut instrumented = self.instrumentation_call("__cyg_profile_func_enter");
        let body = std::mem::take(&mut self.instructions);
        for instr in body {
            if matches!(instr, TackyInstr::Return(_)) {
                instrumented.extend(self.instrumentation_call("__cyg_profile_func_exit"));
            }
            instrumented.push(instr);
        }
        self.instructions = instrumented;
    }

    fn string_array_initializer(init: &Exp) -> Option<&String> {
        match init {
            Exp::StringLiteral(s) | Exp::WideStringLiteral(s) => Some(s),
            Exp::ArrayInit(elems) => match elems.as_slice() {
                [Exp::StringLiteral(s)] | [Exp::WideStringLiteral(s)] => Some(s),
                _ => None,
            },
            _ => None,
        }
    }

    fn static_pointer_initializer(&mut self, init: &Exp) -> Option<StaticInit> {
        match self.static_address_constant(init)? {
            (Some(label), 0) => Some(StaticInit::PointerInit(label)),
            (Some(label), offset) => Some(StaticInit::PointerInitOffset(label, offset)),
            (None, offset) => Some(StaticInit::LongInit(offset)),
        }
    }

    fn static_symbol_offset_integer_initializer(
        &mut self,
        init: &Exp,
        ctype: CType,
    ) -> Option<StaticInit> {
        if !matches!(ctype, CType::Long | CType::ULong) {
            return None;
        }
        match self.static_address_constant(init)? {
            (Some(label), 0) => Some(StaticInit::PointerInit(label)),
            (Some(label), offset) => Some(StaticInit::PointerInitOffset(label, offset)),
            (None, _) => None,
        }
    }

    fn static_label_diff_initializer(&self, init: &Exp, ctype: CType) -> Option<StaticInit> {
        let Exp::Binary(BinaryOp::Sub, left, right) = init else {
            return None;
        };
        let (Exp::LabelAddress(left_label), Exp::LabelAddress(right_label)) =
            (left.as_ref(), right.as_ref())
        else {
            return None;
        };
        let bytes = match ctype {
            CType::Char | CType::SChar | CType::UChar => 1,
            CType::Short | CType::UShort => 2,
            CType::Int | CType::UInt => 4,
            CType::Long | CType::ULong | CType::Pointer => 8,
            _ => return None,
        };
        let function = self
            .label_address_function
            .as_ref()
            .unwrap_or(&self.current_function);
        Some(StaticInit::LabelDiffInit(
            format!("label.{}.{}", function, left_label),
            format!("label.{}.{}", function, right_label),
            bytes,
        ))
    }

    fn static_pointer_diff_integer(&mut self, init: &Exp) -> Option<i64> {
        match init {
            Exp::Cast(_, _, inner) => self.static_pointer_diff_integer(inner),
            Exp::Binary(BinaryOp::Add, left, right) => {
                if let Some(diff) = self.static_pointer_diff_integer(left) {
                    let (value, _, _) = eval_static_integer_constant_exp_with_context(
                        right,
                        &self.struct_defs,
                        &self.full_types,
                    )?;
                    return Some(diff.wrapping_add(value));
                }
                if let Some(diff) = self.static_pointer_diff_integer(right) {
                    let (value, _, _) = eval_static_integer_constant_exp_with_context(
                        left,
                        &self.struct_defs,
                        &self.full_types,
                    )?;
                    return Some(value.wrapping_add(diff));
                }
                None
            }
            Exp::Binary(BinaryOp::Sub, left, right) => {
                if let Some(diff) = self.static_pointer_diff_integer(left) {
                    let (value, _, _) = eval_static_integer_constant_exp_with_context(
                        right,
                        &self.struct_defs,
                        &self.full_types,
                    )?;
                    return Some(diff.wrapping_sub(value));
                }
                if let Some(diff) = Self::static_same_string_lvalue_diff(left, right) {
                    return Some(diff);
                }
                let (left_label, left_offset) = self.static_address_constant(left)?;
                let (right_label, right_offset) = self.static_address_constant(right)?;
                if left_label != right_label {
                    return None;
                }
                let elem_size = match self.static_exp_full_type(left) {
                    Some(FullType::Pointer(pointee)) => {
                        pointee.byte_size_with(&self.struct_defs) as i64
                    }
                    Some(FullType::Array { elem, .. }) => {
                        elem.byte_size_with(&self.struct_defs) as i64
                    }
                    _ => 1,
                };
                if elem_size == 0 {
                    return None;
                }
                let byte_diff = left_offset - right_offset;
                (byte_diff % elem_size == 0).then_some(byte_diff / elem_size)
            }
            _ => None,
        }
    }

    fn static_same_string_lvalue_diff(left: &Exp, right: &Exp) -> Option<i64> {
        let (left_value, left_offset, elem_size) = Self::static_string_lvalue_address(left)?;
        let (right_value, right_offset, right_elem_size) =
            Self::static_string_lvalue_address(right)?;
        if left_value != right_value || elem_size != right_elem_size || elem_size == 0 {
            return None;
        }
        let byte_diff = left_offset - right_offset;
        (byte_diff % elem_size == 0).then_some(byte_diff / elem_size)
    }

    fn static_string_lvalue_address(exp: &Exp) -> Option<(&str, i64, i64)> {
        match exp {
            Exp::Unary(UnaryOp::AddrOf, inner) => Self::static_string_lvalue_address(inner),
            Exp::Cast(_, _, inner) => Self::static_string_lvalue_address(inner),
            Exp::Subscript(arr, idx) => {
                let (value, elem_size) = match arr.as_ref() {
                    Exp::StringLiteral(s) => (s.as_str(), 1),
                    Exp::WideStringLiteral(s) => (s.as_str(), CType::Int.size() as i64),
                    _ => return None,
                };
                let (index, _, _) = eval_static_integer_constant_exp(idx)?;
                Some((value, index * elem_size, elem_size))
            }
            _ => None,
        }
    }

    fn static_address_constant(&mut self, exp: &Exp) -> Option<(Option<String>, i64)> {
        match exp {
            Exp::Var(name) => Some((Some(name.clone()), 0)),
            Exp::LabelAddress(label) => {
                let function = self
                    .label_address_function
                    .as_ref()
                    .unwrap_or(&self.current_function);
                Some((Some(format!("label.{}.{}", function, label)), 0))
            }
            Exp::StringLiteral(s) => Some((Some(self.make_string_constant(s)), 0)),
            Exp::Constant(value) | Exp::LongConstant(value) => Some((None, *value)),
            Exp::UIntConstant(value) | Exp::ULongConstant(value) => Some((None, *value)),
            Exp::Cast(_, _, inner) => self.static_address_constant(inner),
            Exp::Unary(UnaryOp::AddrOf, inner) => self.static_lvalue_address_constant(inner),
            Exp::Unary(UnaryOp::Deref, inner) => self.static_address_constant(inner),
            Exp::Binary(BinaryOp::Add, left, right) => self
                .static_address_add_constant(left, right, 1)
                .or_else(|| self.static_address_add_constant(right, left, 1)),
            Exp::Binary(BinaryOp::Sub, left, right) => {
                self.static_address_add_constant(left, right, -1)
            }
            _ if matches!(self.static_exp_full_type(exp), Some(FullType::Array { .. })) => {
                self.static_lvalue_address_constant(exp)
            }
            _ => None,
        }
    }

    fn static_address_add_constant(
        &mut self,
        address: &Exp,
        constant: &Exp,
        sign: i64,
    ) -> Option<(Option<String>, i64)> {
        let (base, offset) = self.static_address_constant(address)?;
        let (value, _, _) = eval_static_integer_constant_exp_with_context(
            constant,
            &self.struct_defs,
            &self.full_types,
        )?;
        let scale = match self.static_exp_full_type(address) {
            Some(FullType::Array { elem, .. }) => elem.byte_size_with(&self.struct_defs) as i64,
            Some(FullType::Pointer(pointee)) => pointee.byte_size_with(&self.struct_defs) as i64,
            _ => 1,
        };
        Some((base, offset + sign * value * scale))
    }

    fn static_exp_full_type(&self, exp: &Exp) -> Option<FullType> {
        match exp {
            Exp::Dot(inner, member) => self.static_struct_member_full_type(inner, member),
            Exp::Arrow(inner, member) => {
                let tag = match self.static_exp_full_type(inner)? {
                    FullType::Pointer(pointee) => match *pointee {
                        FullType::Struct(tag) => tag,
                        _ => return None,
                    },
                    FullType::Array { elem, .. } => match *elem {
                        FullType::Struct(tag) => tag,
                        _ => return None,
                    },
                    _ => return None,
                };
                self.struct_defs
                    .get(&tag)?
                    .members
                    .iter()
                    .find(|m| m.name == *member)
                    .map(|m| m.member_full_type.clone())
            }
            Exp::Subscript(arr, _) => match self.static_exp_full_type(arr)? {
                FullType::Array { elem, .. } => Some(*elem),
                FullType::Pointer(pointee) => Some(*pointee),
                _ => None,
            },
            Exp::Binary(BinaryOp::Add | BinaryOp::Sub, left, right) => self
                .static_exp_full_type(left)
                .filter(|ft| matches!(ft, FullType::Array { .. } | FullType::Pointer(_)))
                .or_else(|| {
                    self.static_exp_full_type(right)
                        .filter(|ft| matches!(ft, FullType::Array { .. } | FullType::Pointer(_)))
                }),
            Exp::Unary(UnaryOp::AddrOf, inner) => Some(FullType::Pointer(Box::new(
                self.static_exp_full_type(inner)?,
            ))),
            Exp::Unary(UnaryOp::Deref, inner) => match self.static_exp_full_type(inner)? {
                FullType::Pointer(pointee) => Some(*pointee),
                _ => None,
            },
            _ => eval_static_expr_full_type(exp, &self.full_types),
        }
    }

    fn static_struct_member_full_type(&self, inner: &Exp, member: &str) -> Option<FullType> {
        let tag = match self.static_exp_full_type(inner)? {
            FullType::Struct(tag) => tag,
            _ => return None,
        };
        self.struct_defs
            .get(&tag)?
            .members
            .iter()
            .find(|m| m.name == member)
            .map(|m| m.member_full_type.clone())
    }

    fn static_lvalue_address_constant(&mut self, exp: &Exp) -> Option<(Option<String>, i64)> {
        match exp {
            Exp::Var(name) => Some((Some(name.clone()), 0)),
            Exp::Cast(_, Some(ft), inner) if matches!(inner.as_ref(), Exp::ArrayInit(_)) => {
                let label = self.make_static_compound_literal(ft, inner).ok()?;
                Some((Some(label), 0))
            }
            Exp::Dot(inner, member) => {
                let (base, offset) = self.static_lvalue_address_constant(inner)?;
                let tag = match self.static_exp_full_type(inner)? {
                    FullType::Struct(tag) => tag,
                    _ => return None,
                };
                let member_offset = self
                    .struct_defs
                    .get(&tag)?
                    .members
                    .iter()
                    .find(|m| m.name == *member)?
                    .offset as i64;
                Some((base, offset + member_offset))
            }
            Exp::Arrow(inner, member) => {
                let (base, offset) = self.static_address_constant(inner)?;
                let tag = match self.static_exp_full_type(inner)? {
                    FullType::Pointer(pointee) => match *pointee {
                        FullType::Struct(tag) => tag,
                        _ => return None,
                    },
                    FullType::Array { elem, .. } => match *elem {
                        FullType::Struct(tag) => tag,
                        _ => return None,
                    },
                    _ => return None,
                };
                let member_offset = self
                    .struct_defs
                    .get(&tag)?
                    .members
                    .iter()
                    .find(|m| m.name == *member)?
                    .offset as i64;
                Some((base, offset + member_offset))
            }
            Exp::Subscript(arr, idx) => {
                let (base, offset) = self
                    .static_address_constant(arr)
                    .or_else(|| self.static_lvalue_address_constant(arr))?;
                let elem_size = match self.static_exp_full_type(arr)? {
                    FullType::Array { elem, .. } => elem.byte_size_with(&self.struct_defs),
                    FullType::Pointer(pointee) => pointee.byte_size_with(&self.struct_defs),
                    _ => return None,
                } as i64;
                let (index, _, _) = eval_static_integer_constant_exp_with_context(
                    idx,
                    &self.struct_defs,
                    &self.full_types,
                )?;
                Some((base, offset + index * elem_size))
            }
            Exp::Unary(UnaryOp::Deref, inner) => self.static_address_constant(inner),
            Exp::Cast(_, _, inner) => self.static_lvalue_address_constant(inner),
            _ => None,
        }
    }

    fn static_aggregate_initializer(init: &Exp) -> Option<&Exp> {
        match init {
            Exp::ArrayInit(_) | Exp::WideStringLiteral(_) => Some(init),
            Exp::Cast(_, _, inner) if matches!(inner.as_ref(), Exp::ArrayInit(_)) => Some(inner),
            _ => None,
        }
    }

    fn is_one_dimensional_char_array(ft: &FullType) -> bool {
        matches!(
            ft,
            FullType::Array { elem, .. }
                if matches!(
                    elem.as_ref(),
                    FullType::Scalar(CType::Char | CType::SChar | CType::UChar)
                )
        )
    }

    fn next_tmp_name(&mut self) -> String {
        loop {
            let name = format!("__tmp.{}", self.tmp_counter);
            self.tmp_counter += 1;
            if !self.symbol_types.contains_key(&name)
                && !self.var_types.contains_key(&name)
                && !self.full_types.contains_key(&name)
                && !self.array_sizes.contains_key(&name)
            {
                return name;
            }
        }
    }

    fn fresh_var_name(&mut self) -> String {
        self.next_tmp_name()
    }

    fn zero_init_local(&mut self, name: &str, total_bytes: usize) {
        if total_bytes > INLINE_ZERO_INIT_LIMIT {
            let addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(name.to_string()),
                dst: addr.clone(),
            });
            let size = self.fresh_tmp(CType::ULong);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(total_bytes as i64),
                dst: size.clone(),
            });
            let dst = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::FunCall {
                name: "memset".to_string(),
                args: vec![addr, TackyVal::Constant(0), size],
                dst,
                stack_arg_indices: std::collections::HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 3,
                hidden_return: false,
                indirect: false,
            });
            return;
        }

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
        let name = self.next_tmp_name();
        self.symbol_types.insert(name.clone(), t);
        TackyVal::Var(name)
    }

    fn storage_ctype_for_full(&self, ft: &FullType) -> CType {
        if !ft.is_vector() {
            return ft.to_ctype();
        }
        match ft.byte_size_with(&self.struct_defs) {
            0 | 1 => CType::UChar,
            2 => CType::UShort,
            3 | 4 => CType::UInt,
            5..=8 => CType::ULong,
            _ => CType::UInt128,
        }
    }

    fn fresh_tmp_full(&mut self, ft: &FullType) -> TackyVal {
        let name = self.next_tmp_name();
        let ct = self.storage_ctype_for_full(ft);
        self.symbol_types.insert(name.clone(), ct);
        self.var_types.insert(name.clone(), ct);
        self.full_types.insert(name.clone(), ft.clone());
        // Keep pointer-depth metadata in sync with the canonical FullType.
        if let FullType::Pointer(ref inner) = ft {
            let (base, depth) = Self::ptr_info_from_full(inner);
            self.ptr_info.insert(name.clone(), (base, depth));
        }
        if ft.is_vector() {
            self.array_sizes
                .insert(name.clone(), ft.byte_size_with(&self.struct_defs));
        }
        if let FullType::Struct(ref tag) = ft {
            if let Some(def) = self.struct_defs.get(tag) {
                self.array_sizes.insert(name.clone(), def.size);
            }
        }
        TackyVal::Var(name)
    }

    /// Register a variable with its full type
    fn register_var(&mut self, name: &str, ft: FullType) {
        let ct = self.storage_ctype_for_full(&ft);
        self.symbol_types.insert(name.to_string(), ct);
        self.var_types.insert(name.to_string(), ct);
        self.full_types.insert(name.to_string(), ft.clone());

        // Keep pointer-depth metadata in sync with the canonical FullType.
        if let FullType::Pointer(ref inner) = ft {
            let (base, depth) = Self::ptr_info_from_full(inner);
            self.ptr_info.insert(name.to_string(), (base, depth));
        }

        // Track aggregate-sized scalar storage.
        if ft.is_array() || ft.is_vector() {
            self.array_sizes
                .insert(name.to_string(), ft.byte_size_with(&self.struct_defs));
        }
        if let FullType::Struct(ref tag) = ft {
            if let Some(def) = self.struct_defs.get(tag) {
                self.array_sizes.insert(name.to_string(), def.size);
            }
        }
    }

    fn register_dynamic_size(&mut self, name: &str, size: Option<Box<Exp>>) {
        if let Some(size) = size {
            self.dynamic_sizes.insert(name.to_string(), *size);
        }
    }

    fn copy_dynamic_size(&mut self, src: &str, dst: &TackyVal) {
        if let (Some(size), TackyVal::Var(dst_name)) = (self.dynamic_sizes.get(src).cloned(), dst) {
            self.dynamic_sizes.insert(dst_name.clone(), size);
        }
    }

    fn emit_dynamic_size(&mut self, size: Exp) -> TackyResult<TackyVal> {
        let (val, ty) = self.emit_exp(size)?;
        Ok(self.convert_to(val, ty, CType::Long))
    }

    fn emit_memcpy(&mut self, dst: TackyVal, src: TackyVal, size: TackyVal) -> TackyVal {
        let result = self.fresh_tmp(CType::Pointer);
        self.emit(TackyInstr::FunCall {
            name: "memcpy".to_string(),
            args: vec![dst, src, size],
            dst: result.clone(),
            stack_arg_indices: std::collections::HashSet::new(),
            memory_arg_blocks: Vec::new(),
            struct_arg_groups: Vec::new(),
            variadic: false,
            fixed_flat_arg_count: 3,
            hidden_return: false,
            indirect: false,
        });
        result
    }

    fn emit_aligned_pointer(&mut self, ptr: TackyVal, align: usize) -> TackyVal {
        if align <= 8 {
            return ptr;
        }
        let added = self.fresh_tmp(CType::Pointer);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: ptr,
            right: TackyVal::Constant((align - 1) as i64),
            dst: added.clone(),
        });
        let aligned = self.fresh_tmp(CType::Pointer);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: added,
            right: TackyVal::Constant(-(align as i64)),
            dst: aligned.clone(),
        });
        aligned
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
            FullType::Vector { elem, .. } => (elem.to_ctype(), 1),
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

    fn va_arg_helper_type(suffix: &str) -> Option<CType> {
        match suffix {
            "char" => Some(CType::Char),
            "uchar" => Some(CType::UChar),
            "short" => Some(CType::Short),
            "ushort" => Some(CType::UShort),
            "int" => Some(CType::Int),
            "uint" => Some(CType::UInt),
            "long" => Some(CType::Long),
            "ulong" => Some(CType::ULong),
            "ptr" => Some(CType::Pointer),
            "float" => Some(CType::Double),
            "double" => Some(CType::Double),
            "long_double" => Some(CType::LongDouble),
            "int128" => Some(CType::Int128),
            "uint128" => Some(CType::UInt128),
            _ => None,
        }
    }

    fn resolve_struct_tag_name(&self, tag: &str) -> String {
        if self.struct_defs.contains_key(tag) {
            return tag.to_string();
        }
        let scoped_prefix = format!("{}.tag.", tag);
        self.struct_defs
            .keys()
            .find(|candidate| candidate.starts_with(&scoped_prefix))
            .cloned()
            .unwrap_or_else(|| tag.to_string())
    }

    fn builtin_function_info(name: &str) -> Option<BuiltinFunctionInfo> {
        let void_ptr = Self::void_pointer_type();
        let char_ptr = Self::char_pointer_type();
        match name {
            "__builtin_abort" => Some((
                "abort",
                CType::Void,
                FullType::Scalar(CType::Void),
                vec![],
                None,
            )),
            "__builtin_exit" => Some((
                "exit",
                CType::Void,
                FullType::Scalar(CType::Void),
                vec![CType::Int],
                None,
            )),
            "__builtin_printf" => Some((
                "printf",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer],
                None,
            )),
            "__builtin_sprintf" => Some((
                "sprintf",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer, CType::Pointer],
                None,
            )),
            "__builtin_puts" => Some((
                "puts",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer],
                None,
            )),
            "__builtin_stack_save" => Some((
                "__builtin_stack_save",
                CType::Pointer,
                void_ptr.clone(),
                vec![],
                Some((CType::Void, 1)),
            )),
            "__builtin_stack_restore" => Some((
                "__builtin_stack_restore",
                CType::Void,
                FullType::Scalar(CType::Void),
                vec![CType::Pointer],
                None,
            )),
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
            "__builtin_mempcpy" => Some((
                "mempcpy",
                CType::Pointer,
                void_ptr,
                vec![CType::Pointer, CType::Pointer, CType::ULong],
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
            "__builtin_stpcpy" => Some((
                "stpcpy",
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
            "__builtin_snprintf" => Some((
                "snprintf",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer, CType::ULong, CType::Pointer],
                Some((CType::Char, 1)),
            )),
            "__builtin_fabs" => Some((
                "fabs",
                CType::Double,
                FullType::Scalar(CType::Double),
                vec![CType::Double],
                None,
            )),
            "__builtin_fabsf" => Some((
                "fabsf",
                CType::Float,
                FullType::Scalar(CType::Float),
                vec![CType::Float],
                None,
            )),
            "__builtin_fabsl" => Some((
                "fabsl",
                CType::Double,
                FullType::Scalar(CType::Double),
                vec![CType::Double],
                None,
            )),
            "__builtin_copysign" => Some((
                "copysign",
                CType::Double,
                FullType::Scalar(CType::Double),
                vec![CType::Double, CType::Double],
                None,
            )),
            "__builtin_copysignf" => Some((
                "copysignf",
                CType::Float,
                FullType::Scalar(CType::Float),
                vec![CType::Float, CType::Float],
                None,
            )),
            "__builtin_copysignl" => Some((
                "copysignl",
                CType::Double,
                FullType::Scalar(CType::Double),
                vec![CType::Double, CType::Double],
                None,
            )),
            "__builtin_pow" => Some((
                "pow",
                CType::Double,
                FullType::Scalar(CType::Double),
                vec![CType::Double, CType::Double],
                None,
            )),
            "__builtin_powf" => Some((
                "powf",
                CType::Float,
                FullType::Scalar(CType::Float),
                vec![CType::Float, CType::Float],
                None,
            )),
            "__builtin_conjf" => Some((
                "conjf",
                CType::Float,
                FullType::Scalar(CType::Float),
                vec![CType::Float],
                None,
            )),
            "__builtin_conj" => Some((
                "conj",
                CType::Double,
                FullType::Scalar(CType::Double),
                vec![CType::Double],
                None,
            )),
            "__builtin_conjl" => Some((
                "conjl",
                CType::Double,
                FullType::Scalar(CType::Double),
                vec![CType::Double],
                None,
            )),
            "__builtin_isinf" => Some((
                "isinf",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Double],
                None,
            )),
            "__builtin_isinff" => Some((
                "isinf",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Float],
                None,
            )),
            "__builtin_isinfl" => Some((
                "isinf",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::LongDouble],
                None,
            )),
            "alloca" | "__builtin_alloca" => Some((
                "alloca",
                CType::Pointer,
                void_ptr,
                vec![CType::ULong],
                Some((CType::Void, 1)),
            )),
            "__builtin_malloc" => Some((
                "malloc",
                CType::Pointer,
                void_ptr,
                vec![CType::ULong],
                Some((CType::Void, 1)),
            )),
            "__builtin_free" => Some((
                "free",
                CType::Void,
                FullType::Scalar(CType::Void),
                vec![CType::Pointer],
                Some((CType::Void, 1)),
            )),
            "abs" | "__builtin_abs" => Some((
                "abs",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Int],
                None,
            )),
            "labs" | "__builtin_labs" => Some((
                "labs",
                CType::Long,
                FullType::Scalar(CType::Long),
                vec![CType::Long],
                None,
            )),
            "llabs" | "__builtin_llabs" => Some((
                "llabs",
                CType::Long,
                FullType::Scalar(CType::Long),
                vec![CType::Long],
                None,
            )),
            "__builtin_ffs" => Some((
                "ffs",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Int],
                None,
            )),
            "__builtin_setjmp" => Some((
                "setjmp",
                CType::Int,
                FullType::Scalar(CType::Int),
                vec![CType::Pointer],
                Some((CType::Void, 1)),
            )),
            "__builtin_longjmp" => Some((
                "longjmp",
                CType::Void,
                FullType::Scalar(CType::Void),
                vec![CType::Pointer, CType::Int],
                Some((CType::Void, 1)),
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

    fn compatible_pointer_pointees(&self, dst: &FullType, src: &FullType) -> bool {
        if dst == src || Self::is_void_pointer(&FullType::Pointer(Box::new(dst.clone()))) {
            return true;
        }
        match (dst, src) {
            (FullType::Scalar(a), FullType::Scalar(b)) => {
                *a != CType::Struct
                    && *b != CType::Struct
                    && *a != CType::Void
                    && *b != CType::Void
                    && a.size() == b.size()
            }
            _ => self.compatible_full_types(dst, src),
        }
    }

    fn has_unspecified_function_params(params: &[FullType], variadic: bool) -> bool {
        params.is_empty() && variadic
    }

    fn record_zero_fixed_variadic_function(&mut self, fd: &FunctionDeclaration) {
        if fd.zero_fixed_variadic {
            self.zero_fixed_variadic_functions.insert(fd.name.clone());
        } else {
            self.zero_fixed_variadic_functions.remove(&fd.name);
        }
    }

    fn canonical_param_full_type(&self, ft: &FullType) -> FullType {
        if let FullType::Struct(tag) = ft {
            if let Some(member_ft) = self.transparent_unions.get(tag) {
                return member_ft.clone();
            }
        }
        ft.clone()
    }

    fn canonical_param_full_types(&self, param_full_types: &[FullType]) -> Vec<FullType> {
        param_full_types
            .iter()
            .map(|ft| self.canonical_param_full_type(ft))
            .collect()
    }

    fn compatible_full_types(&self, dst: &FullType, src: &FullType) -> bool {
        if dst == src {
            return true;
        }
        match (dst, src) {
            (
                FullType::Vector {
                    complex: true,
                    elem,
                    ..
                },
                FullType::Vector {
                    complex: true,
                    elem: src_elem,
                    ..
                },
            ) => self.compatible_full_types(elem, src_elem),
            (
                FullType::Vector {
                    complex: true,
                    elem,
                    ..
                },
                FullType::Scalar(_),
            ) => self.compatible_full_types(elem, src),
            (
                FullType::Scalar(_),
                FullType::Vector {
                    complex: true,
                    elem,
                    ..
                },
            ) => self.compatible_full_types(dst, elem),
            (FullType::Scalar(CType::Bool), FullType::Pointer(_))
            | (FullType::Scalar(CType::Bool), FullType::Scalar(CType::Pointer)) => true,
            (FullType::Scalar(a), FullType::Scalar(b)) => {
                *a != CType::Struct && *b != CType::Struct && *a != CType::Void && *b != CType::Void
            }
            (
                FullType::Vector {
                    complex: false,
                    elem,
                    ..
                },
                FullType::Scalar(_),
            ) => self.compatible_full_types(elem, src),
            (
                FullType::Scalar(_),
                FullType::Vector {
                    complex: false,
                    elem,
                    ..
                },
            ) => self.compatible_full_types(dst, elem),
            (
                FullType::Vector {
                    complex: false,
                    elem: dst_elem,
                    ..
                },
                FullType::Vector {
                    complex: false,
                    elem: src_elem,
                    ..
                },
            ) => self.compatible_full_types(dst_elem, src_elem),
            (FullType::Pointer(_), FullType::Scalar(CType::Pointer))
            | (FullType::Scalar(CType::Pointer), FullType::Pointer(_)) => true,
            (FullType::Pointer(dst_inner), FullType::Function { .. }) => {
                self.compatible_pointer_pointees(dst_inner, src)
            }
            (
                FullType::Function {
                    return_type: dst_ret,
                    params: dst_params,
                    variadic: dst_variadic,
                },
                FullType::Function {
                    return_type: src_ret,
                    params: src_params,
                    variadic: src_variadic,
                },
            ) => {
                self.compatible_full_types(dst_ret, src_ret)
                    && (Self::has_unspecified_function_params(dst_params, *dst_variadic)
                        || Self::has_unspecified_function_params(src_params, *src_variadic))
            }
            (FullType::Pointer(dst_inner), FullType::Pointer(src_inner)) => {
                Self::is_void_pointer(dst)
                    || Self::is_void_pointer(src)
                    || matches!(
                        src_inner.as_ref(),
                        FullType::Array { elem, .. }
                            if self.compatible_full_types(dst_inner, elem)
                    )
                    || self.compatible_pointer_pointees(dst_inner, src_inner)
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
        if self.permissive
            && Self::is_pointer_like_full_type(dst)
            && Self::is_pointer_like_full_type(src)
        {
            return Ok(());
        }
        if Self::is_integer_full_type(dst) && matches!(src, FullType::Pointer(_))
            || matches!(dst, FullType::Pointer(_)) && Self::is_integer_full_type(src)
        {
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
                        | Exp::Assign(_, _)
                )
            {
                return Ok(());
            }
        }
        self.assert_assignable_full_type(dst, src, context)
    }

    fn is_pointer_like_full_type(ft: &FullType) -> bool {
        matches!(
            ft,
            FullType::Pointer(_) | FullType::Function { .. } | FullType::Scalar(CType::Pointer)
        )
    }

    fn is_integer_full_type(ft: &FullType) -> bool {
        matches!(
            ft,
            FullType::Scalar(
                CType::Bool
                    | CType::Char
                    | CType::SChar
                    | CType::UChar
                    | CType::Short
                    | CType::UShort
                    | CType::Int
                    | CType::UInt
                    | CType::Long
                    | CType::ULong
                    | CType::Int128
                    | CType::UInt128
            )
        )
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
            TackyVal::Int128Constant(_) => FullType::Scalar(CType::Int128),
            TackyVal::UInt128Constant(_) => FullType::Scalar(CType::UInt128),
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
            Exp::Int128Constant(_) => FullType::Scalar(CType::Int128),
            Exp::UIntConstant(_) => FullType::Scalar(CType::UInt),
            Exp::ULongConstant(_) => FullType::Scalar(CType::ULong),
            Exp::UInt128Constant(_) => FullType::Scalar(CType::UInt128),
            Exp::DoubleConstant(_) => FullType::Scalar(CType::Double),
            Exp::LongDoubleConstant(_) => FullType::Scalar(CType::LongDouble),
            Exp::ImaginaryIntConstant(_) => FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Int)),
                lanes: 2,
                complex: true,
            },
            Exp::ImaginaryDoubleConstant(_) => FullType::Vector {
                elem: Box::new(FullType::Scalar(CType::Double)),
                lanes: 2,
                complex: true,
            },
            Exp::StringLiteral(s) => FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size: c_string_byte_len(s) + 1,
            },
            Exp::WideStringLiteral(s) => FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Int)),
                size: s.chars().count() + 1,
            },
            Exp::Var(name) => self.get_full_type(name),
            Exp::LabelAddress(_) => FullType::Pointer(Box::new(FullType::Scalar(CType::Void))),
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
            Exp::Unary(UnaryOp::RealPart | UnaryOp::ImagPart, inner) => {
                match self.typeof_exp(inner) {
                    FullType::Vector {
                        elem,
                        complex: true,
                        ..
                    } => *elem,
                    inner_type => inner_type,
                }
            }
            Exp::Subscript(arr, _) => {
                let arr_ft = self.typeof_exp(arr);
                match arr_ft {
                    FullType::Array { elem, .. } => *elem,
                    FullType::Pointer(inner) => *inner,
                    FullType::Vector { elem, .. } => *elem,
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
                    FullType::Array { elem, .. } => *elem,
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
            | StaticInit::PointerInit(_)
            | StaticInit::PointerInitOffset(_, _) => 8,
            StaticInit::LabelDiffInit(_, _, bytes) => *bytes,
            StaticInit::Int128Init(_) | StaticInit::UInt128Init(_) => 16,
            StaticInit::FloatInit(_) => 4,
            StaticInit::LongDoubleInit(_) => 16,
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

    fn make_static_compound_literal(&mut self, ft: &FullType, init: &Exp) -> TackyResult<String> {
        let label = format!("__compound_literal_{}", self.string_counter);
        self.string_counter += 1;
        let total_bytes = ft.byte_size_with(&self.struct_defs);
        let alignment = ft.alignment_with(&self.struct_defs).max(1);
        let init_values = self.build_static_initializer(ft, init)?;
        self.register_var(&label, ft.clone());
        self.static_vars.push(TackyStaticVar {
            name: label.clone(),
            global: false,
            thread_local: false,
            alignment,
            init_values: if init_values.is_empty() {
                vec![StaticInit::ZeroInit(total_bytes)]
            } else {
                init_values
            },
        });
        Ok(label)
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
            TackyVal::Int128Constant(_) => CType::Int128,
            TackyVal::UInt128Constant(_) => CType::UInt128,
            TackyVal::DoubleConstant(_) => CType::Double,
            TackyVal::Var(name) => *self
                .symbol_types
                .get(name)
                .or_else(|| self.var_types.get(name))
                .unwrap_or(&CType::Int),
        }
    }

    fn integer_range_for_type(ty: CType) -> Option<(i128, i128)> {
        match ty {
            CType::Bool => Some((0, 1)),
            CType::Char | CType::SChar => Some((i8::MIN as i128, i8::MAX as i128)),
            CType::Short => Some((i16::MIN as i128, i16::MAX as i128)),
            CType::Int => Some((i32::MIN as i128, i32::MAX as i128)),
            CType::Long => Some((i64::MIN as i128, i64::MAX as i128)),
            CType::UChar => Some((0, u8::MAX as i128)),
            CType::UShort => Some((0, u16::MAX as i128)),
            CType::UInt => Some((0, u32::MAX as i128)),
            CType::ULong => Some((0, u64::MAX as i128)),
            _ => None,
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
        if to == CType::LongDouble {
            let src = if !from.is_floating()
                && (matches!(val, TackyVal::Constant(_)) || from.size() < CType::Int.size())
            {
                let promoted = if from.is_signed() {
                    CType::Int
                } else {
                    CType::UInt
                };
                let tmp = self.fresh_tmp(promoted);
                if from == promoted || matches!(val, TackyVal::Constant(_)) {
                    self.emit(TackyInstr::Copy {
                        src: val,
                        dst: tmp.clone(),
                    });
                } else if from.is_signed() {
                    self.emit(TackyInstr::SignExtend {
                        src: val,
                        dst: tmp.clone(),
                    });
                } else {
                    self.emit(TackyInstr::ZeroExtend {
                        src: val,
                        dst: tmp.clone(),
                    });
                }
                tmp
            } else {
                val
            };
            self.emit(TackyInstr::Copy {
                src,
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
        if from == CType::Double && matches!(to, CType::Int128 | CType::UInt128) {
            let intermediate_type = if to.is_signed() {
                CType::Long
            } else {
                CType::ULong
            };
            let intermediate = self.fresh_tmp(intermediate_type);
            if to.is_signed() {
                self.emit(TackyInstr::DoubleToInt {
                    src: val,
                    dst: intermediate.clone(),
                });
                self.emit(TackyInstr::SignExtend {
                    src: intermediate,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::DoubleToUInt {
                    src: val,
                    dst: intermediate.clone(),
                });
                self.emit(TackyInstr::ZeroExtend {
                    src: intermediate,
                    dst: dst.clone(),
                });
            }
            return dst;
        }
        if from == CType::Float && matches!(to, CType::Int128 | CType::UInt128) {
            let intermediate_type = if to.is_signed() {
                CType::Long
            } else {
                CType::ULong
            };
            let intermediate = self.fresh_tmp(intermediate_type);
            if to.is_signed() {
                self.emit(TackyInstr::FloatToInt {
                    src: val,
                    dst: intermediate.clone(),
                });
                self.emit(TackyInstr::SignExtend {
                    src: intermediate,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::FloatToUInt {
                    src: val,
                    dst: intermediate.clone(),
                });
                self.emit(TackyInstr::ZeroExtend {
                    src: intermediate,
                    dst: dst.clone(),
                });
            }
            return dst;
        }
        if from == CType::LongDouble && matches!(to, CType::Int128 | CType::UInt128) {
            let intermediate_type = if to.is_signed() {
                CType::Long
            } else {
                CType::ULong
            };
            let intermediate = self.fresh_tmp(intermediate_type);
            if to.is_signed() {
                self.emit(TackyInstr::DoubleToInt {
                    src: val,
                    dst: intermediate.clone(),
                });
                self.emit(TackyInstr::SignExtend {
                    src: intermediate,
                    dst: dst.clone(),
                });
            } else {
                self.emit(TackyInstr::DoubleToUInt {
                    src: val,
                    dst: intermediate.clone(),
                });
                self.emit(TackyInstr::ZeroExtend {
                    src: intermediate,
                    dst: dst.clone(),
                });
            }
            return dst;
        }
        if from == CType::LongDouble && !to.is_floating() {
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
            Exp::LabelAddress(label) => {
                let function = self.label_address_owner(&label);
                let dst = self
                    .fresh_tmp_full(&FullType::Pointer(Box::new(FullType::Scalar(CType::Void))));
                self.emit(TackyInstr::LoadLabelAddress(
                    format!("label.{}.{}", function, label),
                    dst.clone(),
                ));
                Ok((dst, CType::Pointer))
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
            Exp::Int128Constant(val) => {
                let dst = self.fresh_tmp(CType::Int128);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Int128Constant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::Int128))
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
            Exp::UInt128Constant(val) => {
                let dst = self.fresh_tmp(CType::UInt128);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::UInt128Constant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::UInt128))
            }
            Exp::DoubleConstant(val) => {
                let dst = self.fresh_tmp(CType::Double);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::Double))
            }
            Exp::LongDoubleConstant(val) => {
                let dst = self.fresh_tmp(CType::LongDouble);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(val),
                    dst: dst.clone(),
                });
                Ok((dst, CType::LongDouble))
            }
            Exp::ImaginaryIntConstant(val) => {
                let ft = FullType::Vector {
                    elem: Box::new(FullType::Scalar(CType::Int)),
                    lanes: 2,
                    complex: true,
                };
                let dst = self.fresh_tmp_full(&ft);
                let TackyVal::Var(dst_name) = dst.clone() else {
                    return Err("complex literal result must be addressable".to_string());
                };
                self.zero_init_local(&dst_name, ft.byte_size_with(&self.struct_defs));
                self.emit(TackyInstr::CopyToOffset {
                    src: TackyVal::Constant(val),
                    dst_name,
                    offset: CType::Int.size() as i64,
                });
                Ok((dst, CType::Int))
            }
            Exp::ImaginaryDoubleConstant(val) => {
                let ft = FullType::Vector {
                    elem: Box::new(FullType::Scalar(CType::Double)),
                    lanes: 2,
                    complex: true,
                };
                let dst = self.fresh_tmp_full(&ft);
                let TackyVal::Var(dst_name) = dst.clone() else {
                    return Err("complex literal result must be addressable".to_string());
                };
                self.zero_init_local(&dst_name, ft.byte_size_with(&self.struct_defs));
                self.emit(TackyInstr::CopyToOffset {
                    src: TackyVal::DoubleConstant(val),
                    dst_name,
                    offset: CType::Double.size() as i64,
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
            Exp::WideStringLiteral(s) => {
                let label = self.make_string_constant(&wide_string_bytes(&s));
                let decayed_ft = FullType::Pointer(Box::new(FullType::Scalar(CType::Int)));
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
            Exp::Unary(op @ (UnaryOp::RealPart | UnaryOp::ImagPart), inner) => {
                self.emit_complex_lane_value(op, *inner)
            }
            Exp::Assign(left, right)
                if matches!(
                    left.as_ref(),
                    Exp::Unary(UnaryOp::RealPart | UnaryOp::ImagPart, _)
                ) =>
            {
                let Exp::Unary(op, inner) = *left else {
                    unreachable!();
                };
                self.emit_complex_lane_assignment(op, *inner, *right)
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
                        rhs.clone()
                    } else {
                        self.get_struct_addr(rhs.clone())
                    };
                    if let TackyVal::Var(ref ptr_name) = ptr {
                        if let Some(size_exp) = self.dynamic_sizes.get(ptr_name).cloned() {
                            let size = self.emit_dynamic_size(size_exp)?;
                            self.emit_memcpy(ptr.clone(), src_addr, size);
                            return Ok((rhs, rhs_type));
                        }
                    }
                    self.emit_struct_copy_ptr_to_ptr(src_addr, ptr.clone(), struct_size);
                    return Ok((rhs, rhs_type));
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
                if elem_ft.is_complex() {
                    self.emit_complex_value_to_ptr(ptr, &elem_ft, rhs.clone(), rhs_type, rhs_ft)?;
                    return Ok((rhs, elem_type));
                }
                let rhs_conv = self.convert_to(rhs, rhs_type, elem_type);
                self.emit(TackyInstr::Store {
                    src: rhs_conv.clone(),
                    dst_ptr: ptr,
                });
                Ok((rhs_conv, elem_type))
            }
            Exp::Assign(left, right) if matches!(left.as_ref(), Exp::Unary(UnaryOp::Deref, _)) => {
                let lhs_ft = self.typeof_exp(&left);
                if lhs_ft.is_struct() || lhs_ft.is_vector() {
                    let struct_size = lhs_ft.byte_size_with(&self.struct_defs);
                    let Exp::Unary(UnaryOp::Deref, ptr_exp) = *left else {
                        return Err("Expression is not a dereference lvalue".to_string());
                    };
                    let (ptr, _) = self.emit_exp(*ptr_exp)?;
                    let ptr_ft = self.val_full_type(&ptr);
                    let pointee_ft = match ptr_ft {
                        FullType::Pointer(inner) => *inner,
                        _ => lhs_ft.clone(),
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
                    if pointee_ft.is_complex() {
                        self.emit_complex_value_to_ptr(
                            ptr.clone(),
                            &pointee_ft,
                            rhs.clone(),
                            _rhs_type,
                            rhs_ft,
                        )?;
                        return Ok((rhs, lhs_ft.to_ctype()));
                    }
                    let src_addr = if let TackyVal::Var(ref n) = rhs {
                        if self.array_sizes.contains_key(n) {
                            let a = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: rhs.clone(),
                                dst: a.clone(),
                            });
                            a
                        } else {
                            rhs.clone()
                        }
                    } else {
                        let a = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: rhs.clone(),
                            dst: a.clone(),
                        });
                        a
                    };
                    self.emit_struct_copy_ptr_to_ptr(src_addr, ptr, struct_size);
                    return Ok((rhs, lhs_ft.to_ctype()));
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
                            rhs.clone()
                        } else {
                            self.get_struct_addr(rhs.clone())
                        };
                        self.emit_struct_copy_ptr_to_ptr(
                            src_addr,
                            dst_addr,
                            mem_ft.byte_size_with(&self.struct_defs),
                        );
                        return Ok((rhs, CType::Struct));
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
                    if mem_ft.is_complex() {
                        self.emit_complex_value_to_offset(
                            &struct_name,
                            &mem_ft,
                            rhs.clone(),
                            rhs_type,
                            rhs_ft,
                            mem.offset as i64,
                        )?;
                        return Ok((rhs, mem.member_type));
                    }
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
                    let mem = match left_exp {
                        Exp::Dot(ref inner, ref member) => {
                            let tag = self.dot_inner_tag(inner)?;
                            self.struct_member(&tag, member)?
                        }
                        _ => return Err("internal error: expected dot expression".to_string()),
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
                            rhs.clone()
                        } else {
                            self.get_struct_addr(rhs.clone())
                        };
                        self.emit_struct_copy_ptr_to_ptr(
                            src_addr,
                            dst_addr,
                            mem_ft.byte_size_with(&self.struct_defs),
                        );
                        return Ok((rhs, CType::Struct));
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
                    if mem_ft.is_complex() {
                        self.emit_complex_value_to_ptr(
                            member_addr,
                            &mem_ft,
                            rhs.clone(),
                            rhs_type,
                            rhs_ft,
                        )?;
                        return Ok((rhs, mem_type));
                    }
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
                        rhs.clone()
                    } else {
                        self.get_struct_addr(rhs.clone())
                    };
                    self.emit_struct_copy_ptr_to_ptr(
                        src_addr,
                        mem_ptr,
                        mem_ft.byte_size_with(&self.struct_defs),
                    );
                    return Ok((rhs, CType::Struct));
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
                if mem_ft.is_complex() {
                    self.emit_complex_value_to_ptr(
                        mem_ptr,
                        &mem_ft,
                        rhs.clone(),
                        rhs_type,
                        rhs_ft,
                    )?;
                    return Ok((rhs, mem_type));
                }
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
                if lhs_ft.is_struct() || lhs_ft.is_vector() {
                    let Exp::Var(lhs_name) = *left else {
                        return Err("Expression is not a variable lvalue".to_string());
                    };
                    let right_for_type = (*right).clone();
                    let (rhs, rhs_type) = self.emit_exp(*right)?;
                    let rhs_ft = self.val_full_type(&rhs);
                    self.assert_assignable_exp_full_type(
                        &lhs_ft,
                        &rhs_ft,
                        &right_for_type,
                        "assignment",
                    )?;
                    if lhs_ft.is_complex() {
                        self.emit_complex_value_to_offset(
                            &lhs_name, &lhs_ft, rhs, rhs_type, rhs_ft, 0,
                        )?;
                        return Ok((TackyVal::Var(lhs_name), lhs_ft.to_ctype()));
                    }
                    let struct_size = lhs_ft.byte_size_with(&self.struct_defs);
                    let rhs_struct_name = if rhs_type == CType::Struct || rhs_ft.is_vector() {
                        if let TackyVal::Var(ref n) = rhs {
                            Some(n.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let src_addr = if rhs_type == CType::Pointer {
                        Some(rhs.clone())
                    } else if rhs_ft.is_vector() {
                        Some(self.get_struct_addr(rhs.clone()))
                    } else {
                        None
                    };
                    if let Some(src_addr) = src_addr {
                        if let Some(size_exp) = self.dynamic_sizes.get(&lhs_name).cloned() {
                            let size = self.emit_dynamic_size(size_exp)?;
                            let dst_addr = self.get_struct_addr(TackyVal::Var(lhs_name.clone()));
                            self.emit_memcpy(dst_addr, src_addr, size);
                        } else {
                            self.emit_struct_copy_to(src_addr, &lhs_name, struct_size);
                        }
                    } else if let Some(src_name) = rhs_struct_name {
                        if let Some(size_exp) = self.dynamic_sizes.get(&lhs_name).cloned() {
                            let size = self.emit_dynamic_size(size_exp)?;
                            let src_addr = self.get_struct_addr(TackyVal::Var(src_name));
                            let dst_addr = self.get_struct_addr(TackyVal::Var(lhs_name.clone()));
                            self.emit_memcpy(dst_addr, src_addr, size);
                        } else {
                            self.emit(TackyInstr::CopyStruct {
                                src_name,
                                dst_name: lhs_name.clone(),
                            });
                        }
                    } else {
                        self.zero_init_local(&lhs_name, struct_size);
                        let rhs_conv = self.convert_to(rhs, rhs_type, lhs_ft.to_ctype());
                        self.emit(TackyInstr::CopyToOffset {
                            src: rhs_conv,
                            dst_name: lhs_name.clone(),
                            offset: 0,
                        });
                    }
                    return Ok((TackyVal::Var(lhs_name), lhs_ft.to_ctype()));
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
                            if !self.full_types.contains_key(lhs_name) {
                                if let Some(&info) = self.ptr_info.get(rhs_name) {
                                    self.ptr_info.insert(lhs_name.clone(), info);
                                }
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
                if matches!(
                    left.as_ref(),
                    Exp::Unary(UnaryOp::RealPart | UnaryOp::ImagPart, _)
                ) =>
            {
                let Exp::Unary(component_op, inner) = *left else {
                    unreachable!();
                };
                self.emit_complex_lane_compound_assignment(component_op, op, *inner, *right)
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
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let cur_val = self.fresh_tmp_full(&elem_full);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: cur_val.clone(),
                });
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
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let cur_val = self.fresh_tmp(pointee_type);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: cur_val.clone(),
                });
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
                let mem = match left.as_ref() {
                    Exp::Dot(inner, member) | Exp::Arrow(inner, member) => {
                        let tag = self.dot_inner_tag(inner)?;
                        self.struct_member(&tag, member)?
                    }
                    _ => return Err("internal error: expected dot or arrow expression".to_string()),
                };
                if mem.bit_width.is_some() {
                    let ptr = self.emit_dot_address(&left)?;
                    let (rhs, rhs_type) = self.emit_exp(*right)?;
                    let unit = self.fresh_tmp(mem.member_type);
                    self.emit(TackyInstr::Load {
                        src_ptr: ptr.clone(),
                        dst: unit.clone(),
                    });
                    let current = self.extract_bit_field(unit, &mem)?;
                    let current_type = mem
                        .bit_width
                        .map(|width| Self::bit_field_promoted_type(&mem, width))
                        .unwrap_or(mem.member_type);
                    let common = CType::common(current_type, rhs_type);
                    let lhs_conv = self.convert_to(current, current_type, common);
                    let rhs_conv = self.convert_to(rhs, rhs_type, common);
                    let result = self.fresh_tmp(common);
                    let tacky_op = Self::convert_binop(op)?;
                    self.emit(TackyInstr::Binary {
                        op: tacky_op,
                        left: lhs_conv,
                        right: rhs_conv,
                        dst: result.clone(),
                    });
                    let result_conv = self.convert_to(result, common, mem.member_type);
                    let value = self.store_bit_field_to_ptr(ptr, &mem, result_conv)?;
                    return Ok((value, mem.member_type));
                }
                let (ptr, lhs_type, lhs_ft) = self
                    .scalar_lvalue_address(*left)?
                    .ok_or_else(|| "Expression is not a simple lvalue".to_string())?;
                if lhs_ft.is_vector() {
                    let cur_val = self.fresh_tmp_full(&lhs_ft);
                    self.emit(TackyInstr::Load {
                        src_ptr: ptr.clone(),
                        dst: cur_val.clone(),
                    });
                    let TackyVal::Var(cur_name) = cur_val else {
                        return Err(
                            "vector compound assignment requires a named temporary".to_string()
                        );
                    };
                    let (result, _) = self.emit_binary(op, Exp::Var(cur_name), *right)?;
                    let src_addr = self.get_struct_addr(result);
                    self.emit_struct_copy_ptr_to_ptr(
                        src_addr,
                        ptr,
                        lhs_ft.byte_size_with(&self.struct_defs),
                    );
                    return Ok((TackyVal::Constant(0), lhs_type));
                }
                let (rhs, rhs_type) = self.emit_exp(*right)?;
                let cur_val = self.fresh_tmp_full(&lhs_ft);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: cur_val.clone(),
                });
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
                if lhs_ft.is_vector() {
                    let Exp::Var(lhs_name) = *left else {
                        return Err("Expression is not a variable lvalue".to_string());
                    };
                    let (result, _) = self.emit_binary(op, Exp::Var(lhs_name.clone()), *right)?;
                    let src_addr = self.get_struct_addr(result.clone());
                    self.emit_struct_copy_to(
                        src_addr,
                        &lhs_name,
                        lhs_ft.byte_size_with(&self.struct_defs),
                    );
                    return Ok((TackyVal::Var(lhs_name), lhs_ft.to_ctype()));
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
                    let result = if mem.bit_width.is_some() {
                        self.fresh_tmp(mem.member_type)
                    } else {
                        self.fresh_tmp_full(&mem_ft)
                    };
                    self.emit(TackyInstr::CopyFromOffset {
                        src_name: n.clone(),
                        offset: mem.offset as i64,
                        dst: result.clone(),
                    });
                    let result = self.extract_bit_field(result, &mem)?;
                    let result_type = mem
                        .bit_width
                        .map(|width| Self::bit_field_promoted_type(&mem, width))
                        .unwrap_or(mem.member_type);
                    Ok((result, result_type))
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
            Exp::BuiltinExpect(value, hints) => {
                let (val, val_type) = self.emit_exp(*value)?;
                let ft = self.val_full_type(&val);
                let saved = self.fresh_tmp_full(&ft);
                self.emit(TackyInstr::Copy {
                    src: val,
                    dst: saved.clone(),
                });
                for hint in hints {
                    self.emit_exp(hint)?;
                }
                Ok((saved, val_type))
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
                self.emit_unary(op, *inner)
            }
            Exp::Unary(UnaryOp::LogicalNot, inner) => {
                let (src, _) = self.emit_exp(*inner)?;
                let src_ft = self.val_full_type(&src);
                if src_ft.is_complex() {
                    let FullType::Vector { elem, .. } = src_ft.clone() else {
                        return Err("internal error: expected complex vector type".to_string());
                    };
                    let elem_type = elem.to_ctype();
                    let elem_size = elem.byte_size_with(&self.struct_defs);
                    let real = self.emit_complex_component_value(
                        src.clone(),
                        src_ft.clone(),
                        elem_type,
                        elem_size,
                        0,
                    )?;
                    let imag =
                        self.emit_complex_component_value(src, src_ft, elem_type, elem_size, 1)?;
                    let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                    let real_eq = self.fresh_tmp(CType::Int);
                    let imag_eq = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Equal,
                        left: real,
                        right: zero.clone(),
                        dst: real_eq.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Equal,
                        left: imag,
                        right: zero,
                        dst: imag_eq.clone(),
                    });
                    let dst = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::BitwiseAnd,
                        left: real_eq,
                        right: imag_eq,
                        dst: dst.clone(),
                    });
                    return Ok((dst, CType::Int));
                }
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

    fn label_address_owner(&self, label: &str) -> String {
        if self
            .local_label_stack
            .last()
            .is_some_and(|labels| labels.contains(label))
        {
            return self.current_function.clone();
        }
        self.label_address_function
            .as_ref()
            .unwrap_or(&self.current_function)
            .clone()
    }

    fn emit_var(&mut self, name: String) -> TackyResult<(TackyVal, CType)> {
        if self.function_symbols.contains(&name) {
            self.emit_nested_capture_updates(&name);
            let (return_type, _, _, variadic) =
                self.func_types
                    .get(&name)
                    .cloned()
                    .unwrap_or((CType::Int, Vec::new(), None, true));
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
                src: TackyVal::Var(name.clone()),
                dst: ptr.clone(),
            });
            self.copy_dynamic_size(&name, &ptr);
            return Ok((ptr, decayed.to_ctype()));
        }
        if matches!(ft, FullType::Function { .. }) {
            let ptr_ft = FullType::Pointer(Box::new(ft));
            let ptr = self.fresh_tmp_full(&ptr_ft);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(name),
                dst: ptr.clone(),
            });
            return Ok((ptr, CType::Pointer));
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
        let source_ft = self.typeof_exp(&inner);
        let (val, from_type) = self.emit_exp(inner)?;
        if let Some(ft) = cast_ft.as_ref() {
            if ft.is_complex() {
                let FullType::Vector { elem, .. } = ft.clone() else {
                    return Err("internal error: expected complex vector type".to_string());
                };
                let elem_type = elem.to_ctype();
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let result = self.fresh_tmp_full(ft);
                let total_bytes = ft.byte_size_with(&self.struct_defs);
                if let TackyVal::Var(ref result_name) = result {
                    self.zero_init_local(result_name, total_bytes);
                    let real = if source_ft.is_complex() {
                        self.emit_complex_component_value(
                            val.clone(),
                            source_ft.clone(),
                            elem_type,
                            elem_size,
                            0,
                        )?
                    } else {
                        self.convert_to(val.clone(), from_type, elem_type)
                    };
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    if source_ft.is_complex() {
                        let imag = self.emit_complex_component_value(
                            val, source_ft, elem_type, elem_size, 1,
                        )?;
                        self.emit(TackyInstr::CopyToOffset {
                            src: imag,
                            dst_name: result_name.clone(),
                            offset: elem_size as i64,
                        });
                    }
                }
                return Ok((result, target_type));
            }
        }
        if cast_ft.is_none()
            && source_ft.is_vector()
            && target_type.size() as usize == source_ft.byte_size_with(&self.struct_defs)
        {
            let target_ft = FullType::Scalar(target_type);
            let result = self.fresh_tmp_full(&target_ft);
            let src_addr = self.get_struct_addr(val);
            self.emit(TackyInstr::Load {
                src_ptr: src_addr,
                dst: result.clone(),
            });
            return Ok((result, target_type));
        }
        if let Some(ft) = cast_ft.as_ref() {
            if source_ft.is_vector()
                && matches!(ft, FullType::Scalar(_))
                && ft.byte_size_with(&self.struct_defs)
                    == source_ft.byte_size_with(&self.struct_defs)
            {
                let result = self.fresh_tmp_full(ft);
                let src_addr = self.get_struct_addr(val);
                self.emit(TackyInstr::Load {
                    src_ptr: src_addr,
                    dst: result.clone(),
                });
                return Ok((result, target_type));
            }
            if ft.is_vector()
                && matches!(source_ft, FullType::Scalar(_))
                && ft.byte_size_with(&self.struct_defs)
                    == source_ft.byte_size_with(&self.struct_defs)
            {
                let source = self.fresh_tmp(from_type);
                self.emit(TackyInstr::Copy {
                    src: val,
                    dst: source.clone(),
                });
                let src_addr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: source,
                    dst: src_addr.clone(),
                });
                let result = self.fresh_tmp_full(ft);
                let size = ft.byte_size_with(&self.struct_defs);
                if let TackyVal::Var(ref result_name) = result {
                    self.emit_struct_copy_to(src_addr, result_name, size);
                }
                return Ok((result, target_type));
            }
            if ft.is_vector()
                && source_ft.is_vector()
                && ft.byte_size_with(&self.struct_defs)
                    == source_ft.byte_size_with(&self.struct_defs)
            {
                let result = self.fresh_tmp_full(ft);
                let size = ft.byte_size_with(&self.struct_defs);
                if let TackyVal::Var(ref result_name) = result {
                    let src_addr = self.get_struct_addr(val);
                    self.emit_struct_copy_to(src_addr, result_name, size);
                }
                return Ok((result, target_type));
            }
        }
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
            self.array_sizes.insert(tmp_name.clone(), size);
            self.zero_init_local(&tmp_name, size);
            self.emit_struct_init_at(&tmp_name, &inner, tag, 0)?;
            return Ok((TackyVal::Var(tmp_name), target_type));
        }

        if ft.is_vector() {
            let result = self.fresh_tmp_full(&ft);
            let total_bytes = ft.byte_size_with(&self.struct_defs);
            if let TackyVal::Var(ref name) = result {
                self.zero_init_local(name, total_bytes);
            }
            if let Exp::ArrayInit(elems) = inner {
                let elem_ft = match &ft {
                    FullType::Vector { elem, .. } => elem.as_ref().clone(),
                    _ => FullType::Scalar(target_type),
                };
                let elem_type = elem_ft.to_ctype();
                let elem_size = elem_ft.byte_size_with(&self.struct_defs);
                if let TackyVal::Var(ref name) = result {
                    if ft.is_complex() && elems.len() == 1 {
                        let elem = elems.into_iter().next().unwrap();
                        if self.typeof_exp(&elem).is_complex() {
                            let (val, val_type) = self.emit_exp(elem)?;
                            let val_ft = self.val_full_type(&val);
                            self.emit_complex_value_to_offset(name, &ft, val, val_type, val_ft, 0)?;
                            return Ok((result, target_type));
                        }
                        let (val, from_type) = self.emit_exp(elem)?;
                        let converted = self.convert_to(val, from_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: name.clone(),
                            offset: 0,
                        });
                        return Ok((result, target_type));
                    }
                    for (index, elem) in elems.into_iter().enumerate() {
                        let (val, from_type) = self.emit_exp(elem)?;
                        let converted = self.convert_to(val, from_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: name.clone(),
                            offset: (index * elem_size) as i64,
                        });
                    }
                }
            }
            return Ok((result, target_type));
        }

        if ft.is_array() {
            let tmp_name = self.fresh_var_name();
            let ft = match (&ft, &inner) {
                (FullType::Array { elem, size: 0 }, Exp::ArrayInit(elems)) => FullType::Array {
                    elem: elem.clone(),
                    size: elems.len(),
                },
                _ => ft.clone(),
            };
            self.register_var(&tmp_name, ft.clone());
            let size = ft.byte_size_with(&self.struct_defs);
            self.array_sizes.insert(tmp_name.clone(), size);
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

    fn bit_builtin_signature(name: &str) -> Option<(BitBuiltinKind, CType, i64)> {
        let (kind, suffix) = if let Some(suffix) = name.strip_prefix("__builtin_ffs") {
            (BitBuiltinKind::Ffs, suffix)
        } else if let Some(suffix) = name.strip_prefix("__builtin_clz") {
            (BitBuiltinKind::Clz, suffix)
        } else if let Some(suffix) = name.strip_prefix("__builtin_ctz") {
            (BitBuiltinKind::Ctz, suffix)
        } else if let Some(suffix) = name.strip_prefix("__builtin_clrsb") {
            (BitBuiltinKind::Clrsb, suffix)
        } else if let Some(suffix) = name.strip_prefix("__builtin_popcount") {
            (BitBuiltinKind::Popcount, suffix)
        } else if let Some(suffix) = name.strip_prefix("__builtin_parity") {
            (BitBuiltinKind::Parity, suffix)
        } else {
            return None;
        };

        match suffix {
            "" => Some((kind, CType::UInt, 32)),
            "l" | "ll" => Some((kind, CType::ULong, 64)),
            _ => None,
        }
    }

    fn bit_mask_constant(width: i64, bit: i64) -> TackyVal {
        if width == 64 && bit == 63 {
            TackyVal::Constant(i64::MIN)
        } else {
            TackyVal::Constant(1_i64 << bit)
        }
    }

    fn emit_bit_builtin(
        &mut self,
        kind: BitBuiltinKind,
        arg_type: CType,
        width: i64,
        arg_exp: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let (arg, from_type) = self.emit_exp(arg_exp)?;
        let value = self.convert_to(arg, from_type, arg_type);
        let result = self.fresh_tmp(CType::Int);
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: result.clone(),
        });

        match kind {
            BitBuiltinKind::Ffs => {
                let mask = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(1),
                    dst: mask.clone(),
                });
                let index = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(1),
                    dst: index.clone(),
                });
                let loop_label = self.fresh_label("builtin_ffs_loop");
                let next_label = self.fresh_label("builtin_ffs_next");
                let end_label = self.fresh_label("builtin_ffs_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let exhausted = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterThan,
                    left: index.clone(),
                    right: TackyVal::Constant(width),
                    dst: exhausted.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(exhausted, end_label.clone()));
                let bit = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: value.clone(),
                    right: mask.clone(),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfZero(bit, next_label.clone()));
                self.emit(TackyInstr::Copy {
                    src: index.clone(),
                    dst: result.clone(),
                });
                self.emit(TackyInstr::Jump(end_label.clone()));
                self.emit(TackyInstr::Label(next_label));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftLeft,
                    left: mask.clone(),
                    right: TackyVal::Constant(1),
                    dst: mask,
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: index.clone(),
                    right: TackyVal::Constant(1),
                    dst: index,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            }
            BitBuiltinKind::Ctz => {
                let current = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Copy {
                    src: value,
                    dst: current.clone(),
                });
                let loop_label = self.fresh_label("builtin_ctz_loop");
                let end_label = self.fresh_label("builtin_ctz_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let exhausted = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterEqual,
                    left: result.clone(),
                    right: TackyVal::Constant(width),
                    dst: exhausted.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(exhausted, end_label.clone()));
                let bit = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: current.clone(),
                    right: TackyVal::Constant(1),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(bit, end_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: result.clone(),
                    right: TackyVal::Constant(1),
                    dst: result.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: current.clone(),
                    right: TackyVal::Constant(1),
                    dst: current,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            }
            BitBuiltinKind::Clz => {
                let mask = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Copy {
                    src: Self::bit_mask_constant(width, width - 1),
                    dst: mask.clone(),
                });
                let loop_label = self.fresh_label("builtin_clz_loop");
                let end_label = self.fresh_label("builtin_clz_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let exhausted = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterEqual,
                    left: result.clone(),
                    right: TackyVal::Constant(width),
                    dst: exhausted.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(exhausted, end_label.clone()));
                let bit = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: value.clone(),
                    right: mask.clone(),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(bit, end_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: result.clone(),
                    right: TackyVal::Constant(1),
                    dst: result.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: mask.clone(),
                    right: TackyVal::Constant(1),
                    dst: mask,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            }
            BitBuiltinKind::Clrsb => {
                let sign_mask = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Copy {
                    src: Self::bit_mask_constant(width, width - 1),
                    dst: sign_mask.clone(),
                });
                let sign_bit = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: value.clone(),
                    right: sign_mask.clone(),
                    dst: sign_bit.clone(),
                });
                let sign_is_zero = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Equal,
                    left: sign_bit,
                    right: TackyVal::Constant(0),
                    dst: sign_is_zero.clone(),
                });
                let mask = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: sign_mask,
                    right: TackyVal::Constant(1),
                    dst: mask.clone(),
                });
                let loop_label = self.fresh_label("builtin_clrsb_loop");
                let end_label = self.fresh_label("builtin_clrsb_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                self.emit(TackyInstr::JumpIfZero(mask.clone(), end_label.clone()));
                let bit = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: value.clone(),
                    right: mask.clone(),
                    dst: bit.clone(),
                });
                let bit_is_zero = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Equal,
                    left: bit,
                    right: TackyVal::Constant(0),
                    dst: bit_is_zero.clone(),
                });
                let different = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::NotEqual,
                    left: bit_is_zero,
                    right: sign_is_zero.clone(),
                    dst: different.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(different, end_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: result.clone(),
                    right: TackyVal::Constant(1),
                    dst: result.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: mask.clone(),
                    right: TackyVal::Constant(1),
                    dst: mask,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            }
            BitBuiltinKind::Popcount | BitBuiltinKind::Parity => {
                let mask = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(1),
                    dst: mask.clone(),
                });
                let index = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(0),
                    dst: index.clone(),
                });
                let loop_label = self.fresh_label("builtin_popcount_loop");
                let next_label = self.fresh_label("builtin_popcount_next");
                let end_label = self.fresh_label("builtin_popcount_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let exhausted = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterEqual,
                    left: index.clone(),
                    right: TackyVal::Constant(width),
                    dst: exhausted.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(exhausted, end_label.clone()));
                let bit = self.fresh_tmp(arg_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: value.clone(),
                    right: mask.clone(),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfZero(bit, next_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: result.clone(),
                    right: TackyVal::Constant(1),
                    dst: result.clone(),
                });
                self.emit(TackyInstr::Label(next_label));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftLeft,
                    left: mask.clone(),
                    right: TackyVal::Constant(1),
                    dst: mask,
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: index.clone(),
                    right: TackyVal::Constant(1),
                    dst: index,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));

                if matches!(kind, BitBuiltinKind::Parity) {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::BitwiseAnd,
                        left: result.clone(),
                        right: TackyVal::Constant(1),
                        dst: result.clone(),
                    });
                }
            }
        }

        Ok((result, CType::Int))
    }

    fn builtin_apply_target_name(exp: &Exp) -> Option<String> {
        match exp {
            Exp::Var(name) | Exp::FunctionCall(name, _) => Some(name.clone()),
            Exp::Cast(_, _, inner) => Self::builtin_apply_target_name(inner),
            Exp::Unary(UnaryOp::AddrOf, inner) => Self::builtin_apply_target_name(inner),
            _ => None,
        }
    }

    fn exp_contains_va_arg_pack(exp: &Exp) -> bool {
        match exp {
            Exp::FunctionCall(name, args) => {
                (name == "__builtin_va_arg_pack" && args.is_empty())
                    || args.iter().any(Self::exp_contains_va_arg_pack)
            }
            Exp::Cast(_, _, inner)
            | Exp::Unary(_, inner)
            | Exp::SizeOf(inner)
            | Exp::Dot(inner, _)
            | Exp::Arrow(inner, _) => Self::exp_contains_va_arg_pack(inner),
            Exp::Binary(_, left, right)
            | Exp::Assign(left, right)
            | Exp::CompoundAssign(_, left, right)
            | Exp::Subscript(left, right)
            | Exp::Comma(left, right) => {
                Self::exp_contains_va_arg_pack(left) || Self::exp_contains_va_arg_pack(right)
            }
            Exp::Conditional(cond, then_exp, else_exp) => {
                Self::exp_contains_va_arg_pack(cond)
                    || Self::exp_contains_va_arg_pack(then_exp)
                    || Self::exp_contains_va_arg_pack(else_exp)
            }
            Exp::BuiltinExpect(value, hints) => {
                Self::exp_contains_va_arg_pack(value)
                    || hints.iter().any(Self::exp_contains_va_arg_pack)
            }
            Exp::ArrayInit(elems) => elems.iter().any(Self::exp_contains_va_arg_pack),
            Exp::DesignatedInit(_, value) => Self::exp_contains_va_arg_pack(value),
            Exp::StatementExpr(block, tail, _) => {
                Self::block_contains_va_arg_pack(block)
                    || tail
                        .as_ref()
                        .is_some_and(|exp| Self::exp_contains_va_arg_pack(exp))
            }
            Exp::IndirectCall(callee, args) => {
                Self::exp_contains_va_arg_pack(callee)
                    || args.iter().any(Self::exp_contains_va_arg_pack)
            }
            Exp::AtomicFetch { ptr, arg, .. } => {
                Self::exp_contains_va_arg_pack(ptr) || Self::exp_contains_va_arg_pack(arg)
            }
            Exp::AtomicExchange { ptr, value } => {
                Self::exp_contains_va_arg_pack(ptr) || Self::exp_contains_va_arg_pack(value)
            }
            Exp::AtomicCompareExchange {
                ptr,
                expected,
                desired,
            }
            | Exp::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                ..
            } => {
                Self::exp_contains_va_arg_pack(ptr)
                    || Self::exp_contains_va_arg_pack(expected)
                    || Self::exp_contains_va_arg_pack(desired)
            }
            Exp::Constant(_)
            | Exp::LongConstant(_)
            | Exp::Int128Constant(_)
            | Exp::UIntConstant(_)
            | Exp::ULongConstant(_)
            | Exp::UInt128Constant(_)
            | Exp::DoubleConstant(_)
            | Exp::LongDoubleConstant(_)
            | Exp::ImaginaryIntConstant(_)
            | Exp::ImaginaryDoubleConstant(_)
            | Exp::StringLiteral(_)
            | Exp::WideStringLiteral(_)
            | Exp::Var(_)
            | Exp::LabelAddress(_)
            | Exp::SizeOfType(_, _)
            | Exp::AlignOfType(_)
            | Exp::Unreachable
            | Exp::AtomicFence => false,
        }
    }

    fn statement_contains_va_arg_pack(statement: &Statement) -> bool {
        match statement {
            Statement::Return(Some(exp)) | Statement::Expression(exp) => {
                Self::exp_contains_va_arg_pack(exp)
            }
            Statement::Return(None)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::Null => false,
            Statement::If(cond, then_stmt, else_stmt) => {
                Self::exp_contains_va_arg_pack(cond)
                    || Self::statement_contains_va_arg_pack(then_stmt)
                    || else_stmt
                        .as_ref()
                        .is_some_and(|stmt| Self::statement_contains_va_arg_pack(stmt))
            }
            Statement::Block(block) => Self::block_contains_va_arg_pack(block),
            Statement::While {
                condition, body, ..
            } => {
                Self::exp_contains_va_arg_pack(condition)
                    || Self::statement_contains_va_arg_pack(body)
            }
            Statement::DoWhile {
                body, condition, ..
            } => {
                Self::statement_contains_va_arg_pack(body)
                    || Self::exp_contains_va_arg_pack(condition)
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                ..
            } => {
                matches!(init.as_ref(), ForInit::Expression(Some(exp)) if Self::exp_contains_va_arg_pack(exp))
                    || condition
                        .as_ref()
                        .is_some_and(Self::exp_contains_va_arg_pack)
                    || post.as_ref().is_some_and(Self::exp_contains_va_arg_pack)
                    || Self::statement_contains_va_arg_pack(body)
            }
            Statement::IndirectGoto(exp) => Self::exp_contains_va_arg_pack(exp),
            Statement::Label(_, body)
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::statement_contains_va_arg_pack(body),
            Statement::Switch { control, body, .. } => {
                Self::exp_contains_va_arg_pack(control)
                    || Self::statement_contains_va_arg_pack(body)
            }
        }
    }

    fn block_contains_va_arg_pack(block: &Block) -> bool {
        block.iter().any(|item| match item {
            BlockItem::Declaration(_) => false,
            BlockItem::Statement(stmt) => Self::statement_contains_va_arg_pack(stmt),
        })
    }

    fn inline_return_expression(statement: &Statement) -> Option<Exp> {
        match statement {
            Statement::Return(Some(exp)) => Some(exp.clone()),
            Statement::Block(block) => Self::inline_block_expression(block),
            Statement::If(cond, then_stmt, Some(else_stmt)) => {
                let then_exp = Self::inline_return_expression(then_stmt)?;
                let else_exp = Self::inline_return_expression(else_stmt)?;
                Some(Exp::Conditional(
                    Box::new(cond.clone()),
                    Box::new(then_exp),
                    Box::new(else_exp),
                ))
            }
            _ => None,
        }
    }

    fn substitute_inline_locals_exp(exp: Exp, locals: &HashMap<String, Exp>) -> Exp {
        match exp {
            Exp::Var(name) => locals.get(&name).cloned().unwrap_or(Exp::Var(name)),
            Exp::FunctionCall(name, args) => Exp::FunctionCall(
                name,
                args.into_iter()
                    .map(|arg| Self::substitute_inline_locals_exp(arg, locals))
                    .collect(),
            ),
            Exp::Cast(ct, ft, inner) => Exp::Cast(
                ct,
                ft,
                Box::new(Self::substitute_inline_locals_exp(*inner, locals)),
            ),
            Exp::Unary(op, inner) => Exp::Unary(
                op,
                Box::new(Self::substitute_inline_locals_exp(*inner, locals)),
            ),
            Exp::Binary(op, left, right) => Exp::Binary(
                op,
                Box::new(Self::substitute_inline_locals_exp(*left, locals)),
                Box::new(Self::substitute_inline_locals_exp(*right, locals)),
            ),
            Exp::Conditional(cond, then_exp, else_exp) => Exp::Conditional(
                Box::new(Self::substitute_inline_locals_exp(*cond, locals)),
                Box::new(Self::substitute_inline_locals_exp(*then_exp, locals)),
                Box::new(Self::substitute_inline_locals_exp(*else_exp, locals)),
            ),
            Exp::Comma(left, right) => Exp::Comma(
                Box::new(Self::substitute_inline_locals_exp(*left, locals)),
                Box::new(Self::substitute_inline_locals_exp(*right, locals)),
            ),
            other => other,
        }
    }

    fn inline_block_expression(block: &Block) -> Option<Exp> {
        match block.as_slice() {
            [BlockItem::Statement(statement)] => Self::inline_return_expression(statement),
            [BlockItem::Statement(Statement::If(cond, then_stmt, None)), BlockItem::Statement(fallback)] =>
            {
                let then_exp = Self::inline_return_expression(then_stmt)?;
                let else_exp = Self::inline_return_expression(fallback)?;
                Some(Exp::Conditional(
                    Box::new(cond.clone()),
                    Box::new(then_exp),
                    Box::new(else_exp),
                ))
            }
            _ => {
                let (last, prefix) = block.split_last()?;
                let BlockItem::Statement(statement) = last else {
                    return None;
                };
                let mut locals = HashMap::new();
                let mut side_effects = Vec::new();
                for item in prefix {
                    match item {
                        BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                            let init = vd.init.as_ref()?.clone();
                            locals.insert(vd.name.clone(), init);
                        }
                        BlockItem::Statement(Statement::Expression(exp)) => {
                            side_effects.push(exp.clone());
                        }
                        _ => return None,
                    }
                }
                let mut result = Self::inline_return_expression(statement)?;
                result = Self::substitute_inline_locals_exp(result, &locals);
                for effect in side_effects.into_iter().rev() {
                    result = Exp::Comma(Box::new(effect), Box::new(result));
                }
                Some(result)
            }
        }
    }

    fn substitute_va_arg_pack_exp(
        exp: &Exp,
        params: &HashMap<String, Exp>,
        tail_args: &[Exp],
    ) -> Option<Exp> {
        match exp {
            Exp::Var(name) => Some(params.get(name).cloned().unwrap_or_else(|| exp.clone())),
            Exp::FunctionCall(name, args) => {
                if name == "__builtin_va_arg_pack" && args.is_empty() {
                    return None;
                }
                let mut substituted = Vec::new();
                for arg in args {
                    if matches!(arg, Exp::FunctionCall(inner, inner_args)
                        if inner == "__builtin_va_arg_pack" && inner_args.is_empty())
                    {
                        substituted.extend(tail_args.iter().cloned());
                    } else {
                        substituted.push(Self::substitute_va_arg_pack_exp(arg, params, tail_args)?);
                    }
                }
                Some(Exp::FunctionCall(name.clone(), substituted))
            }
            Exp::Cast(ct, ft, inner) => Some(Exp::Cast(
                *ct,
                ft.clone(),
                Box::new(Self::substitute_va_arg_pack_exp(inner, params, tail_args)?),
            )),
            Exp::Unary(op, inner) => Some(Exp::Unary(
                op.clone(),
                Box::new(Self::substitute_va_arg_pack_exp(inner, params, tail_args)?),
            )),
            Exp::Binary(op, left, right) => Some(Exp::Binary(
                op.clone(),
                Box::new(Self::substitute_va_arg_pack_exp(left, params, tail_args)?),
                Box::new(Self::substitute_va_arg_pack_exp(right, params, tail_args)?),
            )),
            Exp::Assign(left, right) => Some(Exp::Assign(
                Box::new(Self::substitute_va_arg_pack_exp(left, params, tail_args)?),
                Box::new(Self::substitute_va_arg_pack_exp(right, params, tail_args)?),
            )),
            Exp::CompoundAssign(op, left, right) => Some(Exp::CompoundAssign(
                op.clone(),
                Box::new(Self::substitute_va_arg_pack_exp(left, params, tail_args)?),
                Box::new(Self::substitute_va_arg_pack_exp(right, params, tail_args)?),
            )),
            Exp::Conditional(cond, then_exp, else_exp) => Some(Exp::Conditional(
                Box::new(Self::substitute_va_arg_pack_exp(cond, params, tail_args)?),
                Box::new(Self::substitute_va_arg_pack_exp(
                    then_exp, params, tail_args,
                )?),
                Box::new(Self::substitute_va_arg_pack_exp(
                    else_exp, params, tail_args,
                )?),
            )),
            Exp::Comma(left, right) => Some(Exp::Comma(
                Box::new(Self::substitute_va_arg_pack_exp(left, params, tail_args)?),
                Box::new(Self::substitute_va_arg_pack_exp(right, params, tail_args)?),
            )),
            _ => Some(exp.clone()),
        }
    }

    fn expand_inline_va_arg_pack_call(&self, name: &str, args: &[Exp]) -> Option<Exp> {
        let func = self.inline_va_arg_pack_functions.get(name)?;
        if !func.variadic || args.len() < func.params.len() {
            return None;
        }
        let body = func.body.as_ref()?;
        let return_exp = Self::inline_block_expression(body)?;
        let mut params = HashMap::new();
        for ((param_name, _, _), arg) in func.params.iter().zip(args.iter()) {
            params.insert(param_name.clone(), arg.clone());
        }
        let tail_args = &args[func.params.len()..];
        Self::substitute_va_arg_pack_exp(&return_exp, &params, tail_args)
    }

    fn emit_function_call(
        &mut self,
        name: String,
        args: Vec<Exp>,
    ) -> TackyResult<(TackyVal, CType)> {
        if let Some(inlined) = self.expand_inline_va_arg_pack_call(&name, &args) {
            return self.emit_exp(inlined);
        }
        self.emit_nested_capture_updates(&name);
        if name == "__builtin_apply_args" && args.is_empty() {
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            self.emit(TackyInstr::FrameAddress { dst: dst.clone() });
            return Ok((dst, CType::Pointer));
        }
        if name == "__builtin_apply" && args.len() == 3 {
            let Some(target) = Self::builtin_apply_target_name(&args[0]) else {
                return Err("__builtin_apply requires a direct function target".to_string());
            };
            if !matches!(&args[1], Exp::FunctionCall(inner, inner_args)
                if inner == "__builtin_apply_args" && inner_args.is_empty())
            {
                let dst = self.fresh_tmp_full(&Self::void_pointer_type());
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(0),
                    dst: dst.clone(),
                });
                return Ok((dst, CType::Pointer));
            }
            let forwarded_args = self
                .current_function_params
                .iter()
                .cloned()
                .map(Exp::Var)
                .collect();
            let _ = self.emit_function_call(target, forwarded_args)?;
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }
        if name == "__builtin_return_address" && args.len() == 1 {
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }
        if name == "__builtin_extract_return_addr" && args.len() == 1 {
            let Some(arg_exp) = args.into_iter().next() else {
                return Err("__builtin_extract_return_addr requires an argument".to_string());
            };
            let (arg, arg_type) = self.emit_exp(arg_exp)?;
            let converted = self.convert_to(arg, arg_type, CType::Pointer);
            return Ok((converted, CType::Pointer));
        }
        if name == "__builtin_frame_address" && args.len() == 1 {
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            if matches!(args.first(), Some(Exp::Constant(0))) {
                self.emit(TackyInstr::FrameAddress { dst: dst.clone() });
            } else {
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(0),
                    dst: dst.clone(),
                });
            }
            return Ok((dst, CType::Pointer));
        }
        if args.len() == 1 {
            if let Some((kind, arg_type, width)) = Self::bit_builtin_signature(&name) {
                let Some(arg_exp) = args.into_iter().next() else {
                    return Err(format!("{} requires an argument", name));
                };
                return self.emit_bit_builtin(kind, arg_type, width, arg_exp);
            }
        }
        if matches!(
            name.as_str(),
            "__builtin_conjf" | "__builtin_conj" | "__builtin_conjl"
        ) && args.len() == 1
        {
            let Some(arg_exp) = args.into_iter().next() else {
                return Err(format!("{} requires an argument", name));
            };
            if self.typeof_exp(&arg_exp).is_complex() {
                return self.emit_unary(UnaryOp::Complement, arg_exp);
            }
            return self.emit_exp(arg_exp);
        }
        if matches!(
            name.as_str(),
            "__builtin_inf"
                | "__builtin_inff"
                | "__builtin_infl"
                | "__builtin_huge_val"
                | "__builtin_huge_valf"
                | "__builtin_huge_vall"
        ) && args.is_empty()
        {
            let ret_type = match name.as_str() {
                "__builtin_inff" | "__builtin_huge_valf" => CType::Float,
                "__builtin_infl" | "__builtin_huge_vall" => CType::LongDouble,
                _ => CType::Double,
            };
            let raw_inf = self.fresh_tmp(CType::Double);
            self.emit(TackyInstr::Copy {
                src: TackyVal::DoubleConstant(f64::INFINITY),
                dst: raw_inf.clone(),
            });
            let dst = self.convert_to(raw_inf, CType::Double, ret_type);
            return Ok((dst, ret_type));
        }
        if matches!(
            name.as_str(),
            "__builtin_isinf" | "__builtin_isinff" | "__builtin_isinfl"
        ) && args.len() == 1
        {
            let arg_type = match name.as_str() {
                "__builtin_isinff" => CType::Float,
                "__builtin_isinfl" => CType::LongDouble,
                _ => CType::Double,
            };
            let Some(arg_exp) = args.into_iter().next() else {
                return Err(format!("{} requires an argument", name));
            };
            let (arg, from_type) = self.emit_exp(arg_exp)?;
            let value = self.convert_to(arg, from_type, arg_type);
            let (high_op, low_op, limit) =
                (TackyBinaryOp::Equal, TackyBinaryOp::Equal, f64::INFINITY);
            let raw_high_limit = self.fresh_tmp(CType::Double);
            self.emit(TackyInstr::Copy {
                src: TackyVal::DoubleConstant(limit),
                dst: raw_high_limit.clone(),
            });
            let high_limit = self.convert_to(raw_high_limit, CType::Double, arg_type);
            let high = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: high_op,
                left: value.clone(),
                right: high_limit,
                dst: high.clone(),
            });
            let raw_low_limit = self.fresh_tmp(CType::Double);
            self.emit(TackyInstr::Copy {
                src: TackyVal::DoubleConstant(-limit),
                dst: raw_low_limit.clone(),
            });
            let low_limit = self.convert_to(raw_low_limit, CType::Double, arg_type);
            let low = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: low_op,
                left: value,
                right: low_limit,
                dst: low.clone(),
            });
            let result = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseOr,
                left: high,
                right: low,
                dst: result.clone(),
            });
            return Ok((result, CType::Int));
        }
        if name == "__builtin_shuffle" && args.len() == 2 {
            let src_exp = args[0].clone();
            let mask_exp = args[1].clone();
            let src_ft = self.typeof_exp(&src_exp);
            let FullType::Vector { elem, lanes, .. } = src_ft.clone() else {
                return Err("__builtin_shuffle requires a vector source".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&src_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("__builtin_shuffle result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, src_ft.byte_size_with(&self.struct_defs));
            for lane in 0..lanes {
                let (mask_val, mask_type) =
                    self.emit_subscript(mask_exp.clone(), Exp::Constant(lane as i64))?;
                let mut index = self.convert_to(mask_val, mask_type, CType::Long);
                if lanes.is_power_of_two() {
                    let masked = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::BitwiseAnd,
                        left: index,
                        right: TackyVal::Constant((lanes - 1) as i64),
                        dst: masked.clone(),
                    });
                    index = masked;
                }
                let index_name = match index {
                    TackyVal::Var(name) => name,
                    other => {
                        let tmp = self.fresh_tmp(CType::Long);
                        self.emit(TackyInstr::Copy {
                            src: other,
                            dst: tmp.clone(),
                        });
                        match tmp {
                            TackyVal::Var(name) => name,
                            _ => {
                                return Err(
                                    "internal error: expected temporary variable".to_string()
                                )
                            }
                        }
                    }
                };
                let (src_ptr, _, _) =
                    self.emit_subscript_addr(src_exp.clone(), Exp::Var(index_name))?;
                let lane_val = self.fresh_tmp(elem_type);
                self.emit(TackyInstr::Load {
                    src_ptr,
                    dst: lane_val.clone(),
                });
                self.emit(TackyInstr::CopyToOffset {
                    src: lane_val,
                    dst_name: result_name.clone(),
                    offset: (lane * elem_size) as i64,
                });
            }
            return Ok((result, src_ft.to_ctype()));
        }
        if name == "__builtin_setjmp" && args.len() == 1 {
            let (buf, buf_ty) = self.emit_exp(args[0].clone())?;
            let buf = self.convert_to(buf, buf_ty, CType::Pointer);
            let dst = self.fresh_tmp(CType::Int);
            let label = self.fresh_label("builtin_setjmp_resume");
            let end_label = self.fresh_label("builtin_setjmp_end");
            self.emit(TackyInstr::BuiltinSetjmp {
                buf,
                dst: dst.clone(),
                label,
                end_label,
            });
            return Ok((dst, CType::Int));
        }
        if name == "__builtin_longjmp" && args.len() == 2 {
            let (buf, buf_ty) = self.emit_exp(args[0].clone())?;
            let buf = self.convert_to(buf, buf_ty, CType::Pointer);
            let (value, value_ty) = self.emit_exp(args[1].clone())?;
            let value = self.convert_to(value, value_ty, CType::Int);
            self.emit(TackyInstr::BuiltinLongjmp { buf, value });
            return Ok((TackyVal::Constant(0), CType::Void));
        }
        if name == "__builtin_va_start" && !args.is_empty() {
            match &args[0] {
                Exp::Var(ap_name) => {
                    self.emit(TackyInstr::VaStart {
                        dst: TackyVal::Var(ap_name.clone()),
                    });
                }
                _ => {
                    let Some((ap_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())? else {
                        return Err("__builtin_va_start requires a va_list object".to_string());
                    };
                    let current = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::VaStart {
                        dst: current.clone(),
                    });
                    self.emit(TackyInstr::Store {
                        src: current,
                        dst_ptr: ap_addr,
                    });
                }
            }
            return Ok((TackyVal::Constant(0), CType::Int));
        }
        if matches!(name.as_str(), "__builtin_va_copy" | "__va_copy") && args.len() == 2 {
            let (src, _) = self.emit_exp(args[1].clone())?;
            match &args[0] {
                Exp::Var(dst_name) => {
                    self.emit(TackyInstr::Copy {
                        src,
                        dst: TackyVal::Var(dst_name.clone()),
                    });
                }
                _ => {
                    let Some((dst_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())?
                    else {
                        return Err("__builtin_va_copy requires a va_list destination".to_string());
                    };
                    self.emit(TackyInstr::Store {
                        src,
                        dst_ptr: dst_addr,
                    });
                }
            }
            return Ok((TackyVal::Constant(0), CType::Int));
        }
        if name == "__builtin_bswap16" && args.len() == 1 {
            let (value, value_ty) = self.emit_exp(args[0].clone())?;
            let value = self.convert_to(value, value_ty, CType::UInt);
            let low = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: value.clone(),
                right: TackyVal::Constant(0xff),
                dst: low.clone(),
            });
            let low_shifted = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: low,
                right: TackyVal::Constant(8),
                dst: low_shifted.clone(),
            });
            let high = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftRight,
                left: value,
                right: TackyVal::Constant(8),
                dst: high.clone(),
            });
            let high_masked = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: high,
                right: TackyVal::Constant(0xff),
                dst: high_masked.clone(),
            });
            let result = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseOr,
                left: low_shifted,
                right: high_masked,
                dst: result.clone(),
            });
            return Ok((result, CType::UInt));
        }
        if let Some(tag) = name.strip_prefix("__rnqcc_va_arg_struct_") {
            if args.len() != 1 {
                return Err("__builtin_va_arg requires one va_list argument".to_string());
            }
            let tag = self.resolve_struct_tag_name(tag);
            let ft = FullType::Struct(tag.to_string());
            let struct_size = ft.byte_size_with(&self.struct_defs);
            let (slot_ptr, ap_store) = match &args[0] {
                Exp::Var(ap_name) => (
                    self.emit_exp(args[0].clone())?.0,
                    TackyVal::Var(ap_name.clone()),
                ),
                _ => {
                    let Some((ap_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())? else {
                        return Err("__builtin_va_arg requires a va_list object".to_string());
                    };
                    let current = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::Load {
                        src_ptr: ap_addr.clone(),
                        dst: current.clone(),
                    });
                    (current, ap_addr)
                }
            };
            let dst = self.fresh_tmp_full(&ft);
            let slot_ptr =
                self.emit_aligned_pointer(slot_ptr, ft.alignment_with(&self.struct_defs).min(16));
            if let TackyVal::Var(ref dst_name) = dst {
                self.emit_struct_copy_to(slot_ptr.clone(), dst_name, struct_size);
            }
            let next = self.fresh_tmp(CType::Pointer);
            let slot_size = struct_size.next_multiple_of(8);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: slot_ptr,
                right: TackyVal::Constant(slot_size as i64),
                dst: next.clone(),
            });
            match args[0] {
                Exp::Var(_) => self.emit(TackyInstr::Copy {
                    src: next,
                    dst: ap_store,
                }),
                _ => self.emit(TackyInstr::Store {
                    src: next,
                    dst_ptr: ap_store,
                }),
            }
            return Ok((dst, CType::Struct));
        }
        if let Some(arg_type) = name
            .strip_prefix("__rnqcc_va_arg_")
            .and_then(Self::va_arg_helper_type)
        {
            if args.len() != 1 {
                return Err("__builtin_va_arg requires one va_list argument".to_string());
            }
            let (slot_ptr, ap_store) = match &args[0] {
                Exp::Var(ap_name) => (
                    self.emit_exp(args[0].clone())?.0,
                    TackyVal::Var(ap_name.clone()),
                ),
                _ => {
                    let Some((ap_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())? else {
                        return Err("__builtin_va_arg requires a va_list object".to_string());
                    };
                    let current = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::Load {
                        src_ptr: ap_addr.clone(),
                        dst: current.clone(),
                    });
                    (current, ap_addr)
                }
            };
            let dst = self.fresh_tmp(arg_type);
            let slot_ptr =
                self.emit_aligned_pointer(slot_ptr, std::cmp::min(arg_type.size() as usize, 16));
            self.emit(TackyInstr::Load {
                src_ptr: slot_ptr.clone(),
                dst: dst.clone(),
            });
            let next = self.fresh_tmp(CType::Pointer);
            let size = arg_type.size();
            let slot_size = std::cmp::max(8, ((size + 7) / 8) * 8);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: slot_ptr,
                right: TackyVal::Constant(slot_size as i64),
                dst: next.clone(),
            });
            match args[0] {
                Exp::Var(_) => self.emit(TackyInstr::Copy {
                    src: next,
                    dst: ap_store,
                }),
                _ => self.emit(TackyInstr::Store {
                    src: next,
                    dst_ptr: ap_store,
                }),
            }
            return Ok((dst, arg_type));
        }
        if name == "__builtin___sprintf_chk" && args.len() >= 4 {
            self.func_types.insert(
                "sprintf".to_string(),
                (CType::Int, vec![CType::Pointer, CType::Pointer], None, true),
            );
            self.func_full_types
                .insert("sprintf".to_string(), FullType::Scalar(CType::Int));
            self.func_param_full_types.insert(
                "sprintf".to_string(),
                vec![Self::char_pointer_type(), Self::char_pointer_type()],
            );
            let mut lowered_args = Vec::with_capacity(args.len() - 2);
            lowered_args.push(args[0].clone());
            lowered_args.push(args[3].clone());
            lowered_args.extend(args.into_iter().skip(4));
            return self.emit_function_call("sprintf".to_string(), lowered_args);
        }
        if name == "mempcpy" && args.len() == 3 {
            self.emit_function_call("memcpy".to_string(), args.clone())?;
            let (dst, dst_ty) = self.emit_exp(args[0].clone())?;
            let (count, count_ty) = self.emit_exp(args[2].clone())?;
            let count = self.convert_to(count, count_ty, CType::Long);
            let dst = self.convert_to(dst, dst_ty, CType::Pointer);
            let result = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst,
                right: count,
                dst: result.clone(),
            });
            return Ok((result, CType::Pointer));
        }
        if name == "__builtin_mul_overflow" && args.len() == 3 {
            let out_ft = self.typeof_exp(&args[2]);
            let target_type = match out_ft {
                FullType::Pointer(inner) => inner.to_ctype(),
                _ => CType::Long,
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let (out_ptr, _) = self.emit_exp(args[2].clone())?;
            if let Some((min, max)) = match target_type {
                CType::Char | CType::SChar => Some((i8::MIN as i128, i8::MAX as i128)),
                CType::Short => Some((i16::MIN as i128, i16::MAX as i128)),
                CType::Int => Some((i32::MIN as i128, i32::MAX as i128)),
                CType::Long => Some((i64::MIN as i128, i64::MAX as i128)),
                CType::UChar => Some((0, u8::MAX as i128)),
                CType::UShort => Some((0, u16::MAX as i128)),
                CType::UInt => Some((0, u32::MAX as i128)),
                CType::ULong => Some((0, u64::MAX as i128)),
                _ => None,
            } {
                let left_wide = self.convert_to(left, left_ty, CType::Int128);
                let right_wide = self.convert_to(right, right_ty, CType::Int128);
                let product_wide = self.fresh_tmp(CType::Int128);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Mul,
                    left: left_wide,
                    right: right_wide,
                    dst: product_wide.clone(),
                });
                let high = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterThan,
                    left: product_wide.clone(),
                    right: TackyVal::Int128Constant(max),
                    dst: high.clone(),
                });
                let low = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::LessThan,
                    left: product_wide.clone(),
                    right: TackyVal::Int128Constant(min),
                    dst: low.clone(),
                });
                let overflow = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseOr,
                    left: high,
                    right: low,
                    dst: overflow.clone(),
                });
                let product = self.convert_to(product_wide, CType::Int128, target_type);
                self.emit(TackyInstr::Store {
                    src: product,
                    dst_ptr: out_ptr,
                });
                return Ok((overflow, CType::Int));
            }
            let left = self.convert_to(left, left_ty, target_type);
            let right = self.convert_to(right, right_ty, target_type);
            let product = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Mul,
                left: left.clone(),
                right: right.clone(),
                dst: product.clone(),
            });
            self.emit(TackyInstr::Store {
                src: product.clone(),
                dst_ptr: out_ptr,
            });

            if matches!(target_type, CType::Int128 | CType::UInt128) {
                return Ok((TackyVal::Constant(0), CType::Int));
            }

            let overflow = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: overflow.clone(),
            });
            let zero = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Equal,
                left: left.clone(),
                right: TackyVal::Constant(0),
                dst: zero.clone(),
            });
            let end_label = self.fresh_label("builtin_mul_overflow_end");
            self.emit(TackyInstr::JumpIfNotZero(zero, end_label.clone()));
            let quotient = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Div,
                left: product,
                right: left,
                dst: quotient.clone(),
            });
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::NotEqual,
                left: quotient,
                right,
                dst: overflow.clone(),
            });
            self.emit(TackyInstr::Label(end_label));
            return Ok((overflow, CType::Int));
        }
        if name == "__builtin_add_overflow" && args.len() == 3 {
            let out_ft = self.typeof_exp(&args[2]);
            let target_type = match out_ft {
                FullType::Pointer(inner) => inner.to_ctype(),
                _ => CType::Long,
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let (out_ptr, _) = self.emit_exp(args[2].clone())?;
            if let Some((min, max)) = match target_type {
                CType::Char | CType::SChar => Some((i8::MIN as i128, i8::MAX as i128)),
                CType::Short => Some((i16::MIN as i128, i16::MAX as i128)),
                CType::Int => Some((i32::MIN as i128, i32::MAX as i128)),
                CType::Long => Some((i64::MIN as i128, i64::MAX as i128)),
                CType::UChar => Some((0, u8::MAX as i128)),
                CType::UShort => Some((0, u16::MAX as i128)),
                CType::UInt => Some((0, u32::MAX as i128)),
                CType::ULong => Some((0, u64::MAX as i128)),
                _ => None,
            } {
                let left_wide = self.convert_to(left, left_ty, CType::Int128);
                let right_wide = self.convert_to(right, right_ty, CType::Int128);
                let sum_wide = self.fresh_tmp(CType::Int128);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: left_wide,
                    right: right_wide,
                    dst: sum_wide.clone(),
                });
                let high = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterThan,
                    left: sum_wide.clone(),
                    right: TackyVal::Int128Constant(max),
                    dst: high.clone(),
                });
                let low = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::LessThan,
                    left: sum_wide.clone(),
                    right: TackyVal::Int128Constant(min),
                    dst: low.clone(),
                });
                let overflow = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseOr,
                    left: high,
                    right: low,
                    dst: overflow.clone(),
                });
                let sum = self.convert_to(sum_wide, CType::Int128, target_type);
                self.emit(TackyInstr::Store {
                    src: sum,
                    dst_ptr: out_ptr,
                });
                return Ok((overflow, CType::Int));
            }
            let left = self.convert_to(left, left_ty, target_type);
            let right = self.convert_to(right, right_ty, target_type);
            let sum = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: left.clone(),
                right,
                dst: sum.clone(),
            });
            let overflow = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: sum.clone(),
                right: left,
                dst: overflow.clone(),
            });
            self.emit(TackyInstr::Store {
                src: sum,
                dst_ptr: out_ptr,
            });
            return Ok((overflow, CType::Int));
        }
        if name == "__builtin_sub_overflow" && args.len() == 3 {
            let out_ft = if let Exp::Unary(UnaryOp::AddrOf, inner) = &args[2] {
                FullType::Pointer(Box::new(self.typeof_exp(inner)))
            } else {
                self.typeof_exp(&args[2])
            };
            let target_type = match out_ft {
                FullType::Pointer(inner) => inner.to_ctype(),
                _ => CType::Long,
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let (out_ptr, _) = self.emit_exp(args[2].clone())?;
            if let Some((min, max)) = match target_type {
                CType::Char | CType::SChar => Some((i8::MIN as i128, i8::MAX as i128)),
                CType::Short => Some((i16::MIN as i128, i16::MAX as i128)),
                CType::Int => Some((i32::MIN as i128, i32::MAX as i128)),
                CType::Long => Some((i64::MIN as i128, i64::MAX as i128)),
                CType::UChar => Some((0, u8::MAX as i128)),
                CType::UShort => Some((0, u16::MAX as i128)),
                CType::UInt => Some((0, u32::MAX as i128)),
                CType::ULong => Some((0, u64::MAX as i128)),
                _ => None,
            } {
                let left_wide = self.convert_to(left, left_ty, CType::Int128);
                let right_wide = self.convert_to(right, right_ty, CType::Int128);
                let diff_wide = self.fresh_tmp(CType::Int128);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Sub,
                    left: left_wide,
                    right: right_wide,
                    dst: diff_wide.clone(),
                });
                let high = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterThan,
                    left: diff_wide.clone(),
                    right: TackyVal::Int128Constant(max),
                    dst: high.clone(),
                });
                let low = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::LessThan,
                    left: diff_wide.clone(),
                    right: TackyVal::Int128Constant(min),
                    dst: low.clone(),
                });
                let overflow = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseOr,
                    left: high,
                    right: low,
                    dst: overflow.clone(),
                });
                let diff = self.convert_to(diff_wide, CType::Int128, target_type);
                self.emit(TackyInstr::Store {
                    src: diff,
                    dst_ptr: out_ptr,
                });
                return Ok((overflow, CType::Int));
            }
            let left = self.convert_to(left, left_ty, target_type);
            let right = self.convert_to(right, right_ty, target_type);
            let diff = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Sub,
                left: left.clone(),
                right: right.clone(),
                dst: diff.clone(),
            });
            let overflow = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: left.clone(),
                right,
                dst: overflow.clone(),
            });
            self.emit(TackyInstr::Store {
                src: diff,
                dst_ptr: out_ptr,
            });
            return Ok((overflow, CType::Int));
        }
        if name == "__builtin_mul_overflow_p" && args.len() == 3 {
            let target_type = self.typeof_exp(&args[2]).to_ctype();
            let Some((min, max)) = Self::integer_range_for_type(target_type) else {
                return Ok((TackyVal::Constant(0), CType::Int));
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let left_wide = self.convert_to(left, left_ty, CType::Int128);
            let right_wide = self.convert_to(right, right_ty, CType::Int128);
            let product_wide = self.fresh_tmp(CType::Int128);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Mul,
                left: left_wide,
                right: right_wide,
                dst: product_wide.clone(),
            });
            let high = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::GreaterThan,
                left: product_wide.clone(),
                right: TackyVal::Int128Constant(max),
                dst: high.clone(),
            });
            let low = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: product_wide,
                right: TackyVal::Int128Constant(min),
                dst: low.clone(),
            });
            let dst = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseOr,
                left: high,
                right: low,
                dst: dst.clone(),
            });
            return Ok((dst, CType::Int));
        }
        if matches!(name.as_str(), "__builtin_ctz" | "__builtin_clz") && args.len() == 1 {
            let (arg, arg_ty) = self.emit_exp(args[0].clone())?;
            let value = self.convert_to(arg, arg_ty, CType::UInt);
            let count = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: count.clone(),
            });
            if name == "__builtin_ctz" {
                let current = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Copy {
                    src: value,
                    dst: current.clone(),
                });
                let loop_label = self.fresh_label("builtin_ctz_loop");
                let end_label = self.fresh_label("builtin_ctz_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let bit = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: current.clone(),
                    right: TackyVal::Constant(1),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(bit, end_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: count.clone(),
                    right: TackyVal::Constant(1),
                    dst: count.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: current.clone(),
                    right: TackyVal::Constant(1),
                    dst: current,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            } else {
                let mask = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(1_i64 << 31),
                    dst: mask.clone(),
                });
                let loop_label = self.fresh_label("builtin_clz_loop");
                let end_label = self.fresh_label("builtin_clz_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let bit = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: value.clone(),
                    right: mask.clone(),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(bit, end_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: count.clone(),
                    right: TackyVal::Constant(1),
                    dst: count.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: mask.clone(),
                    right: TackyVal::Constant(1),
                    dst: mask,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            }
            return Ok((count, CType::Int));
        }
        let pointer_sig = self
            .full_types
            .get(&name)
            .and_then(Self::function_signature_from_full);
        let user_declares_function = self.func_types.contains_key(&name) || pointer_sig.is_some();
        let builtin_info = if user_declares_function && !name.starts_with("__builtin_") {
            None
        } else {
            Self::builtin_function_info(&name)
        };
        let call_name = builtin_info
            .as_ref()
            .map(|(call_name, _, _, _, _)| (*call_name).to_string())
            .unwrap_or_else(|| name.clone());
        if matches!(name.as_str(), "alloca" | "__builtin_alloca") && args.len() == 1 {
            let size = eval_static_integer_constant_exp_with_context(
                &args[0],
                &self.struct_defs,
                &self.full_types,
            )
            .and_then(|(size, _, _)| usize::try_from(size).ok());
            let Some(size) = size else {
                let _ = self.emit_exp(args[0].clone())?;
                let name = format!("__alloca_dynamic.{}", self.current_function);
                let size = 1024 * 1024;
                if !self.array_sizes.contains_key(&name) {
                    let ft = FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::Char)),
                        size,
                    };
                    self.register_var(&name, ft);
                    self.array_sizes.insert(name.clone(), size);
                }
                let dst = self.fresh_tmp_full(&Self::void_pointer_type());
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(name),
                    dst: dst.clone(),
                });
                return Ok((dst, CType::Pointer));
            };
            if size == 0 {
                let dst = self.fresh_tmp_full(&Self::void_pointer_type());
                self.emit(TackyInstr::FrameAddress { dst: dst.clone() });
                return Ok((dst, CType::Pointer));
            }
            let name = self.fresh_var_name();
            let ft = FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size,
            };
            self.register_var(&name, ft);
            self.array_sizes.insert(name.clone(), size);
            let ptr_ft = Self::void_pointer_type();
            let dst = self.fresh_tmp_full(&ptr_ft);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(name),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }
        let uses_standard_abs_builtin = builtin_info.is_some()
            || self
                .func_types
                .get(&name)
                .is_some_and(|(ret, params, _, _)| {
                    params.len() == 1
                        && match name.as_str() {
                            "abs" | "__builtin_abs" => {
                                *ret == CType::Int && params[0] == CType::Int
                            }
                            "labs" | "__builtin_labs" | "llabs" | "__builtin_llabs" => {
                                *ret == CType::Long && params[0] == CType::Long
                            }
                            _ => false,
                        }
                });
        if matches!(
            name.as_str(),
            "abs" | "__builtin_abs" | "labs" | "__builtin_labs" | "llabs" | "__builtin_llabs"
        ) && uses_standard_abs_builtin
            && args.len() == 1
        {
            let ret_type = match name.as_str() {
                "abs" | "__builtin_abs" => CType::Int,
                _ => CType::Long,
            };
            let Some(arg_exp) = args.into_iter().next() else {
                return Err(format!("{} requires an argument", name));
            };
            let (arg, arg_type) = self.emit_exp(arg_exp)?;
            let arg = self.convert_to(arg, arg_type, ret_type);
            let dst = self.fresh_tmp(ret_type);
            self.emit(TackyInstr::Copy {
                src: arg,
                dst: dst.clone(),
            });
            let is_negative = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: dst.clone(),
                right: TackyVal::Constant(0),
                dst: is_negative.clone(),
            });
            let end_label = self.fresh_label("abs_end");
            self.emit(TackyInstr::JumpIfZero(is_negative, end_label.clone()));
            self.emit(TackyInstr::Unary {
                op: TackyUnaryOp::Negate,
                src: dst.clone(),
                dst: dst.clone(),
            });
            self.emit(TackyInstr::Label(end_label));
            return Ok((dst, ret_type));
        }
        let (ret_type, param_types, ret_pi, variadic) =
            if let Some((_, ret_type, _, param_types, ret_pi)) = builtin_info.as_ref() {
                (
                    *ret_type,
                    param_types.clone(),
                    *ret_pi,
                    matches!(
                        name.as_str(),
                        "__builtin_printf" | "__builtin_sprintf" | "__builtin_snprintf"
                    ),
                )
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
        let direct_old_style_call = self.old_style_functions.contains(&name)
            && builtin_info.is_none()
            && pointer_sig.is_none();
        let param_types = if direct_old_style_call {
            Vec::new()
        } else {
            param_types
        };
        let has_prototype = builtin_info.is_some()
            || pointer_sig.is_some()
            || (self.func_types.contains_key(&name) && !direct_old_style_call);
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
        let param_full_types: Vec<FullType> = if direct_old_style_call {
            Vec::new()
        } else {
            self.func_param_full_types
                .get(&name)
                .cloned()
                .or_else(|| {
                    pointer_sig
                        .as_ref()
                        .map(|(_, param_fts, _)| param_fts.clone())
                })
                .unwrap_or_default()
        };
        let param_full_types = self.canonical_param_full_types(&param_full_types);
        let param_types = if param_full_types.is_empty() {
            param_types
        } else {
            param_full_types
                .iter()
                .map(FullType::to_ctype)
                .collect::<Vec<_>>()
        };

        let mut tacky_args = Vec::new();
        let stack_arg_indices = std::collections::HashSet::new();
        let mut memory_arg_blocks = Vec::new();
        let mut struct_arg_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut fixed_flat_arg_count = 0usize;
        for (i, arg) in args.into_iter().enumerate() {
            let arg_for_type = arg.clone();
            let (val, val_type) = self.emit_exp(arg)?;
            let val_ft = self.val_full_type(&val);
            let expected = param_full_types
                .get(i)
                .map(|ft| self.storage_ctype_for_full(ft))
                .or_else(|| param_types.get(i).copied())
                .unwrap_or_else(|| val_type.promote());
            if let Some(expected_ft) = param_full_types.get(i) {
                let context = format!("function call to {} argument {}", name, i + 1);
                self.assert_assignable_exp_full_type(
                    expected_ft,
                    &val_ft,
                    &arg_for_type,
                    &context,
                )?;
            }

            let source_ft = self.typeof_exp(&arg_for_type);
            let expected_ft_is_complex = param_full_types.get(i).is_some_and(FullType::is_complex);
            let is_complex_arg =
                expected_ft_is_complex || val_ft.is_complex() || source_ft.is_complex();
            if is_complex_arg {
                let complex_ft = param_full_types
                    .get(i)
                    .filter(|ft| ft.is_complex())
                    .cloned()
                    .unwrap_or_else(|| {
                        if val_ft.is_complex() {
                            val_ft.clone()
                        } else {
                            source_ft.clone()
                        }
                    });
                let FullType::Vector { elem, .. } = complex_ft.clone() else {
                    return Err("internal error: expected complex vector type".to_string());
                };
                let elem_type = elem.to_ctype();
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let group_start = tacky_args.len();
                let real = self.emit_complex_component_value(
                    val.clone(),
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    0,
                )?;
                let imag = self.emit_complex_component_value(
                    val,
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    1,
                )?;
                tacky_args.push(real);
                tacky_args.push(imag);
                let lane_is_float = elem_type.is_floating();
                struct_arg_groups.push((group_start, 2, vec![lane_is_float, lane_is_float]));
                if i + 1 == param_types.len() {
                    fixed_flat_arg_count = tacky_args.len();
                }
                continue;
            }
            let is_struct_arg =
                source_ft.is_struct() || val_ft.is_struct() || val_type == CType::Struct;
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
                    let is_variadic_extra = variadic && i >= param_types.len();
                    if is_variadic_extra || (classes.len() == 1 && classes[0] == ParamClass::Memory)
                    {
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
                        let arg_idx = tacky_args.len();
                        tacky_args.push(struct_addr);
                        memory_arg_blocks.push((arg_idx, def.size, def.alignment));
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
                        if !classes.is_empty() {
                            struct_arg_groups.push((group_start, classes.len(), is_sse_vec));
                        }
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
                } else if param_full_types.get(i).is_some_and(FullType::is_vector) {
                    tacky_args.push(val);
                } else {
                    tacky_args.push(self.convert_to(val, val_type, expected));
                }
            } else if param_full_types.get(i).is_some_and(FullType::is_vector) {
                tacky_args.push(val);
            } else {
                tacky_args.push(self.convert_to(val, val_type, expected));
            }

            if i + 1 == param_types.len() {
                fixed_flat_arg_count = tacky_args.len();
            }
        }
        if param_types.is_empty() {
            fixed_flat_arg_count =
                if variadic && !self.zero_fixed_variadic_functions.contains(&name) {
                    tacky_args.len()
                } else {
                    0
                };
        }

        let uses_hidden_ptr = match ret_ft.as_ref() {
            Some(FullType::Struct(tag)) => self
                .struct_defs
                .get(tag)
                .map(|d| d.size > 16)
                .unwrap_or(false),
            Some(ft) => ft.is_complex(),
            None => false,
        };
        let is_indirect = builtin_info.is_none()
            && !self.func_types.contains_key(&name)
            && !self.function_symbols.contains(&name)
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
            } else if rft.is_complex() {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    self.array_sizes
                        .insert(tmp_name.clone(), rft.byte_size_with(&self.struct_defs));
                }
            }
            let ret_addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: tmp.clone(),
                dst: ret_addr.clone(),
            });
            tacky_args.insert(0, ret_addr);
            let shifted_stack = stack_arg_indices.iter().map(|&i| i + 1).collect();
            let shifted_memory_blocks: Vec<(usize, usize, usize)> = memory_arg_blocks
                .iter()
                .map(|(index, size, align)| (index + 1, *size, *align))
                .collect();
            let shifted_groups: Vec<(usize, usize, Vec<bool>)> = struct_arg_groups
                .iter()
                .map(|(start, count, classes)| (start + 1, *count, classes.clone()))
                .collect();
            self.emit(TackyInstr::FunCall {
                name: call_name,
                args: tacky_args,
                dst: tmp.clone(),
                stack_arg_indices: shifted_stack,
                memory_arg_blocks: shifted_memory_blocks,
                struct_arg_groups: shifted_groups,
                variadic,
                fixed_flat_arg_count: fixed_flat_arg_count + 1,
                hidden_return: true,
                indirect: is_indirect,
            });
            if let Some(pi) = ret_pi {
                if let TackyVal::Var(ref dst_name) = tmp {
                    self.ptr_info.insert(dst_name.clone(), pi);
                }
            }
            return Ok((tmp, ret_type));
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
            } else if rft.is_complex() {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    self.array_sizes
                        .insert(tmp_name.clone(), rft.byte_size_with(&self.struct_defs));
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
            memory_arg_blocks,
            struct_arg_groups,
            variadic,
            fixed_flat_arg_count,
            hidden_return: false,
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
        let param_full_types: Vec<FullType> = pointer_sig
            .as_ref()
            .map(|(_, param_fts, _)| param_fts.clone())
            .unwrap_or_default();
        let param_full_types = self.canonical_param_full_types(&param_full_types);
        let param_types = if param_full_types.is_empty() {
            param_types
        } else {
            param_full_types
                .iter()
                .map(FullType::to_ctype)
                .collect::<Vec<_>>()
        };
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
        let stack_arg_indices = std::collections::HashSet::new();
        let mut memory_arg_blocks = Vec::new();
        let mut struct_arg_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut fixed_flat_arg_count = 0usize;
        for (i, arg) in args.into_iter().enumerate() {
            let arg_for_type = arg.clone();
            let (val, val_type) = self.emit_exp(arg)?;
            let val_ft = self.val_full_type(&val);
            let expected = param_full_types
                .get(i)
                .map(|ft| self.storage_ctype_for_full(ft))
                .or_else(|| param_types.get(i).copied())
                .unwrap_or_else(|| val_type.promote());
            if let Some(expected_ft) = param_full_types.get(i) {
                self.assert_assignable_exp_full_type(
                    expected_ft,
                    &val_ft,
                    &arg_for_type,
                    "function pointer call",
                )?;
            }

            let source_ft = self.typeof_exp(&arg_for_type);
            let expected_ft_is_complex = param_full_types.get(i).is_some_and(FullType::is_complex);
            let is_complex_arg =
                expected_ft_is_complex || val_ft.is_complex() || source_ft.is_complex();
            if is_complex_arg {
                let complex_ft = param_full_types
                    .get(i)
                    .filter(|ft| ft.is_complex())
                    .cloned()
                    .unwrap_or_else(|| {
                        if val_ft.is_complex() {
                            val_ft.clone()
                        } else {
                            source_ft.clone()
                        }
                    });
                let FullType::Vector { elem, .. } = complex_ft.clone() else {
                    return Err("internal error: expected complex vector type".to_string());
                };
                let elem_type = elem.to_ctype();
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let group_start = tacky_args.len();
                let real = self.emit_complex_component_value(
                    val.clone(),
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    0,
                )?;
                let imag = self.emit_complex_component_value(
                    val,
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    1,
                )?;
                tacky_args.push(real);
                tacky_args.push(imag);
                let lane_is_float = elem_type.is_floating();
                struct_arg_groups.push((group_start, 2, vec![lane_is_float, lane_is_float]));
                if i + 1 == param_types.len() {
                    fixed_flat_arg_count = tacky_args.len();
                }
                continue;
            }
            let is_struct_arg =
                source_ft.is_struct() || val_ft.is_struct() || val_type == CType::Struct;
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
                    let is_variadic_extra = variadic && i >= param_types.len();
                    if is_variadic_extra || (classes.len() == 1 && classes[0] == ParamClass::Memory)
                    {
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
                        let arg_idx = tacky_args.len();
                        tacky_args.push(struct_addr);
                        memory_arg_blocks.push((arg_idx, def.size, def.alignment));
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
                        if !classes.is_empty() {
                            struct_arg_groups.push((group_start, classes.len(), is_sse_vec));
                        }
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
        let uses_hidden_ptr = match ret_ft {
            FullType::Struct(ref tag) => self
                .struct_defs
                .get(tag)
                .map(|def| def.size > 16)
                .unwrap_or(false),
            _ => ret_ft.is_complex(),
        };
        if uses_hidden_ptr {
            let dst = self.fresh_tmp_full(&ret_ft);
            if let FullType::Struct(ref tag) = ret_ft {
                if let TackyVal::Var(ref dst_name) = dst {
                    if let Some(def) = self.struct_defs.get(tag) {
                        self.array_sizes.insert(dst_name.clone(), def.size);
                    }
                }
            } else if ret_ft.is_complex() {
                if let TackyVal::Var(ref dst_name) = dst {
                    self.array_sizes
                        .insert(dst_name.clone(), ret_ft.byte_size_with(&self.struct_defs));
                }
            }
            let ret_addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: dst.clone(),
                dst: ret_addr.clone(),
            });
            tacky_args.insert(0, ret_addr);
            let shifted_stack = stack_arg_indices.iter().map(|&i| i + 1).collect();
            let shifted_memory_blocks: Vec<(usize, usize, usize)> = memory_arg_blocks
                .iter()
                .map(|(index, size, align)| (index + 1, *size, *align))
                .collect();
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
                memory_arg_blocks: shifted_memory_blocks,
                struct_arg_groups: shifted_groups,
                variadic,
                hidden_return: true,
                indirect: true,
            });
            return Ok((dst, ret_type));
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
        } else if ret_ft.is_complex() {
            if let TackyVal::Var(ref dst_name) = dst {
                self.array_sizes
                    .insert(dst_name.clone(), ret_ft.byte_size_with(&self.struct_defs));
            }
        }
        self.emit(TackyInstr::FunCall {
            name: ptr_name,
            fixed_flat_arg_count,
            args: tacky_args,
            dst: dst.clone(),
            stack_arg_indices,
            memory_arg_blocks,
            struct_arg_groups,
            variadic,
            hidden_return: false,
            indirect: true,
        });
        Ok((dst, ret_type))
    }

    fn emit_addr_of(&mut self, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
            return self.emit_exp(*ptr_exp);
        }

        if let Exp::Unary(op @ (UnaryOp::RealPart | UnaryOp::ImagPart), component_inner) = inner {
            return self.emit_complex_lane_address(op, *component_inner);
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

        if let Exp::Cast(target_type, Some(ft), boxed) = inner.clone() {
            if ft.is_scalar() && matches!(boxed.as_ref(), Exp::ArrayInit(_)) {
                let tmp_name = self.fresh_var_name();
                self.register_var(&tmp_name, ft.clone());
                let (value, value_type) =
                    self.emit_compound_literal_cast(target_type, Some(ft.clone()), *boxed)?;
                let converted = self.convert_to(value, value_type, target_type);
                self.emit(TackyInstr::Copy {
                    src: converted,
                    dst: TackyVal::Var(tmp_name.clone()),
                });
                let dst = self.fresh_tmp_full(&FullType::Pointer(Box::new(ft)));
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(tmp_name),
                    dst: dst.clone(),
                });
                return Ok((dst, CType::Pointer));
            }
        }

        let is_aggregate_compound_literal = matches!(
            &inner,
            Exp::Cast(_, Some(ft), boxed) if (ft.is_struct() || ft.is_vector()) && matches!(boxed.as_ref(), Exp::ArrayInit(_))
        );
        if is_aggregate_compound_literal {
            let Exp::Cast(target_type, Some(ft), boxed) = inner else {
                return Err("internal error: expected aggregate compound literal".to_string());
            };
            let pointee_ft = ft.clone();
            let (var, _) = self.emit_compound_literal_cast(target_type, Some(ft), *boxed)?;
            let dst = self.fresh_tmp_full(&FullType::Pointer(Box::new(pointee_ft)));
            self.emit(TackyInstr::GetAddress {
                src: var,
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }

        if let Exp::Var(ref name) = inner {
            if self.function_symbols.contains(name) {
                self.emit_nested_capture_updates(name);
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
            if inner_ft.is_vector() {
                let result = self.fresh_tmp_full(inner_ft);
                if let TackyVal::Var(ref dst_name) = result {
                    self.emit_struct_copy_to(
                        ptr,
                        dst_name,
                        inner_ft.byte_size_with(&self.struct_defs),
                    );
                }
                return Ok((result, inner_ft.to_ctype()));
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
        if let FullType::Scalar(elem_type) = self.typeof_exp(&arr) {
            if let Some((index, _, _)) = eval_static_integer_constant_exp_with_context(
                &idx,
                &self.struct_defs,
                &self.full_types,
            ) {
                let (arr_val, arr_type) = self.emit_exp(arr)?;
                if index == 0 {
                    return Ok((self.convert_to(arr_val, arr_type, elem_type), elem_type));
                }
                let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                return Ok((zero, elem_type));
            }
        }

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

        if elem_full.is_vector() {
            let result = self.fresh_tmp_full(&elem_full);
            if let TackyVal::Var(ref name) = result {
                self.emit_struct_copy_to(ptr, name, elem_full.byte_size_with(&self.struct_defs));
            }
            return Ok((result, elem_full.to_ctype()));
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

        let then_ft = self.val_full_type(&then_val);
        if then_type == CType::Pointer && matches!(then_ft, FullType::Pointer(_)) {
            let else_exp_for_type = else_exp.clone();
            let result = self.fresh_tmp_full(&then_ft);
            self.emit(TackyInstr::Copy {
                src: then_val,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Jump(end_label.clone()));
            self.emit(TackyInstr::Label(else_label));
            let (else_val, else_type) = self.emit_exp(else_exp)?;
            if else_type != CType::Pointer {
                if Self::is_null_pointer_constant(&else_exp_for_type) {
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: result.clone(),
                    });
                    self.emit(TackyInstr::Label(end_label));
                    return Ok((result, CType::Pointer));
                }
                return Err(format!(
                    "conditional pointer arm has incompatible type {:?}",
                    else_type
                ));
            }
            self.emit(TackyInstr::Copy {
                src: else_val,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Label(end_label));
            return Ok((result, CType::Pointer));
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

    fn emit_complex_lane_value(
        &mut self,
        op: UnaryOp,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let (src, src_type) = self.emit_exp(inner)?;
        let src_ft = self.val_full_type(&src);
        if src_ft.is_complex() {
            let FullType::Vector { elem, .. } = src_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let value =
                self.emit_complex_component_value(src, src_ft, elem_type, elem_size, lane)?;
            Ok((value, elem_type))
        } else if lane == 0 {
            Ok((src, src_type))
        } else {
            Ok((
                self.convert_to(TackyVal::Constant(0), CType::Int, src_type),
                src_type,
            ))
        }
    }

    fn emit_complex_lane_assignment(
        &mut self,
        op: UnaryOp,
        inner: Exp,
        right: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let lhs_ft = self.typeof_exp(&inner);
        if !lhs_ft.is_complex() {
            if lane == 0 {
                return self.emit_exp(Exp::Assign(Box::new(inner), Box::new(right)));
            }
            return Err("assignment to scalar imaginary component".to_string());
        }
        let FullType::Vector { elem, .. } = lhs_ft.clone() else {
            return Err("internal error: expected complex vector type".to_string());
        };
        let elem_type = elem.to_ctype();
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let right_for_type = right.clone();
        let (rhs, rhs_type) = self.emit_exp(right)?;
        let rhs_ft = self.val_full_type(&rhs);
        self.assert_assignable_exp_full_type(&elem, &rhs_ft, &right_for_type, "assignment")?;
        let rhs_conv = self.convert_to(rhs, rhs_type, elem_type);
        let offset = (lane * elem_size) as i64;

        if let Exp::Var(name) = inner {
            self.emit(TackyInstr::CopyToOffset {
                src: rhs_conv.clone(),
                dst_name: name,
                offset,
            });
            return Ok((rhs_conv, elem_type));
        }

        let Some((mut ptr, _, _)) = self.scalar_lvalue_address(inner)? else {
            return Err("Expression is not a simple lvalue".to_string());
        };
        if offset != 0 {
            let lane_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: ptr,
                right: TackyVal::Constant(offset),
                dst: lane_ptr.clone(),
            });
            ptr = lane_ptr;
        }
        self.emit(TackyInstr::Store {
            src: rhs_conv.clone(),
            dst_ptr: ptr,
        });
        Ok((rhs_conv, elem_type))
    }

    fn emit_complex_lane_compound_assignment(
        &mut self,
        component_op: UnaryOp,
        op: BinaryOp,
        inner: Exp,
        right: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match component_op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let lhs_ft = self.typeof_exp(&inner);
        if !lhs_ft.is_complex() {
            if lane == 0 {
                return self.emit_exp(Exp::CompoundAssign(op, Box::new(inner), Box::new(right)));
            }
            return Err("compound assignment to scalar imaginary component".to_string());
        }
        let FullType::Vector { elem, .. } = lhs_ft.clone() else {
            return Err("internal error: expected complex vector type".to_string());
        };
        let elem_type = elem.to_ctype();
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let offset = (lane * elem_size) as i64;

        let (current, dst_ptr, dst_name) = if let Exp::Var(name) = inner {
            let current = self.fresh_tmp(elem_type);
            self.emit(TackyInstr::CopyFromOffset {
                src_name: name.clone(),
                offset,
                dst: current.clone(),
            });
            (current, None, Some(name))
        } else {
            let Some((mut ptr, _, _)) = self.scalar_lvalue_address(inner)? else {
                return Err("Expression is not a simple lvalue".to_string());
            };
            if offset != 0 {
                let lane_ptr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: ptr,
                    right: TackyVal::Constant(offset),
                    dst: lane_ptr.clone(),
                });
                ptr = lane_ptr;
            }
            let current = self.fresh_tmp(elem_type);
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            (current, Some(ptr), None)
        };

        let (rhs, rhs_type) = self.emit_exp(right)?;
        let common = CType::common(elem_type, rhs_type);
        let lhs_conv = self.convert_to(current, elem_type, common);
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
        if let Some(ptr) = dst_ptr {
            self.emit(TackyInstr::Store {
                src: result_conv.clone(),
                dst_ptr: ptr,
            });
        } else if let Some(name) = dst_name {
            self.emit(TackyInstr::CopyToOffset {
                src: result_conv.clone(),
                dst_name: name,
                offset,
            });
        }
        Ok((result_conv, elem_type))
    }

    fn emit_complex_lane_address(
        &mut self,
        op: UnaryOp,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let inner_ft = self.typeof_exp(&inner);
        if !inner_ft.is_complex() {
            if lane == 0 {
                return self.emit_addr_of(inner);
            }
            return Err("cannot take address of scalar imaginary component".to_string());
        }
        let FullType::Vector { elem, .. } = inner_ft else {
            return Err("internal error: expected complex vector type".to_string());
        };
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let offset = (lane * elem_size) as i64;
        let ptr_ft = FullType::Pointer(elem);
        let mut ptr = match inner {
            Exp::Var(name) => {
                let base = self.fresh_tmp_full(&ptr_ft);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(name),
                    dst: base.clone(),
                });
                base
            }
            other => {
                let Some((base, _, _)) = self.scalar_lvalue_address(other)? else {
                    return Err("Expression is not a simple lvalue".to_string());
                };
                let typed_base = self.fresh_tmp_full(&ptr_ft);
                self.emit(TackyInstr::Copy {
                    src: base,
                    dst: typed_base.clone(),
                });
                typed_base
            }
        };
        if offset != 0 {
            let lane_ptr = self.fresh_tmp_full(&ptr_ft);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: ptr,
                right: TackyVal::Constant(offset),
                dst: lane_ptr.clone(),
            });
            ptr = lane_ptr;
        }
        Ok((ptr, CType::Pointer))
    }

    fn emit_complex_value_parts(
        &mut self,
        target_ft: &FullType,
        value: TackyVal,
        value_type: CType,
        value_ft: FullType,
    ) -> TackyResult<(TackyVal, TackyVal, CType, usize)> {
        let FullType::Vector { elem, .. } = target_ft else {
            return Err("internal error: expected complex target type".to_string());
        };
        let elem_type = elem.to_ctype();
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let real = if value_ft.is_complex() {
            self.emit_complex_component_value(
                value.clone(),
                value_ft.clone(),
                elem_type,
                elem_size,
                0,
            )?
        } else {
            self.convert_to(value.clone(), value_type, elem_type)
        };
        let imag = if value_ft.is_complex() {
            self.emit_complex_component_value(value, value_ft, elem_type, elem_size, 1)?
        } else {
            self.convert_to(TackyVal::Constant(0), CType::Int, elem_type)
        };
        Ok((real, imag, elem_type, elem_size))
    }

    fn emit_complex_value_to_offset(
        &mut self,
        dst_name: &str,
        target_ft: &FullType,
        value: TackyVal,
        value_type: CType,
        value_ft: FullType,
        offset: i64,
    ) -> TackyResult<()> {
        let (real, imag, _elem_type, elem_size) =
            self.emit_complex_value_parts(target_ft, value, value_type, value_ft)?;
        self.emit(TackyInstr::CopyToOffset {
            src: real,
            dst_name: dst_name.to_string(),
            offset,
        });
        self.emit(TackyInstr::CopyToOffset {
            src: imag,
            dst_name: dst_name.to_string(),
            offset: offset + elem_size as i64,
        });
        Ok(())
    }

    fn emit_complex_value_to_ptr(
        &mut self,
        dst_ptr: TackyVal,
        target_ft: &FullType,
        value: TackyVal,
        value_type: CType,
        value_ft: FullType,
    ) -> TackyResult<()> {
        let (real, imag, _elem_type, elem_size) =
            self.emit_complex_value_parts(target_ft, value, value_type, value_ft)?;
        self.emit(TackyInstr::Store {
            src: real,
            dst_ptr: dst_ptr.clone(),
        });
        let imag_ptr = self.fresh_tmp(CType::Pointer);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: dst_ptr,
            right: TackyVal::Constant(elem_size as i64),
            dst: imag_ptr.clone(),
        });
        self.emit(TackyInstr::Store {
            src: imag,
            dst_ptr: imag_ptr,
        });
        Ok(())
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

    fn emit_unary(&mut self, op: UnaryOp, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        let (src, src_type) = self.emit_exp(inner)?;
        let value_ft = self.val_full_type(&src);
        if value_ft.is_complex() {
            let FullType::Vector { elem, .. } = value_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&value_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("complex unary result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, value_ft.byte_size_with(&self.struct_defs));
            match op {
                UnaryOp::Negate => {
                    let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                    let real = self.emit_complex_component_value(
                        src.clone(),
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        0,
                    )?;
                    let imag = self.emit_complex_component_value(
                        src,
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        1,
                    )?;
                    let neg_real = self.fresh_tmp(elem_type);
                    let neg_imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: zero.clone(),
                        right: real,
                        dst: neg_real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: zero,
                        right: imag,
                        dst: neg_imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: neg_real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: neg_imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, value_ft.to_ctype()));
                }
                UnaryOp::Complement => {
                    let real = self.emit_complex_component_value(
                        src.clone(),
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        0,
                    )?;
                    let imag = self.emit_complex_component_value(
                        src,
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        1,
                    )?;
                    let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                    let neg_imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: zero,
                        right: imag,
                        dst: neg_imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: neg_imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, value_ft.to_ctype()));
                }
                _ => return Err("invalid complex unary operator".to_string()),
            }
        }
        if !value_ft.is_vector() {
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
            return Ok((dst, promoted));
        }

        let FullType::Vector { elem, lanes, .. } = value_ft.clone() else {
            return Err("internal error: expected vector type".to_string());
        };
        let elem_type = elem.to_ctype();
        let calc_type = match elem_type {
            CType::UChar | CType::UShort => CType::UInt,
            CType::Char | CType::SChar | CType::Short | CType::Bool => CType::Int,
            _ => elem_type,
        };
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let result = self.fresh_tmp_full(&value_ft);
        let TackyVal::Var(result_name) = result.clone() else {
            return Err("vector unary result must be addressable".to_string());
        };
        self.zero_init_local(&result_name, value_ft.byte_size_with(&self.struct_defs));
        let tacky_op = match op {
            UnaryOp::Negate => TackyUnaryOp::Negate,
            UnaryOp::Complement => TackyUnaryOp::Complement,
            _ => return Err(format!("invalid vector unary operator: {:?}", op)),
        };
        for lane in 0..lanes {
            let lane_value = self.emit_vector_lane_value(
                src.clone(),
                value_ft.clone(),
                elem_type,
                elem_size,
                lane,
            )?;
            let lane_value = self.convert_to(lane_value, elem_type, calc_type);
            let dst = self.fresh_tmp(calc_type);
            self.emit(TackyInstr::Unary {
                op: tacky_op.clone(),
                src: lane_value,
                dst: dst.clone(),
            });
            let stored = self.convert_to(dst, calc_type, elem_type);
            self.emit(TackyInstr::CopyToOffset {
                src: stored,
                dst_name: result_name.clone(),
                offset: (lane * elem_size) as i64,
            });
        }
        Ok((result, value_ft.to_ctype()))
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

        if matches!(inner, Exp::Dot(_, _) | Exp::Arrow(_, _)) {
            let mem = match &inner {
                Exp::Dot(base, member) | Exp::Arrow(base, member) => {
                    let tag = self.dot_inner_tag(base)?;
                    self.struct_member(&tag, member)?
                }
                _ => return Err("internal error: expected dot or arrow expression".to_string()),
            };
            if let Some(width) = mem.bit_width {
                let ptr = self.emit_dot_address(&inner)?;
                let unit = self.fresh_tmp(mem.member_type);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: unit.clone(),
                });
                let current = self.extract_bit_field(unit, &mem)?;
                let promoted_type = Self::bit_field_promoted_type(&mem, width);
                let increment = self.convert_to(TackyVal::Constant(1), CType::Int, promoted_type);
                let result = self.fresh_tmp(promoted_type);
                self.emit(TackyInstr::Binary {
                    op: binop,
                    left: current.clone(),
                    right: increment,
                    dst: result.clone(),
                });
                let result_for_store =
                    self.convert_to(result.clone(), promoted_type, mem.member_type);
                let stored = self.store_bit_field_to_ptr(ptr, &mem, result_for_store)?;
                let stored = self.convert_to(stored, mem.member_type, promoted_type);
                if !mem.member_type.is_signed() {
                    self.mark_bit_precision(&stored, width);
                }
                return Ok((if is_pre { stored } else { current }, promoted_type));
            }
        }

        if let Exp::Subscript(arr, idx) = inner {
            let (ptr, pt, pt_ft) = self.emit_subscript_addr(*arr, *idx)?;
            let current = if pt == CType::Pointer {
                self.fresh_tmp_full(&pt_ft)
            } else {
                self.fresh_tmp(pt)
            };
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let increment = if pt == CType::Pointer {
                let elem_size = match &pt_ft {
                    FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                    _ => 1,
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, pt)
            };
            let result = if pt == CType::Pointer {
                self.fresh_tmp_full(&pt_ft)
            } else {
                self.fresh_tmp(pt)
            };
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
            return Ok((if is_pre { result } else { current }, pt));
        }

        if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
            let (ptr, _) = self.emit_exp(*ptr_exp)?;
            let elem_ft = match self.val_full_type(&ptr) {
                FullType::Pointer(inner) => *inner,
                _ => FullType::Scalar(CType::Int),
            };
            let pt = elem_ft.to_ctype();
            let current = if pt == CType::Pointer {
                self.fresh_tmp_full(&elem_ft)
            } else {
                self.fresh_tmp(pt)
            };
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let increment = if pt == CType::Pointer {
                let elem_size = match &elem_ft {
                    FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                    _ => 1,
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, pt)
            };
            let result = if pt == CType::Pointer {
                self.fresh_tmp_full(&elem_ft)
            } else {
                self.fresh_tmp(pt)
            };
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

    fn bit_field_promoted_type(mem: &StructMember, width: u8) -> CType {
        if width < 32 {
            CType::Int
        } else if width == 32 {
            if mem.member_type.is_signed() {
                CType::Int
            } else {
                CType::UInt
            }
        } else if mem.member_type.is_signed() {
            CType::Long
        } else {
            CType::ULong
        }
    }

    fn mark_bit_precision(&mut self, value: &TackyVal, width: u8) {
        if let TackyVal::Var(name) = value {
            if width > 32 && width < 64 {
                self.bit_precisions.insert(name.clone(), width);
            }
        }
    }

    fn bit_precision(&self, value: &TackyVal) -> Option<u8> {
        let TackyVal::Var(name) = value else {
            return None;
        };
        self.bit_precisions.get(name).copied()
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
        let mut value = if mem.reverse_storage_order {
            self.byteswap_storage_value(unit, mem.member_type)
        } else {
            unit
        };
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
            let promoted_type = Self::bit_field_promoted_type(mem, width);
            let value = self.convert_to(value, mem.member_type, promoted_type);
            if !mem.member_type.is_signed() {
                self.mark_bit_precision(&value, width);
            }
            return Ok(value);
        }
        let masked = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: value,
            right: TackyVal::Constant(Self::bit_mask(width)),
            dst: masked.clone(),
        });
        let value = self.sign_extend_bit_field_value(masked, mem, width);
        let promoted_type = Self::bit_field_promoted_type(mem, width);
        let value = self.convert_to(value, mem.member_type, promoted_type);
        if !mem.member_type.is_signed() {
            self.mark_bit_precision(&value, width);
        }
        Ok(value)
    }

    fn byteswap_storage_value(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        match ty.size() {
            2 => self.byteswap_2(value, ty),
            4 => self.byteswap_4(value, ty),
            8 => self.byteswap_8(value, ty),
            _ => value,
        }
    }

    fn bitwise_and_const(&mut self, value: TackyVal, ty: CType, mask: i64) -> TackyVal {
        let dst = self.fresh_tmp(ty);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: value,
            right: TackyVal::Constant(mask),
            dst: dst.clone(),
        });
        dst
    }

    fn shift_const(
        &mut self,
        op: TackyBinaryOp,
        value: TackyVal,
        ty: CType,
        bits: i64,
    ) -> TackyVal {
        let dst = self.fresh_tmp(ty);
        self.emit(TackyInstr::Binary {
            op,
            left: value,
            right: TackyVal::Constant(bits),
            dst: dst.clone(),
        });
        dst
    }

    fn bitwise_or(&mut self, left: TackyVal, right: TackyVal, ty: CType) -> TackyVal {
        let dst = self.fresh_tmp(ty);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left,
            right,
            dst: dst.clone(),
        });
        dst
    }

    fn byteswap_2(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        let lo = self.bitwise_and_const(value.clone(), ty, 0x00ff);
        let lo = self.shift_const(TackyBinaryOp::ShiftLeft, lo, ty, 8);
        let hi = self.shift_const(TackyBinaryOp::ShiftRight, value, ty, 8);
        let hi = self.bitwise_and_const(hi, ty, 0x00ff);
        self.bitwise_or(lo, hi, ty)
    }

    fn byteswap_4(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        let b0 = self.bitwise_and_const(value.clone(), ty, 0x000000ff);
        let b0 = self.shift_const(TackyBinaryOp::ShiftLeft, b0, ty, 24);
        let b1 = self.bitwise_and_const(value.clone(), ty, 0x0000ff00);
        let b1 = self.shift_const(TackyBinaryOp::ShiftLeft, b1, ty, 8);
        let b2 = self.shift_const(TackyBinaryOp::ShiftRight, value.clone(), ty, 8);
        let b2 = self.bitwise_and_const(b2, ty, 0x0000ff00);
        let b3 = self.shift_const(TackyBinaryOp::ShiftRight, value, ty, 24);
        let b3 = self.bitwise_and_const(b3, ty, 0x000000ff);
        let lo = self.bitwise_or(b0, b1, ty);
        let hi = self.bitwise_or(b2, b3, ty);
        self.bitwise_or(lo, hi, ty)
    }

    fn byteswap_8(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        let mut out = self.fresh_tmp(ty);
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: out.clone(),
        });
        for byte in 0..8 {
            let shifted = self.shift_const(TackyBinaryOp::ShiftRight, value.clone(), ty, byte * 8);
            let masked = self.bitwise_and_const(shifted, ty, 0xff);
            let moved = self.shift_const(TackyBinaryOp::ShiftLeft, masked, ty, (7 - byte) * 8);
            out = self.bitwise_or(out, moved, ty);
        }
        out
    }

    fn store_bit_field_to_offset(
        &mut self,
        dst_name: String,
        mem: &StructMember,
        rhs: TackyVal,
    ) -> TackyResult<TackyVal> {
        self.store_bit_field_to_absolute_offset(dst_name, mem, mem.offset as i64, rhs)
    }

    fn store_bit_field_to_absolute_offset(
        &mut self,
        dst_name: String,
        mem: &StructMember,
        offset: i64,
        rhs: TackyVal,
    ) -> TackyResult<TackyVal> {
        let Some(width) = mem.bit_width else {
            return Err("store_bit_field_to_offset called for non-bit-field".to_string());
        };
        let unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::CopyFromOffset {
            src_name: dst_name.clone(),
            offset,
            dst: unit.clone(),
        });
        let unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(unit, mem.member_type)
        } else {
            unit
        };
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
        let stored_unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(new_unit, mem.member_type)
        } else {
            new_unit
        };
        self.emit(TackyInstr::CopyToOffset {
            src: stored_unit,
            dst_name,
            offset,
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
        let unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(unit, mem.member_type)
        } else {
            unit
        };
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
        let stored_unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(new_unit, mem.member_type)
        } else {
            new_unit
        };
        self.emit(TackyInstr::Store {
            src: stored_unit,
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
            let result = if mem.bit_width.is_some() {
                self.fresh_tmp(mem.member_type)
            } else {
                self.fresh_tmp_full(&mem_ft)
            };
            self.emit(TackyInstr::Load {
                src_ptr: mem_ptr,
                dst: result.clone(),
            });
            let result = self.extract_bit_field(result, &mem)?;
            let result_type = mem
                .bit_width
                .map(|width| Self::bit_field_promoted_type(&mem, width))
                .unwrap_or(mem_type);
            Ok((result, result_type))
        }
    }

    /// Get the address of a struct value, handling deref temps correctly
    fn get_struct_addr(&mut self, val: TackyVal) -> TackyVal {
        if let TackyVal::Var(ref n) = val {
            if self.array_sizes.contains_key(n) || self.get_full_type(n).is_vector() {
                // Proper aggregate/vector variable — take its address
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
        if let FullType::Vector { elem, .. } = first_full.clone() {
            let elem_full = elem.as_ref().clone();
            let elem_type = elem_full.to_ctype();
            let idx_long = self.convert_to(second_val, second_type, CType::Long);
            let base_ptr = self.fresh_tmp_full(&FullType::Pointer(Box::new(elem_full.clone())));
            self.emit(TackyInstr::GetAddress {
                src: first_val,
                dst: base_ptr.clone(),
            });
            let ptr = self.fresh_tmp_full(&FullType::Pointer(Box::new(elem_full.clone())));
            self.emit(TackyInstr::AddPtr {
                ptr: base_ptr,
                index: idx_long,
                scale: elem_full.byte_size_with(&self.struct_defs) as i64,
                dst: ptr.clone(),
            });
            return Ok((ptr, elem_type, elem_full));
        }
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
        if let TackyVal::Var(ref arr_name) = arr_val {
            if let Some(scale_exp) = self
                .dynamic_sizes
                .get(arr_name)
                .cloned()
                .or_else(|| self.vla_param_bounds.get(arr_name).cloned())
                .filter(|_| elem_full.is_array() || elem_full.is_struct())
            {
                let (scale_val, scale_type) = self.emit_exp(scale_exp)?;
                let scale_long = self.convert_to(scale_val, scale_type, CType::Long);
                let byte_offset = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Mul,
                    left: idx_long,
                    right: scale_long,
                    dst: byte_offset.clone(),
                });
                let result_ptr_type = FullType::Pointer(Box::new(elem_full.clone()));
                let ptr = self.fresh_tmp_full(&result_ptr_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: arr_val.clone(),
                    right: byte_offset,
                    dst: ptr.clone(),
                });
                if let TackyVal::Var(ref pname) = ptr {
                    if let Some(info) = self.deref_info(arr_name) {
                        self.ptr_info.insert(pname.clone(), info);
                    } else {
                        self.ptr_info.insert(pname.clone(), (elem_type, 1));
                    }
                    if let Some(size) = self
                        .dynamic_sizes
                        .get(arr_name)
                        .cloned()
                        .filter(|_| elem_full.is_array() || elem_full.is_struct())
                    {
                        self.dynamic_sizes.insert(pname.clone(), size);
                    }
                }
                return Ok((ptr, elem_type, elem_full));
            }
        }
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

    fn emit_vector_lane_value(
        &mut self,
        value: TackyVal,
        value_ft: FullType,
        elem_type: CType,
        elem_size: usize,
        lane: usize,
    ) -> TackyResult<TackyVal> {
        if value_ft.is_vector() {
            if let TackyVal::Var(name) = value {
                let lane_value = self.fresh_tmp(elem_type);
                self.emit(TackyInstr::CopyFromOffset {
                    src_name: name,
                    offset: (lane * elem_size) as i64,
                    dst: lane_value.clone(),
                });
                return Ok(lane_value);
            }
            let addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: value,
                dst: addr.clone(),
            });
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::AddPtr {
                ptr: addr,
                index: TackyVal::Constant(lane as i64),
                scale: elem_size as i64,
                dst: ptr.clone(),
            });
            let lane_value = self.fresh_tmp(elem_type);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: lane_value.clone(),
            });
            Ok(lane_value)
        } else {
            Ok(self.convert_to(value, value_ft.to_ctype(), elem_type))
        }
    }

    fn emit_complex_component_value(
        &mut self,
        value: TackyVal,
        value_ft: FullType,
        elem_type: CType,
        _elem_size: usize,
        lane: usize,
    ) -> TackyResult<TackyVal> {
        if value_ft.is_complex() {
            let TackyVal::Var(name) = value else {
                return Err("complex values must lower to addressable temporaries".to_string());
            };
            let FullType::Vector { elem, .. } = value_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let source_elem_type = elem.to_ctype();
            let source_elem_size = elem.byte_size_with(&self.struct_defs);
            let dst = self.fresh_tmp(source_elem_type);
            self.emit(TackyInstr::CopyFromOffset {
                src_name: name,
                offset: (lane * source_elem_size) as i64,
                dst: dst.clone(),
            });
            Ok(self.convert_to(dst, source_elem_type, elem_type))
        } else if lane == 0 {
            Ok(self.convert_to(value, value_ft.to_ctype(), elem_type))
        } else {
            Ok(self.convert_to(TackyVal::Constant(0), CType::Int, elem_type))
        }
    }

    fn emit_binary(
        &mut self,
        op: BinaryOp,
        left: Exp,
        right: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let vector_comparison_elem = if is_comparison_op(&op) {
            match (self.typeof_exp(&left), self.typeof_exp(&right)) {
                (FullType::Vector { elem, .. }, _) | (_, FullType::Vector { elem, .. }) => {
                    Some(elem.to_ctype())
                }
                _ => None,
            }
        } else {
            None
        };
        let (l, l_type) = self.emit_exp(left)?;
        let (r, r_type) = self.emit_exp(right)?;
        let bit_precision = self.bit_precision(&l).max(self.bit_precision(&r));
        let l_full = self.val_full_type(&l);
        let r_full = self.val_full_type(&r);

        if l_full.is_complex() || r_full.is_complex() {
            let complex_ft = if l_full.is_complex() {
                l_full.clone()
            } else {
                r_full.clone()
            };
            let FullType::Vector { elem, .. } = complex_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&complex_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("complex result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, complex_ft.byte_size_with(&self.struct_defs));

            let left_real = self.emit_complex_component_value(
                l.clone(),
                l_full.clone(),
                elem_type,
                elem_size,
                0,
            )?;
            let left_imag = self.emit_complex_component_value(
                l.clone(),
                l_full.clone(),
                elem_type,
                elem_size,
                1,
            )?;
            let right_real = self.emit_complex_component_value(
                r.clone(),
                r_full.clone(),
                elem_type,
                elem_size,
                0,
            )?;
            let right_imag = self.emit_complex_component_value(
                r.clone(),
                r_full.clone(),
                elem_type,
                elem_size,
                1,
            )?;

            match op {
                BinaryOp::Add | BinaryOp::Sub => {
                    let tacky_op = Self::convert_binop(op)?;
                    let real = self.fresh_tmp(elem_type);
                    let imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: tacky_op.clone(),
                        left: left_real,
                        right: right_real,
                        dst: real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: tacky_op,
                        left: left_imag,
                        right: right_imag,
                        dst: imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, elem_type));
                }
                BinaryOp::Mul => {
                    let ar_br = self.fresh_tmp(elem_type);
                    let ai_bi = self.fresh_tmp(elem_type);
                    let ar_bi = self.fresh_tmp(elem_type);
                    let ai_br = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real.clone(),
                        right: right_real.clone(),
                        dst: ar_br.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag.clone(),
                        right: right_imag.clone(),
                        dst: ai_bi.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real,
                        right: right_imag,
                        dst: ar_bi.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag,
                        right: right_real,
                        dst: ai_br.clone(),
                    });
                    let real = self.fresh_tmp(elem_type);
                    let imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: ar_br,
                        right: ai_bi,
                        dst: real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ar_bi,
                        right: ai_br,
                        dst: imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, elem_type));
                }
                BinaryOp::Div => {
                    let br2 = self.fresh_tmp(elem_type);
                    let bi2 = self.fresh_tmp(elem_type);
                    let denom = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: right_real.clone(),
                        right: right_real.clone(),
                        dst: br2.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: right_imag.clone(),
                        right: right_imag.clone(),
                        dst: bi2.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: br2,
                        right: bi2,
                        dst: denom.clone(),
                    });
                    let ar_br = self.fresh_tmp(elem_type);
                    let ai_bi = self.fresh_tmp(elem_type);
                    let ai_br = self.fresh_tmp(elem_type);
                    let ar_bi = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real.clone(),
                        right: right_real.clone(),
                        dst: ar_br.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag.clone(),
                        right: right_imag.clone(),
                        dst: ai_bi.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag,
                        right: right_real,
                        dst: ai_br.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real,
                        right: right_imag,
                        dst: ar_bi.clone(),
                    });
                    let real_num = self.fresh_tmp(elem_type);
                    let imag_num = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ar_br,
                        right: ai_bi,
                        dst: real_num.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: ai_br,
                        right: ar_bi,
                        dst: imag_num.clone(),
                    });
                    let real = self.fresh_tmp(elem_type);
                    let imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Div,
                        left: real_num,
                        right: denom.clone(),
                        dst: real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Div,
                        left: imag_num,
                        right: denom,
                        dst: imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, elem_type));
                }
                BinaryOp::Equal | BinaryOp::NotEqual => {
                    let real_cmp = self.fresh_tmp(CType::Int);
                    let imag_cmp = self.fresh_tmp(CType::Int);
                    let is_equal = matches!(op, BinaryOp::Equal);
                    let cmp_op = Self::convert_binop(op.clone())?;
                    self.emit(TackyInstr::Binary {
                        op: cmp_op.clone(),
                        left: left_real,
                        right: right_real,
                        dst: real_cmp.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: cmp_op,
                        left: left_imag,
                        right: right_imag,
                        dst: imag_cmp.clone(),
                    });
                    let combine_op = if is_equal {
                        TackyBinaryOp::BitwiseAnd
                    } else {
                        TackyBinaryOp::BitwiseOr
                    };
                    let dst = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Binary {
                        op: combine_op,
                        left: real_cmp,
                        right: imag_cmp,
                        dst: dst.clone(),
                    });
                    return Ok((dst, CType::Int));
                }
                _ => {
                    return Err("unsupported complex operator".to_string());
                }
            }
        }

        if is_comparison_op(&op) && (l_full.is_vector() || r_full.is_vector()) {
            let vector_ft = if l_full.is_vector() {
                l_full.clone()
            } else {
                r_full.clone()
            };
            let FullType::Vector { elem, lanes, .. } = vector_ft.clone() else {
                return Err("internal error: expected vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&vector_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("vector comparison result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, vector_ft.byte_size_with(&self.struct_defs));
            let tacky_op = Self::convert_binop(op)?;
            for lane in 0..lanes {
                let left_lane = self.emit_vector_lane_value(
                    l.clone(),
                    l_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let right_lane = self.emit_vector_lane_value(
                    r.clone(),
                    r_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let cmp = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: tacky_op.clone(),
                    left: left_lane,
                    right: right_lane,
                    dst: cmp.clone(),
                });
                let bool_as_elem = self.convert_to(cmp, CType::Int, elem_type);
                let lane_mask = self.fresh_tmp(elem_type);
                let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Sub,
                    left: zero,
                    right: bool_as_elem,
                    dst: lane_mask.clone(),
                });
                self.emit(TackyInstr::CopyToOffset {
                    src: lane_mask,
                    dst_name: result_name.clone(),
                    offset: (lane * elem_size) as i64,
                });
            }
            return Ok((result, vector_ft.to_ctype()));
        }

        if !is_comparison_op(&op) && (l_full.is_vector() || r_full.is_vector()) {
            let vector_ft = if l_full.is_vector() {
                l_full.clone()
            } else {
                r_full.clone()
            };
            let FullType::Vector { elem, lanes, .. } = vector_ft.clone() else {
                return Err("internal error: expected vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let calc_type = match elem_type {
                CType::UChar | CType::UShort => CType::UInt,
                CType::Char | CType::SChar | CType::Short | CType::Bool => CType::Int,
                _ => elem_type,
            };
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&vector_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("vector binary result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, vector_ft.byte_size_with(&self.struct_defs));
            let tacky_op = Self::convert_binop(op)?;
            for lane in 0..lanes {
                let left_lane = self.emit_vector_lane_value(
                    l.clone(),
                    l_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let right_lane = self.emit_vector_lane_value(
                    r.clone(),
                    r_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let left_lane = self.convert_to(left_lane, elem_type, calc_type);
                let right_lane = self.convert_to(right_lane, elem_type, calc_type);
                let dst = self.fresh_tmp(calc_type);
                self.emit(TackyInstr::Binary {
                    op: tacky_op.clone(),
                    left: left_lane,
                    right: right_lane,
                    dst: dst.clone(),
                });
                let stored = self.convert_to(dst, calc_type, elem_type);
                self.emit(TackyInstr::CopyToOffset {
                    src: stored,
                    dst_name: result_name.clone(),
                    offset: (lane * elem_size) as i64,
                });
            }
            return Ok((result, vector_ft.to_ctype()));
        }

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
            if let Some(width) = bit_precision {
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: dst.clone(),
                    right: TackyVal::Constant(Self::bit_mask(width)),
                    dst: dst.clone(),
                });
                self.mark_bit_precision(&dst, width);
            }
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
        let is_comparison = is_comparison_op(&op);
        let tacky_op = Self::convert_binop(op)?;
        self.emit(TackyInstr::Binary {
            op: tacky_op,
            left: l_conv,
            right: r_conv,
            dst: dst.clone(),
        });
        if let Some(elem_type) = vector_comparison_elem {
            let bool_as_elem = self.convert_to(dst, CType::Int, elem_type);
            let result = self.fresh_tmp(elem_type);
            let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Sub,
                left: zero,
                right: bool_as_elem,
                dst: result.clone(),
            });
            return Ok((result, elem_type));
        }
        if let Some(width) = bit_precision.filter(|_| !is_comparison) {
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: dst.clone(),
                right: TackyVal::Constant(Self::bit_mask(width)),
                dst: dst.clone(),
            });
        }
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
                        let ret_ft = self.func_full_types.get(&self.current_function).cloned();
                        let val_ft = self.val_full_type(&val);
                        let src_addr = if let Some(ref ret_ft) = ret_ft {
                            if ret_ft.is_complex() {
                                if val_ft.is_complex() {
                                    self.get_struct_addr(val)
                                } else {
                                    let tmp = self.fresh_tmp_full(ret_ft);
                                    let TackyVal::Var(ref tmp_name) = tmp else {
                                        return Err(
                                            "internal error: hidden return value must be addressable"
                                                .to_string(),
                                        );
                                    };
                                    self.zero_init_local(
                                        tmp_name,
                                        ret_ft.byte_size_with(&self.struct_defs),
                                    );
                                    let FullType::Vector { elem, .. } = ret_ft.clone() else {
                                        return Err("internal error: expected complex return type"
                                            .to_string());
                                    };
                                    let elem_type = elem.to_ctype();
                                    let real = self.convert_to(val, val_type, elem_type);
                                    self.emit(TackyInstr::CopyToOffset {
                                        src: real,
                                        dst_name: tmp_name.clone(),
                                        offset: 0,
                                    });
                                    self.get_struct_addr(tmp)
                                }
                            } else if val_type == CType::Struct {
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
                                if let FullType::Struct(ret_tag) = ret_ft {
                                    if let FullType::Pointer(pointee) = &val_ft {
                                        if matches!(pointee.as_ref(), FullType::Struct(src_tag) if src_tag == ret_tag)
                                        {
                                            val
                                        } else {
                                            let a = self.fresh_tmp(CType::Pointer);
                                            self.emit(TackyInstr::GetAddress {
                                                src: val,
                                                dst: a.clone(),
                                            });
                                            a
                                        }
                                    } else {
                                        let a = self.fresh_tmp(CType::Pointer);
                                        self.emit(TackyInstr::GetAddress {
                                            src: val,
                                            dst: a.clone(),
                                        });
                                        a
                                    }
                                } else {
                                    let a = self.fresh_tmp(CType::Pointer);
                                    self.emit(TackyInstr::GetAddress {
                                        src: val,
                                        dst: a.clone(),
                                    });
                                    a
                                }
                            }
                        } else {
                            val
                        };
                        // Copy aggregate/complex result to hidden return pointer location
                        let ret_size = ret_ft
                            .as_ref()
                            .map(|ft| ft.byte_size_with(&self.struct_defs))
                            .unwrap_or(0);
                        self.emit_struct_copy_ptr_to_ptr(src_addr, ret_ptr_val.clone(), ret_size);
                        self.emit(TackyInstr::Return(ret_ptr_val));
                    } else {
                        let ret_ft = self
                            .func_full_types
                            .get(&self.current_function)
                            .cloned()
                            .unwrap_or(FullType::Scalar(ret_type));
                        let val_ft = self.val_full_type(&val);
                        if let (FullType::Struct(ref ret_tag), FullType::Pointer(ref pointee)) =
                            (&ret_ft, &val_ft)
                        {
                            if matches!(pointee.as_ref(), FullType::Struct(tag) if tag == ret_tag) {
                                let struct_size = self
                                    .struct_defs
                                    .get(ret_tag)
                                    .map(|def| def.size)
                                    .unwrap_or(0);
                                let result = self.fresh_tmp_full(&ret_ft);
                                if let TackyVal::Var(ref result_name) = result {
                                    self.array_sizes.insert(result_name.clone(), struct_size);
                                    self.emit_struct_copy_to(val, result_name, struct_size);
                                }
                                self.emit(TackyInstr::Return(result));
                                return Ok(());
                            }
                        }
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
                let branch_has_label = Self::statement_contains_label(then_stmt.as_ref())
                    || else_stmt
                        .as_deref()
                        .is_some_and(Self::statement_contains_label);
                if !branch_has_label {
                    if let Some((value, _, _)) = eval_static_integer_constant_exp_with_context(
                        &cond,
                        &self.struct_defs,
                        &self.full_types,
                    ) {
                        if value != 0 {
                            self.emit_statement(*then_stmt)?;
                        } else if let Some(else_s) = else_stmt {
                            self.emit_statement(*else_s)?;
                        }
                        return Ok(());
                    }
                } else if let Some((value, _, _)) = eval_static_integer_constant_exp_with_context(
                    &cond,
                    &self.struct_defs,
                    &self.full_types,
                ) {
                    if value == 0 && else_stmt.is_none() {
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::Jump(end_label.clone()));
                        if let Some(pruned_then) =
                            Self::prune_unreachable_prefix_to_label(then_stmt.as_ref().clone())
                        {
                            self.emit_statement(pruned_then)?;
                        }
                        self.emit(TackyInstr::Label(end_label));
                        return Ok(());
                    }
                }
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
                let local = self
                    .local_label_stack
                    .last()
                    .is_none_or(|labels| labels.contains(&label));
                let target_function = if local {
                    self.current_function.clone()
                } else if let Some(parent) = self.label_address_function.as_ref() {
                    parent.clone()
                } else {
                    self.current_function.clone()
                };
                let target = format!("label.{}.{}", target_function, label);
                if local || target_function == self.current_function {
                    self.emit(TackyInstr::Jump(target));
                } else {
                    self.emit(TackyInstr::NonlocalJump(target));
                }
            }
            Statement::IndirectGoto(exp) => {
                let (target, _) = self.emit_exp(exp)?;
                self.emit(TackyInstr::JumpIndirect(target));
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
                        if let Some(end_val) = case.end_value {
                            let ge_low = self.fresh_tmp(CType::Int);
                            let le_high = self.fresh_tmp(CType::Int);
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::GreaterEqual,
                                left: control_val.clone(),
                                right: TackyVal::Constant(val),
                                dst: ge_low.clone(),
                            });
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::LessEqual,
                                left: control_val.clone(),
                                right: TackyVal::Constant(end_val),
                                dst: le_high.clone(),
                            });
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::BitwiseAnd,
                                left: ge_low,
                                right: le_high,
                                dst: cmp_value.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::Equal,
                                left: control_val.clone(),
                                right: TackyVal::Constant(val),
                                dst: cmp_value.clone(),
                            });
                        }
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
    fn emit_initializer_list_at(
        &mut self,
        arr_name: &str,
        target_ft: &FullType,
        elems: &[Exp],
        index: &mut usize,
        base_offset: i64,
    ) -> TackyResult<()> {
        if *index >= elems.len() {
            return Ok(());
        }

        match target_ft {
            FullType::Array { elem, size } => {
                if *index < elems.len()
                    && matches!(elems[*index], Exp::StringLiteral(_))
                    && Self::is_one_dimensional_char_array(target_ft)
                {
                    self.emit_initializer_value_at(
                        arr_name,
                        target_ft,
                        &elems[*index],
                        base_offset,
                    )?;
                    *index += 1;
                    return Ok(());
                }
                while *index < elems.len() && matches!(elems[*index], Exp::DesignatedInit(_, _)) {
                    self.emit_initializer_value_at(
                        arr_name,
                        target_ft,
                        &elems[*index],
                        base_offset,
                    )?;
                    *index += 1;
                }
                if *index >= elems.len() {
                    return Ok(());
                }
                let elem_size = elem.byte_size_with(&self.struct_defs) as i64;
                let array_len = if *size == 0 {
                    elems.len().saturating_sub(*index)
                } else {
                    *size
                };
                for i in 0..array_len {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if (Self::static_aggregate_initializer(elem_init).is_some()
                        || matches!(elem_init, Exp::StringLiteral(_)))
                        && (elem.is_array() || elem.is_struct())
                    {
                        self.emit_initializer_value_at(
                            arr_name,
                            elem,
                            elem_init,
                            base_offset + i as i64 * elem_size,
                        )?;
                        *index += 1;
                    } else {
                        self.emit_initializer_list_at(
                            arr_name,
                            elem,
                            elems,
                            index,
                            base_offset + i as i64 * elem_size,
                        )?;
                    }
                }
            }
            FullType::Struct(tag) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .cloned()
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let max_members = if def.is_union { 1 } else { def.members.len() };
                for mem in def.members.iter().take(max_members) {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if mem.bit_width.is_some() {
                        let (val, val_type) = self.emit_exp(elem_init.clone())?;
                        let val_conv = self.convert_to(val, val_type, mem.member_type);
                        self.store_bit_field_to_absolute_offset(
                            arr_name.to_string(),
                            mem,
                            base_offset + mem.offset as i64,
                            val_conv,
                        )?;
                        *index += 1;
                        continue;
                    }
                    if mem.member_full_type.is_struct()
                        || ((Self::static_aggregate_initializer(elem_init).is_some()
                            || matches!(elem_init, Exp::StringLiteral(_)))
                            && (mem.member_full_type.is_array()
                                || mem.member_full_type.is_struct()))
                    {
                        self.emit_initializer_value_at(
                            arr_name,
                            &mem.member_full_type,
                            elem_init,
                            base_offset + mem.offset as i64,
                        )?;
                        *index += 1;
                    } else {
                        self.emit_initializer_list_at(
                            arr_name,
                            &mem.member_full_type,
                            elems,
                            index,
                            base_offset + mem.offset as i64,
                        )?;
                    }
                }
            }
            FullType::Scalar(_) | FullType::Pointer(_) | FullType::Vector { .. } => {
                self.emit_initializer_value_at(arr_name, target_ft, &elems[*index], base_offset)?;
                *index += 1;
            }
            FullType::Function { .. } => {
                return Err("function type is not an initializer target".to_string());
            }
        }
        Ok(())
    }

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
            if !has_designators {
                let mut index = 0usize;
                return self.emit_initializer_list_at(
                    arr_name,
                    &FullType::Struct(tag.to_string()),
                    elems,
                    &mut index,
                    base_offset,
                );
            }
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
                        let member_init = if elems.len() == 1 { &elems[0] } else { init };
                        self.emit_struct_init_at(arr_name, member_init, inner_tag, mem_offset)?;
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
                        if mem.bit_width.is_some() {
                            self.store_bit_field_to_offset(arr_name.to_string(), mem, val_conv)?;
                        } else {
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: arr_name.to_string(),
                                offset: mem_offset,
                            });
                        }
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
                            let struct_tag =
                                self.array_struct_element_tag_at_offset(arr_name, base_offset)?;
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

    fn direct_array_struct_elem(ft: &FullType) -> Option<(&str, usize)> {
        match ft {
            FullType::Array { elem, size } => match elem.as_ref() {
                FullType::Struct(tag) => Some((tag.as_str(), *size)),
                _ => None,
            },
            _ => None,
        }
    }

    fn emit_struct_member_initializer(
        &mut self,
        arr_name: &str,
        mem: &StructMember,
        value: &Exp,
        offset: i64,
    ) -> TackyResult<()> {
        if mem.bit_width.is_some() {
            let (val, val_type) = self.emit_exp(value.clone())?;
            let val_conv = self.convert_to(val, val_type, mem.member_type);
            self.store_bit_field_to_absolute_offset(arr_name.to_string(), mem, offset, val_conv)?;
            return Ok(());
        }

        self.emit_initializer_value_at(arr_name, &mem.member_full_type, value, offset)
    }

    fn emit_struct_array_init_flat(
        &mut self,
        arr_name: &str,
        init: &Exp,
        tag: &str,
        array_len: usize,
        base_offset: i64,
    ) -> TackyResult<()> {
        let Exp::ArrayInit(elems) = init else {
            return self.emit_initializer_value_at(
                arr_name,
                &FullType::Array {
                    elem: Box::new(FullType::Struct(tag.to_string())),
                    size: array_len,
                },
                init,
                base_offset,
            );
        };

        let def = self
            .struct_defs
            .get(tag)
            .cloned()
            .ok_or_else(|| format!("Undefined struct: {}", tag))?;
        let struct_size = def.size as i64;
        let max_members = if def.is_union { 1 } else { def.members.len() };
        let mut elem_index = 0usize;
        let mut member_index = 0usize;

        for elem in elems {
            if elem_index >= array_len {
                break;
            }
            let elem_offset = base_offset + elem_index as i64 * struct_size;
            match elem {
                Exp::DesignatedInit(designators, value) => {
                    self.emit_designated_init_at(
                        arr_name,
                        &FullType::Array {
                            elem: Box::new(FullType::Struct(tag.to_string())),
                            size: array_len,
                        },
                        designators,
                        value,
                        base_offset,
                    )?;
                }
                Exp::ArrayInit(_) => {
                    self.emit_struct_init_at(arr_name, elem, tag, elem_offset)?;
                    elem_index += 1;
                    member_index = 0;
                }
                _ => {
                    if member_index >= max_members {
                        elem_index += 1;
                        member_index = 0;
                    }
                    if elem_index >= array_len || member_index >= max_members {
                        break;
                    }
                    let elem_offset = base_offset + elem_index as i64 * struct_size;
                    let mem = &def.members[member_index];
                    self.emit_struct_member_initializer(
                        arr_name,
                        mem,
                        elem,
                        elem_offset + mem.offset as i64,
                    )?;
                    member_index += 1;
                    if member_index >= max_members {
                        elem_index += 1;
                        member_index = 0;
                    }
                }
            }
        }
        Ok(())
    }

    fn array_struct_element_tag_at_offset(
        &self,
        arr_name: &str,
        base_offset: i64,
    ) -> TackyResult<String> {
        let arr_ft = self.get_full_type(arr_name);
        let mut t = &arr_ft;
        while let FullType::Array { elem: e, .. } = t {
            t = e;
        }
        if let FullType::Struct(tag) = t {
            if let Some(def) = self.struct_defs.get(tag) {
                for mem in &def.members {
                    let start = mem.offset as i64;
                    let end = start + mem.member_full_type.byte_size_with(&self.struct_defs) as i64;
                    if base_offset >= start && base_offset < end {
                        let mut mt = &mem.member_full_type;
                        while let FullType::Array { elem, .. } = mt {
                            mt = elem;
                        }
                        if let FullType::Struct(member_tag) = mt {
                            return Ok(member_tag.clone());
                        }
                    }
                }
            }
            return Ok(tag.clone());
        }
        Err("Expected struct in array".to_string())
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

    fn static_designated_initializer_target(
        &self,
        base_ft: &FullType,
        designators: &[Designator],
        value: &Exp,
        base_offset: usize,
    ) -> TackyResult<(FullType, usize, Exp)> {
        let Some((designator, rest)) = designators.split_first() else {
            return Ok((base_ft.clone(), base_offset, value.clone()));
        };

        match (designator, base_ft) {
            (Designator::Index(index), FullType::Array { elem, size }) => {
                let index = Self::eval_designator_index(index)
                    .ok_or_else(|| "array designator index must be constant".to_string())?;
                if index < 0 || index as usize >= *size {
                    return Err(format!("array designator index {} out of bounds", index));
                }
                self.static_designated_initializer_target(
                    elem,
                    rest,
                    value,
                    base_offset + index as usize * elem.byte_size_with(&self.struct_defs),
                )
            }
            (Designator::IndexRange(_, _), FullType::Array { .. }) => {
                Err("array range designator is only valid inside initializer lists".to_string())
            }
            (Designator::Field(name), FullType::Struct(tag)) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let mem = def
                    .find_member(name)
                    .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?;
                self.static_designated_initializer_target(
                    &mem.member_full_type,
                    rest,
                    value,
                    base_offset + mem.offset,
                )
            }
            _ => Err(format!(
                "invalid initializer designator for type {:?}",
                base_ft
            )),
        }
    }

    fn statement_contains_label(stmt: &Statement) -> bool {
        match stmt {
            Statement::Label(_, _) | Statement::Case { .. } | Statement::Default { .. } => true,
            Statement::Block(block) => block.iter().any(|item| {
                matches!(item, BlockItem::Statement(stmt) if Self::statement_contains_label(stmt))
            }),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::statement_contains_label(then_stmt)
                    || else_stmt
                        .as_deref()
                        .is_some_and(Self::statement_contains_label)
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. } => Self::statement_contains_label(body),
            _ => false,
        }
    }

    fn prune_unreachable_prefix_to_label(stmt: Statement) -> Option<Statement> {
        match stmt {
            Statement::Label(_, _) | Statement::Case { .. } | Statement::Default { .. } => {
                Some(stmt)
            }
            Statement::Block(block) => {
                let mut out = Vec::new();
                let mut found = false;
                for item in block {
                    if found {
                        out.push(item);
                        continue;
                    }
                    if let BlockItem::Statement(stmt) = item {
                        if let Some(pruned) = Self::prune_unreachable_prefix_to_label(stmt) {
                            out.push(BlockItem::Statement(pruned));
                            found = true;
                        }
                    }
                }
                found.then_some(Statement::Block(out))
            }
            Statement::If(_, then_stmt, else_stmt) => {
                Self::prune_unreachable_prefix_to_label(*then_stmt).or_else(|| {
                    else_stmt
                        .and_then(|else_stmt| Self::prune_unreachable_prefix_to_label(*else_stmt))
                })
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. } => Self::prune_unreachable_prefix_to_label(*body),
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
        if target_ft.is_complex() {
            let (elem_type, elem_size) = match target_ft {
                FullType::Vector { elem, .. } => {
                    (elem.to_ctype(), elem.byte_size_with(&self.struct_defs))
                }
                _ => (CType::Double, 8),
            };
            match value {
                Exp::ArrayInit(elems) => {
                    if let Some(first) = elems.first() {
                        if elems.len() == 1 && self.typeof_exp(first).is_complex() {
                            let (val, val_type) = self.emit_exp(first.clone())?;
                            let val_ft = self.val_full_type(&val);
                            self.emit_complex_value_to_offset(
                                arr_name, target_ft, val, val_type, val_ft, offset,
                            )?;
                            return Ok(());
                        }
                        let (val, val_type) = self.emit_exp(first.clone())?;
                        let real = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: real,
                            dst_name: arr_name.to_string(),
                            offset,
                        });
                    }
                    if let Some(second) = elems.get(1) {
                        let (val, val_type) = self.emit_exp(second.clone())?;
                        let imag = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: imag,
                            dst_name: arr_name.to_string(),
                            offset: offset + elem_size as i64,
                        });
                    }
                }
                _ => {
                    let (val, val_type) = self.emit_exp(value.clone())?;
                    let real = self.convert_to(val, val_type, elem_type);
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: arr_name.to_string(),
                        offset,
                    });
                }
            }
            return Ok(());
        }
        match (target_ft, value) {
            (_, Exp::DesignatedInit(designators, inner)) => {
                self.emit_designated_init_at(arr_name, target_ft, designators, inner, offset)?;
            }
            (FullType::Array { .. }, Exp::ArrayInit(elems)) => {
                let mut index = 0usize;
                self.emit_initializer_list_at(arr_name, target_ft, elems, &mut index, offset)?;
            }
            (FullType::Array { .. }, Exp::StringLiteral(_)) => {
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
            (FullType::Struct(tag), _) => {
                if let Some(first_member) = self
                    .struct_defs
                    .get(tag)
                    .filter(|def| def.is_union)
                    .and_then(|def| def.members.first())
                    .cloned()
                {
                    return self.emit_struct_member_initializer(
                        arr_name,
                        &first_member,
                        value,
                        offset + first_member.offset as i64,
                    );
                }
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
            (Designator::IndexRange(start, end), FullType::Array { elem, size }) => {
                let start = Self::eval_designator_index(start)
                    .ok_or_else(|| "array designator range start must be constant".to_string())?;
                let end = Self::eval_designator_index(end)
                    .ok_or_else(|| "array designator range end must be constant".to_string())?;
                if start < 0 || end < start || end as usize >= *size {
                    return Err(format!(
                        "array designator range {}...{} out of bounds",
                        start, end
                    ));
                }
                let elem_size = elem.byte_size_with(&self.struct_defs) as i64;
                for index in start..=end {
                    self.emit_designated_init_at(
                        arr_name,
                        elem,
                        &designators[1..],
                        value,
                        base_offset + index * elem_size,
                    )?;
                }
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
                let member_offset = base_offset + mem.offset as i64;
                if designators.len() == 1 {
                    self.emit_struct_member_initializer(arr_name, &mem, value, member_offset)?;
                } else {
                    self.emit_designated_init_at(
                        arr_name,
                        &mem.member_full_type,
                        &designators[1..],
                        value,
                        member_offset,
                    )?;
                }
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
        if base_ft.is_complex() {
            let (elem_type, elem_size) = match base_ft {
                FullType::Vector { elem, .. } => {
                    (elem.to_ctype(), elem.byte_size_with(&self.struct_defs))
                }
                _ => (CType::Double, 8),
            };
            match init {
                Exp::ArrayInit(elems) => {
                    if let Some(first) = elems.first() {
                        let (v, is_dbl, is_uns) =
                            self.eval_static_constant_init(&Some(first.clone()))?;
                        let cv = convert_init_value(v, elem_type, is_dbl, is_uns);
                        builder.put(base_offset, make_static_init(cv, elem_type))?;
                    }
                    if let Some(second) = elems.get(1) {
                        let (v, is_dbl, is_uns) =
                            self.eval_static_constant_init(&Some(second.clone()))?;
                        let cv = convert_init_value(v, elem_type, is_dbl, is_uns);
                        builder.put(base_offset + elem_size, make_static_init(cv, elem_type))?;
                    }
                }
                _ => {
                    let ((real, real_dbl, real_uns), (imag, imag_dbl, imag_uns)) = self
                        .eval_static_complex_constant_init(init)
                        .ok_or_else(|| "Static complex initializer must be constant".to_string())?;
                    let cv = convert_init_value(real, elem_type, real_dbl, real_uns);
                    builder.put(base_offset, make_static_init(cv, elem_type))?;
                    let cv = convert_init_value(imag, elem_type, imag_dbl, imag_uns);
                    builder.put(base_offset + elem_size, make_static_init(cv, elem_type))?;
                }
            }
            return Ok(());
        }
        if let Exp::DesignatedInit(designators, value) = init {
            if let (FullType::Struct(tag), [Designator::Field(name)]) =
                (base_ft, designators.as_slice())
            {
                let def = self
                    .struct_defs
                    .get(tag)
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let mem = def
                    .find_member(name)
                    .ok_or_else(|| format!("struct '{}' has no member '{}'", tag, name))?;
                if let Some(width) = mem.bit_width {
                    let (raw, is_dbl, is_uns) =
                        self.eval_static_constant_init(&Some(value.as_ref().clone()))?;
                    let converted = convert_init_value(raw, mem.member_type, is_dbl, is_uns);
                    builder.put_bit_field(
                        base_offset + mem.offset,
                        mem.member_type,
                        converted,
                        mem.bit_offset,
                        width,
                    )?;
                    return Ok(());
                }
            }
            if let (
                Some((Designator::IndexRange(start, end), rest)),
                FullType::Array { elem, size },
            ) = (designators.split_first(), base_ft)
            {
                let start = Self::eval_designator_index(start)
                    .ok_or_else(|| "array designator range start must be constant".to_string())?;
                let end = Self::eval_designator_index(end)
                    .ok_or_else(|| "array designator range end must be constant".to_string())?;
                if start < 0 || end < start || end as usize >= *size {
                    return Err(format!(
                        "array designator range {}...{} out of bounds",
                        start, end
                    ));
                }
                let value = if rest.is_empty() {
                    value.as_ref().clone()
                } else {
                    Exp::DesignatedInit(rest.to_vec(), value.clone())
                };
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for index in start as usize..=end as usize {
                    self.put_static_initializer(
                        builder,
                        elem,
                        &value,
                        base_offset + index * elem_size,
                    )?;
                }
                return Ok(());
            }
            let (target_ft, target_offset, target_value) = self
                .static_designated_initializer_target(base_ft, designators, value, base_offset)?;
            return self.put_static_initializer(builder, &target_ft, &target_value, target_offset);
        }
        if let Exp::Cast(_, _, inner) = init {
            if let Exp::ArrayInit(elems) = inner.as_ref() {
                if base_ft.is_array() || base_ft.is_struct() || base_ft.is_vector() {
                    return self.put_static_initializer(builder, base_ft, inner, base_offset);
                }
                if let [value] = elems.as_slice() {
                    return self.put_static_initializer(builder, base_ft, value, base_offset);
                }
            }
        }
        match (base_ft, init) {
            (FullType::Array { .. }, Exp::ArrayInit(_))
                if Self::is_one_dimensional_char_array(base_ft)
                    && Self::string_array_initializer(init).is_some() =>
            {
                let s = Self::string_array_initializer(init)
                    .ok_or_else(|| "expected string array initializer".to_string())?;
                self.put_static_initializer(
                    builder,
                    base_ft,
                    &Exp::StringLiteral(s.clone()),
                    base_offset,
                )?;
            }
            (FullType::Array { size: 0, .. }, Exp::StringLiteral(s))
                if Self::is_one_dimensional_char_array(base_ft) =>
            {
                builder.put(base_offset, StaticInit::StringInit(s.clone(), false))?;
            }
            (FullType::Array { elem, size }, Exp::ArrayInit(elems)) => {
                if !elems
                    .iter()
                    .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)))
                {
                    let mut index = 0usize;
                    self.put_static_initializer_list(
                        builder,
                        base_ft,
                        elems,
                        &mut index,
                        base_offset,
                    )?;
                    return Ok(());
                }
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let mut positional_index = 0usize;
                for elem_init in elems {
                    if let Exp::DesignatedInit(designators, value) = elem_init {
                        let (designator, rest) = designators
                            .split_first()
                            .ok_or_else(|| "empty initializer designator".to_string())?;
                        let value = if rest.is_empty() {
                            value.as_ref().clone()
                        } else {
                            Exp::DesignatedInit(rest.to_vec(), value.clone())
                        };
                        match designator {
                            Designator::Index(index) => {
                                let index =
                                    Self::eval_designator_index(index).ok_or_else(|| {
                                        "array designator index must be constant".to_string()
                                    })?;
                                if index < 0 || index as usize >= *size {
                                    return Err(format!(
                                        "array designator index {} out of bounds",
                                        index
                                    ));
                                }
                                let index = index as usize;
                                self.put_static_initializer(
                                    builder,
                                    elem,
                                    &value,
                                    base_offset + index * elem_size,
                                )?;
                                positional_index = index + 1;
                            }
                            Designator::IndexRange(start, end) => {
                                let start =
                                    Self::eval_designator_index(start).ok_or_else(|| {
                                        "array designator range start must be constant".to_string()
                                    })?;
                                let end = Self::eval_designator_index(end).ok_or_else(|| {
                                    "array designator range end must be constant".to_string()
                                })?;
                                if start < 0 || end < start || end as usize >= *size {
                                    return Err(format!(
                                        "array designator range {}...{} out of bounds",
                                        start, end
                                    ));
                                }
                                for index in start as usize..=end as usize {
                                    self.put_static_initializer(
                                        builder,
                                        elem,
                                        &value,
                                        base_offset + index * elem_size,
                                    )?;
                                }
                                positional_index = end as usize + 1;
                            }
                            Designator::Field(_) => {
                                return Err("invalid array initializer designator".to_string());
                            }
                        }
                    } else {
                        let index = positional_index;
                        positional_index += 1;
                        if index >= *size {
                            break;
                        }
                        self.put_static_initializer(
                            builder,
                            elem,
                            elem_init,
                            base_offset + index * elem_size,
                        )?;
                    }
                }
            }
            (FullType::Array { elem, size }, Exp::WideStringLiteral(s))
                if !elem.to_ctype().is_char() =>
            {
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for (index, ch) in s.chars().take(*size).enumerate() {
                    self.put_static_initializer(
                        builder,
                        elem,
                        &Exp::Constant(ch as i64),
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
                if def.is_union && elems.len() == 1 && matches!(elems[0], Exp::ArrayInit(_)) {
                    let Exp::ArrayInit(inner_elems) = &elems[0] else {
                        return Err("internal error: expected nested array initializer".to_string());
                    };
                    let mut index = 0usize;
                    for mem in &def.members {
                        if index >= inner_elems.len() {
                            break;
                        }
                        self.put_static_initializer_list(
                            builder,
                            &mem.member_full_type,
                            inner_elems,
                            &mut index,
                            base_offset + mem.offset,
                        )?;
                    }
                    return Ok(());
                }
                if !elems
                    .iter()
                    .any(|elem| matches!(elem, Exp::DesignatedInit(_, _)))
                {
                    let mut index = 0usize;
                    self.put_static_initializer_list(
                        builder,
                        base_ft,
                        elems,
                        &mut index,
                        base_offset,
                    )?;
                    return Ok(());
                }
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
                    if let Some(width) = member.bit_width {
                        let (raw, is_dbl, is_uns) =
                            self.eval_static_constant_init(&Some(value.clone()))?;
                        let converted = convert_init_value(raw, member.member_type, is_dbl, is_uns);
                        builder.put_bit_field(
                            base_offset + member.offset,
                            member.member_type,
                            converted,
                            member.bit_offset,
                            width,
                        )?;
                        continue;
                    }
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
            (FullType::Vector { elem, lanes, .. }, Exp::ArrayInit(elems)) => {
                let elem_size = elem.byte_size_with(&self.struct_defs);
                for (index, elem_init) in elems.iter().take(*lanes).enumerate() {
                    self.put_static_initializer(
                        builder,
                        elem,
                        elem_init,
                        base_offset + index * elem_size,
                    )?;
                }
            }
            (FullType::Pointer(_), _) => {
                if let Some(ptr_init) = self.static_pointer_initializer(init) {
                    builder.put(base_offset, ptr_init)?;
                } else {
                    let (v, is_dbl, is_uns) =
                        self.eval_static_constant_init(&Some(init.clone()))?;
                    let cv = convert_init_value(v, CType::Pointer, is_dbl, is_uns);
                    builder.put(base_offset, make_static_init(cv, CType::Pointer))?;
                }
            }
            (FullType::Scalar(ctype), _) => {
                if let Some(label_diff) = self.static_label_diff_initializer(init, *ctype) {
                    builder.put(base_offset, label_diff)?;
                    return Ok(());
                }
                if let Some(pointer_diff) = self.static_pointer_diff_integer(init) {
                    builder.put(base_offset, make_static_init(pointer_diff, *ctype))?;
                    return Ok(());
                }
                let (v, is_dbl, is_uns) = self.eval_static_constant_init(&Some(init.clone()))?;
                let cv = convert_init_value(v, *ctype, is_dbl, is_uns);
                builder.put(base_offset, make_static_init(cv, *ctype))?;
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
        let mut builder = StaticInitBuilder::new();
        self.put_static_initializer(&mut builder, ft, init, 0)?;
        let total_bytes = ft
            .byte_size_with(&self.struct_defs)
            .max(builder.required_bytes());
        builder.finish(total_bytes)
    }

    fn eval_static_constant_init(&self, init: &Option<Exp>) -> TackyResult<(i64, bool, bool)> {
        if let Some(exp) = init {
            eval_static_integer_constant_exp_with_context_and_values(
                exp,
                &self.struct_defs,
                &self.full_types,
                &self.static_const_values,
            )
            .ok_or_else(|| "Static variable initializer must be a constant".to_string())
        } else {
            Ok((0, false, false))
        }
    }

    fn eval_static_complex_constant_init(&self, init: &Exp) -> Option<StaticComplexValue> {
        let zero = (0, false, false);
        match init {
            Exp::ImaginaryIntConstant(value) => Some((zero, (*value, false, false))),
            Exp::ImaginaryDoubleConstant(value) => {
                Some((zero, (value.to_bits() as i64, true, false)))
            }
            Exp::Unary(UnaryOp::Negate, inner) => {
                let (real, imag) = self.eval_static_complex_constant_init(inner)?;
                Some((neg_static_init_value(real), neg_static_init_value(imag)))
            }
            Exp::Binary(
                op @ (BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div),
                left,
                right,
            ) => {
                let (left_real, left_imag) = self.eval_static_complex_constant_init(left)?;
                let (right_real, right_imag) = self.eval_static_complex_constant_init(right)?;
                let left_real = static_init_value_to_f64(left_real);
                let left_imag = static_init_value_to_f64(left_imag);
                let right_real = static_init_value_to_f64(right_real);
                let right_imag = static_init_value_to_f64(right_imag);
                let (real, imag) = match op {
                    BinaryOp::Add => (left_real + right_real, left_imag + right_imag),
                    BinaryOp::Sub => (left_real - right_real, left_imag - right_imag),
                    BinaryOp::Mul => (
                        left_real * right_real - left_imag * right_imag,
                        left_real * right_imag + left_imag * right_real,
                    ),
                    BinaryOp::Div => {
                        let denom = right_real * right_real + right_imag * right_imag;
                        (
                            (left_real * right_real + left_imag * right_imag) / denom,
                            (left_imag * right_real - left_real * right_imag) / denom,
                        )
                    }
                    _ => unreachable!(),
                };
                Some((
                    (real.to_bits() as i64, true, false),
                    (imag.to_bits() as i64, true, false),
                ))
            }
            _ => {
                let value = self.eval_static_constant_init(&Some(init.clone())).ok()?;
                Some((value, zero))
            }
        }
    }

    fn put_static_initializer_list(
        &mut self,
        builder: &mut StaticInitBuilder,
        base_ft: &FullType,
        elems: &[Exp],
        index: &mut usize,
        base_offset: usize,
    ) -> TackyResult<()> {
        if *index >= elems.len() {
            return Ok(());
        }

        match base_ft {
            FullType::Array { elem, size } => {
                if *index < elems.len()
                    && matches!(elems[*index], Exp::StringLiteral(_))
                    && Self::is_one_dimensional_char_array(base_ft)
                {
                    self.put_static_initializer(builder, base_ft, &elems[*index], base_offset)?;
                    *index += 1;
                    return Ok(());
                }
                if *index < elems.len() && matches!(elems[*index], Exp::DesignatedInit(_, _)) {
                    self.put_static_initializer(builder, base_ft, &elems[*index], base_offset)?;
                    *index += 1;
                    return Ok(());
                }
                let elem_size = elem.byte_size_with(&self.struct_defs);
                if *size == 0 {
                    if *index >= elems.len() {
                        return Ok(());
                    }
                    if let Exp::ArrayInit(inner_elems) = &elems[*index] {
                        let mut inner_index = 0usize;
                        let mut elem_index = 0usize;
                        while inner_index < inner_elems.len() {
                            self.put_static_initializer_list(
                                builder,
                                elem,
                                inner_elems,
                                &mut inner_index,
                                base_offset + elem_index * elem_size,
                            )?;
                            elem_index += 1;
                        }
                        *index += 1;
                    } else {
                        let mut elem_index = 0usize;
                        while *index < elems.len() {
                            self.put_static_initializer_list(
                                builder,
                                elem,
                                elems,
                                index,
                                base_offset + elem_index * elem_size,
                            )?;
                            elem_index += 1;
                        }
                    }
                    return Ok(());
                }
                for i in 0..*size {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if (Self::static_aggregate_initializer(elem_init).is_some()
                        || matches!(elem_init, Exp::StringLiteral(_)))
                        && (elem.is_array() || elem.is_struct())
                    {
                        self.put_static_initializer(
                            builder,
                            elem,
                            elem_init,
                            base_offset + i * elem_size,
                        )?;
                        *index += 1;
                    } else {
                        self.put_static_initializer_list(
                            builder,
                            elem,
                            elems,
                            index,
                            base_offset + i * elem_size,
                        )?;
                    }
                }
            }
            FullType::Struct(tag) => {
                let def = self
                    .struct_defs
                    .get(tag)
                    .cloned()
                    .ok_or_else(|| format!("Undefined struct: {}", tag))?;
                let max_members = if def.is_union { 1 } else { def.members.len() };
                for mem in def.members.iter().take(max_members) {
                    if *index >= elems.len() {
                        break;
                    }
                    let elem_init = &elems[*index];
                    if let Some(width) = mem.bit_width {
                        let (value, _, _) =
                            self.eval_static_constant_init(&Some(elem_init.clone()))?;
                        builder.put_bit_field(
                            base_offset + mem.offset,
                            mem.member_type,
                            value,
                            mem.bit_offset,
                            width,
                        )?;
                        *index += 1;
                        continue;
                    }
                    if (Self::static_aggregate_initializer(elem_init).is_some()
                        || matches!(elem_init, Exp::StringLiteral(_)))
                        && (mem.member_full_type.is_array() || mem.member_full_type.is_struct())
                    {
                        self.put_static_initializer(
                            builder,
                            &mem.member_full_type,
                            elem_init,
                            base_offset + mem.offset,
                        )?;
                        *index += 1;
                    } else {
                        self.put_static_initializer_list(
                            builder,
                            &mem.member_full_type,
                            elems,
                            index,
                            base_offset + mem.offset,
                        )?;
                    }
                }
            }
            FullType::Pointer(_) | FullType::Scalar(_) | FullType::Vector { .. } => {
                self.put_static_initializer(builder, base_ft, &elems[*index], base_offset)?;
                *index += 1;
            }
            FullType::Function { .. } => {
                return Err("function type is not a static initializer target".to_string());
            }
        }
        Ok(())
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
            self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
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
            self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
            self.array_sizes.insert(vd.name.clone(), total_bytes);
            let scalar_type = {
                let mut t = &full_type;
                while let FullType::Array { elem, .. } = t {
                    t = elem;
                }
                t.to_ctype()
            };
            // Aggregate initializers zero-fill omitted elements; uninitialized automatic arrays do not.
            if vd.init.is_some() {
                self.zero_init_local(&vd.name, total_bytes);
            }
            if let Some(s) = vd
                .init
                .as_ref()
                .and_then(Self::string_array_initializer)
                .filter(|_| Self::is_one_dimensional_char_array(&full_type))
            {
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
                if let Exp::ArrayInit(elems) = &init {
                    let mut index = 0usize;
                    self.emit_initializer_list_at(&vd.name, &full_type, elems, &mut index, 0)?;
                    return Ok(());
                }
                if let Some((tag, array_len)) = Self::direct_array_struct_elem(&full_type) {
                    let tag = tag.to_string();
                    self.emit_struct_array_init_flat(&vd.name, &init, &tag, array_len, 0)?;
                    return Ok(());
                }
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
                self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
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
                self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
                self.extern_vars.push(vd.name);
                return Ok(());
            }
            self.register_var(&vd.name, ft);
            self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
            self.array_sizes.insert(vd.name.clone(), struct_size);
            // Aggregate initializers zero-fill omitted members; uninitialized automatic structs do not.
            if vd.init.is_some() {
                self.zero_init_local(&vd.name, struct_size);
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
                    if elems.is_empty() {
                        return Ok(());
                    }
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
                        if mem.bit_width.is_some() {
                            self.store_bit_field_to_offset(vd.name.clone(), mem, val_conv)?;
                        } else {
                            self.emit(TackyInstr::CopyToOffset {
                                src: val_conv,
                                dst_name: vd.name.clone(),
                                offset: 0,
                            });
                        }
                    }
                } else {
                    let max_members = def.members.len();
                    if max_members == 1 && def.members[0].member_full_type.is_array() {
                        let member = &def.members[0];
                        let elem_sizes =
                            Self::compute_elem_sizes(&member.member_full_type, &self.struct_defs);
                        let scalar_t = {
                            let mut t = &member.member_full_type;
                            while let FullType::Array { elem: e, .. } = t {
                                t = e;
                            }
                            t.to_ctype()
                        };
                        self.emit_array_init_flat(
                            &vd.name,
                            init_ref,
                            scalar_t,
                            member.offset as i64,
                            &elem_sizes,
                        )?;
                        return Ok(());
                    }
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
                            if member.bit_width.is_some() {
                                self.store_bit_field_to_offset(vd.name.clone(), member, val_conv)?;
                            } else {
                                self.emit(TackyInstr::CopyToOffset {
                                    src: val_conv,
                                    dst_name: vd.name.clone(),
                                    offset: member.offset as i64,
                                });
                            }
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

        // Use decl_full_type if available (preserves pointer-to-array info)
        let ft = if let Some(ref dft) = vd.decl_full_type {
            dft.clone()
        } else {
            FullType::from_decl(vd.var_type, vd.ptr_info, &None)
        };
        // Regular scalar/pointer variable. Vectors keep their full type for
        // lane operations, but use a storage-width scalar for backend moves.
        let storage_type = self.storage_ctype_for_full(&ft);
        self.var_types.insert(vd.name.clone(), storage_type);
        self.symbol_types.insert(vd.name.clone(), storage_type);
        if let Some(pi) = vd.ptr_info {
            self.ptr_info.insert(vd.name.clone(), pi);
        }
        self.full_types.insert(vd.name.clone(), ft.clone());
        self.register_dynamic_size(&vd.name, vd.dynamic_size.clone());
        if ft.is_vector() {
            self.array_sizes
                .insert(vd.name.clone(), ft.byte_size_with(&self.struct_defs));
        }
        let init = match vd.init {
            Some(Exp::ArrayInit(mut elems)) if elems.len() == 1 => Some(elems.remove(0)),
            Some(Exp::ArrayInit(elems)) if elems.is_empty() => Some(Exp::Constant(0)),
            other => other,
        };

        if vd
            .storage_class
            .as_ref()
            .is_some_and(StorageClass::is_static)
        {
            // Static pointer initialized with string literal: static char *p = "hello";
            if let Some(Exp::StringLiteral(ref s)) = init {
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
            if let Some(ptr_init) = (vd.var_type == CType::Pointer)
                .then(|| {
                    init.as_ref()
                        .and_then(|init| self.static_pointer_initializer(init))
                })
                .flatten()
            {
                let align = std::cmp::max(vd.var_type.size() as usize, 1);
                let align = vd.alignment.map_or(align, |a| a.get().max(align));
                self.static_vars.push(TackyStaticVar {
                    name: vd.name,
                    global: false,
                    thread_local: is_thread_local,
                    alignment: align,
                    init_values: vec![ptr_init],
                });
                return Ok(());
            }
            let (raw_val, is_dbl, is_uns) = self.eval_static_constant_init(&init)?;
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
        } else if let Some(init) = init {
            let vd_name = vd.name.clone();
            let init_for_type = init.clone();
            if ft.is_complex() {
                let size = ft.byte_size_with(&self.struct_defs);
                self.zero_init_local(&vd_name, size);
                if let Exp::ArrayInit(elems) = init {
                    let (elem_ft, elem_type, elem_size) = match &ft {
                        FullType::Vector { elem, .. } => (
                            elem.as_ref().clone(),
                            elem.to_ctype(),
                            elem.byte_size_with(&self.struct_defs),
                        ),
                        _ => (
                            FullType::Scalar(vd.var_type),
                            vd.var_type,
                            vd.var_type.size() as usize,
                        ),
                    };
                    for (index, elem_init) in elems.into_iter().take(2).enumerate() {
                        let (val, val_type) = self.emit_exp(elem_init.clone())?;
                        let val_ft = self.val_full_type(&val);
                        self.assert_assignable_exp_full_type(
                            &elem_ft,
                            &val_ft,
                            &elem_init,
                            "initializer",
                        )?;
                        let converted = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: vd_name.clone(),
                            offset: (index * elem_size) as i64,
                        });
                    }
                    return Ok(());
                }
                let (val, val_type) = self.emit_exp(init)?;
                let val_ft = self.val_full_type(&val);
                self.assert_assignable_exp_full_type(&ft, &val_ft, &init_for_type, "initializer")?;
                if val_ft.is_complex() {
                    let src_addr = self.get_struct_addr(val);
                    self.emit_struct_copy_to(src_addr, &vd_name, size);
                } else {
                    let elem_type = match &ft {
                        FullType::Vector { elem, .. } => elem.to_ctype(),
                        _ => vd.var_type,
                    };
                    let real = self.convert_to(val, val_type, elem_type);
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: vd_name.clone(),
                        offset: 0,
                    });
                }
                return Ok(());
            }
            if ft.is_vector() {
                let size = ft.byte_size_with(&self.struct_defs);
                self.zero_init_local(&vd_name, size);
                if let Exp::ArrayInit(elems) = init {
                    let (elem_ft, elem_type, elem_size) = match &ft {
                        FullType::Vector { elem, .. } => (
                            elem.as_ref().clone(),
                            elem.to_ctype(),
                            elem.byte_size_with(&self.struct_defs),
                        ),
                        _ => (
                            FullType::Scalar(vd.var_type),
                            vd.var_type,
                            vd.var_type.size() as usize,
                        ),
                    };
                    for (index, elem_init) in elems.into_iter().enumerate() {
                        let (val, val_type) = self.emit_exp(elem_init.clone())?;
                        let val_ft = self.val_full_type(&val);
                        self.assert_assignable_exp_full_type(
                            &elem_ft,
                            &val_ft,
                            &elem_init,
                            "initializer",
                        )?;
                        let converted = self.convert_to(val, val_type, elem_type);
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: vd_name.clone(),
                            offset: (index * elem_size) as i64,
                        });
                    }
                    return Ok(());
                }
            }
            let (val, val_type) = self.emit_exp(init)?;
            let val_ft = self.val_full_type(&val);
            self.assert_assignable_exp_full_type(&ft, &val_ft, &init_for_type, "initializer")?;
            if ft.is_vector() {
                let size = ft.byte_size_with(&self.struct_defs);
                let src_addr = if let TackyVal::Var(ref src_name) = val {
                    if self.array_sizes.contains_key(src_name) {
                        let addr = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: val,
                            dst: addr.clone(),
                        });
                        addr
                    } else {
                        self.zero_init_local(&vd_name, size);
                        let converted = self.convert_to(val, val_type, ft.to_ctype());
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: vd_name.clone(),
                            offset: 0,
                        });
                        return Ok(());
                    }
                } else {
                    self.zero_init_local(&vd_name, size);
                    let converted = self.convert_to(val, val_type, ft.to_ctype());
                    self.emit(TackyInstr::CopyToOffset {
                        src: converted,
                        dst_name: vd_name.clone(),
                        offset: 0,
                    });
                    return Ok(());
                };
                self.emit_struct_copy_to(src_addr, &vd_name, size);
                return Ok(());
            }
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
                    self.function_symbols.insert(fd.name.clone());
                    let param_types: Vec<CType> = fd.params.iter().map(|(_, t, _)| *t).collect();
                    self.func_types.insert(
                        fd.name.clone(),
                        (fd.return_type, param_types, fd.return_ptr_info, fd.variadic),
                    );
                    if fd.old_style {
                        self.old_style_functions.insert(fd.name.clone());
                    } else {
                        self.old_style_functions.remove(&fd.name);
                    }
                    self.record_zero_fixed_variadic_function(&fd);
                    self.func_param_full_types
                        .insert(fd.name.clone(), fd.param_full_types.clone());
                    if let Some(ref rft) = fd.return_full_type {
                        self.func_full_types.insert(fd.name.clone(), rft.clone());
                    }
                    if fd.body.is_some() {
                        self.emit_nested_function(fd)?;
                    }
                }
                BlockItem::Declaration(Declaration::StructDecl(sd)) => {
                    if sd.is_union && sd.transparent_union {
                        if let Some(member) = sd.members.first() {
                            self.transparent_unions
                                .insert(sd.tag.clone(), member.member_full_type.clone());
                        }
                    }
                    let def = StructDef::from_declaration(&sd, &self.struct_defs)?;
                    self.struct_defs.insert(sd.tag.clone(), def);
                }
                BlockItem::Declaration(Declaration::TypedefDecl) => {}
                BlockItem::Statement(stmt) => self.emit_statement(stmt)?,
            }
        }
        Ok(())
    }

    fn emit_nested_capture_updates(&mut self, function_name: &str) {
        let Some(captures) = self.nested_capture_slots.get(function_name).cloned() else {
            return;
        };
        for (capture, slot) in captures {
            let src = self
                .nested_capture_slots
                .get(&self.current_function)
                .and_then(|current_captures| {
                    current_captures
                        .iter()
                        .find_map(|(current_capture, current_slot)| {
                            (current_capture == &capture)
                                .then(|| TackyVal::Var(current_slot.clone()))
                        })
                });
            let src = if let Some(src) = src {
                src
            } else {
                let addr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(capture),
                    dst: addr.clone(),
                });
                addr
            };
            self.emit(TackyInstr::Copy {
                src,
                dst: TackyVal::Var(slot),
            });
        }
    }

    fn emit_nested_function(&mut self, mut fd: FunctionDeclaration) -> TackyResult<()> {
        let mut nested_labels = HashSet::new();
        if let Some(body) = fd.body.as_ref() {
            Self::collect_block_labels(body, &mut nested_labels);
        }
        if self.current_escaped_functions.contains(&fd.name) {
            if let Some(body) = fd.body.take() {
                fd.body = Some(Self::rewrite_parent_label_gotos_block(
                    body,
                    &nested_labels,
                    &self.current_label_bodies,
                ));
            }
        }
        let captures = self.collect_captures_for_nested(&fd);
        let mut capture_map = HashMap::new();
        let mut capture_slots = Vec::new();
        for capture in captures {
            let Some(captured_ft) = self.full_types.get(&capture).cloned() else {
                continue;
            };
            let slot = format!("__rnqcc_chain_{}_{}", fd.name, capture.replace('.', "_"));
            let slot_ft = FullType::Pointer(Box::new(captured_ft));
            self.register_var(&slot, slot_ft);
            self.static_vars.push(TackyStaticVar {
                name: slot.clone(),
                global: false,
                thread_local: false,
                alignment: 8,
                init_values: vec![StaticInit::ZeroInit(8)],
            });
            capture_slots.push((capture.clone(), slot.clone()));
            capture_map.insert(capture, slot);
        }
        self.nested_capture_slots
            .insert(fd.name.clone(), capture_slots);
        if let Some(body) = fd.body.take() {
            fd.body = Some(Self::rewrite_capture_block(body, &capture_map));
        }

        let saved_instructions = std::mem::take(&mut self.instructions);
        let saved_current = std::mem::take(&mut self.current_function);
        let saved_current_params = std::mem::take(&mut self.current_function_params);
        let saved_label_function = self.label_address_function.take();
        let saved_label_bodies = std::mem::take(&mut self.current_label_bodies);
        let saved_escaped_functions = std::mem::take(&mut self.current_escaped_functions);
        let saved_hidden_ret = self.hidden_ret_ptr.take();
        if !saved_current.is_empty() {
            self.label_address_function = Some(saved_current.clone());
        }
        if let Some(mut nested) = self.emit_function(fd)? {
            nested.global = false;
            self.nested_functions.push(nested);
        }
        self.instructions = saved_instructions;
        self.current_function = saved_current;
        self.current_function_params = saved_current_params;
        self.label_address_function = saved_label_function;
        self.current_label_bodies = saved_label_bodies;
        self.current_escaped_functions = saved_escaped_functions;
        self.hidden_ret_ptr = saved_hidden_ret;
        Ok(())
    }

    fn rewrite_parent_label_gotos_block(
        block: Block,
        local_labels: &HashSet<String>,
        parent_labels: &HashMap<String, Statement>,
    ) -> Block {
        block
            .into_iter()
            .map(|item| match item {
                BlockItem::Statement(stmt) => BlockItem::Statement(
                    Self::rewrite_parent_label_gotos_stmt(stmt, local_labels, parent_labels),
                ),
                other => other,
            })
            .collect()
    }

    fn rewrite_parent_label_gotos_stmt(
        stmt: Statement,
        local_labels: &HashSet<String>,
        parent_labels: &HashMap<String, Statement>,
    ) -> Statement {
        match stmt {
            Statement::Goto(label) if !local_labels.contains(&label) => parent_labels
                .get(&label)
                .cloned()
                .unwrap_or(Statement::Goto(label)),
            Statement::If(cond, then_stmt, else_stmt) => Statement::If(
                cond,
                Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *then_stmt,
                    local_labels,
                    parent_labels,
                )),
                else_stmt.map(|stmt| {
                    Box::new(Self::rewrite_parent_label_gotos_stmt(
                        *stmt,
                        local_labels,
                        parent_labels,
                    ))
                }),
            ),
            Statement::Block(block) => Statement::Block(Self::rewrite_parent_label_gotos_block(
                block,
                local_labels,
                parent_labels,
            )),
            Statement::While {
                condition,
                body,
                label,
            } => Statement::While {
                condition,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            Statement::DoWhile {
                body,
                condition,
                label,
            } => Statement::DoWhile {
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                condition,
                label,
            },
            Statement::For {
                init,
                condition,
                post,
                body,
                label,
            } => Statement::For {
                init,
                condition,
                post,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            Statement::Label(label, body) => Statement::Label(
                label,
                Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
            ),
            Statement::Switch {
                control,
                body,
                label,
                cases,
            } => Statement::Switch {
                control,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
                cases,
            },
            Statement::Case {
                value,
                end_value,
                body,
                label,
            } => Statement::Case {
                value,
                end_value,
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            Statement::Default { body, label } => Statement::Default {
                body: Box::new(Self::rewrite_parent_label_gotos_stmt(
                    *body,
                    local_labels,
                    parent_labels,
                )),
                label,
            },
            other => other,
        }
    }

    fn collect_captures_for_nested(&self, fd: &FunctionDeclaration) -> Vec<String> {
        let mut local_names: std::collections::HashSet<String> =
            fd.params.iter().map(|(name, _, _)| name.clone()).collect();
        if let Some(body) = fd.body.as_ref() {
            Self::collect_declared_names(body, &mut local_names);
        }
        let mut used = Vec::new();
        if let Some(body) = fd.body.as_ref() {
            Self::collect_used_vars_block(body, &mut used);
        }
        let mut captures: Vec<String> = used
            .iter()
            .filter(|name| {
                !local_names.contains(*name)
                    && self.full_types.contains_key(*name)
                    && !self.function_symbols.contains(*name)
            })
            .cloned()
            .collect();
        for name in used {
            if let Some(nested_captures) = self.nested_capture_slots.get(&name) {
                for (capture, _) in nested_captures {
                    if !local_names.contains(capture)
                        && self.full_types.contains_key(capture)
                        && !captures.iter().any(|existing| existing == capture)
                    {
                        captures.push(capture.clone());
                    }
                }
            }
        }
        captures
    }

    fn collect_declared_names(block: &Block, names: &mut std::collections::HashSet<String>) {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    names.insert(vd.name.clone());
                }
                BlockItem::Declaration(Declaration::FunDecl(fd)) => {
                    names.insert(fd.name.clone());
                }
                BlockItem::Statement(stmt) => Self::collect_declared_names_stmt(stmt, names),
                _ => {}
            }
        }
    }

    fn collect_declared_names_stmt(
        stmt: &Statement,
        names: &mut std::collections::HashSet<String>,
    ) {
        match stmt {
            Statement::Block(block) => Self::collect_declared_names(block, names),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_declared_names_stmt(then_stmt, names);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_declared_names_stmt(else_stmt, names);
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::Label(_, body)
            | Statement::Case { body, .. }
            | Statement::Default { body, .. }
            | Statement::Switch { body, .. } => Self::collect_declared_names_stmt(body, names),
            Statement::For { init, body, .. } => {
                if let ForInit::Declaration(vd) = init.as_ref() {
                    names.insert(vd.name.clone());
                }
                Self::collect_declared_names_stmt(body, names);
            }
            _ => {}
        }
    }

    fn collect_used_vars_block(block: &Block, used: &mut Vec<String>) {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    if let Some(init) = vd.init.as_ref() {
                        Self::collect_used_vars_exp(init, used);
                    }
                }
                BlockItem::Declaration(Declaration::FunDecl(_)) => {}
                BlockItem::Statement(stmt) => Self::collect_used_vars_stmt(stmt, used),
                _ => {}
            }
        }
    }

    fn push_used_var(name: &str, used: &mut Vec<String>) {
        if !used.iter().any(|existing| existing == name) {
            used.push(name.to_string());
        }
    }

    fn collect_used_vars_exp(exp: &Exp, used: &mut Vec<String>) {
        match exp {
            Exp::Var(name) => Self::push_used_var(name, used),
            Exp::Cast(_, _, inner)
            | Exp::Unary(_, inner)
            | Exp::SizeOf(inner)
            | Exp::Dot(inner, _)
            | Exp::Arrow(inner, _) => Self::collect_used_vars_exp(inner, used),
            Exp::Binary(_, left, right)
            | Exp::Assign(left, right)
            | Exp::CompoundAssign(_, left, right)
            | Exp::Subscript(left, right)
            | Exp::Comma(left, right) => {
                Self::collect_used_vars_exp(left, used);
                Self::collect_used_vars_exp(right, used);
            }
            Exp::Conditional(a, b, c) => {
                Self::collect_used_vars_exp(a, used);
                Self::collect_used_vars_exp(b, used);
                Self::collect_used_vars_exp(c, used);
            }
            Exp::BuiltinExpect(value, hints) => {
                Self::collect_used_vars_exp(value, used);
                for hint in hints {
                    Self::collect_used_vars_exp(hint, used);
                }
            }
            Exp::FunctionCall(name, args) => {
                Self::push_used_var(name, used);
                for arg in args {
                    Self::collect_used_vars_exp(arg, used);
                }
            }
            Exp::IndirectCall(callee, args) => {
                Self::collect_used_vars_exp(callee, used);
                for arg in args {
                    Self::collect_used_vars_exp(arg, used);
                }
            }
            Exp::ArrayInit(elems) => {
                for elem in elems {
                    Self::collect_used_vars_exp(elem, used);
                }
            }
            Exp::DesignatedInit(designators, value) => {
                for designator in designators {
                    match designator {
                        Designator::Index(index) => Self::collect_used_vars_exp(index, used),
                        Designator::IndexRange(start, end) => {
                            Self::collect_used_vars_exp(start, used);
                            Self::collect_used_vars_exp(end, used);
                        }
                        Designator::Field(_) => {}
                    }
                }
                Self::collect_used_vars_exp(value, used);
            }
            Exp::AtomicFetch { ptr, arg, .. } => {
                Self::collect_used_vars_exp(ptr, used);
                Self::collect_used_vars_exp(arg, used);
            }
            Exp::AtomicExchange { ptr, value } => {
                Self::collect_used_vars_exp(ptr, used);
                Self::collect_used_vars_exp(value, used);
            }
            Exp::AtomicCompareExchange {
                ptr,
                expected,
                desired,
            }
            | Exp::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                ..
            } => {
                Self::collect_used_vars_exp(ptr, used);
                Self::collect_used_vars_exp(expected, used);
                Self::collect_used_vars_exp(desired, used);
            }
            Exp::StatementExpr(block, result, _) => {
                Self::collect_used_vars_block(block, used);
                if let Some(result) = result {
                    Self::collect_used_vars_exp(result, used);
                }
            }
            _ => {}
        }
    }

    fn collect_used_vars_stmt(stmt: &Statement, used: &mut Vec<String>) {
        match stmt {
            Statement::Return(Some(exp))
            | Statement::Expression(exp)
            | Statement::IndirectGoto(exp) => {
                Self::collect_used_vars_exp(exp, used);
            }
            Statement::If(cond, then_stmt, else_stmt) => {
                Self::collect_used_vars_exp(cond, used);
                Self::collect_used_vars_stmt(then_stmt, used);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_used_vars_stmt(else_stmt, used);
                }
            }
            Statement::Block(block) => Self::collect_used_vars_block(block, used),
            Statement::While {
                condition, body, ..
            } => {
                Self::collect_used_vars_exp(condition, used);
                Self::collect_used_vars_stmt(body, used);
            }
            Statement::DoWhile {
                body, condition, ..
            } => {
                Self::collect_used_vars_stmt(body, used);
                Self::collect_used_vars_exp(condition, used);
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                ..
            } => {
                match init.as_ref() {
                    ForInit::Declaration(vd) => {
                        if let Some(init) = vd.init.as_ref() {
                            Self::collect_used_vars_exp(init, used);
                        }
                    }
                    ForInit::Expression(Some(exp)) => Self::collect_used_vars_exp(exp, used),
                    ForInit::Expression(None) => {}
                }
                if let Some(condition) = condition {
                    Self::collect_used_vars_exp(condition, used);
                }
                if let Some(post) = post {
                    Self::collect_used_vars_exp(post, used);
                }
                Self::collect_used_vars_stmt(body, used);
            }
            Statement::Label(_, body)
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::collect_used_vars_stmt(body, used),
            Statement::Switch { control, body, .. } => {
                Self::collect_used_vars_exp(control, used);
                Self::collect_used_vars_stmt(body, used);
            }
            _ => {}
        }
    }

    fn rewrite_capture_block(block: Block, capture_map: &HashMap<String, String>) -> Block {
        block
            .into_iter()
            .map(|item| match item {
                BlockItem::Declaration(Declaration::VarDecl(mut vd)) => {
                    vd.init = vd
                        .init
                        .map(|init| Self::rewrite_capture_exp(init, capture_map));
                    BlockItem::Declaration(Declaration::VarDecl(vd))
                }
                BlockItem::Statement(stmt) => {
                    BlockItem::Statement(Self::rewrite_capture_stmt(stmt, capture_map))
                }
                other => other,
            })
            .collect()
    }

    fn rewrite_capture_exp(exp: Exp, capture_map: &HashMap<String, String>) -> Exp {
        match exp {
            Exp::Var(name) => capture_map.get(&name).map_or(Exp::Var(name), |slot| {
                Exp::Unary(UnaryOp::Deref, Box::new(Exp::Var(slot.clone())))
            }),
            Exp::Cast(ct, ft, inner) => Exp::Cast(
                ct,
                ft,
                Box::new(Self::rewrite_capture_exp(*inner, capture_map)),
            ),
            Exp::Unary(op, inner) => {
                Exp::Unary(op, Box::new(Self::rewrite_capture_exp(*inner, capture_map)))
            }
            Exp::Binary(op, left, right) => Exp::Binary(
                op,
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::Assign(left, right) => Exp::Assign(
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::CompoundAssign(op, left, right) => Exp::CompoundAssign(
                op,
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::Conditional(a, b, c) => Exp::Conditional(
                Box::new(Self::rewrite_capture_exp(*a, capture_map)),
                Box::new(Self::rewrite_capture_exp(*b, capture_map)),
                Box::new(Self::rewrite_capture_exp(*c, capture_map)),
            ),
            Exp::BuiltinExpect(value, hints) => Exp::BuiltinExpect(
                Box::new(Self::rewrite_capture_exp(*value, capture_map)),
                hints
                    .into_iter()
                    .map(|arg| Self::rewrite_capture_exp(arg, capture_map))
                    .collect(),
            ),
            Exp::FunctionCall(name, args) => Exp::FunctionCall(
                name,
                args.into_iter()
                    .map(|arg| Self::rewrite_capture_exp(arg, capture_map))
                    .collect(),
            ),
            Exp::Subscript(a, b) => Exp::Subscript(
                Box::new(Self::rewrite_capture_exp(*a, capture_map)),
                Box::new(Self::rewrite_capture_exp(*b, capture_map)),
            ),
            Exp::ArrayInit(elems) => Exp::ArrayInit(
                elems
                    .into_iter()
                    .map(|elem| Self::rewrite_capture_exp(elem, capture_map))
                    .collect(),
            ),
            Exp::DesignatedInit(designators, value) => Exp::DesignatedInit(
                designators
                    .into_iter()
                    .map(|designator| match designator {
                        Designator::Index(index) => Designator::Index(Box::new(
                            Self::rewrite_capture_exp(*index, capture_map),
                        )),
                        Designator::IndexRange(start, end) => Designator::IndexRange(
                            Box::new(Self::rewrite_capture_exp(*start, capture_map)),
                            Box::new(Self::rewrite_capture_exp(*end, capture_map)),
                        ),
                        other => other,
                    })
                    .collect(),
                Box::new(Self::rewrite_capture_exp(*value, capture_map)),
            ),
            Exp::Dot(inner, member) => Exp::Dot(
                Box::new(Self::rewrite_capture_exp(*inner, capture_map)),
                member,
            ),
            Exp::Arrow(inner, member) => Exp::Arrow(
                Box::new(Self::rewrite_capture_exp(*inner, capture_map)),
                member,
            ),
            Exp::Comma(left, right) => Exp::Comma(
                Box::new(Self::rewrite_capture_exp(*left, capture_map)),
                Box::new(Self::rewrite_capture_exp(*right, capture_map)),
            ),
            Exp::StatementExpr(block, result, ft) => Exp::StatementExpr(
                Self::rewrite_capture_block(block, capture_map),
                result.map(|result| Box::new(Self::rewrite_capture_exp(*result, capture_map))),
                ft,
            ),
            Exp::IndirectCall(callee, args) => Exp::IndirectCall(
                Box::new(Self::rewrite_capture_exp(*callee, capture_map)),
                args.into_iter()
                    .map(|arg| Self::rewrite_capture_exp(arg, capture_map))
                    .collect(),
            ),
            other => other,
        }
    }

    fn rewrite_capture_stmt(stmt: Statement, capture_map: &HashMap<String, String>) -> Statement {
        match stmt {
            Statement::Return(exp) => {
                Statement::Return(exp.map(|exp| Self::rewrite_capture_exp(exp, capture_map)))
            }
            Statement::Expression(exp) => {
                Statement::Expression(Self::rewrite_capture_exp(exp, capture_map))
            }
            Statement::IndirectGoto(exp) => {
                Statement::IndirectGoto(Self::rewrite_capture_exp(exp, capture_map))
            }
            Statement::If(cond, then_stmt, else_stmt) => Statement::If(
                Self::rewrite_capture_exp(cond, capture_map),
                Box::new(Self::rewrite_capture_stmt(*then_stmt, capture_map)),
                else_stmt.map(|stmt| Box::new(Self::rewrite_capture_stmt(*stmt, capture_map))),
            ),
            Statement::Block(block) => {
                Statement::Block(Self::rewrite_capture_block(block, capture_map))
            }
            Statement::While {
                condition,
                body,
                label,
            } => Statement::While {
                condition: Self::rewrite_capture_exp(condition, capture_map),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            Statement::DoWhile {
                body,
                condition,
                label,
            } => Statement::DoWhile {
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                condition: Self::rewrite_capture_exp(condition, capture_map),
                label,
            },
            Statement::For {
                init,
                condition,
                post,
                body,
                label,
            } => Statement::For {
                init: Box::new(match *init {
                    ForInit::Declaration(mut vd) => {
                        vd.init = vd
                            .init
                            .map(|init| Self::rewrite_capture_exp(init, capture_map));
                        ForInit::Declaration(vd)
                    }
                    ForInit::Expression(exp) => ForInit::Expression(
                        exp.map(|exp| Self::rewrite_capture_exp(exp, capture_map)),
                    ),
                }),
                condition: condition.map(|exp| Self::rewrite_capture_exp(exp, capture_map)),
                post: post.map(|exp| Self::rewrite_capture_exp(exp, capture_map)),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            Statement::Label(name, body) => Statement::Label(
                name,
                Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
            ),
            Statement::Switch {
                control,
                body,
                label,
                cases,
            } => Statement::Switch {
                control: Self::rewrite_capture_exp(control, capture_map),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
                cases,
            },
            Statement::Case {
                value,
                end_value,
                body,
                label,
            } => Statement::Case {
                value: Self::rewrite_capture_exp(value, capture_map),
                end_value: end_value
                    .map(|end_value| Self::rewrite_capture_exp(end_value, capture_map)),
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            Statement::Default { body, label } => Statement::Default {
                body: Box::new(Self::rewrite_capture_stmt(*body, capture_map)),
                label,
            },
            other => other,
        }
    }

    fn collect_statement_labels(stmt: &Statement, labels: &mut HashSet<String>) {
        match stmt {
            Statement::Label(name, body) => {
                labels.insert(name.clone());
                Self::collect_statement_labels(body, labels);
            }
            Statement::Block(block) => Self::collect_block_labels(block, labels),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_statement_labels(then_stmt, labels);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_statement_labels(else_stmt, labels);
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. }
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::collect_statement_labels(body, labels),
            Statement::Return(_)
            | Statement::Expression(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::IndirectGoto(_)
            | Statement::Null => {}
        }
    }

    fn collect_statement_label_bodies(stmt: &Statement, labels: &mut HashMap<String, Statement>) {
        match stmt {
            Statement::Label(name, body) => {
                labels.insert(name.clone(), body.as_ref().clone());
                Self::collect_statement_label_bodies(body, labels);
            }
            Statement::Block(block) => Self::collect_block_label_bodies(block, labels),
            Statement::If(_, then_stmt, else_stmt) => {
                Self::collect_statement_label_bodies(then_stmt, labels);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_statement_label_bodies(else_stmt, labels);
                }
            }
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::Switch { body, .. }
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => Self::collect_statement_label_bodies(body, labels),
            Statement::Return(_)
            | Statement::Expression(_)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::IndirectGoto(_)
            | Statement::Null => {}
        }
    }

    fn collect_block_labels(block: &Block, labels: &mut HashSet<String>) {
        for item in block {
            if let BlockItem::Statement(stmt) = item {
                Self::collect_statement_labels(stmt, labels);
            }
        }
    }

    fn collect_block_label_bodies(block: &Block, labels: &mut HashMap<String, Statement>) {
        for item in block {
            if let BlockItem::Statement(stmt) = item {
                Self::collect_statement_label_bodies(stmt, labels);
            }
        }
    }

    fn collect_escaped_function_refs_exp(exp: &Exp, refs: &mut HashSet<String>) {
        match exp {
            Exp::Var(name) => {
                refs.insert(name.clone());
            }
            Exp::FunctionCall(_, args) => {
                for arg in args {
                    Self::collect_escaped_function_refs_exp(arg, refs);
                }
            }
            Exp::Cast(_, _, inner)
            | Exp::Unary(_, inner)
            | Exp::SizeOf(inner)
            | Exp::Dot(inner, _)
            | Exp::Arrow(inner, _) => Self::collect_escaped_function_refs_exp(inner, refs),
            Exp::Binary(_, left, right)
            | Exp::Assign(left, right)
            | Exp::CompoundAssign(_, left, right)
            | Exp::Subscript(left, right)
            | Exp::Comma(left, right) => {
                Self::collect_escaped_function_refs_exp(left, refs);
                Self::collect_escaped_function_refs_exp(right, refs);
            }
            Exp::Conditional(cond, then_exp, else_exp) => {
                Self::collect_escaped_function_refs_exp(cond, refs);
                Self::collect_escaped_function_refs_exp(then_exp, refs);
                Self::collect_escaped_function_refs_exp(else_exp, refs);
            }
            Exp::BuiltinExpect(value, hints) => {
                Self::collect_escaped_function_refs_exp(value, refs);
                for hint in hints {
                    Self::collect_escaped_function_refs_exp(hint, refs);
                }
            }
            Exp::ArrayInit(elems) => {
                for elem in elems {
                    Self::collect_escaped_function_refs_exp(elem, refs);
                }
            }
            Exp::DesignatedInit(_, value) => Self::collect_escaped_function_refs_exp(value, refs),
            Exp::StatementExpr(block, tail, _) => {
                Self::collect_escaped_function_refs_block(block, refs);
                if let Some(tail) = tail {
                    Self::collect_escaped_function_refs_exp(tail, refs);
                }
            }
            Exp::IndirectCall(callee, args) => {
                Self::collect_escaped_function_refs_exp(callee, refs);
                for arg in args {
                    Self::collect_escaped_function_refs_exp(arg, refs);
                }
            }
            Exp::AtomicFetch { ptr, arg, .. } => {
                Self::collect_escaped_function_refs_exp(ptr, refs);
                Self::collect_escaped_function_refs_exp(arg, refs);
            }
            Exp::AtomicExchange { ptr, value } => {
                Self::collect_escaped_function_refs_exp(ptr, refs);
                Self::collect_escaped_function_refs_exp(value, refs);
            }
            Exp::AtomicCompareExchange {
                ptr,
                expected,
                desired,
            }
            | Exp::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                ..
            } => {
                Self::collect_escaped_function_refs_exp(ptr, refs);
                Self::collect_escaped_function_refs_exp(expected, refs);
                Self::collect_escaped_function_refs_exp(desired, refs);
            }
            Exp::Constant(_)
            | Exp::LongConstant(_)
            | Exp::Int128Constant(_)
            | Exp::UIntConstant(_)
            | Exp::ULongConstant(_)
            | Exp::UInt128Constant(_)
            | Exp::DoubleConstant(_)
            | Exp::LongDoubleConstant(_)
            | Exp::ImaginaryIntConstant(_)
            | Exp::ImaginaryDoubleConstant(_)
            | Exp::StringLiteral(_)
            | Exp::WideStringLiteral(_)
            | Exp::LabelAddress(_)
            | Exp::SizeOfType(_, _)
            | Exp::AlignOfType(_)
            | Exp::Unreachable
            | Exp::AtomicFence => {}
        }
    }

    fn collect_escaped_function_refs_stmt(stmt: &Statement, refs: &mut HashSet<String>) {
        match stmt {
            Statement::Return(Some(exp))
            | Statement::Expression(exp)
            | Statement::IndirectGoto(exp) => Self::collect_escaped_function_refs_exp(exp, refs),
            Statement::Return(None)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Goto(_)
            | Statement::Null => {}
            Statement::If(cond, then_stmt, else_stmt) => {
                Self::collect_escaped_function_refs_exp(cond, refs);
                Self::collect_escaped_function_refs_stmt(then_stmt, refs);
                if let Some(else_stmt) = else_stmt {
                    Self::collect_escaped_function_refs_stmt(else_stmt, refs);
                }
            }
            Statement::Block(block) => Self::collect_escaped_function_refs_block(block, refs),
            Statement::While {
                condition, body, ..
            }
            | Statement::DoWhile {
                condition, body, ..
            } => {
                Self::collect_escaped_function_refs_exp(condition, refs);
                Self::collect_escaped_function_refs_stmt(body, refs);
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                ..
            } => {
                if let ForInit::Expression(Some(exp)) = init.as_ref() {
                    Self::collect_escaped_function_refs_exp(exp, refs);
                }
                if let Some(condition) = condition {
                    Self::collect_escaped_function_refs_exp(condition, refs);
                }
                if let Some(post) = post {
                    Self::collect_escaped_function_refs_exp(post, refs);
                }
                Self::collect_escaped_function_refs_stmt(body, refs);
            }
            Statement::Label(_, body)
            | Statement::Case { body, .. }
            | Statement::Default { body, .. } => {
                Self::collect_escaped_function_refs_stmt(body, refs)
            }
            Statement::Switch { control, body, .. } => {
                Self::collect_escaped_function_refs_exp(control, refs);
                Self::collect_escaped_function_refs_stmt(body, refs);
            }
        }
    }

    fn collect_escaped_function_refs_block(block: &Block, refs: &mut HashSet<String>) {
        for item in block {
            match item {
                BlockItem::Declaration(Declaration::VarDecl(vd)) => {
                    if let Some(init) = vd.init.as_ref() {
                        Self::collect_escaped_function_refs_exp(init, refs);
                    }
                }
                BlockItem::Statement(stmt) => Self::collect_escaped_function_refs_stmt(stmt, refs),
                _ => {}
            }
        }
    }

    fn emit_function(&mut self, func: FunctionDeclaration) -> TackyResult<Option<TackyFunction>> {
        self.function_symbols.insert(func.name.clone());
        let param_types: Vec<CType> = func.params.iter().map(|(_, t, _)| *t).collect();
        self.func_types.insert(
            func.name.clone(),
            (
                func.return_type,
                param_types,
                func.return_ptr_info,
                func.variadic,
            ),
        );
        if func.old_style {
            self.old_style_functions.insert(func.name.clone());
        } else {
            self.old_style_functions.remove(&func.name);
        }
        self.record_zero_fixed_variadic_function(&func);
        self.func_param_full_types
            .insert(func.name.clone(), func.param_full_types.clone());
        if let Some(ref rft) = func.return_full_type {
            self.func_full_types.insert(func.name.clone(), rft.clone());
        }

        let Some(body) = func.body else {
            return Ok(None);
        };

        self.current_function = func.name.clone();
        self.current_function_params = func
            .params
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect();
        self.instructions.clear();
        let mut local_labels = HashSet::new();
        Self::collect_block_labels(&body, &mut local_labels);
        let saved_label_bodies = std::mem::take(&mut self.current_label_bodies);
        Self::collect_block_label_bodies(&body, &mut self.current_label_bodies);
        let saved_escaped_functions = std::mem::take(&mut self.current_escaped_functions);
        Self::collect_escaped_function_refs_block(&body, &mut self.current_escaped_functions);
        self.local_label_stack.push(local_labels);

        // Check if return type requires hidden pointer
        let ret_needs_hidden_ptr = if let Some(ref ret_ft) = func.return_full_type {
            let needs_large_struct_ptr = if let FullType::Struct(tag) = ret_ft {
                self.struct_defs
                    .get(tag)
                    .map(|d| d.size > 16)
                    .unwrap_or(false)
            } else {
                false
            };
            needs_large_struct_ptr || ret_ft.is_complex()
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
        let mut memory_param_blocks = Vec::new();
        let mut struct_param_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut complex_param_fixups: Vec<(String, usize)> = Vec::new();
        if let Some(ref ret_ptr) = hidden_ret_ptr_name {
            tacky_params.push(ret_ptr.clone());
        }
        let mut struct_param_fixups: Vec<(String, String, StructDef)> = Vec::new(); // (original_name, tag, def)
        let mut param_vla_bounds = func.param_vla_bounds.iter();
        for (i, (name, ptype, pi)) in func.params.iter().enumerate() {
            let ft = if i < func.param_full_types.len() {
                func.param_full_types[i].clone()
            } else {
                FullType::from_decl(*ptype, *pi, &None)
            };
            if let FullType::Pointer(pointee) = &ft {
                if let FullType::Array { elem, size } = pointee.as_ref() {
                    if *size == VLA_STATIC_SCALE_FALLBACK {
                        if let Some(bound) = param_vla_bounds.next() {
                            self.vla_param_bounds.insert(
                                name.clone(),
                                Exp::Binary(
                                    BinaryOp::Mul,
                                    Box::new(bound.clone()),
                                    Box::new(Exp::SizeOfType(
                                        elem.to_ctype(),
                                        elem.as_ref().clone(),
                                    )),
                                ),
                            );
                        }
                    }
                }
            }

            if let FullType::Struct(ref tag) = ft {
                if let Some(def) = self.struct_defs.get(tag).cloned() {
                    let classes = def.classify_with(&self.struct_defs);
                    if classes.len() == 1 && classes[0] == ParamClass::Memory {
                        let param_name = format!("{}_mem", name);
                        let param_idx = tacky_params.len();
                        self.var_types.insert(param_name.clone(), CType::Pointer);
                        self.symbol_types.insert(param_name.clone(), CType::Pointer);
                        tacky_params.push(param_name.clone());
                        stack_params.insert(param_name);
                        memory_param_blocks.push((param_idx, name.clone(), def.size));
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
                        if !classes.is_empty() {
                            struct_param_groups.push((group_start, classes.len(), is_sse_vec));
                        }
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
            if ft.is_complex() {
                let FullType::Vector { elem, .. } = &ft else {
                    return Err("internal error: expected complex vector type".to_string());
                };
                let elem_type = elem.to_ctype();
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let group_start = tacky_params.len();
                let flat_names = [format!("{}_eb0", name), format!("{}_eb1", name)];
                for flat_name in &flat_names {
                    self.var_types.insert(flat_name.clone(), elem_type);
                    self.symbol_types.insert(flat_name.clone(), elem_type);
                    tacky_params.push(flat_name.clone());
                }
                let is_fp = elem_type.is_floating();
                struct_param_groups.push((group_start, 2, vec![is_fp, is_fp]));
                complex_param_fixups.push((name.clone(), elem_size));
                self.register_var(name, ft.clone());
                self.array_sizes
                    .insert(name.clone(), ft.byte_size_with(&self.struct_defs));
                continue;
            }

            let storage_type = self.storage_ctype_for_full(&ft);
            self.var_types.insert(name.clone(), storage_type);
            self.symbol_types.insert(name.clone(), storage_type);
            if let Some(info) = pi {
                self.ptr_info.insert(name.clone(), *info);
            }
            if ft.is_vector() {
                self.array_sizes
                    .insert(name.clone(), ft.byte_size_with(&self.struct_defs));
            }
            self.full_types.insert(name.clone(), ft);
            tacky_params.push(name.clone());
        }

        // Reassemble struct params from eightbytes
        for (name, _tag, def) in &struct_param_fixups {
            let classes = def.classify_with(&self.struct_defs);
            if classes.len() == 1 && classes[0] == ParamClass::Memory {
                continue;
            }
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

        for (name, elem_size) in &complex_param_fixups {
            for eb_idx in 0..2 {
                let param_name = format!("{}_eb{}", name, eb_idx);
                let eb_offset = (eb_idx * *elem_size) as i64;
                self.emit(TackyInstr::CopyToOffset {
                    src: TackyVal::Var(param_name),
                    dst_name: name.clone(),
                    offset: eb_offset,
                });
            }
        }

        let emit_result = self.emit_block(body);
        self.local_label_stack.pop();
        self.current_label_bodies = saved_label_bodies;
        self.current_escaped_functions = saved_escaped_functions;
        emit_result?;
        self.emit(TackyInstr::Return(TackyVal::Constant(0)));
        self.apply_function_instrumentation(func.no_instrument_function);

        Ok(Some(TackyFunction {
            name: func.name,
            params: tacky_params,
            global: true, // overridden by linkage map in generate()
            body: std::mem::take(&mut self.instructions),
            stack_params,
            memory_param_blocks,
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
    if target == CType::LongDouble && !source_is_double {
        let d = if source_is_unsigned {
            (val as u64) as f64
        } else {
            val as f64
        };
        return d.to_bits() as i64;
    }
    if !matches!(target, CType::Double | CType::LongDouble) && source_is_double {
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
        CType::Long | CType::ULong | CType::Double | CType::LongDouble | CType::Pointer => val,
        CType::Int128 | CType::UInt128 => val,
        CType::Float => (val as f32).to_bits() as i64,
        CType::Void | CType::Struct => val,
    }
}

fn static_init_value_to_f64((val, source_is_double, source_is_unsigned): StaticScalarValue) -> f64 {
    if source_is_double {
        f64::from_bits(val as u64)
    } else if source_is_unsigned {
        val as u64 as f64
    } else {
        val as f64
    }
}

fn neg_static_init_value(
    (val, source_is_double, source_is_unsigned): StaticScalarValue,
) -> StaticScalarValue {
    if source_is_double {
        ((-f64::from_bits(val as u64)).to_bits() as i64, true, false)
    } else {
        (val.wrapping_neg(), false, source_is_unsigned)
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
            CType::Int128 => StaticInit::Int128Init(val as i128),
            CType::UInt128 => StaticInit::UInt128Init(val as u64 as u128),
            CType::Float => StaticInit::FloatInit(f32::from_bits(val as u32)),
            CType::Double => StaticInit::DoubleInit(f64::from_bits(val as u64)),
            CType::LongDouble => StaticInit::LongDoubleInit(f64::from_bits(val as u64)),
            CType::Void | CType::Struct => StaticInit::ZeroInit(0),
        }
    }
}

fn eval_static_expr_full_type(
    exp: &Exp,
    full_types: &HashMap<String, FullType>,
) -> Option<FullType> {
    match exp {
        Exp::Var(name) => full_types.get(name).cloned(),
        Exp::StringLiteral(s) => Some(FullType::Array {
            elem: Box::new(FullType::Scalar(CType::Char)),
            size: c_string_byte_len(s) + 1,
        }),
        Exp::WideStringLiteral(s) => Some(FullType::Array {
            elem: Box::new(FullType::Scalar(CType::Int)),
            size: s.chars().count() + 1,
        }),
        Exp::Cast(_, Some(ft), _) => Some(ft.clone()),
        Exp::Cast(_, None, inner) => eval_static_expr_full_type(inner, full_types),
        Exp::Unary(UnaryOp::Deref, inner) => match eval_static_expr_full_type(inner, full_types)? {
            FullType::Pointer(pointee) => Some(*pointee),
            _ => None,
        },
        Exp::Unary(UnaryOp::AddrOf, inner) => Some(FullType::Pointer(Box::new(
            eval_static_expr_full_type(inner, full_types)?,
        ))),
        Exp::Subscript(arr, _) => match eval_static_expr_full_type(arr, full_types)? {
            FullType::Array { elem, .. } => Some(*elem),
            FullType::Pointer(pointee) => Some(*pointee),
            _ => None,
        },
        _ => None,
    }
}

fn eval_static_integer_constant_exp_with_context(
    exp: &Exp,
    struct_defs: &HashMap<String, StructDef>,
    full_types: &HashMap<String, FullType>,
) -> Option<(i64, bool, bool)> {
    eval_static_integer_constant_exp_with_context_and_values(
        exp,
        struct_defs,
        full_types,
        &HashMap::new(),
    )
}

fn eval_static_integer_constant_exp_with_context_and_values(
    exp: &Exp,
    struct_defs: &HashMap<String, StructDef>,
    full_types: &HashMap<String, FullType>,
    static_const_values: &HashMap<String, (i64, bool, bool)>,
) -> Option<(i64, bool, bool)> {
    match exp {
        Exp::Constant(c) | Exp::LongConstant(c) => Some((*c, false, false)),
        Exp::UIntConstant(c) | Exp::ULongConstant(c) => Some((*c, false, true)),
        Exp::Int128Constant(c) => Some((*c as i64, false, false)),
        Exp::UInt128Constant(c) => Some((*c as i64, false, true)),
        Exp::DoubleConstant(d) | Exp::LongDoubleConstant(d) => {
            Some((d.to_bits() as i64, true, false))
        }
        Exp::Var(name) => static_const_values.get(name).copied(),
        Exp::SizeOf(inner) => {
            let ft = eval_static_expr_full_type(inner, full_types)?;
            Some((ft.byte_size_with(struct_defs) as i64, false, true))
        }
        Exp::SizeOfType(_, ft) => Some((ft.byte_size_with(struct_defs) as i64, false, true)),
        Exp::AlignOfType(ft) => Some((ft.alignment_with(struct_defs) as i64, false, true)),
        Exp::Cast(target, _, inner) => {
            if let Exp::ArrayInit(elems) = inner.as_ref() {
                let [value] = elems.as_slice() else {
                    return None;
                };
                eval_static_integer_constant_exp_with_context_and_values(
                    value,
                    struct_defs,
                    full_types,
                    static_const_values,
                )
            } else {
                let (value, is_double, is_unsigned) =
                    eval_static_integer_constant_exp_with_context_and_values(
                        inner,
                        struct_defs,
                        full_types,
                        static_const_values,
                    )?;
                if target.is_floating() {
                    let value = if is_double {
                        f64::from_bits(value as u64)
                    } else if is_unsigned {
                        value as u64 as f64
                    } else {
                        value as f64
                    };
                    let value = if *target == CType::Float {
                        value as f32 as f64
                    } else {
                        value
                    };
                    Some((value.to_bits() as i64, true, *target == CType::Float))
                } else if is_double {
                    let value = f64::from_bits(value as u64);
                    let target_unsigned = matches!(
                        target,
                        CType::Bool
                            | CType::UChar
                            | CType::UShort
                            | CType::UInt
                            | CType::ULong
                            | CType::UInt128
                    );
                    let raw = if target_unsigned {
                        value as u64 as i64
                    } else {
                        value as i64
                    };
                    Some((raw, false, target_unsigned))
                } else {
                    let target_unsigned = matches!(
                        target,
                        CType::Bool
                            | CType::UChar
                            | CType::UShort
                            | CType::UInt
                            | CType::ULong
                            | CType::UInt128
                    );
                    Some((
                        convert_init_value(value, *target, false, is_unsigned),
                        false,
                        target_unsigned,
                    ))
                }
            }
        }
        Exp::Unary(op, inner) => {
            let (value, is_double, is_unsigned) =
                eval_static_integer_constant_exp_with_context_and_values(
                    inner,
                    struct_defs,
                    full_types,
                    static_const_values,
                )?;
            match op {
                UnaryOp::Negate if is_double => {
                    let d = -f64::from_bits(value as u64);
                    Some((d.to_bits() as i64, true, false))
                }
                UnaryOp::Negate => Some((value.wrapping_neg(), false, is_unsigned)),
                UnaryOp::Complement if !is_double => Some((!value, false, is_unsigned)),
                UnaryOp::LogicalNot if !is_double => Some(((value == 0) as i64, false, false)),
                _ => None,
            }
        }
        Exp::Binary(op, left, right) => {
            let (left, left_double, left_unsigned) =
                eval_static_integer_constant_exp_with_context_and_values(
                    left,
                    struct_defs,
                    full_types,
                    static_const_values,
                )?;
            let (right, right_double, right_unsigned) =
                eval_static_integer_constant_exp_with_context_and_values(
                    right,
                    struct_defs,
                    full_types,
                    static_const_values,
                )?;
            if left_double || right_double {
                let use_float =
                    (left_unsigned || !left_double) && (right_unsigned || !right_double);
                let left = if left_double {
                    f64::from_bits(left as u64)
                } else if use_float {
                    left as f32 as f64
                } else {
                    left as f64
                };
                let right = if right_double {
                    f64::from_bits(right as u64)
                } else if use_float {
                    right as f32 as f64
                } else {
                    right as f64
                };
                return match op {
                    BinaryOp::Add => {
                        let value = if use_float {
                            (left + right) as f32 as f64
                        } else {
                            left + right
                        };
                        Some((value.to_bits() as i64, true, use_float))
                    }
                    BinaryOp::Sub => {
                        let value = if use_float {
                            (left - right) as f32 as f64
                        } else {
                            left - right
                        };
                        Some((value.to_bits() as i64, true, use_float))
                    }
                    BinaryOp::Mul => {
                        let value = if use_float {
                            (left * right) as f32 as f64
                        } else {
                            left * right
                        };
                        Some((value.to_bits() as i64, true, use_float))
                    }
                    BinaryOp::Div => {
                        let value = if use_float {
                            (left / right) as f32 as f64
                        } else {
                            left / right
                        };
                        Some((value.to_bits() as i64, true, use_float))
                    }
                    BinaryOp::LogicalAnd => {
                        Some(((left != 0.0 && right != 0.0) as i64, false, false))
                    }
                    BinaryOp::LogicalOr => {
                        Some(((left != 0.0 || right != 0.0) as i64, false, false))
                    }
                    BinaryOp::Equal => Some(((left == right) as i64, false, false)),
                    BinaryOp::NotEqual => Some(((left != right) as i64, false, false)),
                    BinaryOp::LessThan => Some(((left < right) as i64, false, false)),
                    BinaryOp::GreaterThan => Some(((left > right) as i64, false, false)),
                    BinaryOp::LessEqual => Some(((left <= right) as i64, false, false)),
                    BinaryOp::GreaterEqual => Some(((left >= right) as i64, false, false)),
                    _ => None,
                };
            }
            let is_unsigned = left_unsigned || right_unsigned;
            if is_unsigned {
                let left_u = left as u64;
                let right_u = right as u64;
                let value = match op {
                    BinaryOp::BitwiseAnd => (left_u & right_u) as i64,
                    BinaryOp::BitwiseNand => (!(left_u & right_u)) as i64,
                    BinaryOp::BitwiseOr => (left_u | right_u) as i64,
                    BinaryOp::BitwiseXor => (left_u ^ right_u) as i64,
                    BinaryOp::Equal => (left_u == right_u) as i64,
                    BinaryOp::NotEqual => (left_u != right_u) as i64,
                    BinaryOp::LessThan => (left_u < right_u) as i64,
                    BinaryOp::GreaterThan => (left_u > right_u) as i64,
                    BinaryOp::LessEqual => (left_u <= right_u) as i64,
                    BinaryOp::GreaterEqual => (left_u >= right_u) as i64,
                    _ => {
                        let value = match op {
                            BinaryOp::Add => left_u.wrapping_add(right_u),
                            BinaryOp::Sub => left_u.wrapping_sub(right_u),
                            BinaryOp::Mul => left_u.wrapping_mul(right_u),
                            BinaryOp::Div => {
                                if right_u == 0 {
                                    return None;
                                }
                                left_u / right_u
                            }
                            BinaryOp::Mod => {
                                if right_u == 0 {
                                    return None;
                                }
                                left_u % right_u
                            }
                            BinaryOp::ShiftLeft => {
                                let amount = u32::try_from(right).ok()?;
                                left_u.checked_shl(amount)?
                            }
                            BinaryOp::ShiftRight => {
                                let amount = u32::try_from(right).ok()?;
                                left_u.checked_shr(amount)?
                            }
                            BinaryOp::LogicalAnd => (left_u != 0 && right_u != 0) as u64,
                            BinaryOp::LogicalOr => (left_u != 0 || right_u != 0) as u64,
                            _ => return None,
                        };
                        value as i64
                    }
                };
                return Some((value, false, true));
            }
            let value = match op {
                BinaryOp::Add => left.wrapping_add(right),
                BinaryOp::Sub => left.wrapping_sub(right),
                BinaryOp::Mul => left.wrapping_mul(right),
                BinaryOp::Div => {
                    if right == 0 {
                        return None;
                    }
                    left.checked_div(right)?
                }
                BinaryOp::Mod => {
                    if right == 0 {
                        return None;
                    }
                    left.checked_rem(right)?
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
            let (cond, is_double, _) = eval_static_integer_constant_exp_with_context_and_values(
                cond,
                struct_defs,
                full_types,
                static_const_values,
            )?;
            if is_double {
                return None;
            }
            if cond != 0 {
                eval_static_integer_constant_exp_with_context_and_values(
                    then_exp,
                    struct_defs,
                    full_types,
                    static_const_values,
                )
            } else {
                eval_static_integer_constant_exp_with_context_and_values(
                    else_exp,
                    struct_defs,
                    full_types,
                    static_const_values,
                )
            }
        }
        _ => None,
    }
}

fn eval_static_integer_constant_exp(exp: &Exp) -> Option<(i64, bool, bool)> {
    eval_static_integer_constant_exp_with_context(exp, &HashMap::new(), &HashMap::new())
}

fn wide_string_bytes(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        for byte in (ch as u32).to_le_bytes() {
            out.push(char::from(byte));
        }
    }
    out
}

pub fn generate(program: Program) -> TackyResult<TackyProgram> {
    generate_with_options(program, false, false)
}

pub fn generate_with_options(
    program: Program,
    instrument_functions: bool,
    permissive: bool,
) -> TackyResult<TackyProgram> {
    let mut gen = TackyGen::new();
    gen.instrument_functions = instrument_functions;
    gen.permissive = permissive;
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
    let external_function_definitions: HashSet<String> = program
        .declarations
        .iter()
        .filter_map(|decl| {
            let Declaration::FunDecl(fd) = decl else {
                return None;
            };
            (fd.body.is_some()
                && !(fd.is_inline
                    && fd
                        .storage_class
                        .as_ref()
                        .is_some_and(StorageClass::is_extern)))
            .then(|| fd.name.clone())
        })
        .collect();

    // Collect function types and file-scope variable types
    for decl in &program.declarations {
        match decl {
            Declaration::FunDecl(fd) => {
                gen.function_symbols.insert(fd.name.clone());
                if fd.no_instrument_function {
                    gen.no_instrument_functions.insert(fd.name.clone());
                }
                if fd
                    .body
                    .as_ref()
                    .is_some_and(TackyGen::block_contains_va_arg_pack)
                {
                    gen.inline_va_arg_pack_functions
                        .insert(fd.name.clone(), fd.clone());
                }
                if fd.body.is_some() {
                    continue;
                }
                let param_types: Vec<CType> = fd.params.iter().map(|(_, t, _)| *t).collect();
                gen.func_types.insert(
                    fd.name.clone(),
                    (fd.return_type, param_types, fd.return_ptr_info, fd.variadic),
                );
                if fd.old_style {
                    gen.old_style_functions.insert(fd.name.clone());
                } else {
                    gen.old_style_functions.remove(&fd.name);
                }
                gen.record_zero_fixed_variadic_function(fd);
                gen.func_param_full_types
                    .insert(fd.name.clone(), fd.param_full_types.clone());
                if let Some(ref rft) = fd.return_full_type {
                    gen.func_full_types.insert(fd.name.clone(), rft.clone());
                }
            }
            Declaration::VarDecl(vd) => {
                let decl_ft = vd
                    .decl_full_type
                    .clone()
                    .unwrap_or_else(|| FullType::from_decl(vd.var_type, vd.ptr_info, &None));
                let storage_type = gen.storage_ctype_for_full(&decl_ft);
                gen.var_types.insert(vd.name.clone(), storage_type);
                gen.symbol_types.insert(vd.name.clone(), storage_type);
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
                    if dft.is_array() || dft.is_vector() {
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
                if sd.is_union && sd.transparent_union {
                    if let Some(member) = sd.members.first() {
                        gen.transparent_unions
                            .insert(sd.tag.clone(), member.member_full_type.clone());
                    }
                }
                let def = StructDef::from_declaration(sd, &gen.struct_defs)?;
                gen.struct_defs.insert(sd.tag.clone(), def);
            }
            Declaration::TypedefDecl => {}
        }
    }

    // Collect file-scope variables, merging
    let mut file_scope_vars: HashMap<String, FileScopeVarInfo> = HashMap::new();
    let mut file_scope_static_inits: HashMap<String, StaticInit> = HashMap::new();
    let mut file_scope_alignments: HashMap<String, usize> = HashMap::new();
    let mut file_scope_order: Vec<String> = Vec::new();

    for decl in &program.declarations {
        if let Declaration::VarDecl(vd) = decl {
            global_vars.insert(vd.name.clone());
            if let Some(target) = &vd.alias {
                top_level.push(TackyTopLevel::Alias {
                    name: vd.name.clone(),
                    target: target.clone(),
                });
                continue;
            }
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
            let symbolic_static_init = vd
                .init
                .as_ref()
                .and_then(|exp| gen.static_symbol_offset_integer_initializer(exp, vd.var_type));
            let init_val: Option<(i64, bool, bool)> = match &vd.init {
                Some(_) if symbolic_static_init.is_some() => None,
                Some(_)
                    if matches!(
                        vd.decl_full_type,
                        Some(FullType::Vector { complex: true, .. })
                    ) =>
                {
                    None
                }
                Some(exp)
                    if (vd.array_dims.is_some()
                        || matches!(
                            vd.decl_full_type,
                            Some(FullType::Struct(_)) | Some(FullType::Vector { .. })
                        ))
                        && TackyGen::static_aggregate_initializer(exp).is_some() =>
                {
                    None // Aggregate init handled separately
                }
                Some(Exp::StringLiteral(_) | Exp::WideStringLiteral(_)) => None, // String init handled separately
                Some(exp)
                    if vd.var_type == CType::Pointer
                        && gen.static_pointer_initializer(exp).is_some() =>
                {
                    None
                }
                Some(exp) if gen.static_pointer_diff_integer(exp).is_some() => gen
                    .static_pointer_diff_integer(exp)
                    .map(|v| (v, false, false)),
                Some(exp) => Some(
                    eval_static_integer_constant_exp_with_context_and_values(
                        exp,
                        &gen.struct_defs,
                        &gen.full_types,
                        &gen.static_const_values,
                    )
                    .ok_or_else(|| "Global initializer must be constant".to_string())?,
                ),
                None => None,
            };
            let is_global = *linkage.get(&vd.name).unwrap_or(&true);
            if let Some(alignment) = vd.alignment {
                file_scope_alignments.insert(vd.name.clone(), alignment.get());
            }
            if let Some(value) = init_val {
                gen.static_const_values.insert(vd.name.clone(), value);
            }
            if let Some(init) = symbolic_static_init {
                file_scope_static_inits.insert(vd.name.clone(), init);
            } else if init_val.is_some() {
                file_scope_static_inits.remove(&vd.name);
            }
            if let Some(entry) = file_scope_vars.get_mut(&vd.name) {
                if init_val.is_some() {
                    entry.2 = init_val;
                }
                if file_scope_static_inits.contains_key(&vd.name) {
                    entry.2 = None;
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
            if vd.alias.is_some() {
                continue;
            }
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
            if let Some(FullType::Vector { .. }) = vd.decl_full_type {
                if !vd
                    .storage_class
                    .as_ref()
                    .is_some_and(StorageClass::is_extern)
                    && !global_array_names.contains(&vd.name)
                {
                    let ft = vd.decl_full_type.clone().ok_or_else(|| {
                        format!("internal error: missing full type for {}", vd.name)
                    })?;
                    let total_bytes = ft.byte_size_with(&gen.struct_defs);
                    let align = if total_bytes >= 16 {
                        16
                    } else {
                        std::cmp::max(vd.var_type.size() as usize, 1)
                    };
                    let align = vd.alignment.map_or(align, |a| a.get().max(align));
                    let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                    gen.register_var(&vd.name, ft.clone());
                    global_array_names.insert(vd.name.clone());
                    file_scope_vars.remove(&vd.name);
                    let init_values = if let Some(init_exp) = vd.init.as_ref() {
                        gen.build_static_initializer(&ft, init_exp)?
                    } else {
                        vec![StaticInit::ZeroInit(total_bytes)]
                    };
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
            }
            // Handle uninitialized global arrays (skip extern and already-handled)
            if vd.array_dims.is_some()
                && !matches!(
                    &vd.init,
                    Some(Exp::ArrayInit(_)) | Some(Exp::StringLiteral(_))
                )
                && vd
                    .init
                    .as_ref()
                    .is_none_or(|init| TackyGen::static_aggregate_initializer(init).is_none())
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
            // Global char array initialized with string literal, including `char a[] = {"x"}`.
            if let (Some(ref dims), Some(s)) = (
                &vd.array_dims,
                vd.init
                    .as_ref()
                    .and_then(TackyGen::string_array_initializer),
            ) {
                let base_type = vd.var_type;
                if !matches!(base_type, CType::Char | CType::SChar | CType::UChar) {
                    let requested_elems: usize = dims.iter().product();
                    let total_elems = if requested_elems == 0 {
                        s.chars().count() + 1
                    } else {
                        requested_elems
                    };
                    let total_bytes = total_elems * base_type.size() as usize;
                    let align = vd.alignment.map_or(base_type.size() as usize, |a| {
                        a.get().max(base_type.size() as usize)
                    });
                    let mut init_values = Vec::new();
                    for ch in s.chars().take(total_elems) {
                        init_values.push(make_static_init(ch as i64, base_type));
                    }
                    if init_values.len() < total_elems {
                        init_values.push(make_static_init(0, base_type));
                    }
                    while init_values.len() < total_elems {
                        init_values.push(make_static_init(0, base_type));
                    }
                    let is_global = *linkage.get(&vd.name).unwrap_or(&true);
                    gen.register_var(
                        &vd.name,
                        FullType::Array {
                            elem: Box::new(FullType::Scalar(base_type)),
                            size: total_elems,
                        },
                    );
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
                        init_values,
                    }));
                    gen.array_sizes.insert(vd.name.clone(), total_bytes);
                    continue;
                }
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
            if let (None, true, Some(ptr_init)) = (
                &vd.array_dims,
                vd.var_type == CType::Pointer,
                vd.init
                    .as_ref()
                    .and_then(|init| gen.static_pointer_initializer(init)),
            ) {
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
                    init_values: vec![ptr_init],
                }));
                file_scope_vars.remove(&vd.name);
                global_array_names.insert(vd.name.clone());
                continue;
            }
            if let (Some(_), Some(init_exp)) = (
                &vd.array_dims,
                vd.init
                    .as_ref()
                    .and_then(TackyGen::static_aggregate_initializer),
            ) {
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
                if fd.is_inline
                    && fd
                        .storage_class
                        .as_ref()
                        .is_some_and(StorageClass::is_extern)
                    && (external_function_definitions.contains(&fname)
                        || fd
                            .body
                            .as_ref()
                            .is_some_and(TackyGen::block_contains_va_arg_pack))
                {
                    continue;
                }
                if let Some(mut tf) = gen.emit_function(fd)? {
                    tf.global = *linkage.get(&fname).unwrap_or(&true);
                    top_level.push(TackyTopLevel::Function(tf));
                }
                for nested in gen.nested_functions.drain(..) {
                    top_level.push(TackyTopLevel::Function(nested));
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

    for sv in gen.static_vars.drain(..) {
        global_vars.insert(sv.name.clone());
        top_level.push(TackyTopLevel::StaticVar(sv));
    }
    for sc in gen.static_constants.drain(..) {
        global_vars.insert(sc.name.clone());
        top_level.push(TackyTopLevel::StaticConstant(sc));
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
        let init_v = file_scope_static_inits
            .remove(&name)
            .unwrap_or_else(|| make_static_init(converted_init, var_type));
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
            TackyTopLevel::Alias { name, .. } => {
                global_vars.insert(name.clone());
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
    fn static_bitfield_initializer_uses_only_occupied_bytes() -> Result<(), String> {
        let program = lower(
            "struct packed { signed a:6; signed b:7; signed c:6; signed d:5; unsigned char e; };\n\
             static struct packed p = { 8, 9, 2, 4, 0x10 };\n",
        )?;
        let static_var = require_some(
            program.top_level.iter().find_map(|item| match item {
                TackyTopLevel::StaticVar(var) if var.name == "p" => Some(var),
                _ => None,
            }),
            "expected static variable",
        )?;

        assert!(matches!(
            static_var.init_values[0],
            StaticInit::UCharInit(72)
        ));
        assert!(matches!(
            static_var.init_values[1],
            StaticInit::UCharInit(66)
        ));
        assert!(matches!(
            static_var.init_values[2],
            StaticInit::UCharInit(32)
        ));
        assert!(matches!(
            static_var.init_values[3],
            StaticInit::UCharInit(0x10)
        ));
        Ok(())
    }

    #[test]
    fn global_pointer_initializer_accepts_nested_array_subscript_address() -> Result<(), String> {
        let program = lower("int a[6][9] = {}; int *c = &a[3][5];\n")?;
        let static_var = require_some(
            program.top_level.iter().find_map(|item| match item {
                TackyTopLevel::StaticVar(var) if var.name == "c" => Some(var),
                _ => None,
            }),
            "expected static pointer variable",
        )?;

        assert!(matches!(
            &static_var.init_values[0],
            StaticInit::PointerInitOffset(label, 128) if label == "a"
        ));
        Ok(())
    }

    #[test]
    fn static_initializer_accepts_local_label_difference() -> Result<(), String> {
        let program = lower(
            "void f(void) {\n\
                 static int offsets[] = { &&lab1 - &&lab0 };\n\
             lab1:\n\
             lab0:\n\
                 ;\n\
             }\n",
        )?;
        let debug = format!("{program:#?}");
        assert!(debug.contains("LabelDiffInit"));
        assert!(debug.contains("label.f.lab1"));
        assert!(debug.contains("label.f.lab0"));
        Ok(())
    }

    #[test]
    fn local_scalar_initializer_accepts_single_brace_layer() -> Result<(), String> {
        let program = lower("long f(void) { long v = { (long) f }; return v == (long) f; }\n")?;
        let debug = format!("{program:#?}");
        assert!(debug.contains("name: \"f\""));
        assert!(debug.contains("Return"));
        Ok(())
    }

    #[test]
    fn return_struct_array_element_materializes_value() -> Result<(), String> {
        let program = lower(
            "struct A { int b; };\n\
             struct A foo(void) { struct A h[2] = { {1}, {2} }; return h[1]; }\n",
        )?;
        let returned_name = require_some(
            program.top_level.iter().find_map(|item| match item {
                TackyTopLevel::Function(fun) if fun.name == "foo" => {
                    fun.body.iter().find_map(|instr| match instr {
                        TackyInstr::Return(TackyVal::Var(name))
                            if matches!(program.array_sizes.get(name), Some(4)) =>
                        {
                            Some(name.clone())
                        }
                        _ => None,
                    })
                }
                _ => None,
            }),
            "expected returned temporary",
        )?;

        assert_eq!(program.array_sizes.get(&returned_name), Some(&4));
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
                    reverse_storage_order: false,
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
                dynamic_size: None,
                init: Some(Exp::ArrayInit(vec![Exp::Binary(
                    BinaryOp::Add,
                    Box::new(Exp::ArrayInit(vec![Exp::Constant(1)])),
                    Box::new(Exp::Constant(2)),
                )])),
                storage_class: None,
                alignment: None,
                alias: None,
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
                    reverse_storage_order: false,
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
                        reverse_storage_order: false,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 4,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                        reverse_storage_order: false,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 4,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
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
                        reverse_storage_order: false,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 8,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
                    },
                    StructMember {
                        name: "c".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 16,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
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
                        reverse_storage_order: false,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Int,
                        member_full_type: FullType::Scalar(CType::Int),
                        offset: 4,
                        size: 4,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
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
                        reverse_storage_order: false,
                    },
                    StructMember {
                        name: "b".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 8,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
                    },
                    StructMember {
                        name: "c".to_string(),
                        member_type: CType::Long,
                        member_full_type: FullType::Scalar(CType::Long),
                        offset: 16,
                        size: 8,
                        bit_width: None,
                        bit_offset: 0,
                        reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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

        assert_eq!(ty, CType::Struct);
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
                    reverse_storage_order: false,
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
