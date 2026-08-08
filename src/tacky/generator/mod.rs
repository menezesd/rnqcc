use crate::diagnostic::{Phase, Warning, WarningKind};
use crate::types::*;
use std::collections::{HashMap, HashSet};

use crate::tacky::{TackyOutput, TackyResult};
use indexmap::IndexMap;

mod lower_expr;
mod lower_init;
mod static_eval;
use static_eval::*;

type BuiltinFunctionInfo = (&'static str, CType, FullType, Vec<CType>, Option<PtrInfo>);

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

struct FileScopeVarInfo {
    global: bool,
    thread_local: bool,
    init_val: Option<StaticScalarValue>,
    var_type: CType,
}

#[derive(Copy, Clone)]
struct StaticScalarValue {
    value: i64,
    source_is_double: bool,
    source_is_unsigned: bool,
}

impl StaticScalarValue {
    fn integer(value: i64) -> Self {
        Self {
            value,
            source_is_double: false,
            source_is_unsigned: false,
        }
    }

    fn double_bits(value: f64) -> Self {
        Self {
            value: value.to_bits() as i64,
            source_is_double: true,
            source_is_unsigned: false,
        }
    }
}

struct StaticComplexValue {
    real: StaticScalarValue,
    imag: StaticScalarValue,
}

#[derive(Clone)]
struct NestedCaptureSlot {
    capture: String,
    slot: String,
}

#[derive(Copy, Clone)]
struct StaticIntegerConstant {
    value: i64,
    is_double: bool,
    is_unsigned: bool,
}

impl StaticIntegerConstant {
    fn as_scalar_value(self) -> StaticScalarValue {
        StaticScalarValue {
            value: self.value,
            source_is_double: self.is_double,
            source_is_unsigned: self.is_unsigned,
        }
    }
}

fn static_integer_constant(
    value: i64,
    is_double: bool,
    is_unsigned: bool,
) -> StaticIntegerConstant {
    StaticIntegerConstant {
        value,
        is_double,
        is_unsigned,
    }
}

#[derive(Copy, Clone)]
struct StaticWideIntegerConstant {
    value: i128,
    is_unsigned: bool,
}

impl StaticWideIntegerConstant {
    fn new(value: i128, is_unsigned: bool) -> Self {
        Self { value, is_unsigned }
    }

    fn is_zero(self) -> bool {
        if self.is_unsigned {
            self.value as u128 == 0
        } else {
            self.value == 0
        }
    }
}

struct StaticAddressConstant {
    base: Option<String>,
    offset: i64,
}

struct StaticStringLiteralElementAddress {
    literal_key: String,
    offset: i64,
    elem_size: i64,
}

struct IntegerRange {
    min: i128,
    max: i128,
}

struct DirectArrayStructElem<'a> {
    tag: &'a str,
    array_len: usize,
}

struct BitBuiltinSignature {
    kind: BitBuiltinKind,
    arg_type: CType,
    width: i64,
}

struct FunctionSignature {
    return_type: FullType,
    params: Vec<FullType>,
    variadic: bool,
}

struct StaticInitPiece {
    offset: usize,
    init: StaticInit,
}

struct StaticInitBuilder {
    pieces: Vec<StaticInitPiece>,
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
        let end = offset
            .checked_add(size)
            .ok_or_else(|| "static initializer offset is too large".to_string())?;
        if let Some(pos) = self.pieces.iter().position(|piece| {
            piece.offset == offset && TackyGen::static_init_size(&piece.init) == size
        }) {
            self.pieces[pos] = StaticInitPiece { offset, init };
            return Ok(());
        }
        for piece in &self.pieces {
            let existing_end = piece
                .offset
                .checked_add(TackyGen::static_init_size(&piece.init))
                .ok_or_else(|| "static initializer offset is too large".to_string())?;
            if offset < existing_end && piece.offset < end {
                return Err("overlapping static initializer designators".to_string());
            }
        }
        self.pieces.push(StaticInitPiece { offset, init });
        Ok(())
    }

    fn required_bytes(&self) -> TackyResult<usize> {
        self.pieces.iter().try_fold(0usize, |max_end, piece| {
            piece
                .offset
                .checked_add(TackyGen::static_init_size(&piece.init))
                .map(|end| max_end.max(end))
                .ok_or_else(|| "static initializer offset is too large".to_string())
        })
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
        if let Some(pos) = self.pieces.iter().position(|piece| {
            piece.offset == offset && TackyGen::static_init_size(&piece.init) == 1
        }) {
            let current = Self::init_to_u8(&self.pieces[pos].init)
                .ok_or_else(|| "cannot merge byte static initializer".to_string())?;
            self.pieces[pos].init = StaticInit::UCharInit((current & !mask) | (value & mask));
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
            let byte_offset = offset
                .checked_add(byte_index)
                .ok_or_else(|| "static initializer offset is too large".to_string())?;
            self.put_byte_masked(byte_offset, byte_value, mask)?;
        }
        Ok(())
    }

    fn finish(mut self, total_bytes: usize) -> TackyResult<Vec<StaticInit>> {
        self.pieces.sort_by_key(|piece| piece.offset);
        let mut out = Vec::new();
        let mut cursor = 0usize;
        for piece in self.pieces {
            if piece.offset > total_bytes {
                break;
            }
            if piece.offset > cursor {
                out.push(StaticInit::ZeroInit(piece.offset - cursor));
                cursor = piece.offset;
            }
            let size = TackyGen::static_init_size(&piece.init);
            let end = cursor
                .checked_add(size)
                .ok_or_else(|| "static initializer size is too large".to_string())?;
            if end > total_bytes {
                return Err("static initializer exceeds object size".to_string());
            }
            out.push(piece.init);
            cursor = end;
        }
        if cursor < total_bytes {
            out.push(StaticInit::ZeroInit(total_bytes - cursor));
        }
        Ok(out)
    }
}

pub struct TackyGen {
    target: Target,
    tmp_counter: usize,
    label_counter: usize,
    string_counter: usize,
    instructions: Vec<TackyInstr>,
    current_function: String,
    current_function_params: Vec<String>,
    current_function_locals: HashSet<String>,
    label_address_function: Option<String>,
    local_label_stack: Vec<HashSet<String>>,
    current_label_bodies: IndexMap<String, Statement>,
    current_escaped_functions: HashSet<String>,
    /// Hidden return pointer name for functions returning large structs
    hidden_ret_ptr: Option<String>,
    static_vars: Vec<TackyStaticVar>,
    static_constants: Vec<TackyStaticConstant>,
    static_const_values: IndexMap<String, StaticScalarValue>,
    static_wide_const_values: IndexMap<String, StaticWideIntegerConstant>,
    extern_vars: Vec<String>,
    /// CType for each variable/temporary (for codegen output)
    symbol_types: IndexMap<String, CType>,
    /// Rich type info (tracks arrays, pointer targets)
    full_types: IndexMap<String, FullType>,
    /// Function types: (return_type, param_types, return_ptr_info)
    func_types: IndexMap<String, FunctionTypeInfo>,
    /// Names that are functions, even when their prototype is not visible at
    /// the current source position.
    function_symbols: HashSet<String>,
    file_scope_symbols: HashSet<String>,
    /// Function return full types
    func_full_types: IndexMap<String, FullType>,
    /// Function parameter full types.
    func_param_full_types: IndexMap<String, Vec<FullType>>,
    old_style_functions: HashSet<String>,
    zero_fixed_variadic_functions: HashSet<String>,
    vla_param_bounds: IndexMap<String, Exp>,
    dynamic_sizes: IndexMap<String, Exp>,
    /// Scalar type cache for variables and temporaries.
    var_types: IndexMap<String, CType>,
    symbol_alignments: IndexMap<String, usize>,
    ptr_info: IndexMap<String, (CType, usize)>,
    bit_precisions: IndexMap<String, u8>,
    /// Array storage sizes for stack allocation
    array_sizes: IndexMap<String, usize>,
    /// Struct definitions
    struct_defs: IndexMap<String, StructDef>,
    transparent_unions: IndexMap<String, FullType>,
    nested_functions: Vec<TackyFunction>,
    instrument_functions: bool,
    permissive: bool,
    no_instrument_functions: std::collections::HashSet<String>,
    inline_va_arg_pack_functions: IndexMap<String, FunctionDeclaration>,
    nested_capture_slots: HashMap<String, Vec<NestedCaptureSlot>>,
    current_nonlocal_label_envs: HashMap<String, String>,
    current_parent_label_env_slots: HashMap<String, String>,
    deprecated_vars: HashMap<String, Option<String>>,
    warned_deprecated_vars: HashSet<String>,
    warnings: Vec<Warning>,
}

impl TackyGen {
    pub fn new() -> Self {
        Self::new_for_target(Target::host())
    }

    pub fn new_for_target(target: Target) -> Self {
        TackyGen {
            target,
            tmp_counter: 0,
            label_counter: 0,
            string_counter: 0,
            instructions: Vec::new(),
            current_function: String::new(),
            current_function_params: Vec::new(),
            current_function_locals: HashSet::new(),
            label_address_function: None,
            local_label_stack: Vec::new(),
            current_label_bodies: IndexMap::new(),
            current_escaped_functions: HashSet::new(),
            hidden_ret_ptr: None,
            static_vars: Vec::new(),
            static_constants: Vec::new(),
            static_const_values: IndexMap::new(),
            static_wide_const_values: IndexMap::new(),
            extern_vars: Vec::new(),
            symbol_types: IndexMap::new(),
            full_types: IndexMap::new(),
            func_types: IndexMap::new(),
            function_symbols: HashSet::new(),
            file_scope_symbols: HashSet::new(),
            func_full_types: IndexMap::new(),
            func_param_full_types: IndexMap::new(),
            old_style_functions: HashSet::new(),
            zero_fixed_variadic_functions: HashSet::new(),
            vla_param_bounds: IndexMap::new(),
            dynamic_sizes: IndexMap::new(),
            var_types: IndexMap::new(),
            symbol_alignments: IndexMap::new(),
            ptr_info: IndexMap::new(),
            bit_precisions: IndexMap::new(),
            array_sizes: IndexMap::new(),
            struct_defs: IndexMap::new(),
            transparent_unions: IndexMap::new(),
            nested_functions: Vec::new(),
            instrument_functions: false,
            permissive: false,
            no_instrument_functions: std::collections::HashSet::new(),
            inline_va_arg_pack_functions: IndexMap::new(),
            nested_capture_slots: HashMap::new(),
            current_nonlocal_label_envs: HashMap::new(),
            current_parent_label_env_slots: HashMap::new(),
            deprecated_vars: HashMap::new(),
            warned_deprecated_vars: HashSet::new(),
            warnings: Vec::new(),
        }
    }

    pub fn generate_with_options(
        program: Program,
        target: Target,
        instrument_functions: bool,
        permissive: bool,
    ) -> TackyResult<TackyOutput> {
        generate_with_target_options_and_warnings(program, target, instrument_functions, permissive)
    }

    fn long_double_ctype(&self) -> CType {
        if self.target.long_double_size() > CType::Double.size() as usize {
            CType::LongDouble
        } else {
            CType::Double
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
            Exp::StringLiteral(s) => Some(s),
            Exp::ArrayInit(elems) => match elems.as_slice() {
                [Exp::StringLiteral(s)] => Some(s),
                _ => None,
            },
            _ => None,
        }
    }

    fn static_pointer_initializer(&mut self, init: &Exp) -> Option<StaticInit> {
        let address = self.static_address_constant(init)?;
        match address.base {
            Some(label) if address.offset == 0 => Some(StaticInit::PointerInit(label)),
            Some(label) => Some(StaticInit::PointerInitOffset(label, address.offset)),
            None => Some(StaticInit::LongInit(address.offset)),
        }
    }

    fn static_address_constant_candidate(init: &Exp) -> bool {
        match init {
            Exp::Var(_)
            | Exp::LabelAddress(_)
            | Exp::StringLiteral(_)
            | Exp::WideStringLiteral(_)
            | Exp::Utf16StringLiteral(_)
            | Exp::Utf32StringLiteral(_)
            | Exp::Constant(_)
            | Exp::LongConstant(_)
            | Exp::UIntConstant(_)
            | Exp::ULongConstant(_) => true,
            Exp::Cast(_, Some(_), inner) if matches!(inner.as_ref(), Exp::ArrayInit(_)) => true,
            Exp::Cast(_, _, inner)
            | Exp::Unary(UnaryOp::AddrOf, inner)
            | Exp::Unary(UnaryOp::Deref, inner) => Self::static_address_constant_candidate(inner),
            Exp::Subscript(arr, _) | Exp::Dot(arr, _) | Exp::Arrow(arr, _) => {
                Self::static_address_constant_candidate(arr)
            }
            Exp::Binary(BinaryOp::Add | BinaryOp::Sub, left, right) => {
                Self::static_address_constant_candidate(left)
                    || Self::static_address_constant_candidate(right)
            }
            _ => false,
        }
    }

    fn static_pointer_initializer_candidate(init: &Exp) -> bool {
        Self::static_address_constant_candidate(init)
    }

    fn assert_static_pointer_initializer_assignable(
        &self,
        target: &FullType,
        init: &Exp,
    ) -> TackyResult<()> {
        if let Some(src) = self.static_exp_full_type(init) {
            self.assert_assignable_exp_full_type(target, &src, init, "initializer")?;
        }
        Ok(())
    }

    fn assert_pointer_initializer_assignable(
        &self,
        target: &FullType,
        init: &Exp,
    ) -> TackyResult<()> {
        if matches!(target, FullType::Pointer(_)) {
            let src = self.typeof_exp(init);
            self.assert_assignable_exp_full_type(target, &src, init, "initializer")?;
        }
        Ok(())
    }

    fn declaration_is_pointer(vd: &VarDeclaration) -> bool {
        matches!(vd.decl_full_type, Some(FullType::Pointer(_))) || vd.var_type == CType::Pointer
    }

    fn static_symbol_offset_integer_initializer(
        &mut self,
        init: &Exp,
        ctype: CType,
    ) -> Option<StaticInit> {
        if !matches!(ctype, CType::Long | CType::ULong) {
            return None;
        }
        let address = self.static_address_constant(init)?;
        match address.base {
            Some(label) if address.offset == 0 => Some(StaticInit::PointerInit(label)),
            Some(label) => Some(StaticInit::PointerInitOffset(label, address.offset)),
            None => None,
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
                    let value = eval_static_integer_constant_exp_with_context(
                        right,
                        &self.struct_defs,
                        &self.full_types,
                    )?
                    .value;
                    return Some(diff.wrapping_add(value));
                }
                if let Some(diff) = self.static_pointer_diff_integer(right) {
                    let value = eval_static_integer_constant_exp_with_context(
                        left,
                        &self.struct_defs,
                        &self.full_types,
                    )?
                    .value;
                    return Some(value.wrapping_add(diff));
                }
                None
            }
            Exp::Binary(BinaryOp::Sub, left, right) => {
                if let Some(diff) = self.static_pointer_diff_integer(left) {
                    let value = eval_static_integer_constant_exp_with_context(
                        right,
                        &self.struct_defs,
                        &self.full_types,
                    )?
                    .value;
                    return Some(diff.wrapping_sub(value));
                }
                if let (Some(left), Some(right)) = (
                    self.static_string_literal_element_address(left),
                    self.static_string_literal_element_address(right),
                ) {
                    if left.literal_key == right.literal_key && left.elem_size == right.elem_size {
                        let byte_diff = left.offset - right.offset;
                        return (byte_diff % left.elem_size == 0)
                            .then_some(byte_diff / left.elem_size);
                    }
                }
                let left_address = self.static_address_constant(left)?;
                let right_address = self.static_address_constant(right)?;
                if left_address.base != right_address.base {
                    return None;
                }
                let elem_size = match self.static_exp_full_type(left) {
                    Some(FullType::Pointer(pointee)) => pointee
                        .checked_byte_size_with(&self.struct_defs)
                        .and_then(|size| i64::try_from(size).ok())?,
                    Some(FullType::Array { elem, .. }) => elem
                        .checked_byte_size_with(&self.struct_defs)
                        .and_then(|size| i64::try_from(size).ok())?,
                    _ => 1,
                };
                if elem_size == 0 {
                    return None;
                }
                let byte_diff = left_address.offset.checked_sub(right_address.offset)?;
                (byte_diff % elem_size == 0).then_some(byte_diff / elem_size)
            }
            _ => None,
        }
    }

    fn static_string_literal_element_address(
        &self,
        exp: &Exp,
    ) -> Option<StaticStringLiteralElementAddress> {
        let Exp::Unary(UnaryOp::AddrOf, inner) = exp else {
            return None;
        };
        let Exp::Subscript(array, index) = inner.as_ref() else {
            return None;
        };
        let index = eval_static_integer_constant_exp_with_context(
            index,
            &self.struct_defs,
            &self.full_types,
        )?
        .value;
        match array.as_ref() {
            Exp::StringLiteral(s) => Some(StaticStringLiteralElementAddress {
                literal_key: format!("c:{s}"),
                offset: index,
                elem_size: 1,
            }),
            Exp::WideStringLiteral(s) => {
                let elem_size =
                    FullType::Scalar(CType::Int).byte_size_with(&self.struct_defs) as i64;
                Some(StaticStringLiteralElementAddress {
                    literal_key: format!("w:{s}"),
                    offset: index.checked_mul(elem_size)?,
                    elem_size,
                })
            }
            Exp::Utf16StringLiteral(s) => {
                let elem_size =
                    FullType::Scalar(CType::UShort).byte_size_with(&self.struct_defs) as i64;
                Some(StaticStringLiteralElementAddress {
                    literal_key: format!("u16:{s}"),
                    offset: index.checked_mul(elem_size)?,
                    elem_size,
                })
            }
            Exp::Utf32StringLiteral(s) => {
                let elem_size =
                    FullType::Scalar(CType::UInt).byte_size_with(&self.struct_defs) as i64;
                Some(StaticStringLiteralElementAddress {
                    literal_key: format!("u32:{s}"),
                    offset: index.checked_mul(elem_size)?,
                    elem_size,
                })
            }
            _ => None,
        }
    }

    fn static_address_constant(&mut self, exp: &Exp) -> Option<StaticAddressConstant> {
        match exp {
            Exp::Var(name) => Some(StaticAddressConstant {
                base: Some(name.clone()),
                offset: 0,
            }),
            Exp::LabelAddress(label) => {
                let function = self
                    .label_address_function
                    .as_ref()
                    .unwrap_or(&self.current_function);
                Some(StaticAddressConstant {
                    base: Some(format!("label.{}.{}", function, label)),
                    offset: 0,
                })
            }
            Exp::StringLiteral(s) => Some(StaticAddressConstant {
                base: Some(self.make_string_constant(s)),
                offset: 0,
            }),
            Exp::WideStringLiteral(s) => Some(StaticAddressConstant {
                base: Some(self.make_raw_string_constant(
                    wide_string_bytes_with_null(s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::Int)),
                        size: s.chars().count() + 1,
                    },
                    FullType::Scalar(CType::Int).alignment(),
                )),
                offset: 0,
            }),
            Exp::Utf16StringLiteral(s) => Some(StaticAddressConstant {
                base: Some(self.make_raw_string_constant(
                    utf16_string_bytes_with_null(s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::UShort)),
                        size: s.encode_utf16().count() + 1,
                    },
                    FullType::Scalar(CType::UShort).alignment(),
                )),
                offset: 0,
            }),
            Exp::Utf32StringLiteral(s) => Some(StaticAddressConstant {
                base: Some(self.make_raw_string_constant(
                    utf32_string_bytes_with_null(s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::UInt)),
                        size: s.chars().count() + 1,
                    },
                    FullType::Scalar(CType::UInt).alignment(),
                )),
                offset: 0,
            }),
            Exp::Constant(value) | Exp::LongConstant(value) => Some(StaticAddressConstant {
                base: None,
                offset: *value,
            }),
            Exp::UIntConstant(value) | Exp::ULongConstant(value) => Some(StaticAddressConstant {
                base: None,
                offset: *value,
            }),
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
    ) -> Option<StaticAddressConstant> {
        let address_constant = self.static_address_constant(address)?;
        let value = eval_static_integer_constant_exp_with_context(
            constant,
            &self.struct_defs,
            &self.full_types,
        )?
        .value;
        let scale = match self.static_exp_full_type(address) {
            Some(FullType::Array { elem, .. }) => {
                i64::try_from(elem.checked_byte_size_with(&self.struct_defs)?).ok()?
            }
            Some(FullType::Pointer(pointee)) => {
                i64::try_from(pointee.checked_byte_size_with(&self.struct_defs)?).ok()?
            }
            _ => 1,
        };
        let scaled = value.checked_mul(scale)?;
        let signed_scaled = scaled.checked_mul(sign)?;
        Some(StaticAddressConstant {
            base: address_constant.base,
            offset: address_constant.offset.checked_add(signed_scaled)?,
        })
    }

    fn static_exp_full_type(&self, exp: &Exp) -> Option<FullType> {
        match exp {
            Exp::Var(name) if self.function_symbols.contains(name) => {
                self.function_designator_full_type(name)
            }
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

    fn function_designator_full_type(&self, name: &str) -> Option<FullType> {
        let (return_type, params, _return_ptr_info, variadic) = self.func_types.get(name)?;
        let return_type = self
            .func_full_types
            .get(name)
            .cloned()
            .unwrap_or(FullType::Scalar(*return_type));
        let params = self
            .func_param_full_types
            .get(name)
            .cloned()
            .unwrap_or_else(|| params.iter().copied().map(FullType::Scalar).collect());
        Some(FullType::Function {
            return_type: Box::new(return_type),
            params,
            variadic: *variadic,
        })
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

    fn static_lvalue_address_constant(&mut self, exp: &Exp) -> Option<StaticAddressConstant> {
        match exp {
            Exp::Var(name) => Some(StaticAddressConstant {
                base: Some(name.clone()),
                offset: 0,
            }),
            Exp::Cast(_, Some(ft), inner) if matches!(inner.as_ref(), Exp::ArrayInit(_)) => {
                let label = self.make_static_compound_literal(ft, inner).ok()?;
                Some(StaticAddressConstant {
                    base: Some(label),
                    offset: 0,
                })
            }
            Exp::Dot(inner, member) => {
                let address = self.static_lvalue_address_constant(inner)?;
                let tag = match self.static_exp_full_type(inner)? {
                    FullType::Struct(tag) => tag,
                    _ => return None,
                };
                let member_offset = i64::try_from(
                    self.struct_defs
                        .get(&tag)?
                        .members
                        .iter()
                        .find(|m| m.name == *member)?
                        .offset,
                )
                .ok()?;
                Some(StaticAddressConstant {
                    base: address.base,
                    offset: address.offset.checked_add(member_offset)?,
                })
            }
            Exp::Arrow(inner, member) => {
                let address = self.static_address_constant(inner)?;
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
                let member_offset = i64::try_from(
                    self.struct_defs
                        .get(&tag)?
                        .members
                        .iter()
                        .find(|m| m.name == *member)?
                        .offset,
                )
                .ok()?;
                Some(StaticAddressConstant {
                    base: address.base,
                    offset: address.offset.checked_add(member_offset)?,
                })
            }
            Exp::Subscript(arr, idx) => {
                let address = self
                    .static_address_constant(arr)
                    .or_else(|| self.static_lvalue_address_constant(arr))?;
                let elem_size = match self.static_exp_full_type(arr)? {
                    FullType::Array { elem, .. } => {
                        elem.checked_byte_size_with(&self.struct_defs)?
                    }
                    FullType::Pointer(pointee) => {
                        pointee.checked_byte_size_with(&self.struct_defs)?
                    }
                    _ => return None,
                };
                let elem_size = i64::try_from(elem_size).ok()?;
                let index = eval_static_integer_constant_exp_with_context(
                    idx,
                    &self.struct_defs,
                    &self.full_types,
                )?
                .value;
                let offset = index.checked_mul(elem_size)?;
                Some(StaticAddressConstant {
                    base: address.base,
                    offset: address.offset.checked_add(offset)?,
                })
            }
            Exp::Unary(UnaryOp::Deref, inner) => self.static_address_constant(inner),
            Exp::Cast(_, _, inner) => self.static_lvalue_address_constant(inner),
            _ => None,
        }
    }

    fn static_aggregate_initializer(init: &Exp) -> Option<&Exp> {
        match init {
            Exp::ArrayInit(_)
            | Exp::WideStringLiteral(_)
            | Exp::Utf16StringLiteral(_)
            | Exp::Utf32StringLiteral(_) => Some(init),
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
            let name = format!("__rnqcc_tmp.{}", self.tmp_counter);
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
        while off <= total_bytes.saturating_sub(8) {
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
        while off <= total_bytes.saturating_sub(4) {
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

    /// SysV passes vectors larger than two eightbytes in memory, and returns
    /// them through the same hidden-result pointer used for large aggregates.
    fn vector_requires_memory_abi(&self, ft: &FullType) -> bool {
        ft.is_vector() && !ft.is_complex() && ft.byte_size_with(&self.struct_defs) > 16
    }

    fn return_requires_hidden_pointer(&self, ft: &FullType) -> bool {
        match ft {
            FullType::Struct(tag) => self
                .struct_defs
                .get(tag)
                .map(|def| def.size > 16)
                .unwrap_or(false),
            _ => ft.is_complex() || self.vector_requires_memory_abi(ft),
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

    fn function_signature_from_full(ft: &FullType) -> Option<FunctionSignature> {
        match ft {
            FullType::Function {
                return_type,
                params,
                variadic,
            } => Some(FunctionSignature {
                return_type: return_type.as_ref().clone(),
                params: params.clone(),
                variadic: *variadic,
            }),
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

    fn builtin_function_info(&self, name: &str) -> Option<BuiltinFunctionInfo> {
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
            "__builtin_sqrtl" => {
                let long_double = self.long_double_ctype();
                Some((
                    "sqrtl",
                    long_double,
                    FullType::Scalar(long_double),
                    vec![long_double],
                    None,
                ))
            }
            "__builtin_atan2l" => {
                let long_double = self.long_double_ctype();
                Some((
                    "atan2l",
                    long_double,
                    FullType::Scalar(long_double),
                    vec![long_double, long_double],
                    None,
                ))
            }
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
                vec![self.long_double_ctype()],
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

    fn is_void_full_type(ft: &FullType) -> bool {
        matches!(ft, FullType::Scalar(CType::Void))
    }

    fn should_warn_compare_distinct_pointer_types(
        op: &BinaryOp,
        left: &FullType,
        right: &FullType,
    ) -> bool {
        let (FullType::Pointer(left_inner), FullType::Pointer(right_inner)) = (left, right) else {
            return false;
        };
        if left_inner == right_inner {
            return false;
        }
        if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual)
            && (Self::is_void_full_type(left_inner) || Self::is_void_full_type(right_inner))
        {
            return false;
        }
        true
    }

    fn warn_compare_distinct_pointer_types(&mut self) {
        self.warnings.push(Warning {
            phase: Phase::Tacky,
            kind: WarningKind::CompareDistinctPointerTypes,
            message: "comparison of distinct pointer types".to_string(),
            span: None,
        });
    }

    fn warn_deprecated_declaration(&mut self, name: &str, message: Option<String>) {
        let display_name = name
            .rsplit_once('.')
            .filter(|(_, suffix)| suffix.chars().all(|ch| ch.is_ascii_digit()))
            .map(|(base, _)| base)
            .unwrap_or(name);
        let kind = WarningKind::DeprecatedDeclaration {
            name: display_name.to_string(),
            message,
        };
        let message = Warning::resolve(kind.clone()).message;
        self.warnings.push(Warning {
            phase: Phase::Tacky,
            kind,
            message,
            span: None,
        });
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
                    && (*dst_variadic == *src_variadic
                        && (dst_params == src_params
                            || self.canonical_param_full_types(dst_params)
                                == self.canonical_param_full_types(src_params))
                        || Self::has_unspecified_function_params(dst_params, *dst_variadic)
                        || Self::has_unspecified_function_params(src_params, *src_variadic))
            }
            (FullType::Pointer(dst_inner), FullType::Pointer(src_inner)) => {
                Self::is_void_pointer(dst)
                    || Self::is_void_pointer(src)
                    || matches!(
                        src_inner.as_ref(),
                        FullType::Array { elem, .. }
                            if self.compatible_pointer_pointees(dst_inner, elem)
                    )
                    || self.compatible_pointer_pointees(dst_inner, src_inner)
            }
            (FullType::Pointer(pointee), FullType::Array { elem, .. }) => {
                Self::is_void_pointer(dst) || self.compatible_pointer_pointees(pointee, elem)
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
            FullType::Pointer(_)
                | FullType::Array { .. }
                | FullType::Function { .. }
                | FullType::Scalar(CType::Pointer)
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
    fn ptr_elem_size(&self, name: &str) -> TackyResult<i64> {
        if let Some(ft) = self.full_types.get(name) {
            match ft {
                FullType::Pointer(inner) => inner
                    .checked_byte_size_with(&self.struct_defs)
                    .and_then(|size| i64::try_from(size).ok())
                    .ok_or_else(|| "pointer element size is too large".to_string()),
                _ => Ok(self.deref_type(name).size() as i64),
            }
        } else {
            Ok(self.deref_type(name).size() as i64)
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
            Exp::LongDoubleConstant(_) => FullType::Scalar(self.long_double_ctype()),
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
            Exp::Utf16StringLiteral(s) => FullType::Array {
                elem: Box::new(FullType::Scalar(CType::UShort)),
                size: s.encode_utf16().count() + 1,
            },
            Exp::Utf32StringLiteral(s) => FullType::Array {
                elem: Box::new(FullType::Scalar(CType::UInt)),
                size: s.chars().count() + 1,
            },
            Exp::Var(name) if self.function_symbols.contains(name) => self
                .function_designator_full_type(name)
                .unwrap_or(FullType::Scalar(CType::Int)),
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
            Exp::FunctionCall(name, _) | Exp::ImplicitFunctionCall(name, _) => {
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
                        FullType::Scalar(ct) => FullType::Scalar(ct.promote()),
                        _ => ft,
                    }
                } else {
                    let l = self.typeof_exp(left);
                    let r = self.typeof_exp(right);
                    if matches!(op, BinaryOp::Add | BinaryOp::Sub) {
                        if matches!(l, FullType::Pointer(_)) && matches!(r, FullType::Pointer(_)) {
                            return FullType::Scalar(CType::Long);
                        }
                        if matches!(l, FullType::Pointer(_)) {
                            return l;
                        }
                        if matches!(r, FullType::Pointer(_)) && matches!(op, BinaryOp::Add) {
                            return r;
                        }
                    }
                    match (&l, &r) {
                        (FullType::Scalar(lt), FullType::Scalar(rt)) => {
                            FullType::Scalar(CType::common(*lt, *rt))
                        }
                        _ if l == r => l,
                        _ if l.byte_size_with(&self.struct_defs)
                            >= r.byte_size_with(&self.struct_defs) =>
                        {
                            l
                        }
                        _ => r,
                    }
                }
            }
            Exp::Assign(left, _) => self.typeof_exp(left),
            Exp::CompoundAssign(_, left, _) => self.typeof_exp(left),
            Exp::Conditional(_, then_e, else_e) => {
                let t = self.typeof_exp(then_e);
                let e = self.typeof_exp(else_e);
                match (&t, &e) {
                    _ if t == e => t,
                    (FullType::Scalar(tt), FullType::Scalar(et)) => {
                        FullType::Scalar(CType::common(*tt, *et))
                    }
                    _ if t.byte_size_with(&self.struct_defs)
                        >= e.byte_size_with(&self.struct_defs) =>
                    {
                        t
                    }
                    _ => e,
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

    fn make_raw_string_constant(
        &mut self,
        bytes: String,
        ft: FullType,
        alignment: usize,
    ) -> String {
        let label = format!("__string_const_{}", self.string_counter);
        self.string_counter += 1;
        self.register_var(&label, ft);
        self.static_constants.push(TackyStaticConstant {
            name: label.clone(),
            alignment,
            init: StaticInit::StringInit(bytes, false),
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

    fn integer_range_for_type(ty: CType) -> Option<IntegerRange> {
        match ty {
            CType::Bool => Some(IntegerRange { min: 0, max: 1 }),
            CType::Char | CType::SChar => Some(IntegerRange {
                min: i8::MIN as i128,
                max: i8::MAX as i128,
            }),
            CType::Short => Some(IntegerRange {
                min: i16::MIN as i128,
                max: i16::MAX as i128,
            }),
            CType::Int => Some(IntegerRange {
                min: i32::MIN as i128,
                max: i32::MAX as i128,
            }),
            CType::Long => Some(IntegerRange {
                min: i64::MIN as i128,
                max: i64::MAX as i128,
            }),
            CType::UChar => Some(IntegerRange {
                min: 0,
                max: u8::MAX as i128,
            }),
            CType::UShort => Some(IntegerRange {
                min: 0,
                max: u16::MAX as i128,
            }),
            CType::UInt => Some(IntegerRange {
                min: 0,
                max: u32::MAX as i128,
            }),
            CType::ULong => Some(IntegerRange {
                min: 0,
                max: u64::MAX as i128,
            }),
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
        if matches!(from, CType::Double | CType::Float)
            && matches!(to, CType::Int128 | CType::UInt128)
        {
            let helper = match (from, to.is_signed()) {
                (CType::Double, true) => "__fixdfti",
                (CType::Double, false) => "__fixunsdfti",
                (CType::Float, true) => "__fixsfti",
                (CType::Float, false) => "__fixunssfti",
                _ => unreachable!(),
            };
            self.emit(TackyInstr::FunCall {
                name: helper.to_string(),
                args: vec![val],
                dst: dst.clone(),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 1,
                hidden_return: false,
                indirect: false,
            });
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

    fn switch_case_constant(&mut self, value: SwitchCaseValue, target_type: CType) -> TackyVal {
        let src_type = value.ctype;
        let src = match src_type {
            CType::Int128 => TackyVal::Int128Constant(value.value),
            CType::UInt128 => TackyVal::UInt128Constant(value.value as u128),
            CType::UInt | CType::ULong => TackyVal::Constant(value.value as u64 as i64),
            _ => TackyVal::Constant(value.value as i64),
        };
        if src_type == target_type {
            let dst = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Copy {
                src,
                dst: dst.clone(),
            });
            return dst;
        }
        self.convert_to(src, src_type, target_type)
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
                let ty = self.long_double_ctype();
                let dst = self.fresh_tmp(ty);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(val),
                    dst: dst.clone(),
                });
                Ok((dst, ty))
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
                let size = ft
                    .checked_byte_size_with(&self.struct_defs)
                    .and_then(|size| i64::try_from(size).ok())
                    .ok_or_else(|| "sizeof operand is too large".to_string())?;
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(size),
                    dst: dst.clone(),
                });
                Ok((dst, CType::ULong))
            }
            Exp::AlignOfType(ft) => {
                let alignment = i64::try_from(ft.alignment_with(&self.struct_defs))
                    .map_err(|_| "alignment operand is too large".to_string())?;
                let dst = self.fresh_tmp(CType::ULong);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(alignment),
                    dst: dst.clone(),
                });
                Ok((dst, CType::ULong))
            }
            Exp::SizeOf(inner) => {
                let size = i64::try_from(self.sizeof_exp(&inner))
                    .map_err(|_| "sizeof operand is too large".to_string())?;
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
                let label = self.make_raw_string_constant(
                    wide_string_bytes_with_null(&s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::Int)),
                        size: s.chars().count() + 1,
                    },
                    FullType::Scalar(CType::Int).alignment(),
                );
                let decayed_ft = FullType::Pointer(Box::new(FullType::Scalar(CType::Int)));
                let ptr = self.fresh_tmp_full(&decayed_ft);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(label),
                    dst: ptr.clone(),
                });
                Ok((ptr, CType::Pointer))
            }
            Exp::Utf16StringLiteral(s) => {
                let label = self.make_raw_string_constant(
                    utf16_string_bytes_with_null(&s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::UShort)),
                        size: s.encode_utf16().count() + 1,
                    },
                    FullType::Scalar(CType::UShort).alignment(),
                );
                let decayed_ft = FullType::Pointer(Box::new(FullType::Scalar(CType::UShort)));
                let ptr = self.fresh_tmp_full(&decayed_ft);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(label),
                    dst: ptr.clone(),
                });
                Ok((ptr, CType::Pointer))
            }
            Exp::Utf32StringLiteral(s) => {
                let label = self.make_raw_string_constant(
                    utf32_string_bytes_with_null(&s),
                    FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::UInt)),
                        size: s.chars().count() + 1,
                    },
                    FullType::Scalar(CType::UInt).alignment(),
                );
                let decayed_ft = FullType::Pointer(Box::new(FullType::Scalar(CType::UInt)));
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
                            Self::checked_offset(0, mem.offset, "member initializer")?,
                        )?;
                        return Ok((rhs, mem.member_type));
                    }
                    let rhs_conv = self.convert_to(rhs, rhs_type, mem.member_type);
                    self.emit(TackyInstr::CopyToOffset {
                        src: rhs_conv.clone(),
                        dst_name: struct_name,
                        offset: Self::checked_offset(0, mem.offset, "member initializer")?,
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
                let mem_offset = i64::try_from(mem.offset)
                    .map_err(|_| "struct member offset is too large".to_string())?;
                let mem_ft = mem.member_full_type.clone();
                let mem_ptr = self.fresh_tmp(CType::Pointer);
                if mem_offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ptr_val,
                        right: TackyVal::Constant(mem_offset),
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
                        FullType::Pointer(inner) => inner
                            .checked_byte_size_with(&self.struct_defs)
                            .and_then(|size| i64::try_from(size).ok())
                            .ok_or_else(|| "pointer element size is too large".to_string())?,
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
                        FullType::Pointer(inner) => inner
                            .checked_byte_size_with(&self.struct_defs)
                            .and_then(|size| i64::try_from(size).ok())
                            .ok_or_else(|| "pointer element size is too large".to_string())?,
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
                        self.ptr_elem_size(n)?
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
                                right: TackyVal::Constant(Self::checked_offset(
                                    0,
                                    mem.offset,
                                    "member address",
                                )?),
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
                        offset: Self::checked_offset(0, mem.offset, "member access")?,
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
            Exp::FunctionCall(name, args) => self.emit_function_call(name, args, false),
            Exp::ImplicitFunctionCall(name, args) => self.emit_function_call(name, args, true),
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
        if let Some(message) = self.deprecated_vars.get(&name).cloned() {
            if self.warned_deprecated_vars.insert(name.clone()) {
                self.warn_deprecated_declaration(&name, message);
            }
        }
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
                            offset: Self::checked_offset(0, elem_size, "complex cast")?,
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
            let total_bytes = ft
                .checked_byte_size_with(&self.struct_defs)
                .ok_or_else(|| "vector initializer size is too large".to_string())?;
            if let TackyVal::Var(ref name) = result {
                self.zero_init_local(name, total_bytes);
            }
            if let Exp::ArrayInit(elems) = inner {
                let elem_ft = match &ft {
                    FullType::Vector { elem, .. } => elem.as_ref().clone(),
                    _ => FullType::Scalar(target_type),
                };
                let elem_type = elem_ft.to_ctype();
                let elem_size = elem_ft
                    .checked_byte_size_with(&self.struct_defs)
                    .ok_or_else(|| "vector element size is too large".to_string())?;
                if let TackyVal::Var(ref name) = result {
                    if ft.is_complex() && elems.len() == 1 {
                        let Some(elem) = elems.into_iter().next() else {
                            return Ok((result, target_type));
                        };
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
                        let offset = Self::checked_offset(
                            0,
                            index.checked_mul(elem_size).ok_or_else(|| {
                                "vector initializer offset is too large".to_string()
                            })?,
                            "vector initializer",
                        )?;
                        self.emit(TackyInstr::CopyToOffset {
                            src: converted,
                            dst_name: name.clone(),
                            offset,
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
            let elem_sizes = Self::compute_elem_sizes(&ft, &self.struct_defs)?;
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

    fn bit_builtin_signature(name: &str) -> Option<BitBuiltinSignature> {
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
        } else {
            let suffix = name.strip_prefix("__builtin_parity")?;
            (BitBuiltinKind::Parity, suffix)
        };

        match suffix {
            "" => Some(BitBuiltinSignature {
                kind,
                arg_type: CType::UInt,
                width: 32,
            }),
            "l" | "ll" => Some(BitBuiltinSignature {
                kind,
                arg_type: CType::ULong,
                width: 64,
            }),
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
            Exp::Var(name) | Exp::FunctionCall(name, _) | Exp::ImplicitFunctionCall(name, _) => {
                Some(name.clone())
            }
            Exp::Cast(_, _, inner) => Self::builtin_apply_target_name(inner),
            Exp::Unary(UnaryOp::AddrOf, inner) => Self::builtin_apply_target_name(inner),
            _ => None,
        }
    }

    fn exp_contains_va_arg_pack(exp: &Exp) -> bool {
        match exp {
            Exp::FunctionCall(name, args) | Exp::ImplicitFunctionCall(name, args) => {
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
            | Exp::Utf16StringLiteral(_)
            | Exp::Utf32StringLiteral(_)
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
            Exp::ImplicitFunctionCall(name, args) => Exp::ImplicitFunctionCall(
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
            Exp::FunctionCall(name, args) | Exp::ImplicitFunctionCall(name, args) => {
                if name == "__builtin_va_arg_pack" && args.is_empty() {
                    return None;
                }
                let mut substituted = Vec::new();
                for arg in args {
                    if matches!(arg, Exp::FunctionCall(inner, inner_args) | Exp::ImplicitFunctionCall(inner, inner_args)
                        if inner == "__builtin_va_arg_pack" && inner_args.is_empty())
                    {
                        substituted.extend(tail_args.iter().cloned());
                    } else {
                        substituted.push(Self::substitute_va_arg_pack_exp(arg, params, tail_args)?);
                    }
                }
                Some(match exp {
                    Exp::FunctionCall(_, _) => Exp::FunctionCall(name.clone(), substituted),
                    Exp::ImplicitFunctionCall(_, _) => {
                        Exp::ImplicitFunctionCall(name.clone(), substituted)
                    }
                    _ => unreachable!(),
                })
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
}

impl Default for TackyGen {
    fn default() -> Self {
        Self::new()
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

fn push_raw_byte(out: &mut String, byte: u8) {
    out.push(char::from(byte));
}

fn wide_string_bytes_with_null(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_mul(4).saturating_add(4));
    for ch in s.chars() {
        for byte in (ch as u32).to_le_bytes() {
            push_raw_byte(&mut out, byte);
        }
    }
    for byte in 0u32.to_le_bytes() {
        push_raw_byte(&mut out, byte);
    }
    out
}

fn utf16_string_bytes_with_null(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_mul(2).saturating_add(2));
    for unit in s.encode_utf16() {
        for byte in unit.to_le_bytes() {
            push_raw_byte(&mut out, byte);
        }
    }
    for byte in 0u16.to_le_bytes() {
        push_raw_byte(&mut out, byte);
    }
    out
}

fn utf32_string_bytes_with_null(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_mul(4).saturating_add(4));
    for ch in s.chars() {
        for byte in (ch as u32).to_le_bytes() {
            push_raw_byte(&mut out, byte);
        }
    }
    for byte in 0u32.to_le_bytes() {
        push_raw_byte(&mut out, byte);
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
    generate_with_target_options(program, Target::host(), instrument_functions, permissive)
}

pub fn generate_with_target_options(
    program: Program,
    target: Target,
    instrument_functions: bool,
    permissive: bool,
) -> TackyResult<TackyProgram> {
    generate_with_target_options_and_warnings(program, target, instrument_functions, permissive)
        .map(|output| output.program)
}

pub fn generate_with_target_options_and_warnings(
    program: Program,
    target: Target,
    instrument_functions: bool,
    permissive: bool,
) -> TackyResult<TackyOutput> {
    let mut gen = TackyGen::new_for_target(target);
    gen.instrument_functions = instrument_functions;
    gen.permissive = permissive;
    let declaration_count = program.declarations.len();
    let mut top_level = Vec::with_capacity(declaration_count);
    let mut global_vars = std::collections::HashSet::with_capacity(declaration_count);
    let mut thread_local_vars = std::collections::HashSet::with_capacity(declaration_count);

    use std::collections::HashMap;

    // Determine linkage
    let mut linkage: HashMap<String, bool> = HashMap::with_capacity(declaration_count);
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
                if fd.body.is_some() {
                    continue;
                }
            }
            Declaration::VarDecl(vd) => {
                gen.file_scope_symbols.insert(vd.name.clone());
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
                if sd.members.is_empty() && gen.struct_defs.contains_key(&sd.tag) {
                    continue;
                }
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
            let init_val: Option<StaticScalarValue> = match &vd.init {
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
                Some(
                    Exp::StringLiteral(_)
                    | Exp::WideStringLiteral(_)
                    | Exp::Utf16StringLiteral(_)
                    | Exp::Utf32StringLiteral(_),
                ) => None, // String init handled separately
                Some(exp)
                    if TackyGen::declaration_is_pointer(vd)
                        && TackyGen::static_pointer_initializer_candidate(exp) =>
                {
                    None
                }
                Some(exp) if gen.static_pointer_diff_integer(exp).is_some() => gen
                    .static_pointer_diff_integer(exp)
                    .map(StaticScalarValue::integer),
                Some(_) if matches!(vd.var_type, CType::Int128 | CType::UInt128) => None,
                Some(exp) => Some(
                    gen.eval_static_scalar_init_for_type(&Some(exp.clone()), vd.var_type)
                        .map_err(|_| "Global initializer must be constant".to_string())?,
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
            } else if matches!(vd.var_type, CType::Int128 | CType::UInt128) {
                if let Some(exp) = vd.init.as_ref() {
                    let value = eval_static_wide_integer_constant_exp_with_context_and_values(
                        exp,
                        &gen.struct_defs,
                        &gen.full_types,
                        &gen.static_const_values,
                        &gen.static_wide_const_values,
                    )
                    .ok_or_else(|| "Global initializer must be constant".to_string())?;
                    let value = cast_static_wide_integer(value, vd.var_type);
                    gen.static_wide_const_values.insert(vd.name.clone(), value);
                    file_scope_static_inits.insert(
                        vd.name.clone(),
                        make_static_wide_integer_init(value, vd.var_type),
                    );
                }
            } else if init_val.is_some() {
                file_scope_static_inits.remove(&vd.name);
            }
            if let Some(entry) = file_scope_vars.get_mut(&vd.name) {
                if init_val.is_some() {
                    entry.init_val = init_val;
                }
                if file_scope_static_inits.contains_key(&vd.name) {
                    entry.init_val = None;
                }
                entry.thread_local |= is_thread_local;
            } else {
                file_scope_vars.insert(
                    vd.name.clone(),
                    FileScopeVarInfo {
                        global: is_global,
                        thread_local: is_thread_local,
                        init_val,
                        var_type: vd.var_type,
                    },
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
                    let requested_elems = dims
                        .iter()
                        .try_fold(1usize, |count, dimension| count.checked_mul(*dimension))
                        .ok_or_else(|| format!("array '{}' is too large", vd.name))?;
                    let total_elems = if requested_elems == 0 {
                        s.chars().count() + 1
                    } else {
                        requested_elems
                    };
                    let total_bytes = total_elems
                        .checked_mul(base_type.size() as usize)
                        .ok_or_else(|| format!("array '{}' is too large", vd.name))?;
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
                let total_elems = dims
                    .iter()
                    .try_fold(1usize, |count, dimension| count.checked_mul(*dimension))
                    .ok_or_else(|| format!("array '{}' is too large", vd.name))?;
                let total_bytes = total_elems
                    .checked_mul(base_type.size() as usize)
                    .ok_or_else(|| format!("array '{}' is too large", vd.name))?;
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
            if let (None, Some(init @ Exp::StringLiteral(ref s))) = (&vd.array_dims, &vd.init) {
                let target_ft = vd
                    .decl_full_type
                    .clone()
                    .unwrap_or_else(|| FullType::from_decl(vd.var_type, vd.ptr_info, &None));
                gen.assert_static_pointer_initializer_assignable(&target_ft, init)?;
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
            if let (None, true, Some(init)) = (
                &vd.array_dims,
                TackyGen::declaration_is_pointer(vd),
                vd.init.as_ref(),
            ) {
                let target_ft = vd
                    .decl_full_type
                    .clone()
                    .unwrap_or_else(|| FullType::from_decl(vd.var_type, vd.ptr_info, &None));
                gen.assert_static_pointer_initializer_assignable(&target_ft, init)?;
                if let Some(ptr_init) = gen.static_pointer_initializer(init) {
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
                let initialized_bytes = init_values.iter().try_fold(0usize, |size, init| {
                    size.checked_add(TackyGen::static_init_size(init))
                        .ok_or_else(|| "static initializer size is too large".to_string())
                })?;
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
        let Some(info) = file_scope_vars.remove(&name) else {
            continue;
        };
        let raw_init = info
            .init_val
            .unwrap_or_else(|| StaticScalarValue::integer(0));
        let converted_init = convert_init_value(raw_init, info.var_type);
        let align = if info.var_type == CType::Double {
            16
        } else {
            info.var_type.size() as usize
        };
        let align = file_scope_alignments
            .remove(&name)
            .map_or(align, |a| a.max(align));
        let init_v = file_scope_static_inits
            .remove(&name)
            .unwrap_or_else(|| make_static_init(converted_init, info.var_type));
        top_level.push(TackyTopLevel::StaticVar(TackyStaticVar {
            name,
            global: info.global,
            thread_local: info.thread_local,
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

    Ok(TackyOutput {
        program: TackyProgram {
            top_level,
            global_vars,
            thread_local_vars,
            symbol_types: gen.symbol_types,
            symbol_alignments: gen.symbol_alignments,
            array_sizes: gen.array_sizes,
            struct_defs: gen.struct_defs,
            var_struct_tags,
        },
        warnings: gen.warnings,
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

    fn uint32_zero_minus_one_exp() -> Exp {
        Exp::Binary(
            BinaryOp::Sub,
            Box::new(Exp::Cast(CType::UInt, None, Box::new(Exp::UIntConstant(0)))),
            Box::new(Exp::UIntConstant(1)),
        )
    }

    #[test]
    fn static_integer_eval_preserves_uint32_wrap() -> Result<(), String> {
        let constant = require_some(
            eval_static_integer_constant_exp(&uint32_zero_minus_one_exp()),
            "expected constant fold",
        )?;

        assert_eq!(constant.value, 4_294_967_295);
        assert!(constant.is_unsigned);
        Ok(())
    }

    #[test]
    fn static_integer_eval_unsigned_comparison_returns_int() -> Result<(), String> {
        let constant = require_some(
            eval_static_integer_constant_exp(&Exp::Binary(
                BinaryOp::Equal,
                Box::new(uint32_zero_minus_one_exp()),
                Box::new(Exp::UIntConstant(4_294_967_295)),
            )),
            "expected constant fold",
        )?;

        assert_eq!(constant.value, 1);
        assert!(!constant.is_unsigned);
        Ok(())
    }

    #[test]
    fn static_integer_eval_reports_comparison_expression_type_as_int() -> Result<(), String> {
        let ctype = require_some(
            eval_static_integer_expr_ctype(
                &Exp::Binary(
                    BinaryOp::GreaterThan,
                    Box::new(uint32_zero_minus_one_exp()),
                    Box::new(Exp::UIntConstant(4_294_967_294)),
                ),
                &IndexMap::new(),
            ),
            "expected expression type",
        )?;

        assert_eq!(ctype, CType::Int);
        Ok(())
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                    flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                        flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
                    flexible_array: false,
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
