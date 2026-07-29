use crate::types::*;
use std::collections::HashMap;

macro_rules! define_eval_binary {
    ($name:ident, $ty:ty, $wrapping_add:ident, $wrapping_sub:ident, $wrapping_mul:ident, $wrapping_div:ident, $wrapping_rem:ident, $wrapping_shl:ident, $wrapping_shr:ident) => {
        fn $name(op: &TackyBinaryOp, l: $ty, r: $ty) -> Option<$ty> {
            match op {
                TackyBinaryOp::Add => Some(l.$wrapping_add(r)),
                TackyBinaryOp::Sub => Some(l.$wrapping_sub(r)),
                TackyBinaryOp::Mul => Some(l.$wrapping_mul(r)),
                TackyBinaryOp::Div => {
                    if r == 0 {
                        None
                    } else {
                        Some(l.$wrapping_div(r))
                    }
                }
                TackyBinaryOp::Mod => {
                    if r == 0 {
                        None
                    } else {
                        Some(l.$wrapping_rem(r))
                    }
                }
                TackyBinaryOp::BitwiseAnd => Some(l & r),
                TackyBinaryOp::BitwiseNand => Some(!(l & r)),
                TackyBinaryOp::BitwiseOr => Some(l | r),
                TackyBinaryOp::BitwiseXor => Some(l ^ r),
                TackyBinaryOp::ShiftLeft => Some(l.$wrapping_shl(r as u32)),
                TackyBinaryOp::ShiftRight => Some(l.$wrapping_shr(r as u32)),
                TackyBinaryOp::Equal => Some(if l == r { 1 } else { 0 }),
                TackyBinaryOp::NotEqual => Some(if l != r { 1 } else { 0 }),
                TackyBinaryOp::LessThan => Some(if l < r { 1 } else { 0 }),
                TackyBinaryOp::GreaterThan => Some(if l > r { 1 } else { 0 }),
                TackyBinaryOp::LessEqual => Some(if l <= r { 1 } else { 0 }),
                TackyBinaryOp::GreaterEqual => Some(if l >= r { 1 } else { 0 }),
            }
        }
    };
}

define_eval_binary!(
    eval_binary_i32,
    i32,
    wrapping_add,
    wrapping_sub,
    wrapping_mul,
    wrapping_div,
    wrapping_rem,
    wrapping_shl,
    wrapping_shr
);
define_eval_binary!(
    eval_binary_u32,
    u32,
    wrapping_add,
    wrapping_sub,
    wrapping_mul,
    wrapping_div,
    wrapping_rem,
    wrapping_shl,
    wrapping_shr
);
define_eval_binary!(
    eval_binary_u64,
    u64,
    wrapping_add,
    wrapping_sub,
    wrapping_mul,
    wrapping_div,
    wrapping_rem,
    wrapping_shl,
    wrapping_shr
);
define_eval_binary!(
    eval_binary_i128,
    i128,
    wrapping_add,
    wrapping_sub,
    wrapping_mul,
    wrapping_div,
    wrapping_rem,
    wrapping_shl,
    wrapping_shr
);
define_eval_binary!(
    eval_binary_u128,
    u128,
    wrapping_add,
    wrapping_sub,
    wrapping_mul,
    wrapping_div,
    wrapping_rem,
    wrapping_shl,
    wrapping_shr
);

fn eval_binary(op: &TackyBinaryOp, l: i64, r: i64) -> Option<i64> {
    eval_binary_i64(op, l, r)
}

define_eval_binary!(
    eval_binary_i64,
    i64,
    wrapping_add,
    wrapping_sub,
    wrapping_mul,
    wrapping_div,
    wrapping_rem,
    wrapping_shl,
    wrapping_shr
);

macro_rules! define_eval_unary {
    ($name:ident, $ty:ty) => {
        fn $name(op: &TackyUnaryOp, v: $ty) -> Option<$ty> {
            match op {
                TackyUnaryOp::Negate => Some(v.wrapping_neg()),
                TackyUnaryOp::Complement => Some(!v),
                TackyUnaryOp::LogicalNot => Some(if v == 0 { 1 } else { 0 }),
            }
        }
    };
}

define_eval_unary!(eval_unary_i128, i128);
define_eval_unary!(eval_unary_u128, u128);
define_eval_unary!(eval_unary_i64, i64);

fn eval_unary(op: &TackyUnaryOp, v: i64) -> Option<i64> {
    eval_unary_i64(op, v)
}

#[derive(Clone)]
struct KnownConstant {
    value: TackyVal,
    value_type: CType,
}

pub(super) fn constant_folding(
    instructions: Vec<TackyInstr>,
    types: &indexmap::IndexMap<String, CType>,
) -> (Vec<TackyInstr>, bool) {
    let mut const_map: HashMap<String, KnownConstant> = HashMap::new();
    let mut changed = false;

    let folded = instructions
        .into_iter()
        .map(|instr| {
            if let TackyInstr::Label(_) = &instr {
                const_map.clear();
                return instr;
            }
            if is_constant_folding_barrier(&instr) {
                const_map.clear();
            }

            let src_type_hint = match &instr {
                TackyInstr::Binary { left, right, .. } => {
                    let lt = resolve_val_type(left, &const_map, types);
                    let rt = resolve_val_type(right, &const_map, types);
                    Some(CType::common(lt, rt))
                }
                TackyInstr::Truncate { src, .. }
                | TackyInstr::SignExtend { src, .. }
                | TackyInstr::ZeroExtend { src, .. }
                | TackyInstr::DoubleToInt { src, .. }
                | TackyInstr::FloatToInt { src, .. }
                | TackyInstr::DoubleToUInt { src, .. }
                | TackyInstr::FloatToUInt { src, .. }
                | TackyInstr::IntToDouble { src, .. }
                | TackyInstr::IntToFloat { src, .. }
                | TackyInstr::UIntToDouble { src, .. }
                | TackyInstr::UIntToFloat { src, .. }
                | TackyInstr::FloatToDouble { src, .. }
                | TackyInstr::DoubleToFloat { src, .. } => {
                    Some(resolve_val_type(src, &const_map, types))
                }
                _ => None,
            };

            let original_instr = instr;
            let instr = if matches!(
                &original_instr,
                TackyInstr::Copy { .. }
                    | TackyInstr::Store { .. }
                    | TackyInstr::CopyToOffset { .. }
            ) {
                original_instr.clone()
            } else {
                resolve_constants(&original_instr, &const_map)
            };

            let mut folded = fold_instruction(instr, types, src_type_hint);
            if let (
                TackyInstr::Binary {
                    op: original_op, ..
                },
                TackyInstr::Binary { .. },
            ) = (&original_instr, &folded)
            {
                if is_typed_sensitive_binary(original_op) {
                    folded = original_instr.clone();
                }
            }
            if folded != original_instr {
                changed = true;
            }

            match &folded {
                TackyInstr::Copy {
                    src: TackyVal::Constant(c),
                    dst: TackyVal::Var(name),
                } => {
                    let t = types.get(name).copied().unwrap_or(CType::Int);
                    const_map.insert(
                        name.clone(),
                        KnownConstant {
                            value: TackyVal::Constant(*c),
                            value_type: t,
                        },
                    );
                }
                TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(d),
                    dst: TackyVal::Var(name),
                } => {
                    let dst_type = types.get(name).copied().unwrap_or(CType::Double);
                    if matches!(dst_type, CType::Float | CType::Double) {
                        const_map.insert(
                            name.clone(),
                            KnownConstant {
                                value: TackyVal::DoubleConstant(*d),
                                value_type: CType::Double,
                            },
                        );
                    } else {
                        const_map.remove(name);
                    }
                }
                TackyInstr::Copy {
                    src: TackyVal::Int128Constant(c),
                    dst: TackyVal::Var(name),
                } => {
                    let dst_type = types.get(name).copied().unwrap_or(CType::Int128);
                    if dst_type == CType::Int128 {
                        const_map.insert(
                            name.clone(),
                            KnownConstant {
                                value: TackyVal::Int128Constant(*c),
                                value_type: CType::Int128,
                            },
                        );
                    } else {
                        const_map.remove(name);
                    }
                }
                TackyInstr::Copy {
                    src: TackyVal::UInt128Constant(c),
                    dst: TackyVal::Var(name),
                } => {
                    let dst_type = types.get(name).copied().unwrap_or(CType::UInt128);
                    if dst_type == CType::UInt128 {
                        const_map.insert(
                            name.clone(),
                            KnownConstant {
                                value: TackyVal::UInt128Constant(*c),
                                value_type: CType::UInt128,
                            },
                        );
                    } else {
                        const_map.remove(name);
                    }
                }
                TackyInstr::Copy {
                    src: TackyVal::Var(s),
                    dst: TackyVal::Var(name),
                } => {
                    if let Some(constant) = const_map.get(s).cloned() {
                        let dst_type = types.get(name).copied().unwrap_or(CType::Int);
                        if same_copy_type(constant.value_type, dst_type) {
                            const_map.insert(name.clone(), constant);
                        } else {
                            const_map.remove(name);
                        }
                    } else {
                        const_map.remove(name);
                    }
                }
                _ => {
                    if let Some(dst_name) = get_dst_name(&folded) {
                        const_map.remove(dst_name);
                    }
                }
            }

            folded
        })
        .collect();

    (folded, changed)
}

fn is_constant_folding_barrier(instr: &TackyInstr) -> bool {
    matches!(
        instr,
        TackyInstr::FunCall { .. }
            | TackyInstr::Store { .. }
            | TackyInstr::AtomicFence
            | TackyInstr::AtomicFetch { .. }
            | TackyInstr::AtomicExchange { .. }
            | TackyInstr::AtomicCompareExchange { .. }
            | TackyInstr::AtomicCompareSwap { .. }
            | TackyInstr::BuiltinSetjmp { .. }
            | TackyInstr::BuiltinLongjmp { .. }
            | TackyInstr::VaStart { .. }
    )
}

fn same_copy_type(src: CType, dst: CType) -> bool {
    src == dst || (src.is_signed() == dst.is_signed() && src.size() == dst.size())
}

fn resolve_constants(instr: &TackyInstr, const_map: &HashMap<String, KnownConstant>) -> TackyInstr {
    match instr {
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            let new_left = resolve_val(left, const_map);
            let new_right = resolve_val(right, const_map);
            TackyInstr::Binary {
                op: op.clone(),
                left: new_left,
                right: new_right,
                dst: dst.clone(),
            }
        }
        TackyInstr::Unary { op, src, dst } => {
            let new_src = resolve_val(src, const_map);
            TackyInstr::Unary {
                op: op.clone(),
                src: new_src,
                dst: dst.clone(),
            }
        }
        TackyInstr::JumpIfZero(val, target) => {
            TackyInstr::JumpIfZero(resolve_val(val, const_map), target.clone())
        }
        TackyInstr::JumpIfNotZero(val, target) => {
            TackyInstr::JumpIfNotZero(resolve_val(val, const_map), target.clone())
        }
        TackyInstr::Return(val) => TackyInstr::Return(resolve_val(val, const_map)),
        TackyInstr::Truncate { src, dst } => TackyInstr::Truncate {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::SignExtend { src, dst } => TackyInstr::SignExtend {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::ZeroExtend { src, dst } => TackyInstr::ZeroExtend {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::DoubleToInt { src, dst } => TackyInstr::DoubleToInt {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::FloatToInt { src, dst } => TackyInstr::FloatToInt {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::DoubleToUInt { src, dst } => TackyInstr::DoubleToUInt {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::FloatToUInt { src, dst } => TackyInstr::FloatToUInt {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::IntToDouble { src, dst } => TackyInstr::IntToDouble {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::IntToFloat { src, dst } => TackyInstr::IntToFloat {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::UIntToDouble { src, dst } => TackyInstr::UIntToDouble {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::UIntToFloat { src, dst } => TackyInstr::UIntToFloat {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::FloatToDouble { src, dst } => TackyInstr::FloatToDouble {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::DoubleToFloat { src, dst } => TackyInstr::DoubleToFloat {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::Copy { src, dst } => TackyInstr::Copy {
            src: resolve_val(src, const_map),
            dst: dst.clone(),
        },
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => TackyInstr::AddPtr {
            ptr: resolve_val(ptr, const_map),
            index: resolve_val(index, const_map),
            scale: *scale,
            dst: dst.clone(),
        },
        TackyInstr::Store { src, dst_ptr } => TackyInstr::Store {
            src: resolve_val(src, const_map),
            dst_ptr: resolve_val(dst_ptr, const_map),
        },
        TackyInstr::Load { src_ptr, dst } => TackyInstr::Load {
            src_ptr: resolve_val(src_ptr, const_map),
            dst: dst.clone(),
        },
        TackyInstr::FunCall {
            name,
            args,
            dst,
            stack_arg_indices,
            memory_arg_blocks,
            struct_arg_groups,
            variadic,
            fixed_flat_arg_count,
            hidden_return,
            indirect,
        } => {
            let mut new_args = Vec::with_capacity(args.len());
            for arg in args {
                new_args.push(resolve_val(arg, const_map));
            }
            TackyInstr::FunCall {
                name: name.clone(),
                args: new_args,
                dst: dst.clone(),
                stack_arg_indices: stack_arg_indices.clone(),
                memory_arg_blocks: memory_arg_blocks.clone(),
                struct_arg_groups: struct_arg_groups.clone(),
                variadic: *variadic,
                fixed_flat_arg_count: *fixed_flat_arg_count,
                hidden_return: *hidden_return,
                indirect: *indirect,
            }
        }
        other => other.clone(),
    }
}

fn resolve_val(val: &TackyVal, const_map: &HashMap<String, KnownConstant>) -> TackyVal {
    if let TackyVal::Var(name) = val {
        if let Some(constant) = const_map.get(name) {
            return constant.value.clone();
        }
    }
    val.clone()
}

fn resolve_val_type(
    val: &TackyVal,
    const_map: &HashMap<String, KnownConstant>,
    types: &indexmap::IndexMap<String, CType>,
) -> CType {
    if let TackyVal::Var(name) = val {
        if let Some(constant) = const_map.get(name) {
            return constant.value_type;
        }
        if let Some(t) = types.get(name) {
            return *t;
        }
    }
    match val {
        TackyVal::Constant(_) => CType::Int,
        TackyVal::Int128Constant(_) => CType::Int128,
        TackyVal::UInt128Constant(_) => CType::UInt128,
        TackyVal::DoubleConstant(_) => CType::Double,
        TackyVal::Var(_) => CType::Int,
    }
}

fn get_dst_name(instr: &TackyInstr) -> Option<&str> {
    match instr {
        TackyInstr::Binary {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::Unary {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::Copy {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::Truncate {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::SignExtend {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::ZeroExtend {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::DoubleToInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::FloatToInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::DoubleToUInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::FloatToUInt {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::IntToDouble {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::IntToFloat {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::UIntToDouble {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::UIntToFloat {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::FloatToDouble {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::DoubleToFloat {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::Load {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::GetAddress {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::AddPtr {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::FrameAddress {
            dst: TackyVal::Var(n),
        }
        | TackyInstr::CopyFromOffset {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::LoadLabelAddress(_, TackyVal::Var(n)) => Some(n),
        TackyInstr::FunCall {
            dst: TackyVal::Var(n),
            ..
        } => Some(n),
        _ => None,
    }
}

fn fold_instruction(
    instr: TackyInstr,
    types: &indexmap::IndexMap<String, CType>,
    src_type_hint: Option<CType>,
) -> TackyInstr {
    match instr {
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            let dst_type = if let TackyVal::Var(ref n) = dst {
                types.get(n).copied().unwrap_or(CType::Int)
            } else {
                CType::Int
            };
            let op_type = if is_comparison(&op) {
                src_type_hint.unwrap_or(dst_type)
            } else {
                dst_type
            };
            match op_type {
                CType::Int128 => {
                    if let (Some(l), Some(r)) = (const_i128_val(&left), const_i128_val(&right)) {
                        if let Some(result) = eval_binary_i128(&op, l, r) {
                            let src = if is_comparison(&op) {
                                TackyVal::Constant(result as i64)
                            } else {
                                TackyVal::Int128Constant(result)
                            };
                            return TackyInstr::Copy { src, dst };
                        }
                    }
                }
                CType::UInt128 => {
                    if let (Some(l), Some(r)) = (const_u128_val(&left), const_u128_val(&right)) {
                        if let Some(result) = eval_binary_u128(&op, l, r) {
                            let src = if is_comparison(&op) {
                                TackyVal::Constant(result as i64)
                            } else {
                                TackyVal::UInt128Constant(result)
                            };
                            return TackyInstr::Copy { src, dst };
                        }
                    }
                }
                _ => {}
            }
            if let (Some(l), Some(r)) = (const_val(&left), const_val(&right)) {
                if let Some(result) = eval_binary_typed(&op, l, r, dst_type, src_type_hint) {
                    return TackyInstr::Copy {
                        src: TackyVal::Constant(result),
                        dst,
                    };
                }
            }
            if let (Some(l), Some(r)) = (const_double(&left), const_double(&right)) {
                if is_comparison(&op) {
                    if let Some(result) = eval_binary_double(&op, l, r) {
                        return TackyInstr::Copy {
                            src: TackyVal::Constant(result as i64),
                            dst,
                        };
                    }
                } else if let Some(result) = eval_binary_double(&op, l, r) {
                    return TackyInstr::Copy {
                        src: TackyVal::DoubleConstant(result),
                        dst,
                    };
                }
            }
            if let Some(simplified) =
                simplify_binary_identity(&op, &left, &right, &dst, dst_type, src_type_hint, types)
            {
                return simplified;
            }
            TackyInstr::Binary {
                op,
                left,
                right,
                dst,
            }
        }
        TackyInstr::Unary { op, src, dst } => {
            let dst_type = if let TackyVal::Var(ref n) = dst {
                types.get(n).copied().unwrap_or(CType::Int)
            } else {
                CType::Int
            };
            match dst_type {
                CType::Int128 => {
                    if let Some(v) = const_i128_val(&src) {
                        if let Some(result) = eval_unary_i128(&op, v) {
                            let src = match op {
                                TackyUnaryOp::LogicalNot => TackyVal::Constant(result as i64),
                                _ => TackyVal::Int128Constant(result),
                            };
                            return TackyInstr::Copy { src, dst };
                        }
                    }
                }
                CType::UInt128 => {
                    if let Some(v) = const_u128_val(&src) {
                        if let Some(result) = eval_unary_u128(&op, v) {
                            let src = match op {
                                TackyUnaryOp::LogicalNot => TackyVal::Constant(result as i64),
                                _ => TackyVal::UInt128Constant(result),
                            };
                            return TackyInstr::Copy { src, dst };
                        }
                    }
                }
                _ => {}
            }
            if let Some(v) = const_val(&src) {
                if let Some(result) = eval_unary(&op, v) {
                    return TackyInstr::Copy {
                        src: TackyVal::Constant(result),
                        dst,
                    };
                }
            }
            if let Some(d) = const_double(&src) {
                match op {
                    TackyUnaryOp::Negate => {
                        return TackyInstr::Copy {
                            src: TackyVal::DoubleConstant(-d),
                            dst,
                        };
                    }
                    TackyUnaryOp::LogicalNot => {
                        return TackyInstr::Copy {
                            src: TackyVal::Constant(if d == 0.0 { 1 } else { 0 }),
                            dst,
                        };
                    }
                    _ => {}
                }
            }
            TackyInstr::Unary { op, src, dst }
        }
        TackyInstr::JumpIfZero(val, target) => {
            if let Some(v) = const_i128_val(&val) {
                if v == 0 {
                    return TackyInstr::Jump(target);
                } else {
                    return TackyInstr::Nop;
                }
            }
            if let Some(v) = const_val(&val) {
                if v == 0 {
                    return TackyInstr::Jump(target);
                } else {
                    return TackyInstr::Nop;
                }
            }
            if let Some(d) = const_double(&val) {
                if d == 0.0 {
                    return TackyInstr::Jump(target);
                } else {
                    return TackyInstr::Nop;
                }
            }
            TackyInstr::JumpIfZero(val, target)
        }
        TackyInstr::JumpIfNotZero(val, target) => {
            if let Some(v) = const_i128_val(&val) {
                if v != 0 {
                    return TackyInstr::Jump(target);
                } else {
                    return TackyInstr::Nop;
                }
            }
            if let Some(v) = const_val(&val) {
                if v != 0 {
                    return TackyInstr::Jump(target);
                } else {
                    return TackyInstr::Nop;
                }
            }
            if let Some(d) = const_double(&val) {
                if d != 0.0 {
                    return TackyInstr::Jump(target);
                } else {
                    return TackyInstr::Nop;
                }
            }
            TackyInstr::JumpIfNotZero(val, target)
        }
        TackyInstr::Truncate { src, dst } => {
            let dst_type = if let TackyVal::Var(ref n) = dst {
                types.get(n).copied().unwrap_or(CType::Int)
            } else {
                CType::Int
            };
            if let Some(v) = const_i128_val(&src) {
                return TackyInstr::Copy {
                    src: integer_tacky_constant(cast_integer_constant_wide(v, dst_type), dst_type),
                    dst,
                };
            }
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::Constant(cast_integer_constant(v, dst_type)),
                    dst,
                };
            }
            TackyInstr::Truncate { src, dst }
        }
        TackyInstr::SignExtend { src, dst } => {
            let dst_type = if let TackyVal::Var(ref n) = dst {
                types.get(n).copied().unwrap_or(CType::Int)
            } else {
                CType::Int
            };
            let src_type = src_type_hint.unwrap_or(dst_type);
            if let Some(v) = const_i128_val(&src) {
                return TackyInstr::Copy {
                    src: integer_tacky_constant(
                        cast_integer_constant_wide(
                            sign_extend_integer_constant_wide(v, src_type),
                            dst_type,
                        ),
                        dst_type,
                    ),
                    dst,
                };
            }
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::Constant(cast_integer_constant(
                        sign_extend_integer_constant(v, src_type),
                        dst_type,
                    )),
                    dst,
                };
            }
            TackyInstr::SignExtend { src, dst }
        }
        TackyInstr::ZeroExtend { src, dst } => {
            let dst_type = if let TackyVal::Var(ref n) = dst {
                types.get(n).copied().unwrap_or(CType::UInt)
            } else {
                CType::UInt
            };
            let src_type = src_type_hint.unwrap_or(dst_type);
            if let Some(v) = const_i128_val(&src) {
                return TackyInstr::Copy {
                    src: integer_tacky_constant(
                        cast_integer_constant_wide(
                            zero_extend_integer_constant_wide(v, src_type),
                            dst_type,
                        ),
                        dst_type,
                    ),
                    dst,
                };
            }
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::Constant(cast_integer_constant(
                        zero_extend_integer_constant(v, src_type),
                        dst_type,
                    )),
                    dst,
                };
            }
            TackyInstr::ZeroExtend { src, dst }
        }
        TackyInstr::DoubleToInt { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::Int)
                } else {
                    CType::Int
                };
                let v = match dst_type {
                    CType::Int => d as i32 as i128,
                    CType::Long => d as i64 as i128,
                    CType::Char | CType::SChar => d as i8 as i128,
                    CType::Short => d as i16 as i128,
                    _ => d as i64 as i128,
                };
                return TackyInstr::Copy {
                    src: integer_tacky_constant(v, dst_type),
                    dst,
                };
            }
            TackyInstr::DoubleToInt { src, dst }
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::UInt)
                } else {
                    CType::UInt
                };
                let v = match dst_type {
                    CType::UInt => d as u32 as i128,
                    CType::ULong => d as u64 as i128,
                    CType::UChar => d as u8 as i128,
                    CType::UShort => d as u16 as i128,
                    _ => d as u64 as i128,
                };
                return TackyInstr::Copy {
                    src: integer_tacky_constant(v, dst_type),
                    dst,
                };
            }
            TackyInstr::DoubleToUInt { src, dst }
        }
        TackyInstr::FloatToInt { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                return TackyInstr::DoubleToInt {
                    src: TackyVal::DoubleConstant(d as f32 as f64),
                    dst,
                };
            }
            TackyInstr::FloatToInt { src, dst }
        }
        TackyInstr::FloatToUInt { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                return TackyInstr::DoubleToUInt {
                    src: TackyVal::DoubleConstant(d as f32 as f64),
                    dst,
                };
            }
            TackyInstr::FloatToUInt { src, dst }
        }
        TackyInstr::IntToDouble { src, dst } => {
            if let Some(v) = const_i128_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(v as f64),
                    dst,
                };
            }
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(v as f64),
                    dst,
                };
            }
            TackyInstr::IntToDouble { src, dst }
        }
        TackyInstr::IntToFloat { src, dst } => {
            if let Some(v) = const_i128_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(v as f32 as f64),
                    dst,
                };
            }
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(v as f32 as f64),
                    dst,
                };
            }
            TackyInstr::IntToFloat { src, dst }
        }
        TackyInstr::UIntToDouble { src, dst } => {
            match src {
                TackyVal::UInt128Constant(v) => {
                    return TackyInstr::Copy {
                        src: TackyVal::DoubleConstant(v as f64),
                        dst,
                    };
                }
                TackyVal::Int128Constant(v) => {
                    return TackyInstr::Copy {
                        src: TackyVal::DoubleConstant((v as u128) as f64),
                        dst,
                    };
                }
                _ => {}
            }
            if let Some(v) = const_val(&src) {
                let src_type = src_type_hint.unwrap_or(CType::ULong);
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(
                        unsigned_integer_constant_as_u64(v, src_type) as f64
                    ),
                    dst,
                };
            }
            TackyInstr::UIntToDouble { src, dst }
        }
        TackyInstr::UIntToFloat { src, dst } => {
            match src {
                TackyVal::UInt128Constant(v) => {
                    return TackyInstr::Copy {
                        src: TackyVal::DoubleConstant(v as f32 as f64),
                        dst,
                    };
                }
                TackyVal::Int128Constant(v) => {
                    return TackyInstr::Copy {
                        src: TackyVal::DoubleConstant((v as u128) as f32 as f64),
                        dst,
                    };
                }
                _ => {}
            }
            if let Some(v) = const_val(&src) {
                let src_type = src_type_hint.unwrap_or(CType::ULong);
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(
                        unsigned_integer_constant_as_u64(v, src_type) as f32 as f64
                    ),
                    dst,
                };
            }
            TackyInstr::UIntToFloat { src, dst }
        }
        TackyInstr::FloatToDouble { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(d),
                    dst,
                };
            }
            TackyInstr::FloatToDouble { src, dst }
        }
        TackyInstr::DoubleToFloat { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(d as f32 as f64),
                    dst,
                };
            }
            TackyInstr::DoubleToFloat { src, dst }
        }
        TackyInstr::FunCall {
            name,
            args,
            dst,
            stack_arg_indices,
            memory_arg_blocks,
            struct_arg_groups,
            variadic,
            fixed_flat_arg_count,
            hidden_return,
            indirect,
        } => {
            if args.len() == 1 && !variadic && !hidden_return && !indirect {
                if let Some(d) = const_double(&args[0]) {
                    let src = match name.as_str() {
                        "__fixdfti" => Some(TackyVal::Int128Constant(d as i128)),
                        "__fixunsdfti" => Some(TackyVal::UInt128Constant(d as u128)),
                        "__fixsfti" => Some(TackyVal::Int128Constant(d as f32 as i128)),
                        "__fixunssfti" => Some(TackyVal::UInt128Constant(d as f32 as u128)),
                        _ => None,
                    };
                    if let Some(src) = src {
                        return TackyInstr::Copy { src, dst };
                    }
                }
            }
            TackyInstr::FunCall {
                name,
                args,
                dst,
                stack_arg_indices,
                memory_arg_blocks,
                struct_arg_groups,
                variadic,
                fixed_flat_arg_count,
                hidden_return,
                indirect,
            }
        }
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => {
            if let Some(idx) = const_val(&index) {
                let offset = idx.wrapping_mul(scale);
                if offset == 0 {
                    return TackyInstr::Copy { src: ptr, dst };
                }
                if scale != 1 {
                    return TackyInstr::AddPtr {
                        ptr,
                        index: TackyVal::Constant(offset),
                        scale: 1,
                        dst,
                    };
                }
            }
            TackyInstr::AddPtr {
                ptr,
                index,
                scale,
                dst,
            }
        }
        other => other,
    }
}

fn simplify_binary_identity(
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    dst_type: CType,
    src_type_hint: Option<CType>,
    types: &indexmap::IndexMap<String, CType>,
) -> Option<TackyInstr> {
    let op_type = if is_comparison(op) {
        src_type_hint.unwrap_or(dst_type)
    } else {
        dst_type
    };
    if !is_integer_scalar_type(op_type) {
        return None;
    }

    if left == right {
        match op {
            TackyBinaryOp::Sub | TackyBinaryOp::BitwiseXor => {
                return Some(TackyInstr::Copy {
                    src: integer_tacky_constant(0, dst_type),
                    dst: dst.clone(),
                });
            }
            TackyBinaryOp::BitwiseAnd | TackyBinaryOp::BitwiseOr => {
                return copy_identity_val(left, dst, dst_type, types);
            }
            TackyBinaryOp::BitwiseNand if can_copy_identity_val(left, dst_type, types) => {
                return Some(TackyInstr::Unary {
                    op: TackyUnaryOp::Complement,
                    src: left.clone(),
                    dst: dst.clone(),
                });
            }
            _ => {}
        }
    }

    // Unsigned arithmetic is defined modulo 2^N, so these reductions preserve
    // both the value and the operation's width. Do not apply them to signed
    // operands: a left shift of a negative value has different C semantics
    // from multiplication.
    if let Some(shift) = unsigned_power_of_two_shift_count(right, op_type) {
        match op {
            TackyBinaryOp::Mul => {
                return Some(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftLeft,
                    left: left.clone(),
                    right: TackyVal::Constant(shift),
                    dst: dst.clone(),
                });
            }
            TackyBinaryOp::Div => {
                return Some(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: left.clone(),
                    right: TackyVal::Constant(shift),
                    dst: dst.clone(),
                });
            }
            TackyBinaryOp::Mod => {
                return Some(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: left.clone(),
                    right: unsigned_power_of_two_mask(shift, op_type),
                    dst: dst.clone(),
                });
            }
            _ => {}
        }
    }
    if matches!(op, TackyBinaryOp::Mul) {
        if let Some(shift) = unsigned_power_of_two_shift_count(left, op_type) {
            return Some(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: right.clone(),
                right: TackyVal::Constant(shift),
                dst: dst.clone(),
            });
        }
    }

    match op {
        TackyBinaryOp::Add | TackyBinaryOp::BitwiseOr | TackyBinaryOp::BitwiseXor => {
            if integer_zero_val(right) {
                copy_identity_val(left, dst, dst_type, types)
            } else if integer_zero_val(left) {
                copy_identity_val(right, dst, dst_type, types)
            } else if matches!(op, TackyBinaryOp::BitwiseOr)
                && (integer_all_ones_val(left, dst_type) || integer_all_ones_val(right, dst_type))
            {
                Some(TackyInstr::Copy {
                    src: integer_all_ones_constant(dst_type),
                    dst: dst.clone(),
                })
            } else if matches!(op, TackyBinaryOp::BitwiseXor)
                && integer_all_ones_val(right, dst_type)
                && can_copy_identity_val(left, dst_type, types)
            {
                Some(TackyInstr::Unary {
                    op: TackyUnaryOp::Complement,
                    src: left.clone(),
                    dst: dst.clone(),
                })
            } else if matches!(op, TackyBinaryOp::BitwiseXor)
                && integer_all_ones_val(left, dst_type)
                && can_copy_identity_val(right, dst_type, types)
            {
                Some(TackyInstr::Unary {
                    op: TackyUnaryOp::Complement,
                    src: right.clone(),
                    dst: dst.clone(),
                })
            } else {
                None
            }
        }
        TackyBinaryOp::Sub => {
            if integer_zero_val(right) {
                copy_identity_val(left, dst, dst_type, types)
            } else {
                None
            }
        }
        TackyBinaryOp::Mul => {
            if integer_zero_val(left) || integer_zero_val(right) {
                Some(TackyInstr::Copy {
                    src: integer_tacky_constant(0, dst_type),
                    dst: dst.clone(),
                })
            } else if integer_one_val(right) {
                copy_identity_val(left, dst, dst_type, types)
            } else if integer_one_val(left) {
                copy_identity_val(right, dst, dst_type, types)
            } else {
                None
            }
        }
        TackyBinaryOp::Div => {
            if integer_zero_val(left) {
                Some(TackyInstr::Copy {
                    src: integer_tacky_constant(0, dst_type),
                    dst: dst.clone(),
                })
            } else if integer_one_val(right) {
                copy_identity_val(left, dst, dst_type, types)
            } else {
                None
            }
        }
        TackyBinaryOp::Mod => {
            if integer_zero_val(left) || integer_one_val(right) {
                Some(TackyInstr::Copy {
                    src: integer_tacky_constant(0, dst_type),
                    dst: dst.clone(),
                })
            } else {
                None
            }
        }
        TackyBinaryOp::BitwiseAnd => {
            if integer_zero_val(left) || integer_zero_val(right) {
                Some(TackyInstr::Copy {
                    src: integer_tacky_constant(0, dst_type),
                    dst: dst.clone(),
                })
            } else if integer_all_ones_val(right, dst_type) {
                copy_identity_val(left, dst, dst_type, types)
            } else if integer_all_ones_val(left, dst_type) {
                copy_identity_val(right, dst, dst_type, types)
            } else {
                None
            }
        }
        TackyBinaryOp::BitwiseNand => {
            if integer_zero_val(left) || integer_zero_val(right) {
                Some(TackyInstr::Copy {
                    src: integer_all_ones_constant(dst_type),
                    dst: dst.clone(),
                })
            } else if integer_all_ones_val(right, dst_type)
                && can_copy_identity_val(left, dst_type, types)
            {
                Some(TackyInstr::Unary {
                    op: TackyUnaryOp::Complement,
                    src: left.clone(),
                    dst: dst.clone(),
                })
            } else if integer_all_ones_val(left, dst_type)
                && can_copy_identity_val(right, dst_type, types)
            {
                Some(TackyInstr::Unary {
                    op: TackyUnaryOp::Complement,
                    src: right.clone(),
                    dst: dst.clone(),
                })
            } else {
                None
            }
        }
        TackyBinaryOp::ShiftLeft | TackyBinaryOp::ShiftRight => {
            if integer_zero_val(left) {
                Some(TackyInstr::Copy {
                    src: integer_tacky_constant(0, dst_type),
                    dst: dst.clone(),
                })
            } else if integer_zero_val(right) {
                copy_identity_val(left, dst, dst_type, types)
            } else {
                None
            }
        }
        TackyBinaryOp::Equal
        | TackyBinaryOp::NotEqual
        | TackyBinaryOp::LessThan
        | TackyBinaryOp::LessEqual
        | TackyBinaryOp::GreaterThan
        | TackyBinaryOp::GreaterEqual
            if left == right =>
        {
            let result = match op {
                TackyBinaryOp::Equal | TackyBinaryOp::LessEqual | TackyBinaryOp::GreaterEqual => 1,
                TackyBinaryOp::NotEqual | TackyBinaryOp::LessThan | TackyBinaryOp::GreaterThan => 0,
                _ => unreachable!(),
            };
            Some(TackyInstr::Copy {
                src: TackyVal::Constant(result),
                dst: dst.clone(),
            })
        }
        _ => None,
    }
}

fn unsigned_power_of_two_shift_count(val: &TackyVal, ty: CType) -> Option<i64> {
    let value = match (ty, val) {
        (CType::UInt, TackyVal::Constant(value)) => *value as u32 as u128,
        (CType::ULong, TackyVal::Constant(value)) => *value as u64 as u128,
        (CType::UInt128, TackyVal::UInt128Constant(value)) => *value,
        _ => return None,
    };
    if value > 1 && value.is_power_of_two() {
        Some(value.trailing_zeros() as i64)
    } else {
        None
    }
}

fn unsigned_power_of_two_mask(shift: i64, ty: CType) -> TackyVal {
    debug_assert!(shift > 0);
    let mask = (1_u128 << shift) - 1;
    integer_tacky_constant(mask as i128, ty)
}

fn copy_identity_val(
    src: &TackyVal,
    dst: &TackyVal,
    dst_type: CType,
    types: &indexmap::IndexMap<String, CType>,
) -> Option<TackyInstr> {
    if !can_copy_identity_val(src, dst_type, types) {
        return None;
    }
    Some(TackyInstr::Copy {
        src: src.clone(),
        dst: dst.clone(),
    })
}

fn can_copy_identity_val(
    val: &TackyVal,
    dst_type: CType,
    types: &indexmap::IndexMap<String, CType>,
) -> bool {
    match val {
        TackyVal::Var(name) => types
            .get(name)
            .copied()
            .is_some_and(|src_type| same_copy_type(src_type, dst_type)),
        TackyVal::Constant(_) => !matches!(dst_type, CType::Int128 | CType::UInt128),
        TackyVal::Int128Constant(_) => dst_type == CType::Int128,
        TackyVal::UInt128Constant(_) => dst_type == CType::UInt128,
        TackyVal::DoubleConstant(_) => false,
    }
}

fn integer_zero_val(val: &TackyVal) -> bool {
    match val {
        TackyVal::Constant(v) => *v == 0,
        TackyVal::Int128Constant(v) => *v == 0,
        TackyVal::UInt128Constant(v) => *v == 0,
        _ => false,
    }
}

fn integer_one_val(val: &TackyVal) -> bool {
    match val {
        TackyVal::Constant(v) => *v == 1,
        TackyVal::Int128Constant(v) => *v == 1,
        TackyVal::UInt128Constant(v) => *v == 1,
        _ => false,
    }
}

fn integer_all_ones_val(val: &TackyVal, dst_type: CType) -> bool {
    let all_ones = cast_integer_constant_wide(-1, dst_type);
    match val {
        TackyVal::Constant(v) => cast_integer_constant_wide(*v as i128, dst_type) == all_ones,
        TackyVal::Int128Constant(v) => cast_integer_constant_wide(*v, dst_type) == all_ones,
        TackyVal::UInt128Constant(v) => {
            cast_integer_constant_wide(*v as i128, dst_type) == all_ones
        }
        _ => false,
    }
}

fn integer_all_ones_constant(dst_type: CType) -> TackyVal {
    integer_tacky_constant(cast_integer_constant_wide(-1, dst_type), dst_type)
}

fn is_integer_scalar_type(ty: CType) -> bool {
    !matches!(
        ty,
        CType::Void
            | CType::Float
            | CType::Double
            | CType::LongDouble
            | CType::Pointer
            | CType::Struct
    )
}

fn const_val(val: &TackyVal) -> Option<i64> {
    match val {
        TackyVal::Constant(c) => Some(*c),
        _ => None,
    }
}

fn const_i128_val(val: &TackyVal) -> Option<i128> {
    match val {
        TackyVal::Constant(c) => Some(*c as i128),
        TackyVal::Int128Constant(c) => Some(*c),
        TackyVal::UInt128Constant(c) => Some(*c as i128),
        _ => None,
    }
}

fn const_u128_val(val: &TackyVal) -> Option<u128> {
    match val {
        TackyVal::Constant(c) => Some(*c as u128),
        TackyVal::Int128Constant(c) => Some(*c as u128),
        TackyVal::UInt128Constant(c) => Some(*c),
        _ => None,
    }
}

fn cast_integer_constant(value: i64, dst_type: CType) -> i64 {
    match dst_type {
        CType::Bool => (value != 0) as i64,
        CType::Char | CType::SChar => value as i8 as i64,
        CType::UChar => value as u8 as i64,
        CType::Short => value as i16 as i64,
        CType::UShort => value as u16 as i64,
        CType::Int => value as i32 as i64,
        CType::UInt => value as u32 as i64,
        CType::Long => value,
        CType::ULong => value as u64 as i64,
        _ => value,
    }
}

fn integer_tacky_constant(value: i128, dst_type: CType) -> TackyVal {
    match dst_type {
        CType::Int128 => TackyVal::Int128Constant(value),
        CType::UInt128 => TackyVal::UInt128Constant(value as u128),
        CType::UInt | CType::ULong | CType::Pointer => TackyVal::Constant(value as u64 as i64),
        _ => TackyVal::Constant(value as i64),
    }
}

fn cast_integer_constant_wide(value: i128, dst_type: CType) -> i128 {
    match dst_type {
        CType::Bool => (value != 0) as i128,
        CType::Char | CType::SChar => value as i8 as i128,
        CType::UChar => value as u8 as i128,
        CType::Short => value as i16 as i128,
        CType::UShort => value as u16 as i128,
        CType::Int => value as i32 as i128,
        CType::UInt => value as u32 as i128,
        CType::Long => value as i64 as i128,
        CType::ULong | CType::Pointer => value as u64 as i128,
        CType::Int128 => value,
        CType::UInt128 => value as u128 as i128,
        _ => value,
    }
}

fn sign_extend_integer_constant_wide(value: i128, src_type: CType) -> i128 {
    match src_type {
        CType::Bool => (value != 0) as i128,
        CType::Char | CType::SChar => value as i8 as i128,
        CType::UChar => value as u8 as i128,
        CType::Short => value as i16 as i128,
        CType::UShort => value as u16 as i128,
        CType::Int => value as i32 as i128,
        CType::UInt => value as u32 as i128,
        CType::Long | CType::ULong => value as i64 as i128,
        CType::Int128 | CType::UInt128 => value,
        _ => value,
    }
}

fn zero_extend_integer_constant_wide(value: i128, src_type: CType) -> i128 {
    match src_type {
        CType::Bool => (value != 0) as i128,
        CType::Char | CType::SChar | CType::UChar => value as u8 as i128,
        CType::Short | CType::UShort => value as u16 as i128,
        CType::Int | CType::UInt => value as u32 as i128,
        CType::Long | CType::ULong => value as u64 as i128,
        CType::Int128 | CType::UInt128 => value as u128 as i128,
        _ => value,
    }
}

fn sign_extend_integer_constant(value: i64, src_type: CType) -> i64 {
    match src_type {
        CType::Bool => (value != 0) as i64,
        CType::Char | CType::SChar => value as i8 as i64,
        CType::UChar => value as u8 as i64,
        CType::Short => value as i16 as i64,
        CType::UShort => value as u16 as i64,
        CType::Int => value as i32 as i64,
        CType::UInt => value as u32 as i64,
        CType::Long | CType::ULong => value,
        _ => value,
    }
}

fn zero_extend_integer_constant(value: i64, src_type: CType) -> i64 {
    match src_type {
        CType::Bool => (value != 0) as i64,
        CType::Char | CType::SChar | CType::UChar => value as u8 as i64,
        CType::Short | CType::UShort => value as u16 as i64,
        CType::Int | CType::UInt => value as u32 as i64,
        CType::Long | CType::ULong => value as u64 as i64,
        _ => value,
    }
}

fn is_comparison(op: &TackyBinaryOp) -> bool {
    matches!(
        op,
        TackyBinaryOp::Equal
            | TackyBinaryOp::NotEqual
            | TackyBinaryOp::LessThan
            | TackyBinaryOp::GreaterThan
            | TackyBinaryOp::LessEqual
            | TackyBinaryOp::GreaterEqual
    )
}

fn is_typed_sensitive_binary(op: &TackyBinaryOp) -> bool {
    is_comparison(op) || matches!(op, TackyBinaryOp::ShiftLeft | TackyBinaryOp::ShiftRight)
}

fn eval_binary_typed(
    op: &TackyBinaryOp,
    l: i64,
    r: i64,
    dst_type: CType,
    src_type_hint: Option<CType>,
) -> Option<i64> {
    let op_type = if is_comparison(op) {
        src_type_hint.unwrap_or(dst_type)
    } else {
        dst_type
    };

    match op_type {
        CType::Int | CType::Char | CType::SChar | CType::Short => {
            eval_binary_i32(op, l as i32, r as i32).map(|v| v as i64)
        }
        CType::UInt | CType::UChar | CType::UShort => {
            eval_binary_u32(op, l as u32, r as u32).map(|v| v as i64)
        }
        CType::Long => eval_binary(op, l, r),
        CType::ULong => eval_binary_u64(op, l as u64, r as u64).map(|v| v as i64),
        CType::Int128 | CType::UInt128 => None,
        _ => eval_binary(op, l, r),
    }
}

fn unsigned_integer_constant_as_u64(value: i64, src_type: CType) -> u64 {
    match src_type {
        CType::Bool => (value != 0) as u64,
        CType::Char | CType::SChar | CType::UChar => value as u8 as u64,
        CType::Short | CType::UShort => value as u16 as u64,
        CType::Int | CType::UInt => value as u32 as u64,
        CType::Long | CType::ULong => value as u64,
        _ => value as u64,
    }
}

fn const_double(val: &TackyVal) -> Option<f64> {
    match val {
        TackyVal::DoubleConstant(d) => Some(*d),
        _ => None,
    }
}

fn eval_binary_double(op: &TackyBinaryOp, l: f64, r: f64) -> Option<f64> {
    match op {
        TackyBinaryOp::Add => Some(l + r),
        TackyBinaryOp::Sub => Some(l - r),
        TackyBinaryOp::Mul => Some(l * r),
        TackyBinaryOp::Div => Some(l / r),
        TackyBinaryOp::Equal => Some(if l == r { 1.0 } else { 0.0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1.0 } else { 0.0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1.0 } else { 0.0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1.0 } else { 0.0 }),
        TackyBinaryOp::LessEqual => Some(if l <= r { 1.0 } else { 0.0 }),
        TackyBinaryOp::GreaterEqual => Some(if l >= r { 1.0 } else { 0.0 }),
        _ => None,
    }
}
