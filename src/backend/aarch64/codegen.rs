use crate::types::*;
use std::collections::{HashMap, HashSet};

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
const STACK_ALIGNMENT: i32 = 16;
const STACK_SLOT_SIZE: i32 = 8;
const MIN_STACK_SLOT_SIZE: i32 = 4;
const LINK_REGISTER_SAVE_SIZE: i32 = 16;

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

fn stack_arg_offset(base: i32, index: usize) -> i32 {
    base + (index as i32 * STACK_SLOT_SIZE)
}

fn outgoing_stack_size(stack_arg_count: usize) -> i32 {
    align_to(stack_arg_count as i32 * STACK_SLOT_SIZE, STACK_ALIGNMENT)
}

fn emit_epilogue(
    instructions: &mut Vec<AsmInstr>,
    frame_size: i32,
    link_register_offset: Option<i32>,
) {
    if let Some(offset) = link_register_offset {
        instructions.push(AsmInstr::AArch64RestoreLink(offset));
    }
    if frame_size > 0 {
        instructions.push(AsmInstr::DeallocateStack(frame_size));
    }
    instructions.push(AsmInstr::Ret);
}

fn asm_type_for_val(val: &TackyVal, types: &HashMap<String, CType>) -> Result<AsmType, String> {
    match val {
        TackyVal::Constant(c) => {
            if *c > i32::MAX as i64 || *c < i32::MIN as i64 {
                Ok(AsmType::Quadword)
            } else {
                Ok(AsmType::Longword)
            }
        }
        TackyVal::DoubleConstant(_) => Ok(AsmType::Double),
        TackyVal::Var(name) => match types.get(name).copied().unwrap_or(CType::Int) {
            CType::Char | CType::SChar | CType::UChar | CType::Bool => Ok(AsmType::Byte),
            CType::Short | CType::UShort => Ok(AsmType::Word),
            CType::Int | CType::UInt => Ok(AsmType::Longword),
            CType::Long | CType::ULong | CType::Pointer => Ok(AsmType::Quadword),
            CType::Float => Ok(AsmType::Float),
            CType::Double => Ok(AsmType::Double),
            CType::Void => Ok(AsmType::Longword),
            CType::Struct => Err("AArch64 backend does not support struct values yet".to_string()),
        },
    }
}

fn val_operand(
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<AsmOperand, String> {
    match val {
        TackyVal::Constant(c) => Ok(AsmOperand::Imm(*c)),
        TackyVal::DoubleConstant(d) => Ok(AsmOperand::Imm(d.to_bits() as i64)),
        TackyVal::Var(name) => {
            if global_vars.contains(name) {
                Ok(AsmOperand::Data(name.clone()))
            } else {
                stack_slots
                    .get(name)
                    .copied()
                    .map(AsmOperand::Stack)
                    .ok_or_else(|| format!("AArch64 backend missing stack slot for {}", name))
            }
        }
    }
}

fn collect_var(val: &TackyVal, vars: &mut Vec<String>, global_vars: &HashSet<String>) {
    if let TackyVal::Var(name) = val {
        if !global_vars.contains(name) && !vars.contains(name) {
            vars.push(name.clone());
        }
    }
}

fn val_ctype(val: &TackyVal, types: &HashMap<String, CType>) -> Option<CType> {
    match val {
        TackyVal::Var(name) => types.get(name).copied(),
        TackyVal::Constant(_) => Some(CType::Int),
        TackyVal::DoubleConstant(_) => Some(CType::Double),
    }
}

fn group_register_needs(is_sse: &[bool]) -> (usize, usize) {
    let fp_needed = is_sse.iter().filter(|&&is_fp| is_fp).count();
    (is_sse.len() - fp_needed, fp_needed)
}

fn data_operand_with_offset(name: &str, offset: i32) -> AsmOperand {
    if offset == 0 {
        AsmOperand::Data(name.to_string())
    } else {
        AsmOperand::Data(format!("{}+{}", name, offset))
    }
}

fn tls_name_offset(name: &str, tls_vars: &HashSet<String>) -> Option<(String, i32)> {
    if tls_vars.contains(name) {
        return Some((name.to_string(), 0));
    }
    let (base, offset) = name.rsplit_once('+')?;
    if tls_vars.contains(base) {
        let offset = offset.parse().ok()?;
        Some((base.to_string(), offset))
    } else {
        None
    }
}

fn rewrite_tls_operand(op: &mut AsmOperand, tls_vars: &HashSet<String>) {
    if let AsmOperand::Data(name) = op {
        if let Some((base, offset)) = tls_name_offset(name, tls_vars) {
            *op = AsmOperand::TlsData(base, offset);
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
            .map(|base| AsmOperand::Stack(base + offset))
            .ok_or_else(|| format!("AArch64 backend missing stack slot for {}", name))
    }
}

fn aggregate_size(
    name: &str,
    array_sizes: &HashMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
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
    struct_defs: &HashMap<String, StructDef>,
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
    array_sizes: &HashMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Option<usize> {
    let TackyVal::Var(name) = val else {
        return None;
    };
    aggregate_size(name, array_sizes, var_struct_tags, struct_defs)
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
    while offset + 8 <= size {
        let byte_offset = i32::try_from(offset)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", src_name))?;
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            stack_or_data_operand(src_name, byte_offset, stack_slots, global_vars)?,
            AsmOperand::Reg(Reg::R10),
        ));
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Reg(Reg::R10),
            stack_or_data_operand(dst_name, byte_offset, stack_slots, global_vars)?,
        ));
        offset += 8;
    }
    while offset + 4 <= size {
        let byte_offset = i32::try_from(offset)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", src_name))?;
        instructions.push(AsmInstr::Mov(
            AsmType::Longword,
            stack_or_data_operand(src_name, byte_offset, stack_slots, global_vars)?,
            AsmOperand::Reg(Reg::R10),
        ));
        instructions.push(AsmInstr::Mov(
            AsmType::Longword,
            AsmOperand::Reg(Reg::R10),
            stack_or_data_operand(dst_name, byte_offset, stack_slots, global_vars)?,
        ));
        offset += 4;
    }
    while offset < size {
        let byte_offset = i32::try_from(offset)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", src_name))?;
        instructions.push(AsmInstr::Mov(
            AsmType::Byte,
            stack_or_data_operand(src_name, byte_offset, stack_slots, global_vars)?,
            AsmOperand::Reg(Reg::R10),
        ));
        instructions.push(AsmInstr::Mov(
            AsmType::Byte,
            AsmOperand::Reg(Reg::R10),
            stack_or_data_operand(dst_name, byte_offset, stack_slots, global_vars)?,
        ));
        offset += 1;
    }
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
    let gp_regs = [Reg::AX, Reg::DX];
    let fp_regs = [XmmReg::XMM0, XmmReg::XMM1];
    let mut gp_idx = 0usize;
    let mut fp_idx = 0usize;
    for (chunk_idx, class) in classes.iter().enumerate() {
        let offset = i32::try_from(chunk_idx * 8)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", name))?;
        match class {
            ParamClass::Integer if gp_idx < gp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                    AsmOperand::Reg(gp_regs[gp_idx]),
                ));
                gp_idx += 1;
            }
            ParamClass::Sse if fp_idx < fp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                    AsmOperand::Xmm(fp_regs[fp_idx]),
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
    let gp_regs = [Reg::AX, Reg::DX];
    let fp_regs = [XmmReg::XMM0, XmmReg::XMM1];
    let mut gp_idx = 0usize;
    let mut fp_idx = 0usize;
    for (chunk_idx, class) in classes.iter().enumerate() {
        let offset = i32::try_from(chunk_idx * 8)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", name))?;
        match class {
            ParamClass::Integer if gp_idx < gp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(gp_regs[gp_idx]),
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                ));
                gp_idx += 1;
            }
            ParamClass::Sse if fp_idx < fp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    AsmOperand::Xmm(fp_regs[fp_idx]),
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
    types: &HashMap<String, CType>,
    array_sizes: &HashMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
    global_vars: &HashSet<String>,
    alignments: &HashMap<String, usize>,
) -> Result<(HashMap<String, i32>, i32), String> {
    let mut vars = Vec::new();
    for param in &function.params {
        if !vars.contains(param) {
            vars.push(param.clone());
        }
    }

    for instr in &function.body {
        match instr {
            TackyInstr::Return(val) => collect_var(val, &mut vars, global_vars),
            TackyInstr::Unary { src, dst, .. } => {
                collect_var(src, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::Binary {
                left, right, dst, ..
            } => {
                collect_var(left, &mut vars, global_vars);
                collect_var(right, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
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
                collect_var(src, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::JumpIfZero(val, _) | TackyInstr::JumpIfNotZero(val, _) => {
                collect_var(val, &mut vars, global_vars);
            }
            TackyInstr::FunCall { args, dst, .. } => {
                for arg in args {
                    collect_var(arg, &mut vars, global_vars);
                }
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::GetAddress { src, dst } => {
                collect_var(src, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::Load { src_ptr, dst } => {
                collect_var(src_ptr, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::Store { src, dst_ptr } => {
                collect_var(src, &mut vars, global_vars);
                collect_var(dst_ptr, &mut vars, global_vars);
            }
            TackyInstr::AtomicFetch { ptr, arg, dst, .. } => {
                collect_var(ptr, &mut vars, global_vars);
                collect_var(arg, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::AtomicExchange { ptr, value, dst } => {
                collect_var(ptr, &mut vars, global_vars);
                collect_var(value, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::AtomicCompareExchange {
                ptr,
                expected,
                desired,
                dst,
            } => {
                collect_var(ptr, &mut vars, global_vars);
                collect_var(expected, &mut vars, global_vars);
                collect_var(desired, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                dst,
                ..
            } => {
                collect_var(ptr, &mut vars, global_vars);
                collect_var(expected, &mut vars, global_vars);
                collect_var(desired, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::AddPtr {
                ptr, index, dst, ..
            } => {
                collect_var(ptr, &mut vars, global_vars);
                collect_var(index, &mut vars, global_vars);
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::CopyToOffset { src, dst_name, .. } => {
                collect_var(src, &mut vars, global_vars);
                if !global_vars.contains(dst_name) && !vars.contains(dst_name) {
                    vars.push(dst_name.clone());
                }
            }
            TackyInstr::CopyFromOffset { src_name, dst, .. } => {
                if !global_vars.contains(src_name) && !vars.contains(src_name) {
                    vars.push(src_name.clone());
                }
                collect_var(dst, &mut vars, global_vars);
            }
            TackyInstr::CopyStruct { src_name, dst_name } => {
                if !global_vars.contains(src_name) && !vars.contains(src_name) {
                    vars.push(src_name.clone());
                }
                if !global_vars.contains(dst_name) && !vars.contains(dst_name) {
                    vars.push(dst_name.clone());
                }
            }
            TackyInstr::Jump(_)
            | TackyInstr::Label(_)
            | TackyInstr::Nop
            | TackyInstr::Unreachable
            | TackyInstr::AtomicFence => {}
        }
    }

    let mut slots = HashMap::new();
    let mut offset = 0i32;
    for var in vars {
        let size =
            if let Some(size) = aggregate_size(&var, array_sizes, var_struct_tags, struct_defs) {
                i32::try_from(size)
                    .map_err(|_| format!("AArch64 backend local array too large: {}", var))?
            } else {
                match types.get(&var).copied().unwrap_or(CType::Int) {
                    CType::Char | CType::SChar | CType::UChar | CType::Bool => 1,
                    CType::Short | CType::UShort => 2,
                    CType::Int | CType::UInt => 4,
                    CType::Float => 4,
                    CType::Long | CType::ULong | CType::Pointer => 8,
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

    let stack_size = align_to(offset, STACK_ALIGNMENT);
    Ok((slots, stack_size))
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

fn is_unsigned_val(val: &TackyVal, types: &HashMap<String, CType>) -> bool {
    match val {
        TackyVal::Var(name) => types
            .get(name)
            .copied()
            .is_some_and(|ctype| !ctype.is_signed()),
        _ => false,
    }
}

fn is_unsigned_comparison_val(val: &TackyVal, types: &HashMap<String, CType>) -> bool {
    match val {
        TackyVal::Var(name) => types
            .get(name)
            .copied()
            .is_some_and(|ctype| ctype != CType::Double && !ctype.is_signed()),
        _ => false,
    }
}

fn convert_function(
    function: &TackyFunction,
    target: &Target,
    program: &TackyProgram,
) -> Result<AsmFunction, String> {
    let types = &program.symbol_types;
    let array_sizes = &program.array_sizes;
    let var_struct_tags = &program.var_struct_tags;
    let struct_defs = &program.struct_defs;
    let global_vars = &program.global_vars;
    let alignments = &program.symbol_alignments;
    let (stack_slots, stack_size) = collect_stack_slots(
        function,
        types,
        array_sizes,
        var_struct_tags,
        struct_defs,
        global_vars,
        alignments,
    )?;
    let saves_link_register = function
        .body
        .iter()
        .any(|instr| matches!(instr, TackyInstr::FunCall { .. }));
    let frame_size = compute_frame_size(stack_size, saves_link_register);
    let link_register_offset =
        saves_link_register.then(|| compute_link_register_offset(frame_size));
    let mut instructions = Vec::new();
    if frame_size > 0 {
        instructions.push(AsmInstr::AllocateStack(frame_size));
    }
    if let Some(offset) = link_register_offset {
        instructions.push(AsmInstr::AArch64SaveLink(offset));
    }
    let param_groups: HashMap<usize, (usize, Vec<bool>)> = function
        .struct_param_groups
        .iter()
        .map(|(start, count, is_sse)| (*start, (*count, is_sse.clone())))
        .collect();
    let mut gp_param_count = 0usize;
    let mut fp_param_count = 0usize;
    let mut stack_param_count = 0usize;
    let mut param_index = 0usize;
    while param_index < function.params.len() {
        if let Some((count, is_sse)) = param_groups.get(&param_index) {
            let (gp_needed, fp_needed) = group_register_needs(is_sse);
            let fits_registers = gp_param_count + gp_needed <= ARG_REGS.len()
                && fp_param_count + fp_needed <= FP_ARG_REGS.len();
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
                    let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
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
        let src = if function.stack_params.contains(param) {
            let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
            stack_param_count += 1;
            src
        } else if matches!(ty, AsmType::Float | AsmType::Double) {
            if fp_param_count < FP_ARG_REGS.len() {
                let src = AsmOperand::Xmm(FP_ARG_REGS[fp_param_count]);
                fp_param_count += 1;
                src
            } else {
                let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
                stack_param_count += 1;
                src
            }
        } else if gp_param_count < ARG_REGS.len() {
            let src = AsmOperand::Reg(ARG_REGS[gp_param_count]);
            gp_param_count += 1;
            src
        } else {
            let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
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

    for instr in &function.body {
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
                    emit_epilogue(&mut instructions, frame_size, link_register_offset);
                    continue;
                }
                let ty = asm_type_for_val(val, types)?;
                let ret_dst = if matches!(ty, AsmType::Float | AsmType::Double) {
                    AsmOperand::Xmm(XmmReg::XMM0)
                } else {
                    AsmOperand::Reg(Reg::AX)
                };
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(val, &stack_slots, global_vars)?,
                    ret_dst,
                ));
                emit_epilogue(&mut instructions, frame_size, link_register_offset);
            }
            TackyInstr::Copy { src, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::SignExtend { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                let dst_ty = asm_type_for_val(dst, types)?;
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
                    instructions.push(AsmInstr::Cmp(
                        src_ty,
                        AsmOperand::Imm(0),
                        val_operand(src, &stack_slots, global_vars)?,
                    ));
                    instructions.push(AsmInstr::SetCC(CondCode::E, dst_op));
                    continue;
                }
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    dst_op.clone(),
                ));
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
                instructions.push(AsmInstr::Unary(ty, asm_op, dst_op));
            }
            TackyInstr::Jump(label) => {
                instructions.push(AsmInstr::Jmp(label.clone()));
            }
            TackyInstr::JumpIfZero(val, label) => {
                let ty = asm_type_for_val(val, types)?;
                instructions.push(AsmInstr::Cmp(
                    ty,
                    AsmOperand::Imm(0),
                    val_operand(val, &stack_slots, global_vars)?,
                ));
                instructions.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            }
            TackyInstr::JumpIfNotZero(val, label) => {
                let ty = asm_type_for_val(val, types)?;
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
            TackyInstr::GetAddress { src, dst } => {
                let TackyVal::Var(name) = src else {
                    return Err(
                        "AArch64 backend can only take addresses of local variables".to_string()
                    );
                };
                instructions.push(AsmInstr::Lea(
                    stack_or_data_operand(name, 0, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
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
                struct_arg_groups,
                variadic,
                fixed_flat_arg_count,
                indirect,
            } => {
                let arg_groups: HashMap<usize, (usize, Vec<bool>)> = struct_arg_groups
                    .iter()
                    .map(|(start, count, is_sse)| (*start, (*count, is_sse.clone())))
                    .collect();
                let mut gp_arg_count = 0usize;
                let mut fp_arg_count = 0usize;
                let mut stack_args = Vec::new();
                let mut arg_index = 0usize;
                while arg_index < args.len() {
                    if let Some((count, is_sse)) = arg_groups.get(&arg_index) {
                        let force_stack_for_darwin_vararg = *variadic
                            && target.os == TargetOs::MacOs
                            && arg_index >= *fixed_flat_arg_count;
                        let (gp_needed, fp_needed) = group_register_needs(is_sse);
                        let fits_registers = gp_arg_count + gp_needed <= ARG_REGS.len()
                            && fp_arg_count + fp_needed <= FP_ARG_REGS.len();
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
                                stack_args.push((ty, arg));
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
                        stack_args.push((ty, arg));
                    } else if matches!(ty, AsmType::Float | AsmType::Double) {
                        if fp_arg_count < FP_ARG_REGS.len() {
                            instructions.push(AsmInstr::Mov(
                                ty,
                                val_operand(arg, &stack_slots, global_vars)?,
                                AsmOperand::Xmm(FP_ARG_REGS[fp_arg_count]),
                            ));
                            fp_arg_count += 1;
                        } else {
                            stack_args.push((ty, arg));
                        }
                    } else if gp_arg_count < ARG_REGS.len() {
                        instructions.push(AsmInstr::Mov(
                            ty,
                            val_operand(arg, &stack_slots, global_vars)?,
                            AsmOperand::Reg(ARG_REGS[gp_arg_count]),
                        ));
                        gp_arg_count += 1;
                    } else {
                        stack_args.push((ty, arg));
                    }
                    arg_index += 1;
                }

                let stack_arg_count = stack_args.len();
                let outgoing_bytes = outgoing_stack_size(stack_arg_count);
                if outgoing_bytes > 0 {
                    instructions.push(AsmInstr::AllocateStack(outgoing_bytes));
                    for (stack_index, (ty, arg)) in stack_args.iter().enumerate() {
                        instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                            *ty,
                            val_operand(arg, &stack_slots, global_vars)?,
                            stack_arg_offset(0, stack_index),
                            outgoing_bytes,
                        ));
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
                instructions.push(AsmInstr::Call(name.clone(), args.len(), 0, *indirect));
                if outgoing_bytes > 0 {
                    instructions.push(AsmInstr::DeallocateStack(outgoing_bytes));
                }
                if val_ctype(dst, types) == Some(CType::Struct) {
                    if struct_size_for_val(dst, array_sizes, var_struct_tags, struct_defs)
                        .is_some_and(|size| size > 16)
                    {
                        continue;
                    }
                    let classes = struct_classes_for_val(dst, var_struct_tags, struct_defs)
                        .ok_or_else(|| {
                            "AArch64 backend missing struct class for call return".to_string()
                        })?;
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
                let ret_src = if matches!(dst_ty, AsmType::Float | AsmType::Double) {
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
                let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                if let Some(cc) = convert_comparison_op(op, is_unsigned_comparison_val(left, types))
                {
                    let cmp_ty = match (
                        asm_type_for_val(left, types)?,
                        asm_type_for_val(right, types)?,
                    ) {
                        (AsmType::Double, _) | (_, AsmType::Double) => AsmType::Double,
                        (AsmType::Float, _) | (_, AsmType::Float) => AsmType::Float,
                        (AsmType::Quadword, _) | (_, AsmType::Quadword) => AsmType::Quadword,
                        _ => AsmType::Longword,
                    };
                    instructions.push(AsmInstr::Cmp(
                        cmp_ty,
                        val_operand(right, &stack_slots, global_vars)?,
                        val_operand(left, &stack_slots, global_vars)?,
                    ));
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
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(left, &stack_slots, global_vars)?,
                    dst_op.clone(),
                ));
                instructions.push(AsmInstr::Binary(
                    ty,
                    asm_op,
                    val_operand(right, &stack_slots, global_vars)?,
                    dst_op,
                ));
            }
        }
    }

    Ok(AsmFunction {
        name: function.name.clone(),
        global: function.global,
        instructions,
    })
}

pub fn gen(program: &TackyProgram, target: &Target) -> Result<AsmProgram, String> {
    let mut top_level = Vec::new();
    for item in &program.top_level {
        match item {
            TackyTopLevel::Function(function) => {
                let mut function = convert_function(function, target, program)?;
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
        }
    }
    Ok(AsmProgram { top_level })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lex, parse, resolve, tacky};

    fn codegen_source(source: &str) -> Result<AsmProgram, String> {
        let tokens = lex::lex(source)?;
        let ast = parse::parse(tokens)?;
        let resolved = resolve::resolve(ast).map_err(|err| err.render())?.program;
        let tacky = tacky::generate(resolved)?;
        gen(&tacky, &Target::aarch64_linux())
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
}
