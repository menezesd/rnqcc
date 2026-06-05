use crate::types::*;
use std::collections::{HashMap, HashSet, VecDeque};

// ============================================================
// Register Sets
// ============================================================

// GP allocatable registers (k=12): caller-saved first (colors 0-6), callee-saved last (7-11)
const GP_COLOR_ORDER: [Reg; 12] = [
    Reg::AX,
    Reg::CX,
    Reg::DX,
    Reg::DI,
    Reg::SI,
    Reg::R8,
    Reg::R9,
    Reg::BX,
    Reg::R12,
    Reg::R13,
    Reg::R14,
    Reg::R15,
];
const GP_K: usize = 12;

// XMM allocatable registers (k=14): all caller-saved
const XMM_COLOR_ORDER: [XmmReg; 14] = [
    XmmReg::XMM0,
    XmmReg::XMM1,
    XmmReg::XMM2,
    XmmReg::XMM3,
    XmmReg::XMM4,
    XmmReg::XMM5,
    XmmReg::XMM6,
    XmmReg::XMM7,
    XmmReg::XMM8,
    XmmReg::XMM9,
    XmmReg::XMM10,
    XmmReg::XMM11,
    XmmReg::XMM12,
    XmmReg::XMM13,
];
const XMM_K: usize = 14;

pub const GP_CALLEE_SAVED: [Reg; 5] = [Reg::BX, Reg::R12, Reg::R13, Reg::R14, Reg::R15];

const ARG_INT_REGS: [Reg; 6] = [Reg::DI, Reg::SI, Reg::DX, Reg::CX, Reg::R8, Reg::R9];
const ARG_SSE_REGS: [XmmReg; 8] = [
    XmmReg::XMM0,
    XmmReg::XMM1,
    XmmReg::XMM2,
    XmmReg::XMM3,
    XmmReg::XMM4,
    XmmReg::XMM5,
    XmmReg::XMM6,
    XmmReg::XMM7,
];

const CALLER_SAVED_GP: [Reg; 9] = [
    Reg::AX,
    Reg::CX,
    Reg::DX,
    Reg::DI,
    Reg::SI,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
];

const ALL_XMM: [XmmReg; 16] = [
    XmmReg::XMM0,
    XmmReg::XMM1,
    XmmReg::XMM2,
    XmmReg::XMM3,
    XmmReg::XMM4,
    XmmReg::XMM5,
    XmmReg::XMM6,
    XmmReg::XMM7,
    XmmReg::XMM8,
    XmmReg::XMM9,
    XmmReg::XMM10,
    XmmReg::XMM11,
    XmmReg::XMM12,
    XmmReg::XMM13,
    XmmReg::XMM14,
    XmmReg::XMM15,
];

// ============================================================
// Register Identifier (node in interference graph)
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RegId {
    Gp(Reg),
    Xmm(XmmReg),
    Pseudo(String),
}

// ============================================================
// Assembly CFG for Liveness Analysis
// ============================================================

struct AsmBlock {
    start: usize,
    end: usize,        // exclusive
    succs: Vec<usize>, // block indices
    preds: Vec<usize>,
    reaches_exit: bool, // has Ret as last instruction
}

fn build_asm_cfg(instrs: &[AsmInstr]) -> Vec<AsmBlock> {
    if instrs.is_empty() {
        return vec![];
    }

    // Identify block boundaries: labels start blocks; jmp/jmpcc/ret end blocks
    let mut leaders: Vec<usize> = vec![0]; // first instruction is always a leader
    let mut leader_set: HashSet<usize> = HashSet::from([0]);
    for (i, instr) in instrs.iter().enumerate() {
        match instr {
            AsmInstr::Label(_) if leader_set.insert(i) => {
                leaders.push(i);
            }
            AsmInstr::Jmp(_)
            | AsmInstr::NonlocalJmp(_)
            | AsmInstr::JmpCC(_, _)
            | AsmInstr::JmpIndirect(_)
            | AsmInstr::Ret
            | AsmInstr::Unreachable
                if i + 1 < instrs.len() && leader_set.insert(i + 1) =>
            {
                leaders.push(i + 1);
            }
            _ => {}
        }
    }
    leaders.sort();
    leaders.dedup();

    // Create blocks
    let mut blocks: Vec<AsmBlock> = Vec::new();
    for w in leaders.windows(2) {
        blocks.push(AsmBlock {
            start: w[0],
            end: w[1],
            succs: vec![],
            preds: vec![],
            reaches_exit: false,
        });
    }
    blocks.push(AsmBlock {
        start: leaders.last().copied().unwrap_or(0),
        end: instrs.len(),
        succs: vec![],
        preds: vec![],
        reaches_exit: false,
    });

    // Build label → block index map
    let mut label_to_block: HashMap<String, usize> = HashMap::new();
    for (bi, block) in blocks.iter().enumerate() {
        if let AsmInstr::Label(label) = &instrs[block.start] {
            label_to_block.insert(label.clone(), bi);
        }
    }

    // Add edges
    let num = blocks.len();
    for bi in 0..num {
        let last_idx = blocks[bi].end - 1;
        let last = &instrs[last_idx];
        match last {
            AsmInstr::Ret | AsmInstr::Unreachable | AsmInstr::NonlocalJmp(_) => {
                blocks[bi].reaches_exit = true;
            }
            AsmInstr::Jmp(label) => {
                if let Some(&ti) = label_to_block.get(label) {
                    blocks[bi].succs.push(ti);
                    blocks[ti].preds.push(bi);
                }
            }
            AsmInstr::JmpCC(_, label) => {
                if let Some(&ti) = label_to_block.get(label) {
                    blocks[bi].succs.push(ti);
                    blocks[ti].preds.push(bi);
                }
                // Fall-through
                if bi + 1 < num {
                    blocks[bi].succs.push(bi + 1);
                    blocks[bi + 1].preds.push(bi);
                }
            }
            AsmInstr::JmpIndirect(_) => {
                for ti in label_to_block.values().copied() {
                    blocks[bi].succs.push(ti);
                    blocks[ti].preds.push(bi);
                }
            }
            _ => {
                // Fall-through
                if bi + 1 < num {
                    blocks[bi].succs.push(bi + 1);
                    blocks[bi + 1].preds.push(bi);
                }
            }
        }
    }

    blocks
}

// ============================================================
// Used & Updated Sets
// ============================================================

fn operand_reads(op: &AsmOperand) -> Vec<RegId> {
    match op {
        AsmOperand::Reg(r) => vec![RegId::Gp(*r)],
        AsmOperand::Xmm(x) => vec![RegId::Xmm(*x)],
        AsmOperand::Pseudo(name) => vec![RegId::Pseudo(name.clone())],
        AsmOperand::Indexed(base, idx, _) => vec![RegId::Gp(*base), RegId::Gp(*idx)],
        _ => vec![], // Imm, Stack, Data, PseudoMem
    }
}

fn operand_writes(op: &AsmOperand) -> Vec<RegId> {
    match op {
        AsmOperand::Reg(r) => vec![RegId::Gp(*r)],
        AsmOperand::Xmm(x) => vec![RegId::Xmm(*x)],
        AsmOperand::Pseudo(name) => vec![RegId::Pseudo(name.clone())],
        _ => vec![], // Memory destinations don't write registers
    }
}

fn find_used_and_updated(instr: &AsmInstr) -> (Vec<RegId>, Vec<RegId>) {
    match instr {
        AsmInstr::Mov(_, src, dst) => {
            let mut used = operand_reads(src);
            // If dst is Indexed, base/idx are used for addressing
            if let AsmOperand::Indexed(base, idx, _) = dst {
                used.push(RegId::Gp(*base));
                used.push(RegId::Gp(*idx));
            }
            (used, operand_writes(dst))
        }
        AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
            (operand_reads(src), operand_writes(dst))
        }
        AsmInstr::Binary(_, _, src, dst) => {
            let mut used = operand_reads(src);
            used.extend(operand_reads(dst));
            (used, operand_writes(dst))
        }
        AsmInstr::Unary(_, _, dst) => (operand_reads(dst), operand_writes(dst)),
        AsmInstr::Cmp(_, src, dst) => {
            let mut used = operand_reads(src);
            used.extend(operand_reads(dst));
            (used, vec![])
        }
        AsmInstr::SetCC(_, dst) => (vec![], operand_writes(dst)),
        AsmInstr::Push(val) => (operand_reads(val), vec![]),
        AsmInstr::Pop(reg) => (vec![], vec![RegId::Gp(*reg)]),
        AsmInstr::MulFull(_, divisor) | AsmInstr::Idiv(_, divisor) | AsmInstr::Div(_, divisor) => {
            let mut used = operand_reads(divisor);
            used.push(RegId::Gp(Reg::AX));
            used.push(RegId::Gp(Reg::DX));
            (used, vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DX)])
        }
        AsmInstr::Cdq(_) => (vec![RegId::Gp(Reg::AX)], vec![RegId::Gp(Reg::DX)]),
        AsmInstr::Call(_, int_regs, sse_regs, _, _) => {
            let mut used = Vec::new();
            for reg in ARG_INT_REGS.iter().take(*int_regs) {
                used.push(RegId::Gp(*reg));
            }
            for reg in ARG_SSE_REGS.iter().take(*sse_regs) {
                used.push(RegId::Xmm(*reg));
            }
            let mut updated = Vec::new();
            for r in &CALLER_SAVED_GP {
                updated.push(RegId::Gp(*r));
            }
            for x in &ALL_XMM {
                updated.push(RegId::Xmm(*x));
            }
            (used, updated)
        }
        AsmInstr::X86SetVarargsXmmCount(_) => (vec![], vec![RegId::Gp(Reg::AX)]),
        AsmInstr::JmpIndirect(target) => (operand_reads(target), vec![]),
        AsmInstr::LoadLabelAddress(_, dst) => (vec![], operand_writes(dst)),
        AsmInstr::Cvtsi2sd(_, src, dst)
        | AsmInstr::Cvtsi2ss(_, src, dst)
        | AsmInstr::Cvttsd2si(_, src, dst)
        | AsmInstr::Cvttss2si(_, src, dst)
        | AsmInstr::AArch64UIntToDouble(_, src, dst)
        | AsmInstr::AArch64UIntToFloat(_, src, dst)
        | AsmInstr::AArch64DoubleToUInt(_, src, dst)
        | AsmInstr::AArch64FloatToUInt(_, src, dst) => (operand_reads(src), operand_writes(dst)),
        AsmInstr::Cvtss2sd(src, dst)
        | AsmInstr::Cvtsd2ss(src, dst)
        | AsmInstr::AArch64FloatToDouble(src, dst)
        | AsmInstr::AArch64DoubleToFloat(src, dst) => (operand_reads(src), operand_writes(dst)),
        AsmInstr::X87Load(_, src) => (operand_reads(src), vec![]),
        AsmInstr::X87Store(dst) => (vec![], operand_writes(dst)),
        AsmInstr::X87StoreFloat(_, _) => (vec![], vec![]),
        AsmInstr::X87StoreInt(_, dst) => (vec![], operand_writes(dst)),
        AsmInstr::X87LoadIndirect(_, reg) | AsmInstr::X87StoreIndirect(reg) => {
            (vec![RegId::Gp(*reg)], vec![])
        }
        AsmInstr::X87UnaryNeg | AsmInstr::X87Binary(_) | AsmInstr::X87Compare => (vec![], vec![]),
        AsmInstr::Lea(src, dst) => {
            // Lea reads address components from src, writes result to dst
            let used = match src {
                AsmOperand::Pseudo(name) => vec![RegId::Pseudo(name.clone())],
                AsmOperand::Indexed(base, idx, _) => vec![RegId::Gp(*base), RegId::Gp(*idx)],
                AsmOperand::Stack(_) => vec![],
                _ => operand_reads(src),
            };
            (used, operand_writes(dst))
        }
        AsmInstr::LoadIndirect(_, reg, dst) => (vec![RegId::Gp(*reg)], operand_writes(dst)),
        AsmInstr::StoreIndirect(_, src, reg) => {
            let mut used = operand_reads(src);
            used.push(RegId::Gp(*reg));
            (used, vec![])
        }
        AsmInstr::CopyToStackArg { src_ptr, .. } => {
            let used = operand_reads(src_ptr);
            (
                used,
                vec![RegId::Gp(Reg::SI), RegId::Gp(Reg::DI), RegId::Gp(Reg::CX)],
            )
        }
        AsmInstr::CopyFromStackArg { .. } => (
            vec![],
            vec![RegId::Gp(Reg::SI), RegId::Gp(Reg::DI), RegId::Gp(Reg::CX)],
        ),
        AsmInstr::BuiltinSetjmp { buf, dst, .. } => (operand_reads(buf), operand_writes(dst)),
        AsmInstr::BuiltinLongjmp { buf, value } => {
            let mut used = operand_reads(buf);
            used.extend(operand_reads(value));
            (used, vec![])
        }
        AsmInstr::AtomicRmw(_, _, return_old, dst) => {
            let used = vec![RegId::Gp(Reg::R10), RegId::Gp(Reg::R11)];
            let mut updated = operand_writes(dst);
            if *return_old {
                updated.push(RegId::Gp(Reg::AX));
                updated.push(RegId::Gp(Reg::R12));
            }
            (used, updated)
        }
        AsmInstr::AtomicExchange(_, dst) => {
            let used = vec![RegId::Gp(Reg::R10), RegId::Gp(Reg::R11)];
            (used, operand_writes(dst))
        }
        AsmInstr::AtomicCompareExchange(_, dst) => {
            let used = vec![
                RegId::Gp(Reg::R10),
                RegId::Gp(Reg::R11),
                RegId::Gp(Reg::R12),
            ];
            let mut updated = operand_writes(dst);
            updated.push(RegId::Gp(Reg::AX));
            updated.push(RegId::Gp(Reg::R10));
            (used, updated)
        }
        AsmInstr::AtomicCompareSwap(_, return_old, dst) => {
            let used = vec![
                RegId::Gp(Reg::R10),
                RegId::Gp(Reg::R11),
                RegId::Gp(Reg::R12),
            ];
            let mut updated = operand_writes(dst);
            updated.push(RegId::Gp(Reg::AX));
            if !return_old {
                updated.push(RegId::Gp(Reg::R10));
            }
            (used, updated)
        }
        // Terminators and others: no register effects for liveness
        AsmInstr::Ret
        | AsmInstr::Unreachable
        | AsmInstr::Jmp(_)
        | AsmInstr::NonlocalJmp(_)
        | AsmInstr::JmpCC(_, _)
        | AsmInstr::Label(_)
        | AsmInstr::AllocateStack(_)
        | AsmInstr::DeallocateStack(_)
        | AsmInstr::AArch64AddPtr(..)
        | AsmInstr::AArch64LoadAdjusted(..)
        | AsmInstr::AArch64StoreOutgoingArg(..)
        | AsmInstr::AArch64Rem(..)
        | AsmInstr::AArch64SaveLink(..)
        | AsmInstr::AArch64RestoreLink(..)
        | AsmInstr::AArch64AllocateLargeStack(..)
        | AsmInstr::AArch64DeallocateLargeStack(..)
        | AsmInstr::AArch64StoreLargeLocalBase { .. }
        | AsmInstr::AtomicFence => (vec![], vec![]),
    }
}

/// Check if instruction is a plain Mov between register-allocatable operands.
/// Returns (src_id, dst_id) if so.
fn mov_operands(instr: &AsmInstr) -> Option<(RegId, RegId)> {
    if let AsmInstr::Mov(_, src, dst) = instr {
        let s = match src {
            AsmOperand::Reg(r) => Some(RegId::Gp(*r)),
            AsmOperand::Xmm(x) => Some(RegId::Xmm(*x)),
            AsmOperand::Pseudo(n) => Some(RegId::Pseudo(n.clone())),
            _ => None,
        };
        let d = match dst {
            AsmOperand::Reg(r) => Some(RegId::Gp(*r)),
            AsmOperand::Xmm(x) => Some(RegId::Xmm(*x)),
            AsmOperand::Pseudo(n) => Some(RegId::Pseudo(n.clone())),
            _ => None,
        };
        if let (Some(s), Some(d)) = (s, d) {
            if s != d {
                return Some((s, d));
            }
        }
    }
    None
}

// ============================================================
// Liveness Analysis (backward dataflow)
// ============================================================

fn liveness_analysis(instrs: &[AsmInstr], exit_live: &HashSet<RegId>) -> Vec<HashSet<RegId>> {
    let blocks = build_asm_cfg(instrs);
    if blocks.is_empty() {
        return vec![HashSet::new(); instrs.len()];
    }

    let num_blocks = blocks.len();
    let mut block_live_in: Vec<HashSet<RegId>> = vec![HashSet::new(); num_blocks];
    let mut worklist: VecDeque<usize> = (0..num_blocks).rev().collect();
    let mut queued: Vec<bool> = vec![true; num_blocks];

    while let Some(bi) = worklist.pop_front() {
        queued[bi] = false;
        // Meet: union of successors' live_in
        let mut live_out: HashSet<RegId> = HashSet::new();
        for &si in &blocks[bi].succs {
            live_out.extend(block_live_in[si].iter().cloned());
        }
        if blocks[bi].reaches_exit {
            live_out.extend(exit_live.iter().cloned());
        }

        // Transfer: backward through instructions
        let mut live = live_out;
        for i in (blocks[bi].start..blocks[bi].end).rev() {
            let (used, updated) = find_used_and_updated(&instrs[i]);
            for u in &updated {
                live.remove(u);
            }
            for u in &used {
                live.insert(u.clone());
            }
        }

        if live != block_live_in[bi] {
            block_live_in[bi] = live;
            for &pi in &blocks[bi].preds {
                if !queued[pi] {
                    worklist.push_back(pi);
                    queued[pi] = true;
                }
            }
        }
    }

    // Compute per-instruction live_after
    let mut live_after: Vec<HashSet<RegId>> = vec![HashSet::new(); instrs.len()];
    for (bi, block) in blocks.iter().enumerate().take(num_blocks) {
        // Recompute live_out for this block
        let mut live: HashSet<RegId> = HashSet::new();
        for &si in &block.succs {
            live.extend(block_live_in[si].iter().cloned());
        }
        if block.reaches_exit {
            live.extend(exit_live.iter().cloned());
        }

        // Backward pass to fill live_after
        for i in (blocks[bi].start..blocks[bi].end).rev() {
            live_after[i] = live.clone();
            let (used, updated) = find_used_and_updated(&instrs[i]);
            for u in &updated {
                live.remove(u);
            }
            for u in &used {
                live.insert(u.clone());
            }
        }
    }

    live_after
}

// ============================================================
// Interference Graph
// ============================================================

struct Graph {
    adj: HashMap<RegId, HashSet<RegId>>,
    spill_cost: HashMap<RegId, f64>,
    color: HashMap<RegId, Option<usize>>,
}

impl Graph {
    fn new() -> Self {
        Graph {
            adj: HashMap::new(),
            spill_cost: HashMap::new(),
            color: HashMap::new(),
        }
    }

    fn add_node(&mut self, id: RegId, cost: f64) {
        self.adj.entry(id.clone()).or_default();
        self.spill_cost.insert(id.clone(), cost);
        self.color.insert(id, None);
    }

    fn add_edge(&mut self, a: &RegId, b: &RegId) {
        if a == b {
            return;
        }
        if !self.adj.contains_key(a) || !self.adj.contains_key(b) {
            return;
        }
        if let Some(edges) = self.adj.get_mut(a) {
            edges.insert(b.clone());
        }
        if let Some(edges) = self.adj.get_mut(b) {
            edges.insert(a.clone());
        }
    }

    fn has_node(&self, id: &RegId) -> bool {
        self.adj.contains_key(id)
    }

    fn are_neighbors(&self, a: &RegId, b: &RegId) -> bool {
        self.adj.get(a).map(|s| s.contains(b)).unwrap_or(false)
    }
}

fn build_interference_graph(
    instrs: &[AsmInstr],
    live_after: &[HashSet<RegId>],
    candidates: &HashSet<String>,
    hard_reg_ids: &[RegId],
    _k: usize,
) -> Graph {
    let mut graph = Graph::new();

    // Add hard register nodes (pre-colored, infinite spill cost)
    for (color_idx, hr) in hard_reg_ids.iter().enumerate() {
        graph.add_node(hr.clone(), f64::INFINITY);
        graph.color.insert(hr.clone(), Some(color_idx));
    }
    // Hard registers are all connected to each other
    for i in 0..hard_reg_ids.len() {
        for j in (i + 1)..hard_reg_ids.len() {
            graph.add_edge(&hard_reg_ids[i], &hard_reg_ids[j]);
        }
    }

    // Add pseudo-register nodes (candidates only)
    // Count occurrences for spill cost
    let mut occurrence_count: HashMap<String, f64> = HashMap::new();
    for instr in instrs {
        let (used, updated) = find_used_and_updated(instr);
        for id in used.iter().chain(updated.iter()) {
            if let RegId::Pseudo(name) = id {
                if candidates.contains(name) {
                    *occurrence_count.entry(name.clone()).or_default() += 1.0;
                }
            }
        }
        if let AsmInstr::LoadLabelAddress(_, AsmOperand::Pseudo(name)) = instr {
            if candidates.contains(name) {
                *occurrence_count.entry(name.clone()).or_default() += 1000.0;
            }
        }
    }
    for name in candidates {
        let cost = occurrence_count.get(name).copied().unwrap_or(0.0);
        graph.add_node(RegId::Pseudo(name.clone()), cost);
    }

    // Add interference edges from liveness
    for (i, instr) in instrs.iter().enumerate() {
        let (_, updated) = find_used_and_updated(instr);
        let is_mov = mov_operands(instr);
        let mov_src = is_mov.as_ref().map(|(s, _)| s);

        for u in &updated {
            if !graph.has_node(u) {
                continue;
            }
            for l in &live_after[i] {
                if u == l {
                    continue;
                }
                if !graph.has_node(l) {
                    continue;
                }
                // Mov exception: don't add edge between dst and src
                if let Some(src) = mov_src {
                    if l == src {
                        continue;
                    }
                }
                graph.add_edge(u, l);
            }
        }
    }

    graph
}

// ============================================================
// Graph Coloring (simplify-select)
// ============================================================

fn color_graph(graph: &mut Graph, hard_reg_ids: &[RegId], k: usize) {
    let hard_set: HashSet<RegId> = hard_reg_ids.iter().cloned().collect();

    // Collect pseudo nodes
    let pseudo_nodes: Vec<RegId> = graph
        .adj
        .keys()
        .filter(|id| !hard_set.contains(id))
        .cloned()
        .collect();

    if pseudo_nodes.is_empty() {
        return;
    }

    // Simplify: push nodes to stack. Keep mutable degrees so each prune is
    // proportional to the removed node's neighbors, not to all remaining nodes.
    let mut stack: Vec<RegId> = Vec::new();
    let mut remaining: HashSet<RegId> = pseudo_nodes.iter().cloned().collect();
    let mut degree: HashMap<RegId, usize> = pseudo_nodes
        .iter()
        .map(|node| {
            let degree = graph.adj.get(node).map(|nbrs| nbrs.len()).unwrap_or(0);
            (node.clone(), degree)
        })
        .collect();
    let mut low_degree: VecDeque<RegId> = pseudo_nodes
        .iter()
        .filter(|node| degree.get(*node).copied().unwrap_or(0) < k)
        .cloned()
        .collect();

    while !remaining.is_empty() {
        let node = loop {
            match low_degree.pop_front() {
                Some(candidate)
                    if remaining.contains(&candidate)
                        && degree.get(&candidate).copied().unwrap_or(0) < k =>
                {
                    break candidate;
                }
                Some(_) => continue,
                None => {
                    break remaining
                        .iter()
                        .min_by(|a, b| {
                            let cost_a = graph.spill_cost.get(*a).copied().unwrap_or(0.0);
                            let cost_b = graph.spill_cost.get(*b).copied().unwrap_or(0.0);
                            let deg_a = degree.get(*a).copied().unwrap_or(0).max(1) as f64;
                            let deg_b = degree.get(*b).copied().unwrap_or(0).max(1) as f64;
                            (cost_a / deg_a)
                                .partial_cmp(&(cost_b / deg_b))
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .cloned()
                        .expect("remaining pseudo set is non-empty");
                }
            }
        };

        remaining.remove(&node);
        stack.push(node.clone());

        if let Some(neighbors) = graph.adj.get(&node) {
            for neighbor in neighbors {
                if !remaining.contains(neighbor) {
                    continue;
                }
                let Some(neighbor_degree) = degree.get_mut(neighbor) else {
                    continue;
                };
                *neighbor_degree = neighbor_degree.saturating_sub(1);
                if *neighbor_degree < k {
                    low_degree.push_back(neighbor.clone());
                }
            }
        }
    }

    // Select: pop from stack and assign colors
    while let Some(node) = stack.pop() {
        let mut used_colors = vec![false; k];
        if let Some(neighbors) = graph.adj.get(&node) {
            for neighbor in neighbors {
                if let Some(color) = graph.color.get(neighbor).and_then(|color| *color) {
                    if color < k {
                        used_colors[color] = true;
                    }
                }
            }
        }

        let color = used_colors.iter().position(|used| !*used);
        graph.color.insert(node, color);
    }
}

// ============================================================
// Coalescing
// ============================================================

struct UnionFind {
    parent: HashMap<RegId, RegId>,
}

impl UnionFind {
    fn new() -> Self {
        UnionFind {
            parent: HashMap::new(),
        }
    }

    fn find(&self, x: &RegId) -> RegId {
        let mut current = x.clone();
        while let Some(p) = self.parent.get(&current) {
            if p == &current {
                break;
            }
            current = p.clone();
        }
        current
    }

    fn union(&mut self, x: &RegId, y: &RegId) {
        let rx = self.find(x);
        let ry = self.find(y);
        if rx != ry {
            self.parent.insert(rx, ry);
        }
    }
}

fn briggs_test(graph: &Graph, x: &RegId, y: &RegId, k: usize, pruned: &HashSet<RegId>) -> bool {
    // Count neighbors of merged node with significant degree (>= k)
    let x_nbrs = graph.adj.get(x).cloned().unwrap_or_default();
    let y_nbrs = graph.adj.get(y).cloned().unwrap_or_default();
    let mut merged_nbrs: HashSet<RegId> = HashSet::new();
    for n in x_nbrs.iter().chain(y_nbrs.iter()) {
        if n != x && n != y && !pruned.contains(n) {
            merged_nbrs.insert(n.clone());
        }
    }
    let significant = merged_nbrs
        .iter()
        .filter(|n| {
            let deg = graph
                .adj
                .get(*n)
                .map(|s| s.iter().filter(|nn| !pruned.contains(nn)).count())
                .unwrap_or(0);
            deg >= k
        })
        .count();
    significant < k
}

fn george_test(
    graph: &Graph,
    pseudo: &RegId,
    hard: &RegId,
    k: usize,
    pruned: &HashSet<RegId>,
) -> bool {
    // For each neighbor of pseudo: either it already interferes with hard, or it has degree < k
    let nbrs = graph.adj.get(pseudo).cloned().unwrap_or_default();
    for n in &nbrs {
        if n == hard || pruned.contains(n) {
            continue;
        }
        let interferes_with_hard = graph.are_neighbors(n, hard);
        let deg = graph
            .adj
            .get(n)
            .map(|s| s.iter().filter(|nn| !pruned.contains(nn)).count())
            .unwrap_or(0);
        if !interferes_with_hard && deg >= k {
            return false;
        }
    }
    true
}

fn is_hard_reg(id: &RegId) -> bool {
    matches!(id, RegId::Gp(_) | RegId::Xmm(_))
}

fn coalesce_pass(
    instrs: &[AsmInstr],
    graph: &mut Graph,
    _hard_reg_ids: &[RegId],
    k: usize,
) -> Option<UnionFind> {
    let pruned: HashSet<RegId> = HashSet::new(); // no nodes pruned during coalescing

    let mut uf = UnionFind::new();
    let mut coalesced_any = false;

    // Collect Mov instructions that are coalescing candidates
    for instr in instrs {
        if let Some((src, dst)) = mov_operands(instr) {
            let src_r = uf.find(&src);
            let dst_r = uf.find(&dst);
            if src_r == dst_r {
                continue;
            }
            if !graph.has_node(&src_r) || !graph.has_node(&dst_r) {
                continue;
            }
            if graph.are_neighbors(&src_r, &dst_r) {
                continue;
            }

            // Determine which is pseudo and which is hard (or both pseudo)
            let can_coalesce = if is_hard_reg(&src_r) && is_hard_reg(&dst_r) {
                false // Can't coalesce two hard regs
            } else if is_hard_reg(&src_r) {
                // George test: coalesce pseudo dst_r into hard src_r
                george_test(graph, &dst_r, &src_r, k, &pruned)
            } else if is_hard_reg(&dst_r) {
                // George test: coalesce pseudo src_r into hard dst_r
                george_test(graph, &src_r, &dst_r, k, &pruned)
            } else {
                // Both pseudos: Briggs test
                briggs_test(graph, &src_r, &dst_r, k, &pruned)
            };

            if can_coalesce {
                // Merge src_r into dst_r (if one is hard reg, it should be the target)
                let (from, into) = if is_hard_reg(&dst_r) {
                    (src_r.clone(), dst_r.clone())
                } else if is_hard_reg(&src_r) {
                    (dst_r.clone(), src_r.clone())
                } else {
                    (src_r.clone(), dst_r.clone())
                };

                // Transfer edges from `from` to `into`
                let from_nbrs: Vec<RegId> = graph
                    .adj
                    .get(&from)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                for n in &from_nbrs {
                    if n != &into {
                        graph.add_edge(&into, n);
                    }
                    // Remove edge from→n
                    if let Some(ns) = graph.adj.get_mut(n) {
                        ns.remove(&from);
                    }
                }
                // Remove `from` from graph
                graph.adj.remove(&from);
                graph.spill_cost.remove(&from);
                graph.color.remove(&from);
                // Also remove `from` from `into`'s neighbors
                if let Some(ns) = graph.adj.get_mut(&into) {
                    ns.remove(&from);
                }

                uf.union(&from, &into);
                coalesced_any = true;
            }
        }
    }

    if coalesced_any {
        Some(uf)
    } else {
        None
    }
}

fn rewrite_coalesced(instrs: &mut Vec<AsmInstr>, uf: &UnionFind) {
    fn rewrite_op(op: &mut AsmOperand, uf: &UnionFind) {
        match op {
            AsmOperand::Pseudo(name) => {
                let root = uf.find(&RegId::Pseudo(name.clone()));
                match root {
                    RegId::Pseudo(new_name) => *name = new_name,
                    RegId::Gp(reg) => *op = AsmOperand::Reg(reg),
                    RegId::Xmm(xmm) => *op = AsmOperand::Xmm(xmm),
                }
            }
            AsmOperand::Reg(reg) => {
                let root = uf.find(&RegId::Gp(*reg));
                if let RegId::Gp(new_reg) = root {
                    *reg = new_reg;
                }
            }
            AsmOperand::Xmm(xmm) => {
                let root = uf.find(&RegId::Xmm(*xmm));
                if let RegId::Xmm(new_xmm) = root {
                    *xmm = new_xmm;
                }
            }
            _ => {}
        }
    }

    for instr in instrs.iter_mut() {
        match instr {
            AsmInstr::Mov(_, src, dst) | AsmInstr::Cmp(_, src, dst) => {
                rewrite_op(src, uf);
                rewrite_op(dst, uf);
            }
            AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
                rewrite_op(src, uf);
                rewrite_op(dst, uf);
            }
            AsmInstr::Binary(_, _, src, dst) => {
                rewrite_op(src, uf);
                rewrite_op(dst, uf);
            }
            AsmInstr::Unary(_, _, op) => {
                rewrite_op(op, uf);
            }
            AsmInstr::MulFull(_, op) | AsmInstr::Idiv(_, op) | AsmInstr::Div(_, op) => {
                rewrite_op(op, uf);
            }
            AsmInstr::SetCC(_, op) => {
                rewrite_op(op, uf);
            }
            AsmInstr::Push(op) => {
                rewrite_op(op, uf);
            }
            AsmInstr::JmpIndirect(target) => {
                rewrite_op(target, uf);
            }
            AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvtsi2ss(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Cvttss2si(_, src, dst) => {
                rewrite_op(src, uf);
                rewrite_op(dst, uf);
            }
            AsmInstr::Cvtss2sd(src, dst) | AsmInstr::Cvtsd2ss(src, dst) => {
                rewrite_op(src, uf);
                rewrite_op(dst, uf);
            }
            AsmInstr::X87Load(t, src) if *t != AsmType::LongDouble => {
                rewrite_op(src, uf);
            }
            AsmInstr::X87StoreInt(_, dst) => {
                rewrite_op(dst, uf);
            }
            AsmInstr::Lea(src, dst) => {
                rewrite_op(src, uf);
                rewrite_op(dst, uf);
            }
            AsmInstr::LoadLabelAddress(_, dst) => {
                rewrite_op(dst, uf);
            }
            AsmInstr::LoadIndirect(_, _, dst) => {
                rewrite_op(dst, uf);
            }
            AsmInstr::StoreIndirect(_, src, _) => {
                rewrite_op(src, uf);
            }
            AsmInstr::CopyToStackArg { src_ptr, .. } => {
                rewrite_op(src_ptr, uf);
            }
            AsmInstr::BuiltinSetjmp { buf, dst, .. } => {
                rewrite_op(buf, uf);
                rewrite_op(dst, uf);
            }
            AsmInstr::BuiltinLongjmp { buf, value } => {
                rewrite_op(buf, uf);
                rewrite_op(value, uf);
            }
            AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => {
                rewrite_op(dst, uf);
            }
            _ => {}
        }
    }

    // Remove Mov where src == dst
    instrs.retain(|instr| {
        if let AsmInstr::Mov(_, src, dst) = instr {
            !operands_equal(src, dst)
        } else {
            true
        }
    });
}

fn operands_equal(a: &AsmOperand, b: &AsmOperand) -> bool {
    match (a, b) {
        (AsmOperand::Reg(r1), AsmOperand::Reg(r2)) => r1 == r2,
        (AsmOperand::Xmm(x1), AsmOperand::Xmm(x2)) => x1 == x2,
        (AsmOperand::Pseudo(n1), AsmOperand::Pseudo(n2)) => n1 == n2,
        _ => false,
    }
}

// ============================================================
// Apply Coloring: replace pseudos with hard registers
// ============================================================

fn build_color_map(graph: &Graph, hard_reg_ids: &[RegId]) -> HashMap<String, RegId> {
    // Map color index → hard register
    let mut color_to_reg: HashMap<usize, RegId> = HashMap::new();
    for (i, hr) in hard_reg_ids.iter().enumerate() {
        color_to_reg.insert(i, hr.clone());
    }

    let mut map = HashMap::new();
    for (id, color_opt) in &graph.color {
        if let RegId::Pseudo(name) = id {
            if let Some(color) = color_opt {
                if let Some(reg) = color_to_reg.get(color) {
                    map.insert(name.clone(), reg.clone());
                }
            }
        }
    }
    map
}

fn apply_register_map(instrs: &mut Vec<AsmInstr>, map: &HashMap<String, RegId>) {
    fn replace_op(op: &mut AsmOperand, map: &HashMap<String, RegId>) {
        if let AsmOperand::Pseudo(name) = op {
            if let Some(reg_id) = map.get(name) {
                match reg_id {
                    RegId::Gp(r) => *op = AsmOperand::Reg(*r),
                    RegId::Xmm(x) => *op = AsmOperand::Xmm(*x),
                    _ => {}
                }
            }
        }
    }

    for instr in instrs.iter_mut() {
        match instr {
            AsmInstr::Mov(_, src, dst) | AsmInstr::Cmp(_, src, dst) => {
                replace_op(src, map);
                replace_op(dst, map);
            }
            AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
                replace_op(src, map);
                replace_op(dst, map);
            }
            AsmInstr::Binary(_, _, src, dst) => {
                replace_op(src, map);
                replace_op(dst, map);
            }
            AsmInstr::Unary(_, _, op) => {
                replace_op(op, map);
            }
            AsmInstr::MulFull(_, op) | AsmInstr::Idiv(_, op) | AsmInstr::Div(_, op) => {
                replace_op(op, map);
            }
            AsmInstr::SetCC(_, op) => {
                replace_op(op, map);
            }
            AsmInstr::Push(op) => {
                replace_op(op, map);
            }
            AsmInstr::JmpIndirect(target) => {
                replace_op(target, map);
            }
            AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvtsi2ss(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Cvttss2si(_, src, dst) => {
                replace_op(src, map);
                replace_op(dst, map);
            }
            AsmInstr::Cvtss2sd(src, dst) | AsmInstr::Cvtsd2ss(src, dst) => {
                replace_op(src, map);
                replace_op(dst, map);
            }
            AsmInstr::X87Load(t, src) if *t != AsmType::LongDouble => {
                replace_op(src, map);
            }
            AsmInstr::X87StoreInt(_, dst) => {
                replace_op(dst, map);
            }
            AsmInstr::Lea(src, dst) => {
                replace_op(src, map);
                replace_op(dst, map);
            }
            AsmInstr::LoadLabelAddress(_, dst) => {
                replace_op(dst, map);
            }
            AsmInstr::LoadIndirect(_, _, dst) => {
                replace_op(dst, map);
            }
            AsmInstr::StoreIndirect(_, src, _) => {
                replace_op(src, map);
            }
            AsmInstr::CopyToStackArg { src_ptr, .. } => {
                replace_op(src_ptr, map);
            }
            AsmInstr::BuiltinSetjmp { buf, dst, .. } => {
                replace_op(buf, map);
                replace_op(dst, map);
            }
            AsmInstr::BuiltinLongjmp { buf, value } => {
                replace_op(buf, map);
                replace_op(value, map);
            }
            AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => {
                replace_op(dst, map);
            }
            _ => {}
        }
    }

    // Remove Mov where src == dst after replacement
    instrs.retain(|instr| {
        if let AsmInstr::Mov(_, src, dst) = instr {
            !operands_equal(src, dst)
        } else {
            true
        }
    });
}

// ============================================================
// Public API
// ============================================================

pub struct RegAllocResult {
    pub callee_saved: Vec<Reg>,
}

pub fn allocate_registers(
    func: &mut AsmFunction,
    aliased: &HashSet<String>,
    types: &HashMap<String, CType>,
    arr_sizes: &HashMap<String, usize>,
    ret_regs: &[RegId],
    no_coalescing: bool,
) -> RegAllocResult {
    let exit_live: HashSet<RegId> = ret_regs.iter().cloned().collect();

    // Determine candidate pseudo-registers
    let mut gp_candidates: HashSet<String> = HashSet::new();
    let mut xmm_candidates: HashSet<String> = HashSet::new();
    let mut x87_float_store_dsts: HashSet<String> = HashSet::new();

    for instr in &func.instructions {
        if let AsmInstr::X87StoreFloat(_, AsmOperand::Pseudo(name)) = instr {
            x87_float_store_dsts.insert(name.clone());
        }
    }

    // Scan instructions for all pseudo names
    for instr in &func.instructions {
        let (used, updated) = find_used_and_updated(instr);
        for id in used.iter().chain(updated.iter()) {
            if let RegId::Pseudo(name) = id {
                if aliased.contains(name)
                    || arr_sizes.contains_key(name)
                    || x87_float_store_dsts.contains(name)
                {
                    continue;
                }
                let ct = types.get(name).copied().unwrap_or(CType::Int);
                match ct {
                    CType::Float | CType::Double => {
                        xmm_candidates.insert(name.clone());
                    }
                    CType::LongDouble | CType::Struct => {}
                    _ => {
                        gp_candidates.insert(name.clone());
                    }
                }
            }
        }
    }

    // --- GP Register Allocation ---
    let gp_hard_ids: Vec<RegId> = GP_COLOR_ORDER.iter().map(|r| RegId::Gp(*r)).collect();
    allocate_one_pass(
        &mut func.instructions,
        &exit_live,
        &gp_candidates,
        &gp_hard_ids,
        GP_K,
        no_coalescing,
    );

    // --- XMM Register Allocation ---
    let xmm_hard_ids: Vec<RegId> = XMM_COLOR_ORDER.iter().map(|r| RegId::Xmm(*r)).collect();
    allocate_one_pass(
        &mut func.instructions,
        &exit_live,
        &xmm_candidates,
        &xmm_hard_ids,
        XMM_K,
        no_coalescing,
    );

    // Determine which callee-saved GP registers were used
    let mut callee_saved_used: Vec<Reg> = Vec::new();
    let callee_set: HashSet<Reg> = GP_CALLEE_SAVED.iter().cloned().collect();
    for instr in &func.instructions {
        visit_operands(instr, |op| {
            if let AsmOperand::Reg(r) = op {
                if callee_set.contains(r) && !callee_saved_used.contains(r) {
                    callee_saved_used.push(*r);
                }
            }
        });
    }

    RegAllocResult {
        callee_saved: callee_saved_used,
    }
}

fn allocate_one_pass(
    instrs: &mut Vec<AsmInstr>,
    exit_live: &HashSet<RegId>,
    candidates: &HashSet<String>,
    hard_reg_ids: &[RegId],
    k: usize,
    no_coalescing: bool,
) {
    if candidates.is_empty() {
        return;
    }

    // Build-coalesce loop
    let mut graph;
    loop {
        let live_after = liveness_analysis(instrs, exit_live);
        graph = build_interference_graph(instrs, &live_after, candidates, hard_reg_ids, k);

        if no_coalescing {
            break;
        }

        if let Some(uf) = coalesce_pass(instrs, &mut graph, hard_reg_ids, k) {
            rewrite_coalesced(instrs, &uf);
        } else {
            break;
        }
    }

    // Color the graph
    color_graph(&mut graph, hard_reg_ids, k);

    // Build register map and apply
    let color_map = build_color_map(&graph, hard_reg_ids);
    apply_register_map(instrs, &color_map);
}

fn visit_operands<F: FnMut(&AsmOperand)>(instr: &AsmInstr, mut f: F) {
    match instr {
        AsmInstr::Mov(_, src, dst) | AsmInstr::Cmp(_, src, dst) => {
            f(src);
            f(dst);
        }
        AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
            f(src);
            f(dst);
        }
        AsmInstr::Binary(_, _, src, dst) => {
            f(src);
            f(dst);
        }
        AsmInstr::Unary(_, _, op) => {
            f(op);
        }
        AsmInstr::MulFull(_, op) | AsmInstr::Idiv(_, op) | AsmInstr::Div(_, op) => {
            f(op);
        }
        AsmInstr::SetCC(_, op) => {
            f(op);
        }
        AsmInstr::Push(op) => {
            f(op);
        }
        AsmInstr::Cvtsi2sd(_, src, dst)
        | AsmInstr::Cvtsi2ss(_, src, dst)
        | AsmInstr::Cvttsd2si(_, src, dst)
        | AsmInstr::Cvttss2si(_, src, dst) => {
            f(src);
            f(dst);
        }
        AsmInstr::Cvtss2sd(src, dst) | AsmInstr::Cvtsd2ss(src, dst) => {
            f(src);
            f(dst);
        }
        AsmInstr::X87Load(t, src) if *t != AsmType::LongDouble => {
            f(src);
        }
        AsmInstr::X87StoreInt(_, dst) => {
            f(dst);
        }
        AsmInstr::Lea(src, dst) => {
            f(src);
            f(dst);
        }
        AsmInstr::LoadIndirect(_, _, dst) => {
            f(dst);
        }
        AsmInstr::StoreIndirect(_, src, _) => {
            f(src);
        }
        AsmInstr::JmpIndirect(target) => {
            f(target);
        }
        AsmInstr::CopyToStackArg { src_ptr, .. } => {
            f(src_ptr);
        }
        AsmInstr::BuiltinSetjmp { buf, dst, .. } => {
            f(buf);
            f(dst);
        }
        AsmInstr::BuiltinLongjmp { buf, value } => {
            f(buf);
            f(value);
        }
        AsmInstr::AtomicRmw(_, _, _, dst)
        | AsmInstr::AtomicExchange(_, dst)
        | AsmInstr::AtomicCompareExchange(_, dst)
        | AsmInstr::AtomicCompareSwap(_, _, dst) => {
            f(dst);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indirect_jump_terminates_block_and_targets_all_labels() {
        let instrs = vec![
            AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Pseudo("base".to_string()),
                AsmOperand::Pseudo("target".to_string()),
            ),
            AsmInstr::JmpIndirect(AsmOperand::Pseudo("target".to_string())),
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Label("a".to_string()),
            AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Pseudo("base".to_string()),
                AsmOperand::Pseudo("target_a".to_string()),
            ),
            AsmInstr::JmpIndirect(AsmOperand::Pseudo("target_a".to_string())),
            AsmInstr::Label("b".to_string()),
            AsmInstr::Ret,
        ];

        let blocks = build_asm_cfg(&instrs);
        assert_eq!(blocks[0].end, 2);

        let label_blocks: HashSet<usize> = blocks
            .iter()
            .enumerate()
            .filter_map(|(idx, block)| match &instrs[block.start] {
                AsmInstr::Label(label) if label == "a" || label == "b" => Some(idx),
                _ => None,
            })
            .collect();
        let succs: HashSet<usize> = blocks[0].succs.iter().copied().collect();
        assert!(label_blocks.is_subset(&succs));
    }

    #[test]
    fn indirect_jump_keeps_label_base_live_at_computed_targets() {
        let instrs = vec![
            AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Pseudo("base".to_string()),
                AsmOperand::Pseudo("target".to_string()),
            ),
            AsmInstr::JmpIndirect(AsmOperand::Pseudo("target".to_string())),
            AsmInstr::Label("again".to_string()),
            AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Pseudo("base".to_string()),
                AsmOperand::Pseudo("next".to_string()),
            ),
            AsmInstr::JmpIndirect(AsmOperand::Pseudo("next".to_string())),
        ];
        let live_after = liveness_analysis(&instrs, &HashSet::new());

        assert!(live_after[1].contains(&RegId::Pseudo("base".to_string())));
    }
}
