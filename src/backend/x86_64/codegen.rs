use crate::types::*;
use indexmap::IndexMap;
use std::collections::HashMap;

const ARG_REGISTERS: [Reg; 6] = [Reg::DI, Reg::SI, Reg::DX, Reg::CX, Reg::R8, Reg::R9];
const ARG_SSE_REGISTERS: usize = 8;

// ============================================================
// Phase 1: TACKY → Assembly (with pseudo-registers)
// ============================================================

fn convert_val(val: &TackyVal) -> AsmOperand {
    match val {
        TackyVal::Constant(c) => AsmOperand::Imm(*c),
        TackyVal::Int128Constant(c) => AsmOperand::Imm(*c as i64),
        TackyVal::UInt128Constant(c) => AsmOperand::Imm(*c as i64),
        TackyVal::DoubleConstant(d) => {
            // Treat as integer bits when used in non-double context
            AsmOperand::Imm(d.to_bits() as i64)
        }
        TackyVal::Var(name) => AsmOperand::Pseudo(name.clone()),
    }
}

fn val_type(val: &TackyVal, types: &IndexMap<String, CType>) -> AsmType {
    match val {
        TackyVal::Constant(c) => {
            if *c > i32::MAX as i64 || *c < i32::MIN as i64 {
                AsmType::Quadword
            } else {
                AsmType::Longword
            }
        }
        TackyVal::Int128Constant(_) | TackyVal::UInt128Constant(_) => AsmType::Octword,
        TackyVal::DoubleConstant(_) => AsmType::Double,
        TackyVal::Var(name) => {
            let ct = types.get(name).copied().unwrap_or(CType::Int);
            ct.into()
        }
    }
}

fn promoted_cmp_operand(
    out: &mut Vec<AsmInstr>,
    val: &TackyVal,
    cmp_type: AsmType,
    scratch: Reg,
    types: &IndexMap<String, CType>,
) -> AsmOperand {
    if !matches!(cmp_type, AsmType::Longword | AsmType::Quadword) {
        return convert_val(val);
    }
    let TackyVal::Var(name) = val else {
        return convert_val(val);
    };
    let Some(ctype) = types.get(name).copied() else {
        return convert_val(val);
    };
    let src_t: AsmType = ctype.into();
    if !matches!(src_t, AsmType::Byte | AsmType::Word) {
        return convert_val(val);
    }
    let dst = AsmOperand::Reg(scratch);
    if ctype.is_signed() {
        out.push(AsmInstr::Movsx(
            src_t,
            cmp_type,
            convert_val(val),
            dst.clone(),
        ));
    } else {
        out.push(AsmInstr::MovZeroExtend(
            src_t,
            cmp_type,
            convert_val(val),
            dst.clone(),
        ));
    }
    dst
}

/// For doubles, we use unsigned condition codes (above/below) because
/// comisd sets CF/ZF like unsigned comparisons
fn is_comparison(op: &TackyBinaryOp, is_unsigned: bool) -> Option<CondCode> {
    match (op, is_unsigned) {
        (TackyBinaryOp::Equal, _) => Some(CondCode::E),
        (TackyBinaryOp::NotEqual, _) => Some(CondCode::NE),
        (TackyBinaryOp::LessThan, false) => Some(CondCode::L),
        (TackyBinaryOp::LessEqual, false) => Some(CondCode::LE),
        (TackyBinaryOp::GreaterThan, false) => Some(CondCode::G),
        (TackyBinaryOp::GreaterEqual, false) => Some(CondCode::GE),
        (TackyBinaryOp::LessThan, true) => Some(CondCode::B),
        (TackyBinaryOp::LessEqual, true) => Some(CondCode::BE),
        (TackyBinaryOp::GreaterThan, true) => Some(CondCode::A),
        (TackyBinaryOp::GreaterEqual, true) => Some(CondCode::AE),
        _ => None,
    }
}

fn emit_float_comparison_result(out: &mut Vec<AsmInstr>, op: &TackyBinaryOp, dst: AsmOperand) {
    let cmp_cc = match op {
        TackyBinaryOp::Equal => CondCode::E,
        TackyBinaryOp::NotEqual => CondCode::NE,
        TackyBinaryOp::LessThan => CondCode::B,
        TackyBinaryOp::LessEqual => CondCode::BE,
        TackyBinaryOp::GreaterThan => CondCode::A,
        TackyBinaryOp::GreaterEqual => CondCode::AE,
        _ => unreachable!("not a floating comparison"),
    };
    out.push(AsmInstr::Mov(
        AsmType::Longword,
        AsmOperand::Imm(0),
        dst.clone(),
    ));
    out.push(AsmInstr::SetCC(cmp_cc, dst.clone()));
    out.push(AsmInstr::SetCC(
        if matches!(op, TackyBinaryOp::NotEqual) {
            CondCode::P
        } else {
            CondCode::NP
        },
        AsmOperand::Reg(Reg::R10),
    ));
    out.push(AsmInstr::Binary(
        AsmType::Byte,
        if matches!(op, TackyBinaryOp::NotEqual) {
            AsmBinaryOp::Or
        } else {
            AsmBinaryOp::And
        },
        AsmOperand::Reg(Reg::R10),
        dst,
    ));
}

// Double constants need to be emitted as static data and referenced by label
fn double_const_label(static_doubles: &[(String, f64)]) -> String {
    format!("__double_const_{}", static_doubles.len())
}

fn float_const_label(static_floats: &[(String, f32)]) -> String {
    format!("__float_const_{}", static_floats.len())
}

fn intern_double_const(static_doubles: &mut Vec<(String, f64)>, value: f64) -> String {
    let bits = value.to_bits();
    if let Some((label, _)) = static_doubles
        .iter()
        .find(|(_, existing)| existing.to_bits() == bits)
    {
        return label.clone();
    }
    let label = double_const_label(static_doubles);
    static_doubles.push((label.clone(), value));
    label
}

fn intern_float_const(static_floats: &mut Vec<(String, f32)>, value: f32) -> String {
    let bits = value.to_bits();
    if let Some((label, _)) = static_floats
        .iter()
        .find(|(_, existing)| existing.to_bits() == bits)
    {
        return label.clone();
    }
    let label = float_const_label(static_floats);
    static_floats.push((label.clone(), value));
    label
}

/// Convert a TackyVal for doubles, emitting a data label for double constants
fn convert_double_val(val: &TackyVal, static_doubles: &mut Vec<(String, f64)>) -> AsmOperand {
    match val {
        TackyVal::DoubleConstant(d) => {
            let label = intern_double_const(static_doubles, *d);
            AsmOperand::Data(label)
        }
        TackyVal::Constant(c) => AsmOperand::Imm(*c),
        TackyVal::Int128Constant(c) => AsmOperand::Imm(*c as i64),
        TackyVal::UInt128Constant(c) => AsmOperand::Imm(*c as i64),
        TackyVal::Var(name) => AsmOperand::Pseudo(name.clone()),
    }
}

fn convert_float_return_val(val: &TackyVal, static_floats: &mut Vec<(String, f32)>) -> AsmOperand {
    match val {
        TackyVal::DoubleConstant(d) => {
            AsmOperand::Data(intern_float_const(static_floats, *d as f32))
        }
        TackyVal::Constant(c) => AsmOperand::Data(intern_float_const(static_floats, *c as f32)),
        TackyVal::Var(name) => AsmOperand::Pseudo(name.clone()),
        TackyVal::Int128Constant(c) => {
            AsmOperand::Data(intern_float_const(static_floats, *c as f32))
        }
        TackyVal::UInt128Constant(c) => {
            AsmOperand::Data(intern_float_const(static_floats, *c as f32))
        }
    }
}

fn convert_double_return_val(
    val: &TackyVal,
    static_doubles: &mut Vec<(String, f64)>,
) -> AsmOperand {
    match val {
        TackyVal::DoubleConstant(d) => AsmOperand::Data(intern_double_const(static_doubles, *d)),
        TackyVal::Constant(c) => AsmOperand::Data(intern_double_const(static_doubles, *c as f64)),
        TackyVal::Var(name) => AsmOperand::Pseudo(name.clone()),
        TackyVal::Int128Constant(c) => {
            AsmOperand::Data(intern_double_const(static_doubles, *c as f64))
        }
        TackyVal::UInt128Constant(c) => {
            AsmOperand::Data(intern_double_const(static_doubles, *c as f64))
        }
    }
}

fn convert_floating_val(
    ty: AsmType,
    val: &TackyVal,
    static_doubles: &mut Vec<(String, f64)>,
    static_floats: &mut Vec<(String, f32)>,
) -> AsmOperand {
    if ty == AsmType::Float {
        convert_float_return_val(val, static_floats)
    } else {
        convert_double_val(val, static_doubles)
    }
}

fn is_positive_float_zero_return(t: AsmType, val: &TackyVal) -> bool {
    match (t, val) {
        (_, TackyVal::Constant(0))
        | (_, TackyVal::Int128Constant(0))
        | (_, TackyVal::UInt128Constant(0)) => true,
        (AsmType::Float, TackyVal::DoubleConstant(d)) => (*d as f32).to_bits() == 0,
        (AsmType::Double, TackyVal::DoubleConstant(d)) => d.to_bits() == 0,
        _ => false,
    }
}

fn x87_load_val(
    out: &mut Vec<AsmInstr>,
    val: &TackyVal,
    types: &IndexMap<String, CType>,
    static_doubles: &mut Vec<(String, f64)>,
) {
    let ty = val_type(val, types);
    match val {
        TackyVal::DoubleConstant(_) => {
            out.push(AsmInstr::X87Load(
                AsmType::Double,
                convert_double_val(val, static_doubles),
            ));
        }
        _ if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) => {
            out.push(AsmInstr::X87Load(ty, convert_val(val)));
        }
        _ => {
            out.push(AsmInstr::X87Load(ty, convert_val(val)));
        }
    }
}

fn x87_copy_to_long_double(
    out: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst: &TackyVal,
    types: &IndexMap<String, CType>,
    static_doubles: &mut Vec<(String, f64)>,
    label_counter: &mut usize,
    function_name: &str,
) {
    let src_t = val_type(src, types);
    if is_unsigned_val(src, types) {
        if matches!(src_t, AsmType::Byte | AsmType::Word | AsmType::Longword) {
            out.push(AsmInstr::MovZeroExtend(
                src_t,
                AsmType::Quadword,
                convert_val(src),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::X87Load(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::X87Store(convert_val(dst)));
            return;
        }
        if src_t == AsmType::Quadword {
            let base = *label_counter;
            *label_counter += 1;
            let ok_label = format!("uint_to_long_double_ok.{}.{}", function_name, base);
            let end_label = format!("uint_to_long_double_end.{}.{}", function_name, base);
            out.push(AsmInstr::Cmp(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                convert_val(src),
            ));
            out.push(AsmInstr::JmpCC(CondCode::GE, ok_label.clone()));
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(src),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::X87Load(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R10),
            ));
            let bias = intern_double_const(static_doubles, 9223372036854775808.0);
            out.push(AsmInstr::X87Load(AsmType::Double, AsmOperand::Data(bias)));
            out.push(AsmInstr::X87Binary(AsmX87BinaryOp::Add));
            out.push(AsmInstr::X87Store(convert_val(dst)));
            out.push(AsmInstr::Jmp(end_label.clone()));
            out.push(AsmInstr::Label(ok_label));
            out.push(AsmInstr::X87Load(src_t, convert_val(src)));
            out.push(AsmInstr::X87Store(convert_val(dst)));
            out.push(AsmInstr::Label(end_label));
            return;
        }
    }
    x87_load_val(out, src, types, static_doubles);
    out.push(AsmInstr::X87Store(convert_val(dst)));
}

fn low64_operand(op: AsmOperand) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Pseudo(name) => Ok(AsmOperand::PseudoMem(name, 0)),
        AsmOperand::PseudoMem(name, offset) => Ok(AsmOperand::PseudoMem(name, offset)),
        AsmOperand::Reg(reg) => Ok(AsmOperand::Reg(reg)),
        AsmOperand::Imm(value) => Ok(AsmOperand::Imm(value)),
        other => Err(format!(
            "x86-64 backend cannot address low half of {:?}",
            other
        )),
    }
}

fn high64_operand(op: AsmOperand) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Pseudo(name) => Ok(AsmOperand::PseudoMem(name, 8)),
        AsmOperand::PseudoMem(name, offset) => Ok(AsmOperand::PseudoMem(name, offset + 8)),
        AsmOperand::Reg(Reg::AX) => Ok(AsmOperand::Reg(Reg::DX)),
        AsmOperand::Reg(reg) => Err(format!(
            "x86-64 backend cannot address high half of 128-bit register {:?}",
            reg
        )),
        AsmOperand::Imm(_) => {
            Err("x86-64 backend cannot address high half of immediate".to_string())
        }
        other => Err(format!(
            "x86-64 backend cannot address high half of {:?}",
            other
        )),
    }
}

fn i128_part_operands(val: &TackyVal) -> Result<(AsmOperand, AsmOperand), String> {
    match val {
        TackyVal::Constant(value) => Ok((
            AsmOperand::Imm(*value),
            AsmOperand::Imm(if *value < 0 { -1 } else { 0 }),
        )),
        TackyVal::Int128Constant(value) => {
            let (low, high) = crate::backend::common::i128_parts_signed(*value);
            Ok((AsmOperand::Imm(low), AsmOperand::Imm(high)))
        }
        TackyVal::UInt128Constant(value) => {
            let (low, high) = crate::backend::common::i128_parts_unsigned(*value);
            Ok((AsmOperand::Imm(low), AsmOperand::Imm(high)))
        }
        _ => {
            let op = convert_val(val);
            Ok((low64_operand(op.clone())?, high64_operand(op)?))
        }
    }
}

fn emit_i128_copy(out: &mut Vec<AsmInstr>, src: &TackyVal, dst: &TackyVal) -> Result<(), String> {
    let (src_low, src_high) = i128_part_operands(src)?;
    let dst_op = convert_val(dst);
    emit_i128_parts_to_operands(
        out,
        src_low,
        src_high,
        low64_operand(dst_op.clone())?,
        high64_operand(dst_op)?,
    );
    Ok(())
}

fn emit_i128_zero(out: &mut Vec<AsmInstr>, dst: &TackyVal) -> Result<(), String> {
    let dst_op = convert_val(dst);
    emit_i128_parts_to_operands(
        out,
        AsmOperand::Imm(0),
        AsmOperand::Imm(0),
        low64_operand(dst_op.clone())?,
        high64_operand(dst_op)?,
    );
    Ok(())
}

fn i128_constant_is_zero(value: &TackyVal) -> bool {
    matches!(
        value,
        TackyVal::Constant(0) | TackyVal::Int128Constant(0) | TackyVal::UInt128Constant(0)
    )
}

fn i128_constant_is_one(value: &TackyVal) -> bool {
    matches!(
        value,
        TackyVal::Constant(1) | TackyVal::Int128Constant(1) | TackyVal::UInt128Constant(1)
    )
}

fn i128_constant_is_all_ones(value: &TackyVal) -> bool {
    matches!(
        value,
        TackyVal::Constant(-1)
            | TackyVal::Int128Constant(-1)
            | TackyVal::UInt128Constant(u128::MAX)
    )
}

fn emit_i128_parts_to_operands(
    out: &mut Vec<AsmInstr>,
    low: AsmOperand,
    high: AsmOperand,
    dst_low: AsmOperand,
    dst_high: AsmOperand,
) {
    if low != dst_low {
        out.push(AsmInstr::Mov(AsmType::Quadword, low, dst_low));
    }
    if high != dst_high {
        out.push(AsmInstr::Mov(AsmType::Quadword, high, dst_high));
    }
}

fn clamp_floating_to_signed_int_overflow(
    out: &mut Vec<AsmInstr>,
    src_ty: AsmType,
    src: &TackyVal,
    dst: &TackyVal,
    static_doubles: &mut Vec<(String, f64)>,
    label_counter: &mut usize,
    function_name: &str,
) {
    let base = *label_counter;
    *label_counter += 1;
    let end_label = format!("float_to_int_ok.{}.{}", function_name, base);
    let src_op = if src_ty == AsmType::Float {
        let tmp = AsmOperand::Xmm(XmmReg::XMM14);
        out.push(AsmInstr::Cvtss2sd(convert_val(src), tmp.clone()));
        tmp
    } else {
        convert_double_val(src, static_doubles)
    };
    let max_label = intern_double_const(static_doubles, 2147483648.0);
    out.push(AsmInstr::Cmp(
        AsmType::Double,
        AsmOperand::Data(max_label),
        src_op,
    ));
    out.push(AsmInstr::JmpCC(CondCode::B, end_label.clone()));
    out.push(AsmInstr::Mov(
        AsmType::Longword,
        AsmOperand::Imm(i32::MAX as i64),
        convert_val(dst),
    ));
    out.push(AsmInstr::Label(end_label));
}

fn is_unsigned_val(val: &TackyVal, types: &IndexMap<String, CType>) -> bool {
    match val {
        TackyVal::UInt128Constant(_) => true,
        TackyVal::Int128Constant(_) | TackyVal::Constant(_) | TackyVal::DoubleConstant(_) => false,
        TackyVal::Var(name) => types
            .get(name)
            .is_some_and(|ctype| ctype != &CType::Double && !ctype.is_signed()),
    }
}

struct LabelContext<'a> {
    function_name: &'a str,
    counter: &'a mut usize,
}

fn emit_i128_variable_shift(
    out: &mut Vec<AsmInstr>,
    labels: &mut LabelContext<'_>,
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    types: &IndexMap<String, CType>,
) -> Result<(), String> {
    let (left_low, left_high) = i128_part_operands(left)?;
    let dst_op = convert_val(dst);
    let right_ty = val_type(right, types);
    let amount_src = if right_ty == AsmType::Octword {
        low64_operand(convert_val(right))?
    } else {
        convert_val(right)
    };
    let id = *labels.counter;
    *labels.counter += 1;
    let loop_label = format!("i128_shift_loop.{}.{}", labels.function_name, id);
    let end_label = format!("i128_shift_end.{}.{}", labels.function_name, id);

    out.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_low,
        AsmOperand::Reg(Reg::R10),
    ));
    out.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_high,
        AsmOperand::Reg(Reg::R11),
    ));
    out.push(AsmInstr::Mov(
        AsmType::Longword,
        amount_src,
        AsmOperand::Reg(Reg::CX),
    ));
    out.push(AsmInstr::Label(loop_label.clone()));
    out.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::CX),
    ));
    out.push(AsmInstr::JmpCC(CondCode::E, end_label.clone()));

    match op {
        TackyBinaryOp::ShiftLeft => {
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R9),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Imm(63),
                AsmOperand::Reg(Reg::R9),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Or,
                AsmOperand::Reg(Reg::R9),
                AsmOperand::Reg(Reg::R11),
            ));
        }
        TackyBinaryOp::ShiftRight => {
            let high_shift = if is_unsigned_val(left, types) {
                AsmBinaryOp::Shr
            } else {
                AsmBinaryOp::Sar
            };
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R11),
                AsmOperand::Reg(Reg::R9),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Imm(63),
                AsmOperand::Reg(Reg::R9),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                high_shift,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Or,
                AsmOperand::Reg(Reg::R9),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        _ => return Err("internal error: expected i128 shift op".to_string()),
    }

    out.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Sub,
        AsmOperand::Imm(1),
        AsmOperand::Reg(Reg::CX),
    ));
    out.push(AsmInstr::Jmp(loop_label));
    out.push(AsmInstr::Label(end_label));
    emit_i128_parts_to_operands(
        out,
        AsmOperand::Reg(Reg::R10),
        AsmOperand::Reg(Reg::R11),
        low64_operand(dst_op.clone())?,
        high64_operand(dst_op)?,
    );
    Ok(())
}

fn i128_constant_shift_amount(val: &TackyVal) -> Option<i64> {
    match val {
        TackyVal::Constant(value) => Some(*value),
        TackyVal::Int128Constant(value) => i64::try_from(*value).ok(),
        TackyVal::UInt128Constant(value) => i64::try_from(*value).ok(),
        TackyVal::DoubleConstant(_) | TackyVal::Var(_) => None,
    }
}

fn emit_i128_load(
    out: &mut Vec<AsmInstr>,
    src_ptr: &TackyVal,
    dst: &TackyVal,
) -> Result<(), String> {
    let dst_op = convert_val(dst);
    let src_ptr_op = convert_val(src_ptr);
    out.push(AsmInstr::Mov(
        AsmType::Quadword,
        src_ptr_op.clone(),
        AsmOperand::Reg(Reg::R11),
    ));
    out.push(AsmInstr::LoadIndirect(
        AsmType::Quadword,
        Reg::R11,
        low64_operand(dst_op.clone())?,
    ));
    out.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Add,
        AsmOperand::Imm(8),
        AsmOperand::Reg(Reg::R11),
    ));
    out.push(AsmInstr::LoadIndirect(
        AsmType::Quadword,
        Reg::R11,
        high64_operand(dst_op)?,
    ));
    Ok(())
}

fn emit_i128_store(
    out: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst_ptr: &TackyVal,
) -> Result<(), String> {
    let (src_low, src_high) = i128_part_operands(src)?;
    let dst_ptr_op = convert_val(dst_ptr);
    out.push(AsmInstr::Mov(
        AsmType::Quadword,
        dst_ptr_op,
        AsmOperand::Reg(Reg::R11),
    ));
    out.push(AsmInstr::StoreIndirect(
        AsmType::Quadword,
        src_low,
        Reg::R11,
    ));
    out.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Add,
        AsmOperand::Imm(8),
        AsmOperand::Reg(Reg::R11),
    ));
    out.push(AsmInstr::StoreIndirect(
        AsmType::Quadword,
        src_high,
        Reg::R11,
    ));
    Ok(())
}

fn get_struct_def<'a>(
    name: &str,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &'a IndexMap<String, StructDef>,
) -> Option<&'a StructDef> {
    var_struct_tags
        .get(name)
        .and_then(|tag| struct_defs.get(tag))
}

fn get_struct_classes(
    name: &str,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &IndexMap<String, StructDef>,
) -> Option<Vec<ParamClass>> {
    if let Some(tag) = var_struct_tags.get(name) {
        if let Some(def) = struct_defs.get(tag) {
            return Some(def.classify_with(struct_defs));
        }
    }
    None
}

#[derive(Clone, Copy)]
struct StackArgLayout {
    align: usize,
    size: usize,
}

impl StackArgLayout {
    fn for_scalar(t: AsmType) -> Self {
        match t {
            AsmType::LongDouble | AsmType::Octword => Self {
                align: 16,
                size: 16,
            },
            _ => Self { align: 8, size: 8 },
        }
    }

    fn for_memory_block(size: usize, align: usize) -> Self {
        Self {
            align: align.clamp(1, 16),
            size: size.next_multiple_of(8),
        }
    }

    fn place_at(self, offset: usize) -> usize {
        offset.next_multiple_of(self.align)
    }
}

struct InstructionContext<'a> {
    function_name: &'a str,
    return_type: CType,
    target: &'a Target,
    types: &'a IndexMap<String, CType>,
    out: &'a mut Vec<AsmInstr>,
    static_doubles: &'a mut Vec<(String, f64)>,
    static_floats: &'a mut Vec<(String, f32)>,
    label_counter: &'a mut usize,
    var_struct_tags: &'a HashMap<String, String>,
    struct_defs: &'a IndexMap<String, StructDef>,
    local_function_names: &'a std::collections::HashSet<String>,
    va_start_stack_offset: i32,
}

fn convert_instruction(instr: &TackyInstr, ctx: &mut InstructionContext<'_>) -> Result<(), String> {
    let function_name = ctx.function_name;
    let target = ctx.target;
    let types = ctx.types;
    let out = &mut *ctx.out;
    let static_doubles = &mut *ctx.static_doubles;
    let static_floats = &mut *ctx.static_floats;
    let label_counter = &mut *ctx.label_counter;
    let var_struct_tags = ctx.var_struct_tags;
    let struct_defs = ctx.struct_defs;
    let local_function_names = ctx.local_function_names;
    let va_start_stack_offset = ctx.va_start_stack_offset;
    match instr {
        TackyInstr::Nop => { /* skip */ }
        TackyInstr::Unreachable => {
            out.push(AsmInstr::Unreachable);
        }
        TackyInstr::AtomicFence => {
            out.push(AsmInstr::AtomicFence);
        }
        TackyInstr::AtomicFetch {
            op,
            ptr,
            arg,
            return_old,
            dst,
        } => {
            let t = val_type(dst, types);
            if matches!(t, AsmType::Float | AsmType::Double) {
                return Err("x86-64 backend cannot atomic-fetch floating values".to_string());
            }
            let asm_op = match op {
                TackyBinaryOp::Add => AsmBinaryOp::Add,
                TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                TackyBinaryOp::BitwiseNand => AsmBinaryOp::Nand,
                TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                _ => return Err(format!("unsupported x86-64 atomic fetch op: {:?}", op)),
            };
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                t,
                convert_val(arg),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicRmw(
                t,
                asm_op,
                *return_old,
                convert_val(dst),
            ));
        }
        TackyInstr::AtomicExchange { ptr, value, dst } => {
            let t = val_type(dst, types);
            if matches!(t, AsmType::Float | AsmType::Double) {
                return Err("x86-64 backend cannot atomic-exchange floating values".to_string());
            }
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                t,
                convert_val(value),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicExchange(t, convert_val(dst)));
        }
        TackyInstr::AtomicCompareExchange {
            ptr,
            expected,
            desired,
            dst,
        } => {
            let desired_ty = val_type(desired, types);
            if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                return Err(
                    "x86-64 backend cannot atomic-compare-exchange floating values".to_string(),
                );
            }
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(expected),
                AsmOperand::Reg(Reg::R12),
            ));
            out.push(AsmInstr::Mov(
                desired_ty,
                convert_val(desired),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicCompareExchange(
                desired_ty,
                convert_val(dst),
            ));
        }
        TackyInstr::AtomicCompareSwap {
            ptr,
            expected,
            desired,
            return_old,
            dst,
        } => {
            let desired_ty = val_type(desired, types);
            if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                return Err(
                    "x86-64 backend cannot sync compare-and-swap floating values".to_string(),
                );
            }
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                desired_ty,
                convert_val(expected),
                AsmOperand::Reg(Reg::R12),
            ));
            out.push(AsmInstr::Mov(
                desired_ty,
                convert_val(desired),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicCompareSwap(
                desired_ty,
                *return_old,
                convert_val(dst),
            ));
        }
        TackyInstr::CopyStruct { src_name, dst_name } => {
            // Emit bytewise copy for struct-to-struct assignment
            let struct_size = get_struct_def(dst_name, var_struct_tags, struct_defs)
                .map(|d| d.size)
                .unwrap_or(0);
            let mut off = 0i32;
            while (off as usize) + 8 <= struct_size {
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::PseudoMem(src_name.clone(), off),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::PseudoMem(dst_name.clone(), off),
                ));
                off += 8;
            }
            while (off as usize) + 4 <= struct_size {
                out.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::PseudoMem(src_name.clone(), off),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::PseudoMem(dst_name.clone(), off),
                ));
                off += 4;
            }
            while (off as usize) < struct_size {
                out.push(AsmInstr::Mov(
                    AsmType::Byte,
                    AsmOperand::PseudoMem(src_name.clone(), off),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Byte,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::PseudoMem(dst_name.clone(), off),
                ));
                off += 1;
            }
        }
        TackyInstr::Return(val) => {
            let t = match val {
                TackyVal::Var(_) => val_type(val, types),
                _ => ctx.return_type.into(),
            };
            // Check if returning a struct
            if let TackyVal::Var(ref name) = val {
                if types.get(name).copied() == Some(CType::Struct) {
                    if let Some(classes) = get_struct_classes(name, var_struct_tags, struct_defs) {
                        let mut int_ret_idx = 0;
                        let mut sse_ret_idx = 0;
                        let int_ret_regs = [Reg::AX, Reg::DX];
                        let sse_ret_regs = [XmmReg::XMM0, XmmReg::XMM1];
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i32;
                            match class {
                                ParamClass::Sse if sse_ret_idx < 2 => {
                                    out.push(AsmInstr::Mov(
                                        AsmType::Double,
                                        AsmOperand::PseudoMem(name.clone(), eb_offset),
                                        AsmOperand::Xmm(sse_ret_regs[sse_ret_idx]),
                                    ));
                                    sse_ret_idx += 1;
                                }
                                ParamClass::Integer if int_ret_idx < 2 => {
                                    out.push(AsmInstr::Mov(
                                        AsmType::Quadword,
                                        AsmOperand::PseudoMem(name.clone(), eb_offset),
                                        AsmOperand::Reg(int_ret_regs[int_ret_idx]),
                                    ));
                                    int_ret_idx += 1;
                                }
                                _ => {}
                            }
                        }
                    }
                    out.push(AsmInstr::Ret);
                    return Ok(());
                }
            }
            if t == AsmType::LongDouble {
                x87_load_val(out, val, types, static_doubles);
            } else if matches!(t, AsmType::Float | AsmType::Double) {
                if is_positive_float_zero_return(t, val) {
                    out.push(AsmInstr::Binary(
                        t,
                        AsmBinaryOp::Xor,
                        AsmOperand::Xmm(XmmReg::XMM0),
                        AsmOperand::Xmm(XmmReg::XMM0),
                    ));
                } else {
                    let src = match (t, val) {
                        (AsmType::Float, _) => convert_float_return_val(val, static_floats),
                        (AsmType::Double, _) => convert_double_return_val(val, static_doubles),
                        _ => unreachable!("non-floating return type handled above"),
                    };
                    out.push(AsmInstr::Mov(t, src, AsmOperand::Xmm(XmmReg::XMM0)));
                }
            } else if t == AsmType::Octword {
                let (low, high) = i128_part_operands(val)?;
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    low,
                    AsmOperand::Reg(Reg::AX),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    high,
                    AsmOperand::Reg(Reg::DX),
                ));
            } else {
                out.push(AsmInstr::Mov(t, convert_val(val), AsmOperand::Reg(Reg::AX)));
            }
            out.push(AsmInstr::Ret);
        }
        TackyInstr::SignExtend { src, dst } => {
            let src_t = val_type(src, types);
            let dst_t = val_type(dst, types);
            if dst_t == AsmType::Octword {
                let dst_op = convert_val(dst);
                match src {
                    TackyVal::Constant(c) => {
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Imm(*c),
                            low64_operand(dst_op.clone())?,
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Imm(if *c < 0 { -1 } else { 0 }),
                            high64_operand(dst_op)?,
                        ));
                    }
                    _ => {
                        if src_t == AsmType::Quadword {
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                convert_val(src),
                                low64_operand(dst_op.clone())?,
                            ));
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                convert_val(src),
                                AsmOperand::Reg(Reg::R10),
                            ));
                        } else {
                            out.push(AsmInstr::Movsx(
                                src_t,
                                AsmType::Quadword,
                                convert_val(src),
                                AsmOperand::Reg(Reg::R10),
                            ));
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Reg(Reg::R10),
                                low64_operand(dst_op.clone())?,
                            ));
                        }
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            AsmBinaryOp::Sar,
                            AsmOperand::Imm(63),
                            AsmOperand::Reg(Reg::R10),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Reg(Reg::R10),
                            high64_operand(dst_op)?,
                        ));
                    }
                }
                return Ok(());
            }
            match src {
                TackyVal::Constant(c) if dst_t != AsmType::Byte => {
                    out.push(AsmInstr::Mov(dst_t, AsmOperand::Imm(*c), convert_val(dst)));
                }
                _ => {
                    out.push(AsmInstr::Movsx(
                        src_t,
                        dst_t,
                        convert_val(src),
                        convert_val(dst),
                    ));
                }
            }
        }
        TackyInstr::ZeroExtend { src, dst } => {
            let src_t = val_type(src, types);
            let dst_t = val_type(dst, types);
            if dst_t == AsmType::Octword {
                let dst_op = convert_val(dst);
                if src_t == AsmType::Quadword {
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        convert_val(src),
                        low64_operand(dst_op.clone())?,
                    ));
                } else {
                    out.push(AsmInstr::MovZeroExtend(
                        src_t,
                        AsmType::Quadword,
                        convert_val(src),
                        AsmOperand::Reg(Reg::R10),
                    ));
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::R10),
                        low64_operand(dst_op.clone())?,
                    ));
                }
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    high64_operand(dst_op)?,
                ));
                return Ok(());
            }
            match src {
                TackyVal::Constant(c) if dst_t != AsmType::Byte => {
                    out.push(AsmInstr::Mov(dst_t, AsmOperand::Imm(*c), convert_val(dst)));
                }
                _ => {
                    out.push(AsmInstr::MovZeroExtend(
                        src_t,
                        dst_t,
                        convert_val(src),
                        convert_val(dst),
                    ));
                }
            }
        }
        TackyInstr::Truncate { src, dst } => {
            let dst_t = val_type(dst, types);
            let src_t = val_type(src, types);
            if matches!(dst_t, AsmType::Float | AsmType::Double) && src_t == AsmType::LongDouble {
                x87_load_val(out, src, types, static_doubles);
                out.push(AsmInstr::X87StoreFloat(dst_t, convert_val(dst)));
                return Ok(());
            }
            let src_op = if src_t == AsmType::Octword {
                low64_operand(convert_val(src))?
            } else {
                convert_val(src)
            };
            out.push(AsmInstr::Mov(dst_t, src_op, convert_val(dst)));
        }
        TackyInstr::Unary {
            op: TackyUnaryOp::LogicalNot,
            src,
            dst,
        } => {
            let t = val_type(src, types);
            let dst_op = convert_val(dst);
            if matches!(t, AsmType::Float | AsmType::Double) {
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Xor,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Cmp(
                    AsmType::Double,
                    src_op,
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(src)));
            }
            out.push(AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                dst_op.clone(),
            ));
            out.push(AsmInstr::SetCC(CondCode::E, dst_op.clone()));
            if matches!(t, AsmType::Float | AsmType::Double) {
                out.push(AsmInstr::SetCC(CondCode::NP, AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Binary(
                    AsmType::Byte,
                    AsmBinaryOp::And,
                    AsmOperand::Reg(Reg::R10),
                    dst_op,
                ));
            }
        }
        TackyInstr::Unary { op, src, dst } => {
            let t = val_type(dst, types);
            if t == AsmType::LongDouble && matches!(op, TackyUnaryOp::Negate) {
                x87_load_val(out, src, types, static_doubles);
                out.push(AsmInstr::X87UnaryNeg);
                out.push(AsmInstr::X87Store(convert_val(dst)));
            } else if matches!(t, AsmType::Float | AsmType::Double)
                && matches!(op, TackyUnaryOp::Negate)
            {
                // Double negation: XOR with sign bit mask (bit 63)
                // Emit a static constant with just the sign bit set
                let sign_bit: u64 = if t == AsmType::Float {
                    (1u32 << 31) as u64
                } else {
                    1u64 << 63
                };
                let sign_mask_label = intern_double_const(static_doubles, f64::from_bits(sign_bit));
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Mov(t, src_op, convert_val(dst)));
                out.push(AsmInstr::Binary(
                    t,
                    AsmBinaryOp::Xor,
                    AsmOperand::Data(sign_mask_label),
                    convert_val(dst),
                ));
            } else {
                let asm_op = match op {
                    TackyUnaryOp::Negate => AsmUnaryOp::Neg,
                    TackyUnaryOp::Complement => AsmUnaryOp::Not,
                    TackyUnaryOp::LogicalNot => {
                        return Err("logical-not should be lowered before x86-64 unary emission"
                            .to_string())
                    }
                };
                if t == AsmType::Octword {
                    emit_i128_copy(out, src, dst)?;
                    let dst_op = convert_val(dst);
                    let dst_low = low64_operand(dst_op.clone())?;
                    let dst_high = high64_operand(dst_op)?;
                    out.push(AsmInstr::Unary(
                        AsmType::Quadword,
                        AsmUnaryOp::Not,
                        dst_low.clone(),
                    ));
                    out.push(AsmInstr::Unary(
                        AsmType::Quadword,
                        AsmUnaryOp::Not,
                        dst_high.clone(),
                    ));
                    if matches!(op, TackyUnaryOp::Negate) {
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            AsmBinaryOp::AddSetFlags,
                            AsmOperand::Imm(1),
                            dst_low,
                        ));
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            AsmBinaryOp::Adc,
                            AsmOperand::Imm(0),
                            dst_high,
                        ));
                    }
                    return Ok(());
                }
                out.push(AsmInstr::Mov(t, convert_val(src), convert_val(dst)));
                out.push(AsmInstr::Unary(t, asm_op, convert_val(dst)));
            }
        }
        TackyInstr::Binary {
            op: TackyBinaryOp::Div,
            left,
            right,
            dst,
        } if val_type(dst, types) == AsmType::LongDouble => {
            x87_load_val(out, left, types, static_doubles);
            x87_load_val(out, right, types, static_doubles);
            out.push(AsmInstr::X87Binary(AsmX87BinaryOp::Div));
            out.push(AsmInstr::X87Store(convert_val(dst)));
        }
        TackyInstr::Binary {
            op: TackyBinaryOp::Div,
            left,
            right,
            dst,
        } if matches!(val_type(dst, types), AsmType::Float | AsmType::Double) => {
            let t = val_type(dst, types);
            let left_op = convert_floating_val(t, left, static_doubles, static_floats);
            let right_op = convert_floating_val(t, right, static_doubles, static_floats);
            let dst_op = convert_val(dst);
            out.push(AsmInstr::Mov(t, left_op, dst_op.clone()));
            out.push(AsmInstr::Binary(
                t,
                AsmBinaryOp::DivDouble,
                right_op,
                dst_op,
            ));
        }
        TackyInstr::Binary {
            op: op @ (TackyBinaryOp::Div | TackyBinaryOp::Mod),
            left,
            right,
            dst,
        } => {
            let t = val_type(dst, types);
            let dst_ctype = types
                .get(match dst {
                    TackyVal::Var(n) => n.as_str(),
                    _ => "",
                })
                .copied()
                .unwrap_or(CType::Int);
            let is_unsigned = !dst_ctype.is_signed();
            if t == AsmType::Octword {
                let helper = match (op, is_unsigned) {
                    (TackyBinaryOp::Div, true) => "__udivti3",
                    (TackyBinaryOp::Div, false) => "__divti3",
                    (TackyBinaryOp::Mod, true) => "__umodti3",
                    (TackyBinaryOp::Mod, false) => "__modti3",
                    _ => return Err("internal error: expected x86-64 i128 div/mod".to_string()),
                };
                let (left_low, left_high) = i128_part_operands(left)?;
                let (right_low, right_high) = i128_part_operands(right)?;
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    left_low,
                    AsmOperand::Reg(Reg::DI),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    left_high,
                    AsmOperand::Reg(Reg::SI),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    right_low,
                    AsmOperand::Reg(Reg::DX),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    right_high,
                    AsmOperand::Reg(Reg::CX),
                ));
                out.push(AsmInstr::Call(helper.to_string(), 4, 0, false, false));
                let dst_op = convert_val(dst);
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::AX),
                    low64_operand(dst_op.clone())?,
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::DX),
                    high64_operand(dst_op)?,
                ));
                return Ok(());
            }
            if matches!(t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::Mov(
                    t,
                    convert_val(left),
                    AsmOperand::Reg(Reg::AX),
                ));
                out.push(AsmInstr::Mov(
                    t,
                    convert_val(right),
                    AsmOperand::Reg(Reg::R10),
                ));
                if is_unsigned {
                    out.push(AsmInstr::MovZeroExtend(
                        t,
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::AX),
                        AsmOperand::Reg(Reg::AX),
                    ));
                    out.push(AsmInstr::MovZeroExtend(
                        t,
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Reg(Reg::R10),
                    ));
                    out.push(AsmInstr::Mov(
                        AsmType::Longword,
                        AsmOperand::Imm(0),
                        AsmOperand::Reg(Reg::DX),
                    ));
                    out.push(AsmInstr::Div(AsmType::Longword, AsmOperand::Reg(Reg::R10)));
                } else {
                    out.push(AsmInstr::Movsx(
                        t,
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::AX),
                        AsmOperand::Reg(Reg::AX),
                    ));
                    out.push(AsmInstr::Movsx(
                        t,
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Reg(Reg::R10),
                    ));
                    out.push(AsmInstr::Cdq(AsmType::Longword));
                    out.push(AsmInstr::Idiv(AsmType::Longword, AsmOperand::Reg(Reg::R10)));
                }
                let result_reg = if matches!(op, TackyBinaryOp::Mod) {
                    Reg::DX
                } else {
                    Reg::AX
                };
                out.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Reg(result_reg),
                    convert_val(dst),
                ));
                return Ok(());
            }
            out.push(AsmInstr::Mov(
                t,
                convert_val(left),
                AsmOperand::Reg(Reg::AX),
            ));
            if is_unsigned {
                // Zero EDX/RDX for unsigned division
                out.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(0),
                    AsmOperand::Reg(Reg::DX),
                ));
                out.push(AsmInstr::Div(t, convert_val(right)));
            } else {
                out.push(AsmInstr::Cdq(t));
                out.push(AsmInstr::Idiv(t, convert_val(right)));
            }
            let result_reg = if matches!(op, TackyBinaryOp::Mod) {
                Reg::DX
            } else {
                Reg::AX
            };
            out.push(AsmInstr::Mov(
                t,
                AsmOperand::Reg(result_reg),
                convert_val(dst),
            ));
        }
        TackyInstr::Binary {
            op: op @ (TackyBinaryOp::ShiftLeft | TackyBinaryOp::ShiftRight),
            left,
            right,
            dst,
        } => {
            let t = val_type(dst, types);
            if t == AsmType::Octword {
                let mut binary_ctx = BinaryContext {
                    types,
                    out,
                    static_doubles,
                    static_floats,
                    label_counter,
                    function_name,
                };
                convert_binary(op, left, right, dst, &mut binary_ctx)?;
                return Ok(());
            }
            let dst_ctype = match dst {
                TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int),
                _ => CType::Int,
            };
            let asm_op = match op {
                TackyBinaryOp::ShiftLeft => AsmBinaryOp::Sal,
                TackyBinaryOp::ShiftRight => {
                    if dst_ctype.is_signed() {
                        AsmBinaryOp::Sar
                    } else {
                        AsmBinaryOp::Shr
                    }
                }
                _ => return Err(format!("unsupported x86-64 shift op: {:?}", op)),
            };
            out.push(AsmInstr::Mov(t, convert_val(left), convert_val(dst)));
            let shift_amount = match right {
                TackyVal::Constant(_)
                | TackyVal::Int128Constant(_)
                | TackyVal::UInt128Constant(_) => {
                    // x86-64 has an immediate-count encoding for shifts.  Avoid
                    // tying up %rcx when the count is known at compile time.
                    convert_val(right)
                }
                TackyVal::Var(_) => {
                    out.push(AsmInstr::Mov(
                        AsmType::Longword,
                        convert_val(right),
                        AsmOperand::Reg(Reg::CX),
                    ));
                    AsmOperand::Reg(Reg::CX)
                }
                TackyVal::DoubleConstant(_) => {
                    return Err("internal error: floating-point shift count".to_string())
                }
            };
            out.push(AsmInstr::Binary(t, asm_op, shift_amount, convert_val(dst)));
        }
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            let mut binary_ctx = BinaryContext {
                types,
                out,
                static_doubles,
                static_floats,
                label_counter,
                function_name,
            };
            convert_binary(op, left, right, dst, &mut binary_ctx)?;
        }
        TackyInstr::Copy { src, dst } => {
            let t = val_type(dst, types);
            if t == AsmType::LongDouble {
                x87_copy_to_long_double(
                    out,
                    src,
                    dst,
                    types,
                    static_doubles,
                    label_counter,
                    function_name,
                );
                return Ok(());
            }
            if matches!(t, AsmType::Float | AsmType::Double)
                && val_type(src, types) == AsmType::LongDouble
            {
                x87_load_val(out, src, types, static_doubles);
                out.push(AsmInstr::X87StoreFloat(t, convert_val(dst)));
                return Ok(());
            }
            if t == AsmType::Octword {
                emit_i128_copy(out, src, dst)?;
                return Ok(());
            }
            let src_op = if t == AsmType::Float {
                convert_float_return_val(src, static_floats)
            } else if t == AsmType::Double || matches!(src, TackyVal::DoubleConstant(_)) {
                convert_double_val(src, static_doubles)
            } else {
                convert_val(src)
            };
            out.push(AsmInstr::Mov(t, src_op, convert_val(dst)));
        }
        TackyInstr::IntToDouble { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                // narrow integer→double: extend to int first, then cvtsi2sd
                out.push(AsmInstr::Movsx(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvtsi2sd(
                    src_t,
                    convert_val(src),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::IntToFloat { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::Movsx(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvtsi2ss(
                    src_t,
                    convert_val(src),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::DoubleToInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if val_type(src, types) == AsmType::LongDouble {
                x87_load_val(out, src, types, static_doubles);
                if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                    out.push(AsmInstr::X87StoreInt(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    out.push(AsmInstr::Mov(
                        dst_t,
                        AsmOperand::Reg(Reg::R10),
                        convert_val(dst),
                    ));
                } else {
                    out.push(AsmInstr::X87StoreInt(dst_t, convert_val(dst)));
                }
            } else if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                // double→narrow integer: cvttsd2si to int, then truncate
                out.push(AsmInstr::Cvttsd2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttsd2si(
                    dst_t,
                    convert_val(src),
                    convert_val(dst),
                ));
                if dst_t == AsmType::Longword {
                    clamp_floating_to_signed_int_overflow(
                        out,
                        AsmType::Double,
                        src,
                        dst,
                        static_doubles,
                        label_counter,
                        function_name,
                    );
                }
            }
        }
        TackyInstr::FloatToInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::Cvttss2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttss2si(
                    dst_t,
                    convert_val(src),
                    convert_val(dst),
                ));
                if dst_t == AsmType::Longword {
                    clamp_floating_to_signed_int_overflow(
                        out,
                        AsmType::Float,
                        src,
                        dst,
                        static_doubles,
                        label_counter,
                        function_name,
                    );
                }
            }
        }
        TackyInstr::UIntToDouble { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                // narrow unsigned integer→double: zero-extend to int, then cvtsi2sd
                out.push(AsmInstr::MovZeroExtend(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else if src_t == AsmType::Longword {
                // Unsigned int (32-bit): zero-extend to R10 (64-bit), then cvtsi2sdq
                out.push(AsmInstr::MovZeroExtend(
                    AsmType::Longword,
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                // Unsigned long (64-bit): need to handle values > LONG_MAX
                // Algorithm: test if negative (as signed); if not, cvtsi2sdq directly
                // If so: shift right 1, save LSB, OR LSB into shifted value,
                // cvtsi2sdq, then addsd result to itself
                let base = *label_counter;
                *label_counter += 1;
                let ok_label = format!("uint_to_double_ok.{}.{}", function_name, base);
                let end_label = format!("uint_to_double_end.{}.{}", function_name, base);
                out.push(AsmInstr::Cmp(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    convert_val(src),
                ));
                out.push(AsmInstr::JmpCC(CondCode::GE, ok_label.clone()));
                // Negative as signed = >= LONG_MAX as unsigned
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::Reg(Reg::R11),
                ));
                // R11 = src & 1 (save LSB for rounding)
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::And,
                    AsmOperand::Imm(1),
                    AsmOperand::Reg(Reg::R11),
                ));
                // R10 = src >> 1
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Shr,
                    AsmOperand::Imm(1),
                    AsmOperand::Reg(Reg::R10),
                ));
                // R10 = R10 | R11 (round-to-odd)
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Or,
                    AsmOperand::Reg(Reg::R11),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
                // Double the result: dst = dst + dst
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Add,
                    convert_val(dst),
                    convert_val(dst),
                ));
                out.push(AsmInstr::Jmp(end_label.clone()));
                out.push(AsmInstr::Label(ok_label));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Quadword,
                    convert_val(src),
                    convert_val(dst),
                ));
                out.push(AsmInstr::Label(end_label));
            }
        }
        TackyInstr::UIntToFloat { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::MovZeroExtend(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else if src_t == AsmType::Longword {
                out.push(AsmInstr::MovZeroExtend(
                    AsmType::Longword,
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                let base = *label_counter;
                *label_counter += 1;
                let ok_label = format!("uint_to_float_ok.{}.{}", function_name, base);
                let end_label = format!("uint_to_float_end.{}.{}", function_name, base);
                out.push(AsmInstr::Cmp(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    convert_val(src),
                ));
                out.push(AsmInstr::JmpCC(CondCode::GE, ok_label.clone()));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::Reg(Reg::R11),
                ));
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::And,
                    AsmOperand::Imm(1),
                    AsmOperand::Reg(Reg::R11),
                ));
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Shr,
                    AsmOperand::Imm(1),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Or,
                    AsmOperand::Reg(Reg::R11),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
                out.push(AsmInstr::Binary(
                    AsmType::Float,
                    AsmBinaryOp::Add,
                    convert_val(dst),
                    convert_val(dst),
                ));
                out.push(AsmInstr::Jmp(end_label.clone()));
                out.push(AsmInstr::Label(ok_label));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Quadword,
                    convert_val(src),
                    convert_val(dst),
                ));
                out.push(AsmInstr::Label(end_label));
            }
        }
        TackyInstr::GetAddress { src, dst } => {
            out.push(AsmInstr::Lea(convert_val(src), convert_val(dst)));
        }
        TackyInstr::VaStart { dst } => {
            out.push(AsmInstr::Lea(
                AsmOperand::Stack(i64::from(va_start_stack_offset)),
                convert_val(dst),
            ));
        }
        TackyInstr::Load { src_ptr, dst } => {
            let dst_t = val_type(dst, types);
            if dst_t == AsmType::Octword {
                emit_i128_load(out, src_ptr, dst)?;
            } else if dst_t == AsmType::LongDouble {
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(src_ptr),
                    AsmOperand::Reg(Reg::R11),
                ));
                out.push(AsmInstr::X87LoadIndirect(AsmType::LongDouble, Reg::R11));
                out.push(AsmInstr::X87Store(convert_val(dst)));
            } else {
                // Load pointer value into R11, then load indirectly
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(src_ptr),
                    AsmOperand::Reg(Reg::R11),
                ));
                out.push(AsmInstr::LoadIndirect(dst_t, Reg::R11, convert_val(dst)));
            }
        }
        TackyInstr::Store { src, dst_ptr } => {
            let src_t = val_type(src, types);
            if src_t == AsmType::Octword {
                emit_i128_store(out, src, dst_ptr)?;
            } else if src_t == AsmType::LongDouble {
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(dst_ptr),
                    AsmOperand::Reg(Reg::R11),
                ));
                x87_load_val(out, src, types, static_doubles);
                out.push(AsmInstr::X87StoreIndirect(Reg::R11));
            } else {
                // Load pointer value into R11, then store indirectly
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(dst_ptr),
                    AsmOperand::Reg(Reg::R11),
                ));
                let src_op = if src_t == AsmType::Float {
                    convert_float_return_val(src, static_floats)
                } else if src_t == AsmType::Double || matches!(src, TackyVal::DoubleConstant(_)) {
                    convert_double_val(src, static_doubles)
                } else {
                    convert_val(src)
                };
                out.push(AsmInstr::StoreIndirect(src_t, src_op, Reg::R11));
            }
        }
        TackyInstr::CopyToOffset {
            src,
            dst_name,
            offset,
        } => {
            let src_t = val_type(src, types);
            if src_t == AsmType::Octword {
                let (low, high) = i128_part_operands(src)?;
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    low,
                    AsmOperand::PseudoMem(dst_name.clone(), *offset as i32),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    high,
                    AsmOperand::PseudoMem(dst_name.clone(), (*offset + 8) as i32),
                ));
            } else if src_t == AsmType::LongDouble {
                x87_load_val(out, src, types, static_doubles);
                out.push(AsmInstr::X87Store(AsmOperand::PseudoMem(
                    dst_name.clone(),
                    *offset as i32,
                )));
            } else if matches!(src_t, AsmType::Float | AsmType::Double) {
                let src_op = if src_t == AsmType::Float {
                    convert_float_return_val(src, static_floats)
                } else {
                    convert_double_val(src, static_doubles)
                };
                out.push(AsmInstr::Mov(
                    src_t,
                    src_op,
                    AsmOperand::PseudoMem(dst_name.clone(), *offset as i32),
                ));
            } else {
                out.push(AsmInstr::Mov(
                    src_t,
                    convert_val(src),
                    AsmOperand::PseudoMem(dst_name.clone(), *offset as i32),
                ));
            }
        }
        TackyInstr::CopyFromOffset {
            src_name,
            offset,
            dst,
        } => {
            let dst_t = val_type(dst, types);
            if dst_t == AsmType::Octword {
                let dst_op = convert_val(dst);
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::PseudoMem(src_name.clone(), *offset as i32),
                    low64_operand(dst_op.clone())?,
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::PseudoMem(src_name.clone(), (*offset + 8) as i32),
                    high64_operand(dst_op)?,
                ));
            } else if dst_t == AsmType::LongDouble {
                out.push(AsmInstr::X87Load(
                    AsmType::LongDouble,
                    AsmOperand::PseudoMem(src_name.clone(), *offset as i32),
                ));
                out.push(AsmInstr::X87Store(convert_val(dst)));
            } else {
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::PseudoMem(src_name.clone(), *offset as i32),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => {
            // ptr + index * scale → dst
            // If index is a constant, compute offset at compile time
            if let TackyVal::Constant(idx) = index {
                let offset = idx.wrapping_mul(*scale);
                if offset == 0 {
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        convert_val(ptr),
                        convert_val(dst),
                    ));
                } else {
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        convert_val(ptr),
                        convert_val(dst),
                    ));
                    let (op, imm) = if offset == i64::MIN {
                        (AsmBinaryOp::Add, offset)
                    } else if offset < 0 {
                        (AsmBinaryOp::Sub, -offset)
                    } else {
                        (AsmBinaryOp::Add, offset)
                    };
                    out.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        op,
                        AsmOperand::Imm(imm),
                        convert_val(dst),
                    ));
                }
            } else {
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(ptr),
                    AsmOperand::Reg(Reg::AX),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(index),
                    AsmOperand::Reg(Reg::DX),
                ));
                if *scale == 1 || *scale == 2 || *scale == 4 || *scale == 8 {
                    out.push(AsmInstr::Lea(
                        AsmOperand::Indexed(Reg::AX, Reg::DX, *scale as i32),
                        convert_val(dst),
                    ));
                } else {
                    out.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        AsmBinaryOp::Mul,
                        AsmOperand::Imm(*scale),
                        AsmOperand::Reg(Reg::DX),
                    ));
                    out.push(AsmInstr::Lea(
                        AsmOperand::Indexed(Reg::AX, Reg::DX, 1),
                        convert_val(dst),
                    ));
                }
            }
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if val_type(src, types) == AsmType::LongDouble {
                x87_load_val(out, src, types, static_doubles);
                if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                    out.push(AsmInstr::X87StoreInt(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    out.push(AsmInstr::Mov(
                        dst_t,
                        AsmOperand::Reg(Reg::R10),
                        convert_val(dst),
                    ));
                } else if dst_t == AsmType::Longword {
                    out.push(AsmInstr::X87StoreInt(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    out.push(AsmInstr::Mov(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        convert_val(dst),
                    ));
                } else {
                    out.push(AsmInstr::X87StoreInt(dst_t, convert_val(dst)));
                }
            } else if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                // double→narrow unsigned integer: cvttsd2si to int, truncate
                out.push(AsmInstr::Cvttsd2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttsd2si(
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                if dst_t == AsmType::Longword {
                    out.push(AsmInstr::Mov(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        convert_val(dst),
                    ));
                } else {
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::R10),
                        convert_val(dst),
                    ));
                }
            }
        }
        TackyInstr::FloatToUInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::Cvttss2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttss2si(
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::FloatToDouble { src, dst } => {
            out.push(AsmInstr::Cvtss2sd(
                convert_float_return_val(src, static_floats),
                convert_val(dst),
            ));
        }
        TackyInstr::DoubleToFloat { src, dst } => {
            out.push(AsmInstr::Cvtsd2ss(
                convert_double_val(src, static_doubles),
                convert_val(dst),
            ));
        }
        TackyInstr::Jump(label) => {
            out.push(AsmInstr::Jmp(label.clone()));
        }
        TackyInstr::NonlocalJump(label) => {
            out.push(AsmInstr::NonlocalJmp(label.clone()));
        }
        TackyInstr::JumpIndirect(target) => {
            out.push(AsmInstr::JmpIndirect(convert_val(target)));
        }
        TackyInstr::JumpIfZero(val, label) => {
            let t = val_type(val, types);
            if matches!(t, AsmType::Float | AsmType::Double) {
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Xor,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                let skip_label = format!(
                    "float_jz_unordered.{}.{}",
                    ctx.function_name, *ctx.label_counter
                );
                *ctx.label_counter += 1;
                out.push(AsmInstr::Cmp(
                    t,
                    convert_double_val(val, static_doubles),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                out.push(AsmInstr::JmpCC(CondCode::P, skip_label.clone()));
                out.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
                out.push(AsmInstr::Label(skip_label));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(val)));
                out.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            }
        }
        TackyInstr::JumpIfNotZero(val, label) => {
            let t = val_type(val, types);
            if matches!(t, AsmType::Float | AsmType::Double) {
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Xor,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                out.push(AsmInstr::Cmp(
                    t,
                    convert_double_val(val, static_doubles),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                out.push(AsmInstr::JmpCC(CondCode::P, label.clone()));
                out.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(val)));
                out.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            }
        }
        TackyInstr::Label(label) => {
            out.push(AsmInstr::Label(label.clone()));
        }
        TackyInstr::LoadLabelAddress(label, dst) => {
            out.push(AsmInstr::LoadLabelAddress(label.clone(), convert_val(dst)));
        }
        TackyInstr::FrameAddress { dst } => {
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::BP),
                convert_val(dst),
            ));
        }
        TackyInstr::BuiltinSetjmp {
            buf,
            dst,
            label,
            end_label,
        } => {
            out.push(AsmInstr::BuiltinSetjmp {
                buf: convert_val(buf),
                dst: convert_val(dst),
                label: label.clone(),
                end_label: end_label.clone(),
            });
        }
        TackyInstr::BuiltinLongjmp { buf, value } => {
            out.push(AsmInstr::BuiltinLongjmp {
                buf: convert_val(buf),
                value: convert_val(value),
            });
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
            let mut ctx = FuncallContext {
                types,
                out,
                static_doubles,
                var_struct_tags,
                struct_defs,
                local_function_names,
                target,
                variadic: *variadic,
                fixed_flat_arg_count: *fixed_flat_arg_count,
                hidden_return: *hidden_return,
            };
            let call = FuncallArgs {
                name,
                args,
                dst,
                stack_arg_indices,
                memory_arg_blocks,
                struct_arg_groups,
                indirect: *indirect,
            };
            convert_funcall(&call, &mut ctx)?;
        }
    };
    Ok(())
}

struct FuncallContext<'a> {
    types: &'a IndexMap<String, CType>,
    out: &'a mut Vec<AsmInstr>,
    static_doubles: &'a mut Vec<(String, f64)>,
    var_struct_tags: &'a HashMap<String, String>,
    struct_defs: &'a IndexMap<String, StructDef>,
    local_function_names: &'a std::collections::HashSet<String>,
    target: &'a Target,
    variadic: bool,
    fixed_flat_arg_count: usize,
    hidden_return: bool,
}

struct FuncallArgs<'a> {
    name: &'a str,
    args: &'a [TackyVal],
    dst: &'a TackyVal,
    stack_arg_indices: &'a std::collections::HashSet<usize>,
    memory_arg_blocks: &'a [(usize, usize, usize)],
    struct_arg_groups: &'a [(usize, usize, Vec<bool>)],
    indirect: bool,
}

fn x86_64_linux_libc_va_list_arg(name: &str) -> Option<usize> {
    match name {
        "vprintf" => Some(1),
        "vfprintf" => Some(2),
        "vsnprintf" => Some(3),
        "__vprintf_chk" => Some(2),
        "__vfprintf_chk" => Some(3),
        "__vsnprintf_chk" => Some(5),
        "vsyslog" => Some(2),
        _ => None,
    }
}

fn convert_funcall(call: &FuncallArgs<'_>, ctx: &mut FuncallContext<'_>) -> Result<(), String> {
    let name = call.name;
    let args = call.args;
    let dst = call.dst;
    let stack_arg_indices = call.stack_arg_indices;
    let memory_arg_blocks = call.memory_arg_blocks;
    let struct_arg_groups = call.struct_arg_groups;
    let indirect = call.indirect;
    let types = ctx.types;
    let out = &mut *ctx.out;
    let static_doubles = &mut *ctx.static_doubles;
    let var_struct_tags = ctx.var_struct_tags;
    let struct_defs = ctx.struct_defs;
    let use_shadow_varargs = ctx.variadic && !indirect && ctx.local_function_names.contains(name);
    let libc_va_list_arg = if !indirect
        && ctx.target.os == TargetOs::Linux
        && !ctx.local_function_names.contains(name)
    {
        x86_64_linux_libc_va_list_arg(name)
    } else {
        None
    };
    let mut memory_blocks: std::collections::HashMap<usize, (usize, usize)> =
        std::collections::HashMap::with_capacity(memory_arg_blocks.len());
    for (index, size, align) in memory_arg_blocks {
        memory_blocks.insert(*index, (*size, *align));
    }

    {
        // Pre-compute which args must go on stack due to struct group overflow
        let mut force_stack_args: std::collections::HashSet<usize> =
            std::collections::HashSet::with_capacity(args.len());
        {
            let mut sim_int = 0usize;
            let mut sim_xmm = 0usize;
            for (arg_idx, arg) in args.iter().enumerate() {
                if memory_blocks.contains_key(&arg_idx) {
                    continue;
                }
                if stack_arg_indices.contains(&arg_idx) {
                    force_stack_args.insert(arg_idx);
                    continue;
                }
                let group = struct_arg_groups
                    .iter()
                    .find(|(start, count, _)| arg_idx >= *start && arg_idx < *start + *count);
                if let Some((start, count, is_sse_vec)) = group {
                    if arg_idx == *start {
                        let int_needed: usize =
                            is_sse_vec.iter().filter(|&&is_sse| !is_sse).count();
                        let sse_needed: usize = is_sse_vec.iter().filter(|&&is_sse| is_sse).count();
                        if sim_int + int_needed <= 6 && sim_xmm + sse_needed <= 8 {
                            sim_int += int_needed;
                            sim_xmm += sse_needed;
                        } else {
                            for j in *start..*start + *count {
                                force_stack_args.insert(j);
                            }
                        }
                    }
                    continue;
                }
                let t = val_type(arg, types);
                if t == AsmType::LongDouble {
                    force_stack_args.insert(arg_idx);
                } else if matches!(t, AsmType::Float | AsmType::Double) {
                    if sim_xmm < 8 {
                        sim_xmm += 1;
                    }
                } else {
                    if sim_int < 6 {
                        sim_int += 1;
                    }
                }
            }
        }

        enum StackArg<'a> {
            Scalar(&'a TackyVal),
            WideScalar(&'a TackyVal),
            LongDouble(&'a TackyVal),
            MemoryBlock {
                src_ptr: &'a TackyVal,
                size: usize,
                align: usize,
            },
        }

        impl StackArg<'_> {
            fn layout(&self) -> StackArgLayout {
                match self {
                    StackArg::Scalar(_) => StackArgLayout::for_scalar(AsmType::Quadword),
                    StackArg::WideScalar(_) => StackArgLayout::for_scalar(AsmType::Octword),
                    StackArg::LongDouble(_) => StackArgLayout::for_scalar(AsmType::LongDouble),
                    StackArg::MemoryBlock { size, align, .. } => {
                        StackArgLayout::for_memory_block(*size, *align)
                    }
                }
            }
        }

        // Classify args into int regs, xmm regs, and stack
        let mut int_reg_args = Vec::with_capacity(args.len().min(ARG_REGISTERS.len()));
        let mut wide_int_reg_args = Vec::with_capacity(args.len() / 2);
        let mut xmm_reg_args = Vec::with_capacity(args.len().min(ARG_SSE_REGISTERS));
        let mut stack_args_list = Vec::with_capacity(args.len());
        let mut int_idx = 0usize;
        let mut xmm_idx = 0usize;

        for (arg_idx, arg) in args.iter().enumerate() {
            let is_variadic_extra = use_shadow_varargs && arg_idx >= ctx.fixed_flat_arg_count;
            if let Some((size, align)) = memory_blocks.get(&arg_idx).copied() {
                stack_args_list.push(StackArg::MemoryBlock {
                    src_ptr: arg,
                    size,
                    align,
                });
                continue;
            }
            if force_stack_args.contains(&arg_idx) {
                if val_type(arg, types) == AsmType::LongDouble {
                    stack_args_list.push(StackArg::LongDouble(arg));
                } else {
                    stack_args_list.push(StackArg::Scalar(arg));
                }
                continue;
            }
            let t = val_type(arg, types);
            if is_variadic_extra {
                // rnqcc-defined variadic callees read unnamed arguments from
                // this ordered shadow area instead of the platform va_list ABI.
                if t == AsmType::LongDouble {
                    stack_args_list.push(StackArg::LongDouble(arg));
                } else if t == AsmType::Octword {
                    stack_args_list.push(StackArg::WideScalar(arg));
                } else {
                    stack_args_list.push(StackArg::Scalar(arg));
                }
                continue;
            }
            if t == AsmType::LongDouble {
                stack_args_list.push(StackArg::LongDouble(arg));
            } else if matches!(t, AsmType::Float | AsmType::Double) {
                if xmm_idx < 8 {
                    xmm_reg_args.push((xmm_idx, arg));
                    xmm_idx += 1;
                } else {
                    stack_args_list.push(StackArg::Scalar(arg));
                }
            } else if t == AsmType::Octword {
                if int_idx + 1 < 6 {
                    wide_int_reg_args.push((int_idx, arg));
                    int_idx += 2;
                } else {
                    stack_args_list.push(StackArg::WideScalar(arg));
                }
            } else {
                if int_idx < 6 {
                    int_reg_args.push((int_idx, arg_idx, arg));
                    int_idx += 1;
                } else {
                    stack_args_list.push(StackArg::Scalar(arg));
                }
            }
        }

        let stack_bytes = stack_args_list.iter().fold(0usize, |offset, item| {
            let layout = item.layout();
            layout.place_at(offset) + layout.size
        });
        let libc_va_list_bridge = libc_va_list_arg.and_then(|arg_idx| {
            int_reg_args
                .iter()
                .find(|(_, actual_arg_idx, _)| *actual_arg_idx == arg_idx)
                .map(|(reg_idx, _, arg)| (*reg_idx, *arg, stack_bytes as i32))
        });
        let stack_bytes_with_bridge = if libc_va_list_bridge.is_some() {
            stack_bytes + 32
        } else {
            stack_bytes
        };
        let padding = if !stack_bytes_with_bridge.is_multiple_of(16) {
            8
        } else {
            0
        };
        let outgoing_bytes = stack_bytes_with_bridge + padding;
        if outgoing_bytes > 0 {
            out.push(AsmInstr::AllocateStack(outgoing_bytes as i64));
            let mut stack_offset = 0i32;
            for item in &stack_args_list {
                let layout = item.layout();
                stack_offset = layout
                    .place_at(stack_offset as usize)
                    .try_into()
                    .map_err(|_| "x86-64 stack argument offset overflow".to_string())?;
                match item {
                    StackArg::Scalar(arg) => {
                        let t = val_type(arg, types);
                        if t == AsmType::Double {
                            let src = convert_double_val(arg, static_doubles);
                            out.push(AsmInstr::Mov(t, src, AsmOperand::StackArg(stack_offset)));
                        } else {
                            out.push(AsmInstr::Mov(
                                t,
                                convert_val(arg),
                                AsmOperand::StackArg(stack_offset),
                            ));
                        }
                        stack_offset += layout.size as i32;
                    }
                    StackArg::WideScalar(arg) => {
                        let (low, high) = i128_part_operands(arg)?;
                        emit_i128_parts_to_operands(
                            out,
                            low,
                            high,
                            AsmOperand::StackArg(stack_offset),
                            AsmOperand::StackArg(stack_offset + 8),
                        );
                        stack_offset += layout.size as i32;
                    }
                    StackArg::LongDouble(arg) => {
                        x87_load_val(out, arg, types, static_doubles);
                        out.push(AsmInstr::X87Store(AsmOperand::StackArg(stack_offset)));
                        stack_offset += layout.size as i32;
                    }
                    StackArg::MemoryBlock { src_ptr, size, .. } => {
                        out.push(AsmInstr::CopyToStackArg {
                            src_ptr: convert_val(src_ptr),
                            dst_offset: stack_offset,
                            size: *size,
                        });
                        stack_offset += layout.size as i32;
                    }
                }
            }
            if let Some((_, va_list_arg, va_list_offset)) = libc_va_list_bridge {
                // SysV x86-64 va_list is an array of one struct:
                // { unsigned gp_offset; unsigned fp_offset; void *overflow; void *reg_save; }.
                // rnqcc va_list values already point at the ordered shadow overflow area.
                out.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Imm(48),
                    AsmOperand::StackArg(va_list_offset),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Imm(304),
                    AsmOperand::StackArg(va_list_offset + 4),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(va_list_arg),
                    AsmOperand::StackArg(va_list_offset + 8),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    AsmOperand::StackArg(va_list_offset + 16),
                ));
            }
        }
        // Move int register args
        for (i, arg_idx, arg) in &int_reg_args {
            let t = val_type(arg, types);
            let bridge_va_list_offset =
                libc_va_list_bridge.and_then(|(bridge_reg_idx, _, va_list_offset)| {
                    (bridge_reg_idx == *i && Some(*arg_idx) == libc_va_list_arg)
                        .then_some(va_list_offset)
                });
            if let Some(va_list_offset) = bridge_va_list_offset {
                out.push(AsmInstr::Lea(
                    AsmOperand::StackArg(va_list_offset),
                    AsmOperand::Reg(ARG_REGISTERS[*i]),
                ));
                continue;
            }
            // For constants going into registers: use Quadword when value is negative
            // (movl zero-extends, which changes the meaning of negative values)
            let t = match arg {
                TackyVal::Constant(v) if *v < 0 && t == AsmType::Longword => AsmType::Quadword,
                _ => t,
            };
            out.push(AsmInstr::Mov(
                t,
                convert_val(arg),
                AsmOperand::Reg(ARG_REGISTERS[*i]),
            ));
        }
        for (i, arg) in &wide_int_reg_args {
            let (low, high) = i128_part_operands(arg)?;
            emit_i128_parts_to_operands(
                out,
                low,
                high,
                AsmOperand::Reg(ARG_REGISTERS[*i]),
                AsmOperand::Reg(ARG_REGISTERS[*i + 1]),
            );
        }
        // Move xmm register args
        for (i, arg) in &xmm_reg_args {
            let t = val_type(arg, types);
            let src = convert_double_val(arg, static_doubles);
            out.push(AsmInstr::Mov(
                t,
                src,
                AsmOperand::Xmm(XMM_ARG_REGISTERS[*i]),
            ));
        }

        if indirect {
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Pseudo(name.to_string()),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        if ctx.variadic {
            out.push(AsmInstr::X86SetVarargsXmmCount(xmm_reg_args.len()));
        }
        out.push(AsmInstr::Call(
            name.to_string(),
            int_reg_args.len(),
            xmm_reg_args.len(),
            indirect,
            !indirect && ctx.local_function_names.contains(name),
        ));
        let bytes_to_dealloc = outgoing_bytes as i64;
        if bytes_to_dealloc > 0 {
            out.push(AsmInstr::DeallocateStack(bytes_to_dealloc));
        }
        if ctx.hidden_return {
            return Ok(());
        }
        let ret_t = val_type(dst, types);
        // Check if return value is a struct
        if let TackyVal::Var(ref dst_name) = dst {
            if types.get(dst_name).copied() == Some(CType::Struct) {
                if let Some(classes) = get_struct_classes(dst_name, var_struct_tags, struct_defs) {
                    let mut int_ret_idx = 0;
                    let mut sse_ret_idx = 0;
                    let int_ret_regs = [Reg::AX, Reg::DX];
                    let sse_ret_regs = [XmmReg::XMM0, XmmReg::XMM1];
                    for (eb_idx, class) in classes.iter().enumerate() {
                        let eb_offset = (eb_idx * 8) as i32;
                        match class {
                            ParamClass::Sse if sse_ret_idx < 2 => {
                                out.push(AsmInstr::Mov(
                                    AsmType::Double,
                                    AsmOperand::Xmm(sse_ret_regs[sse_ret_idx]),
                                    AsmOperand::PseudoMem(dst_name.clone(), eb_offset),
                                ));
                                sse_ret_idx += 1;
                            }
                            ParamClass::Integer if int_ret_idx < 2 => {
                                out.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    AsmOperand::Reg(int_ret_regs[int_ret_idx]),
                                    AsmOperand::PseudoMem(dst_name.clone(), eb_offset),
                                ));
                                int_ret_idx += 1;
                            }
                            _ => {}
                        }
                    }
                    return Ok(());
                }
            }
        }
        if ret_t == AsmType::LongDouble {
            out.push(AsmInstr::X87Store(convert_val(dst)));
        } else if matches!(ret_t, AsmType::Float | AsmType::Double) {
            out.push(AsmInstr::Mov(
                ret_t,
                AsmOperand::Xmm(XmmReg::XMM0),
                convert_val(dst),
            ));
        } else if ret_t == AsmType::Octword {
            let dst_op = convert_val(dst);
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::AX),
                low64_operand(dst_op.clone()).unwrap_or_else(|_| convert_val(dst)),
            ));
            if let Ok(high) = high64_operand(dst_op) {
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::DX),
                    high,
                ));
            }
        } else {
            out.push(AsmInstr::Mov(
                ret_t,
                AsmOperand::Reg(Reg::AX),
                convert_val(dst),
            ));
        }
    }
    Ok(())
}

struct BinaryContext<'a> {
    types: &'a IndexMap<String, CType>,
    out: &'a mut Vec<AsmInstr>,
    static_doubles: &'a mut Vec<(String, f64)>,
    static_floats: &'a mut Vec<(String, f32)>,
    label_counter: &'a mut usize,
    function_name: &'a str,
}

fn convert_binary(
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    ctx: &mut BinaryContext<'_>,
) -> Result<(), String> {
    let types = ctx.types;
    let out = &mut *ctx.out;
    let static_doubles = &mut *ctx.static_doubles;
    let static_floats = &mut *ctx.static_floats;
    let left_ctype = match left {
        TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int),
        TackyVal::Constant(_) => CType::Int,
        TackyVal::Int128Constant(_) => CType::Int128,
        TackyVal::UInt128Constant(_) => CType::UInt128,
        TackyVal::DoubleConstant(_) => CType::Double,
    };
    let right_ctype = match right {
        TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int),
        TackyVal::Constant(_) => CType::Int,
        TackyVal::Int128Constant(_) => CType::Int128,
        TackyVal::UInt128Constant(_) => CType::UInt128,
        TackyVal::DoubleConstant(_) => CType::Double,
    };
    let is_unsigned = !left_ctype.is_signed()
        || !right_ctype.is_signed()
        || left_ctype.is_floating()
        || right_ctype.is_floating();

    if let Some(cc) = is_comparison(op, is_unsigned) {
        let var_type = |v: &TackyVal| -> Option<AsmType> {
            match v {
                TackyVal::Var(n) => Some(match types.get(n).copied().unwrap_or(CType::Int) {
                    CType::Long | CType::ULong | CType::Pointer => AsmType::Quadword,
                    CType::Int128 | CType::UInt128 => AsmType::Octword,
                    CType::Float => AsmType::Float,
                    CType::Double => AsmType::Double,
                    CType::LongDouble => AsmType::LongDouble,
                    _ => AsmType::Longword,
                }),
                TackyVal::DoubleConstant(_) => Some(AsmType::Double),
                _ => None,
            }
        };
        let cmp_type = match (var_type(left), var_type(right)) {
            (Some(AsmType::Double), _) | (_, Some(AsmType::Double)) => AsmType::Double,
            (Some(AsmType::Float), _) | (_, Some(AsmType::Float)) => AsmType::Float,
            (Some(AsmType::Quadword), _) | (_, Some(AsmType::Quadword)) => AsmType::Quadword,
            (Some(t), _) => t,
            (_, Some(t)) => t,
            _ => {
                let lt = val_type(left, types);
                let rt = val_type(right, types);
                if lt == AsmType::Quadword || rt == AsmType::Quadword {
                    AsmType::Quadword
                } else {
                    AsmType::Longword
                }
            }
        };
        if cmp_type == AsmType::LongDouble {
            x87_load_val(out, right, types, static_doubles);
            x87_load_val(out, left, types, static_doubles);
            out.push(AsmInstr::X87Compare);
        } else if cmp_type == AsmType::Octword {
            let (left_low, left_high) = i128_part_operands(left)?;
            let (right_low, right_high) = i128_part_operands(right)?;
            let dst_op = convert_val(dst);
            let base = *ctx.label_counter;
            *ctx.label_counter += 1;
            let true_label = format!("i128_cmp_true.{}.{}", ctx.function_name, base);
            let low_label = format!("i128_cmp_low.{}.{}", ctx.function_name, base);
            let end_label = format!("i128_cmp_end.{}.{}", ctx.function_name, base);
            out.push(AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                dst_op.clone(),
            ));
            if matches!(op, TackyBinaryOp::Equal | TackyBinaryOp::NotEqual) {
                // Equality depends on both halves; ordering comparisons can decide
                // from the high half before falling through to the low half.
                out.push(AsmInstr::Cmp(AsmType::Quadword, right_high, left_high));
                if matches!(op, TackyBinaryOp::Equal) {
                    out.push(AsmInstr::JmpCC(CondCode::NE, end_label.clone()));
                    out.push(AsmInstr::Cmp(AsmType::Quadword, right_low, left_low));
                    out.push(AsmInstr::JmpCC(CondCode::NE, end_label.clone()));
                } else {
                    out.push(AsmInstr::JmpCC(CondCode::NE, true_label.clone()));
                    out.push(AsmInstr::Cmp(AsmType::Quadword, right_low, left_low));
                    out.push(AsmInstr::JmpCC(CondCode::NE, true_label.clone()));
                    out.push(AsmInstr::Jmp(end_label.clone()));
                }
                out.push(AsmInstr::Label(true_label));
                out.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(1), dst_op));
                out.push(AsmInstr::Label(end_label));
                return Ok(());
            }
            let (high_true, high_false, low_true) = match op {
                TackyBinaryOp::LessThan => {
                    if is_unsigned {
                        (CondCode::B, CondCode::A, CondCode::B)
                    } else {
                        (CondCode::L, CondCode::G, CondCode::B)
                    }
                }
                TackyBinaryOp::LessEqual => {
                    if is_unsigned {
                        (CondCode::B, CondCode::A, CondCode::BE)
                    } else {
                        (CondCode::L, CondCode::G, CondCode::BE)
                    }
                }
                TackyBinaryOp::GreaterThan => {
                    if is_unsigned {
                        (CondCode::A, CondCode::B, CondCode::A)
                    } else {
                        (CondCode::G, CondCode::L, CondCode::A)
                    }
                }
                TackyBinaryOp::GreaterEqual => {
                    if is_unsigned {
                        (CondCode::A, CondCode::B, CondCode::AE)
                    } else {
                        (CondCode::G, CondCode::L, CondCode::AE)
                    }
                }
                _ => return Err(format!("unsupported x86-64 i128 comparison op: {:?}", op)),
            };
            out.push(AsmInstr::Cmp(AsmType::Quadword, right_high, left_high));
            out.push(AsmInstr::JmpCC(high_true, true_label.clone()));
            out.push(AsmInstr::JmpCC(high_false, end_label.clone()));
            out.push(AsmInstr::Label(low_label.clone()));
            out.push(AsmInstr::Cmp(AsmType::Quadword, right_low, left_low));
            out.push(AsmInstr::JmpCC(low_true, true_label.clone()));
            out.push(AsmInstr::Jmp(end_label.clone()));
            out.push(AsmInstr::Label(true_label));
            out.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(1), dst_op));
            out.push(AsmInstr::Label(end_label));
            return Ok(());
        }
        if cmp_type == AsmType::LongDouble {
        } else if matches!(cmp_type, AsmType::Float | AsmType::Double) {
            let l = convert_floating_val(cmp_type, left, static_doubles, static_floats);
            let r = convert_floating_val(cmp_type, right, static_doubles, static_floats);
            out.push(AsmInstr::Cmp(cmp_type, r, l));
        } else {
            let r = promoted_cmp_operand(out, right, cmp_type, Reg::R10, types);
            let l = promoted_cmp_operand(out, left, cmp_type, Reg::R11, types);
            out.push(AsmInstr::Cmp(cmp_type, r, l));
        }
        if matches!(
            cmp_type,
            AsmType::Float | AsmType::Double | AsmType::LongDouble
        ) {
            emit_float_comparison_result(out, op, convert_val(dst));
        } else {
            out.push(AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                convert_val(dst),
            ));
            out.push(AsmInstr::SetCC(cc, convert_val(dst)));
        }
        Ok(())
    } else {
        let t = val_type(dst, types);
        if t == AsmType::LongDouble {
            let asm_op = match op {
                TackyBinaryOp::Add => AsmX87BinaryOp::Add,
                TackyBinaryOp::Sub => AsmX87BinaryOp::Sub,
                TackyBinaryOp::Mul => AsmX87BinaryOp::Mul,
                TackyBinaryOp::Div => AsmX87BinaryOp::Div,
                _ => return Err(format!("Unsupported long double binary op: {:?}", op)),
            };
            x87_load_val(out, left, types, static_doubles);
            x87_load_val(out, right, types, static_doubles);
            out.push(AsmInstr::X87Binary(asm_op));
            out.push(AsmInstr::X87Store(convert_val(dst)));
        } else if matches!(t, AsmType::Float | AsmType::Double) {
            let asm_op = match op {
                TackyBinaryOp::Add => AsmBinaryOp::Add,
                TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                TackyBinaryOp::Mul => AsmBinaryOp::Mul,
                _ => return Err(format!("Unsupported floating binary op: {:?}", op)),
            };
            let l = convert_floating_val(t, left, static_doubles, static_floats);
            let r = convert_floating_val(t, right, static_doubles, static_floats);
            out.push(AsmInstr::Mov(t, l, convert_val(dst)));
            out.push(AsmInstr::Binary(t, asm_op, r, convert_val(dst)));
        } else if t == AsmType::Octword {
            match op {
                TackyBinaryOp::Add
                | TackyBinaryOp::Sub
                | TackyBinaryOp::Mul
                | TackyBinaryOp::BitwiseAnd
                | TackyBinaryOp::BitwiseOr
                | TackyBinaryOp::BitwiseXor
                | TackyBinaryOp::ShiftLeft
                | TackyBinaryOp::ShiftRight => {
                    let dst_op = convert_val(dst);
                    if matches!(op, TackyBinaryOp::ShiftLeft) {
                        let Some(amount) = i128_constant_shift_amount(right) else {
                            let mut labels = LabelContext {
                                function_name: ctx.function_name,
                                counter: ctx.label_counter,
                            };
                            emit_i128_variable_shift(
                                out,
                                &mut labels,
                                op,
                                left,
                                right,
                                dst,
                                types,
                            )?;
                            return Ok(());
                        };
                        if !(0..128).contains(&amount) {
                            let mut labels = LabelContext {
                                function_name: ctx.function_name,
                                counter: ctx.label_counter,
                            };
                            emit_i128_variable_shift(
                                out,
                                &mut labels,
                                op,
                                left,
                                right,
                                dst,
                                types,
                            )?;
                            return Ok(());
                        }
                        emit_i128_copy(out, left, dst)?;
                        if amount == 0 {
                            return Ok(());
                        }
                        if amount == 64 {
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                low64_operand(dst_op.clone())?,
                                high64_operand(dst_op.clone())?,
                            ));
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Imm(0),
                                low64_operand(dst_op)?,
                            ));
                            return Ok(());
                        }
                        if (65..128).contains(&amount) {
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                low64_operand(dst_op.clone())?,
                                high64_operand(dst_op.clone())?,
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Sal,
                                AsmOperand::Imm(amount - 64),
                                high64_operand(dst_op.clone())?,
                            ));
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Imm(0),
                                low64_operand(dst_op)?,
                            ));
                            return Ok(());
                        }
                        if (1..64).contains(&amount) {
                            let dst_low = low64_operand(dst_op.clone())?;
                            let dst_high = high64_operand(dst_op.clone())?;
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                dst_low.clone(),
                                AsmOperand::Reg(Reg::R10),
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Sal,
                                AsmOperand::Imm(amount),
                                dst_high.clone(),
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Shr,
                                AsmOperand::Imm(64 - amount),
                                AsmOperand::Reg(Reg::R10),
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Or,
                                AsmOperand::Reg(Reg::R10),
                                dst_high,
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Sal,
                                AsmOperand::Imm(amount),
                                dst_low,
                            ));
                            return Ok(());
                        }
                        return Err(format!(
                            "internal error: unhandled 128-bit left shift amount: {}",
                            amount
                        ));
                    }
                    if matches!(op, TackyBinaryOp::ShiftRight) {
                        let Some(amount) = i128_constant_shift_amount(right) else {
                            let mut labels = LabelContext {
                                function_name: ctx.function_name,
                                counter: ctx.label_counter,
                            };
                            emit_i128_variable_shift(
                                out,
                                &mut labels,
                                op,
                                left,
                                right,
                                dst,
                                types,
                            )?;
                            return Ok(());
                        };
                        if !(0..128).contains(&amount) {
                            let mut labels = LabelContext {
                                function_name: ctx.function_name,
                                counter: ctx.label_counter,
                            };
                            emit_i128_variable_shift(
                                out,
                                &mut labels,
                                op,
                                left,
                                right,
                                dst,
                                types,
                            )?;
                            return Ok(());
                        }
                        emit_i128_copy(out, left, dst)?;
                        let dst_low = low64_operand(dst_op.clone())?;
                        let dst_high = high64_operand(dst_op.clone())?;
                        let high_shift = if is_unsigned {
                            AsmBinaryOp::Shr
                        } else {
                            AsmBinaryOp::Sar
                        };
                        if amount == 0 {
                            return Ok(());
                        }
                        if amount == 64 {
                            out.push(AsmInstr::Mov(AsmType::Quadword, dst_high.clone(), dst_low));
                            let fill = if is_unsigned {
                                AsmOperand::Imm(0)
                            } else {
                                out.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    dst_high.clone(),
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                out.push(AsmInstr::Binary(
                                    AsmType::Quadword,
                                    AsmBinaryOp::Sar,
                                    AsmOperand::Imm(63),
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                AsmOperand::Reg(Reg::R10)
                            };
                            out.push(AsmInstr::Mov(AsmType::Quadword, fill, dst_high));
                            return Ok(());
                        }
                        if (65..128).contains(&amount) {
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                dst_high.clone(),
                                dst_low.clone(),
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                high_shift.clone(),
                                AsmOperand::Imm(amount - 64),
                                dst_low,
                            ));
                            if is_unsigned {
                                out.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    AsmOperand::Imm(0),
                                    dst_high,
                                ));
                            } else {
                                out.push(AsmInstr::Binary(
                                    AsmType::Quadword,
                                    AsmBinaryOp::Sar,
                                    AsmOperand::Imm(63),
                                    dst_high,
                                ));
                            }
                            return Ok(());
                        }
                        if (1..64).contains(&amount) {
                            out.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                dst_high.clone(),
                                AsmOperand::Reg(Reg::R10),
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Shr,
                                AsmOperand::Imm(amount),
                                dst_low.clone(),
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Sal,
                                AsmOperand::Imm(64 - amount),
                                AsmOperand::Reg(Reg::R10),
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Or,
                                AsmOperand::Reg(Reg::R10),
                                dst_low,
                            ));
                            out.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                high_shift,
                                AsmOperand::Imm(amount),
                                dst_high,
                            ));
                            return Ok(());
                        }
                        return Err(format!(
                            "internal error: unhandled 128-bit right shift amount: {}",
                            amount
                        ));
                    }
                    if matches!(op, TackyBinaryOp::Add) {
                        if i128_constant_is_zero(left) {
                            emit_i128_copy(out, right, dst)?;
                            return Ok(());
                        }
                        if i128_constant_is_zero(right) {
                            emit_i128_copy(out, left, dst)?;
                            return Ok(());
                        }
                    }
                    if matches!(op, TackyBinaryOp::Sub) && i128_constant_is_zero(right) {
                        emit_i128_copy(out, left, dst)?;
                        return Ok(());
                    }
                    if matches!(op, TackyBinaryOp::Mul) {
                        if i128_constant_is_zero(left) || i128_constant_is_zero(right) {
                            emit_i128_zero(out, dst)?;
                            return Ok(());
                        }
                        if i128_constant_is_one(left) {
                            emit_i128_copy(out, right, dst)?;
                            return Ok(());
                        }
                        if i128_constant_is_one(right) {
                            emit_i128_copy(out, left, dst)?;
                            return Ok(());
                        }
                    }
                    if matches!(op, TackyBinaryOp::BitwiseAnd) {
                        if i128_constant_is_zero(left) || i128_constant_is_zero(right) {
                            emit_i128_zero(out, dst)?;
                            return Ok(());
                        }
                        if i128_constant_is_all_ones(left) {
                            emit_i128_copy(out, right, dst)?;
                            return Ok(());
                        }
                        if i128_constant_is_all_ones(right) {
                            emit_i128_copy(out, left, dst)?;
                            return Ok(());
                        }
                    }
                    if matches!(op, TackyBinaryOp::BitwiseOr | TackyBinaryOp::BitwiseXor) {
                        if i128_constant_is_zero(left) {
                            emit_i128_copy(out, right, dst)?;
                            return Ok(());
                        }
                        if i128_constant_is_zero(right) {
                            emit_i128_copy(out, left, dst)?;
                            return Ok(());
                        }
                    }

                    emit_i128_copy(out, left, dst)?;
                    let (right_low, right_high) = i128_part_operands(right)?;
                    if matches!(op, TackyBinaryOp::Mul) {
                        let left_low = low64_operand(dst_op.clone())?;
                        let left_high = high64_operand(dst_op.clone())?;
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            left_low.clone(),
                            AsmOperand::Reg(Reg::R10),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            left_high.clone(),
                            AsmOperand::Reg(Reg::R11),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Reg(Reg::R10),
                            AsmOperand::Reg(Reg::AX),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            right_low.clone(),
                            AsmOperand::Reg(Reg::DX),
                        ));
                        out.push(AsmInstr::MulFull(
                            AsmType::Quadword,
                            AsmOperand::Reg(Reg::DX),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Reg(Reg::AX),
                            left_low,
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Reg(Reg::DX),
                            left_high.clone(),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            right_high,
                            AsmOperand::Reg(Reg::DX),
                        ));
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            AsmBinaryOp::Mul,
                            AsmOperand::Reg(Reg::DX),
                            AsmOperand::Reg(Reg::R10),
                        ));
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            AsmBinaryOp::Add,
                            AsmOperand::Reg(Reg::R10),
                            left_high.clone(),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Reg(Reg::R11),
                            AsmOperand::Reg(Reg::R10),
                        ));
                        out.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            right_low,
                            AsmOperand::Reg(Reg::DX),
                        ));
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            AsmBinaryOp::Mul,
                            AsmOperand::Reg(Reg::DX),
                            AsmOperand::Reg(Reg::R10),
                        ));
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            AsmBinaryOp::Add,
                            AsmOperand::Reg(Reg::R10),
                            left_high,
                        ));
                        return Ok(());
                    }
                    if matches!(
                        op,
                        TackyBinaryOp::BitwiseAnd
                            | TackyBinaryOp::BitwiseOr
                            | TackyBinaryOp::BitwiseXor
                    ) {
                        let asm_op = match op {
                            TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                            TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                            TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                            _ => {
                                return Err("internal error: expected bitwise binary op".to_string())
                            }
                        };
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            asm_op.clone(),
                            right_low,
                            low64_operand(dst_op.clone())?,
                        ));
                        out.push(AsmInstr::Binary(
                            AsmType::Quadword,
                            asm_op,
                            right_high,
                            high64_operand(dst_op)?,
                        ));
                        return Ok(());
                    }
                    let asm_op = if matches!(op, TackyBinaryOp::Add) {
                        AsmBinaryOp::AddSetFlags
                    } else {
                        AsmBinaryOp::SubSetFlags
                    };
                    out.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        asm_op.clone(),
                        right_low,
                        low64_operand(dst_op.clone())?,
                    ));
                    out.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        if matches!(op, TackyBinaryOp::Add) {
                            AsmBinaryOp::Adc
                        } else {
                            AsmBinaryOp::Sbb
                        },
                        right_high,
                        high64_operand(dst_op)?,
                    ));
                }
                _ => {
                    return Err(format!(
                        "x86-64 backend does not support 128-bit binary op yet: {:?}",
                        op
                    ));
                }
            }
        } else {
            let asm_op = match op {
                TackyBinaryOp::Add => AsmBinaryOp::Add,
                TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                TackyBinaryOp::Mul => AsmBinaryOp::Mul,
                TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                _ => return Err(format!("unsupported x86-64 binary op: {:?}", op)),
            };
            out.push(AsmInstr::Mov(t, convert_val(left), convert_val(dst)));
            out.push(AsmInstr::Binary(
                t,
                asm_op,
                convert_val(right),
                convert_val(dst),
            ));
        }
        Ok(())
    }
}

const XMM_ARG_REGISTERS: [XmmReg; 8] = [
    XmmReg::XMM0,
    XmmReg::XMM1,
    XmmReg::XMM2,
    XmmReg::XMM3,
    XmmReg::XMM4,
    XmmReg::XMM5,
    XmmReg::XMM6,
    XmmReg::XMM7,
];

struct X86FunctionContext<'a> {
    target: &'a Target,
    types: &'a IndexMap<String, CType>,
    var_struct_tags: &'a HashMap<String, String>,
    struct_defs: &'a IndexMap<String, StructDef>,
    local_function_names: &'a std::collections::HashSet<String>,
}

fn convert_function(
    func: &TackyFunction,
    ctx: &X86FunctionContext<'_>,
    static_doubles: &mut Vec<(String, f64)>,
    static_floats: &mut Vec<(String, f32)>,
) -> Result<AsmFunction, String> {
    let mut instructions = Vec::with_capacity(func.body.len() + func.params.len() * 2 + 8);
    static I128_LABEL_BASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let mut label_counter = I128_LABEL_BASE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // System V ABI: integer args in DI,SI,DX,CX,R8,R9; double args in XMM0-XMM7
    let mut int_reg_idx = 0usize;
    let mut xmm_reg_idx = 0usize;
    let mut stack_arg_offset = 0usize;
    let mut memory_param_blocks: HashMap<usize, (&String, usize)> =
        HashMap::with_capacity(func.memory_param_blocks.len());
    for (index, name, size) in &func.memory_param_blocks {
        memory_param_blocks.insert(*index, (name, *size));
    }

    // Pre-compute which params must go on stack due to struct group overflow
    let mut force_stack: std::collections::HashSet<usize> =
        std::collections::HashSet::with_capacity(func.params.len());
    {
        let mut sim_int_idx = 0usize;
        let mut sim_xmm_idx = 0usize;
        // Account for hidden return pointer
        for (i, param) in func.params.iter().enumerate() {
            if memory_param_blocks.contains_key(&i) {
                continue;
            }
            if func.stack_params.contains(param) {
                force_stack.insert(i);
                continue;
            }
            // Check if this param is part of a struct group
            let group = func
                .struct_param_groups
                .iter()
                .find(|(start, count, _)| i >= *start && i < *start + *count);
            if let Some((start, count, is_sse_vec)) = group {
                if i == *start {
                    // First eightbyte in group — check if ALL fit
                    let int_needed: usize = is_sse_vec.iter().filter(|&&is_sse| !is_sse).count();
                    let sse_needed: usize = is_sse_vec.iter().filter(|&&is_sse| is_sse).count();
                    if sim_int_idx + int_needed <= 6 && sim_xmm_idx + sse_needed <= 8 {
                        // All fit — consume registers
                        sim_int_idx += int_needed;
                        sim_xmm_idx += sse_needed;
                    } else {
                        // Don't fit — force all to stack
                        for j in *start..*start + *count {
                            force_stack.insert(j);
                        }
                    }
                }
                // Skip non-first eightbytes (already handled)
                continue;
            }
            // Regular param
            let t: AsmType = ctx.types.get(param).copied().unwrap_or(CType::Int).into();
            if t == AsmType::LongDouble {
                force_stack.insert(i);
            } else if matches!(t, AsmType::Float | AsmType::Double) {
                if sim_xmm_idx < 8 {
                    sim_xmm_idx += 1;
                }
                // else overflow
            } else {
                if sim_int_idx < 6 {
                    sim_int_idx += 1;
                }
            }
        }
    }

    let mut register_param_instructions = Vec::with_capacity(func.params.len());
    let mut stack_param_instructions = Vec::with_capacity(func.params.len());

    for (i, param) in func.params.iter().enumerate() {
        if let Some((dst_name, size)) = memory_param_blocks.get(&i).copied() {
            let align = if ctx.types.get(dst_name) == Some(&CType::UInt128) {
                // Oversized vectors are MEMORY-class values but retain their
                // mandatory 16-byte ABI stack alignment.
                16
            } else {
                get_struct_def(dst_name, ctx.var_struct_tags, ctx.struct_defs)
                    .map(|def| def.alignment.clamp(1, 16))
                    .unwrap_or(8)
            };
            let layout = StackArgLayout::for_memory_block(size, align);
            stack_arg_offset = layout.place_at(stack_arg_offset);
            let offset = 16 + stack_arg_offset as i32;
            stack_param_instructions.push(AsmInstr::CopyFromStackArg {
                src_offset: offset,
                dst: AsmOperand::PseudoMem(dst_name.clone(), 0),
                size,
            });
            stack_arg_offset += layout.size;
            continue;
        }
        if force_stack.contains(&i) || func.stack_params.contains(param) {
            let t: AsmType = ctx.types.get(param).copied().unwrap_or(CType::Long).into();
            let layout = StackArgLayout::for_scalar(t);
            stack_arg_offset = layout.place_at(stack_arg_offset);
            let offset = 16 + stack_arg_offset as i32;
            if t == AsmType::LongDouble {
                stack_param_instructions.push(AsmInstr::X87Load(
                    AsmType::LongDouble,
                    AsmOperand::Stack(i64::from(offset)),
                ));
                stack_param_instructions
                    .push(AsmInstr::X87Store(AsmOperand::Pseudo(param.clone())));
            } else if t == AsmType::Octword {
                stack_param_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(i64::from(offset)),
                    AsmOperand::PseudoMem(param.clone(), 0),
                ));
                stack_param_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(i64::from(offset + 8)),
                    AsmOperand::PseudoMem(param.clone(), 8),
                ));
            } else {
                stack_param_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Stack(i64::from(offset)),
                    AsmOperand::Pseudo(param.clone()),
                ));
            }
            stack_arg_offset += layout.size;
            continue;
        }
        let t: AsmType = ctx.types.get(param).copied().unwrap_or(CType::Int).into();
        if t == AsmType::LongDouble {
            let layout = StackArgLayout::for_scalar(t);
            stack_arg_offset = layout.place_at(stack_arg_offset);
            let offset = 16 + stack_arg_offset as i32;
            stack_param_instructions.push(AsmInstr::X87Load(
                AsmType::LongDouble,
                AsmOperand::Stack(i64::from(offset)),
            ));
            stack_param_instructions.push(AsmInstr::X87Store(AsmOperand::Pseudo(param.clone())));
            stack_arg_offset += layout.size;
        } else if matches!(t, AsmType::Float | AsmType::Double) {
            if xmm_reg_idx < 8 {
                register_param_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Xmm(XMM_ARG_REGISTERS[xmm_reg_idx]),
                    AsmOperand::Pseudo(param.clone()),
                ));
                xmm_reg_idx += 1;
            } else {
                let layout = StackArgLayout::for_scalar(t);
                stack_arg_offset = layout.place_at(stack_arg_offset);
                let offset = 16 + stack_arg_offset as i32;
                stack_param_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Stack(i64::from(offset)),
                    AsmOperand::Pseudo(param.clone()),
                ));
                stack_arg_offset += layout.size;
            }
        } else if t == AsmType::Octword {
            if int_reg_idx + 1 < 6 {
                register_param_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(ARG_REGISTERS[int_reg_idx]),
                    AsmOperand::PseudoMem(param.clone(), 0),
                ));
                register_param_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(ARG_REGISTERS[int_reg_idx + 1]),
                    AsmOperand::PseudoMem(param.clone(), 8),
                ));
                int_reg_idx += 2;
            } else {
                let layout = StackArgLayout::for_scalar(t);
                stack_arg_offset = layout.place_at(stack_arg_offset);
                let offset = 16 + stack_arg_offset as i32;
                stack_param_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(i64::from(offset)),
                    AsmOperand::PseudoMem(param.clone(), 0),
                ));
                stack_param_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(i64::from(offset + 8)),
                    AsmOperand::PseudoMem(param.clone(), 8),
                ));
                stack_arg_offset += layout.size;
            }
        } else {
            if int_reg_idx < 6 {
                register_param_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Reg(ARG_REGISTERS[int_reg_idx]),
                    AsmOperand::Pseudo(param.clone()),
                ));
                int_reg_idx += 1;
            } else {
                let layout = StackArgLayout::for_scalar(t);
                stack_arg_offset = layout.place_at(stack_arg_offset);
                let offset = 16 + stack_arg_offset as i32;
                stack_param_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Stack(i64::from(offset)),
                    AsmOperand::Pseudo(param.clone()),
                ));
                stack_arg_offset += layout.size;
            }
        }
    }
    instructions.extend(register_param_instructions);
    instructions.extend(stack_param_instructions);

    for instr in &func.body {
        let va_start_stack_offset = 16 + stack_arg_offset as i32;
        let mut instruction_ctx = InstructionContext {
            function_name: &func.name,
            return_type: func.return_type,
            target: ctx.target,
            types: ctx.types,
            out: &mut instructions,
            static_doubles,
            static_floats,
            label_counter: &mut label_counter,
            var_struct_tags: ctx.var_struct_tags,
            struct_defs: ctx.struct_defs,
            local_function_names: ctx.local_function_names,
            va_start_stack_offset,
        };
        convert_instruction(instr, &mut instruction_ctx)?;
    }
    Ok(AsmFunction {
        name: func.name.clone(),
        global: func.global,
        instructions,
    })
}

/// Remove the temporary boolean in `cmp; mov $0; setcc; cmp $0; j{e,ne}`
/// before register allocation. The intervening instructions preserve the
/// comparison flags, and the temporary is used only by the following branch.
fn fuse_setcc_branches(func: &mut AsmFunction) {
    fn invert_condition(cc: CondCode) -> CondCode {
        match cc {
            CondCode::E => CondCode::NE,
            CondCode::NE => CondCode::E,
            CondCode::L => CondCode::GE,
            CondCode::LE => CondCode::G,
            CondCode::G => CondCode::LE,
            CondCode::GE => CondCode::L,
            CondCode::B => CondCode::AE,
            CondCode::BE => CondCode::A,
            CondCode::A => CondCode::BE,
            CondCode::AE => CondCode::B,
            CondCode::P => CondCode::NP,
            CondCode::NP => CondCode::P,
            CondCode::S => CondCode::NS,
            CondCode::NS => CondCode::S,
        }
    }

    let old_instructions = std::mem::take(&mut func.instructions);
    let mut instructions = Vec::with_capacity(old_instructions.len());
    let mut index = 0;
    while index < old_instructions.len() {
        let fused_branch = match (
            old_instructions.get(index),
            old_instructions.get(index + 1),
            old_instructions.get(index + 2),
            old_instructions.get(index + 3),
            old_instructions.get(index + 4),
        ) {
            (
                Some(AsmInstr::Cmp(..)),
                Some(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Imm(0),
                    AsmOperand::Pseudo(zeroed),
                )),
                Some(AsmInstr::SetCC(set_cc, AsmOperand::Pseudo(set_dst))),
                Some(AsmInstr::Cmp(
                    AsmType::Longword,
                    AsmOperand::Imm(0),
                    AsmOperand::Pseudo(compared),
                )),
                Some(AsmInstr::JmpCC(branch_cc, label)),
            ) if zeroed == set_dst && zeroed == compared => match branch_cc {
                CondCode::E => Some((invert_condition(*set_cc), label)),
                CondCode::NE => Some((*set_cc, label)),
                _ => None,
            },
            _ => None,
        };
        if let Some((cc, label)) = fused_branch {
            instructions.push(old_instructions[index].clone());
            instructions.push(AsmInstr::JmpCC(cc, label.clone()));
            index += 5;
        } else {
            instructions.push(old_instructions[index].clone());
            index += 1;
        }
    }
    func.instructions = instructions;
}

// ============================================================
// Phase 2: Replace pseudo-registers with stack slots
// ============================================================

struct ReplacePseudoContext<'a> {
    statics: &'a std::collections::HashSet<String>,
    tls_vars: &'a std::collections::HashSet<String>,
    types: &'a IndexMap<String, CType>,
    arr_sizes: &'a IndexMap<String, usize>,
    alignments: &'a IndexMap<String, usize>,
    var_struct_tags: &'a HashMap<String, String>,
    struct_defs: &'a IndexMap<String, StructDef>,
}

fn replace_pseudos(func: &mut AsmFunction, ctx: &ReplacePseudoContext<'_>) -> Result<i64, String> {
    let mut pseudo_map: HashMap<String, i64> = HashMap::with_capacity(func.instructions.len());
    let mut stack_offset: i64 = 0;

    fn stack_size_for_name(name: &str, ctx: &ReplacePseudoContext<'_>) -> Result<i64, String> {
        if let Some(&arr_size) = ctx.arr_sizes.get(name) {
            return i64::try_from(arr_size)
                .map_err(|_| format!("stack object `{}` is too large", name));
        }
        let ct = ctx.types.get(name).copied().unwrap_or(CType::Int);
        if ct == CType::Void {
            Ok(4)
        } else if ct == CType::Struct {
            ctx.var_struct_tags
                .get(name)
                .and_then(|tag| ctx.struct_defs.get(tag))
                .map(|def| {
                    i64::try_from(def.size)
                        .map_err(|_| format!("stack object `{}` is too large", name))
                })
                .unwrap_or(Ok(0))
        } else {
            Ok(i64::from(std::cmp::max(ct.size(), 1)))
        }
    }

    fn stack_align_for_name(
        name: &str,
        size: i64,
        ctx: &ReplacePseudoContext<'_>,
    ) -> Result<i64, String> {
        let align = if let Some(&decl_align) = ctx.alignments.get(name) {
            decl_align
        } else if let Some(&arr_size) = ctx.arr_sizes.get(name) {
            if arr_size >= 16 {
                16
            } else {
                std::cmp::max(size.min(16) as usize, 1)
            }
        } else {
            std::cmp::max(size.min(16) as usize, 1)
        };
        i64::try_from(align).map_err(|_| format!("stack object `{}` alignment is too large", name))
    }

    fn allocate_stack_slot(
        name: &str,
        map: &mut HashMap<String, i64>,
        offset: &mut i64,
        ctx: &ReplacePseudoContext<'_>,
    ) -> Result<i64, String> {
        if let Some(&o) = map.get(name) {
            return Ok(o);
        }

        let size = stack_size_for_name(name, ctx)?;
        let align = stack_align_for_name(name, size, ctx)?;
        *offset = offset
            .checked_sub(size)
            .ok_or_else(|| format!("stack frame for `{}` is too large", name))?;
        *offset &= -align;
        map.insert(name.to_string(), *offset);
        Ok(*offset)
    }

    fn replace_operand(
        op: &mut AsmOperand,
        map: &mut HashMap<String, i64>,
        offset: &mut i64,
        ctx: &ReplacePseudoContext<'_>,
    ) -> Result<(), String> {
        match op {
            AsmOperand::Pseudo(name) => {
                let name = name.clone();
                if ctx.tls_vars.contains(&name) {
                    *op = AsmOperand::TlsData(name, 0);
                } else if ctx.statics.contains(&name) {
                    *op = AsmOperand::Data(name);
                } else {
                    let off = allocate_stack_slot(&name, map, offset, ctx)?;
                    *op = AsmOperand::Stack(off);
                }
            }
            AsmOperand::PseudoMem(name, mem_offset) => {
                let name = name.clone();
                let mem_off = *mem_offset;
                if ctx.tls_vars.contains(&name) {
                    *op = AsmOperand::TlsData(name, mem_off);
                } else if ctx.statics.contains(&name) {
                    if mem_off != 0 {
                        *op = AsmOperand::Data(format!(
                            "{}{}",
                            name,
                            assembly_offset_suffix(i64::from(mem_off))
                        ));
                    } else {
                        *op = AsmOperand::Data(name);
                    }
                } else {
                    let base_off = allocate_stack_slot(&name, map, offset, ctx)?;
                    let stack_off = base_off
                        .checked_add(i64::from(mem_off))
                        .ok_or_else(|| format!("stack access for `{}` overflows", name))?;
                    *op = AsmOperand::Stack(stack_off);
                }
            }
            AsmOperand::StackArg(_) => {}
            _ => {}
        }
        Ok(())
    }

    for instr in &mut func.instructions {
        match instr {
            AsmInstr::Mov(_, src, dst) | AsmInstr::Cmp(_, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::Binary(_, _, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::Unary(_, _, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::MulFull(_, op) | AsmInstr::Idiv(_, op) | AsmInstr::Div(_, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::SetCC(_, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::Push(op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::JmpIndirect(target) => {
                replace_operand(target, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvtsi2ss(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Cvttss2si(_, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::Cvtss2sd(src, dst) | AsmInstr::Cvtsd2ss(src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::X87Load(_, src) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::X87Store(dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::X87StoreFloat(_, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::X87StoreInt(_, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::X87LoadIndirect(_, _) | AsmInstr::X87StoreIndirect(_) => {}
            AsmInstr::Lea(src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::LoadLabelAddress(_, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::LoadIndirect(_, _, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::CopyToStackArg { src_ptr, .. } => {
                replace_operand(src_ptr, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::CopyFromStackArg { dst, .. } => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::StoreIndirect(_, src, _) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::BuiltinSetjmp { buf, dst, .. } => {
                replace_operand(buf, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            AsmInstr::BuiltinLongjmp { buf, value } => {
                replace_operand(buf, &mut pseudo_map, &mut stack_offset, ctx)?;
                replace_operand(value, &mut pseudo_map, &mut stack_offset, ctx)?;
            }
            _ => {}
        }
    }

    Ok(-stack_offset)
}

// ============================================================
// Phase 3: Fix up invalid instructions
// ============================================================

fn is_memory(op: &AsmOperand) -> bool {
    matches!(
        op,
        AsmOperand::Stack(_)
            | AsmOperand::Data(_)
            | AsmOperand::TlsData(_, _)
            | AsmOperand::StackArg(_)
    )
}

fn stack_offset_exceeds_disp32(op: &AsmOperand) -> Option<i64> {
    match op {
        AsmOperand::Stack(offset)
            if *offset > i64::from(i32::MAX) || *offset < i64::from(i32::MIN) =>
        {
            Some(*offset)
        }
        _ => None,
    }
}

fn materialize_stack_address(out: &mut Vec<AsmInstr>, offset: i64, scratch: Reg) {
    out.push(AsmInstr::Mov(
        AsmType::Quadword,
        AsmOperand::Imm(offset),
        AsmOperand::Reg(scratch),
    ));
    out.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Add,
        AsmOperand::Reg(Reg::BP),
        AsmOperand::Reg(scratch),
    ));
}

fn fixup_instructions(func: &mut AsmFunction, stack_size: i64, callee_saved: &[Reg]) {
    let num_cs = callee_saved.len() as i64;
    let total_aligned = (stack_size + 8 * num_cs + 15) & !15;
    let adjusted_stack = total_aligned - 8 * num_cs;
    let old_instructions = std::mem::take(&mut func.instructions);
    let mut new_instructions = Vec::with_capacity(old_instructions.len() + callee_saved.len() + 2);

    // Prologue placeholder
    new_instructions.push(AsmInstr::Push(AsmOperand::Reg(Reg::AX)));
    new_instructions.push(AsmInstr::AllocateStack(adjusted_stack));
    // Push callee-saved registers
    for reg in callee_saved {
        new_instructions.push(AsmInstr::Push(AsmOperand::Reg(*reg)));
    }

    for instr in old_instructions {
        match instr {
            AsmInstr::Lea(AsmOperand::Stack(offset), ref dst)
                if stack_offset_exceeds_disp32(&AsmOperand::Stack(offset)).is_some() =>
            {
                materialize_stack_address(&mut new_instructions, offset, Reg::R10);
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            AsmInstr::Mov(t, ref src, ref dst)
                if stack_offset_exceeds_disp32(src).is_some()
                    || stack_offset_exceeds_disp32(dst).is_some() =>
            {
                match (
                    stack_offset_exceeds_disp32(src),
                    stack_offset_exceeds_disp32(dst),
                ) {
                    (Some(src_offset), Some(dst_offset)) => {
                        materialize_stack_address(&mut new_instructions, src_offset, Reg::R10);
                        materialize_stack_address(&mut new_instructions, dst_offset, Reg::R11);
                        if matches!(t, AsmType::Float | AsmType::Double) {
                            new_instructions.push(AsmInstr::LoadIndirect(
                                t,
                                Reg::R10,
                                AsmOperand::Xmm(XmmReg::XMM14),
                            ));
                            new_instructions.push(AsmInstr::StoreIndirect(
                                t,
                                AsmOperand::Xmm(XmmReg::XMM14),
                                Reg::R11,
                            ));
                        } else {
                            new_instructions.push(AsmInstr::LoadIndirect(
                                t,
                                Reg::R10,
                                AsmOperand::Reg(Reg::R10),
                            ));
                            new_instructions.push(AsmInstr::StoreIndirect(
                                t,
                                AsmOperand::Reg(Reg::R10),
                                Reg::R11,
                            ));
                        }
                    }
                    (Some(src_offset), None) => {
                        materialize_stack_address(&mut new_instructions, src_offset, Reg::R10);
                        if is_memory(dst) {
                            if matches!(t, AsmType::Float | AsmType::Double) {
                                new_instructions.push(AsmInstr::LoadIndirect(
                                    t,
                                    Reg::R10,
                                    AsmOperand::Xmm(XmmReg::XMM14),
                                ));
                                new_instructions.push(AsmInstr::Mov(
                                    t,
                                    AsmOperand::Xmm(XmmReg::XMM14),
                                    dst.clone(),
                                ));
                            } else {
                                new_instructions.push(AsmInstr::LoadIndirect(
                                    t,
                                    Reg::R10,
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                new_instructions.push(AsmInstr::Mov(
                                    t,
                                    AsmOperand::Reg(Reg::R10),
                                    dst.clone(),
                                ));
                            }
                        } else {
                            new_instructions.push(AsmInstr::LoadIndirect(t, Reg::R10, dst.clone()));
                        }
                    }
                    (None, Some(dst_offset)) => {
                        materialize_stack_address(&mut new_instructions, dst_offset, Reg::R11);
                        if is_memory(src) {
                            if matches!(t, AsmType::Float | AsmType::Double) {
                                new_instructions.push(AsmInstr::Mov(
                                    t,
                                    src.clone(),
                                    AsmOperand::Xmm(XmmReg::XMM14),
                                ));
                                new_instructions.push(AsmInstr::StoreIndirect(
                                    t,
                                    AsmOperand::Xmm(XmmReg::XMM14),
                                    Reg::R11,
                                ));
                            } else {
                                new_instructions.push(AsmInstr::Mov(
                                    t,
                                    src.clone(),
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                new_instructions.push(AsmInstr::StoreIndirect(
                                    t,
                                    AsmOperand::Reg(Reg::R10),
                                    Reg::R11,
                                ));
                            }
                        } else {
                            new_instructions.push(AsmInstr::StoreIndirect(
                                t,
                                src.clone(),
                                Reg::R11,
                            ));
                        }
                    }
                    (None, None) => unreachable!(),
                }
            }
            // mov mem, mem
            AsmInstr::Mov(t @ (AsmType::Float | AsmType::Double), ref src, ref dst)
                if is_memory(src) && is_memory(dst) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            AsmInstr::Mov(AsmType::LongDouble, ref src, ref dst)
                if is_memory(src) && is_memory(dst) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::LongDouble,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::LongDouble,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            AsmInstr::Mov(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            AsmInstr::Movsx(st, dt, ref src, ref dst) if st == dt => {
                new_instructions.push(AsmInstr::Mov(dt, src.clone(), dst.clone()));
            }
            // movsx with memory dst (movslq can't write to memory)
            AsmInstr::Movsx(st, dt, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Movsx(
                    st,
                    dt,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(dt, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // movzx with memory dst
            AsmInstr::MovZeroExtend(st, dt, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::MovZeroExtend(
                    st,
                    dt,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(dt, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // full multiply / divide cannot use immediate operands
            AsmInstr::MulFull(t, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::MulFull(t, AsmOperand::Reg(Reg::R10)));
            }
            // idiv imm / div imm
            AsmInstr::Idiv(t, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Idiv(t, AsmOperand::Reg(Reg::R10)));
            }
            AsmInstr::Div(t, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Div(t, AsmOperand::Reg(Reg::R10)));
            }
            // mul with memory dst (integer only)
            AsmInstr::Binary(AsmType::Byte, AsmBinaryOp::Mul, ref src, ref dst) => {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Imm(0),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Imm(0),
                    AsmOperand::Reg(Reg::R11),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Byte,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Byte,
                    dst.clone(),
                    AsmOperand::Reg(Reg::R11),
                ));
                new_instructions.push(AsmInstr::Binary(
                    AsmType::Longword,
                    AsmBinaryOp::Mul,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::Reg(Reg::R11),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Byte,
                    AsmOperand::Reg(Reg::R11),
                    dst.clone(),
                ));
            }
            AsmInstr::Binary(t, AsmBinaryOp::Mul, ref src, ref dst)
                if is_memory(dst) && !matches!(t, AsmType::Float | AsmType::Double) =>
            {
                if is_memory(src) {
                    new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                    new_instructions.push(AsmInstr::Mov(t, dst.clone(), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Binary(
                        t,
                        AsmBinaryOp::Mul,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Reg(Reg::R11),
                    ));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R11), dst.clone()));
                } else {
                    new_instructions.push(AsmInstr::Mov(t, dst.clone(), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Binary(
                        t,
                        AsmBinaryOp::Mul,
                        src.clone(),
                        AsmOperand::Reg(Reg::R11),
                    ));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R11), dst.clone()));
                }
            }
            // double binary mem, mem
            // double binary: dst must be XMM register
            AsmInstr::Binary(t @ (AsmType::Float | AsmType::Double), ref op, ref src, ref dst)
                if is_memory(dst) || is_memory(src) =>
            {
                // Load dst into XMM14 (for operations like addsd, dst is both src and dest)
                let dst_xmm = AsmOperand::Xmm(XmmReg::XMM14);
                new_instructions.push(AsmInstr::Mov(t, dst.clone(), dst_xmm.clone()));
                let src_op = if is_memory(src) && !matches!(src, AsmOperand::Xmm(_)) {
                    // src can be memory for SSE ops like addsd mem, xmm
                    src.clone()
                } else {
                    src.clone()
                };
                new_instructions.push(AsmInstr::Binary(t, op.clone(), src_op, dst_xmm.clone()));
                new_instructions.push(AsmInstr::Mov(t, dst_xmm, dst.clone()));
            }
            // binary mem, mem (integer)
            AsmInstr::Binary(t, ref op, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Binary(
                    t,
                    op.clone(),
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // double cmp: comisd src, dst — dst MUST be xmm register
            AsmInstr::Cmp(t @ (AsmType::Float | AsmType::Double), ref src, ref dst)
                if !matches!(dst, AsmOperand::Xmm(_)) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    dst.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Cmp(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
            }
            AsmInstr::Cvtss2sd(ref src, ref dst) if !matches!(dst, AsmOperand::Xmm(_)) => {
                new_instructions.push(AsmInstr::Cvtss2sd(
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            AsmInstr::Cvtsd2ss(ref src, ref dst) if !matches!(dst, AsmOperand::Xmm(_)) => {
                new_instructions.push(AsmInstr::Cvtsd2ss(
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Float,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            // cmp mem, mem
            AsmInstr::Cmp(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Cmp(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // cmp src, imm (dst can't be immediate)
            AsmInstr::Cmp(t, ref src, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R11),
                ));
                new_instructions.push(AsmInstr::Cmp(t, src.clone(), AsmOperand::Reg(Reg::R11)));
            }
            // cmp with large immediate src that doesn't fit in 32 bits
            AsmInstr::Cmp(AsmType::Quadword, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Cmp(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // binary with large immediate for quadword
            AsmInstr::Binary(AsmType::Quadword, ref op, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    op.clone(),
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // mov with large immediate for quadword
            AsmInstr::Mov(AsmType::Quadword, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) && is_memory(dst) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // cvtsi2sd with immediate src
            AsmInstr::Cvtsi2sd(t, AsmOperand::Imm(val), ref dst) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                if is_memory(dst) {
                    new_instructions.push(AsmInstr::Cvtsi2sd(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::Mov(
                        AsmType::Double,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        dst.clone(),
                    ));
                } else {
                    new_instructions.push(AsmInstr::Cvtsi2sd(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        dst.clone(),
                    ));
                }
            }
            // cvtsi2ss with immediate src
            AsmInstr::Cvtsi2ss(t, AsmOperand::Imm(val), ref dst) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                if is_memory(dst) {
                    new_instructions.push(AsmInstr::Cvtsi2ss(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::Mov(
                        AsmType::Float,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        dst.clone(),
                    ));
                } else {
                    new_instructions.push(AsmInstr::Cvtsi2ss(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        dst.clone(),
                    ));
                }
            }
            // cvtsi2sd with memory dst
            AsmInstr::Cvtsi2sd(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvtsi2sd(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            // cvtsi2ss with memory dst
            AsmInstr::Cvtsi2ss(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvtsi2ss(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Float,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            // cvttsd2si with memory src AND memory dst
            AsmInstr::Cvttsd2si(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Cvttsd2si(
                    t,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // cvttss2si with memory src AND memory dst
            AsmInstr::Cvttss2si(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Float,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Cvttss2si(
                    t,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            AsmInstr::Cvttsd2si(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvttsd2si(
                    t,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            AsmInstr::Cvttss2si(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvttss2si(
                    t,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // lea with memory dst
            AsmInstr::Lea(ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Lea(src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // LoadIndirect with memory dst
            AsmInstr::LoadIndirect(t, ref reg, ref dst) if is_memory(dst) => {
                if matches!(t, AsmType::Float | AsmType::Double) {
                    new_instructions.push(AsmInstr::LoadIndirect(
                        t,
                        *reg,
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::Mov(
                        t,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        dst.clone(),
                    ));
                } else {
                    new_instructions.push(AsmInstr::LoadIndirect(
                        t,
                        *reg,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
                }
            }
            // StoreIndirect with memory src
            AsmInstr::StoreIndirect(t, ref src, ref reg) if is_memory(src) => {
                if matches!(t, AsmType::Float | AsmType::Double) {
                    new_instructions.push(AsmInstr::Mov(
                        t,
                        src.clone(),
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::StoreIndirect(
                        t,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        *reg,
                    ));
                } else {
                    new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                    new_instructions.push(AsmInstr::StoreIndirect(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        *reg,
                    ));
                }
            }
            // Ret → pop callee-saved registers (reverse order), then epilogue
            instr @ (AsmInstr::Ret | AsmInstr::Unreachable | AsmInstr::NonlocalJmp(_)) => {
                for reg in callee_saved.iter().rev() {
                    new_instructions.push(AsmInstr::Pop(*reg));
                }
                new_instructions.push(instr);
            }
            other => {
                new_instructions.push(other);
            }
        }
    }

    new_instructions.retain(|instr| match instr {
        AsmInstr::Mov(AsmType::Longword, AsmOperand::Reg(src), AsmOperand::Reg(dst))
            if src == dst =>
        {
            true
        }
        AsmInstr::Mov(_, src, dst) => src != dst,
        _ => true,
    });
    func.instructions = new_instructions;
}

fn assert_no_pseudo_operand(op: &AsmOperand, instr: &AsmInstr) -> Result<(), String> {
    if matches!(op, AsmOperand::Pseudo(_) | AsmOperand::PseudoMem(_, _)) {
        return Err(format!(
            "unlowered pseudo operand in final assembly: {:?} in {:?}",
            op, instr
        ));
    }
    if let Some(offset) = stack_offset_exceeds_disp32(op) {
        return Err(format!(
            "unencodable x86-64 stack displacement {} after fixup: {:?}",
            offset, instr
        ));
    }
    Ok(())
}

fn verify_final_function(func: &AsmFunction) -> Result<(), String> {
    for instr in &func.instructions {
        match instr {
            AsmInstr::Mov(_, src, dst)
            | AsmInstr::Binary(_, _, src, dst)
            | AsmInstr::Cmp(_, src, dst)
            | AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Lea(src, dst) => {
                assert_no_pseudo_operand(src, instr)?;
                assert_no_pseudo_operand(dst, instr)?;
            }
            AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
                assert_no_pseudo_operand(src, instr)?;
                assert_no_pseudo_operand(dst, instr)?;
                if is_memory(dst) {
                    return Err(format!(
                        "extension instruction has memory destination after fixup: {:?}",
                        instr
                    ));
                }
            }
            AsmInstr::Unary(_, _, op)
            | AsmInstr::MulFull(_, op)
            | AsmInstr::Idiv(_, op)
            | AsmInstr::Div(_, op)
            | AsmInstr::SetCC(_, op)
            | AsmInstr::Push(op) => {
                assert_no_pseudo_operand(op, instr)?;
            }
            AsmInstr::LoadIndirect(_, _, dst)
            | AsmInstr::StoreIndirect(_, dst, _)
            | AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => {
                assert_no_pseudo_operand(dst, instr)?;
            }
            AsmInstr::CopyToStackArg { src_ptr, .. } => {
                assert_no_pseudo_operand(src_ptr, instr)?;
            }
            AsmInstr::CopyFromStackArg { dst, .. } => {
                assert_no_pseudo_operand(dst, instr)?;
            }
            AsmInstr::X87Load(_, src)
            | AsmInstr::X87Store(src)
            | AsmInstr::X87StoreFloat(_, src) => {
                assert_no_pseudo_operand(src, instr)?;
            }
            AsmInstr::X87StoreInt(_, dst) => {
                assert_no_pseudo_operand(dst, instr)?;
            }
            AsmInstr::X87LoadIndirect(_, _) | AsmInstr::X87StoreIndirect(_) => {}
            _ => {}
        }

        match instr {
            AsmInstr::Mov(_, src, dst) if is_memory(src) && is_memory(dst) => {
                return Err(format!(
                    "memory-to-memory mov after fixup in {}: {:?}",
                    func.name, instr
                ));
            }
            AsmInstr::Binary(AsmType::Double, _, _, dst) if !matches!(dst, AsmOperand::Xmm(_)) => {
                return Err(format!(
                    "double binary destination is not an XMM register in {}: {:?}",
                    func.name, instr
                ));
            }
            AsmInstr::Binary(_, _, src, dst) if is_memory(src) && is_memory(dst) => {
                return Err(format!(
                    "memory-to-memory binary instruction after fixup in {}: {:?}",
                    func.name, instr
                ));
            }
            AsmInstr::Cmp(_, src, dst) if is_memory(src) && is_memory(dst) => {
                return Err(format!(
                    "memory-to-memory cmp after fixup in {}: {:?}",
                    func.name, instr
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

// ============================================================
// Public API
// ============================================================

fn compute_ret_regs(
    func: &TackyFunction,
    types: &IndexMap<String, CType>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &IndexMap<String, StructDef>,
) -> Vec<super::regalloc::RegId> {
    use super::regalloc::RegId;
    for instr in &func.body {
        match instr {
            TackyInstr::Return(TackyVal::DoubleConstant(_)) => {
                return vec![RegId::Xmm(XmmReg::XMM0)];
            }
            TackyInstr::Return(TackyVal::Constant(_)) => {
                return match func.return_type {
                    CType::Float | CType::Double | CType::LongDouble => {
                        vec![RegId::Xmm(XmmReg::XMM0)]
                    }
                    CType::Int128 | CType::UInt128 => vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DX)],
                    CType::Void => vec![],
                    _ => vec![RegId::Gp(Reg::AX)],
                };
            }
            TackyInstr::Return(TackyVal::Int128Constant(_))
            | TackyInstr::Return(TackyVal::UInt128Constant(_)) => {
                return vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DX)];
            }
            TackyInstr::Return(TackyVal::Var(name)) => {
                let ct = types.get(name).copied().unwrap_or(CType::Int);
                return match ct {
                    CType::Float | CType::Double => vec![RegId::Xmm(XmmReg::XMM0)],
                    CType::Int128 | CType::UInt128 => vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DX)],
                    CType::Void => vec![],
                    CType::Struct => {
                        let mut regs = Vec::with_capacity(2);
                        if let Some(tag) = var_struct_tags.get(name) {
                            if let Some(def) = struct_defs.get(tag) {
                                let classes = def.classify_with(struct_defs);
                                let int_rets = [Reg::AX, Reg::DX];
                                let sse_rets = [XmmReg::XMM0, XmmReg::XMM1];
                                let (mut ir, mut sr) = (0usize, 0usize);
                                for c in &classes {
                                    match c {
                                        ParamClass::Integer => {
                                            regs.push(RegId::Gp(int_rets[ir]));
                                            ir += 1;
                                        }
                                        ParamClass::Sse => {
                                            regs.push(RegId::Xmm(sse_rets[sr]));
                                            sr += 1;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        if regs.is_empty() {
                            vec![RegId::Gp(Reg::AX)]
                        } else {
                            regs
                        }
                    }
                    _ => vec![RegId::Gp(Reg::AX)],
                };
            }
            _ => {}
        }
    }
    vec![] // void function
}

pub fn gen(
    program: &TackyProgram,
    target: &Target,
    no_coalescing: bool,
) -> Result<AsmProgram, String> {
    let static_vars = &program.global_vars;
    let types = &program.symbol_types;
    let array_sizes = &program.array_sizes;
    let alignments = &program.symbol_alignments;
    let mut local_function_names: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(program.top_level.len());
    for tl in &program.top_level {
        if let TackyTopLevel::Function(tf) = tl {
            local_function_names.insert(tf.name.clone());
        }
    }
    let mut top_level = Vec::with_capacity(program.top_level.len());
    let mut static_doubles = Vec::with_capacity(program.top_level.len());
    let mut static_floats = Vec::with_capacity(program.top_level.len());
    let function_ctx = X86FunctionContext {
        target,
        types,
        var_struct_tags: &program.var_struct_tags,
        struct_defs: &program.struct_defs,
        local_function_names: &local_function_names,
    };

    for tl in &program.top_level {
        match tl {
            TackyTopLevel::Function(tf) => {
                let mut asm_func =
                    convert_function(tf, &function_ctx, &mut static_doubles, &mut static_floats)?;

                fuse_setcc_branches(&mut asm_func);

                // Compute aliased variables (address-taken + static)
                let aliased = crate::backend::common::compute_aliased(&tf.body, static_vars);

                // Compute return value registers for EXIT node liveness
                let ret_regs =
                    compute_ret_regs(tf, types, &program.var_struct_tags, &program.struct_defs);

                // Register allocation
                let allocation = super::regalloc::allocate_registers(
                    &mut asm_func,
                    &aliased,
                    types,
                    array_sizes,
                    &ret_regs,
                    no_coalescing,
                );

                // Phase 2: replace remaining pseudos with stack slots
                let replace_ctx = ReplacePseudoContext {
                    statics: static_vars,
                    tls_vars: &program.thread_local_vars,
                    types,
                    arr_sizes: array_sizes,
                    alignments,
                    var_struct_tags: &program.var_struct_tags,
                    struct_defs: &program.struct_defs,
                };
                let stack_size = replace_pseudos(&mut asm_func, &replace_ctx)?;

                // Phase 3: fix up instructions + callee-saved register handling
                fixup_instructions(&mut asm_func, stack_size, &allocation.callee_saved);
                verify_final_function(&asm_func)?;
                top_level.push(AsmTopLevel::Function(asm_func));
            }
            TackyTopLevel::StaticVar(sv) => {
                top_level.push(AsmTopLevel::StaticVar(AsmStaticVar {
                    name: sv.name.clone(),
                    global: sv.global,
                    thread_local: sv.thread_local,
                    alignment: sv.alignment,
                    init_values: sv.init_values.clone(),
                }));
            }
            TackyTopLevel::StaticConstant(sc) => {
                top_level.push(AsmTopLevel::StaticConstant(AsmStaticConstant {
                    name: sc.name.clone(),
                    alignment: sc.alignment,
                    init: sc.init.clone(),
                }));
            }
            TackyTopLevel::Alias { name, target } => {
                top_level.push(AsmTopLevel::Alias {
                    name: name.clone(),
                    target: target.clone(),
                });
            }
        }
    }

    // Emit double constants as static data
    for (label, value) in static_floats {
        top_level.push(AsmTopLevel::StaticVar(AsmStaticVar {
            name: label,
            global: false,
            thread_local: false,
            alignment: 4,
            init_values: vec![StaticInit::FloatInit(value)],
        }));
    }

    for (label, value) in static_doubles {
        top_level.push(AsmTopLevel::StaticVar(AsmStaticVar {
            name: label,
            global: false,
            thread_local: false,
            alignment: 16,
            init_values: vec![StaticInit::DoubleInit(value)],
        }));
    }

    Ok(AsmProgram { top_level })
}

#[cfg(test)]
mod tests {
    use super::super::regalloc::RegId;
    use super::*;
    use std::collections::HashSet;

    fn function_returning(return_type: CType, val: TackyVal) -> TackyFunction {
        TackyFunction {
            name: "f".to_string(),
            return_type,
            params: Vec::new(),
            global: false,
            body: vec![TackyInstr::Return(val)],
            stack_params: HashSet::new(),
            memory_param_blocks: Vec::new(),
            struct_param_groups: Vec::new(),
        }
    }

    #[test]
    fn x86_64_preallocation_fuses_setcc_boolean_branch() {
        let mut function = AsmFunction {
            name: "f".to_string(),
            global: false,
            instructions: vec![
                AsmInstr::Cmp(
                    AsmType::Longword,
                    AsmOperand::Imm(1),
                    AsmOperand::Pseudo("x".to_string()),
                ),
                AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Imm(0),
                    AsmOperand::Pseudo("boolean".to_string()),
                ),
                AsmInstr::SetCC(CondCode::LE, AsmOperand::Pseudo("boolean".to_string())),
                AsmInstr::Cmp(
                    AsmType::Longword,
                    AsmOperand::Imm(0),
                    AsmOperand::Pseudo("boolean".to_string()),
                ),
                AsmInstr::JmpCC(CondCode::E, "false".to_string()),
                AsmInstr::Label("false".to_string()),
                AsmInstr::Ret,
            ],
        };

        fuse_setcc_branches(&mut function);

        assert_eq!(function.instructions.len(), 4);
        assert!(matches!(
            function.instructions.as_slice(),
            [
                AsmInstr::Cmp(AsmType::Longword, AsmOperand::Imm(1), AsmOperand::Pseudo(_)),
                AsmInstr::JmpCC(CondCode::G, label),
                AsmInstr::Label(_),
                AsmInstr::Ret,
            ] if label == "false"
        ));
    }

    #[test]
    fn x86_64_ret_regs_include_both_halves_for_int128_constant_returns() {
        let types = IndexMap::new();
        let regs = compute_ret_regs(
            &function_returning(CType::Int128, TackyVal::Int128Constant(1)),
            &types,
            &HashMap::new(),
            &IndexMap::new(),
        );

        assert_eq!(regs, vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DX)]);
    }

    #[test]
    fn x86_64_ret_regs_include_both_halves_for_int128_variable_returns() {
        let mut types = IndexMap::new();
        types.insert("v".to_string(), CType::UInt128);
        let regs = compute_ret_regs(
            &function_returning(CType::UInt128, TackyVal::Var("v".to_string())),
            &types,
            &HashMap::new(),
            &IndexMap::new(),
        );

        assert_eq!(regs, vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DX)]);
    }

    #[test]
    fn x86_64_i128_part_moves_skip_exact_self_moves() {
        let mut out = Vec::new();
        emit_i128_parts_to_operands(
            &mut out,
            AsmOperand::Reg(Reg::AX),
            AsmOperand::Reg(Reg::DX),
            AsmOperand::Reg(Reg::AX),
            AsmOperand::Reg(Reg::DX),
        );

        assert!(out.is_empty());
    }

    #[test]
    fn x86_64_variable_i128_shift_skips_redundant_dst_precopy() -> Result<(), String> {
        let mut types = IndexMap::new();
        types.insert("x".to_string(), CType::UInt128);
        types.insert("n".to_string(), CType::Int);
        types.insert("dst".to_string(), CType::UInt128);
        let mut out = Vec::new();
        let mut static_doubles = Vec::new();
        let mut static_floats = Vec::new();
        let mut label_counter = 0;
        let mut ctx = BinaryContext {
            types: &types,
            out: &mut out,
            static_doubles: &mut static_doubles,
            static_floats: &mut static_floats,
            label_counter: &mut label_counter,
            function_name: "f",
        };

        convert_binary(
            &TackyBinaryOp::ShiftLeft,
            &TackyVal::Var("x".to_string()),
            &TackyVal::Var("n".to_string()),
            &TackyVal::Var("dst".to_string()),
            &mut ctx,
        )?;

        let loop_start = out
            .iter()
            .position(|instr| matches!(instr, AsmInstr::Label(label) if label.starts_with("i128_shift_loop.")))
            .expect("missing variable shift loop");
        let writes_dst_before_loop = out[..loop_start].iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(
                    AsmType::Quadword,
                    _,
                    AsmOperand::PseudoMem(name, 0 | 8)
                ) if name == "dst"
            )
        });

        assert!(!writes_dst_before_loop, "{out:#?}");
        Ok(())
    }

    #[test]
    fn x86_64_out_of_range_constant_i128_shift_uses_variable_fallback() -> Result<(), String> {
        let mut types = IndexMap::new();
        types.insert("x".to_string(), CType::UInt128);
        types.insert("dst".to_string(), CType::UInt128);
        let mut out = Vec::new();
        let mut static_doubles = Vec::new();
        let mut static_floats = Vec::new();
        let mut label_counter = 0;
        let mut ctx = BinaryContext {
            types: &types,
            out: &mut out,
            static_doubles: &mut static_doubles,
            static_floats: &mut static_floats,
            label_counter: &mut label_counter,
            function_name: "f",
        };

        convert_binary(
            &TackyBinaryOp::ShiftLeft,
            &TackyVal::Var("x".to_string()),
            &TackyVal::Constant(128),
            &TackyVal::Var("dst".to_string()),
            &mut ctx,
        )?;

        assert!(
            out.iter()
                .any(|instr| matches!(instr, AsmInstr::Label(label) if label.starts_with("i128_shift_loop."))),
            "{out:#?}"
        );
        Ok(())
    }

    #[test]
    fn x86_64_fixup_removes_non_widening_self_moves() {
        let mut func = AsmFunction {
            name: "f".to_string(),
            global: false,
            instructions: vec![
                AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::AX),
                    AsmOperand::Reg(Reg::AX),
                ),
                AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::AX),
                    AsmOperand::Reg(Reg::AX),
                ),
            ],
        };

        fixup_instructions(&mut func, 0, &[]);

        assert!(!func.instructions.iter().any(|instr| matches!(
            instr,
            AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Reg(Reg::AX)
            )
        )));
        assert!(func.instructions.iter().any(|instr| matches!(
            instr,
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Reg(Reg::AX)
            )
        )));
    }

    #[test]
    fn x86_64_fixup_keeps_longword_register_self_moves_for_zero_extension() {
        let mut func = AsmFunction {
            name: "f".to_string(),
            global: false,
            instructions: vec![AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Reg(Reg::AX),
            )],
        };

        fixup_instructions(&mut func, 0, &[]);

        assert!(func.instructions.iter().any(|instr| matches!(
            instr,
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Reg(Reg::AX)
            )
        )));
    }

    #[test]
    fn x86_64_fixup_removes_longword_memory_self_moves() {
        let mut func = AsmFunction {
            name: "f".to_string(),
            global: false,
            instructions: vec![AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Stack(-4),
                AsmOperand::Stack(-4),
            )],
        };

        fixup_instructions(&mut func, 0, &[]);

        assert!(!func.instructions.iter().any(|instr| matches!(
            instr,
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Stack(-4),
                AsmOperand::Stack(-4)
            )
        )));
    }
}
