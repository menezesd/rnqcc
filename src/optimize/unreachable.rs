use crate::types::TackyInstr;
use std::collections::{HashMap, HashSet, VecDeque};

pub(super) fn unreachable_code_elimination(
    instructions: Vec<TackyInstr>,
) -> (Vec<TackyInstr>, bool) {
    let mut result = instructions;
    let mut changed = false;
    loop {
        let (next, did_change) = unreachable_code_pass(result);
        result = next;
        if !did_change {
            break;
        }
        changed = true;
    }
    (result, changed)
}

fn unreachable_code_pass(instructions: Vec<TackyInstr>) -> (Vec<TackyInstr>, bool) {
    let (threaded, threaded_changed) = thread_jump_chains(instructions);
    let cfg = crate::cfg::Cfg::build(threaded);
    let mut reachable_blocks = HashSet::with_capacity(cfg.blocks.len());
    let mut worklist = VecDeque::with_capacity(cfg.blocks.len());
    if !cfg.blocks.is_empty() {
        reachable_blocks.insert(0usize);
        worklist.push_back(0usize);
    }
    while let Some(block_id) = worklist.pop_front() {
        for successor in &cfg.blocks[block_id].successors {
            let crate::cfg::NodeId::Block(successor_id) = successor else {
                continue;
            };
            if reachable_blocks.insert(*successor_id) {
                worklist.push_back(*successor_id);
            }
        }
    }

    let total_instructions: usize = cfg
        .blocks
        .iter()
        .map(|block| block.instructions.len())
        .sum();
    let mut result = Vec::with_capacity(total_instructions);
    let mut changed = false;
    for block in cfg.blocks {
        if !reachable_blocks.contains(&block.id) {
            changed = true;
            continue;
        }
        for instr in block.instructions {
            if !matches!(instr, TackyInstr::Nop) {
                result.push(instr);
            } else {
                changed = true;
            }
        }
    }

    let (cleaned, cleaned_changed) = remove_redundant_fallthrough_jumps(result);
    (cleaned, changed || cleaned_changed || threaded_changed)
}

fn remove_redundant_fallthrough_jumps(instructions: Vec<TackyInstr>) -> (Vec<TackyInstr>, bool) {
    let mut cleaned = Vec::with_capacity(instructions.len());
    let mut changed = false;
    let mut iter = instructions.into_iter().peekable();
    while let Some(instr) = iter.next() {
        if jump_target(&instr).is_some_and(
            |target| matches!(iter.peek(), Some(TackyInstr::Label(label)) if label == target),
        ) {
            changed = true;
            continue;
        }
        cleaned.push(instr);
    }
    (cleaned, changed)
}

fn thread_jump_chains(instructions: Vec<TackyInstr>) -> (Vec<TackyInstr>, bool) {
    let label_positions = label_positions(&instructions);
    let resolved_targets = resolved_jump_targets(&instructions, &label_positions);
    let mut threaded = Vec::with_capacity(instructions.len());
    let mut changed = false;

    for mut instr in instructions {
        match &mut instr {
            TackyInstr::Jump(target)
            | TackyInstr::NonlocalJump(target)
            | TackyInstr::JumpIfZero(_, target)
            | TackyInstr::JumpIfNotZero(_, target) => {
                if let Some(resolved) = resolved_targets.get(target.as_str()) {
                    if resolved != target {
                        *target = resolved.clone();
                        changed = true;
                    }
                }
            }
            _ => {}
        }
        threaded.push(instr);
    }

    (threaded, changed)
}

fn resolved_jump_targets(
    instructions: &[TackyInstr],
    label_positions: &HashMap<String, usize>,
) -> HashMap<String, String> {
    let mut resolved = HashMap::new();
    for label in label_positions.keys() {
        if let Some(target) = resolve_jump_target(label, label_positions, instructions) {
            resolved.insert(label.clone(), target);
        }
    }
    resolved
}

fn label_positions(instructions: &[TackyInstr]) -> HashMap<String, usize> {
    let mut positions = HashMap::new();
    for (idx, instr) in instructions.iter().enumerate() {
        if let TackyInstr::Label(label) = instr {
            positions.entry(label.clone()).or_insert(idx);
        }
    }
    positions
}

fn resolve_jump_target(
    target: &str,
    label_positions: &HashMap<String, usize>,
    instructions: &[TackyInstr],
) -> Option<String> {
    let mut visited = HashSet::<&str>::new();
    let mut current = target;

    loop {
        if !visited.insert(current) {
            return None;
        }
        let &label_idx = label_positions.get(current)?;
        let mut next_idx = label_idx + 1;
        while matches!(instructions.get(next_idx), Some(TackyInstr::Label(_))) {
            next_idx += 1;
        }
        let Some(TackyInstr::Jump(next_target)) = instructions.get(next_idx) else {
            return Some(current.to_string());
        };
        current = next_target.as_str();
    }
}

fn jump_target(instr: &TackyInstr) -> Option<&str> {
    match instr {
        TackyInstr::Jump(target)
        | TackyInstr::JumpIfZero(_, target)
        | TackyInstr::JumpIfNotZero(_, target) => Some(target.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_threading_terminates_on_cyclic_label_chain() {
        let instructions = vec![
            TackyInstr::Label("first".to_string()),
            TackyInstr::Jump("second".to_string()),
            TackyInstr::Label("second".to_string()),
            TackyInstr::Jump("first".to_string()),
        ];

        let (optimized, changed) = thread_jump_chains(instructions.clone());

        assert!(!changed);
        assert_eq!(optimized, instructions);
    }
}
