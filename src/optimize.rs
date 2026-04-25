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
    // Track which variables hold known constant values
    let mut const_map: std::collections::HashMap<String, TackyVal> = std::collections::HashMap::new();

    instructions.into_iter().map(|instr| {
        // At labels, clear the const_map (control flow can merge)
        if let TackyInstr::Label(_) = &instr {
            const_map.clear();
            return instr;
        }
        // At jumps, function calls — be conservative
        if matches!(&instr, TackyInstr::FunCall { .. }) {
            // Function calls may modify globals — clear non-temp variables
            // For simplicity, clear everything
            const_map.clear();
        }

        // First, try to resolve operands using known constants
        let instr = resolve_constants(&instr, &const_map);

        // Then fold the instruction
        let folded = fold_instruction(instr);

        // Track constants: if Copy(Constant(x), Var(v)), record v = x
        match &folded {
            TackyInstr::Copy { src: TackyVal::Constant(c), dst: TackyVal::Var(name) } => {
                const_map.insert(name.clone(), TackyVal::Constant(*c));
            }
            TackyInstr::Copy { src: TackyVal::DoubleConstant(d), dst: TackyVal::Var(name) } => {
                const_map.insert(name.clone(), TackyVal::DoubleConstant(*d));
            }
            // Any instruction that writes to a variable invalidates our knowledge
            _ => {
                if let Some(dst_name) = get_dst_name(&folded) {
                    const_map.remove(&dst_name);
                }
            }
        }

        folded
    }).collect()
}

fn resolve_constants(instr: &TackyInstr, const_map: &std::collections::HashMap<String, TackyVal>) -> TackyInstr {
    // Replace variable operands with their known constant values
    match instr {
        TackyInstr::Binary { op, left, right, dst } => {
            let new_left = resolve_val(left, const_map);
            let new_right = resolve_val(right, const_map);
            TackyInstr::Binary { op: op.clone(), left: new_left, right: new_right, dst: dst.clone() }
        }
        TackyInstr::Unary { op, src, dst } => {
            let new_src = resolve_val(src, const_map);
            TackyInstr::Unary { op: op.clone(), src: new_src, dst: dst.clone() }
        }
        TackyInstr::JumpIfZero(val, target) => {
            TackyInstr::JumpIfZero(resolve_val(val, const_map), target.clone())
        }
        TackyInstr::JumpIfNotZero(val, target) => {
            TackyInstr::JumpIfNotZero(resolve_val(val, const_map), target.clone())
        }
        TackyInstr::Return(val) => {
            TackyInstr::Return(resolve_val(val, const_map))
        }
        TackyInstr::Truncate { src, dst } => {
            TackyInstr::Truncate { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        TackyInstr::SignExtend { src, dst } => {
            TackyInstr::SignExtend { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        TackyInstr::ZeroExtend { src, dst } => {
            TackyInstr::ZeroExtend { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        TackyInstr::DoubleToInt { src, dst } => {
            TackyInstr::DoubleToInt { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            TackyInstr::DoubleToUInt { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        TackyInstr::IntToDouble { src, dst } => {
            TackyInstr::IntToDouble { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        TackyInstr::UIntToDouble { src, dst } => {
            TackyInstr::UIntToDouble { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        TackyInstr::Copy { src, dst } => {
            TackyInstr::Copy { src: resolve_val(src, const_map), dst: dst.clone() }
        }
        other => other.clone(),
    }
}

fn resolve_val(val: &TackyVal, const_map: &std::collections::HashMap<String, TackyVal>) -> TackyVal {
    if let TackyVal::Var(name) = val {
        if let Some(cval) = const_map.get(name) {
            return cval.clone();
        }
    }
    val.clone()
}

fn get_dst_name(instr: &TackyInstr) -> Option<String> {
    match instr {
        TackyInstr::Binary { dst: TackyVal::Var(n), .. } |
        TackyInstr::Unary { dst: TackyVal::Var(n), .. } |
        TackyInstr::Copy { dst: TackyVal::Var(n), .. } |
        TackyInstr::Truncate { dst: TackyVal::Var(n), .. } |
        TackyInstr::SignExtend { dst: TackyVal::Var(n), .. } |
        TackyInstr::ZeroExtend { dst: TackyVal::Var(n), .. } |
        TackyInstr::DoubleToInt { dst: TackyVal::Var(n), .. } |
        TackyInstr::DoubleToUInt { dst: TackyVal::Var(n), .. } |
        TackyInstr::IntToDouble { dst: TackyVal::Var(n), .. } |
        TackyInstr::UIntToDouble { dst: TackyVal::Var(n), .. } |
        TackyInstr::Load { dst: TackyVal::Var(n), .. } |
        TackyInstr::CopyFromOffset { dst: TackyVal::Var(n), .. } => Some(n.clone()),
        TackyInstr::FunCall { dst: TackyVal::Var(n), .. } => Some(n.clone()),
        _ => None,
    }
}

fn fold_instruction(instr: TackyInstr) -> TackyInstr {
    match instr {
        TackyInstr::Binary { op, left, right, dst } => {
            // Try integer constant folding
            if let (Some(l), Some(r)) = (const_val(&left), const_val(&right)) {
                if let Some(result) = eval_binary(&op, l, r) {
                    return TackyInstr::Copy { src: TackyVal::Constant(result), dst };
                }
            }
            // Try double constant folding
            if let (Some(l), Some(r)) = (const_double(&left), const_double(&right)) {
                if let Some(result) = eval_binary_double(&op, l, r) {
                    return TackyInstr::Copy { src: TackyVal::DoubleConstant(result), dst };
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
            if let Some(d) = const_double(&src) {
                match op {
                    TackyUnaryOp::Negate => {
                        return TackyInstr::Copy { src: TackyVal::DoubleConstant(-d), dst };
                    }
                    _ => {}
                }
            }
            TackyInstr::Unary { op, src, dst }
        }
        TackyInstr::JumpIfZero(val, target) => {
            if let Some(v) = const_val(&val) {
                if v == 0 { return TackyInstr::Jump(target); }
                else { return TackyInstr::Nop; }
            }
            if let Some(d) = const_double(&val) {
                if d == 0.0 { return TackyInstr::Jump(target); }
                else { return TackyInstr::Nop; }
            }
            TackyInstr::JumpIfZero(val, target)
        }
        TackyInstr::JumpIfNotZero(val, target) => {
            if let Some(v) = const_val(&val) {
                if v != 0 { return TackyInstr::Jump(target); }
                else { return TackyInstr::Nop; }
            }
            if let Some(d) = const_double(&val) {
                if d != 0.0 { return TackyInstr::Jump(target); }
                else { return TackyInstr::Nop; }
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
        TackyBinaryOp::Div => Some(l / r), // IEEE 754 handles div-by-zero
        TackyBinaryOp::Equal => Some(if l == r { 1.0 } else { 0.0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1.0 } else { 0.0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1.0 } else { 0.0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1.0 } else { 0.0 }),
        TackyBinaryOp::LessEqual => Some(if l <= r { 1.0 } else { 0.0 }),
        TackyBinaryOp::GreaterEqual => Some(if l >= r { 1.0 } else { 0.0 }),
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
