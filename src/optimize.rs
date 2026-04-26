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
    let types = program.symbol_types.clone();
    // Collect static/global variable names
    let mut static_var_names = program.global_vars.clone();
    for top in &program.top_level {
        if let TackyTopLevel::StaticVar(sv) = top {
            static_var_names.insert(sv.name.clone());
        }
    }
    for top in &mut program.top_level {
        if let TackyTopLevel::Function(func) = top {
            optimize_function(func, flags, &types, &static_var_names);
        }
    }
}

fn optimize_function(func: &mut TackyFunction, flags: &OptimizationFlags, types: &std::collections::HashMap<String, CType>, static_var_names: &std::collections::HashSet<String>) {
    if func.body.is_empty() {
        return;
    }

    let static_vars = static_var_names.clone();

    loop {
        let before = func.body.clone();

        let aliased_vars = crate::cfg::find_aliased_vars(&func.body, &static_vars);

        if flags.fold_constants {
            func.body = constant_folding(std::mem::take(&mut func.body), types);
        }

        if flags.eliminate_unreachable_code {
            func.body = unreachable_code_elimination(std::mem::take(&mut func.body));
        }

        if flags.propagate_copies || flags.eliminate_dead_stores {
            let mut cfg = crate::cfg::CFG::build(std::mem::take(&mut func.body));

            if flags.propagate_copies {
                crate::cfg::copy_propagation(&mut cfg, &aliased_vars, types);
            }

            if flags.eliminate_dead_stores {
                crate::cfg::dead_store_elimination(&mut cfg, &aliased_vars, &static_vars);
            }

            func.body = cfg.to_instructions();
        }

        // Simple CSE for CopyFromOffset: replace duplicate reads with Copy
        if flags.propagate_copies {
            func.body = cse_copy_from_offset(std::mem::take(&mut func.body));
        }

        if func.body == before || func.body.is_empty() {
            break;
        }
    }
}

// ============================================================
// Simple CSE for CopyFromOffset
// ============================================================

fn cse_copy_from_offset(instructions: Vec<TackyInstr>) -> Vec<TackyInstr> {
    // Track (src_name, offset) → first output variable
    let mut seen: std::collections::HashMap<(String, i64), String> = std::collections::HashMap::new();
    instructions.into_iter().map(|instr| {
        match &instr {
            TackyInstr::CopyFromOffset { src_name, offset, dst } => {
                let key = (src_name.clone(), *offset);
                if let Some(prev_dst) = seen.get(&key) {
                    // Duplicate CopyFromOffset — replace with Copy from previous output
                    if let TackyVal::Var(d) = dst {
                        return TackyInstr::Copy {
                            src: TackyVal::Var(prev_dst.clone()),
                            dst: TackyVal::Var(d.clone()),
                        };
                    }
                }
                if let TackyVal::Var(d) = dst {
                    seen.insert(key, d.clone());
                }
            }
            // CopyToOffset/CopyStruct/Store/FunCall may modify the struct — invalidate
            TackyInstr::CopyToOffset { dst_name, .. } | TackyInstr::CopyStruct { dst_name, .. } => {
                seen.retain(|k, _| k.0 != *dst_name);
            }
            TackyInstr::Store { .. } | TackyInstr::FunCall { .. } => {
                seen.clear();
            }
            TackyInstr::Label(_) => {
                seen.clear();
            }
            _ => {}
        }
        instr
    }).collect()
}

// ============================================================
// Constant Folding
// ============================================================

fn constant_folding(instructions: Vec<TackyInstr>, types: &std::collections::HashMap<String, CType>) -> Vec<TackyInstr> {
    // Track which variables hold known constant values, along with their type
    let mut const_map: std::collections::HashMap<String, (TackyVal, CType)> = std::collections::HashMap::new();

    instructions.into_iter().map(|instr| {
        // At labels, clear the const_map (control flow can merge with different values)
        if let TackyInstr::Label(_) = &instr {
            const_map.clear();
            return instr;
        }
        // At function calls and stores — be conservative (may modify aliased vars)
        if matches!(&instr, TackyInstr::FunCall { .. } | TackyInstr::Store { .. }) {
            const_map.clear();
        }

        // Capture source operand types before resolution (for typed folding)
        let src_type_hint = match &instr {
            TackyInstr::Binary { left, right, .. } => {
                let lt = resolve_val_type(left, &const_map, types);
                let rt = resolve_val_type(right, &const_map, types);
                Some(CType::common(lt, rt))
            }
            _ => None,
        };

        // Resolve operands using known constants — but NOT for Copy/Store/CopyToOffset sources
        // (Copy sources: handled by CFG-based copy propagation)
        // (Store/CopyToOffset sources: constant replacement may lose type info)
        let instr = if matches!(&instr, TackyInstr::Copy { .. } | TackyInstr::Store { .. } | TackyInstr::CopyToOffset { .. }) {
            instr
        } else {
            resolve_constants(&instr, &const_map)
        };

        let folded = fold_instruction(instr, types, src_type_hint);

        // Track constants: Copy(Constant, Var) and Copy(Var, Var) where Var is known
        match &folded {
            TackyInstr::Copy { src: TackyVal::Constant(c), dst: TackyVal::Var(name) } => {
                let t = types.get(name).copied().unwrap_or(CType::Int);
                const_map.insert(name.clone(), (TackyVal::Constant(*c), t));
            }
            TackyInstr::Copy { src: TackyVal::DoubleConstant(d), dst: TackyVal::Var(name) } => {
                const_map.insert(name.clone(), (TackyVal::DoubleConstant(*d), CType::Double));
            }
            TackyInstr::Copy { src: TackyVal::Var(s), dst: TackyVal::Var(name) } => {
                // If source has a known constant, propagate it
                if let Some((cval, ct)) = const_map.get(s).cloned() {
                    const_map.insert(name.clone(), (cval, ct));
                } else {
                    const_map.remove(name);
                }
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

fn resolve_constants(instr: &TackyInstr, const_map: &std::collections::HashMap<String, (TackyVal, CType)>) -> TackyInstr {
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
        TackyInstr::AddPtr { ptr, index, scale, dst } => {
            TackyInstr::AddPtr {
                ptr: resolve_val(ptr, const_map),
                index: resolve_val(index, const_map),
                scale: *scale,
                dst: dst.clone(),
            }
        }
        TackyInstr::Store { src, dst_ptr } => {
            TackyInstr::Store {
                src: resolve_val(src, const_map),
                dst_ptr: resolve_val(dst_ptr, const_map),
            }
        }
        TackyInstr::Load { src_ptr, dst } => {
            TackyInstr::Load {
                src_ptr: resolve_val(src_ptr, const_map),
                dst: dst.clone(),
            }
        }
        TackyInstr::FunCall { name, args, dst, stack_arg_indices, struct_arg_groups, indirect } => {
            let new_args: Vec<TackyVal> = args.iter().map(|a| resolve_val(a, const_map)).collect();
            TackyInstr::FunCall {
                name: name.clone(),
                args: new_args,
                dst: dst.clone(),
                stack_arg_indices: stack_arg_indices.clone(),
                struct_arg_groups: struct_arg_groups.clone(),
                indirect: *indirect,
            }
        }
        other => other.clone(),
    }
}

fn resolve_val(val: &TackyVal, const_map: &std::collections::HashMap<String, (TackyVal, CType)>) -> TackyVal {
    if let TackyVal::Var(name) = val {
        if let Some((cval, _)) = const_map.get(name) {
            return cval.clone();
        }
    }
    val.clone()
}

fn resolve_val_type(val: &TackyVal, const_map: &std::collections::HashMap<String, (TackyVal, CType)>, types: &std::collections::HashMap<String, CType>) -> CType {
    if let TackyVal::Var(name) = val {
        if let Some((_, t)) = const_map.get(name) {
            return *t;
        }
        if let Some(t) = types.get(name) {
            return *t;
        }
    }
    CType::Int
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

fn fold_instruction(instr: TackyInstr, types: &std::collections::HashMap<String, CType>, src_type_hint: Option<CType>) -> TackyInstr {
    match instr {
        TackyInstr::Binary { op, left, right, dst } => {
            // Try integer constant folding
            if let (Some(l), Some(r)) = (const_val(&left), const_val(&right)) {
                // Determine the type of the operation from the destination
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::Int)
                } else { CType::Int };
                if let Some(result) = eval_binary_typed(&op, l, r, dst_type, src_type_hint) {
                    return TackyInstr::Copy { src: TackyVal::Constant(result), dst };
                }
            }
            // Try double constant folding
            if let (Some(l), Some(r)) = (const_double(&left), const_double(&right)) {
                if is_comparison(&op) {
                    // Comparisons return int, not double
                    if let Some(result) = eval_binary_double(&op, l, r) {
                        return TackyInstr::Copy { src: TackyVal::Constant(result as i64), dst };
                    }
                } else if let Some(result) = eval_binary_double(&op, l, r) {
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
                    TackyUnaryOp::LogicalNot => {
                        return TackyInstr::Copy { src: TackyVal::Constant(if d == 0.0 { 1 } else { 0 }), dst };
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
                // Truncate to the destination type
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::Int)
                } else { CType::Int };
                let truncated = match dst_type {
                    CType::Char | CType::SChar => v as i8 as i64,
                    CType::UChar => v as u8 as i64,
                    CType::Int => v as i32 as i64,
                    CType::UInt => v as u32 as i64,
                    _ => v as i32 as i64,
                };
                return TackyInstr::Copy { src: TackyVal::Constant(truncated), dst };
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
                let dst_type = if let TackyVal::Var(ref n) = dst { types.get(n).copied().unwrap_or(CType::Int) } else { CType::Int };
                let v = match dst_type {
                    CType::Int => d as i32 as i64,
                    CType::Long => d as i64,
                    CType::Char | CType::SChar => d as i8 as i64,
                    _ => d as i64,
                };
                return TackyInstr::Copy { src: TackyVal::Constant(v), dst };
            }
            TackyInstr::DoubleToInt { src, dst }
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            if let TackyVal::DoubleConstant(d) = src {
                let dst_type = if let TackyVal::Var(ref n) = dst { types.get(n).copied().unwrap_or(CType::UInt) } else { CType::UInt };
                let v = match dst_type {
                    CType::UInt => d as u32 as i64,
                    CType::ULong => d as u64 as i64,
                    CType::UChar => d as u8 as i64,
                    _ => d as u64 as i64,
                };
                return TackyInstr::Copy { src: TackyVal::Constant(v), dst };
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
        TackyInstr::AddPtr { ptr, index, scale, dst } => {
            if let Some(idx) = const_val(&index) {
                if idx == 0 {
                    return TackyInstr::Copy { src: ptr, dst };
                }
            }
            TackyInstr::AddPtr { ptr, index, scale, dst }
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

fn is_comparison(op: &TackyBinaryOp) -> bool {
    matches!(op, TackyBinaryOp::Equal | TackyBinaryOp::NotEqual |
        TackyBinaryOp::LessThan | TackyBinaryOp::GreaterThan |
        TackyBinaryOp::LessEqual | TackyBinaryOp::GreaterEqual)
}

fn eval_binary_typed(op: &TackyBinaryOp, l: i64, r: i64, dst_type: CType, src_type_hint: Option<CType>) -> Option<i64> {
    // For comparisons, use the source operand type (not the int result type)
    let op_type = if is_comparison(op) {
        src_type_hint.unwrap_or(dst_type)
    } else {
        dst_type
    };

    match op_type {
        CType::Int | CType::Char | CType::SChar => eval_binary_i32(op, l as i32, r as i32).map(|v| v as i64),
        CType::UInt | CType::UChar => eval_binary_u32(op, l as u32, r as u32).map(|v| v as i64),
        CType::Long => eval_binary(op, l, r),
        CType::ULong => eval_binary_u64(op, l as u64, r as u64).map(|v| v as i64),
        _ => eval_binary(op, l, r),
    }
}

fn eval_binary_i32(op: &TackyBinaryOp, l: i32, r: i32) -> Option<i32> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => if r == 0 { None } else { Some(l.wrapping_div(r)) },
        TackyBinaryOp::Mod => if r == 0 { None } else { Some(l.wrapping_rem(r)) },
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

fn eval_binary_u32(op: &TackyBinaryOp, l: u32, r: u32) -> Option<u32> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => if r == 0 { None } else { Some(l / r) },
        TackyBinaryOp::Mod => if r == 0 { None } else { Some(l % r) },
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseOr => Some(l | r),
        TackyBinaryOp::BitwiseXor => Some(l ^ r),
        TackyBinaryOp::ShiftLeft => Some(l.wrapping_shl(r)),
        TackyBinaryOp::ShiftRight => Some(l.wrapping_shr(r)),
        TackyBinaryOp::Equal => Some(if l == r { 1 } else { 0 }),
        TackyBinaryOp::NotEqual => Some(if l != r { 1 } else { 0 }),
        TackyBinaryOp::LessThan => Some(if l < r { 1 } else { 0 }),
        TackyBinaryOp::GreaterThan => Some(if l > r { 1 } else { 0 }),
        TackyBinaryOp::LessEqual => Some(if l <= r { 1 } else { 0 }),
        TackyBinaryOp::GreaterEqual => Some(if l >= r { 1 } else { 0 }),
        _ => None,
    }
}

fn eval_binary_u64(op: &TackyBinaryOp, l: u64, r: u64) -> Option<u64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => if r == 0 { None } else { Some(l / r) },
        TackyBinaryOp::Mod => if r == 0 { None } else { Some(l % r) },
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
    // Iterative approach: keep removing unreachable code until stable
    let mut result = instructions;
    loop {
        let before_len = result.len();
        result = unreachable_code_pass(result);
        if result.len() == before_len {
            break;
        }
    }
    result
}

fn unreachable_code_pass(instructions: Vec<TackyInstr>) -> Vec<TackyInstr> {
    // Pass 1: linear scan to mark reachable instructions
    // Only consider labels reachable if jumped to from REACHABLE code
    let mut reachable_labels = std::collections::HashSet::new();
    let mut reachable = true;

    for instr in &instructions {
        match instr {
            TackyInstr::Label(label) => {
                if reachable_labels.contains(label) {
                    reachable = true;
                }
                // Labels referenced by earlier reachable jumps make this reachable
            }
            TackyInstr::Jump(target) if reachable => {
                reachable_labels.insert(target.clone());
                reachable = false;
            }
            TackyInstr::JumpIfZero(_, target) if reachable => {
                reachable_labels.insert(target.clone());
                // Conditional jump: next instruction is also reachable
            }
            TackyInstr::JumpIfNotZero(_, target) if reachable => {
                reachable_labels.insert(target.clone());
            }
            TackyInstr::Return(_) if reachable => {
                reachable = false;
            }
            TackyInstr::Jump(_) | TackyInstr::Return(_) => {
                // Already unreachable
            }
            _ => {}
        }
    }

    // Pass 2: keep only reachable instructions
    let mut result = Vec::new();
    reachable = true;
    for instr in instructions {
        match &instr {
            TackyInstr::Label(label) => {
                if reachable_labels.contains(label) {
                    reachable = true;
                } else if !reachable {
                    continue; // Skip unreachable label
                }
                result.push(instr);
            }
            TackyInstr::Jump(_) | TackyInstr::Return(_) => {
                if reachable {
                    result.push(instr);
                }
                reachable = false;
            }
            TackyInstr::Nop => { /* skip */ }
            _ => {
                if reachable {
                    result.push(instr);
                }
            }
        }
    }

    // Pass 3: remove labels that are no longer jumped to
    let mut final_targets = std::collections::HashSet::new();
    for instr in &result {
        match instr {
            TackyInstr::Jump(t) | TackyInstr::JumpIfZero(_, t) | TackyInstr::JumpIfNotZero(_, t) => {
                final_targets.insert(t.clone());
            }
            _ => {}
        }
    }
    result.retain(|instr| {
        if let TackyInstr::Label(label) = instr {
            final_targets.contains(label)
        } else {
            true
        }
    });

    // Pass 4: remove jumps to immediately following label
    let mut cleaned = Vec::new();
    for i in 0..result.len() {
        let target_opt = match &result[i] {
            TackyInstr::Jump(t) => Some(t.clone()),
            TackyInstr::JumpIfZero(_, t) => Some(t.clone()),
            TackyInstr::JumpIfNotZero(_, t) => Some(t.clone()),
            _ => None,
        };
        if let Some(target) = target_opt {
            if i + 1 < result.len() {
                if let TackyInstr::Label(ref label) = result[i + 1] {
                    if target == *label {
                        continue;
                    }
                }
            }
        }
        cleaned.push(result[i].clone());
    }

    cleaned
}
