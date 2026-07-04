use crate::types::*;
use std::collections::{HashMap, HashSet, VecDeque};

use super::instr_utils::for_each_instr_defined_var;

// ============================================================
// CFG-aware local CSE for CopyFromOffset
// ============================================================

pub(super) fn cse_copy_from_offset(instructions: Vec<TackyInstr>) -> Vec<TackyInstr> {
    let mut cfg = crate::cfg::Cfg::build(instructions);
    for block in &mut cfg.blocks {
        block.instructions = cse_copy_from_offset_block(std::mem::take(&mut block.instructions));
    }
    cfg.into_instructions()
}

fn cse_copy_from_offset_block(instructions: Vec<TackyInstr>) -> Vec<TackyInstr> {
    // Track (src_name, offset) → first output variable
    let mut seen: HashMap<(String, i64), String> = HashMap::new();
    let mut seen_by_source: HashMap<String, HashSet<(String, i64)>> = HashMap::new();
    let mut seen_by_holder: HashMap<String, HashSet<(String, i64)>> = HashMap::new();
    let mut rewritten = Vec::with_capacity(instructions.len());
    for instr in instructions {
        for_each_instr_defined_var(&instr, |def| {
            invalidate_copy_from_offset_entries(
                def,
                &mut seen,
                &mut seen_by_source,
                &mut seen_by_holder,
            );
        });
        match &instr {
            // CopyToOffset/CopyStruct/Store/FunCall may modify the struct — invalidate
            TackyInstr::CopyToOffset { dst_name, .. } | TackyInstr::CopyStruct { dst_name, .. } => {
                invalidate_copy_from_offset_source(
                    dst_name,
                    &mut seen,
                    &mut seen_by_source,
                    &mut seen_by_holder,
                );
            }
            TackyInstr::Store { .. }
            | TackyInstr::FunCall { .. }
            | TackyInstr::AtomicFence
            | TackyInstr::AtomicFetch { .. }
            | TackyInstr::AtomicExchange { .. }
            | TackyInstr::AtomicCompareExchange { .. }
            | TackyInstr::AtomicCompareSwap { .. }
            | TackyInstr::BuiltinSetjmp { .. }
            | TackyInstr::BuiltinLongjmp { .. }
            | TackyInstr::VaStart { .. } => {
                seen.clear();
            }
            _ => {}
        }

        match instr {
            TackyInstr::CopyFromOffset {
                src_name,
                offset,
                dst,
            } => {
                let key = (src_name.clone(), offset);
                if let Some(prev_dst) = seen.get(&key) {
                    // Duplicate CopyFromOffset — replace with Copy from previous output
                    if let TackyVal::Var(d) = &dst {
                        rewritten.push(TackyInstr::Copy {
                            src: TackyVal::Var(prev_dst.clone()),
                            dst: TackyVal::Var(d.clone()),
                        });
                        continue;
                    }
                } else if let TackyVal::Var(d) = &dst {
                    insert_copy_from_offset_entry(
                        key,
                        d.clone(),
                        &mut seen,
                        &mut seen_by_source,
                        &mut seen_by_holder,
                    );
                }
                rewritten.push(TackyInstr::CopyFromOffset {
                    src_name,
                    offset,
                    dst,
                });
            }
            instr if is_copy_from_offset_cse_barrier(&instr) => rewritten.push(instr),
            instr => rewritten.push(instr),
        }
    }
    rewritten
}

fn insert_copy_from_offset_entry(
    key: (String, i64),
    holder: String,
    seen: &mut HashMap<(String, i64), String>,
    seen_by_source: &mut HashMap<String, HashSet<(String, i64)>>,
    seen_by_holder: &mut HashMap<String, HashSet<(String, i64)>>,
) {
    seen.insert(key.clone(), holder.clone());
    seen_by_source
        .entry(key.0.clone())
        .or_default()
        .insert(key.clone());
    seen_by_holder.entry(holder).or_default().insert(key);
}

fn invalidate_copy_from_offset_entries(
    def: &str,
    seen: &mut HashMap<(String, i64), String>,
    seen_by_source: &mut HashMap<String, HashSet<(String, i64)>>,
    seen_by_holder: &mut HashMap<String, HashSet<(String, i64)>>,
) {
    let mut keys = HashSet::new();
    if let Some(source_keys) = seen_by_source.remove(def) {
        keys.extend(source_keys);
    }
    if let Some(holder_keys) = seen_by_holder.remove(def) {
        keys.extend(holder_keys);
    }
    for key in keys {
        invalidate_copy_from_offset_key(key, seen, seen_by_source, seen_by_holder);
    }
}

fn invalidate_copy_from_offset_source(
    src_name: &str,
    seen: &mut HashMap<(String, i64), String>,
    seen_by_source: &mut HashMap<String, HashSet<(String, i64)>>,
    seen_by_holder: &mut HashMap<String, HashSet<(String, i64)>>,
) {
    if let Some(keys) = seen_by_source.remove(src_name) {
        for key in keys {
            invalidate_copy_from_offset_key(key, seen, seen_by_source, seen_by_holder);
        }
    }
}

fn invalidate_copy_from_offset_key(
    key: (String, i64),
    seen: &mut HashMap<(String, i64), String>,
    seen_by_source: &mut HashMap<String, HashSet<(String, i64)>>,
    seen_by_holder: &mut HashMap<String, HashSet<(String, i64)>>,
) {
    if let Some(holder) = seen.remove(&key) {
        let remove_holder = if let Some(holder_keys) = seen_by_holder.get_mut(&holder) {
            holder_keys.remove(&key);
            holder_keys.is_empty()
        } else {
            false
        };
        if remove_holder {
            seen_by_holder.remove(&holder);
        }
    }
    let source = key.0.clone();
    let remove_source = if let Some(source_keys) = seen_by_source.get_mut(&source) {
        source_keys.remove(&key);
        source_keys.is_empty()
    } else {
        false
    };
    if remove_source {
        seen_by_source.remove(&source);
    }
}

fn is_copy_from_offset_cse_barrier(instr: &TackyInstr) -> bool {
    matches!(
        instr,
        TackyInstr::Label(_)
            | TackyInstr::Jump(_)
            | TackyInstr::JumpIfZero(_, _)
            | TackyInstr::JumpIfNotZero(_, _)
            | TackyInstr::JumpIndirect(_)
            | TackyInstr::NonlocalJump(_)
            | TackyInstr::Return(_)
            | TackyInstr::Unreachable
    )
}

// ============================================================
// Common Subexpression Elimination
// ============================================================

type AvailableExprs = HashMap<CseExprKey, String>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum CseValKey {
    Var(String),
    Constant(i64),
    Int128Constant(i128),
    UInt128Constant(u128),
    DoubleConstant(u64),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CseExprKey {
    Unary {
        op: TackyUnaryOp,
        src: CseValKey,
        src_type: CType,
        dst_type: CType,
    },
    Binary {
        op: TackyBinaryOp,
        left: CseValKey,
        right: CseValKey,
        left_type: CType,
        right_type: CType,
        dst_type: CType,
    },
    Conversion {
        kind: CseConversionKind,
        src: CseValKey,
        src_type: CType,
        dst_type: CType,
    },
    AddPtr {
        ptr: CseValKey,
        index: CseValKey,
        scale: i64,
        dst_type: CType,
    },
    GetAddress {
        src_name: String,
        dst_type: CType,
    },
    LabelAddress {
        label: String,
        dst_type: CType,
    },
    FrameAddress {
        dst_type: CType,
    },
    Load {
        src_ptr: CseValKey,
        dst_type: CType,
    },
    CopyFromOffset {
        src_name: String,
        offset: i64,
        dst_type: CType,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CseConversionKind {
    Truncate,
    SignExtend,
    ZeroExtend,
    DoubleToInt,
    FloatToInt,
    DoubleToUInt,
    FloatToUInt,
    IntToDouble,
    IntToFloat,
    UIntToDouble,
    UIntToFloat,
    FloatToDouble,
    DoubleToFloat,
}

pub(super) fn common_subexpression_elimination(
    cfg: &mut crate::cfg::Cfg,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    let mut block_out: Vec<AvailableExprs> = vec![HashMap::new(); cfg.blocks.len()];

    let mut worklist: VecDeque<usize> = (0..cfg.blocks.len()).collect();
    let mut queued: HashSet<usize> = (0..cfg.blocks.len()).collect();
    while let Some(block_id) = worklist.pop_front() {
        queued.remove(&block_id);
        let incoming = meet_available_exprs(&cfg.blocks[block_id], &block_out);
        let new_out = transfer_available_exprs(
            &cfg.blocks[block_id].instructions,
            incoming,
            types,
            aliased_vars,
            static_vars,
        );
        if block_out[block_id] != new_out {
            block_out[block_id] = new_out;
            for successor in &cfg.blocks[block_id].successors {
                if let crate::cfg::NodeId::Block(successor_id) = successor {
                    if queued.insert(*successor_id) {
                        worklist.push_back(*successor_id);
                    }
                }
            }
        }
    }

    let mut changed = false;
    for block_id in 0..cfg.blocks.len() {
        let mut available = meet_available_exprs(&cfg.blocks[block_id], &block_out);
        let mut rewritten = Vec::with_capacity(cfg.blocks[block_id].instructions.len());

        for instr in std::mem::take(&mut cfg.blocks[block_id].instructions) {
            let replacement =
                cse_expr_key(&instr, types, aliased_vars, static_vars).and_then(|(key, dst)| {
                    available.get(&key).and_then(|holder| {
                        if holder == &dst {
                            None
                        } else {
                            Some(TackyInstr::Copy {
                                src: TackyVal::Var(holder.clone()),
                                dst: TackyVal::Var(dst),
                            })
                        }
                    })
                });

            if let Some(replacement) = replacement {
                transfer_available_expr_instr(
                    &replacement,
                    &mut available,
                    types,
                    aliased_vars,
                    static_vars,
                );
                rewritten.push(replacement);
                changed = true;
            } else {
                transfer_available_expr_instr(
                    &instr,
                    &mut available,
                    types,
                    aliased_vars,
                    static_vars,
                );
                rewritten.push(instr);
            }
        }

        cfg.blocks[block_id].instructions = rewritten;
    }

    changed
}

fn meet_available_exprs(
    block: &crate::cfg::BasicBlock,
    block_out: &[AvailableExprs],
) -> AvailableExprs {
    let mut best_pred: Option<(usize, usize)> = None;
    for pred in &block.predecessors {
        match pred {
            crate::cfg::NodeId::Entry => return HashMap::new(),
            crate::cfg::NodeId::Block(pred_id) => {
                let pred_out = match block_out.get(*pred_id) {
                    Some(pred_out) => pred_out,
                    None => continue,
                };
                let len = pred_out.len();
                if best_pred.is_none_or(|(_, best_len)| len < best_len) {
                    best_pred = Some((*pred_id, len));
                }
            }
            crate::cfg::NodeId::Exit => {}
        }
    }

    let Some((seed_pred, _)) = best_pred else {
        return HashMap::new();
    };
    let mut incoming = block_out.get(seed_pred).cloned().unwrap_or_default();
    for pred in &block.predecessors {
        let crate::cfg::NodeId::Block(pred_id) = pred else {
            continue;
        };
        if *pred_id == seed_pred {
            continue;
        }
        if let Some(pred_out) = block_out.get(*pred_id) {
            incoming.retain(|key, holder| pred_out.get(key) == Some(holder));
        }
    }
    incoming
}

fn transfer_available_exprs(
    instructions: &[TackyInstr],
    mut available: AvailableExprs,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> AvailableExprs {
    for instr in instructions {
        transfer_available_expr_instr(instr, &mut available, types, aliased_vars, static_vars);
    }
    available
}

fn transfer_available_expr_instr(
    instr: &TackyInstr,
    available: &mut AvailableExprs,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) {
    if is_cse_barrier(instr) {
        available.clear();
    }

    for_each_instr_defined_var(instr, |def| {
        kill_available_exprs_for_var(available, def);
        if aliased_vars.contains(def) || static_vars.contains(def) {
            kill_available_load_exprs(available);
        }
    });

    if let Some((key, dst)) = cse_expr_key(instr, types, aliased_vars, static_vars) {
        if !cse_expr_uses_var(&key, &dst) {
            available.insert(key, dst);
        }
    }
}

fn kill_available_exprs_for_var(available: &mut AvailableExprs, name: &str) {
    available.retain(|key, holder| holder != name && !cse_expr_uses_var(key, name));
}

fn kill_available_load_exprs(available: &mut AvailableExprs) {
    available.retain(|key, _| !matches!(key, CseExprKey::Load { .. }));
}

fn is_cse_barrier(instr: &TackyInstr) -> bool {
    matches!(
        instr,
        TackyInstr::FunCall { .. }
            | TackyInstr::Store { .. }
            | TackyInstr::CopyToOffset { .. }
            | TackyInstr::CopyStruct { .. }
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

fn cse_expr_key(
    instr: &TackyInstr,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> Option<(CseExprKey, String)> {
    let dst = cse_candidate_dst(instr)?;
    if aliased_vars.contains(dst) || static_vars.contains(dst) {
        return None;
    }
    let dst_type = types.get(dst).copied().unwrap_or(CType::Int);
    if matches!(dst_type, CType::Struct | CType::Void | CType::LongDouble) {
        return None;
    }

    let key = match instr {
        TackyInstr::Unary { op, src, .. } => CseExprKey::Unary {
            op: op.clone(),
            src: cse_val_key(src)?,
            src_type: cse_val_type(src, types),
            dst_type,
        },
        TackyInstr::Binary {
            op, left, right, ..
        } => {
            if matches!(op, TackyBinaryOp::Div | TackyBinaryOp::Mod) {
                return None;
            }
            let mut left_key = cse_val_key(left)?;
            let mut right_key = cse_val_key(right)?;
            let mut left_type = cse_val_type(left, types);
            let mut right_type = cse_val_type(right, types);
            if is_cse_commutative_binary(op, left_type, right_type) && right_key < left_key {
                std::mem::swap(&mut left_key, &mut right_key);
                std::mem::swap(&mut left_type, &mut right_type);
            }
            CseExprKey::Binary {
                op: op.clone(),
                left: left_key,
                right: right_key,
                left_type,
                right_type,
                dst_type,
            }
        }
        TackyInstr::Truncate { src, .. } => {
            conversion_cse_key(CseConversionKind::Truncate, src, dst_type, types)?
        }
        TackyInstr::SignExtend { src, .. } => {
            conversion_cse_key(CseConversionKind::SignExtend, src, dst_type, types)?
        }
        TackyInstr::ZeroExtend { src, .. } => {
            conversion_cse_key(CseConversionKind::ZeroExtend, src, dst_type, types)?
        }
        TackyInstr::DoubleToInt { src, .. } => {
            conversion_cse_key(CseConversionKind::DoubleToInt, src, dst_type, types)?
        }
        TackyInstr::FloatToInt { src, .. } => {
            conversion_cse_key(CseConversionKind::FloatToInt, src, dst_type, types)?
        }
        TackyInstr::DoubleToUInt { src, .. } => {
            conversion_cse_key(CseConversionKind::DoubleToUInt, src, dst_type, types)?
        }
        TackyInstr::FloatToUInt { src, .. } => {
            conversion_cse_key(CseConversionKind::FloatToUInt, src, dst_type, types)?
        }
        TackyInstr::IntToDouble { src, .. } => {
            conversion_cse_key(CseConversionKind::IntToDouble, src, dst_type, types)?
        }
        TackyInstr::IntToFloat { src, .. } => {
            conversion_cse_key(CseConversionKind::IntToFloat, src, dst_type, types)?
        }
        TackyInstr::UIntToDouble { src, .. } => {
            conversion_cse_key(CseConversionKind::UIntToDouble, src, dst_type, types)?
        }
        TackyInstr::UIntToFloat { src, .. } => {
            conversion_cse_key(CseConversionKind::UIntToFloat, src, dst_type, types)?
        }
        TackyInstr::FloatToDouble { src, .. } => {
            conversion_cse_key(CseConversionKind::FloatToDouble, src, dst_type, types)?
        }
        TackyInstr::DoubleToFloat { src, .. } => {
            conversion_cse_key(CseConversionKind::DoubleToFloat, src, dst_type, types)?
        }
        TackyInstr::AddPtr {
            ptr, index, scale, ..
        } => CseExprKey::AddPtr {
            ptr: cse_val_key(ptr)?,
            index: cse_val_key(index)?,
            scale: *scale,
            dst_type,
        },
        TackyInstr::GetAddress {
            src: TackyVal::Var(src_name),
            ..
        } => CseExprKey::GetAddress {
            src_name: src_name.clone(),
            dst_type,
        },
        TackyInstr::LoadLabelAddress(label, _) => CseExprKey::LabelAddress {
            label: label.clone(),
            dst_type,
        },
        TackyInstr::FrameAddress { .. } => CseExprKey::FrameAddress { dst_type },
        TackyInstr::Load { src_ptr, .. } => CseExprKey::Load {
            src_ptr: cse_val_key(src_ptr)?,
            dst_type,
        },
        TackyInstr::CopyFromOffset {
            src_name, offset, ..
        } => CseExprKey::CopyFromOffset {
            src_name: src_name.clone(),
            offset: *offset,
            dst_type,
        },
        _ => return None,
    };

    if cse_expr_uses_reserved_var(&key, aliased_vars, static_vars) {
        return None;
    }

    Some((key, dst.to_string()))
}

fn is_cse_commutative_binary(op: &TackyBinaryOp, left_type: CType, right_type: CType) -> bool {
    matches!(
        op,
        TackyBinaryOp::Add
            | TackyBinaryOp::Mul
            | TackyBinaryOp::BitwiseAnd
            | TackyBinaryOp::BitwiseOr
            | TackyBinaryOp::BitwiseXor
            | TackyBinaryOp::Equal
            | TackyBinaryOp::NotEqual
    ) && !left_type.is_floating()
        && !right_type.is_floating()
}

fn conversion_cse_key(
    kind: CseConversionKind,
    src: &TackyVal,
    dst_type: CType,
    types: &indexmap::IndexMap<String, CType>,
) -> Option<CseExprKey> {
    Some(CseExprKey::Conversion {
        kind,
        src: cse_val_key(src)?,
        src_type: cse_val_type(src, types),
        dst_type,
    })
}

fn cse_candidate_dst(instr: &TackyInstr) -> Option<&str> {
    match instr {
        TackyInstr::Unary {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::Binary {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::Truncate {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::SignExtend {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::ZeroExtend {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::DoubleToInt {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::FloatToInt {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::DoubleToUInt {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::FloatToUInt {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::IntToDouble {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::IntToFloat {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::UIntToDouble {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::UIntToFloat {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::FloatToDouble {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::DoubleToFloat {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::Load {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::CopyFromOffset {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::GetAddress {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::FrameAddress {
            dst: TackyVal::Var(name),
        }
        | TackyInstr::LoadLabelAddress(_, TackyVal::Var(name))
        | TackyInstr::AddPtr {
            dst: TackyVal::Var(name),
            ..
        } => Some(name),
        _ => None,
    }
}

fn cse_val_key(val: &TackyVal) -> Option<CseValKey> {
    match val {
        TackyVal::Var(name) => Some(CseValKey::Var(name.clone())),
        TackyVal::Constant(value) => Some(CseValKey::Constant(*value)),
        TackyVal::Int128Constant(value) => Some(CseValKey::Int128Constant(*value)),
        TackyVal::UInt128Constant(value) => Some(CseValKey::UInt128Constant(*value)),
        TackyVal::DoubleConstant(value) => Some(CseValKey::DoubleConstant(value.to_bits())),
    }
}

fn cse_val_type(val: &TackyVal, types: &indexmap::IndexMap<String, CType>) -> CType {
    match val {
        TackyVal::Var(name) => types.get(name).copied().unwrap_or(CType::Int),
        TackyVal::Constant(_) => CType::Int,
        TackyVal::Int128Constant(_) => CType::Int128,
        TackyVal::UInt128Constant(_) => CType::UInt128,
        TackyVal::DoubleConstant(_) => CType::Double,
    }
}

fn cse_expr_uses_var(key: &CseExprKey, name: &str) -> bool {
    match key {
        CseExprKey::Unary { src, .. } | CseExprKey::Conversion { src, .. } => {
            cse_val_uses_name(src, name)
        }
        CseExprKey::Binary { left, right, .. } => {
            cse_val_uses_name(left, name) || cse_val_uses_name(right, name)
        }
        CseExprKey::AddPtr { ptr, index, .. } => {
            cse_val_uses_name(ptr, name) || cse_val_uses_name(index, name)
        }
        CseExprKey::GetAddress { .. } | CseExprKey::LabelAddress { .. } => false,
        CseExprKey::FrameAddress { .. } => false,
        CseExprKey::Load { src_ptr, .. } => cse_val_uses_name(src_ptr, name),
        CseExprKey::CopyFromOffset { src_name, .. } => src_name == name,
    }
}

fn cse_val_uses_name(val: &CseValKey, name: &str) -> bool {
    match val {
        CseValKey::Var(var) => var == name,
        _ => false,
    }
}

fn cse_expr_uses_reserved_var(
    key: &CseExprKey,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    match key {
        CseExprKey::Unary { src, .. } | CseExprKey::Conversion { src, .. } => {
            cse_val_is_reserved(src, aliased_vars, static_vars)
        }
        CseExprKey::Binary { left, right, .. } => {
            cse_val_is_reserved(left, aliased_vars, static_vars)
                || cse_val_is_reserved(right, aliased_vars, static_vars)
        }
        CseExprKey::AddPtr { ptr, index, .. } => {
            cse_val_is_reserved(ptr, aliased_vars, static_vars)
                || cse_val_is_reserved(index, aliased_vars, static_vars)
        }
        CseExprKey::GetAddress { .. } | CseExprKey::LabelAddress { .. } => false,
        CseExprKey::FrameAddress { .. } => false,
        CseExprKey::Load { src_ptr, .. } => cse_val_is_reserved(src_ptr, aliased_vars, static_vars),
        CseExprKey::CopyFromOffset { src_name, .. } => {
            aliased_vars.contains(src_name) || static_vars.contains(src_name)
        }
    }
}

fn cse_val_is_reserved(
    val: &CseValKey,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    match val {
        CseValKey::Var(var) => aliased_vars.contains(var) || static_vars.contains(var),
        _ => false,
    }
}
