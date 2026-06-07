use crate::types::*;

#[derive(Debug, Clone)]
pub struct OptimizationFlags {
    pub fold_constants: bool,
    pub eliminate_unreachable_code: bool,
    pub propagate_copies: bool,
    pub eliminate_dead_stores: bool,
}

#[derive(Clone)]
struct KnownConstant {
    value: TackyVal,
    value_type: CType,
}

impl OptimizationFlags {
    pub fn any_enabled(&self) -> bool {
        self.fold_constants
            || self.eliminate_unreachable_code
            || self.propagate_copies
            || self.eliminate_dead_stores
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

fn optimize_function(
    func: &mut TackyFunction,
    flags: &OptimizationFlags,
    types: &std::collections::HashMap<String, CType>,
    static_var_names: &std::collections::HashSet<String>,
) {
    if func.body.is_empty() {
        return;
    }

    let static_vars = referenced_static_vars(&func.body, static_var_names);

    if flags.fold_constants || flags.eliminate_unreachable_code {
        loop {
            let before = func.body.clone();

            if flags.fold_constants {
                func.body = constant_folding(std::mem::take(&mut func.body), types);
            }

            if flags.eliminate_unreachable_code {
                func.body = unreachable_code_elimination(std::mem::take(&mut func.body));
            }

            if func.body == before || func.body.is_empty() {
                break;
            }
        }
    }

    if flags.propagate_copies && !func.body.is_empty() {
        let aliased_vars = crate::cfg::find_aliased_vars(&func.body, &static_vars);
        let mut cfg = crate::cfg::Cfg::build(std::mem::take(&mut func.body));
        crate::cfg::copy_propagation(&mut cfg, &aliased_vars, types);
        func.body = cfg.to_instructions();
        func.body = cse_copy_from_offset(std::mem::take(&mut func.body));

        if flags.fold_constants && !func.body.is_empty() {
            func.body = constant_folding(std::mem::take(&mut func.body), types);
        }
        if flags.eliminate_unreachable_code && !func.body.is_empty() {
            func.body = unreachable_code_elimination(std::mem::take(&mut func.body));
        }
    }

    if flags.eliminate_dead_stores && !func.body.is_empty() {
        let aliased_vars = crate::cfg::find_aliased_vars(&func.body, &static_vars);
        let mut before_len = func.body.len();
        loop {
            let mut cfg = crate::cfg::Cfg::build(std::mem::take(&mut func.body));
            crate::cfg::dead_store_elimination(&mut cfg, &aliased_vars, &static_vars);
            func.body = cfg.to_instructions();
            let after_len = func.body.len();
            if after_len == before_len || func.body.is_empty() {
                break;
            }
            before_len = after_len;
        }
    }
}

fn referenced_static_vars(
    instructions: &[TackyInstr],
    static_var_names: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let mut referenced = std::collections::HashSet::new();
    for instr in instructions {
        collect_static_refs(instr, static_var_names, &mut referenced);
    }
    referenced
}

fn collect_static_val_ref(
    val: &TackyVal,
    static_var_names: &std::collections::HashSet<String>,
    referenced: &mut std::collections::HashSet<String>,
) {
    if let TackyVal::Var(name) = val {
        if static_var_names.contains(name) {
            referenced.insert(name.clone());
        }
    }
}

fn collect_static_name_ref(
    name: &str,
    static_var_names: &std::collections::HashSet<String>,
    referenced: &mut std::collections::HashSet<String>,
) {
    if static_var_names.contains(name) {
        referenced.insert(name.to_string());
    }
}

fn collect_static_refs(
    instr: &TackyInstr,
    static_var_names: &std::collections::HashSet<String>,
    referenced: &mut std::collections::HashSet<String>,
) {
    match instr {
        TackyInstr::Copy { src, dst }
        | TackyInstr::Store { src, dst_ptr: dst }
        | TackyInstr::BuiltinLongjmp {
            buf: src,
            value: dst,
        } => {
            collect_static_val_ref(src, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::Unary { src, dst, .. }
        | TackyInstr::Truncate { src, dst }
        | TackyInstr::SignExtend { src, dst }
        | TackyInstr::ZeroExtend { src, dst }
        | TackyInstr::DoubleToInt { src, dst }
        | TackyInstr::FloatToInt { src, dst }
        | TackyInstr::DoubleToUInt { src, dst }
        | TackyInstr::FloatToUInt { src, dst }
        | TackyInstr::IntToDouble { src, dst }
        | TackyInstr::IntToFloat { src, dst }
        | TackyInstr::UIntToDouble { src, dst }
        | TackyInstr::UIntToFloat { src, dst }
        | TackyInstr::FloatToDouble { src, dst }
        | TackyInstr::DoubleToFloat { src, dst }
        | TackyInstr::Load { src_ptr: src, dst }
        | TackyInstr::GetAddress { src, dst } => {
            collect_static_val_ref(src, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::Binary {
            left, right, dst, ..
        }
        | TackyInstr::AddPtr {
            ptr: left,
            index: right,
            dst,
            ..
        } => {
            collect_static_val_ref(left, static_var_names, referenced);
            collect_static_val_ref(right, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::Return(val)
        | TackyInstr::JumpIndirect(val)
        | TackyInstr::JumpIfZero(val, _)
        | TackyInstr::JumpIfNotZero(val, _)
        | TackyInstr::VaStart { dst: val }
        | TackyInstr::FrameAddress { dst: val } => {
            collect_static_val_ref(val, static_var_names, referenced);
        }
        TackyInstr::BuiltinSetjmp { buf, dst, .. } => {
            collect_static_val_ref(buf, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::FunCall {
            name,
            args,
            dst,
            indirect,
            ..
        } => {
            if *indirect {
                collect_static_name_ref(name, static_var_names, referenced);
            }
            for arg in args {
                collect_static_val_ref(arg, static_var_names, referenced);
            }
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicFetch { ptr, arg, dst, .. } => {
            collect_static_val_ref(ptr, static_var_names, referenced);
            collect_static_val_ref(arg, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicExchange { ptr, value, dst } => {
            collect_static_val_ref(ptr, static_var_names, referenced);
            collect_static_val_ref(value, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicCompareExchange {
            ptr,
            expected,
            desired,
            dst,
        }
        | TackyInstr::AtomicCompareSwap {
            ptr,
            expected,
            desired,
            dst,
            ..
        } => {
            collect_static_val_ref(ptr, static_var_names, referenced);
            collect_static_val_ref(expected, static_var_names, referenced);
            collect_static_val_ref(desired, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::CopyToOffset { src, dst_name, .. } => {
            collect_static_val_ref(src, static_var_names, referenced);
            collect_static_name_ref(dst_name, static_var_names, referenced);
        }
        TackyInstr::CopyFromOffset { src_name, dst, .. } => {
            collect_static_name_ref(src_name, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::CopyStruct { src_name, dst_name } => {
            collect_static_name_ref(src_name, static_var_names, referenced);
            collect_static_name_ref(dst_name, static_var_names, referenced);
        }
        TackyInstr::LoadLabelAddress(_, dst) => {
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicFence
        | TackyInstr::Jump(_)
        | TackyInstr::NonlocalJump(_)
        | TackyInstr::Label(_)
        | TackyInstr::Unreachable
        | TackyInstr::Nop => {}
    }
}

// ============================================================
// Simple CSE for CopyFromOffset
// ============================================================

fn cse_copy_from_offset(instructions: Vec<TackyInstr>) -> Vec<TackyInstr> {
    // Track (src_name, offset) → first output variable
    let mut seen: std::collections::HashMap<(String, i64), String> =
        std::collections::HashMap::new();
    instructions
        .into_iter()
        .map(|instr| {
            match &instr {
                TackyInstr::CopyFromOffset {
                    src_name,
                    offset,
                    dst,
                } => {
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
                TackyInstr::CopyToOffset { dst_name, .. }
                | TackyInstr::CopyStruct { dst_name, .. } => {
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
        })
        .collect()
}

// ============================================================
// Constant Folding
// ============================================================

fn constant_folding(
    instructions: Vec<TackyInstr>,
    types: &std::collections::HashMap<String, CType>,
) -> Vec<TackyInstr> {
    // Track which variables hold known constant values, along with their type
    let mut const_map: std::collections::HashMap<String, KnownConstant> =
        std::collections::HashMap::new();

    instructions
        .into_iter()
        .map(|instr| {
            // At labels, clear the const_map (control flow can merge with different values)
            if let TackyInstr::Label(_) = &instr {
                const_map.clear();
                return instr;
            }
            // At function calls and stores — be conservative (may modify aliased vars)
            if matches!(
                &instr,
                TackyInstr::FunCall { .. } | TackyInstr::Store { .. }
            ) {
                const_map.clear();
            }

            // Capture source operand types before resolution (for typed folding)
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

            // Resolve operands using known constants — but NOT for Copy/Store/CopyToOffset sources
            // (Copy sources: handled by CFG-based copy propagation)
            // (Store/CopyToOffset sources: constant replacement may lose type info)
            let original_instr = instr.clone();
            let instr = if matches!(
                &instr,
                TackyInstr::Copy { .. }
                    | TackyInstr::Store { .. }
                    | TackyInstr::CopyToOffset { .. }
            ) {
                instr
            } else {
                resolve_constants(&instr, &const_map)
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
                    folded = original_instr;
                }
            }

            // Track constants: Copy(Constant, Var) and Copy(Var, Var) where Var is known
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
                    src: TackyVal::Var(s),
                    dst: TackyVal::Var(name),
                } => {
                    // If source has a known constant, propagate it
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
                // Any instruction that writes to a variable invalidates our knowledge
                _ => {
                    if let Some(dst_name) = get_dst_name(&folded) {
                        const_map.remove(&dst_name);
                    }
                }
            }

            folded
        })
        .collect()
}

fn same_copy_type(src: CType, dst: CType) -> bool {
    src == dst || (src.is_signed() == dst.is_signed() && src.size() == dst.size())
}

fn resolve_constants(
    instr: &TackyInstr,
    const_map: &std::collections::HashMap<String, KnownConstant>,
) -> TackyInstr {
    // Replace variable operands with their known constant values
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
            let new_args: Vec<TackyVal> = args.iter().map(|a| resolve_val(a, const_map)).collect();
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

fn resolve_val(
    val: &TackyVal,
    const_map: &std::collections::HashMap<String, KnownConstant>,
) -> TackyVal {
    if let TackyVal::Var(name) = val {
        if let Some(constant) = const_map.get(name) {
            return constant.value.clone();
        }
    }
    val.clone()
}

fn resolve_val_type(
    val: &TackyVal,
    const_map: &std::collections::HashMap<String, KnownConstant>,
    types: &std::collections::HashMap<String, CType>,
) -> CType {
    if let TackyVal::Var(name) = val {
        if let Some(constant) = const_map.get(name) {
            return constant.value_type;
        }
        if let Some(t) = types.get(name) {
            return *t;
        }
    }
    CType::Int
}

fn get_dst_name(instr: &TackyInstr) -> Option<String> {
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
        | TackyInstr::CopyFromOffset {
            dst: TackyVal::Var(n),
            ..
        } => Some(n.clone()),
        TackyInstr::FunCall {
            dst: TackyVal::Var(n),
            ..
        } => Some(n.clone()),
        _ => None,
    }
}

fn fold_instruction(
    instr: TackyInstr,
    types: &std::collections::HashMap<String, CType>,
    src_type_hint: Option<CType>,
) -> TackyInstr {
    match instr {
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            // Try integer constant folding
            if let (Some(l), Some(r)) = (const_val(&left), const_val(&right)) {
                // Determine the type of the operation from the destination
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::Int)
                } else {
                    CType::Int
                };
                if let Some(result) = eval_binary_typed(&op, l, r, dst_type, src_type_hint) {
                    return TackyInstr::Copy {
                        src: TackyVal::Constant(result),
                        dst,
                    };
                }
            }
            // Try double constant folding
            if let (Some(l), Some(r)) = (const_double(&left), const_double(&right)) {
                if is_comparison(&op) {
                    // Comparisons return int, not double
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
            TackyInstr::Binary {
                op,
                left,
                right,
                dst,
            }
        }
        TackyInstr::Unary { op, src, dst } => {
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
        // Type conversions with constant source
        TackyInstr::Truncate { src, dst } => {
            if let Some(v) = const_val(&src) {
                // Truncate to the destination type
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::Int)
                } else {
                    CType::Int
                };
                return TackyInstr::Copy {
                    src: TackyVal::Constant(cast_integer_constant(v, dst_type)),
                    dst,
                };
            }
            TackyInstr::Truncate { src, dst }
        }
        TackyInstr::SignExtend { src, dst } => {
            if let Some(v) = const_val(&src) {
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::Int)
                } else {
                    CType::Int
                };
                let src_type = src_type_hint.unwrap_or(dst_type);
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
            if let Some(v) = const_val(&src) {
                let dst_type = if let TackyVal::Var(ref n) = dst {
                    types.get(n).copied().unwrap_or(CType::UInt)
                } else {
                    CType::UInt
                };
                let src_type = src_type_hint.unwrap_or(dst_type);
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
                    CType::Int => d as i32 as i64,
                    CType::Long => d as i64,
                    CType::Char | CType::SChar => d as i8 as i64,
                    CType::Short => d as i16 as i64,
                    _ => d as i64,
                };
                return TackyInstr::Copy {
                    src: TackyVal::Constant(v),
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
                    CType::UInt => d as u32 as i64,
                    CType::ULong => d as u64 as i64,
                    CType::UChar => d as u8 as i64,
                    CType::UShort => d as u16 as i64,
                    _ => d as u64 as i64,
                };
                return TackyInstr::Copy {
                    src: TackyVal::Constant(v),
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
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(v as f64),
                    dst,
                };
            }
            TackyInstr::IntToDouble { src, dst }
        }
        TackyInstr::IntToFloat { src, dst } => {
            if let Some(v) = const_val(&src) {
                return TackyInstr::Copy {
                    src: TackyVal::DoubleConstant(v as f32 as f64),
                    dst,
                };
            }
            TackyInstr::IntToFloat { src, dst }
        }
        TackyInstr::UIntToDouble { src, dst } => {
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
        TackyInstr::FloatToDouble { src, dst } => TackyInstr::FloatToDouble { src, dst },
        TackyInstr::DoubleToFloat { src, dst } => TackyInstr::DoubleToFloat { src, dst },
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => {
            if let Some(idx) = const_val(&index) {
                if idx == 0 {
                    return TackyInstr::Copy { src: ptr, dst };
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

fn const_val(val: &TackyVal) -> Option<i64> {
    match val {
        TackyVal::Constant(c) => Some(*c),
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
    // For comparisons, use the source operand type (not the int result type)
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

fn eval_binary_i32(op: &TackyBinaryOp, l: i32, r: i32) -> Option<i32> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => {
            if r == 0 {
                None
            } else {
                Some(l.wrapping_div(r))
            }
        }
        TackyBinaryOp::Mod => {
            if r == 0 {
                None
            } else {
                Some(l.wrapping_rem(r))
            }
        }
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseNand => Some(!(l & r)),
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
    }
}

fn eval_binary_u32(op: &TackyBinaryOp, l: u32, r: u32) -> Option<u32> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => l.checked_div(r),
        TackyBinaryOp::Mod => {
            if r == 0 {
                None
            } else {
                Some(l % r)
            }
        }
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseNand => Some(!(l & r)),
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
    }
}

fn eval_binary_u64(op: &TackyBinaryOp, l: u64, r: u64) -> Option<u64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => l.checked_div(r),
        TackyBinaryOp::Mod => {
            if r == 0 {
                None
            } else {
                Some(l % r)
            }
        }
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseNand => Some(!(l & r)),
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

fn eval_binary(op: &TackyBinaryOp, l: i64, r: i64) -> Option<i64> {
    match op {
        TackyBinaryOp::Add => Some(l.wrapping_add(r)),
        TackyBinaryOp::Sub => Some(l.wrapping_sub(r)),
        TackyBinaryOp::Mul => Some(l.wrapping_mul(r)),
        TackyBinaryOp::Div => {
            if r == 0 {
                None
            } else {
                Some(l.wrapping_div(r))
            }
        }
        TackyBinaryOp::Mod => {
            if r == 0 {
                None
            } else {
                Some(l.wrapping_rem(r))
            }
        }
        TackyBinaryOp::BitwiseAnd => Some(l & r),
        TackyBinaryOp::BitwiseNand => Some(!(l & r)),
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
    let cfg = crate::cfg::Cfg::build(instructions);
    let mut reachable_blocks = std::collections::HashSet::new();
    let mut worklist = std::collections::VecDeque::new();
    if !cfg.blocks.is_empty() {
        reachable_blocks.insert(0usize);
        worklist.push_back(0usize);
    }
    while let Some(block_id) = worklist.pop_front() {
        if cfg.blocks[block_id]
            .instructions
            .iter()
            .any(|instr| matches!(instr, TackyInstr::JumpIndirect(_)))
        {
            for indirect_target_id in 0..cfg.blocks.len() {
                if reachable_blocks.insert(indirect_target_id) {
                    worklist.push_back(indirect_target_id);
                }
            }
        }
        for successor in &cfg.blocks[block_id].successors {
            let crate::cfg::NodeId::Block(successor_id) = successor else {
                continue;
            };
            if reachable_blocks.insert(*successor_id) {
                worklist.push_back(*successor_id);
            }
        }
    }

    let result = cfg
        .blocks
        .into_iter()
        .filter(|block| reachable_blocks.contains(&block.id))
        .flat_map(|block| block.instructions)
        .filter(|instr| !matches!(instr, TackyInstr::Nop))
        .collect::<Vec<_>>();

    // Remove jumps to immediately following label.  Keep surviving
    // labels even when this pass cannot prove they are branch targets: labels
    // also partition later CFG passes, and removing them can invalidate jumps
    // that become visible after branch simplification.
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
