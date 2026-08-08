use crate::types::*;
use std::collections::{HashMap, HashSet, VecDeque};

use super::instr_utils::{for_each_instr_defined_var, for_each_instr_source_var};

// ============================================================
// Loop-Invariant Code Motion
// ============================================================

pub(super) fn loop_invariant_code_motion(
    cfg: &mut crate::cfg::Cfg,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    let mut changed = false;
    while licm_once(cfg, types, aliased_vars, static_vars) {
        changed = true;
    }
    changed
}

fn licm_once(
    cfg: &mut crate::cfg::Cfg,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    let dominators = compute_dominators(cfg);
    let mut backedges = Vec::with_capacity(cfg.blocks.len());

    for block in &cfg.blocks {
        for successor in &block.successors {
            let crate::cfg::NodeId::Block(header) = successor else {
                continue;
            };
            if dominators
                .get(block.id)
                .is_some_and(|doms| doms.contains(header))
            {
                let loop_blocks = natural_loop_blocks(cfg, *header, block.id);
                backedges.push((*header, block.id, loop_blocks));
            }
        }
    }

    backedges.sort_unstable_by_key(|(_, _, loop_blocks)| loop_blocks.len());

    for (header, _, loop_blocks) in backedges {
        let Some(preheader) = loop_preheader(cfg, header, &loop_blocks) else {
            continue;
        };
        let def_counts = collect_loop_def_counts(cfg, &loop_blocks);
        let loop_writes_memory = loop_blocks.iter().any(|block_id| {
            cfg.blocks[*block_id]
                .instructions
                .iter()
                .any(is_memory_write)
        });
        let hoisted_capacity: usize = loop_blocks
            .iter()
            .map(|block_id| cfg.blocks[*block_id].instructions.len())
            .sum();
        let mut hoisted_defs = HashSet::with_capacity(def_counts.len());
        let mut hoisted_instrs = Vec::with_capacity(hoisted_capacity);

        for block_id in 0..cfg.blocks.len() {
            if !loop_blocks.contains(&block_id) {
                continue;
            }
            // Hoisting executes an instruction once before the loop, so the
            // instruction must have executed on every path through the loop
            // originally.  Without this dominance check, an invariant load
            // or conversion in a conditional loop block could be evaluated on
            // iterations that never reached that block.
            if !loop_blocks
                .iter()
                .all(|loop_block| dominators[*loop_block].contains(&block_id))
            {
                continue;
            }
            let mut kept = Vec::with_capacity(cfg.blocks[block_id].instructions.len());
            for instr in std::mem::take(&mut cfg.blocks[block_id].instructions) {
                if is_loop_invariant_candidate(
                    &instr,
                    &def_counts,
                    &hoisted_defs,
                    types,
                    aliased_vars,
                    static_vars,
                    loop_writes_memory,
                ) {
                    if let Some(dst) = licm_candidate_dst(&instr) {
                        hoisted_defs.insert(dst.to_string());
                    }
                    hoisted_instrs.push(instr);
                } else {
                    kept.push(instr);
                }
            }
            cfg.blocks[block_id].instructions = kept;
        }

        if hoisted_instrs.is_empty() {
            continue;
        }

        let preheader_instrs = &mut cfg.blocks[preheader].instructions;
        if let Some(TackyInstr::Jump(_)) = preheader_instrs.last() {
            let Some(jump) = preheader_instrs.pop() else {
                unreachable!("preheader jump disappeared while inserting loop-invariant code");
            };
            preheader_instrs.extend(hoisted_instrs);
            preheader_instrs.push(jump);
        } else {
            preheader_instrs.extend(hoisted_instrs);
        }
        return true;
    }

    false
}

fn compute_dominators(cfg: &crate::cfg::Cfg) -> Vec<HashSet<usize>> {
    let num_blocks = cfg.blocks.len();
    let mut all_blocks = HashSet::with_capacity(num_blocks);
    all_blocks.extend(0..num_blocks);
    let mut dominators = vec![all_blocks; num_blocks];
    if num_blocks == 0 {
        return dominators;
    }
    dominators[0] = HashSet::from([0]);

    loop {
        let mut changed = false;

        for block_id in 1..num_blocks {
            let mut best_pred: Option<(usize, usize)> = None;
            for pred in &cfg.blocks[block_id].predecessors {
                let crate::cfg::NodeId::Block(pred_id) = pred else {
                    continue;
                };
                let len = dominators[*pred_id].len();
                if best_pred.is_none_or(|(_, best_len)| len < best_len) {
                    best_pred = Some((*pred_id, len));
                }
            }

            let mut new_doms = if let Some((seed_pred, _)) = best_pred {
                let mut intersection = dominators[seed_pred].clone();
                for pred in &cfg.blocks[block_id].predecessors {
                    let crate::cfg::NodeId::Block(pred_id) = pred else {
                        continue;
                    };
                    if *pred_id == seed_pred {
                        continue;
                    }
                    intersection.retain(|dom| dominators[*pred_id].contains(dom));
                }
                intersection
            } else {
                HashSet::new()
            };
            new_doms.insert(block_id);

            if new_doms != dominators[block_id] {
                dominators[block_id] = new_doms;
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    dominators
}

fn natural_loop_blocks(cfg: &crate::cfg::Cfg, header: usize, tail: usize) -> HashSet<usize> {
    let mut loop_blocks = HashSet::from([header, tail]);
    let mut worklist = VecDeque::from([tail]);

    while let Some(block_id) = worklist.pop_front() {
        if block_id == header {
            continue;
        }
        for pred in &cfg.blocks[block_id].predecessors {
            let crate::cfg::NodeId::Block(pred_id) = pred else {
                continue;
            };
            if loop_blocks.insert(*pred_id) {
                worklist.push_back(*pred_id);
            }
        }
    }

    loop_blocks
}

fn loop_preheader(
    cfg: &crate::cfg::Cfg,
    header: usize,
    loop_blocks: &HashSet<usize>,
) -> Option<usize> {
    let mut preheader = None;
    for pred in &cfg.blocks[header].predecessors {
        let crate::cfg::NodeId::Block(id) = pred else {
            continue;
        };
        if loop_blocks.contains(id) {
            continue;
        }
        if preheader.replace(*id).is_some() {
            return None;
        }
    }
    let preheader = preheader?;
    if cfg.blocks[preheader].successors.as_slice() == [crate::cfg::NodeId::Block(header)] {
        Some(preheader)
    } else {
        None
    }
}

fn collect_loop_def_counts(
    cfg: &crate::cfg::Cfg,
    loop_blocks: &HashSet<usize>,
) -> HashMap<String, usize> {
    let mut counts = HashMap::with_capacity(loop_blocks.len());
    for block_id in loop_blocks {
        for instr in &cfg.blocks[*block_id].instructions {
            for_each_instr_defined_var(instr, |name| {
                *counts.entry(name.to_string()).or_insert(0) += 1;
            });
        }
    }
    counts
}

fn is_loop_invariant_candidate(
    instr: &TackyInstr,
    def_counts: &HashMap<String, usize>,
    hoisted_defs: &HashSet<String>,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
    loop_writes_memory: bool,
) -> bool {
    if !is_pure_licm_instr(instr) {
        return false;
    }

    let Some(dst) = licm_candidate_dst(instr) else {
        return false;
    };
    if !is_licm_temporary(dst) {
        return false;
    }
    if def_counts.get(dst).copied().unwrap_or_default() != 1 {
        return false;
    }
    if aliased_vars.contains(dst) || static_vars.contains(dst) {
        return false;
    }
    if matches!(
        types.get(dst),
        Some(CType::Struct | CType::Void | CType::LongDouble)
    ) {
        return false;
    }
    if loop_writes_memory && matches!(instr, TackyInstr::CopyFromOffset { .. }) {
        return false;
    }

    let mut ok = true;
    if !matches!(
        instr,
        TackyInstr::GetAddress { .. }
            | TackyInstr::LoadLabelAddress(..)
            | TackyInstr::FrameAddress { .. }
    ) {
        for_each_instr_source_var(instr, |src| {
            if aliased_vars.contains(src)
                || static_vars.contains(src)
                || (def_counts.get(src).copied().unwrap_or_default() != 0
                    && !hoisted_defs.contains(src))
            {
                ok = false;
            }
        });
    }
    ok
}

fn is_licm_temporary(name: &str) -> bool {
    name.starts_with("__rnqcc_tmp.") || name.contains(".__rnqcc_tmp.")
}

fn licm_candidate_dst(instr: &TackyInstr) -> Option<&str> {
    match instr {
        TackyInstr::Copy {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::Unary {
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
        | TackyInstr::GetAddress {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::FrameAddress {
            dst: TackyVal::Var(name),
        }
        | TackyInstr::LoadLabelAddress(_, TackyVal::Var(name))
        | TackyInstr::CopyFromOffset {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::AddPtr {
            dst: TackyVal::Var(name),
            ..
        } => Some(name),
        _ => None,
    }
}

fn is_pure_licm_instr(instr: &TackyInstr) -> bool {
    match instr {
        TackyInstr::Copy { .. }
        | TackyInstr::Unary { .. }
        | TackyInstr::Truncate { .. }
        | TackyInstr::SignExtend { .. }
        | TackyInstr::ZeroExtend { .. }
        | TackyInstr::IntToDouble { .. }
        | TackyInstr::IntToFloat { .. }
        | TackyInstr::UIntToDouble { .. }
        | TackyInstr::UIntToFloat { .. }
        | TackyInstr::FloatToDouble { .. }
        | TackyInstr::DoubleToFloat { .. }
        | TackyInstr::GetAddress { .. }
        | TackyInstr::LoadLabelAddress(..)
        | TackyInstr::FrameAddress { .. }
        | TackyInstr::CopyFromOffset { .. }
        | TackyInstr::AddPtr { .. } => true,
        TackyInstr::Binary { op, .. } => !matches!(op, TackyBinaryOp::Div | TackyBinaryOp::Mod),
        _ => false,
    }
}

fn is_memory_write(instr: &TackyInstr) -> bool {
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
