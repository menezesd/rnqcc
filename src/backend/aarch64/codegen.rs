use crate::types::*;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;

const ARG_REGS: [Reg; 8] = [
    Reg::AX,
    Reg::DI,
    Reg::SI,
    Reg::DX,
    Reg::CX,
    Reg::R8,
    Reg::R9,
    Reg::R12,
];
const FP_ARG_REGS: [XmmReg; 8] = [
    XmmReg::XMM0,
    XmmReg::XMM1,
    XmmReg::XMM2,
    XmmReg::XMM3,
    XmmReg::XMM4,
    XmmReg::XMM5,
    XmmReg::XMM6,
    XmmReg::XMM7,
];
const RETURN_GP_REGS: [Reg; 2] = [Reg::AX, Reg::DI];
const RETURN_FP_REGS: [XmmReg; 2] = [XmmReg::XMM0, XmmReg::XMM1];
const STACK_ALIGNMENT: i32 = 16;
const STACK_SLOT_SIZE: i32 = 8;
const MIN_STACK_SLOT_SIZE: i32 = 4;
const LINK_REGISTER_SAVE_SIZE: i32 = 16;
const LARGE_LOCAL_ALIGNMENT: usize = 16;

#[derive(Debug, Clone)]
struct LargeLocal {
    name: String,
    size: usize,
    base_slot: i32,
}

#[derive(Debug)]
struct StackLayout {
    slots: HashMap<String, i32>,
    local_stack_size: i32,
    large_locals: Vec<LargeLocal>,
}

trait StackSlotLookup {
    fn stack_slot(&self, name: &str) -> Option<i32>;

    fn is_register_candidate(&self, _name: &str) -> bool {
        false
    }
}

impl StackSlotLookup for HashMap<String, i32> {
    fn stack_slot(&self, name: &str) -> Option<i32> {
        self.get(name).copied()
    }
}

struct RegisterStackSlots<'a> {
    slots: &'a HashMap<String, i32>,
    register_vars: &'a HashSet<String>,
}

impl StackSlotLookup for RegisterStackSlots<'_> {
    fn stack_slot(&self, name: &str) -> Option<i32> {
        self.slots.get(name).copied()
    }

    fn is_register_candidate(&self, name: &str) -> bool {
        self.register_vars.contains(name)
    }
}

impl Deref for RegisterStackSlots<'_> {
    type Target = HashMap<String, i32>;

    fn deref(&self) -> &Self::Target {
        self.slots
    }
}

fn align_to(value: i32, alignment: i32) -> i32 {
    if value == 0 {
        0
    } else {
        (value + alignment - 1) & !(alignment - 1)
    }
}

fn compute_frame_size(local_stack_size: i32, saves_link_register: bool) -> i32 {
    if saves_link_register {
        local_stack_size + LINK_REGISTER_SAVE_SIZE
    } else {
        local_stack_size
    }
}

fn compute_link_register_offset(frame_size: i32) -> i32 {
    frame_size - STACK_SLOT_SIZE
}

fn align_usize_to(value: usize, alignment: usize) -> Result<usize, String> {
    if alignment == 0 {
        return Ok(value);
    }
    value
        .checked_add(alignment - 1)
        .map(|rounded| rounded & !(alignment - 1))
        .ok_or_else(|| "AArch64 backend large local alignment overflow".to_string())
}

fn align_i64_to(value: i64, alignment: i64) -> Result<i64, String> {
    if alignment <= 0 {
        return Ok(value);
    }
    value
        .checked_add(alignment - 1)
        .map(|rounded| rounded & !(alignment - 1))
        .ok_or_else(|| "AArch64 backend large stack alignment overflow".to_string())
}

fn stack_arg_offset(base: i32, index: usize) -> i32 {
    base + (index as i32 * STACK_SLOT_SIZE)
}

fn outgoing_stack_size(stack_arg_count: usize) -> i32 {
    align_to(stack_arg_count as i32 * STACK_SLOT_SIZE, STACK_ALIGNMENT)
}

fn emit_epilogue(
    instructions: &mut Vec<AsmInstr>,
    frame_size: i32,
    large_stack_size: i64,
    link_register_offset: Option<i32>,
) {
    if let Some(offset) = link_register_offset {
        instructions.push(AsmInstr::AArch64RestoreLink(offset));
    }
    if frame_size > 0 {
        instructions.push(AsmInstr::DeallocateStack(i64::from(frame_size)));
    }
    if large_stack_size > 0 {
        instructions.push(AsmInstr::AArch64DeallocateLargeStack(large_stack_size));
    }
    instructions.push(AsmInstr::Ret);
}

fn low64_operand(op: &AsmOperand) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Stack(offset) => Ok(AsmOperand::Stack(*offset)),
        AsmOperand::Data(name) => Ok(AsmOperand::Data(name.clone())),
        AsmOperand::Reg(reg) => Ok(AsmOperand::Reg(*reg)),
        AsmOperand::Imm(value) => Ok(AsmOperand::Imm(*value)),
        other => Err(format!(
            "AArch64 backend cannot address low half of {:?}",
            other
        )),
    }
}

fn high64_operand(op: &AsmOperand) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Stack(offset) => Ok(AsmOperand::Stack(*offset + 8)),
        AsmOperand::Data(name) => Ok(data_operand_with_offset(name, 8)),
        AsmOperand::Reg(Reg::AX) => Ok(AsmOperand::Reg(Reg::DI)),
        AsmOperand::Reg(reg) => Err(format!(
            "AArch64 backend cannot address high half of 128-bit register {:?}",
            reg
        )),
        AsmOperand::Imm(_) => {
            Err("AArch64 backend cannot address high half of immediate".to_string())
        }
        other => Err(format!(
            "AArch64 backend cannot address high half of {:?}",
            other
        )),
    }
}

fn byte_offset_operand(op: &AsmOperand, offset: i32) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Stack(base) => Ok(AsmOperand::Stack(*base + i64::from(offset))),
        AsmOperand::Data(name) => Ok(data_operand_with_offset(name, offset)),
        other => Err(format!(
            "AArch64 backend cannot address byte offset {} of {:?}",
            offset, other
        )),
    }
}

fn i128_part_operands(
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(AsmOperand, AsmOperand), String> {
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
            let op = val_operand(val, stack_slots, global_vars)?;
            Ok((low64_operand(&op)?, high64_operand(&op)?))
        }
    }
}

fn emit_i128_copy_to_operand(
    instructions: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst: AsmOperand,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let dst_low = low64_operand(&dst)?;
    let dst_high = high64_operand(&dst)?;
    match src {
        TackyVal::Int128Constant(value) => {
            let (low, high) = crate::backend::common::i128_parts_signed(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                dst_low,
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                dst_high,
            ));
        }
        TackyVal::UInt128Constant(value) => {
            let (low, high) = crate::backend::common::i128_parts_unsigned(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                dst_low,
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                dst_high,
            ));
        }
        _ => {
            let (src_low, src_high) = i128_part_operands(src, stack_slots, global_vars)?;
            instructions.push(AsmInstr::Mov(AsmType::Quadword, src_low, dst_low));
            instructions.push(AsmInstr::Mov(AsmType::Quadword, src_high, dst_high));
        }
    }
    Ok(())
}

fn emit_i128_copy(
    instructions: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let dst_op = val_operand(dst, stack_slots, global_vars)?;
    emit_i128_copy_to_operand(instructions, src, dst_op, stack_slots, global_vars)
}

fn emit_i128_zero_cmp(
    instructions: &mut Vec<AsmInstr>,
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let (low, high) = i128_part_operands(val, stack_slots, global_vars)?;
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        low,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Or,
        high,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::R10),
    ));
    Ok(())
}

fn emit_i128_unary(
    instructions: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst: AsmOperand,
    op: AsmUnaryOp,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let (src_low, src_high) = i128_part_operands(src, stack_slots, global_vars)?;
    let dst_low = low64_operand(&dst)?;
    let dst_high = high64_operand(&dst)?;
    instructions.push(AsmInstr::Mov(AsmType::Quadword, src_low, dst_low.clone()));
    instructions.push(AsmInstr::Mov(AsmType::Quadword, src_high, dst_high.clone()));
    match op {
        AsmUnaryOp::Not => {
            instructions.push(AsmInstr::Unary(AsmType::Quadword, AsmUnaryOp::Not, dst_low));
            instructions.push(AsmInstr::Unary(
                AsmType::Quadword,
                AsmUnaryOp::Not,
                dst_high,
            ));
        }
        AsmUnaryOp::Neg => {
            instructions.push(AsmInstr::Unary(
                AsmType::Quadword,
                AsmUnaryOp::Not,
                dst_low.clone(),
            ));
            instructions.push(AsmInstr::Unary(
                AsmType::Quadword,
                AsmUnaryOp::Not,
                dst_high.clone(),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::AddSetFlags,
                AsmOperand::Imm(1),
                dst_low,
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Adc,
                AsmOperand::Imm(0),
                dst_high,
            ));
        }
    }
    Ok(())
}

fn emit_i128_basic_binary(
    instructions: &mut Vec<AsmInstr>,
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: AsmOperand,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<bool, String> {
    if !matches!(
        op,
        TackyBinaryOp::Add
            | TackyBinaryOp::Sub
            | TackyBinaryOp::Mul
            | TackyBinaryOp::BitwiseAnd
            | TackyBinaryOp::BitwiseOr
            | TackyBinaryOp::BitwiseXor
    ) {
        return Ok(false);
    }

    if matches!(op, TackyBinaryOp::Mul) {
        if i128_constant_is_zero(left) || i128_constant_is_zero(right) {
            let dst_low = low64_operand(&dst)?;
            let dst_high = high64_operand(&dst)?;
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                dst_low,
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                dst_high,
            ));
            return Ok(true);
        }
        if i128_constant_is_one(left) {
            emit_i128_copy_to_operand(instructions, right, dst, stack_slots, global_vars)?;
            return Ok(true);
        }
        if i128_constant_is_one(right) {
            emit_i128_copy_to_operand(instructions, left, dst, stack_slots, global_vars)?;
            return Ok(true);
        }
        if i128_constant_is_negative_one(left) {
            emit_i128_unary(
                instructions,
                right,
                dst,
                AsmUnaryOp::Neg,
                stack_slots,
                global_vars,
            )?;
            return Ok(true);
        }
        if i128_constant_is_negative_one(right) {
            emit_i128_unary(
                instructions,
                left,
                dst,
                AsmUnaryOp::Neg,
                stack_slots,
                global_vars,
            )?;
            return Ok(true);
        }
    }

    emit_i128_copy_to_operand(instructions, left, dst.clone(), stack_slots, global_vars)?;
    let dst_low = low64_operand(&dst)?;
    let dst_high = high64_operand(&dst)?;
    let (right_low, right_high) = i128_part_operands(right, stack_slots, global_vars)?;
    match op {
        TackyBinaryOp::BitwiseAnd | TackyBinaryOp::BitwiseOr | TackyBinaryOp::BitwiseXor => {
            let asm_op = match op {
                TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                _ => return Err("internal error: expected bitwise binary op".to_string()),
            };
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                asm_op.clone(),
                right_low,
                dst_low,
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                asm_op,
                right_high,
                dst_high,
            ));
        }
        TackyBinaryOp::Add | TackyBinaryOp::Sub => {
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                if matches!(op, TackyBinaryOp::Add) {
                    AsmBinaryOp::AddSetFlags
                } else {
                    AsmBinaryOp::SubSetFlags
                },
                right_low,
                dst_low,
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                if matches!(op, TackyBinaryOp::Add) {
                    AsmBinaryOp::Adc
                } else {
                    AsmBinaryOp::Sbb
                },
                right_high,
                dst_high,
            ));
        }
        TackyBinaryOp::Mul => {
            // Keep every input limb live in a scratch register before writing
            // either result limb: `left *= left` and other aliased operands
            // must observe the original 128-bit values.
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                dst_low.clone(),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                dst_high.clone(),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                right_low,
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                right_high,
                AsmOperand::Reg(Reg::R15),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R11),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Mul,
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R11),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R11),
                dst_low,
            ));
            instructions.push(AsmInstr::AArch64Umulh(
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R11),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Mul,
                AsmOperand::Reg(Reg::R15),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Mul,
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Add,
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Add,
                AsmOperand::Reg(Reg::R11),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R13),
                dst_high,
            ));
        }
        _ => return Err("internal error: expected basic 128-bit binary op".to_string()),
    }
    Ok(true)
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

fn i128_constant_is_negative_one(value: &TackyVal) -> bool {
    matches!(
        value,
        TackyVal::Constant(-1)
            | TackyVal::Int128Constant(-1)
            | TackyVal::UInt128Constant(u128::MAX)
    )
}

fn emit_i128_return_regs_to_operand(
    instructions: &mut Vec<AsmInstr>,
    dst: AsmOperand,
) -> Result<(), String> {
    let dst_low = low64_operand(&dst)?;
    let dst_high = high64_operand(&dst)?;
    if dst_low != AsmOperand::Reg(Reg::AX) {
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Reg(Reg::AX),
            dst_low,
        ));
    }
    if dst_high != AsmOperand::Reg(Reg::DI) {
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Reg(Reg::DI),
            dst_high,
        ));
    }
    Ok(())
}

fn emit_i128_helper_binary(
    instructions: &mut Vec<AsmInstr>,
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: AsmOperand,
    ctx: &Aarch64I128Context<'_>,
) -> Result<bool, String> {
    let is_unsigned = is_unsigned_val(left, ctx.types);
    if matches!(op, TackyBinaryOp::Div) && i128_constant_is_one(right) {
        emit_i128_copy_to_operand(instructions, left, dst, ctx.stack_slots, ctx.global_vars)?;
        return Ok(true);
    }
    if matches!(op, TackyBinaryOp::Div) && !is_unsigned && i128_constant_is_negative_one(right) {
        emit_i128_unary(
            instructions,
            left,
            dst,
            AsmUnaryOp::Neg,
            ctx.stack_slots,
            ctx.global_vars,
        )?;
        return Ok(true);
    }
    if matches!(op, TackyBinaryOp::Mod)
        && (i128_constant_is_one(right) || (!is_unsigned && i128_constant_is_negative_one(right)))
    {
        let dst_low = low64_operand(&dst)?;
        let dst_high = high64_operand(&dst)?;
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Imm(0),
            dst_low,
        ));
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Imm(0),
            dst_high,
        ));
        return Ok(true);
    }
    let helper = match (op, is_unsigned_val(left, ctx.types)) {
        (TackyBinaryOp::Mul, _) => "__multi3",
        (TackyBinaryOp::Div, true) => "__udivti3",
        (TackyBinaryOp::Div, false) => "__divti3",
        (TackyBinaryOp::Mod, true) => "__umodti3",
        (TackyBinaryOp::Mod, false) => "__modti3",
        _ => return Ok(false),
    };
    let (left_low, left_high) = i128_part_operands(left, ctx.stack_slots, ctx.global_vars)?;
    let (right_low, right_high) = i128_part_operands(right, ctx.stack_slots, ctx.global_vars)?;
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_low,
        AsmOperand::Reg(Reg::AX),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_high,
        AsmOperand::Reg(Reg::DI),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        right_low,
        AsmOperand::Reg(Reg::SI),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        right_high,
        AsmOperand::Reg(Reg::DX),
    ));
    instructions.push(AsmInstr::Call(helper.to_string(), 4, 0, false, false));
    emit_i128_return_regs_to_operand(instructions, dst)?;
    Ok(true)
}

fn i128_div_or_mod_requires_helper(
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    types: &IndexMap<String, CType>,
) -> bool {
    match op {
        TackyBinaryOp::Div => {
            !i128_constant_is_one(right)
                && (is_unsigned_val(left, types) || !i128_constant_is_negative_one(right))
        }
        TackyBinaryOp::Mod => {
            !i128_constant_is_one(right)
                && (is_unsigned_val(left, types) || !i128_constant_is_negative_one(right))
        }
        _ => false,
    }
}

struct Aarch64I128Context<'a> {
    function_name: &'a str,
    types: &'a IndexMap<String, CType>,
    stack_slots: &'a HashMap<String, i32>,
    global_vars: &'a HashSet<String>,
}

fn emit_i128_signed_cmp(
    instructions: &mut Vec<AsmInstr>,
    left: &TackyVal,
    right: &TackyVal,
    op: &TackyBinaryOp,
    dst: AsmOperand,
    ctx: &Aarch64I128Context<'_>,
) -> Result<(), String> {
    let (left_low, left_high) = i128_part_operands(left, ctx.stack_slots, ctx.global_vars)?;
    let (right_low, right_high) = i128_part_operands(right, ctx.stack_slots, ctx.global_vars)?;
    let id = instructions.len();
    let true_label = format!("i128_cmp_true.{}.{}", ctx.function_name, id);
    let end_label = format!("i128_cmp_end.{}.{}", ctx.function_name, id);
    let (high_true, high_false, low_true) = match op {
        TackyBinaryOp::GreaterThan => (CondCode::G, CondCode::L, CondCode::A),
        TackyBinaryOp::GreaterEqual => (CondCode::G, CondCode::L, CondCode::AE),
        TackyBinaryOp::LessThan => (CondCode::L, CondCode::G, CondCode::B),
        TackyBinaryOp::LessEqual => (CondCode::L, CondCode::G, CondCode::BE),
        _ => return Err(format!("unsupported 128-bit signed comparison: {:?}", op)),
    };

    instructions.push(AsmInstr::Mov(
        AsmType::Longword,
        AsmOperand::Imm(0),
        dst.clone(),
    ));
    instructions.push(AsmInstr::Cmp(AsmType::Quadword, right_high, left_high));
    instructions.push(AsmInstr::JmpCC(high_true, true_label.clone()));
    instructions.push(AsmInstr::JmpCC(high_false, end_label.clone()));
    instructions.push(AsmInstr::Cmp(AsmType::Quadword, right_low, left_low));
    instructions.push(AsmInstr::JmpCC(low_true, true_label.clone()));
    instructions.push(AsmInstr::Jmp(end_label.clone()));
    instructions.push(AsmInstr::Label(true_label));
    instructions.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(1), dst));
    instructions.push(AsmInstr::Label(end_label));
    Ok(())
}

fn emit_i128_unsigned_cmp(
    instructions: &mut Vec<AsmInstr>,
    left: &TackyVal,
    right: &TackyVal,
    op: &TackyBinaryOp,
    dst: AsmOperand,
    ctx: &Aarch64I128Context<'_>,
) -> Result<(), String> {
    let (left_low, left_high) = i128_part_operands(left, ctx.stack_slots, ctx.global_vars)?;
    let (right_low, right_high) = i128_part_operands(right, ctx.stack_slots, ctx.global_vars)?;
    let id = instructions.len();
    let true_label = format!("u128_cmp_true.{}.{}", ctx.function_name, id);
    let end_label = format!("u128_cmp_end.{}.{}", ctx.function_name, id);
    let (high_true, high_false, low_true) = match op {
        TackyBinaryOp::GreaterThan => (CondCode::A, CondCode::B, CondCode::A),
        TackyBinaryOp::GreaterEqual => (CondCode::A, CondCode::B, CondCode::AE),
        TackyBinaryOp::LessThan => (CondCode::B, CondCode::A, CondCode::B),
        TackyBinaryOp::LessEqual => (CondCode::B, CondCode::A, CondCode::BE),
        _ => return Err(format!("unsupported 128-bit unsigned comparison: {:?}", op)),
    };

    instructions.push(AsmInstr::Mov(
        AsmType::Longword,
        AsmOperand::Imm(0),
        dst.clone(),
    ));
    instructions.push(AsmInstr::Cmp(AsmType::Quadword, right_high, left_high));
    instructions.push(AsmInstr::JmpCC(high_true, true_label.clone()));
    instructions.push(AsmInstr::JmpCC(high_false, end_label.clone()));
    instructions.push(AsmInstr::Cmp(AsmType::Quadword, right_low, left_low));
    instructions.push(AsmInstr::JmpCC(low_true, true_label.clone()));
    instructions.push(AsmInstr::Jmp(end_label.clone()));
    instructions.push(AsmInstr::Label(true_label));
    instructions.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(1), dst));
    instructions.push(AsmInstr::Label(end_label));
    Ok(())
}

fn emit_i128_eq_cmp(
    instructions: &mut Vec<AsmInstr>,
    left: &TackyVal,
    right: &TackyVal,
    op: &TackyBinaryOp,
    dst: AsmOperand,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let (left_low, left_high) = i128_part_operands(left, stack_slots, global_vars)?;
    let (right_low, right_high) = i128_part_operands(right, stack_slots, global_vars)?;
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_low,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Xor,
        right_low,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_high,
        AsmOperand::Reg(Reg::R13),
    ));
    instructions.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Xor,
        right_high,
        AsmOperand::Reg(Reg::R13),
    ));
    instructions.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Or,
        AsmOperand::Reg(Reg::R13),
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::SetCC(
        if matches!(op, TackyBinaryOp::Equal) {
            CondCode::E
        } else {
            CondCode::NE
        },
        dst,
    ));
    Ok(())
}

fn emit_i128_return(
    instructions: &mut Vec<AsmInstr>,
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    match val {
        TackyVal::Constant(value) => {
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(*value),
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(if *value < 0 { -1 } else { 0 }),
                AsmOperand::Reg(Reg::DI),
            ));
        }
        TackyVal::Int128Constant(value) => {
            let (low, high) = crate::backend::common::i128_parts_signed(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                AsmOperand::Reg(Reg::DI),
            ));
        }
        TackyVal::UInt128Constant(value) => {
            let (low, high) = crate::backend::common::i128_parts_unsigned(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                AsmOperand::Reg(Reg::DI),
            ));
        }
        _ => {
            let src = val_operand(val, stack_slots, global_vars)?;
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                low64_operand(&src)?,
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                high64_operand(&src)?,
                AsmOperand::Reg(Reg::DI),
            ));
        }
    }
    Ok(())
}

fn emit_i128_variable_shift(
    instructions: &mut Vec<AsmInstr>,
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: AsmOperand,
    ctx: &Aarch64I128Context<'_>,
) -> Result<(), String> {
    let (left_low, left_high) = i128_part_operands(left, ctx.stack_slots, ctx.global_vars)?;
    let dst_low = low64_operand(&dst)?;
    let dst_high = high64_operand(&dst)?;
    let right_ty = asm_type_for_val(right, ctx.types)?;
    let amount_src = if right_ty == AsmType::Octword {
        low64_operand(&val_operand(right, ctx.stack_slots, ctx.global_vars)?)?
    } else {
        val_operand(right, ctx.stack_slots, ctx.global_vars)?
    };
    let id = instructions.len();
    let upper_label = format!("i128_shift_upper.{}.{}", ctx.function_name, id);
    let overflow_label = format!("i128_shift_overflow.{}.{}", ctx.function_name, id);
    let end_label = format!("i128_shift_end.{}.{}", ctx.function_name, id);

    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_low,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_high,
        AsmOperand::Reg(Reg::R13),
    ));
    let counter_ty = if right_ty == AsmType::Octword {
        // A wide shift count contributes only its low 64 bits. The counter is
        // scalar, so load just that limb as a quadword.
        AsmType::Quadword
    } else {
        // Preserve the normal scalar move width: writing w7 deliberately
        // zero-extends 32-bit counts before the quadword loop uses x7.
        right_ty
    };
    instructions.push(AsmInstr::Mov(
        counter_ty,
        amount_src,
        AsmOperand::Reg(Reg::R12),
    ));
    instructions.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::R12),
    ));
    instructions.push(AsmInstr::JmpCC(CondCode::E, end_label.clone()));
    instructions.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(64),
        AsmOperand::Reg(Reg::R12),
    ));
    instructions.push(AsmInstr::JmpCC(CondCode::AE, upper_label.clone()));

    match op {
        TackyBinaryOp::ShiftLeft => {
            // For 1 <= count < 64, the high limb receives its own shifted
            // bits plus the low limb's carry.  Negating the count makes the
            // register shift perform `64 - count` modulo 64.
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sub,
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R15),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R15),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Or,
                AsmOperand::Reg(Reg::R15),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        TackyBinaryOp::ShiftRight => {
            let high_shift = if is_unsigned_val(left, ctx.types) {
                AsmBinaryOp::Shr
            } else {
                AsmBinaryOp::Sar
            };
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sub,
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R13),
                AsmOperand::Reg(Reg::R15),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R15),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                high_shift,
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Or,
                AsmOperand::Reg(Reg::R15),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        _ => return Err("internal error: expected i128 shift op".to_string()),
    }
    instructions.push(AsmInstr::Jmp(end_label.clone()));

    instructions.push(AsmInstr::Label(upper_label));
    instructions.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(128),
        AsmOperand::Reg(Reg::R12),
    ));
    instructions.push(AsmInstr::JmpCC(CondCode::AE, overflow_label.clone()));
    instructions.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Sub,
        AsmOperand::Imm(64),
        AsmOperand::Reg(Reg::R12),
    ));
    match op {
        TackyBinaryOp::ShiftLeft => {
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        TackyBinaryOp::ShiftRight => {
            let high_shift = if is_unsigned_val(left, ctx.types) {
                AsmBinaryOp::Shr
            } else {
                AsmBinaryOp::Sar
            };
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R13),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                high_shift.clone(),
                AsmOperand::Reg(Reg::R12),
                AsmOperand::Reg(Reg::R10),
            ));
            if matches!(high_shift, AsmBinaryOp::Shr) {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    AsmOperand::Reg(Reg::R13),
                ));
            } else {
                instructions.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Sar,
                    AsmOperand::Imm(63),
                    AsmOperand::Reg(Reg::R13),
                ));
            }
        }
        _ => unreachable!(),
    }
    instructions.push(AsmInstr::Jmp(end_label.clone()));

    instructions.push(AsmInstr::Label(overflow_label));
    match op {
        TackyBinaryOp::ShiftLeft => {
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::R13),
            ));
        }
        TackyBinaryOp::ShiftRight if is_unsigned_val(left, ctx.types) => {
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::R13),
            ));
        }
        TackyBinaryOp::ShiftRight => {
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sar,
                AsmOperand::Imm(63),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R13),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        _ => unreachable!(),
    }
    instructions.push(AsmInstr::Label(end_label));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        AsmOperand::Reg(Reg::R10),
        dst_low,
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        AsmOperand::Reg(Reg::R13),
        dst_high,
    ));
    Ok(())
}

fn emit_i128_shift(
    instructions: &mut Vec<AsmInstr>,
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: AsmOperand,
    ctx: &Aarch64I128Context<'_>,
) -> Result<bool, String> {
    if !matches!(op, TackyBinaryOp::ShiftLeft | TackyBinaryOp::ShiftRight) {
        return Ok(false);
    }

    let Some(amount) = i128_constant_shift_amount(right) else {
        emit_i128_variable_shift(instructions, op, left, right, dst, ctx)?;
        return Ok(true);
    };
    if !(0..128).contains(&amount) {
        emit_i128_variable_shift(instructions, op, left, right, dst, ctx)?;
        return Ok(true);
    }

    emit_i128_copy_to_operand(
        instructions,
        left,
        dst.clone(),
        ctx.stack_slots,
        ctx.global_vars,
    )?;
    if amount == 0 {
        return Ok(true);
    }

    let dst_low = low64_operand(&dst)?;
    let dst_high = high64_operand(&dst)?;
    match op {
        TackyBinaryOp::ShiftLeft => {
            if amount == 64 {
                instructions.push(AsmInstr::Mov(AsmType::Quadword, dst_low.clone(), dst_high));
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    dst_low,
                ));
            } else if (65..128).contains(&amount) {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    dst_low.clone(),
                    dst_high.clone(),
                ));
                instructions.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Sal,
                    AsmOperand::Imm(amount - 64),
                    dst_high,
                ));
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    dst_low,
                ));
            } else {
                instructions.push(AsmInstr::AArch64Extr(
                    dst_high.clone(),
                    dst_low.clone(),
                    (64 - amount) as u8,
                    dst_high,
                ));
                instructions.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Sal,
                    AsmOperand::Imm(amount),
                    dst_low,
                ));
            }
        }
        TackyBinaryOp::ShiftRight => {
            let high_shift = if is_unsigned_val(left, ctx.types) {
                AsmBinaryOp::Shr
            } else {
                AsmBinaryOp::Sar
            };
            if amount == 64 {
                instructions.push(AsmInstr::Mov(AsmType::Quadword, dst_high.clone(), dst_low));
                if is_unsigned_val(left, ctx.types) {
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Imm(0),
                        dst_high,
                    ));
                } else {
                    instructions.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        AsmBinaryOp::Sar,
                        AsmOperand::Imm(63),
                        dst_high,
                    ));
                }
            } else if (65..128).contains(&amount) {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    dst_high.clone(),
                    dst_low.clone(),
                ));
                instructions.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    high_shift.clone(),
                    AsmOperand::Imm(amount - 64),
                    dst_low,
                ));
                if is_unsigned_val(left, ctx.types) {
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Imm(0),
                        dst_high,
                    ));
                } else {
                    instructions.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        AsmBinaryOp::Sar,
                        AsmOperand::Imm(63),
                        dst_high,
                    ));
                }
            } else {
                instructions.push(AsmInstr::AArch64Extr(
                    dst_high.clone(),
                    dst_low.clone(),
                    amount as u8,
                    dst_low,
                ));
                instructions.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    high_shift,
                    AsmOperand::Imm(amount),
                    dst_high,
                ));
            }
        }
        _ => return Err("internal error: expected i128 shift op".to_string()),
    }
    Ok(true)
}

fn i128_constant_shift_amount(val: &TackyVal) -> Option<i64> {
    match val {
        TackyVal::Constant(value) => Some(*value),
        TackyVal::Int128Constant(value) => i64::try_from(*value).ok(),
        TackyVal::UInt128Constant(value) => i64::try_from(*value).ok(),
        TackyVal::DoubleConstant(_) | TackyVal::Var(_) => None,
    }
}

fn asm_type_for_val(val: &TackyVal, types: &IndexMap<String, CType>) -> Result<AsmType, String> {
    match val {
        TackyVal::Constant(c) => {
            if *c > i32::MAX as i64 || *c < i32::MIN as i64 {
                Ok(AsmType::Quadword)
            } else {
                Ok(AsmType::Longword)
            }
        }
        TackyVal::Int128Constant(_) | TackyVal::UInt128Constant(_) => Ok(AsmType::Octword),
        TackyVal::DoubleConstant(_) => Ok(AsmType::Double),
        TackyVal::Var(name) => match types.get(name).copied().unwrap_or(CType::Int) {
            CType::Char | CType::SChar | CType::UChar | CType::Bool => Ok(AsmType::Byte),
            CType::Short | CType::UShort => Ok(AsmType::Word),
            CType::Int | CType::UInt => Ok(AsmType::Longword),
            CType::Long | CType::ULong | CType::Pointer => Ok(AsmType::Quadword),
            CType::Int128 | CType::UInt128 => Ok(AsmType::Octword),
            CType::Float => Ok(AsmType::Float),
            CType::Double => Ok(AsmType::Double),
            CType::LongDouble => Ok(AsmType::LongDouble),
            CType::Void => Ok(AsmType::Longword),
            CType::Struct => Ok(AsmType::Quadword),
        },
    }
}

fn val_operand<S: StackSlotLookup + ?Sized>(
    val: &TackyVal,
    stack_slots: &S,
    global_vars: &HashSet<String>,
) -> Result<AsmOperand, String> {
    match val {
        TackyVal::Constant(c) => Ok(AsmOperand::Imm(*c)),
        TackyVal::Int128Constant(c) => Ok(AsmOperand::Imm(*c as i64)),
        TackyVal::UInt128Constant(c) => Ok(AsmOperand::Imm(*c as i64)),
        TackyVal::DoubleConstant(d) => Ok(AsmOperand::Imm(d.to_bits() as i64)),
        TackyVal::Var(name) => {
            if global_vars.contains(name) {
                Ok(AsmOperand::Data(name.clone()))
            } else if stack_slots.is_register_candidate(name) {
                Ok(AsmOperand::Pseudo(name.clone()))
            } else {
                stack_slots
                    .stack_slot(name)
                    .map(|offset| AsmOperand::Stack(i64::from(offset)))
                    .ok_or_else(|| format!("AArch64 backend missing stack slot for {}", name))
            }
        }
    }
}

fn floating_return_operand<S: StackSlotLookup + ?Sized>(
    ty: AsmType,
    val: &TackyVal,
    stack_slots: &S,
    global_vars: &HashSet<String>,
) -> Result<AsmOperand, String> {
    match (ty, val) {
        (AsmType::Float, TackyVal::DoubleConstant(d)) => {
            Ok(AsmOperand::Imm((*d as f32).to_bits() as i64))
        }
        (AsmType::Float, TackyVal::Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f32).to_bits() as i64))
        }
        (AsmType::Float, TackyVal::Int128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f32).to_bits() as i64))
        }
        (AsmType::Float, TackyVal::UInt128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f32).to_bits() as i64))
        }
        (AsmType::Double, TackyVal::DoubleConstant(d)) => Ok(AsmOperand::Imm(d.to_bits() as i64)),
        (AsmType::Double, TackyVal::Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f64).to_bits() as i64))
        }
        (AsmType::Double, TackyVal::Int128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f64).to_bits() as i64))
        }
        (AsmType::Double, TackyVal::UInt128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f64).to_bits() as i64))
        }
        _ => val_operand(val, stack_slots, global_vars),
    }
}

fn intern_long_double_const(pool: &mut Vec<(String, f64)>, value: f64) -> String {
    let bits = value.to_bits();
    if let Some((name, _)) = pool.iter().find(|(_, existing)| existing.to_bits() == bits) {
        return name.clone();
    }

    let name = format!("__aarch64_long_double_const_{}", pool.len());
    pool.push((name.clone(), value));
    name
}

fn rewrite_long_double_immediates(instructions: &mut [AsmInstr], pool: &mut Vec<(String, f64)>) {
    for instr in instructions {
        match instr {
            AsmInstr::Mov(AsmType::LongDouble, src, _)
            | AsmInstr::Binary(AsmType::LongDouble, _, src, _)
            | AsmInstr::Cmp(AsmType::LongDouble, src, _) => {
                if let AsmOperand::Imm(bits) = src {
                    let value = f64::from_bits(*bits as u64);
                    *src = AsmOperand::Data(intern_long_double_const(pool, value));
                }
            }
            _ => {}
        }
    }
}

fn collect_name(
    name: &str,
    vars: &mut Vec<String>,
    seen_vars: &mut HashSet<String>,
    global_vars: &HashSet<String>,
) {
    if !global_vars.contains(name) && seen_vars.insert(name.to_string()) {
        vars.push(name.to_string());
    }
}

fn collect_var(
    val: &TackyVal,
    vars: &mut Vec<String>,
    seen_vars: &mut HashSet<String>,
    global_vars: &HashSet<String>,
) {
    if let TackyVal::Var(name) = val {
        collect_name(name, vars, seen_vars, global_vars);
    }
}

fn val_ctype(val: &TackyVal, types: &IndexMap<String, CType>) -> Option<CType> {
    match val {
        TackyVal::Var(name) => types.get(name).copied(),
        TackyVal::Constant(_) => Some(CType::Int),
        TackyVal::Int128Constant(_) => Some(CType::Int128),
        TackyVal::UInt128Constant(_) => Some(CType::UInt128),
        TackyVal::DoubleConstant(_) => Some(CType::Double),
    }
}

struct RegisterNeeds {
    gp: usize,
    fp: usize,
}

fn group_register_needs(is_sse: &[bool]) -> RegisterNeeds {
    let fp_needed = is_sse.iter().filter(|&&is_fp| is_fp).count();
    RegisterNeeds {
        gp: is_sse.len() - fp_needed,
        fp: fp_needed,
    }
}

fn data_operand_with_offset(name: &str, offset: i32) -> AsmOperand {
    if offset == 0 {
        AsmOperand::Data(name.to_string())
    } else {
        AsmOperand::Data(format!(
            "{}{}",
            name,
            assembly_offset_suffix(i64::from(offset))
        ))
    }
}

struct TlsNameOffset {
    base: String,
    offset: i32,
}

fn tls_name_offset(name: &str, tls_vars: &HashSet<String>) -> Option<TlsNameOffset> {
    if tls_vars.contains(name) {
        return Some(TlsNameOffset {
            base: name.to_string(),
            offset: 0,
        });
    }
    let (base, offset) = name.rsplit_once('+')?;
    if tls_vars.contains(base) {
        let offset = offset.parse().ok()?;
        Some(TlsNameOffset {
            base: base.to_string(),
            offset,
        })
    } else {
        None
    }
}

fn rewrite_tls_operand(op: &mut AsmOperand, tls_vars: &HashSet<String>) {
    if let AsmOperand::Data(name) = op {
        if let Some(tls_offset) = tls_name_offset(name, tls_vars) {
            *op = AsmOperand::TlsData(tls_offset.base, tls_offset.offset);
        }
    }
}

fn rewrite_tls_operands(func: &mut AsmFunction, tls_vars: &HashSet<String>) {
    for instr in &mut func.instructions {
        match instr {
            AsmInstr::Mov(_, src, dst)
            | AsmInstr::Movsx(_, _, src, dst)
            | AsmInstr::MovZeroExtend(_, _, src, dst)
            | AsmInstr::Binary(_, _, src, dst)
            | AsmInstr::Cmp(_, src, dst)
            | AsmInstr::Lea(src, dst)
            | AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Cvtsi2ss(_, src, dst)
            | AsmInstr::Cvttss2si(_, src, dst)
            | AsmInstr::Cvtss2sd(src, dst)
            | AsmInstr::Cvtsd2ss(src, dst)
            | AsmInstr::AArch64UIntToDouble(_, src, dst)
            | AsmInstr::AArch64DoubleToUInt(_, src, dst)
            | AsmInstr::AArch64UIntToFloat(_, src, dst)
            | AsmInstr::AArch64FloatToUInt(_, src, dst)
            | AsmInstr::AArch64FloatToDouble(src, dst)
            | AsmInstr::AArch64DoubleToFloat(src, dst) => {
                rewrite_tls_operand(src, tls_vars);
                rewrite_tls_operand(dst, tls_vars);
            }
            AsmInstr::AArch64AddPtr(ptr, index, _, dst) => {
                rewrite_tls_operand(ptr, tls_vars);
                rewrite_tls_operand(index, tls_vars);
                rewrite_tls_operand(dst, tls_vars);
            }
            AsmInstr::AArch64Extr(high, low, _, dst) => {
                rewrite_tls_operand(high, tls_vars);
                rewrite_tls_operand(low, tls_vars);
                rewrite_tls_operand(dst, tls_vars);
            }
            AsmInstr::AArch64Umulh(left, right, dst) => {
                rewrite_tls_operand(left, tls_vars);
                rewrite_tls_operand(right, tls_vars);
                rewrite_tls_operand(dst, tls_vars);
            }
            AsmInstr::AArch64LoadAdjusted(_, src, _, _)
            | AsmInstr::AArch64StoreOutgoingArg(_, src, _, _) => {
                rewrite_tls_operand(src, tls_vars);
            }
            AsmInstr::AArch64Rem(_, _, left, right, dst) => {
                rewrite_tls_operand(left, tls_vars);
                rewrite_tls_operand(right, tls_vars);
                rewrite_tls_operand(dst, tls_vars);
            }
            AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => rewrite_tls_operand(dst, tls_vars),
            AsmInstr::Unary(_, _, op)
            | AsmInstr::Idiv(_, op)
            | AsmInstr::Div(_, op)
            | AsmInstr::SetCC(_, op)
            | AsmInstr::Push(op)
            | AsmInstr::LoadIndirect(_, _, op)
            | AsmInstr::StoreIndirect(_, op, _) => rewrite_tls_operand(op, tls_vars),
            _ => {}
        }
    }
}

fn stack_or_data_operand(
    name: &str,
    offset: i32,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<AsmOperand, String> {
    if global_vars.contains(name) {
        Ok(data_operand_with_offset(name, offset))
    } else {
        stack_slots
            .get(name)
            .copied()
            .map(|base| AsmOperand::Stack(i64::from(base + offset)))
            .ok_or_else(|| format!("AArch64 backend missing stack slot for {}", name))
    }
}

fn emit_copy_pointer_to_outgoing_arg(
    instructions: &mut Vec<AsmInstr>,
    src_ptr: AsmOperand,
    size: usize,
    dst_start: i32,
    outgoing_bytes: i32,
) {
    instructions.push(AsmInstr::AArch64LoadAdjusted(
        AsmType::Quadword,
        src_ptr,
        Reg::R11,
        outgoing_bytes,
    ));
    instructions.push(AsmInstr::CopyToStackArg {
        src_ptr: AsmOperand::Reg(Reg::R11),
        dst_offset: dst_start,
        size,
    });
}

fn emit_copy_incoming_arg_to_aggregate(
    instructions: &mut Vec<AsmInstr>,
    src_start: i32,
    dst_name: &str,
    size: usize,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    instructions.push(AsmInstr::CopyFromStackArg {
        src_offset: src_start,
        dst: stack_or_data_operand(dst_name, 0, stack_slots, global_vars)?,
        size,
    });
    Ok(())
}

fn aggregate_size(
    name: &str,
    array_sizes: &IndexMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &IndexMap<String, StructDef>,
) -> Option<usize> {
    array_sizes.get(name).copied().or_else(|| {
        var_struct_tags
            .get(name)
            .and_then(|tag| struct_defs.get(tag))
            .map(|def| def.size)
    })
}

fn struct_classes_for_val(
    val: &TackyVal,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &IndexMap<String, StructDef>,
) -> Option<Vec<ParamClass>> {
    let TackyVal::Var(name) = val else {
        return None;
    };
    var_struct_tags
        .get(name)
        .and_then(|tag| struct_defs.get(tag))
        .map(|def| def.classify_with(struct_defs))
}

fn struct_size_for_val(
    val: &TackyVal,
    array_sizes: &IndexMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &IndexMap<String, StructDef>,
) -> Option<usize> {
    let TackyVal::Var(name) = val else {
        return None;
    };
    aggregate_size(name, array_sizes, var_struct_tags, struct_defs)
}

fn struct_classes_return_in_registers(classes: &[ParamClass]) -> bool {
    classes
        .iter()
        .all(|class| matches!(class, ParamClass::Integer | ParamClass::Sse))
}

fn copy_bytes(
    instructions: &mut Vec<AsmInstr>,
    src_name: &str,
    dst_name: &str,
    size: usize,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let mut offset = 0usize;
    for (ty, width) in [
        (AsmType::Quadword, 8),
        (AsmType::Longword, 4),
        (AsmType::Word, 2),
        (AsmType::Byte, 1),
    ] {
        while offset + width <= size {
            emit_aggregate_copy_chunk(
                instructions,
                src_name,
                dst_name,
                offset,
                ty,
                stack_slots,
                global_vars,
            )?;
            offset += width;
        }
    }
    Ok(())
}

fn emit_aggregate_copy_chunk(
    instructions: &mut Vec<AsmInstr>,
    src_name: &str,
    dst_name: &str,
    offset: usize,
    ty: AsmType,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let byte_offset = i32::try_from(offset)
        .map_err(|_| format!("AArch64 backend aggregate offset too large: {src_name}"))?;
    instructions.push(AsmInstr::Mov(
        ty,
        stack_or_data_operand(src_name, byte_offset, stack_slots, global_vars)?,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Mov(
        ty,
        AsmOperand::Reg(Reg::R10),
        stack_or_data_operand(dst_name, byte_offset, stack_slots, global_vars)?,
    ));
    Ok(())
}

fn move_struct_to_return_regs(
    instructions: &mut Vec<AsmInstr>,
    val: &TackyVal,
    classes: &[ParamClass],
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let TackyVal::Var(name) = val else {
        return Err("AArch64 backend can only return struct variables".to_string());
    };
    let mut gp_idx = 0usize;
    let mut fp_idx = 0usize;
    for (chunk_idx, class) in classes.iter().enumerate() {
        let offset = i32::try_from(chunk_idx * 8)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", name))?;
        match class {
            ParamClass::Integer if gp_idx < RETURN_GP_REGS.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                    AsmOperand::Reg(RETURN_GP_REGS[gp_idx]),
                ));
                gp_idx += 1;
            }
            ParamClass::Sse if fp_idx < RETURN_FP_REGS.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                    AsmOperand::Xmm(RETURN_FP_REGS[fp_idx]),
                ));
                fp_idx += 1;
            }
            ParamClass::Memory => {
                return Err(
                    "AArch64 backend does not return memory-class structs in registers yet"
                        .to_string(),
                );
            }
            other => {
                return Err(format!(
                    "AArch64 backend unsupported struct return class: {:?}",
                    other
                ));
            }
        }
    }
    Ok(())
}

fn move_return_regs_to_struct(
    instructions: &mut Vec<AsmInstr>,
    dst: &TackyVal,
    classes: &[ParamClass],
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let TackyVal::Var(name) = dst else {
        return Err("AArch64 backend can only store struct returns into variables".to_string());
    };
    let mut gp_idx = 0usize;
    let mut fp_idx = 0usize;
    for (chunk_idx, class) in classes.iter().enumerate() {
        let offset = i32::try_from(chunk_idx * 8)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", name))?;
        match class {
            ParamClass::Integer if gp_idx < RETURN_GP_REGS.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(RETURN_GP_REGS[gp_idx]),
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                ));
                gp_idx += 1;
            }
            ParamClass::Sse if fp_idx < RETURN_FP_REGS.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    AsmOperand::Xmm(RETURN_FP_REGS[fp_idx]),
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                ));
                fp_idx += 1;
            }
            ParamClass::Memory => {
                return Err(
                    "AArch64 backend does not receive memory-class structs in registers yet"
                        .to_string(),
                );
            }
            other => {
                return Err(format!(
                    "AArch64 backend unsupported struct return class: {:?}",
                    other
                ));
            }
        }
    }
    Ok(())
}

fn collect_stack_slots(
    function: &TackyFunction,
    types: &IndexMap<String, CType>,
    array_sizes: &IndexMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &IndexMap<String, StructDef>,
    global_vars: &HashSet<String>,
    alignments: &IndexMap<String, usize>,
) -> Result<StackLayout, String> {
    let mut vars = Vec::with_capacity(
        function.params.len() + function.memory_param_blocks.len() + function.body.len() * 2,
    );
    let mut seen_vars = HashSet::with_capacity(vars.capacity());
    for param in &function.params {
        collect_name(param, &mut vars, &mut seen_vars, global_vars);
    }
    for (_, name, _) in &function.memory_param_blocks {
        collect_name(name, &mut vars, &mut seen_vars, global_vars);
    }

    let mut body_iter = function.body.iter().peekable();
    while let Some(instr) = body_iter.next() {
        let next_instr = body_iter.peek().copied();
        if let TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } = instr
        {
            if fused_comparison_branch(op, left, right, dst, next_instr, types).is_some() {
                collect_var(left, &mut vars, &mut seen_vars, global_vars);
                collect_var(right, &mut vars, &mut seen_vars, global_vars);
                body_iter.next();
                continue;
            }
        }
        match instr {
            TackyInstr::Return(val) => collect_var(val, &mut vars, &mut seen_vars, global_vars),
            TackyInstr::Unary { src, dst, .. } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Binary {
                left, right, dst, ..
            } => {
                collect_var(left, &mut vars, &mut seen_vars, global_vars);
                collect_var(right, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Copy { src, dst }
            | TackyInstr::SignExtend { src, dst }
            | TackyInstr::ZeroExtend { src, dst }
            | TackyInstr::Truncate { src, dst }
            | TackyInstr::IntToDouble { src, dst }
            | TackyInstr::IntToFloat { src, dst }
            | TackyInstr::DoubleToInt { src, dst }
            | TackyInstr::FloatToInt { src, dst }
            | TackyInstr::UIntToDouble { src, dst }
            | TackyInstr::UIntToFloat { src, dst }
            | TackyInstr::DoubleToUInt { src, dst }
            | TackyInstr::FloatToUInt { src, dst }
            | TackyInstr::FloatToDouble { src, dst }
            | TackyInstr::DoubleToFloat { src, dst } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::JumpIfZero(val, _)
            | TackyInstr::JumpIfNotZero(val, _)
            | TackyInstr::JumpIndirect(val) => {
                collect_var(val, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::LoadLabelAddress(_, dst) => {
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::FrameAddress { dst } => {
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::BuiltinSetjmp { buf, dst, .. } => {
                collect_var(buf, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::BuiltinLongjmp { buf, value } => {
                collect_var(buf, &mut vars, &mut seen_vars, global_vars);
                collect_var(value, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::FunCall {
                name,
                args,
                dst,
                indirect,
                ..
            } => {
                if *indirect {
                    collect_name(name, &mut vars, &mut seen_vars, global_vars);
                }
                for arg in args {
                    collect_var(arg, &mut vars, &mut seen_vars, global_vars);
                }
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::VaStart { dst } => {
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::GetAddress { src, dst } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Load { src_ptr, dst } => {
                collect_var(src_ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Store { src, dst_ptr } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst_ptr, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicFetch { ptr, arg, dst, .. } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(arg, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicExchange { ptr, value, dst } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(value, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicCompareExchange {
                ptr,
                expected,
                desired,
                dst,
            } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(expected, &mut vars, &mut seen_vars, global_vars);
                collect_var(desired, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                dst,
                ..
            } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(expected, &mut vars, &mut seen_vars, global_vars);
                collect_var(desired, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AddPtr {
                ptr, index, dst, ..
            } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(index, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::CopyToOffset { src, dst_name, .. } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_name(dst_name, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::CopyFromOffset { src_name, dst, .. } => {
                collect_name(src_name, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::CopyStruct { src_name, dst_name } => {
                collect_name(src_name, &mut vars, &mut seen_vars, global_vars);
                collect_name(dst_name, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Jump(_)
            | TackyInstr::NonlocalJump(_)
            | TackyInstr::Label(_)
            | TackyInstr::Nop
            | TackyInstr::Unreachable
            | TackyInstr::AtomicFence => {}
        }
    }

    let mut slots = HashMap::with_capacity(vars.len());
    let mut large_locals = Vec::with_capacity(vars.len());
    let mut offset = 0i32;
    for var in vars {
        let size =
            if let Some(size) = aggregate_size(&var, array_sizes, var_struct_tags, struct_defs) {
                if size > i32::MAX as usize {
                    offset = align_to(offset, STACK_SLOT_SIZE);
                    let base_slot = offset;
                    slots.insert(var.clone(), base_slot);
                    offset += STACK_SLOT_SIZE;
                    large_locals.push(LargeLocal {
                        name: var,
                        size,
                        base_slot,
                    });
                    continue;
                } else {
                    size as i32
                }
            } else {
                match types.get(&var).copied().unwrap_or(CType::Int) {
                    CType::Char | CType::SChar | CType::UChar | CType::Bool => 1,
                    CType::Short | CType::UShort => 2,
                    CType::Int | CType::UInt => 4,
                    CType::Float => 4,
                    CType::Long | CType::ULong | CType::Pointer => 8,
                    CType::Int128 | CType::UInt128 | CType::LongDouble => 16,
                    CType::Void => 4,
                    CType::Double => 8,
                    CType::Struct => {
                        return Err(format!(
                            "AArch64 backend missing aggregate size for local {}",
                            var
                        ))
                    }
                }
            };
        let align = if let Some(&decl_align) = alignments.get(&var) {
            i32::try_from(decl_align)
                .map_err(|_| format!("AArch64 backend alignment too large for {}", var))?
        } else if size >= STACK_SLOT_SIZE {
            STACK_SLOT_SIZE
        } else {
            MIN_STACK_SLOT_SIZE
        };
        offset = align_to(offset, align);
        slots.insert(var, offset);
        offset += size.max(MIN_STACK_SLOT_SIZE);
    }

    Ok(StackLayout {
        slots,
        local_stack_size: align_to(offset, STACK_ALIGNMENT),
        large_locals,
    })
}

fn convert_binary_op(op: &TackyBinaryOp) -> Result<AsmBinaryOp, String> {
    match op {
        TackyBinaryOp::Add => Ok(AsmBinaryOp::Add),
        TackyBinaryOp::Sub => Ok(AsmBinaryOp::Sub),
        TackyBinaryOp::Mul => Ok(AsmBinaryOp::Mul),
        TackyBinaryOp::ShiftLeft => Ok(AsmBinaryOp::Sal),
        TackyBinaryOp::BitwiseAnd => Ok(AsmBinaryOp::And),
        TackyBinaryOp::BitwiseNand => Ok(AsmBinaryOp::Nand),
        TackyBinaryOp::BitwiseOr => Ok(AsmBinaryOp::Or),
        TackyBinaryOp::BitwiseXor => Ok(AsmBinaryOp::Xor),
        _ => Err(format!(
            "AArch64 backend does not support binary operator yet: {:?}",
            op
        )),
    }
}

fn long_double_helper(op: &TackyBinaryOp) -> Option<&'static str> {
    match op {
        TackyBinaryOp::Add => Some("__addtf3"),
        TackyBinaryOp::Sub => Some("__subtf3"),
        TackyBinaryOp::Mul => Some("__multf3"),
        TackyBinaryOp::Div => Some("__divtf3"),
        _ => None,
    }
}

struct LongDoubleComparison {
    helper: &'static str,
    condition: CondCode,
}

fn long_double_comparison_helper(op: &TackyBinaryOp) -> Option<LongDoubleComparison> {
    match op {
        TackyBinaryOp::Equal => Some(LongDoubleComparison {
            helper: "__eqtf2",
            condition: CondCode::E,
        }),
        TackyBinaryOp::NotEqual => Some(LongDoubleComparison {
            helper: "__netf2",
            condition: CondCode::NE,
        }),
        TackyBinaryOp::LessThan => Some(LongDoubleComparison {
            helper: "__lttf2",
            condition: CondCode::L,
        }),
        TackyBinaryOp::LessEqual => Some(LongDoubleComparison {
            helper: "__letf2",
            condition: CondCode::LE,
        }),
        TackyBinaryOp::GreaterThan => Some(LongDoubleComparison {
            helper: "__gttf2",
            condition: CondCode::G,
        }),
        TackyBinaryOp::GreaterEqual => Some(LongDoubleComparison {
            helper: "__getf2",
            condition: CondCode::GE,
        }),
        _ => None,
    }
}

fn emit_long_double_helper_call<S: StackSlotLookup + ?Sized>(
    instructions: &mut Vec<AsmInstr>,
    helper: &str,
    left: &TackyVal,
    right: &TackyVal,
    dst: AsmOperand,
    stack_slots: &S,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(left, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM0),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(right, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM1),
    ));
    instructions.push(AsmInstr::Call(helper.to_string(), 0, 2, false, false));
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        AsmOperand::Xmm(XmmReg::XMM0),
        dst,
    ));
    Ok(())
}

fn emit_long_double_comparison<S: StackSlotLookup + ?Sized>(
    instructions: &mut Vec<AsmInstr>,
    comparison: LongDoubleComparison,
    left: &TackyVal,
    right: &TackyVal,
    dst: AsmOperand,
    stack_slots: &S,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(left, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM0),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(right, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM1),
    ));
    instructions.push(AsmInstr::Call(
        comparison.helper.to_string(),
        0,
        2,
        false,
        false,
    ));
    instructions.push(AsmInstr::Cmp(
        AsmType::Longword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::AX),
    ));
    instructions.push(AsmInstr::SetCC(comparison.condition, dst));
    Ok(())
}

fn emit_long_double_negate(
    instructions: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let dst_op = val_operand(dst, stack_slots, global_vars)?;
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(src, stack_slots, global_vars)?,
        dst_op.clone(),
    ));
    let sign_byte = byte_offset_operand(&dst_op, 15)?;
    instructions.push(AsmInstr::Mov(
        AsmType::Byte,
        sign_byte.clone(),
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Binary(
        AsmType::Byte,
        AsmBinaryOp::Xor,
        AsmOperand::Imm(0x80),
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Byte,
        AsmOperand::Reg(Reg::R10),
        sign_byte,
    ));
    Ok(())
}

fn convert_comparison_op(op: &TackyBinaryOp, is_unsigned: bool) -> Option<CondCode> {
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

fn fused_comparison_branch(
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    next: Option<&TackyInstr>,
    types: &IndexMap<String, CType>,
) -> Option<(CondCode, String)> {
    let left_ty = asm_type_for_val(left, types).ok()?;
    let right_ty = asm_type_for_val(right, types).ok()?;
    if matches!(left_ty, AsmType::Octword | AsmType::LongDouble)
        || matches!(right_ty, AsmType::Octword | AsmType::LongDouble)
    {
        return None;
    }
    let cc = convert_comparison_op(op, is_unsigned_comparison_val(left, types))?;
    match next? {
        TackyInstr::JumpIfZero(value, label) if value == dst => {
            Some((invert_condition(&cc), label.clone()))
        }
        TackyInstr::JumpIfNotZero(value, label) if value == dst => Some((cc, label.clone())),
        _ => None,
    }
}

fn invert_condition(cc: &CondCode) -> CondCode {
    match cc {
        CondCode::E => CondCode::NE,
        CondCode::NE => CondCode::E,
        CondCode::L => CondCode::GE,
        CondCode::LE => CondCode::G,
        CondCode::G => CondCode::LE,
        CondCode::GE => CondCode::L,
        CondCode::A => CondCode::BE,
        CondCode::AE => CondCode::B,
        CondCode::B => CondCode::AE,
        CondCode::BE => CondCode::A,
        CondCode::P => CondCode::NP,
        CondCode::NP => CondCode::P,
        CondCode::S => CondCode::NS,
        CondCode::NS => CondCode::S,
    }
}

fn is_unsigned_val(val: &TackyVal, types: &IndexMap<String, CType>) -> bool {
    match val {
        TackyVal::Var(name) => types
            .get(name)
            .copied()
            .is_some_and(|ctype| !ctype.is_signed()),
        _ => false,
    }
}

fn is_unsigned_comparison_val(val: &TackyVal, types: &IndexMap<String, CType>) -> bool {
    match val {
        TackyVal::Var(name) => types
            .get(name)
            .copied()
            .is_some_and(|ctype| ctype != CType::Double && !ctype.is_signed()),
        _ => false,
    }
}

fn is_aarch64_register_candidate_type(ctype: CType) -> bool {
    matches!(
        ctype,
        CType::Char
            | CType::SChar
            | CType::UChar
            | CType::Bool
            | CType::Short
            | CType::UShort
            | CType::Int
            | CType::UInt
            | CType::Long
            | CType::ULong
            | CType::Pointer
    )
}

fn aarch64_register_allocation_enabled_value(value: &str) -> bool {
    matches!(value, "1" | "true" | "on" | "yes")
}

fn aarch64_register_allocation_enabled() -> bool {
    std::env::var("RNQCC_AARCH64_REGALLOC")
        .map(|value| aarch64_register_allocation_enabled_value(&value))
        .unwrap_or(false)
}

fn compute_register_candidates(
    body: &[TackyInstr],
    stack_slots: &HashMap<String, i32>,
    aliased: &HashSet<String>,
    types: &IndexMap<String, CType>,
    array_sizes: &IndexMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    global_vars: &HashSet<String>,
) -> HashSet<String> {
    if !aarch64_register_allocation_enabled() {
        return HashSet::new();
    }

    let mut blocked = HashSet::with_capacity(body.len());
    for instr in body {
        if let TackyInstr::FunCall {
            name, args, dst, ..
        } = instr
        {
            if let TackyVal::Var(name) = dst {
                blocked.insert(name.clone());
            }
            blocked.insert(name.clone());
            for arg in args {
                if let TackyVal::Var(name) = arg {
                    blocked.insert(name.clone());
                }
            }
        }
    }

    let mut candidates = HashSet::with_capacity(stack_slots.len());
    for name in stack_slots.keys() {
        if name.starts_with("__rnqcc_tmp.")
            && !blocked.contains(name)
            && !aliased.contains(name)
            && !global_vars.contains(name)
            && !array_sizes.contains_key(name)
            && !var_struct_tags.contains_key(name)
            && is_aarch64_register_candidate_type(types.get(name).copied().unwrap_or(CType::Int))
        {
            candidates.insert(name.clone());
        }
    }
    candidates
}

fn compute_ret_regs(
    function: &TackyFunction,
    types: &IndexMap<String, CType>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &IndexMap<String, StructDef>,
) -> Vec<crate::backend::x86_64::regalloc::RegId> {
    use crate::backend::x86_64::regalloc::RegId;

    let struct_return_regs = |name: &str| -> Option<Vec<RegId>> {
        if types.get(name).copied() != Some(CType::Struct) {
            return None;
        }
        let tag = var_struct_tags.get(name)?;
        let def = struct_defs.get(tag)?;
        let classes = def.classify_with(struct_defs);
        if !struct_classes_return_in_registers(&classes) {
            return Some(vec![RegId::Gp(Reg::AX)]);
        }

        let mut gp_idx = 0usize;
        let mut fp_idx = 0usize;
        let mut regs = Vec::with_capacity(classes.len());
        for class in classes {
            match class {
                ParamClass::Integer if gp_idx < RETURN_GP_REGS.len() => {
                    regs.push(RegId::Gp(RETURN_GP_REGS[gp_idx]));
                    gp_idx += 1;
                }
                ParamClass::Sse if fp_idx < RETURN_FP_REGS.len() => {
                    regs.push(RegId::Xmm(RETURN_FP_REGS[fp_idx]));
                    fp_idx += 1;
                }
                _ => return Some(vec![RegId::Gp(Reg::AX)]),
            }
        }
        Some(regs)
    };

    for instr in &function.body {
        if let TackyInstr::Return(TackyVal::Var(name)) = instr {
            if let Some(regs) = struct_return_regs(name) {
                return regs;
            }
        }
    }

    match function.return_type {
        CType::Void => vec![],
        CType::Float | CType::Double | CType::LongDouble => vec![RegId::Xmm(XmmReg::XMM0)],
        CType::Int128 | CType::UInt128 => vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DI)],
        _ => vec![RegId::Gp(Reg::AX)],
    }
}

fn scalar_load_return_type(
    return_type: CType,
    loaded: &TackyVal,
    types: &IndexMap<String, CType>,
) -> Result<Option<AsmType>, String> {
    if matches!(
        return_type,
        CType::Void | CType::Struct | CType::Int128 | CType::UInt128 | CType::LongDouble
    ) {
        return Ok(None);
    }

    let ty = asm_type_for_val(loaded, types)?;
    if matches!(ty, AsmType::Octword | AsmType::LongDouble) {
        Ok(None)
    } else {
        Ok(Some(ty))
    }
}

fn scalar_return_operand(ty: AsmType) -> AsmOperand {
    if matches!(ty, AsmType::Float | AsmType::Double) {
        AsmOperand::Xmm(XmmReg::XMM0)
    } else {
        AsmOperand::Reg(Reg::AX)
    }
}

fn scalar_computed_return_type(
    return_type: CType,
    computed: &TackyVal,
    types: &IndexMap<String, CType>,
) -> Result<Option<AsmType>, String> {
    Ok(
        match scalar_load_return_type(return_type, computed, types)? {
            Some(
                ty @ (AsmType::Longword | AsmType::Quadword | AsmType::Float | AsmType::Double),
            ) => Some(ty),
            _ => None,
        },
    )
}

fn is_thread_local_val(val: &TackyVal, thread_local_vars: &HashSet<String>) -> bool {
    matches!(val, TackyVal::Var(name) if thread_local_vars.contains(name))
}

fn is_returned_local(
    dst: &TackyVal,
    next: Option<&TackyInstr>,
    global_vars: &HashSet<String>,
) -> bool {
    let TackyVal::Var(dst_name) = dst else {
        return false;
    };
    if global_vars.contains(dst_name) {
        return false;
    }
    matches!(next, Some(TackyInstr::Return(TackyVal::Var(ret_name))) if ret_name == dst_name)
}

fn replace_spilled_pseudos(
    function: &mut AsmFunction,
    stack_slots: &HashMap<String, i32>,
) -> Result<(), String> {
    fn replace_op(op: &mut AsmOperand, stack_slots: &HashMap<String, i32>) -> Result<(), String> {
        if let AsmOperand::Pseudo(name) = op {
            let offset = stack_slots
                .get(name)
                .copied()
                .ok_or_else(|| format!("AArch64 backend missing spill slot for {}", name))?;
            *op = AsmOperand::Stack(i64::from(offset));
        }
        Ok(())
    }

    let old_instructions = std::mem::take(&mut function.instructions);
    let mut new_instructions = Vec::with_capacity(old_instructions.len());

    for mut instr in old_instructions {
        match &mut instr {
            AsmInstr::Mov(_, src, dst)
            | AsmInstr::Movsx(_, _, src, dst)
            | AsmInstr::MovZeroExtend(_, _, src, dst)
            | AsmInstr::Binary(_, _, src, dst)
            | AsmInstr::Cmp(_, src, dst)
            | AsmInstr::Lea(src, dst)
            | AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Cvtsi2ss(_, src, dst)
            | AsmInstr::Cvttss2si(_, src, dst)
            | AsmInstr::Cvtss2sd(src, dst)
            | AsmInstr::Cvtsd2ss(src, dst)
            | AsmInstr::AArch64UIntToDouble(_, src, dst)
            | AsmInstr::AArch64DoubleToUInt(_, src, dst)
            | AsmInstr::AArch64UIntToFloat(_, src, dst)
            | AsmInstr::AArch64FloatToUInt(_, src, dst)
            | AsmInstr::AArch64FloatToDouble(src, dst)
            | AsmInstr::AArch64DoubleToFloat(src, dst) => {
                replace_op(src, stack_slots)?;
                replace_op(dst, stack_slots)?;
            }
            AsmInstr::Unary(_, _, op)
            | AsmInstr::Idiv(_, op)
            | AsmInstr::Div(_, op)
            | AsmInstr::SetCC(_, op)
            | AsmInstr::Push(op)
            | AsmInstr::JmpIndirect(op)
            | AsmInstr::LoadIndirect(_, _, op)
            | AsmInstr::StoreIndirect(_, op, _) => {
                replace_op(op, stack_slots)?;
            }
            AsmInstr::AArch64AddPtr(ptr, index, _, dst) => {
                replace_op(ptr, stack_slots)?;
                replace_op(index, stack_slots)?;
                replace_op(dst, stack_slots)?;
            }
            AsmInstr::AArch64Extr(high, low, _, dst) => {
                replace_op(high, stack_slots)?;
                replace_op(low, stack_slots)?;
                replace_op(dst, stack_slots)?;
            }
            AsmInstr::AArch64Umulh(left, right, dst) => {
                replace_op(left, stack_slots)?;
                replace_op(right, stack_slots)?;
                replace_op(dst, stack_slots)?;
            }
            AsmInstr::AArch64LoadAdjusted(_, src, _, _) => {
                replace_op(src, stack_slots)?;
            }
            AsmInstr::AArch64StoreOutgoingArg(_, src, _, _) => {
                replace_op(src, stack_slots)?;
            }
            AsmInstr::AArch64Rem(_, _, left, right, dst) => {
                replace_op(left, stack_slots)?;
                replace_op(right, stack_slots)?;
                replace_op(dst, stack_slots)?;
            }
            AsmInstr::CopyToStackArg { src_ptr, .. } => {
                replace_op(src_ptr, stack_slots)?;
            }
            AsmInstr::CopyFromStackArg { dst, .. } => {
                replace_op(dst, stack_slots)?;
            }
            AsmInstr::BuiltinSetjmp { buf, dst, .. } => {
                replace_op(buf, stack_slots)?;
                replace_op(dst, stack_slots)?;
            }
            AsmInstr::BuiltinLongjmp { buf, value } => {
                replace_op(buf, stack_slots)?;
                replace_op(value, stack_slots)?;
            }
            AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst)
            | AsmInstr::X87Store(dst)
            | AsmInstr::X87StoreFloat(_, dst)
            | AsmInstr::X87StoreInt(_, dst)
            | AsmInstr::Fld(_, dst)
            | AsmInstr::Fstp(_, dst)
            | AsmInstr::Fisttp(_, dst)
            | AsmInstr::FldQ(dst)
            | AsmInstr::X87Push(_, dst)
            | AsmInstr::X87Pop(_, dst) => {
                replace_op(dst, stack_slots)?;
            }
            _ => {}
        }

        if !matches!(&instr, AsmInstr::Mov(_, src, dst) if src == dst) {
            new_instructions.push(instr);
        }
    }

    function.instructions = new_instructions;

    Ok(())
}

fn convert_function(
    function: &TackyFunction,
    target: &Target,
    program: &TackyProgram,
    long_double_consts: &mut Vec<(String, f64)>,
    no_coalescing: bool,
) -> Result<AsmFunction, String> {
    let types = &program.symbol_types;
    let array_sizes = &program.array_sizes;
    let var_struct_tags = &program.var_struct_tags;
    let struct_defs = &program.struct_defs;
    let global_vars = &program.global_vars;
    let thread_local_vars = &program.thread_local_vars;
    let alignments = &program.symbol_alignments;
    let stack_layout = collect_stack_slots(
        function,
        types,
        array_sizes,
        var_struct_tags,
        struct_defs,
        global_vars,
        alignments,
    )?;
    let raw_stack_slots = stack_layout.slots;
    let aliased = crate::backend::common::compute_aliased(&function.body, global_vars);
    let register_vars = compute_register_candidates(
        &function.body,
        &raw_stack_slots,
        &aliased,
        types,
        array_sizes,
        var_struct_tags,
        global_vars,
    );
    let stack_slots = RegisterStackSlots {
        slots: &raw_stack_slots,
        register_vars: &register_vars,
    };
    let stack_size = stack_layout.local_stack_size;
    let saves_link_register = function.body.iter().any(|instr| match instr {
        TackyInstr::FunCall { .. } => true,
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            (matches!(asm_type_for_val(dst, types), Ok(AsmType::LongDouble))
                && long_double_helper(op).is_some()
                || (long_double_comparison_helper(op).is_some()
                    && (matches!(asm_type_for_val(left, types), Ok(AsmType::LongDouble))
                        || matches!(asm_type_for_val(right, types), Ok(AsmType::LongDouble)))))
                || (matches!(
                    (op, asm_type_for_val(dst, types)),
                    (
                        TackyBinaryOp::Div | TackyBinaryOp::Mod,
                        Ok(AsmType::Octword)
                    )
                ) && i128_div_or_mod_requires_helper(op, left, right, types))
        }
        _ => false,
    });
    let frame_size = compute_frame_size(stack_size, saves_link_register);
    let link_register_offset =
        saves_link_register.then(|| compute_link_register_offset(frame_size));
    let mut large_local_offsets = HashMap::with_capacity(stack_layout.large_locals.len());
    let mut large_stack_size = 0i64;
    for local in &stack_layout.large_locals {
        let base_offset = frame_size as i64 + large_stack_size;
        large_local_offsets.insert(local.name.clone(), (local.base_slot, base_offset));
        let aligned_size = align_usize_to(local.size, LARGE_LOCAL_ALIGNMENT)?;
        let aligned_size = i64::try_from(aligned_size)
            .map_err(|_| format!("AArch64 backend large local too large: {}", local.name))?;
        large_stack_size = large_stack_size
            .checked_add(aligned_size)
            .ok_or_else(|| "AArch64 backend large stack allocation overflow".to_string())?;
    }
    large_stack_size = align_i64_to(large_stack_size, STACK_ALIGNMENT as i64)?;
    if large_stack_size > 0
        && (!function.stack_params.is_empty()
            || !function.memory_param_blocks.is_empty()
            || function.params.len() > ARG_REGS.len()
            || function
                .body
                .iter()
                .any(|instr| matches!(instr, TackyInstr::VaStart { .. })))
    {
        return Err(format!(
            "AArch64 backend does not yet support large local arrays with incoming stack arguments in {}",
            function.name
        ));
    }
    let mut instructions = Vec::with_capacity(
        function.body.len() + function.params.len() + stack_layout.large_locals.len() + 8,
    );
    if large_stack_size > 0 {
        instructions.push(AsmInstr::AArch64AllocateLargeStack(large_stack_size));
    }
    if frame_size > 0 {
        instructions.push(AsmInstr::AllocateStack(i64::from(frame_size)));
    }
    if let Some(offset) = link_register_offset {
        instructions.push(AsmInstr::AArch64SaveLink(offset));
    }
    for local in &stack_layout.large_locals {
        let Some((base_slot, base_offset)) = large_local_offsets.get(&local.name).copied() else {
            continue;
        };
        instructions.push(AsmInstr::AArch64StoreLargeLocalBase {
            base_offset,
            dst_offset: base_slot,
        });
    }
    let mut param_groups: HashMap<usize, (usize, &[bool])> =
        HashMap::with_capacity(function.struct_param_groups.len());
    for (start, count, is_sse) in &function.struct_param_groups {
        param_groups.insert(*start, (*count, is_sse.as_slice()));
    }
    let mut memory_param_blocks: HashMap<usize, (&String, usize)> =
        HashMap::with_capacity(function.memory_param_blocks.len());
    for (index, name, size) in &function.memory_param_blocks {
        memory_param_blocks.insert(*index, (name, *size));
    }
    let mut gp_param_count = 0usize;
    let mut fp_param_count = 0usize;
    let mut stack_param_count = 0usize;
    let mut param_index = 0usize;
    while param_index < function.params.len() {
        if let Some((dst_name, size)) = memory_param_blocks.get(&param_index).copied() {
            let src_start = stack_arg_offset(frame_size, stack_param_count);
            emit_copy_incoming_arg_to_aggregate(
                &mut instructions,
                src_start,
                dst_name,
                size,
                &stack_slots,
                global_vars,
            )?;
            stack_param_count += size.div_ceil(STACK_SLOT_SIZE as usize);
            param_index += 1;
            continue;
        }
        if let Some((count, is_sse)) = param_groups.get(&param_index) {
            let needs = group_register_needs(is_sse);
            let fits_registers = gp_param_count + needs.gp <= ARG_REGS.len()
                && fp_param_count + needs.fp <= FP_ARG_REGS.len();
            for (group_offset, is_fp) in is_sse.iter().copied().enumerate().take(*count) {
                let param = &function.params[param_index + group_offset];
                let ty = asm_type_for_val(&TackyVal::Var(param.clone()), types)?;
                let src = if fits_registers {
                    if is_fp {
                        let src = AsmOperand::Xmm(FP_ARG_REGS[fp_param_count]);
                        fp_param_count += 1;
                        src
                    } else {
                        let src = AsmOperand::Reg(ARG_REGS[gp_param_count]);
                        gp_param_count += 1;
                        src
                    }
                } else {
                    let src = AsmOperand::Stack(i64::from(stack_arg_offset(
                        frame_size,
                        stack_param_count,
                    )));
                    stack_param_count += 1;
                    src
                };
                instructions.push(AsmInstr::Mov(
                    ty,
                    src,
                    val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?,
                ));
            }
            param_index += *count;
            continue;
        }

        let param = &function.params[param_index];
        let ty = asm_type_for_val(&TackyVal::Var(param.clone()), types)?;
        if ty == AsmType::Octword
            && !function.stack_params.contains(param)
            && gp_param_count + 1 < ARG_REGS.len()
        {
            let dst = val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?;
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(ARG_REGS[gp_param_count]),
                low64_operand(&dst)?,
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(ARG_REGS[gp_param_count + 1]),
                high64_operand(&dst)?,
            ));
            gp_param_count += 2;
            param_index += 1;
            continue;
        }
        let src = if function.stack_params.contains(param) {
            if ty == AsmType::Octword {
                let dst = val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?;
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(i64::from(stack_arg_offset(frame_size, stack_param_count))),
                    low64_operand(&dst)?,
                ));
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(i64::from(stack_arg_offset(
                        frame_size,
                        stack_param_count + 1,
                    ))),
                    high64_operand(&dst)?,
                ));
                stack_param_count += 2;
                param_index += 1;
                continue;
            }
            let src = AsmOperand::Stack(i64::from(stack_arg_offset(frame_size, stack_param_count)));
            stack_param_count += 1;
            src
        } else if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
            if fp_param_count < FP_ARG_REGS.len() {
                let src = AsmOperand::Xmm(FP_ARG_REGS[fp_param_count]);
                fp_param_count += 1;
                src
            } else {
                let src =
                    AsmOperand::Stack(i64::from(stack_arg_offset(frame_size, stack_param_count)));
                stack_param_count += if ty == AsmType::LongDouble { 2 } else { 1 };
                src
            }
        } else if gp_param_count < ARG_REGS.len() {
            let src = AsmOperand::Reg(ARG_REGS[gp_param_count]);
            gp_param_count += 1;
            src
        } else {
            if matches!(ty, AsmType::Octword | AsmType::LongDouble) {
                let dst = val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?;
                if ty == AsmType::LongDouble {
                    instructions.push(AsmInstr::Mov(
                        AsmType::LongDouble,
                        AsmOperand::Stack(i64::from(stack_arg_offset(
                            frame_size,
                            stack_param_count,
                        ))),
                        dst,
                    ));
                } else {
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Stack(i64::from(stack_arg_offset(
                            frame_size,
                            stack_param_count,
                        ))),
                        low64_operand(&dst)?,
                    ));
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Stack(i64::from(stack_arg_offset(
                            frame_size,
                            stack_param_count + 1,
                        ))),
                        high64_operand(&dst)?,
                    ));
                }
                stack_param_count += 2;
                param_index += 1;
                continue;
            }
            let src = AsmOperand::Stack(i64::from(stack_arg_offset(frame_size, stack_param_count)));
            stack_param_count += 1;
            src
        };
        instructions.push(AsmInstr::Mov(
            ty,
            src,
            val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?,
        ));
        param_index += 1;
    }
    let va_start_stack_offset = stack_arg_offset(frame_size, stack_param_count);
    let i128_ctx = Aarch64I128Context {
        function_name: &function.name,
        types,
        stack_slots: &raw_stack_slots,
        global_vars,
    };

    let mut body_iter = function.body.iter().peekable();
    while let Some(instr) = body_iter.next() {
        let next_instr = body_iter.peek().copied();
        if next_instr.is_some() {
            let returned_dst = match instr {
                TackyInstr::Copy { dst, .. }
                | TackyInstr::SignExtend { dst, .. }
                | TackyInstr::ZeroExtend { dst, .. }
                | TackyInstr::Truncate { dst, .. }
                | TackyInstr::IntToDouble { dst, .. }
                | TackyInstr::IntToFloat { dst, .. }
                | TackyInstr::UIntToDouble { dst, .. }
                | TackyInstr::UIntToFloat { dst, .. }
                | TackyInstr::DoubleToInt { dst, .. }
                | TackyInstr::FloatToInt { dst, .. }
                | TackyInstr::DoubleToUInt { dst, .. }
                | TackyInstr::FloatToUInt { dst, .. }
                | TackyInstr::FloatToDouble { dst, .. }
                | TackyInstr::DoubleToFloat { dst, .. } => match dst {
                    _ if is_returned_local(dst, next_instr, global_vars) => Some(dst),
                    _ => None,
                },
                _ => None,
            };

            if let Some(dst) = returned_dst {
                if let TackyInstr::Copy { src, .. } = instr {
                    if matches!(function.return_type, CType::LongDouble)
                        && asm_type_for_val(dst, types)? == AsmType::LongDouble
                    {
                        instructions.push(AsmInstr::Mov(
                            AsmType::LongDouble,
                            val_operand(src, &stack_slots, global_vars)?,
                            AsmOperand::Xmm(XmmReg::XMM0),
                        ));
                        emit_epilogue(
                            &mut instructions,
                            frame_size,
                            large_stack_size,
                            link_register_offset,
                        );
                        body_iter.next();
                        continue;
                    }
                }
                if let Some(dst_ty) = scalar_load_return_type(function.return_type, dst, types)? {
                    let ret_dst = scalar_return_operand(dst_ty);
                    match instr {
                        TackyInstr::Copy { src, .. } => {
                            if dst_ty == AsmType::Octword {
                                emit_i128_return(
                                    &mut instructions,
                                    src,
                                    &stack_slots,
                                    global_vars,
                                )?;
                            } else {
                                let src_operand =
                                    if matches!(dst_ty, AsmType::Float | AsmType::Double) {
                                        floating_return_operand(
                                            dst_ty,
                                            src,
                                            &stack_slots,
                                            global_vars,
                                        )?
                                    } else {
                                        val_operand(src, &stack_slots, global_vars)?
                                    };
                                instructions.push(AsmInstr::Mov(dst_ty, src_operand, ret_dst));
                            }
                        }
                        TackyInstr::SignExtend { src, .. } => {
                            let src_ty = asm_type_for_val(src, types)?;
                            match src {
                                TackyVal::Constant(c) if dst_ty != AsmType::Byte => {
                                    instructions.push(AsmInstr::Mov(
                                        dst_ty,
                                        AsmOperand::Imm(*c),
                                        ret_dst,
                                    ));
                                }
                                _ => {
                                    instructions.push(AsmInstr::Movsx(
                                        src_ty,
                                        dst_ty,
                                        val_operand(src, &stack_slots, global_vars)?,
                                        ret_dst,
                                    ));
                                }
                            }
                        }
                        TackyInstr::ZeroExtend { src, .. } => {
                            let src_ty = asm_type_for_val(src, types)?;
                            match src {
                                TackyVal::Constant(c) if dst_ty != AsmType::Byte => {
                                    instructions.push(AsmInstr::Mov(
                                        dst_ty,
                                        AsmOperand::Imm(*c),
                                        ret_dst,
                                    ));
                                }
                                _ => {
                                    instructions.push(AsmInstr::MovZeroExtend(
                                        src_ty,
                                        dst_ty,
                                        val_operand(src, &stack_slots, global_vars)?,
                                        ret_dst,
                                    ));
                                }
                            }
                        }
                        TackyInstr::Truncate { src, .. } => {
                            let src_op = if asm_type_for_val(src, types)? == AsmType::Octword {
                                low64_operand(&val_operand(src, &stack_slots, global_vars)?)?
                            } else {
                                val_operand(src, &stack_slots, global_vars)?
                            };
                            instructions.push(AsmInstr::Mov(dst_ty, src_op, ret_dst));
                        }
                        TackyInstr::IntToDouble { src, .. } => {
                            let src_ty = asm_type_for_val(src, types)?;
                            if matches!(src_ty, AsmType::Byte | AsmType::Word) {
                                instructions.push(AsmInstr::Movsx(
                                    src_ty,
                                    AsmType::Longword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                instructions.push(AsmInstr::Cvtsi2sd(
                                    AsmType::Longword,
                                    AsmOperand::Reg(Reg::R10),
                                    ret_dst,
                                ));
                            } else {
                                instructions.push(AsmInstr::Cvtsi2sd(
                                    src_ty,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    ret_dst,
                                ));
                            }
                        }
                        TackyInstr::IntToFloat { src, .. } => {
                            let src_ty = asm_type_for_val(src, types)?;
                            if matches!(src_ty, AsmType::Byte | AsmType::Word) {
                                instructions.push(AsmInstr::Movsx(
                                    src_ty,
                                    AsmType::Longword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                instructions.push(AsmInstr::Cvtsi2ss(
                                    AsmType::Longword,
                                    AsmOperand::Reg(Reg::R10),
                                    ret_dst,
                                ));
                            } else {
                                instructions.push(AsmInstr::Cvtsi2ss(
                                    src_ty,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    ret_dst,
                                ));
                            }
                        }
                        TackyInstr::UIntToDouble { src, .. } => {
                            instructions.push(AsmInstr::AArch64UIntToDouble(
                                asm_type_for_val(src, types)?,
                                val_operand(src, &stack_slots, global_vars)?,
                                ret_dst,
                            ));
                        }
                        TackyInstr::UIntToFloat { src, .. } => {
                            instructions.push(AsmInstr::AArch64UIntToFloat(
                                asm_type_for_val(src, types)?,
                                val_operand(src, &stack_slots, global_vars)?,
                                ret_dst,
                            ));
                        }
                        TackyInstr::DoubleToInt { src, .. } => {
                            if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                                instructions.push(AsmInstr::Cvttsd2si(
                                    AsmType::Longword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                instructions.push(AsmInstr::Mov(
                                    dst_ty,
                                    AsmOperand::Reg(Reg::R10),
                                    ret_dst,
                                ));
                            } else {
                                instructions.push(AsmInstr::Cvttsd2si(
                                    dst_ty,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    ret_dst,
                                ));
                            }
                        }
                        TackyInstr::FloatToInt { src, .. } => {
                            if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                                instructions.push(AsmInstr::Cvttss2si(
                                    AsmType::Longword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                instructions.push(AsmInstr::Mov(
                                    dst_ty,
                                    AsmOperand::Reg(Reg::R10),
                                    ret_dst,
                                ));
                            } else {
                                instructions.push(AsmInstr::Cvttss2si(
                                    dst_ty,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    ret_dst,
                                ));
                            }
                        }
                        TackyInstr::DoubleToUInt { src, .. } => {
                            if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                                instructions.push(AsmInstr::AArch64DoubleToUInt(
                                    AsmType::Longword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                instructions.push(AsmInstr::Mov(
                                    dst_ty,
                                    AsmOperand::Reg(Reg::R10),
                                    ret_dst,
                                ));
                            } else {
                                instructions.push(AsmInstr::AArch64DoubleToUInt(
                                    dst_ty,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    ret_dst,
                                ));
                            }
                        }
                        TackyInstr::FloatToUInt { src, .. } => {
                            if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                                instructions.push(AsmInstr::AArch64FloatToUInt(
                                    AsmType::Longword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    AsmOperand::Reg(Reg::R10),
                                ));
                                instructions.push(AsmInstr::Mov(
                                    dst_ty,
                                    AsmOperand::Reg(Reg::R10),
                                    ret_dst,
                                ));
                            } else {
                                instructions.push(AsmInstr::AArch64FloatToUInt(
                                    dst_ty,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    ret_dst,
                                ));
                            }
                        }
                        TackyInstr::FloatToDouble { src, .. } => {
                            instructions.push(AsmInstr::AArch64FloatToDouble(
                                val_operand(src, &stack_slots, global_vars)?,
                                ret_dst,
                            ));
                        }
                        TackyInstr::DoubleToFloat { src, .. } => {
                            instructions.push(AsmInstr::AArch64DoubleToFloat(
                                val_operand(src, &stack_slots, global_vars)?,
                                ret_dst,
                            ));
                        }
                        _ => unreachable!(),
                    }
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
            }
        }
        if let TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } = instr
        {
            if is_returned_local(dst, next_instr, global_vars) {
                if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)? {
                    let ret_dst = scalar_return_operand(ty);
                    instructions.push(AsmInstr::AArch64AddPtr(
                        val_operand(ptr, &stack_slots, global_vars)?,
                        val_operand(index, &stack_slots, global_vars)?,
                        *scale,
                        ret_dst,
                    ));
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
            }
        }
        if let TackyInstr::LoadLabelAddress(label, dst) = instr {
            if is_returned_local(dst, next_instr, global_vars) {
                if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)? {
                    instructions.push(AsmInstr::LoadLabelAddress(
                        label.clone(),
                        scalar_return_operand(ty),
                    ));
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
            }
        }
        if let TackyInstr::FrameAddress { dst } = instr {
            if is_returned_local(dst, next_instr, global_vars) {
                if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)? {
                    instructions.push(AsmInstr::Lea(
                        AsmOperand::Stack(i64::from(frame_size)),
                        scalar_return_operand(ty),
                    ));
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
            }
        }
        if let TackyInstr::GetAddress { src, dst } = instr {
            if is_returned_local(dst, next_instr, global_vars) {
                if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)? {
                    let TackyVal::Var(name) = src else {
                        return Err("AArch64 backend can only take addresses of local variables"
                            .to_string());
                    };
                    let ret_dst = scalar_return_operand(ty);
                    if let Some((base_slot, _)) = large_local_offsets.get(name) {
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            AsmOperand::Stack(i64::from(*base_slot)),
                            ret_dst,
                        ));
                    } else {
                        instructions.push(AsmInstr::Lea(
                            stack_or_data_operand(name, 0, &stack_slots, global_vars)?,
                            ret_dst,
                        ));
                    }
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
            }
        }
        if let TackyInstr::Load { src_ptr, dst } = instr {
            if is_returned_local(dst, next_instr, global_vars) {
                if matches!(function.return_type, CType::Int128 | CType::UInt128)
                    && asm_type_for_val(dst, types)? == AsmType::Octword
                {
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        val_operand(src_ptr, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R11),
                    ));
                    instructions.push(AsmInstr::LoadIndirect(
                        AsmType::Octword,
                        Reg::R11,
                        AsmOperand::Reg(Reg::AX),
                    ));
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
                if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)? {
                    let ret_dst = scalar_return_operand(ty);
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        val_operand(src_ptr, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R11),
                    ));
                    instructions.push(AsmInstr::LoadIndirect(ty, Reg::R11, ret_dst));
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
            }
        }
        if let TackyInstr::CopyFromOffset {
            src_name,
            offset,
            dst,
        } = instr
        {
            if is_returned_local(dst, next_instr, global_vars) {
                if matches!(function.return_type, CType::Int128 | CType::UInt128)
                    && asm_type_for_val(dst, types)? == AsmType::Octword
                {
                    let src_op =
                        stack_or_data_operand(src_name, *offset as i32, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        low64_operand(&src_op)?,
                        AsmOperand::Reg(Reg::AX),
                    ));
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        high64_operand(&src_op)?,
                        AsmOperand::Reg(Reg::DI),
                    ));
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
                if let Some(ty) = scalar_load_return_type(function.return_type, dst, types)? {
                    let ret_dst = scalar_return_operand(ty);
                    instructions.push(AsmInstr::Mov(
                        ty,
                        stack_or_data_operand(src_name, *offset as i32, &stack_slots, global_vars)?,
                        ret_dst,
                    ));
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
            }
        }
        if let TackyInstr::Unary { op, src, dst } = instr {
            if is_returned_local(dst, next_instr, global_vars) {
                if asm_type_for_val(dst, types)? == AsmType::Octword
                    && matches!(function.return_type, CType::Int128 | CType::UInt128)
                {
                    match op {
                        TackyUnaryOp::LogicalNot => {}
                        TackyUnaryOp::Negate | TackyUnaryOp::Complement => {
                            let asm_op = match op {
                                TackyUnaryOp::Negate => AsmUnaryOp::Neg,
                                TackyUnaryOp::Complement => AsmUnaryOp::Not,
                                TackyUnaryOp::LogicalNot => unreachable!(),
                            };
                            emit_i128_unary(
                                &mut instructions,
                                src,
                                AsmOperand::Reg(Reg::AX),
                                asm_op,
                                &stack_slots,
                                global_vars,
                            )?;
                            emit_epilogue(
                                &mut instructions,
                                frame_size,
                                large_stack_size,
                                link_register_offset,
                            );
                            body_iter.next();
                            continue;
                        }
                    }
                }
                if let Some(ty) = scalar_load_return_type(function.return_type, dst, types)? {
                    let ret_dst = scalar_return_operand(ty);
                    if matches!(op, TackyUnaryOp::LogicalNot) {
                        let src_ty = asm_type_for_val(src, types)?;
                        if src_ty == AsmType::Octword {
                            emit_i128_zero_cmp(&mut instructions, src, &stack_slots, global_vars)?;
                            instructions.push(AsmInstr::SetCC(CondCode::E, ret_dst));
                            emit_epilogue(
                                &mut instructions,
                                frame_size,
                                large_stack_size,
                                link_register_offset,
                            );
                            body_iter.next();
                            continue;
                        } else {
                            instructions.push(AsmInstr::Cmp(
                                src_ty,
                                AsmOperand::Imm(0),
                                val_operand(src, &stack_slots, global_vars)?,
                            ));
                            instructions.push(AsmInstr::SetCC(CondCode::E, ret_dst));
                            emit_epilogue(
                                &mut instructions,
                                frame_size,
                                large_stack_size,
                                link_register_offset,
                            );
                            body_iter.next();
                            continue;
                        }
                    } else {
                        let asm_op = match op {
                            TackyUnaryOp::Negate => AsmUnaryOp::Neg,
                            TackyUnaryOp::Complement => AsmUnaryOp::Not,
                            TackyUnaryOp::LogicalNot => unreachable!(),
                        };
                        instructions.push(AsmInstr::Mov(
                            ty,
                            val_operand(src, &stack_slots, global_vars)?,
                            ret_dst.clone(),
                        ));
                        instructions.push(AsmInstr::Unary(ty, asm_op, ret_dst));
                        emit_epilogue(
                            &mut instructions,
                            frame_size,
                            large_stack_size,
                            link_register_offset,
                        );
                        body_iter.next();
                        continue;
                    }
                }
            }
        }
        if let TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } = instr
        {
            if is_returned_local(dst, next_instr, global_vars) {
                let left_ty = asm_type_for_val(left, types)?;
                let right_ty = asm_type_for_val(right, types)?;
                let special_operand = matches!(left_ty, AsmType::Octword | AsmType::LongDouble)
                    || matches!(right_ty, AsmType::Octword | AsmType::LongDouble)
                    || is_thread_local_val(right, thread_local_vars);
                if matches!(op, TackyBinaryOp::Equal | TackyBinaryOp::NotEqual)
                    && (matches!(left_ty, AsmType::Octword) || matches!(right_ty, AsmType::Octword))
                {
                    if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)?
                    {
                        emit_i128_eq_cmp(
                            &mut instructions,
                            left,
                            right,
                            op,
                            scalar_return_operand(ty),
                            &stack_slots,
                            global_vars,
                        )?;
                        emit_epilogue(
                            &mut instructions,
                            frame_size,
                            large_stack_size,
                            link_register_offset,
                        );
                        body_iter.next();
                        continue;
                    }
                }
                if matches!(
                    op,
                    TackyBinaryOp::LessThan
                        | TackyBinaryOp::LessEqual
                        | TackyBinaryOp::GreaterThan
                        | TackyBinaryOp::GreaterEqual
                ) && (matches!(left_ty, AsmType::Octword)
                    || matches!(right_ty, AsmType::Octword))
                {
                    if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)?
                    {
                        if is_unsigned_comparison_val(left, types) {
                            emit_i128_unsigned_cmp(
                                &mut instructions,
                                left,
                                right,
                                op,
                                scalar_return_operand(ty),
                                &i128_ctx,
                            )?;
                        } else {
                            emit_i128_signed_cmp(
                                &mut instructions,
                                left,
                                right,
                                op,
                                scalar_return_operand(ty),
                                &i128_ctx,
                            )?;
                        }
                        emit_epilogue(
                            &mut instructions,
                            frame_size,
                            large_stack_size,
                            link_register_offset,
                        );
                        body_iter.next();
                        continue;
                    }
                }
                if matches!(function.return_type, CType::Int128 | CType::UInt128)
                    && matches!(left_ty, AsmType::Octword)
                    && asm_type_for_val(dst, types)? == AsmType::Octword
                    && emit_i128_shift(
                        &mut instructions,
                        op,
                        left,
                        right,
                        AsmOperand::Reg(Reg::AX),
                        &i128_ctx,
                    )?
                {
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
                if matches!(function.return_type, CType::Int128 | CType::UInt128)
                    && matches!(left_ty, AsmType::Octword)
                    && matches!(right_ty, AsmType::Octword)
                    && asm_type_for_val(dst, types)? == AsmType::Octword
                    && (emit_i128_basic_binary(
                        &mut instructions,
                        op,
                        left,
                        right,
                        AsmOperand::Reg(Reg::AX),
                        &stack_slots,
                        global_vars,
                    )? || emit_i128_helper_binary(
                        &mut instructions,
                        op,
                        left,
                        right,
                        AsmOperand::Reg(Reg::AX),
                        &i128_ctx,
                    )?)
                {
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
                if matches!(left_ty, AsmType::LongDouble) || matches!(right_ty, AsmType::LongDouble)
                {
                    if let Some(comparison) = long_double_comparison_helper(op) {
                        if let Some(ty) =
                            scalar_computed_return_type(function.return_type, dst, types)?
                        {
                            emit_long_double_comparison(
                                &mut instructions,
                                comparison,
                                left,
                                right,
                                scalar_return_operand(ty),
                                &stack_slots,
                                global_vars,
                            )?;
                            emit_epilogue(
                                &mut instructions,
                                frame_size,
                                large_stack_size,
                                link_register_offset,
                            );
                            body_iter.next();
                            continue;
                        }
                    }
                    if let Some(helper) = long_double_helper(op) {
                        if matches!(function.return_type, CType::LongDouble)
                            && asm_type_for_val(dst, types)? == AsmType::LongDouble
                        {
                            emit_long_double_helper_call(
                                &mut instructions,
                                helper,
                                left,
                                right,
                                AsmOperand::Xmm(XmmReg::XMM0),
                                &stack_slots,
                                global_vars,
                            )?;
                            emit_epilogue(
                                &mut instructions,
                                frame_size,
                                large_stack_size,
                                link_register_offset,
                            );
                            body_iter.next();
                            continue;
                        }
                    }
                }
                if !special_operand {
                    if let Some(ty) = scalar_computed_return_type(function.return_type, dst, types)?
                    {
                        let ret_dst = scalar_return_operand(ty);
                        if let Some(cc) =
                            convert_comparison_op(op, is_unsigned_comparison_val(left, types))
                        {
                            let cmp_ty = match (left_ty, right_ty) {
                                (AsmType::Double, _) | (_, AsmType::Double) => AsmType::Double,
                                (AsmType::Float, _) | (_, AsmType::Float) => AsmType::Float,
                                (AsmType::Quadword, _) | (_, AsmType::Quadword) => {
                                    AsmType::Quadword
                                }
                                (AsmType::Longword, _) | (_, AsmType::Longword) => {
                                    AsmType::Longword
                                }
                                (AsmType::Word, _) | (_, AsmType::Word) => AsmType::Word,
                                _ => AsmType::Byte,
                            };
                            let right_op = if matches!(cmp_ty, AsmType::Float | AsmType::Double) {
                                floating_return_operand(cmp_ty, right, &stack_slots, global_vars)?
                            } else {
                                val_operand(right, &stack_slots, global_vars)?
                            };
                            let left_op = if matches!(cmp_ty, AsmType::Float | AsmType::Double) {
                                floating_return_operand(cmp_ty, left, &stack_slots, global_vars)?
                            } else {
                                val_operand(left, &stack_slots, global_vars)?
                            };
                            instructions.push(AsmInstr::Cmp(cmp_ty, right_op, left_op));
                            instructions.push(AsmInstr::SetCC(cc, ret_dst));
                            emit_epilogue(
                                &mut instructions,
                                frame_size,
                                large_stack_size,
                                link_register_offset,
                            );
                            body_iter.next();
                            continue;
                        }
                        let asm_op = match op {
                            TackyBinaryOp::Div
                                if matches!(ty, AsmType::Float | AsmType::Double) =>
                            {
                                AsmBinaryOp::DivDouble
                            }
                            TackyBinaryOp::Div => {
                                if is_unsigned_val(dst, types) {
                                    AsmBinaryOp::UDiv
                                } else {
                                    AsmBinaryOp::SDiv
                                }
                            }
                            TackyBinaryOp::ShiftRight => {
                                if is_unsigned_val(left, types) {
                                    AsmBinaryOp::Shr
                                } else {
                                    AsmBinaryOp::Sar
                                }
                            }
                            TackyBinaryOp::Mod => {
                                instructions.push(AsmInstr::AArch64Rem(
                                    ty,
                                    is_unsigned_val(dst, types),
                                    val_operand(left, &stack_slots, global_vars)?,
                                    val_operand(right, &stack_slots, global_vars)?,
                                    ret_dst,
                                ));
                                emit_epilogue(
                                    &mut instructions,
                                    frame_size,
                                    large_stack_size,
                                    link_register_offset,
                                );
                                body_iter.next();
                                continue;
                            }
                            _ => convert_binary_op(op)?,
                        };
                        let left_op = if matches!(ty, AsmType::Float | AsmType::Double) {
                            floating_return_operand(ty, left, &stack_slots, global_vars)?
                        } else {
                            val_operand(left, &stack_slots, global_vars)?
                        };
                        let right_op = if matches!(ty, AsmType::Float | AsmType::Double) {
                            floating_return_operand(ty, right, &stack_slots, global_vars)?
                        } else {
                            val_operand(right, &stack_slots, global_vars)?
                        };
                        instructions.push(AsmInstr::Mov(ty, left_op, ret_dst.clone()));
                        instructions.push(AsmInstr::Binary(ty, asm_op, right_op, ret_dst));
                        emit_epilogue(
                            &mut instructions,
                            frame_size,
                            large_stack_size,
                            link_register_offset,
                        );
                        body_iter.next();
                        continue;
                    }
                }
            }
        }
        match instr {
            TackyInstr::Unreachable => {
                instructions.push(AsmInstr::Unreachable);
            }
            TackyInstr::Nop => {}
            TackyInstr::AtomicFence => {
                instructions.push(AsmInstr::AtomicFence);
            }
            TackyInstr::AtomicFetch {
                op,
                ptr,
                arg,
                return_old,
                dst,
            } => {
                let ty = asm_type_for_val(dst, types)?;
                if matches!(ty, AsmType::Float | AsmType::Double) {
                    return Err("AArch64 backend cannot atomic-fetch floating values".to_string());
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(arg, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicRmw(
                    ty,
                    convert_binary_op(op)?,
                    *return_old,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AtomicExchange { ptr, value, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                if matches!(ty, AsmType::Float | AsmType::Double) {
                    return Err(
                        "AArch64 backend cannot atomic-exchange floating values".to_string()
                    );
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(value, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicExchange(
                    ty,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AtomicCompareExchange {
                ptr,
                expected,
                desired,
                dst,
            } => {
                let desired_ty = asm_type_for_val(desired, types)?;
                if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                    return Err(
                        "AArch64 backend cannot atomic-compare-exchange floating values"
                            .to_string(),
                    );
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(expected, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R12),
                ));
                instructions.push(AsmInstr::Mov(
                    desired_ty,
                    val_operand(desired, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicCompareExchange(
                    desired_ty,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                return_old,
                dst,
            } => {
                let desired_ty = asm_type_for_val(desired, types)?;
                if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                    return Err(
                        "AArch64 backend cannot sync compare-and-swap floating values".to_string(),
                    );
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    desired_ty,
                    val_operand(expected, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R12),
                ));
                instructions.push(AsmInstr::Mov(
                    desired_ty,
                    val_operand(desired, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicCompareSwap(
                    desired_ty,
                    *return_old,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Return(val) => {
                if val_ctype(val, types) == Some(CType::Struct) {
                    let classes = struct_classes_for_val(val, var_struct_tags, struct_defs)
                        .ok_or_else(|| {
                            "AArch64 backend missing struct class for return value".to_string()
                        })?;
                    if struct_size_for_val(val, array_sizes, var_struct_tags, struct_defs)
                        .is_some_and(|size| size > 16)
                    {
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            val_operand(val, &stack_slots, global_vars)?,
                            AsmOperand::Reg(Reg::AX),
                        ));
                    } else {
                        move_struct_to_return_regs(
                            &mut instructions,
                            val,
                            &classes,
                            &stack_slots,
                            global_vars,
                        )?;
                    }
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    continue;
                }
                let ty = match val {
                    TackyVal::Var(_) => asm_type_for_val(val, types)?,
                    _ => function.return_type.into(),
                };
                if ty == AsmType::Octword {
                    emit_i128_return(&mut instructions, val, &stack_slots, global_vars)?;
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    continue;
                }
                let ret_dst =
                    if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
                        AsmOperand::Xmm(XmmReg::XMM0)
                    } else {
                        AsmOperand::Reg(Reg::AX)
                    };
                let src = if matches!(ty, AsmType::Float | AsmType::Double) {
                    floating_return_operand(ty, val, &stack_slots, global_vars)?
                } else {
                    val_operand(val, &stack_slots, global_vars)?
                };
                instructions.push(AsmInstr::Mov(ty, src, ret_dst));
                emit_epilogue(
                    &mut instructions,
                    frame_size,
                    large_stack_size,
                    link_register_offset,
                );
            }
            TackyInstr::Copy { src, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                if ty == AsmType::Octword {
                    emit_i128_copy(&mut instructions, src, dst, &stack_slots, global_vars)?;
                    continue;
                }
                let src_operand = if matches!(ty, AsmType::Float | AsmType::Double) {
                    floating_return_operand(ty, src, &stack_slots, global_vars)?
                } else {
                    val_operand(src, &stack_slots, global_vars)?
                };
                instructions.push(AsmInstr::Mov(
                    ty,
                    src_operand,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::SignExtend { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                let dst_ty = asm_type_for_val(dst, types)?;
                if dst_ty == AsmType::Octword {
                    let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                    let dst_low = low64_operand(&dst_op)?;
                    let dst_high = high64_operand(&dst_op)?;
                    match src {
                        TackyVal::Constant(c) => {
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Imm(*c),
                                dst_low,
                            ));
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Imm(if *c < 0 { -1 } else { 0 }),
                                dst_high,
                            ));
                        }
                        _ => {
                            if src_ty == AsmType::Quadword {
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    dst_low.clone(),
                                ));
                            } else {
                                instructions.push(AsmInstr::Movsx(
                                    src_ty,
                                    AsmType::Quadword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    dst_low.clone(),
                                ));
                            }
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                dst_low.clone(),
                                dst_high.clone(),
                            ));
                            instructions.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Sar,
                                AsmOperand::Imm(63),
                                dst_high,
                            ));
                        }
                    }
                    continue;
                }
                match src {
                    TackyVal::Constant(c) if dst_ty != AsmType::Byte => {
                        instructions.push(AsmInstr::Mov(
                            dst_ty,
                            AsmOperand::Imm(*c),
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                    _ => {
                        instructions.push(AsmInstr::Movsx(
                            src_ty,
                            dst_ty,
                            val_operand(src, &stack_slots, global_vars)?,
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                }
            }
            TackyInstr::ZeroExtend { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                let dst_ty = asm_type_for_val(dst, types)?;
                if dst_ty == AsmType::Octword {
                    let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                    let dst_low = low64_operand(&dst_op)?;
                    let dst_high = high64_operand(&dst_op)?;
                    if src_ty == AsmType::Quadword {
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            val_operand(src, &stack_slots, global_vars)?,
                            dst_low,
                        ));
                    } else {
                        instructions.push(AsmInstr::MovZeroExtend(
                            src_ty,
                            AsmType::Quadword,
                            val_operand(src, &stack_slots, global_vars)?,
                            dst_low,
                        ));
                    }
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Imm(0),
                        dst_high,
                    ));
                    continue;
                }
                match src {
                    TackyVal::Constant(c) if dst_ty != AsmType::Byte => {
                        instructions.push(AsmInstr::Mov(
                            dst_ty,
                            AsmOperand::Imm(*c),
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                    _ => {
                        instructions.push(AsmInstr::MovZeroExtend(
                            src_ty,
                            dst_ty,
                            val_operand(src, &stack_slots, global_vars)?,
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                }
            }
            TackyInstr::Truncate { src, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                if asm_type_for_val(src, types)? == AsmType::Octword {
                    instructions.push(AsmInstr::Mov(
                        ty,
                        low64_operand(&val_operand(src, &stack_slots, global_vars)?)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                    continue;
                }
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::IntToDouble { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                if matches!(src_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Movsx(
                        src_ty,
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Cvtsi2sd(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvtsi2sd(
                        src_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::IntToFloat { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                if matches!(src_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Movsx(
                        src_ty,
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Cvtsi2ss(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvtsi2ss(
                        src_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::UIntToDouble { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::AArch64UIntToDouble(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::UIntToFloat { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::AArch64UIntToFloat(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::DoubleToInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Cvttsd2si(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvttsd2si(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::FloatToInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Cvttss2si(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvttss2si(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::DoubleToUInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::AArch64DoubleToUInt(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::AArch64DoubleToUInt(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::FloatToUInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::AArch64FloatToUInt(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::AArch64FloatToUInt(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::FloatToDouble { src, dst } => {
                instructions.push(AsmInstr::AArch64FloatToDouble(
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::DoubleToFloat { src, dst } => {
                instructions.push(AsmInstr::AArch64DoubleToFloat(
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Unary { op, src, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                if matches!(op, TackyUnaryOp::LogicalNot) {
                    let src_ty = asm_type_for_val(src, types)?;
                    if src_ty == AsmType::Octword {
                        emit_i128_zero_cmp(&mut instructions, src, &stack_slots, global_vars)?;
                        instructions.push(AsmInstr::SetCC(CondCode::E, dst_op));
                        continue;
                    }
                    instructions.push(AsmInstr::Cmp(
                        src_ty,
                        AsmOperand::Imm(0),
                        val_operand(src, &stack_slots, global_vars)?,
                    ));
                    instructions.push(AsmInstr::SetCC(CondCode::E, dst_op));
                    continue;
                }
                if ty == AsmType::LongDouble && matches!(op, TackyUnaryOp::Negate) {
                    emit_long_double_negate(
                        &mut instructions,
                        src,
                        dst,
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                let asm_op = match op {
                    TackyUnaryOp::Negate => AsmUnaryOp::Neg,
                    TackyUnaryOp::Complement => AsmUnaryOp::Not,
                    TackyUnaryOp::LogicalNot => {
                        return Err(
                            "logical-not should be lowered before AArch64 unary emission"
                                .to_string(),
                        )
                    }
                };
                if ty == AsmType::Octword {
                    emit_i128_unary(
                        &mut instructions,
                        src,
                        dst_op,
                        asm_op,
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    dst_op.clone(),
                ));
                instructions.push(AsmInstr::Unary(ty, asm_op, dst_op));
            }
            TackyInstr::Jump(label) => {
                instructions.push(AsmInstr::Jmp(label.clone()));
            }
            TackyInstr::NonlocalJump(label) => {
                if frame_size > 0 {
                    instructions.push(AsmInstr::DeallocateStack(i64::from(frame_size)));
                }
                if large_stack_size > 0 {
                    instructions.push(AsmInstr::AArch64DeallocateLargeStack(large_stack_size));
                }
                instructions.push(AsmInstr::NonlocalJmp(label.clone()));
            }
            TackyInstr::JumpIndirect(target) => {
                instructions.push(AsmInstr::JmpIndirect(val_operand(
                    target,
                    &stack_slots,
                    global_vars,
                )?));
            }
            TackyInstr::JumpIfZero(val, label) => {
                let ty = asm_type_for_val(val, types)?;
                if ty == AsmType::Octword {
                    emit_i128_zero_cmp(&mut instructions, val, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
                    continue;
                }
                instructions.push(AsmInstr::Cmp(
                    ty,
                    AsmOperand::Imm(0),
                    val_operand(val, &stack_slots, global_vars)?,
                ));
                instructions.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            }
            TackyInstr::JumpIfNotZero(val, label) => {
                let ty = asm_type_for_val(val, types)?;
                if ty == AsmType::Octword {
                    emit_i128_zero_cmp(&mut instructions, val, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
                    continue;
                }
                instructions.push(AsmInstr::Cmp(
                    ty,
                    AsmOperand::Imm(0),
                    val_operand(val, &stack_slots, global_vars)?,
                ));
                instructions.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            }
            TackyInstr::Label(label) => {
                instructions.push(AsmInstr::Label(label.clone()));
            }
            TackyInstr::LoadLabelAddress(label, dst) => {
                instructions.push(AsmInstr::LoadLabelAddress(
                    label.clone(),
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::FrameAddress { dst } => {
                instructions.push(AsmInstr::Lea(
                    AsmOperand::Stack(i64::from(frame_size)),
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::BuiltinSetjmp {
                buf,
                dst,
                label,
                end_label,
            } => {
                instructions.push(AsmInstr::BuiltinSetjmp {
                    buf: val_operand(buf, &stack_slots, global_vars)?,
                    dst: val_operand(dst, &stack_slots, global_vars)?,
                    label: label.clone(),
                    end_label: end_label.clone(),
                });
            }
            TackyInstr::BuiltinLongjmp { buf, value } => {
                instructions.push(AsmInstr::BuiltinLongjmp {
                    buf: val_operand(buf, &stack_slots, global_vars)?,
                    value: val_operand(value, &stack_slots, global_vars)?,
                });
            }
            TackyInstr::GetAddress { src, dst } => {
                let TackyVal::Var(name) = src else {
                    return Err(
                        "AArch64 backend can only take addresses of local variables".to_string()
                    );
                };
                if let Some((base_slot, _)) = large_local_offsets.get(name) {
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Stack(i64::from(*base_slot)),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Lea(
                        stack_or_data_operand(name, 0, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::Load { src_ptr, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(src_ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::LoadIndirect(
                    dst_ty,
                    Reg::R11,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Store { src, dst_ptr } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(dst_ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::StoreIndirect(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    Reg::R11,
                ));
            }
            TackyInstr::CopyToOffset {
                src,
                dst_name,
                offset,
            } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::Mov(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    stack_or_data_operand(dst_name, *offset as i32, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::CopyFromOffset {
                src_name,
                offset,
                dst,
            } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                instructions.push(AsmInstr::Mov(
                    dst_ty,
                    stack_or_data_operand(src_name, *offset as i32, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::CopyStruct { src_name, dst_name } => {
                if src_name == dst_name {
                    continue;
                }
                let size = aggregate_size(dst_name, array_sizes, var_struct_tags, struct_defs)
                    .or_else(|| aggregate_size(src_name, array_sizes, var_struct_tags, struct_defs))
                    .ok_or_else(|| {
                        format!(
                            "AArch64 backend missing aggregate size for struct copy {} -> {}",
                            src_name, dst_name
                        )
                    })?;
                copy_bytes(
                    &mut instructions,
                    src_name,
                    dst_name,
                    size,
                    &stack_slots,
                    global_vars,
                )?;
            }
            TackyInstr::VaStart { dst } => {
                instructions.push(AsmInstr::Lea(
                    AsmOperand::Stack(i64::from(va_start_stack_offset)),
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AddPtr {
                ptr,
                index,
                scale,
                dst,
            } => {
                instructions.push(AsmInstr::AArch64AddPtr(
                    val_operand(ptr, &stack_slots, global_vars)?,
                    val_operand(index, &stack_slots, global_vars)?,
                    *scale,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
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
                let mut arg_groups: HashMap<usize, (usize, &[bool])> =
                    HashMap::with_capacity(struct_arg_groups.len());
                for (start, count, is_sse) in struct_arg_groups {
                    arg_groups.insert(*start, (*count, is_sse.as_slice()));
                }
                let mut memory_blocks: HashMap<usize, (usize, usize)> =
                    HashMap::with_capacity(memory_arg_blocks.len());
                for (index, size, align) in memory_arg_blocks {
                    memory_blocks.insert(*index, (*size, *align));
                }
                enum StackArg<'a> {
                    Scalar(AsmType, &'a TackyVal),
                    MemoryBlock {
                        src_ptr: &'a TackyVal,
                        size: usize,
                        align: usize,
                    },
                }

                impl StackArg<'_> {
                    fn slot_count(&self) -> usize {
                        match self {
                            StackArg::Scalar(AsmType::Octword | AsmType::LongDouble, _) => 2,
                            StackArg::Scalar(_, _) => 1,
                            StackArg::MemoryBlock { size, .. } => {
                                size.div_ceil(STACK_SLOT_SIZE as usize)
                            }
                        }
                    }

                    fn slot_alignment(&self) -> usize {
                        match self {
                            StackArg::Scalar(AsmType::Octword | AsmType::LongDouble, _) => 2,
                            StackArg::MemoryBlock { align, .. } if *align >= 16 => 2,
                            _ => 1,
                        }
                    }
                }

                let mut gp_arg_count = 0usize;
                let mut fp_arg_count = 0usize;
                let mut stack_args = Vec::with_capacity(args.len());
                let mut arg_index = 0usize;
                while arg_index < args.len() {
                    if let Some((size, align)) = memory_blocks.get(&arg_index).copied() {
                        stack_args.push(StackArg::MemoryBlock {
                            src_ptr: &args[arg_index],
                            size,
                            align,
                        });
                        arg_index += 1;
                        continue;
                    }
                    if let Some((count, is_sse)) = arg_groups.get(&arg_index) {
                        let force_stack_for_darwin_vararg = *variadic
                            && target.os == TargetOs::MacOs
                            && arg_index >= *fixed_flat_arg_count;
                        let needs = group_register_needs(is_sse);
                        let fits_registers = gp_arg_count + needs.gp <= ARG_REGS.len()
                            && fp_arg_count + needs.fp <= FP_ARG_REGS.len();
                        for (group_offset, is_fp) in is_sse.iter().copied().enumerate().take(*count)
                        {
                            let arg = &args[arg_index + group_offset];
                            let ty = asm_type_for_val(arg, types)?;
                            if fits_registers && !force_stack_for_darwin_vararg {
                                if is_fp {
                                    instructions.push(AsmInstr::Mov(
                                        ty,
                                        val_operand(arg, &stack_slots, global_vars)?,
                                        AsmOperand::Xmm(FP_ARG_REGS[fp_arg_count]),
                                    ));
                                    fp_arg_count += 1;
                                } else {
                                    instructions.push(AsmInstr::Mov(
                                        ty,
                                        val_operand(arg, &stack_slots, global_vars)?,
                                        AsmOperand::Reg(ARG_REGS[gp_arg_count]),
                                    ));
                                    gp_arg_count += 1;
                                }
                            } else {
                                stack_args.push(StackArg::Scalar(ty, arg));
                            }
                        }
                        arg_index += *count;
                        continue;
                    }

                    let arg = &args[arg_index];
                    let ty = asm_type_for_val(arg, types)?;
                    let force_stack_for_darwin_vararg = *variadic
                        && target.os == TargetOs::MacOs
                        && arg_index >= *fixed_flat_arg_count;
                    if stack_arg_indices.contains(&arg_index) || force_stack_for_darwin_vararg {
                        stack_args.push(StackArg::Scalar(ty, arg));
                    } else if ty == AsmType::Octword && gp_arg_count + 1 < ARG_REGS.len() {
                        let (src_low, src_high) =
                            i128_part_operands(arg, &stack_slots, global_vars)?;
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            src_low,
                            AsmOperand::Reg(ARG_REGS[gp_arg_count]),
                        ));
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            src_high,
                            AsmOperand::Reg(ARG_REGS[gp_arg_count + 1]),
                        ));
                        gp_arg_count += 2;
                    } else if ty == AsmType::Octword {
                        // A wide integer needs two consecutive argument
                        // registers. If only one remains, pass both limbs on
                        // the stack instead of treating it as a scalar.
                        stack_args.push(StackArg::Scalar(ty, arg));
                    } else if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
                        if fp_arg_count < FP_ARG_REGS.len() {
                            instructions.push(AsmInstr::Mov(
                                ty,
                                val_operand(arg, &stack_slots, global_vars)?,
                                AsmOperand::Xmm(FP_ARG_REGS[fp_arg_count]),
                            ));
                            fp_arg_count += 1;
                        } else {
                            stack_args.push(StackArg::Scalar(ty, arg));
                        }
                    } else if gp_arg_count < ARG_REGS.len() {
                        instructions.push(AsmInstr::Mov(
                            ty,
                            val_operand(arg, &stack_slots, global_vars)?,
                            AsmOperand::Reg(ARG_REGS[gp_arg_count]),
                        ));
                        gp_arg_count += 1;
                    } else {
                        stack_args.push(StackArg::Scalar(ty, arg));
                    }
                    arg_index += 1;
                }

                let stack_arg_count = stack_args.iter().fold(0usize, |index, arg| {
                    index.next_multiple_of(arg.slot_alignment()) + arg.slot_count()
                });
                let outgoing_bytes = outgoing_stack_size(stack_arg_count);
                if outgoing_bytes > 0 {
                    instructions.push(AsmInstr::AllocateStack(i64::from(outgoing_bytes)));
                    let mut stack_index = 0usize;
                    for arg in &stack_args {
                        stack_index = stack_index.next_multiple_of(arg.slot_alignment());
                        match arg {
                            StackArg::Scalar(AsmType::Octword, val) => {
                                let (src_low, src_high) =
                                    i128_part_operands(val, &stack_slots, global_vars)?;
                                // The outgoing argument area moves SP, so load
                                // both source limbs through the rebasing-aware
                                // instruction before storing them.  Ordinary
                                // stack arguments do this in
                                // AArch64StoreOutgoingArg; the split wide value
                                // needs it for each limb as well.
                                instructions.push(AsmInstr::AArch64LoadAdjusted(
                                    AsmType::Quadword,
                                    src_low,
                                    Reg::R10,
                                    outgoing_bytes,
                                ));
                                instructions.push(AsmInstr::AArch64LoadAdjusted(
                                    AsmType::Quadword,
                                    src_high,
                                    Reg::R13,
                                    outgoing_bytes,
                                ));
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    AsmType::Quadword,
                                    AsmOperand::Reg(Reg::R10),
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                ));
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    AsmType::Quadword,
                                    AsmOperand::Reg(Reg::R13),
                                    stack_arg_offset(0, stack_index + 1),
                                    outgoing_bytes,
                                ));
                                stack_index += arg.slot_count();
                            }
                            StackArg::Scalar(AsmType::LongDouble, val) => {
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    AsmType::LongDouble,
                                    val_operand(val, &stack_slots, global_vars)?,
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                ));
                                stack_index += arg.slot_count();
                            }
                            StackArg::Scalar(ty, val) => {
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    *ty,
                                    val_operand(val, &stack_slots, global_vars)?,
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                ));
                                stack_index += arg.slot_count();
                            }
                            StackArg::MemoryBlock { src_ptr, size, .. } => {
                                emit_copy_pointer_to_outgoing_arg(
                                    &mut instructions,
                                    val_operand(src_ptr, &stack_slots, global_vars)?,
                                    *size,
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                );
                                stack_index += arg.slot_count();
                            }
                        }
                    }
                }
                if *indirect {
                    let callee =
                        val_operand(&TackyVal::Var(name.clone()), &stack_slots, global_vars)?;
                    if outgoing_bytes > 0 {
                        instructions.push(AsmInstr::AArch64LoadAdjusted(
                            AsmType::Quadword,
                            callee,
                            Reg::R10,
                            outgoing_bytes,
                        ));
                    } else {
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            callee,
                            AsmOperand::Reg(Reg::R10),
                        ));
                    }
                }
                instructions.push(AsmInstr::Call(
                    name.clone(),
                    args.len(),
                    0,
                    *indirect,
                    false,
                ));
                if outgoing_bytes > 0 {
                    instructions.push(AsmInstr::DeallocateStack(i64::from(outgoing_bytes)));
                }
                if *hidden_return {
                    continue;
                }
                if is_returned_local(dst, next_instr, global_vars)
                    && matches!(function.return_type, CType::Int128 | CType::UInt128)
                    && asm_type_for_val(dst, types)? == AsmType::Octword
                {
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
                if is_returned_local(dst, next_instr, global_vars)
                    && matches!(function.return_type, CType::LongDouble)
                    && asm_type_for_val(dst, types)? == AsmType::LongDouble
                {
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
                if is_returned_local(dst, next_instr, global_vars)
                    && scalar_computed_return_type(function.return_type, dst, types)?.is_some()
                {
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    body_iter.next();
                    continue;
                }
                if val_ctype(dst, types) == Some(CType::Struct) {
                    let returns_via_memory =
                        struct_size_for_val(dst, array_sizes, var_struct_tags, struct_defs)
                            .is_some_and(|size| size > 16);
                    if returns_via_memory {
                        continue;
                    }
                    let classes = struct_classes_for_val(dst, var_struct_tags, struct_defs)
                        .ok_or_else(|| {
                            "AArch64 backend missing struct class for call return".to_string()
                        })?;
                    if is_returned_local(dst, next_instr, global_vars)
                        && matches!(function.return_type, CType::Struct)
                        && struct_classes_return_in_registers(&classes)
                    {
                        emit_epilogue(
                            &mut instructions,
                            frame_size,
                            large_stack_size,
                            link_register_offset,
                        );
                        body_iter.next();
                        continue;
                    }
                    move_return_regs_to_struct(
                        &mut instructions,
                        dst,
                        &classes,
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                let dst_ty = asm_type_for_val(dst, types)?;
                if dst_ty == AsmType::Octword {
                    let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::AX),
                        low64_operand(&dst_op)?,
                    ));
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::DI),
                        high64_operand(&dst_op)?,
                    ));
                    continue;
                }
                let ret_src = if matches!(
                    dst_ty,
                    AsmType::Float | AsmType::Double | AsmType::LongDouble
                ) {
                    AsmOperand::Xmm(XmmReg::XMM0)
                } else {
                    AsmOperand::Reg(Reg::AX)
                };
                instructions.push(AsmInstr::Mov(
                    dst_ty,
                    ret_src,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Binary {
                op,
                left,
                right,
                dst,
            } => {
                let ty = asm_type_for_val(dst, types)?;
                if let Some((branch_cc, label)) =
                    fused_comparison_branch(op, left, right, dst, next_instr, types)
                {
                    let left_cmp_ty = asm_type_for_val(left, types)?;
                    let right_cmp_ty = asm_type_for_val(right, types)?;
                    let cmp_ty = match (left_cmp_ty, right_cmp_ty) {
                        (AsmType::Double, _) | (_, AsmType::Double) => AsmType::Double,
                        (AsmType::Float, _) | (_, AsmType::Float) => AsmType::Float,
                        (AsmType::Quadword, _) | (_, AsmType::Quadword) => AsmType::Quadword,
                        (AsmType::Longword, _) | (_, AsmType::Longword) => AsmType::Longword,
                        (AsmType::Word, _) | (_, AsmType::Word) => AsmType::Word,
                        _ => AsmType::Byte,
                    };
                    let right_op = if matches!(cmp_ty, AsmType::Float | AsmType::Double) {
                        floating_return_operand(cmp_ty, right, &stack_slots, global_vars)?
                    } else {
                        val_operand(right, &stack_slots, global_vars)?
                    };
                    let left_op = if matches!(cmp_ty, AsmType::Float | AsmType::Double) {
                        floating_return_operand(cmp_ty, left, &stack_slots, global_vars)?
                    } else {
                        val_operand(left, &stack_slots, global_vars)?
                    };
                    instructions.push(AsmInstr::Cmp(cmp_ty, right_op, left_op));
                    instructions.push(AsmInstr::JmpCC(branch_cc, label));
                    body_iter.next();
                    continue;
                }
                let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                if asm_type_for_val(left, types)? == AsmType::LongDouble
                    || asm_type_for_val(right, types)? == AsmType::LongDouble
                {
                    if let Some(comparison) = long_double_comparison_helper(op) {
                        emit_long_double_comparison(
                            &mut instructions,
                            comparison,
                            left,
                            right,
                            dst_op.clone(),
                            &stack_slots,
                            global_vars,
                        )?;
                        continue;
                    }
                    let Some(helper) = long_double_helper(op) else {
                        return Err(format!(
                            "AArch64 backend does not support long double binary op yet: {:?}",
                            op
                        ));
                    };
                    emit_long_double_helper_call(
                        &mut instructions,
                        helper,
                        left,
                        right,
                        dst_op.clone(),
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                if matches!(op, TackyBinaryOp::Equal | TackyBinaryOp::NotEqual)
                    && (asm_type_for_val(left, types)? == AsmType::Octword
                        || asm_type_for_val(right, types)? == AsmType::Octword)
                {
                    emit_i128_eq_cmp(
                        &mut instructions,
                        left,
                        right,
                        op,
                        dst_op,
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                if matches!(
                    op,
                    TackyBinaryOp::LessThan
                        | TackyBinaryOp::LessEqual
                        | TackyBinaryOp::GreaterThan
                        | TackyBinaryOp::GreaterEqual
                ) && (asm_type_for_val(left, types)? == AsmType::Octword
                    || asm_type_for_val(right, types)? == AsmType::Octword)
                {
                    if is_unsigned_comparison_val(left, types) {
                        emit_i128_unsigned_cmp(
                            &mut instructions,
                            left,
                            right,
                            op,
                            dst_op,
                            &i128_ctx,
                        )?;
                    } else {
                        emit_i128_signed_cmp(
                            &mut instructions,
                            left,
                            right,
                            op,
                            dst_op,
                            &i128_ctx,
                        )?;
                    }
                    continue;
                }
                if ty == AsmType::Octword {
                    if emit_i128_basic_binary(
                        &mut instructions,
                        op,
                        left,
                        right,
                        dst_op.clone(),
                        &stack_slots,
                        global_vars,
                    )? {
                        continue;
                    }
                    if emit_i128_helper_binary(
                        &mut instructions,
                        op,
                        left,
                        right,
                        dst_op.clone(),
                        &i128_ctx,
                    )? {
                        continue;
                    }
                    if emit_i128_shift(
                        &mut instructions,
                        op,
                        left,
                        right,
                        dst_op.clone(),
                        &i128_ctx,
                    )? {
                        continue;
                    }
                    return Err(format!(
                        "AArch64 backend does not support 128-bit binary op yet: {:?}",
                        op
                    ));
                }
                if let Some(cc) = convert_comparison_op(op, is_unsigned_comparison_val(left, types))
                {
                    let left_cmp_ty = asm_type_for_val(left, types)?;
                    let right_cmp_ty = asm_type_for_val(right, types)?;
                    let cmp_ty = match (left_cmp_ty, right_cmp_ty) {
                        (AsmType::Double, _) | (_, AsmType::Double) => AsmType::Double,
                        (AsmType::Float, _) | (_, AsmType::Float) => AsmType::Float,
                        (AsmType::Quadword, _) | (_, AsmType::Quadword) => AsmType::Quadword,
                        (AsmType::Longword, _) | (_, AsmType::Longword) => AsmType::Longword,
                        (AsmType::Word, _) | (_, AsmType::Word) => AsmType::Word,
                        _ => AsmType::Byte,
                    };
                    let right_op = if matches!(cmp_ty, AsmType::Float | AsmType::Double) {
                        floating_return_operand(cmp_ty, right, &stack_slots, global_vars)?
                    } else {
                        val_operand(right, &stack_slots, global_vars)?
                    };
                    let left_op = if matches!(cmp_ty, AsmType::Float | AsmType::Double) {
                        floating_return_operand(cmp_ty, left, &stack_slots, global_vars)?
                    } else {
                        val_operand(left, &stack_slots, global_vars)?
                    };
                    instructions.push(AsmInstr::Cmp(cmp_ty, right_op, left_op));
                    instructions.push(AsmInstr::SetCC(cc, dst_op));
                    continue;
                }
                let asm_op = match op {
                    TackyBinaryOp::Div if matches!(ty, AsmType::Float | AsmType::Double) => {
                        AsmBinaryOp::DivDouble
                    }
                    TackyBinaryOp::Div => {
                        if is_unsigned_val(dst, types) {
                            AsmBinaryOp::UDiv
                        } else {
                            AsmBinaryOp::SDiv
                        }
                    }
                    TackyBinaryOp::ShiftRight => {
                        if is_unsigned_val(left, types) {
                            AsmBinaryOp::Shr
                        } else {
                            AsmBinaryOp::Sar
                        }
                    }
                    TackyBinaryOp::Mod => {
                        instructions.push(AsmInstr::AArch64Rem(
                            ty,
                            is_unsigned_val(dst, types),
                            val_operand(left, &stack_slots, global_vars)?,
                            val_operand(right, &stack_slots, global_vars)?,
                            dst_op,
                        ));
                        continue;
                    }
                    _ => convert_binary_op(op)?,
                };
                let left_op = if matches!(ty, AsmType::Float | AsmType::Double) {
                    floating_return_operand(ty, left, &stack_slots, global_vars)?
                } else {
                    val_operand(left, &stack_slots, global_vars)?
                };
                let right_op = if matches!(ty, AsmType::Float | AsmType::Double) {
                    floating_return_operand(ty, right, &stack_slots, global_vars)?
                } else {
                    val_operand(right, &stack_slots, global_vars)?
                };
                instructions.push(AsmInstr::Mov(ty, left_op, dst_op.clone()));
                instructions.push(AsmInstr::Binary(ty, asm_op, right_op, dst_op));
            }
        }
    }

    rewrite_long_double_immediates(&mut instructions, long_double_consts);

    let mut asm_function = AsmFunction {
        name: function.name.clone(),
        global: function.global,
        instructions,
    };
    let ret_regs = compute_ret_regs(function, types, var_struct_tags, struct_defs);
    let allocation = crate::backend::x86_64::regalloc::allocate_registers_with_profile(
        &mut asm_function,
        &aliased,
        types,
        array_sizes,
        &ret_regs,
        &crate::backend::x86_64::regalloc::AARCH64_REG_ALLOC_PROFILE,
        no_coalescing,
    );
    debug_assert!(allocation.callee_saved.is_empty());
    replace_spilled_pseudos(&mut asm_function, &raw_stack_slots)?;

    Ok(asm_function)
}

pub fn gen(
    program: &TackyProgram,
    target: &Target,
    no_coalescing: bool,
) -> Result<AsmProgram, String> {
    let mut top_level = Vec::with_capacity(program.top_level.len());
    let mut long_double_consts = Vec::with_capacity(program.top_level.len());
    for item in &program.top_level {
        match item {
            TackyTopLevel::Function(function) => {
                let mut function = convert_function(
                    function,
                    target,
                    program,
                    &mut long_double_consts,
                    no_coalescing,
                )?;
                rewrite_tls_operands(&mut function, &program.thread_local_vars);
                top_level.push(AsmTopLevel::Function(function));
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
    for (name, value) in long_double_consts {
        top_level.push(AsmTopLevel::StaticConstant(AsmStaticConstant {
            name,
            alignment: 16,
            init: StaticInit::LongDoubleInit(value),
        }));
    }
    Ok(AsmProgram { top_level })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::x86_64::regalloc::RegId;
    use crate::{lex, parse, resolve, tacky};

    fn codegen_source(source: &str) -> Result<AsmProgram, String> {
        let tokens = lex::lex(source)?;
        let ast = parse::parse(tokens)?;
        let resolved = resolve::resolve(ast).map_err(|err| err.render())?.program;
        let tacky = tacky::generate(resolved)?;
        gen(&tacky, &Target::aarch64_linux(), false)
    }

    #[test]
    fn aggregate_copy_uses_halfword_for_two_byte_tail() -> Result<(), String> {
        let mut instructions = Vec::new();
        let stack_slots = HashMap::from([("src".to_string(), 0), ("dst".to_string(), 32)]);
        copy_bytes(
            &mut instructions,
            "src",
            "dst",
            10,
            &stack_slots,
            &HashSet::new(),
        )?;

        assert_eq!(instructions.len(), 4);
        assert!(matches!(
            instructions[2],
            AsmInstr::Mov(
                AsmType::Word,
                AsmOperand::Stack(8),
                AsmOperand::Reg(Reg::R10)
            )
        ));
        assert!(matches!(
            instructions[3],
            AsmInstr::Mov(
                AsmType::Word,
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Stack(40)
            )
        ));
        Ok(())
    }

    fn function<'a>(program: &'a AsmProgram, name: &str) -> Result<&'a AsmFunction, String> {
        program
            .top_level
            .iter()
            .find_map(|item| match item {
                AsmTopLevel::Function(function) if function.name == name => Some(function),
                _ => None,
            })
            .ok_or_else(|| format!("function `{name}` should exist"))
    }

    fn struct_member(name: &str, member_type: CType, offset: usize) -> StructMember {
        StructMember {
            name: name.to_string(),
            member_type,
            member_full_type: FullType::Scalar(member_type),
            flexible_array: false,
            offset,
            size: member_type.size() as usize,
            bit_width: None,
            bit_offset: 0,
            reverse_storage_order: false,
        }
    }

    fn struct_function_returning() -> TackyFunction {
        TackyFunction {
            name: "f".to_string(),
            return_type: CType::Struct,
            params: Vec::new(),
            global: false,
            body: vec![TackyInstr::Return(TackyVal::Var("ret".to_string()))],
            stack_params: HashSet::new(),
            memory_param_blocks: Vec::new(),
            struct_param_groups: Vec::new(),
        }
    }

    fn struct_ret_regs(def: StructDef) -> Vec<RegId> {
        let mut types = IndexMap::new();
        types.insert("ret".to_string(), CType::Struct);
        let mut var_struct_tags = HashMap::new();
        var_struct_tags.insert("ret".to_string(), def.tag.clone());
        let mut struct_defs = IndexMap::new();
        let tag = def.tag.clone();
        struct_defs.insert(tag.clone(), def);

        compute_ret_regs(
            &struct_function_returning(),
            &types,
            &var_struct_tags,
            &struct_defs,
        )
    }

    #[test]
    fn aarch64_register_allocation_env_value_parser_is_explicit() {
        for value in ["1", "true", "on", "yes"] {
            assert!(aarch64_register_allocation_enabled_value(value));
        }

        for value in ["", "0", "false", "off", "no", "TRUE", "yes "] {
            assert!(!aarch64_register_allocation_enabled_value(value));
        }
    }

    #[test]
    fn aarch64_ret_regs_include_both_integer_halves_for_small_struct_returns() {
        let regs = struct_ret_regs(StructDef {
            tag: "pair".to_string(),
            members: vec![
                struct_member("a", CType::Long, 0),
                struct_member("b", CType::Long, 8),
            ],
            size: 16,
            alignment: 8,
            is_union: false,
        });

        assert_eq!(regs, vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DI)]);
    }

    #[test]
    fn aarch64_integer_struct_returns_use_x1_for_second_eightbyte() -> Result<(), String> {
        let program = codegen_source(
            "struct pair { long a; long b; };\n\
             struct pair make_pair(long a, long b) { return (struct pair){a, b}; }\n\
             long read_second(void) { struct pair p = make_pair(1, 2); return p.b; }\n",
        )?;
        let make_pair = function(&program, "make_pair")?;
        assert!(make_pair.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Quadword, _, AsmOperand::Reg(Reg::DI))
            )
        }));
        assert!(!make_pair.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Quadword, _, AsmOperand::Reg(Reg::DX))
            )
        }));

        let read_second = function(&program, "read_second")?;
        assert!(read_second.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Quadword, AsmOperand::Reg(Reg::DI), _)
            )
        }));
        assert!(!read_second.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Quadword, AsmOperand::Reg(Reg::DX), _)
            )
        }));
        Ok(())
    }

    #[test]
    fn aarch64_ret_regs_include_fp_registers_for_small_struct_returns() {
        let regs = struct_ret_regs(StructDef {
            tag: "box2".to_string(),
            members: vec![
                struct_member("a", CType::Double, 0),
                struct_member("b", CType::Double, 8),
            ],
            size: 16,
            alignment: 8,
            is_union: false,
        });

        assert_eq!(
            regs,
            vec![RegId::Xmm(XmmReg::XMM0), RegId::Xmm(XmmReg::XMM1)]
        );
    }

    #[test]
    fn aarch64_ret_regs_include_mixed_registers_for_small_struct_returns() {
        let regs = struct_ret_regs(StructDef {
            tag: "mixed".to_string(),
            members: vec![
                struct_member("a", CType::Long, 0),
                struct_member("b", CType::Double, 8),
            ],
            size: 16,
            alignment: 8,
            is_union: false,
        });

        assert_eq!(regs, vec![RegId::Gp(Reg::AX), RegId::Xmm(XmmReg::XMM0)]);
    }

    #[test]
    fn grouped_integer_struct_argument_spills_as_a_unit() -> Result<(), String> {
        let program = codegen_source(
            "struct pair { long a; long b; };\n\
             int f(long a, long b, long c, long d, long e, long f, long g, struct pair p) { return p.b; }\n\
             int main(void) { struct pair p = {8, 9}; return f(1, 2, 3, 4, 5, 6, 7, p); }\n",
        )?;
        let main = function(&program, "main")?;
        let outgoing_args = main
            .instructions
            .iter()
            .filter(|instr| matches!(instr, AsmInstr::AArch64StoreOutgoingArg(..)))
            .count();
        assert_eq!(outgoing_args, 2);
        assert!(!main.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(_, _, AsmOperand::Reg(Reg::R12))
                    | AsmInstr::Mov(_, AsmOperand::Reg(Reg::R12), _)
            )
        }));
        Ok(())
    }

    #[test]
    fn wide_stack_arguments_rebase_both_source_limbs() -> Result<(), String> {
        let program = codegen_source(
            "unsigned __int128 take(long a, long b, long c, long d, long e, long f, long g, unsigned __int128 value) { return value; }\n\
             int main(void) { unsigned __int128 value = ((unsigned __int128)1 << 100) | 7; return take(1, 2, 3, 4, 5, 6, 7, value) != value; }\n",
        )?;
        let main = function(&program, "main")?;
        let adjusted_loads: Vec<_> = main
            .instructions
            .iter()
            .filter_map(|instr| match instr {
                AsmInstr::AArch64LoadAdjusted(
                    AsmType::Quadword,
                    AsmOperand::Stack(_),
                    reg,
                    rebase,
                ) => Some((*reg, *rebase)),
                _ => None,
            })
            .collect();

        assert!(adjusted_loads.contains(&(Reg::R10, 16)), "{main:#?}");
        assert!(adjusted_loads.contains(&(Reg::R13, 16)), "{main:#?}");
        Ok(())
    }

    #[test]
    fn i128_shift_counter_moves_preserve_scalar_and_wide_widths() -> Result<(), String> {
        let program = codegen_source(
            "unsigned __int128 scalar_count(unsigned __int128 value, unsigned count) { return value << count; }\n\
             unsigned __int128 wide_count(unsigned __int128 value, unsigned __int128 count) { return value << count; }\n",
        )?;

        let scalar_count = function(&program, "scalar_count")?;
        assert!(scalar_count.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Longword, _, AsmOperand::Reg(Reg::R12))
            )
        }));

        let wide_count = function(&program, "wide_count")?;
        assert!(wide_count.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Quadword, _, AsmOperand::Reg(Reg::R12))
            )
        }));
        Ok(())
    }

    #[test]
    fn constant_i128_cross_limb_shifts_use_extract() -> Result<(), String> {
        let program = codegen_source(
            "unsigned __int128 left(unsigned __int128 x) { return x << 13; }\n\
             unsigned __int128 right(unsigned __int128 x) { return x >> 13; }\n",
        )?;

        let left = function(&program, "left")?;
        assert!(left
            .instructions
            .iter()
            .any(|instr| { matches!(instr, AsmInstr::AArch64Extr(_, _, 51, _)) }));
        let right = function(&program, "right")?;
        assert!(right
            .instructions
            .iter()
            .any(|instr| { matches!(instr, AsmInstr::AArch64Extr(_, _, 13, _)) }));
        Ok(())
    }

    #[test]
    fn i128_multiply_uses_inline_full_width_product() -> Result<(), String> {
        let program = codegen_source(
            "unsigned __int128 multiply(unsigned __int128 a, unsigned __int128 b) {\n\
             return a * b;\n\
             }\n",
        )?;
        let multiply = function(&program, "multiply")?;
        assert!(multiply
            .instructions
            .iter()
            .any(|instr| matches!(instr, AsmInstr::AArch64Umulh(_, _, _))));
        assert!(!multiply
            .instructions
            .iter()
            .any(|instr| matches!(instr, AsmInstr::Call(name, ..) if name == "__multi3")));
        assert!(!multiply
            .instructions
            .iter()
            .any(|instr| matches!(instr, AsmInstr::AArch64SaveLink(_))));
        Ok(())
    }

    #[test]
    fn spilled_pseudo_self_moves_are_removed_during_rewrite() {
        let mut function = AsmFunction {
            name: "f".to_string(),
            global: false,
            instructions: vec![AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Pseudo("tmp".to_string()),
                AsmOperand::Pseudo("tmp".to_string()),
            )],
        };
        let mut stack_slots = HashMap::new();
        stack_slots.insert("tmp".to_string(), 16);

        replace_spilled_pseudos(&mut function, &stack_slots).unwrap();

        assert!(function.instructions.is_empty());
    }

    #[test]
    fn function_designator_argument_uses_label_address() -> Result<(), String> {
        let program = codegen_source(
            "int f(int (*fp)(int), int x) { return fp(x); }\n\
             int inc(int x) { return x + 1; }\n\
             int main(void) { return f(inc, 9); }\n",
        )?;
        let main = function(&program, "main")?;
        assert!(main.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Lea(AsmOperand::Data(name), _) if name == "inc"
            )
        }));
        Ok(())
    }

    #[test]
    fn comparison_branch_avoids_boolean_stack_temporary() -> Result<(), String> {
        let program = codegen_source(
            "int f(int a, int b, int c, int d) { if (a <= b) return c; return d; }",
        )?;
        let function = function(&program, "f")?;

        assert!(function.instructions.windows(2).any(|pair| {
            matches!(
                pair,
                [
                    AsmInstr::Cmp(AsmType::Longword, _, _),
                    AsmInstr::JmpCC(CondCode::G, _)
                ]
            )
        }));
        assert!(!function
            .instructions
            .iter()
            .any(|instr| matches!(instr, AsmInstr::SetCC(_, _))));
        assert!(function
            .instructions
            .iter()
            .any(|instr| matches!(instr, AsmInstr::AllocateStack(16))));
        Ok(())
    }

    #[test]
    fn huge_local_array_uses_saved_large_stack_base() -> Result<(), String> {
        let program = codegen_source(
            "extern int sink(char *);\n\
             int f(void) { char s[0x10000000000UL]; return sink(s); }\n",
        )?;
        let function = function(&program, "f")?;
        assert!(function.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::AArch64AllocateLargeStack(bytes) if *bytes == 0x10000000000
            )
        }));
        let base_slot = function.instructions.iter().find_map(|instr| {
            if let AsmInstr::AArch64StoreLargeLocalBase { dst_offset, .. } = instr {
                Some(*dst_offset)
            } else {
                None
            }
        });
        let Some(base_slot) = base_slot else {
            return Err("expected saved large local base".to_string());
        };
        assert!(function.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Quadword, AsmOperand::Stack(slot), _)
                    if *slot == i64::from(base_slot)
            )
        }));
        assert!(!function.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Lea(AsmOperand::Stack(slot), _)
                    if *slot == i64::from(base_slot)
            )
        }));
        Ok(())
    }
}
