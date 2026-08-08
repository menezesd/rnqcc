//! Compile-time evaluation of static constant expressions used for static
//! initializers (integer/wide-integer constant folding, init-value conversion).
//! Split out of the main TACKY generator; these are pure functions over the AST
//! and share the parent module's `Static*` value types via `use super::*`.

use super::*;

fn static_integer_constant_is_true(value: StaticIntegerConstant) -> bool {
    if value.is_double {
        f64::from_bits(value.value as u64) != 0.0
    } else {
        value.value != 0
    }
}

/// Truncate/convert a constant value to the target type's bit width
pub(super) fn convert_init_value(init_value: StaticScalarValue, target: CType) -> i64 {
    let val = init_value.value;
    let source_is_double = init_value.source_is_double;
    let source_is_unsigned = init_value.source_is_unsigned;

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

pub(super) fn static_init_value_to_f64(value: StaticScalarValue) -> f64 {
    if value.source_is_double {
        f64::from_bits(value.value as u64)
    } else if value.source_is_unsigned {
        value.value as u64 as f64
    } else {
        value.value as f64
    }
}

pub(super) fn neg_static_init_value(value: StaticScalarValue) -> StaticScalarValue {
    if value.source_is_double {
        StaticScalarValue::double_bits(-f64::from_bits(value.value as u64))
    } else {
        StaticScalarValue {
            value: value.value.wrapping_neg(),
            source_is_double: false,
            source_is_unsigned: value.source_is_unsigned,
        }
    }
}

pub(super) fn make_static_init(val: i64, t: CType) -> StaticInit {
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

pub(super) fn make_static_wide_integer_init(
    val: StaticWideIntegerConstant,
    t: CType,
) -> StaticInit {
    if val.is_zero() {
        return StaticInit::ZeroInit(t.size() as usize);
    }
    match t {
        CType::Int128 => StaticInit::Int128Init(val.value),
        CType::UInt128 => StaticInit::UInt128Init(val.value as u128),
        _ => make_static_init(val.value as i64, t),
    }
}

pub(super) fn eval_static_expr_full_type(
    exp: &Exp,
    full_types: &IndexMap<String, FullType>,
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
        Exp::Utf16StringLiteral(s) => Some(FullType::Array {
            elem: Box::new(FullType::Scalar(CType::UShort)),
            size: s.encode_utf16().count() + 1,
        }),
        Exp::Utf32StringLiteral(s) => Some(FullType::Array {
            elem: Box::new(FullType::Scalar(CType::UInt)),
            size: s.chars().count() + 1,
        }),
        Exp::Cast(_, Some(ft), _) => Some(ft.clone()),
        Exp::Cast(ctype, None, _) => Some(FullType::Scalar(*ctype)),
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

pub(super) fn eval_static_integer_expr_ctype(
    exp: &Exp,
    full_types: &IndexMap<String, FullType>,
) -> Option<CType> {
    match exp {
        Exp::Constant(_) => Some(CType::Int),
        Exp::LongConstant(_) => Some(CType::Long),
        Exp::UIntConstant(_) => Some(CType::UInt),
        Exp::ULongConstant(_) => Some(CType::ULong),
        Exp::Int128Constant(_) => Some(CType::Int128),
        Exp::UInt128Constant(_) => Some(CType::UInt128),
        Exp::Cast(ctype, None, _) => Some(*ctype),
        Exp::Cast(_, Some(ft), _) => Some(ft.to_ctype()),
        Exp::Unary(UnaryOp::Negate | UnaryOp::Complement, inner) => {
            Some(eval_static_integer_expr_ctype(inner, full_types)?.promote())
        }
        Exp::Unary(UnaryOp::LogicalNot, _) => Some(CType::Int),
        Exp::Binary(op, left, right) => {
            if static_binary_op_yields_int(op) {
                return Some(CType::Int);
            }
            let left_type = eval_static_integer_expr_ctype(left, full_types)?;
            let right_type = eval_static_integer_expr_ctype(right, full_types)?;
            if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
                Some(left_type.promote())
            } else {
                Some(CType::common(left_type, right_type))
            }
        }
        Exp::Conditional(_, then_exp, else_exp) => {
            let then_type = eval_static_integer_expr_ctype(then_exp, full_types)?;
            let else_type = eval_static_integer_expr_ctype(else_exp, full_types)?;
            Some(CType::common(then_type, else_type))
        }
        _ => Some(eval_static_expr_full_type(exp, full_types)?.to_ctype()),
    }
}

pub(super) fn static_binary_op_yields_int(op: &BinaryOp) -> bool {
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

pub(super) fn eval_static_integer_binary_operand_ctype(
    op: &BinaryOp,
    left: &Exp,
    right: &Exp,
    full_types: &IndexMap<String, FullType>,
) -> Option<CType> {
    let left_type = eval_static_integer_expr_ctype(left, full_types)?;
    let right_type = eval_static_integer_expr_ctype(right, full_types)?;
    if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
        Some(left_type.promote())
    } else {
        Some(CType::common(left_type, right_type))
    }
}

pub(super) fn eval_static_integer_constant_exp_with_context(
    exp: &Exp,
    struct_defs: &IndexMap<String, StructDef>,
    full_types: &IndexMap<String, FullType>,
) -> Option<StaticIntegerConstant> {
    eval_static_integer_constant_exp_with_context_and_values(
        exp,
        struct_defs,
        full_types,
        &IndexMap::new(),
    )
}

pub(super) fn eval_static_integer_constant_exp_with_context_and_values(
    exp: &Exp,
    struct_defs: &IndexMap<String, StructDef>,
    full_types: &IndexMap<String, FullType>,
    static_const_values: &IndexMap<String, StaticScalarValue>,
) -> Option<StaticIntegerConstant> {
    match exp {
        Exp::Constant(c) | Exp::LongConstant(c) => Some(static_integer_constant(*c, false, false)),
        Exp::UIntConstant(c) | Exp::ULongConstant(c) => {
            Some(static_integer_constant(*c, false, true))
        }
        Exp::Int128Constant(c) => Some(static_integer_constant(*c as i64, false, false)),
        Exp::UInt128Constant(c) => Some(static_integer_constant(*c as i64, false, true)),
        Exp::DoubleConstant(d) | Exp::LongDoubleConstant(d) => {
            Some(static_integer_constant(d.to_bits() as i64, true, false))
        }
        Exp::Var(name) => static_const_values.get(name).map(|value| {
            static_integer_constant(
                value.value,
                value.source_is_double,
                value.source_is_unsigned,
            )
        }),
        Exp::SizeOf(inner) => {
            let ft = eval_static_expr_full_type(inner, full_types)?;
            if ft.contains_vla_placeholder_with(struct_defs) {
                return None;
            }
            Some(static_integer_constant(
                i64::try_from(ft.checked_byte_size_with(struct_defs)?).ok()?,
                false,
                true,
            ))
        }
        Exp::SizeOfType(_, ft) if !ft.contains_vla_placeholder_with(struct_defs) => {
            Some(static_integer_constant(
                i64::try_from(ft.checked_byte_size_with(struct_defs)?).ok()?,
                false,
                true,
            ))
        }
        Exp::SizeOfType(_, _) => None,
        Exp::AlignOfType(ft) => Some(static_integer_constant(
            i64::try_from(ft.alignment_with(struct_defs)).ok()?,
            false,
            true,
        )),
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
                let constant = eval_static_integer_constant_exp_with_context_and_values(
                    inner,
                    struct_defs,
                    full_types,
                    static_const_values,
                )?;
                if target.is_floating() {
                    let value = if constant.is_double {
                        f64::from_bits(constant.value as u64)
                    } else if constant.is_unsigned {
                        constant.value as u64 as f64
                    } else {
                        constant.value as f64
                    };
                    let value = if *target == CType::Float {
                        value as f32 as f64
                    } else {
                        value
                    };
                    Some(static_integer_constant(
                        value.to_bits() as i64,
                        true,
                        *target == CType::Float,
                    ))
                } else if constant.is_double {
                    let value = f64::from_bits(constant.value as u64);
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
                    Some(static_integer_constant(raw, false, target_unsigned))
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
                    Some(static_integer_constant(
                        convert_init_value(
                            StaticScalarValue {
                                value: constant.value,
                                source_is_double: false,
                                source_is_unsigned: constant.is_unsigned,
                            },
                            *target,
                        ),
                        false,
                        target_unsigned,
                    ))
                }
            }
        }
        Exp::Unary(op, inner) => {
            let constant = eval_static_integer_constant_exp_with_context_and_values(
                inner,
                struct_defs,
                full_types,
                static_const_values,
            )?;
            match op {
                UnaryOp::Negate if constant.is_double => {
                    let d = -f64::from_bits(constant.value as u64);
                    Some(static_integer_constant(d.to_bits() as i64, true, false))
                }
                UnaryOp::Negate => Some(static_integer_constant(
                    constant.value.wrapping_neg(),
                    false,
                    constant.is_unsigned,
                )),
                UnaryOp::Complement if !constant.is_double => Some(static_integer_constant(
                    !constant.value,
                    false,
                    constant.is_unsigned,
                )),
                UnaryOp::LogicalNot if !constant.is_double => Some(static_integer_constant(
                    (constant.value == 0) as i64,
                    false,
                    false,
                )),
                _ => None,
            }
        }
        Exp::Binary(op, left_exp, right_exp) => {
            let op_type =
                eval_static_integer_binary_operand_ctype(op, left_exp, right_exp, full_types);
            let left = eval_static_integer_constant_exp_with_context_and_values(
                left_exp,
                struct_defs,
                full_types,
                static_const_values,
            )?;
            let left_true = static_integer_constant_is_true(left);
            if (matches!(op, BinaryOp::LogicalAnd) && !left_true)
                || (matches!(op, BinaryOp::LogicalOr) && left_true)
            {
                return Some(static_integer_constant(left_true as i64, false, false));
            }
            let right = eval_static_integer_constant_exp_with_context_and_values(
                right_exp,
                struct_defs,
                full_types,
                static_const_values,
            )?;
            if left.is_double || right.is_double {
                let use_float = (left.is_unsigned || !left.is_double)
                    && (right.is_unsigned || !right.is_double);
                let left_value = if left.is_double {
                    f64::from_bits(left.value as u64)
                } else if use_float {
                    left.value as f32 as f64
                } else {
                    left.value as f64
                };
                let right_value = if right.is_double {
                    f64::from_bits(right.value as u64)
                } else if use_float {
                    right.value as f32 as f64
                } else {
                    right.value as f64
                };
                return match op {
                    BinaryOp::Add => {
                        let value = if use_float {
                            (left_value + right_value) as f32 as f64
                        } else {
                            left_value + right_value
                        };
                        Some(static_integer_constant(
                            value.to_bits() as i64,
                            true,
                            use_float,
                        ))
                    }
                    BinaryOp::Sub => {
                        let value = if use_float {
                            (left_value - right_value) as f32 as f64
                        } else {
                            left_value - right_value
                        };
                        Some(static_integer_constant(
                            value.to_bits() as i64,
                            true,
                            use_float,
                        ))
                    }
                    BinaryOp::Mul => {
                        let value = if use_float {
                            (left_value * right_value) as f32 as f64
                        } else {
                            left_value * right_value
                        };
                        Some(static_integer_constant(
                            value.to_bits() as i64,
                            true,
                            use_float,
                        ))
                    }
                    BinaryOp::Div => {
                        let value = if use_float {
                            (left_value / right_value) as f32 as f64
                        } else {
                            left_value / right_value
                        };
                        Some(static_integer_constant(
                            value.to_bits() as i64,
                            true,
                            use_float,
                        ))
                    }
                    BinaryOp::LogicalAnd => Some(static_integer_constant(
                        (left_value != 0.0 && right_value != 0.0) as i64,
                        false,
                        false,
                    )),
                    BinaryOp::LogicalOr => Some(static_integer_constant(
                        (left_value != 0.0 || right_value != 0.0) as i64,
                        false,
                        false,
                    )),
                    BinaryOp::Equal => Some(static_integer_constant(
                        (left_value == right_value) as i64,
                        false,
                        false,
                    )),
                    BinaryOp::NotEqual => Some(static_integer_constant(
                        (left_value != right_value) as i64,
                        false,
                        false,
                    )),
                    BinaryOp::LessThan => Some(static_integer_constant(
                        (left_value < right_value) as i64,
                        false,
                        false,
                    )),
                    BinaryOp::GreaterThan => Some(static_integer_constant(
                        (left_value > right_value) as i64,
                        false,
                        false,
                    )),
                    BinaryOp::LessEqual => Some(static_integer_constant(
                        (left_value <= right_value) as i64,
                        false,
                        false,
                    )),
                    BinaryOp::GreaterEqual => Some(static_integer_constant(
                        (left_value >= right_value) as i64,
                        false,
                        false,
                    )),
                    _ => None,
                };
            }
            let is_unsigned = op_type.is_some_and(|ctype| !ctype.is_signed())
                || left.is_unsigned
                || right.is_unsigned;
            if is_unsigned {
                let op_type = op_type.unwrap_or(CType::ULong);
                let narrow = op_type.size() <= CType::UInt.size();
                let left_u = if narrow {
                    left.value as u32 as u64
                } else {
                    left.value as u64
                };
                let right_u = if narrow {
                    right.value as u32 as u64
                } else {
                    right.value as u64
                };
                let value = match op {
                    BinaryOp::BitwiseAnd => left_u & right_u,
                    BinaryOp::BitwiseNand => !(left_u & right_u),
                    BinaryOp::BitwiseOr => left_u | right_u,
                    BinaryOp::BitwiseXor => left_u ^ right_u,
                    BinaryOp::Equal => (left_u == right_u) as u64,
                    BinaryOp::NotEqual => (left_u != right_u) as u64,
                    BinaryOp::LessThan => (left_u < right_u) as u64,
                    BinaryOp::GreaterThan => (left_u > right_u) as u64,
                    BinaryOp::LessEqual => (left_u <= right_u) as u64,
                    BinaryOp::GreaterEqual => (left_u >= right_u) as u64,
                    _ => match op {
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
                            let amount = u32::try_from(right.value).ok()?;
                            left_u.checked_shl(amount)?
                        }
                        BinaryOp::ShiftRight => {
                            let amount = u32::try_from(right.value).ok()?;
                            left_u.checked_shr(amount)?
                        }
                        BinaryOp::LogicalAnd => (left_u != 0 && right_u != 0) as u64,
                        BinaryOp::LogicalOr => (left_u != 0 || right_u != 0) as u64,
                        _ => return None,
                    },
                };
                let value = if narrow {
                    value as u32 as i64
                } else {
                    value as i64
                };
                return Some(static_integer_constant(
                    value,
                    false,
                    !static_binary_op_yields_int(op),
                ));
            }
            let value = match op {
                BinaryOp::Add => left.value.wrapping_add(right.value),
                BinaryOp::Sub => left.value.wrapping_sub(right.value),
                BinaryOp::Mul => left.value.wrapping_mul(right.value),
                BinaryOp::Div => {
                    if right.value == 0 {
                        return None;
                    }
                    left.value.checked_div(right.value)?
                }
                BinaryOp::Mod => {
                    if right.value == 0 {
                        return None;
                    }
                    left.value.checked_rem(right.value)?
                }
                BinaryOp::BitwiseAnd => left.value & right.value,
                BinaryOp::BitwiseNand => !(left.value & right.value),
                BinaryOp::BitwiseOr => left.value | right.value,
                BinaryOp::BitwiseXor => left.value ^ right.value,
                BinaryOp::ShiftLeft => {
                    let amount = u32::try_from(right.value).ok()?;
                    left.value.checked_shl(amount)?
                }
                BinaryOp::ShiftRight => {
                    let amount = u32::try_from(right.value).ok()?;
                    left.value.checked_shr(amount)?
                }
                BinaryOp::LogicalAnd => (left.value != 0 && right.value != 0) as i64,
                BinaryOp::LogicalOr => (left.value != 0 || right.value != 0) as i64,
                BinaryOp::Equal => (left.value == right.value) as i64,
                BinaryOp::NotEqual => (left.value != right.value) as i64,
                BinaryOp::LessThan => (left.value < right.value) as i64,
                BinaryOp::GreaterThan => (left.value > right.value) as i64,
                BinaryOp::LessEqual => (left.value <= right.value) as i64,
                BinaryOp::GreaterEqual => (left.value >= right.value) as i64,
            };
            Some(static_integer_constant(
                value,
                false,
                is_unsigned && !static_binary_op_yields_int(op),
            ))
        }
        Exp::Conditional(cond, then_exp, else_exp) => {
            let cond = eval_static_integer_constant_exp_with_context_and_values(
                cond,
                struct_defs,
                full_types,
                static_const_values,
            )?;
            if cond.is_double {
                return None;
            }
            if cond.value != 0 {
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

#[allow(dead_code)]
pub(super) fn eval_static_integer_constant_exp(exp: &Exp) -> Option<StaticIntegerConstant> {
    eval_static_integer_constant_exp_with_context(exp, &IndexMap::new(), &IndexMap::new())
}

pub(super) fn cast_static_wide_integer(
    value: StaticWideIntegerConstant,
    target: CType,
) -> StaticWideIntegerConstant {
    let signed_value = value.value;
    let converted = match target {
        CType::Bool => {
            (if value.is_unsigned {
                value.value as u128 != 0
            } else {
                value.value != 0
            }) as i128
        }
        CType::Char | CType::SChar => signed_value as i8 as i128,
        CType::UChar => signed_value as u8 as i128,
        CType::Short => signed_value as i16 as i128,
        CType::UShort => signed_value as u16 as i128,
        CType::Int => signed_value as i32 as i128,
        CType::UInt => signed_value as u32 as i128,
        CType::Long => signed_value as i64 as i128,
        CType::ULong => signed_value as u64 as i128,
        CType::Int128 => signed_value,
        CType::UInt128 => signed_value as u128 as i128,
        _ => signed_value,
    };
    StaticWideIntegerConstant::new(converted, !target.is_signed())
}

pub(super) fn static_wide_integer_as_narrow_constant(
    value: StaticWideIntegerConstant,
    target: CType,
) -> StaticIntegerConstant {
    let converted = cast_static_wide_integer(value, target);
    static_integer_constant(converted.value as i64, false, !target.is_signed())
}

pub(super) fn eval_static_wide_integer_constant_exp_with_context_and_values(
    exp: &Exp,
    struct_defs: &IndexMap<String, StructDef>,
    full_types: &IndexMap<String, FullType>,
    static_const_values: &IndexMap<String, StaticScalarValue>,
    static_wide_const_values: &IndexMap<String, StaticWideIntegerConstant>,
) -> Option<StaticWideIntegerConstant> {
    match exp {
        Exp::Constant(c) | Exp::LongConstant(c) => {
            Some(StaticWideIntegerConstant::new(*c as i128, false))
        }
        Exp::UIntConstant(c) | Exp::ULongConstant(c) => {
            Some(StaticWideIntegerConstant::new(*c as u64 as i128, true))
        }
        Exp::Int128Constant(c) => Some(StaticWideIntegerConstant::new(*c, false)),
        Exp::UInt128Constant(c) => Some(StaticWideIntegerConstant::new(*c as i128, true)),
        Exp::Var(name) => static_wide_const_values.get(name).copied().or_else(|| {
            static_const_values.get(name).and_then(|value| {
                if value.source_is_double {
                    None
                } else if value.source_is_unsigned {
                    Some(StaticWideIntegerConstant::new(
                        value.value as u64 as i128,
                        true,
                    ))
                } else {
                    Some(StaticWideIntegerConstant::new(value.value as i128, false))
                }
            })
        }),
        Exp::SizeOf(inner) => {
            let ft = eval_static_expr_full_type(inner, full_types)?;
            if ft.contains_vla_placeholder_with(struct_defs) {
                return None;
            }
            Some(StaticWideIntegerConstant::new(
                i128::try_from(ft.checked_byte_size_with(struct_defs)?).ok()?,
                true,
            ))
        }
        Exp::SizeOfType(_, ft) if !ft.contains_vla_placeholder_with(struct_defs) => {
            Some(StaticWideIntegerConstant::new(
                i128::try_from(ft.checked_byte_size_with(struct_defs)?).ok()?,
                true,
            ))
        }
        Exp::SizeOfType(_, _) => None,
        Exp::AlignOfType(ft) => Some(StaticWideIntegerConstant::new(
            i128::try_from(ft.alignment_with(struct_defs)).ok()?,
            true,
        )),
        Exp::Cast(target, _, inner) => {
            let value = if let Exp::ArrayInit(elems) = inner.as_ref() {
                let [value] = elems.as_slice() else {
                    return None;
                };
                eval_static_wide_integer_constant_exp_with_context_and_values(
                    value,
                    struct_defs,
                    full_types,
                    static_const_values,
                    static_wide_const_values,
                )?
            } else {
                eval_static_wide_integer_constant_exp_with_context_and_values(
                    inner,
                    struct_defs,
                    full_types,
                    static_const_values,
                    static_wide_const_values,
                )?
            };
            if target.is_floating() {
                None
            } else {
                Some(cast_static_wide_integer(value, *target))
            }
        }
        Exp::Unary(op, inner) => {
            let value = eval_static_wide_integer_constant_exp_with_context_and_values(
                inner,
                struct_defs,
                full_types,
                static_const_values,
                static_wide_const_values,
            )?;
            match op {
                UnaryOp::Negate => {
                    if value.is_unsigned {
                        Some(StaticWideIntegerConstant::new(
                            (value.value as u128).wrapping_neg() as i128,
                            true,
                        ))
                    } else {
                        Some(StaticWideIntegerConstant::new(
                            value.value.wrapping_neg(),
                            false,
                        ))
                    }
                }
                UnaryOp::Complement => Some(StaticWideIntegerConstant::new(
                    !value.value,
                    value.is_unsigned,
                )),
                UnaryOp::LogicalNot => Some(StaticWideIntegerConstant::new(
                    value.is_zero() as i128,
                    false,
                )),
                _ => None,
            }
        }
        Exp::Binary(op, left_exp, right_exp) => {
            let op_type =
                eval_static_integer_binary_operand_ctype(op, left_exp, right_exp, full_types);
            let left = eval_static_wide_integer_constant_exp_with_context_and_values(
                left_exp,
                struct_defs,
                full_types,
                static_const_values,
                static_wide_const_values,
            )?;
            if (matches!(op, BinaryOp::LogicalAnd) && left.is_zero())
                || (matches!(op, BinaryOp::LogicalOr) && !left.is_zero())
            {
                return Some(StaticWideIntegerConstant::new(
                    (!left.is_zero()) as i128,
                    false,
                ));
            }
            let right = eval_static_wide_integer_constant_exp_with_context_and_values(
                right_exp,
                struct_defs,
                full_types,
                static_const_values,
                static_wide_const_values,
            )?;
            let unsigned = op_type.is_some_and(|ctype| !ctype.is_signed())
                || left.is_unsigned
                || right.is_unsigned;
            let result_unsigned = unsigned && !static_binary_op_yields_int(op);
            if unsigned {
                let l = left.value as u128;
                let r = right.value as u128;
                let value = match op {
                    BinaryOp::Add => l.wrapping_add(r),
                    BinaryOp::Sub => l.wrapping_sub(r),
                    BinaryOp::Mul => l.wrapping_mul(r),
                    BinaryOp::Div => l.checked_div(r)?,
                    BinaryOp::Mod => {
                        if r == 0 {
                            return None;
                        }
                        l % r
                    }
                    BinaryOp::BitwiseAnd => l & r,
                    BinaryOp::BitwiseNand => !(l & r),
                    BinaryOp::BitwiseOr => l | r,
                    BinaryOp::BitwiseXor => l ^ r,
                    BinaryOp::ShiftLeft => {
                        let amount = u32::try_from(right.value).ok()?;
                        l.checked_shl(amount)?
                    }
                    BinaryOp::ShiftRight => {
                        let amount = u32::try_from(right.value).ok()?;
                        l.checked_shr(amount)?
                    }
                    BinaryOp::LogicalAnd => (!left.is_zero() && !right.is_zero()) as u128,
                    BinaryOp::LogicalOr => (!left.is_zero() || !right.is_zero()) as u128,
                    BinaryOp::Equal => (l == r) as u128,
                    BinaryOp::NotEqual => (l != r) as u128,
                    BinaryOp::LessThan => (l < r) as u128,
                    BinaryOp::GreaterThan => (l > r) as u128,
                    BinaryOp::LessEqual => (l <= r) as u128,
                    BinaryOp::GreaterEqual => (l >= r) as u128,
                };
                return Some(StaticWideIntegerConstant::new(
                    value as i128,
                    result_unsigned,
                ));
            }
            let l = left.value;
            let r = right.value;
            let value = match op {
                BinaryOp::Add => l.wrapping_add(r),
                BinaryOp::Sub => l.wrapping_sub(r),
                BinaryOp::Mul => l.wrapping_mul(r),
                BinaryOp::Div => {
                    if r == 0 {
                        return None;
                    }
                    l.checked_div(r)?
                }
                BinaryOp::Mod => {
                    if r == 0 {
                        return None;
                    }
                    l.checked_rem(r)?
                }
                BinaryOp::BitwiseAnd => l & r,
                BinaryOp::BitwiseNand => !(l & r),
                BinaryOp::BitwiseOr => l | r,
                BinaryOp::BitwiseXor => l ^ r,
                BinaryOp::ShiftLeft => {
                    let amount = u32::try_from(r).ok()?;
                    l.checked_shl(amount)?
                }
                BinaryOp::ShiftRight => {
                    let amount = u32::try_from(r).ok()?;
                    l.checked_shr(amount)?
                }
                BinaryOp::LogicalAnd => (l != 0 && r != 0) as i128,
                BinaryOp::LogicalOr => (l != 0 || r != 0) as i128,
                BinaryOp::Equal => (l == r) as i128,
                BinaryOp::NotEqual => (l != r) as i128,
                BinaryOp::LessThan => (l < r) as i128,
                BinaryOp::GreaterThan => (l > r) as i128,
                BinaryOp::LessEqual => (l <= r) as i128,
                BinaryOp::GreaterEqual => (l >= r) as i128,
            };
            Some(StaticWideIntegerConstant::new(value, result_unsigned))
        }
        Exp::Conditional(cond, then_exp, else_exp) => {
            let cond = eval_static_wide_integer_constant_exp_with_context_and_values(
                cond,
                struct_defs,
                full_types,
                static_const_values,
                static_wide_const_values,
            )?;
            if !cond.is_zero() {
                eval_static_wide_integer_constant_exp_with_context_and_values(
                    then_exp,
                    struct_defs,
                    full_types,
                    static_const_values,
                    static_wide_const_values,
                )
            } else {
                eval_static_wide_integer_constant_exp_with_context_and_values(
                    else_exp,
                    struct_defs,
                    full_types,
                    static_const_values,
                    static_wide_const_values,
                )
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignof_rejects_values_that_do_not_fit_the_constant_type() {
        let mut struct_defs = IndexMap::new();
        struct_defs.insert(
            "HugeAlign".to_string(),
            StructDef {
                tag: "HugeAlign".to_string(),
                members: Vec::new(),
                size: 0,
                alignment: i64::MAX as usize + 1,
                is_union: false,
            },
        );
        let expression = Exp::AlignOfType(FullType::Struct("HugeAlign".to_string()));

        assert!(eval_static_integer_constant_exp_with_context(
            &expression,
            &struct_defs,
            &IndexMap::new(),
        )
        .is_none());
    }
}
