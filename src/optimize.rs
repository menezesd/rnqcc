use crate::types::*;

#[derive(Debug, Clone)]
pub struct OptimizationFlags {
    pub fold_constants: bool,
    pub eliminate_unreachable_code: bool,
    pub propagate_copies: bool,
    pub eliminate_dead_stores: bool,
}

impl OptimizationFlags {
    pub fn none() -> Self {
        OptimizationFlags {
            fold_constants: false,
            eliminate_unreachable_code: false,
            propagate_copies: false,
            eliminate_dead_stores: false,
        }
    }

    pub fn any_enabled(&self) -> bool {
        self.fold_constants || self.eliminate_unreachable_code
            || self.propagate_copies || self.eliminate_dead_stores
    }
}

pub fn optimize_program(program: &mut TackyProgram, flags: &OptimizationFlags) {
    if !flags.any_enabled() {
        return;
    }
    for top in &mut program.top_level {
        if let TackyTopLevel::Function(func) = top {
            optimize_function(func, flags);
        }
    }
}

fn optimize_function(func: &mut TackyFunction, flags: &OptimizationFlags) {
    if func.body.is_empty() {
        return;
    }

    loop {
        let before = func.body.clone();

        // Constant folding operates on flat instruction list
        if flags.fold_constants {
            func.body = constant_folding(std::mem::take(&mut func.body));
        }

        // The other three optimizations use control-flow graphs
        // For now, just do unreachable code elimination on the flat list
        if flags.eliminate_unreachable_code {
            func.body = unreachable_code_elimination(std::mem::take(&mut func.body));
        }

        // TODO: copy propagation (requires CFG + data-flow analysis)
        // TODO: dead store elimination (requires CFG + data-flow analysis)

        if func.body == before || func.body.is_empty() {
            break;
        }
    }
}

// ============================================================
// Constant Folding
// ============================================================

fn constant_folding(instructions: Vec<TackyInstr>) -> Vec<TackyInstr> {
    instructions.into_iter().map(|instr| fold_instruction(instr)).collect()
}

fn fold_instruction(instr: TackyInstr) -> TackyInstr {
    match instr {
        TackyInstr::Binary { op, left, right, dst } => {
            if let (Some(l), Some(r)) = (const_val(&left), const_val(&right)) {
                if let Some(result) = eval_binary(&op, l, r) {
                    return TackyInstr::Copy { src: TackyVal::Constant(result), dst };
                }
            }
            TackyInstr::Binary { op, left, right, dst }
        }
        TackyInstr::Unary { op, src, dst } => {
            if let Some(v) = const_val(&src) {
                if let Some(result) = eval_unary(&op, v) {
                    return TackyInstr::Copy { src: TackyVal::Constant(result), dst };
                }
            }
            TackyInstr::Unary { op, src, dst }
        }
        TackyInstr::JumpIfZero(val, target) => {
            if let Some(v) = const_val(&val) {
                if v == 0 {
                    return TackyInstr::Jump(target);
                } else {
                    // Condition is always non-zero, never jumps — remove instruction
                    return TackyInstr::Nop;
                }
            }
            TackyInstr::JumpIfZero(val, target)
        }
        TackyInstr::JumpIfNotZero(val, target) => {
            if let Some(v) = const_val(&val) {
                if v != 0 {
                    return TackyInstr::Jump(target);
                } else {
                    return TackyInstr::Nop;
                }
            }
            TackyInstr::JumpIfNotZero(val, target)
        }
        // Type conversions with constant source
        TackyInstr::Truncate { src, dst } => {
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy { src: TackyVal::Constant(v as i32 as i64), dst };
            }
            TackyInstr::Truncate { src, dst }
        }
        TackyInstr::SignExtend { src, dst } => {
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy { src: TackyVal::Constant(v as i32 as i64), dst };
            }
            TackyInstr::SignExtend { src, dst }
        }
        TackyInstr::ZeroExtend { src, dst } => {
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy { src: TackyVal::Constant(v as u32 as i64), dst };
            }
            TackyInstr::ZeroExtend { src, dst }
        }
        TackyInstr::DoubleToInt { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                return TackyInstr::Copy { src: TackyVal::Constant(d as i64), dst };
            }
            TackyInstr::DoubleToInt { src, dst }
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                return TackyInstr::Copy { src: TackyVal::Constant(d as u64 as i64), dst };
            }
            TackyInstr::DoubleToUInt { src, dst }
        }
        TackyInstr::IntToDouble { src, dst } => {
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy { src: TackyVal::DoubleConstant(v as f64), dst };
            }
            TackyInstr::IntToDouble { src, dst }
        }
        TackyInstr::UIntToDouble { src, dst } => {
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy { src: TackyVal::DoubleConstant(v as u64 as f64), dst };
            }
            TackyInstr::UIntToDouble { src, dst }
        }
        other => other,
    }
}

fn const_val(val: &TackyVal) -> Option<i64> {
    match val {
        TackyVal::Constant(c) => Some(*c),
        _ => None,
    }
}

fn eval_binary(op: &TackyBinaryOp, l: i64, r: i64) -> Option<i64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => {
            if r == 0 { None } else { Some(l.wrapping_div(r)) }
        }
        TackyBinaryOp::Mod => {
            if r == 0 { None } else { Some(l.wrapping_rem(r)) }
        }
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseOr => Some(l | r),
        TackyBinaryOp::BitwiseXor => Some(l ^ r),
        TackyBinaryOp::ShiftLeft => Some(l.wrapping_shl(r as u32)),
        TackyBinaryOp::ShiftRight => Some(l.wrapping_shr(r as u32)),
        TackyBinaryOp::Equal => Some(if l == r { 1 } else { 0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1 } else { 0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1 } else { 0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1 } else { 0 }),
        TackyBinaryOp::LessEqual => Some(if l <= r { 1 } else { 0 }),
        TackyBinaryOp::GreaterEqual => Some(if l >= r { 1 } else { 0 }),
        _ => None,
    }
}

fn eval_unary(op: &TackyUnaryOp, v: i64) -> Option<i64> {
    match op {
        TackyUnaryOp::Negate => Some(v.wrapping_neg()),
        TackyUnaryOp::Complement => Some(!v),
        TackyUnaryOp::LogicalNot => Some(if v == 0 { 1 } else { 0 }),
        _ => None,
    }
}

// ============================================================
// Unreachable Code Elimination
// ============================================================

fn unreachable_code_elimination(instructions: Vec<TackyInstr>) -> Vec<TackyInstr> {
    // First pass: collect all jump targets (labels that are actually jumped to)
    let mut jump_targets = std::collections::HashSet::new();
    for instr in &instructions {
        match instr {
            TackyInstr::Jump(target) => { jump_targets.insert(target.clone()); }
            TackyInstr::JumpIfZero(_, target) => { jump_targets.insert(target.clone()); }
            TackyInstr::JumpIfNotZero(_, target) => { jump_targets.insert(target.clone()); }
            _ => {}
        }
    }

    // Second pass: remove unreachable code
    let mut result = Vec::new();
    let mut reachable = true;

    for instr in instructions {
        match &instr {
            TackyInstr::Label(label) => {
                if jump_targets.contains(label) {
                    reachable = true;
                }
                if reachable {
                    result.push(instr);
                }
            }
            TackyInstr::Jump(_) | TackyInstr::Return(_) => {
                if reachable {
                    result.push(instr);
                }
                reachable = false;
            }
            TackyInstr::Nop => {
                // Skip nops (produced by constant folding)
            }
            _ => {
                if reachable {
                    result.push(instr);
                }
            }
        }
    }

    // Third pass: remove labels that are no longer jumped to
    let mut final_jump_targets = std::collections::HashSet::new();
    for instr in &result {
        match instr {
            TackyInstr::Jump(target) => { final_jump_targets.insert(target.clone()); }
            TackyInstr::JumpIfZero(_, target) => { final_jump_targets.insert(target.clone()); }
            TackyInstr::JumpIfNotZero(_, target) => { final_jump_targets.insert(target.clone()); }
            _ => {}
        }
    }
    result.retain(|instr| {
        if let TackyInstr::Label(label) = instr {
            final_jump_targets.contains(label)
        } else {
            true
        }
    });

    // Remove jumps to the immediately following label
    let mut cleaned = Vec::new();
    for i in 0..result.len() {
        if let TackyInstr::Jump(ref target) = result[i] {
            // Check if next non-nop instruction is this label
            if i + 1 < result.len() {
                if let TackyInstr::Label(ref label) = result[i + 1] {
                    if target == label {
                        continue; // Skip this jump
                    }
                }
            }
        }
        cleaned.push(result[i].clone());
    }

    cleaned
}
