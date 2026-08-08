use crate::types::*;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet, VecDeque};

// ============================================================
// Control-Flow Graph
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeId {
    Entry,
    Exit,
    Block(usize),
}

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: usize,
    pub instructions: Vec<TackyInstr>,
    pub successors: Vec<NodeId>,
    pub predecessors: Vec<NodeId>,
}

#[derive(Debug)]
pub struct Cfg {
    pub blocks: Vec<BasicBlock>,
}

impl Cfg {
    #[must_use]
    pub fn build(instructions: Vec<TackyInstr>) -> Self {
        let partitioned = partition_into_basic_blocks(instructions);
        let mut blocks: Vec<BasicBlock> = partitioned
            .into_iter()
            .enumerate()
            .map(|(i, instrs)| BasicBlock {
                id: i,
                instructions: instrs,
                successors: Vec::new(),
                predecessors: Vec::new(),
            })
            .collect();

        // Build label → block_id map
        let mut label_to_block: HashMap<String, usize> = HashMap::new();
        for block in &blocks {
            if let Some(TackyInstr::Label(label)) = block.instructions.first() {
                label_to_block.insert(label.clone(), block.id);
            }
        }

        // Add edges
        let num_blocks = blocks.len();

        if num_blocks > 0 {
            blocks[0].predecessors.push(NodeId::Entry);
        }

        for i in 0..num_blocks {
            let last = blocks[i].instructions.last();
            let next_id = if i + 1 < num_blocks {
                NodeId::Block(i + 1)
            } else {
                NodeId::Exit
            };

            match last {
                Some(instr) if exits_cfg(instr) => {
                    blocks[i].successors.push(NodeId::Exit);
                }
                Some(TackyInstr::Jump(target)) => {
                    if let Some(&target_id) = label_to_block.get(target) {
                        blocks[i].successors.push(NodeId::Block(target_id));
                        blocks[target_id].predecessors.push(NodeId::Block(i));
                    }
                }
                Some(TackyInstr::JumpIndirect(_)) => {
                    // Indirect gotos are formed from GNU label addresses, so
                    // only blocks beginning with a label can be targets.
                    // Avoiding edges to ordinary blocks keeps dataflow facts
                    // from being needlessly weakened across the function.
                    for &target_id in label_to_block.values() {
                        blocks[i].successors.push(NodeId::Block(target_id));
                        blocks[target_id].predecessors.push(NodeId::Block(i));
                    }
                }
                Some(TackyInstr::JumpIfZero(_, target))
                | Some(TackyInstr::JumpIfNotZero(_, target)) => {
                    if let Some(&target_id) = label_to_block.get(target) {
                        blocks[i].successors.push(NodeId::Block(target_id));
                        blocks[target_id].predecessors.push(NodeId::Block(i));
                    }
                    // Fall-through to next block
                    if !blocks[i].successors.contains(&next_id) {
                        blocks[i].successors.push(next_id);
                        if let NodeId::Block(j) = next_id {
                            blocks[j].predecessors.push(NodeId::Block(i));
                        }
                    }
                }
                _ => {
                    // Fall-through
                    blocks[i].successors.push(next_id);
                    if let NodeId::Block(j) = next_id {
                        blocks[j].predecessors.push(NodeId::Block(i));
                    }
                }
            }
        }

        Cfg { blocks }
    }

    pub fn to_instructions(&self) -> Vec<TackyInstr> {
        let total_len: usize = self
            .blocks
            .iter()
            .map(|block| block.instructions.len())
            .sum();
        let mut instructions = Vec::with_capacity(total_len);
        for block in &self.blocks {
            instructions.extend(block.instructions.iter().cloned());
        }
        instructions
    }

    pub fn into_instructions(self) -> Vec<TackyInstr> {
        let total_len: usize = self
            .blocks
            .iter()
            .map(|block| block.instructions.len())
            .sum();
        let mut instructions = Vec::with_capacity(total_len);
        for block in self.blocks {
            instructions.extend(block.instructions);
        }
        instructions
    }
}

fn partition_into_basic_blocks(instructions: Vec<TackyInstr>) -> Vec<Vec<TackyInstr>> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();

    for instr in instructions {
        match instr {
            TackyInstr::Label(_) => {
                if !current.is_empty() {
                    blocks.push(current);
                    current = Vec::new();
                }
                current.push(instr);
            }
            instr if ends_basic_block(&instr) => {
                current.push(instr);
                blocks.push(current);
                current = Vec::new();
            }
            _ => {
                current.push(instr);
            }
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

fn ends_basic_block(instr: &TackyInstr) -> bool {
    matches!(
        instr,
        TackyInstr::Jump(_)
            | TackyInstr::NonlocalJump(_)
            | TackyInstr::BuiltinLongjmp { .. }
            | TackyInstr::JumpIndirect(_)
            | TackyInstr::JumpIfZero(_, _)
            | TackyInstr::JumpIfNotZero(_, _)
            | TackyInstr::Return(_)
            | TackyInstr::Unreachable
    )
}

fn exits_cfg(instr: &TackyInstr) -> bool {
    matches!(
        instr,
        TackyInstr::Return(_)
            | TackyInstr::NonlocalJump(_)
            | TackyInstr::BuiltinLongjmp { .. }
            | TackyInstr::Unreachable
    )
}

// ============================================================
// Address-Taken Analysis
// ============================================================

pub fn find_aliased_vars(
    instructions: &[TackyInstr],
    static_vars: &HashSet<String>,
) -> HashSet<String> {
    let mut aliased = static_vars.clone();
    for instr in instructions {
        if let TackyInstr::GetAddress {
            src: TackyVal::Var(name),
            ..
        } = instr
        {
            aliased.insert(name.clone());
        }
    }
    aliased
}

// ============================================================
// Copy Propagation
// ============================================================

/// A copy instruction: dst = src (src can be a variable or constant)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CopyInstr {
    pub src: CopySrc,
    pub dst: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CopySrc {
    Var(String),
    Constant(i64),
    Int128Constant(i128),
    UInt128Constant(u128),
    DoubleConstant(u64), // store as bits for Eq/Hash
}

impl CopySrc {
    fn from_constant_val(val: &TackyVal) -> Option<Self> {
        match val {
            TackyVal::Constant(value) => Some(Self::Constant(*value)),
            TackyVal::Int128Constant(value) => Some(Self::Int128Constant(*value)),
            TackyVal::UInt128Constant(value) => Some(Self::UInt128Constant(*value)),
            TackyVal::DoubleConstant(value) => Some(Self::DoubleConstant(value.to_bits())),
            TackyVal::Var(_) => None,
        }
    }

    fn to_tacky_val(&self) -> TackyVal {
        match self {
            Self::Var(name) => TackyVal::Var(name.clone()),
            Self::Constant(value) => TackyVal::Constant(*value),
            Self::Int128Constant(value) => TackyVal::Int128Constant(*value),
            Self::UInt128Constant(value) => TackyVal::UInt128Constant(*value),
            Self::DoubleConstant(bits) => TackyVal::DoubleConstant(f64::from_bits(*bits)),
        }
    }

    fn var_name(&self) -> Option<&str> {
        match self {
            Self::Var(name) => Some(name),
            _ => None,
        }
    }

    fn can_replace_typed(&self, ty: CType) -> bool {
        match self {
            Self::Var(_) => true,
            Self::Constant(_) => false,
            Self::Int128Constant(_) => ty == CType::Int128,
            Self::UInt128Constant(_) => ty == CType::UInt128,
            Self::DoubleConstant(_) => can_propagate_double_constant_to(ty),
        }
    }
}

fn can_propagate_double_constant_to(ty: CType) -> bool {
    matches!(ty, CType::Double)
}

pub fn copy_propagation(
    cfg: &mut Cfg,
    aliased_vars: &HashSet<String>,
    types: &IndexMap<String, CType>,
) {
    // Initialize block facts conservatively. Growing from empty may miss some
    // available copies in exotic unreachable cycles, but never invents copies.
    let mut block_out: Vec<HashSet<CopyInstr>> = vec![HashSet::new(); cfg.blocks.len()];

    // Worklist algorithm
    let mut worklist: VecDeque<usize> = (0..cfg.blocks.len()).collect();
    let mut queued: HashSet<usize> = (0..cfg.blocks.len()).collect();

    while let Some(block_id) = worklist.pop_front() {
        queued.remove(&block_id);
        // Meet: intersection of all predecessors' out-copies
        let incoming = meet_copies(&cfg.blocks[block_id], &block_out);

        // Transfer function
        let new_out = transfer_copies_out(&cfg.blocks[block_id], incoming, aliased_vars, types);

        if block_out[block_id] != new_out {
            block_out[block_id] = new_out;
            // Add successors to worklist
            for succ in &cfg.blocks[block_id].successors {
                if let NodeId::Block(sid) = succ {
                    if queued.insert(*sid) {
                        worklist.push_back(*sid);
                    }
                }
            }
        }
    }

    // Rewrite instructions using reaching copies
    for block_id in 0..cfg.blocks.len() {
        let mut reaching = meet_copies(&cfg.blocks[block_id], &block_out);
        let mut new_instrs = Vec::with_capacity(cfg.blocks[block_id].instructions.len());
        for instr in &cfg.blocks[block_id].instructions {
            if let Some(rewritten) = rewrite_instruction(instr, &reaching, types) {
                transfer_copy_instr(&rewritten, &mut reaching, aliased_vars, types);
                new_instrs.push(rewritten);
            }
        }
        cfg.blocks[block_id].instructions = new_instrs;
    }
}

fn meet_copies(block: &BasicBlock, block_out: &[HashSet<CopyInstr>]) -> HashSet<CopyInstr> {
    let mut best_pred: Option<(usize, usize)> = None;
    for pred in &block.predecessors {
        match pred {
            NodeId::Entry => return HashSet::new(), // No copies reach from entry
            NodeId::Block(pid) => {
                if let Some(pred_out) = block_out.get(*pid) {
                    let len = pred_out.len();
                    if best_pred.is_none_or(|(_, best_len)| len < best_len) {
                        best_pred = Some((*pid, len));
                    }
                }
            }
            _ => {}
        }
    }

    let Some((best_pred_id, _)) = best_pred else {
        return HashSet::new();
    };
    let Some(mut incoming) = block_out.get(best_pred_id).cloned() else {
        return HashSet::new();
    };
    for pred in &block.predecessors {
        let NodeId::Block(pid) = pred else {
            continue;
        };
        if *pid == best_pred_id {
            continue;
        }
        if let Some(pred_out) = block_out.get(*pid) {
            incoming.retain(|copy| pred_out.contains(copy));
        }
    }
    incoming
}

fn transfer_copies_out(
    block: &BasicBlock,
    initial: HashSet<CopyInstr>,
    aliased: &HashSet<String>,
    types: &IndexMap<String, CType>,
) -> HashSet<CopyInstr> {
    let mut current = initial;

    for instr in &block.instructions {
        transfer_copy_instr(instr, &mut current, aliased, types);
    }

    current
}

fn kill_copies_involving_var(current: &mut HashSet<CopyInstr>, name: &str) {
    current.retain(|copy| {
        let src_match = copy.src.var_name() == Some(name);
        copy.dst != name && !src_match
    });
}

fn kill_copies_involving_aliased_vars(current: &mut HashSet<CopyInstr>, aliased: &HashSet<String>) {
    current.retain(|copy| {
        let src_aliased = copy.src.var_name().is_some_and(|src| aliased.contains(src));
        !src_aliased && !aliased.contains(&copy.dst)
    });
}

fn insert_reaching_copy(current: &mut HashSet<CopyInstr>, copy: CopyInstr) {
    if current.contains(&copy) {
        return;
    }
    let dst = copy.dst.clone();
    kill_copies_involving_var(current, &dst);
    current.insert(copy);
}

fn insert_reaching_copy_unless_reverse(
    current: &mut HashSet<CopyInstr>,
    copy: CopyInstr,
    reverse: CopyInstr,
) {
    if current.contains(&copy) || current.contains(&reverse) {
        return;
    }
    let dst = copy.dst.clone();
    kill_copies_involving_var(current, &dst);
    current.insert(copy);
}

fn transfer_copy_instr(
    instr: &TackyInstr,
    current: &mut HashSet<CopyInstr>,
    aliased: &HashSet<String>,
    types: &IndexMap<String, CType>,
) {
    match instr {
        TackyInstr::Copy {
            src: TackyVal::Var(s),
            dst: TackyVal::Var(d),
        } => {
            let st = types.get(s).copied().unwrap_or(CType::Int);
            let dt = types.get(d).copied().unwrap_or(CType::Int);
            let same_type =
                st == dt || (st.is_signed() == dt.is_signed() && st.size() == dt.size());
            if same_type {
                let copy = CopyInstr {
                    src: CopySrc::Var(s.clone()),
                    dst: d.clone(),
                };
                let reverse = CopyInstr {
                    src: CopySrc::Var(d.clone()),
                    dst: s.clone(),
                };
                insert_reaching_copy_unless_reverse(current, copy, reverse);
            } else {
                kill_copies_involving_var(current, d);
            }
        }
        TackyInstr::Copy {
            src: TackyVal::Constant(c),
            dst: TackyVal::Var(d),
        } => {
            let copy = CopyInstr {
                src: CopySrc::Constant(*c),
                dst: d.clone(),
            };
            insert_reaching_copy(current, copy);
        }
        TackyInstr::Copy {
            src: TackyVal::Int128Constant(c),
            dst: TackyVal::Var(d),
        } => {
            if types.get(d).copied() == Some(CType::Int128) {
                let copy = CopyInstr {
                    src: CopySrc::Int128Constant(*c),
                    dst: d.clone(),
                };
                insert_reaching_copy(current, copy);
            } else {
                kill_copies_involving_var(current, d);
            }
        }
        TackyInstr::Copy {
            src: TackyVal::UInt128Constant(c),
            dst: TackyVal::Var(d),
        } => {
            if types.get(d).copied() == Some(CType::UInt128) {
                let copy = CopyInstr {
                    src: CopySrc::UInt128Constant(*c),
                    dst: d.clone(),
                };
                insert_reaching_copy(current, copy);
            } else {
                kill_copies_involving_var(current, d);
            }
        }
        TackyInstr::Copy {
            src: TackyVal::DoubleConstant(c),
            dst: TackyVal::Var(d),
        } => {
            let dt = types.get(d).copied().unwrap_or(CType::Int);
            if can_propagate_double_constant_to(dt) {
                let copy = CopyInstr {
                    src: CopySrc::DoubleConstant(c.to_bits()),
                    dst: d.clone(),
                };
                insert_reaching_copy(current, copy);
            } else {
                kill_copies_involving_var(current, d);
            }
        }
        TackyInstr::FunCall { dst, .. } => {
            kill_copies_involving_aliased_vars(current, aliased);
            if let TackyVal::Var(d) = dst {
                kill_copies_involving_var(current, d);
            }
        }
        TackyInstr::Store { .. } => {
            // A pointer parameter may alias any addressable local or another
            // parameter even when no GetAddress appears in this function.
            // Without clearing all copies, propagation can reuse a value that
            // the store has just overwritten through an unknown pointer.
            current.clear();
        }
        TackyInstr::AtomicFetch { dst, .. }
        | TackyInstr::AtomicExchange { dst, .. }
        | TackyInstr::AtomicCompareExchange { dst, .. }
        | TackyInstr::AtomicCompareSwap { dst, .. } => {
            kill_copies_involving_aliased_vars(current, aliased);
            if let TackyVal::Var(d) = dst {
                kill_copies_involving_var(current, d);
            }
        }
        TackyInstr::BuiltinSetjmp { .. }
        | TackyInstr::BuiltinLongjmp { .. }
        | TackyInstr::VaStart { .. } => {
            current.clear();
        }
        TackyInstr::CopyStruct { src_name, dst_name } => {
            let copy = CopyInstr {
                src: CopySrc::Var(src_name.clone()),
                dst: dst_name.clone(),
            };
            let reverse = CopyInstr {
                src: CopySrc::Var(dst_name.clone()),
                dst: src_name.clone(),
            };
            insert_reaching_copy_unless_reverse(current, copy, reverse);
        }
        _ => {
            if let Some(dst) = get_instr_dst(instr) {
                kill_copies_involving_var(current, dst);
            }
        }
    }
}

fn get_instr_dst(instr: &TackyInstr) -> Option<&str> {
    match instr {
        TackyInstr::Copy {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::Binary {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::Unary {
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
        | TackyInstr::GetAddress {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::CopyFromOffset {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::AddPtr {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::FunCall {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::VaStart {
            dst: TackyVal::Var(n),
        }
        | TackyInstr::FrameAddress {
            dst: TackyVal::Var(n),
        }
        | TackyInstr::BuiltinSetjmp {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::LoadLabelAddress(_, TackyVal::Var(n))
        | TackyInstr::AtomicFetch {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::AtomicExchange {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::AtomicCompareExchange {
            dst: TackyVal::Var(n),
            ..
        }
        | TackyInstr::AtomicCompareSwap {
            dst: TackyVal::Var(n),
            ..
        } => Some(n),
        TackyInstr::CopyToOffset { dst_name, .. } | TackyInstr::CopyStruct { dst_name, .. } => {
            Some(dst_name)
        }
        _ => None,
    }
}

fn replace_operand(val: &TackyVal, reaching: &HashSet<CopyInstr>) -> TackyVal {
    if let TackyVal::Var(name) = val {
        let mut seen = HashSet::new();
        if let Some(replacement) = resolve_reaching_value(name, reaching, &|_| true, &mut seen) {
            return replacement;
        }
    }
    val.clone()
}

fn replace_operand_typed(
    val: &TackyVal,
    reaching: &HashSet<CopyInstr>,
    types: &IndexMap<String, CType>,
) -> TackyVal {
    // Like replace_operand, but for constants, check that the type would be preserved
    if let TackyVal::Var(name) = val {
        let orig_type = types.get(name).copied().unwrap_or(CType::Int);
        let mut seen = HashSet::new();
        if let Some(replacement) = resolve_reaching_value(
            name,
            reaching,
            &|src| src.can_replace_typed(orig_type),
            &mut seen,
        ) {
            if !matches!(
                replacement,
                TackyVal::Constant(_)
                    | TackyVal::Int128Constant(_)
                    | TackyVal::UInt128Constant(_)
                    | TackyVal::DoubleConstant(_)
            ) || replacement_matches_type(&replacement, orig_type)
            {
                return replacement;
            }
        }
    }
    val.clone()
}

fn replace_named_source(name: &str, reaching: &HashSet<CopyInstr>) -> String {
    let mut seen = HashSet::new();
    resolve_reaching_value(
        name,
        reaching,
        &|src| matches!(src, CopySrc::Var(_)),
        &mut seen,
    )
    .and_then(|replacement| match replacement {
        TackyVal::Var(src) => Some(src),
        _ => None,
    })
    .unwrap_or_else(|| name.to_string())
}

fn resolve_reaching_value<F>(
    name: &str,
    reaching: &HashSet<CopyInstr>,
    can_use_src: &F,
    seen: &mut HashSet<String>,
) -> Option<TackyVal>
where
    F: Fn(&CopySrc) -> bool,
{
    if !seen.insert(name.to_string()) {
        return None;
    }
    let copy = select_reaching_copy(name, reaching, can_use_src)?;
    match &copy.src {
        CopySrc::Var(src_name) => {
            if src_name == name {
                Some(TackyVal::Var(src_name.clone()))
            } else {
                resolve_reaching_value(src_name, reaching, can_use_src, seen)
                    .or_else(|| Some(TackyVal::Var(src_name.clone())))
            }
        }
        _ => Some(copy.src.to_tacky_val()),
    }
}

fn replacement_matches_type(val: &TackyVal, ty: CType) -> bool {
    match val {
        TackyVal::Constant(_) => ty == CType::Int,
        TackyVal::Int128Constant(_) => ty == CType::Int128,
        TackyVal::UInt128Constant(_) => ty == CType::UInt128,
        TackyVal::DoubleConstant(_) => ty == CType::Double,
        TackyVal::Var(_) => true,
    }
}

fn select_reaching_copy<'a, F>(
    name: &str,
    reaching: &'a HashSet<CopyInstr>,
    can_use_src: F,
) -> Option<&'a CopyInstr>
where
    F: Fn(&CopySrc) -> bool,
{
    let mut best = None;
    for copy in reaching {
        if copy.dst == name && can_use_src(&copy.src) {
            best = better_reaching_copy(best, copy);
        }
    }
    best
}

fn better_reaching_copy<'a>(
    best: Option<&'a CopyInstr>,
    candidate: &'a CopyInstr,
) -> Option<&'a CopyInstr> {
    match (best, &candidate.src) {
        (None, _) => Some(candidate),
        (Some(current), CopySrc::Var(_)) if !matches!(&current.src, CopySrc::Var(_)) => {
            Some(candidate)
        }
        (Some(current), CopySrc::Var(candidate_src)) => {
            if let CopySrc::Var(current_src) = &current.src {
                if candidate_src < current_src {
                    return Some(candidate);
                }
            }
            Some(current)
        }
        (Some(current), _) => Some(current),
    }
}

fn is_redundant_copy(src: &TackyVal, dst: &TackyVal, reaching: &HashSet<CopyInstr>) -> bool {
    let TackyVal::Var(dst_name) = dst else {
        return false;
    };
    if let TackyVal::Var(src_name) = src {
        if src_name == dst_name {
            return true;
        }
        let fwd = CopyInstr {
            src: CopySrc::Var(src_name.clone()),
            dst: dst_name.clone(),
        };
        let rev = CopyInstr {
            src: CopySrc::Var(dst_name.clone()),
            dst: src_name.clone(),
        };
        return reaching.contains(&fwd) || reaching.contains(&rev);
    }
    CopySrc::from_constant_val(src).is_some_and(|src| {
        reaching.contains(&CopyInstr {
            src,
            dst: dst_name.clone(),
        })
    })
}

fn rewrite_instruction(
    instr: &TackyInstr,
    reaching: &HashSet<CopyInstr>,
    types: &IndexMap<String, CType>,
) -> Option<TackyInstr> {
    match instr {
        TackyInstr::Copy { src, dst } => {
            let new_src = replace_operand(src, reaching);
            if is_redundant_copy(&new_src, dst, reaching) {
                return None;
            }
            Some(TackyInstr::Copy {
                src: new_src,
                dst: dst.clone(),
            })
        }
        TackyInstr::Return(val) => Some(TackyInstr::Return(replace_operand(val, reaching))),
        TackyInstr::Unary { op, src, dst } => Some(TackyInstr::Unary {
            op: op.clone(),
            src: replace_operand(src, reaching),
            dst: dst.clone(),
        }),
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            // Comparisons and right shifts depend on operand signedness.  Keep
            // typed temporaries when replacing them with an untyped constant
            // would make codegen choose a different width or signed shift.
            let is_cmp = matches!(
                op,
                TackyBinaryOp::Equal
                    | TackyBinaryOp::NotEqual
                    | TackyBinaryOp::LessThan
                    | TackyBinaryOp::LessEqual
                    | TackyBinaryOp::GreaterThan
                    | TackyBinaryOp::GreaterEqual
            );
            let needs_typed_replacement = is_cmp || matches!(op, TackyBinaryOp::ShiftRight);
            let new_left = if needs_typed_replacement {
                replace_operand_typed(left, reaching, types)
            } else {
                replace_operand(left, reaching)
            };
            let new_right = if needs_typed_replacement {
                replace_operand_typed(right, reaching, types)
            } else {
                replace_operand(right, reaching)
            };
            Some(TackyInstr::Binary {
                op: op.clone(),
                left: new_left,
                right: new_right,
                dst: dst.clone(),
            })
        }
        TackyInstr::JumpIfZero(val, target) => Some(TackyInstr::JumpIfZero(
            replace_operand(val, reaching),
            target.clone(),
        )),
        TackyInstr::JumpIfNotZero(val, target) => Some(TackyInstr::JumpIfNotZero(
            replace_operand(val, reaching),
            target.clone(),
        )),
        TackyInstr::JumpIndirect(val) => {
            Some(TackyInstr::JumpIndirect(replace_operand(val, reaching)))
        }
        TackyInstr::BuiltinSetjmp {
            buf,
            dst,
            label,
            end_label,
        } => Some(TackyInstr::BuiltinSetjmp {
            buf: replace_operand(buf, reaching),
            dst: dst.clone(),
            label: label.clone(),
            end_label: end_label.clone(),
        }),
        TackyInstr::BuiltinLongjmp { buf, value } => Some(TackyInstr::BuiltinLongjmp {
            buf: replace_operand(buf, reaching),
            value: replace_operand_typed(value, reaching, types),
        }),
        TackyInstr::Truncate { src, dst } => Some(TackyInstr::Truncate {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::SignExtend { src, dst } => Some(TackyInstr::SignExtend {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::ZeroExtend { src, dst } => Some(TackyInstr::ZeroExtend {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::Store { src, dst_ptr } => {
            // Replace Store src, but only with constants if the type is preserved
            let new_src = replace_operand_typed(src, reaching, types);
            Some(TackyInstr::Store {
                src: new_src,
                dst_ptr: replace_operand(dst_ptr, reaching),
            })
        }
        TackyInstr::Load { src_ptr, dst } => Some(TackyInstr::Load {
            src_ptr: replace_operand(src_ptr, reaching),
            dst: dst.clone(),
        }),
        TackyInstr::AtomicFetch {
            op,
            ptr,
            arg,
            return_old,
            dst,
        } => Some(TackyInstr::AtomicFetch {
            op: op.clone(),
            ptr: replace_operand(ptr, reaching),
            arg: replace_operand_typed(arg, reaching, types),
            return_old: *return_old,
            dst: dst.clone(),
        }),
        TackyInstr::AtomicExchange { ptr, value, dst } => Some(TackyInstr::AtomicExchange {
            ptr: replace_operand(ptr, reaching),
            value: replace_operand_typed(value, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::AtomicCompareExchange {
            ptr,
            expected,
            desired,
            dst,
        } => Some(TackyInstr::AtomicCompareExchange {
            ptr: replace_operand(ptr, reaching),
            expected: replace_operand_typed(expected, reaching, types),
            desired: replace_operand_typed(desired, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::AtomicCompareSwap {
            ptr,
            expected,
            desired,
            return_old,
            dst,
        } => Some(TackyInstr::AtomicCompareSwap {
            ptr: replace_operand(ptr, reaching),
            expected: replace_operand_typed(expected, reaching, types),
            desired: replace_operand_typed(desired, reaching, types),
            return_old: *return_old,
            dst: dst.clone(),
        }),
        TackyInstr::DoubleToInt { src, dst } => Some(TackyInstr::DoubleToInt {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::FloatToInt { src, dst } => Some(TackyInstr::FloatToInt {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::DoubleToUInt { src, dst } => Some(TackyInstr::DoubleToUInt {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::FloatToUInt { src, dst } => Some(TackyInstr::FloatToUInt {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::IntToDouble { src, dst } => Some(TackyInstr::IntToDouble {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::IntToFloat { src, dst } => Some(TackyInstr::IntToFloat {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::UIntToDouble { src, dst } => Some(TackyInstr::UIntToDouble {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::UIntToFloat { src, dst } => Some(TackyInstr::UIntToFloat {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::FloatToDouble { src, dst } => Some(TackyInstr::FloatToDouble {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        TackyInstr::DoubleToFloat { src, dst } => Some(TackyInstr::DoubleToFloat {
            src: replace_operand_typed(src, reaching, types),
            dst: dst.clone(),
        }),
        // Don't rewrite GetAddress (uses address, not value; changing address breaks aliasing)
        TackyInstr::GetAddress { .. } => Some(instr.clone()),
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
            let new_args: Vec<TackyVal> = args
                .iter()
                .map(|a| replace_operand_typed(a, reaching, types))
                .collect();
            let new_name = if *indirect {
                replace_named_source(name, reaching)
            } else {
                name.clone()
            };
            Some(TackyInstr::FunCall {
                name: new_name,
                args: new_args,
                dst: dst.clone(),
                stack_arg_indices: stack_arg_indices.clone(),
                memory_arg_blocks: memory_arg_blocks.clone(),
                struct_arg_groups: struct_arg_groups.clone(),
                variadic: *variadic,
                fixed_flat_arg_count: *fixed_flat_arg_count,
                hidden_return: *hidden_return,
                indirect: *indirect,
            })
        }
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => Some(TackyInstr::AddPtr {
            ptr: replace_operand(ptr, reaching),
            index: replace_operand(index, reaching),
            scale: *scale,
            dst: dst.clone(),
        }),
        TackyInstr::CopyToOffset {
            src,
            dst_name,
            offset,
        } => {
            // Use typed replacement to avoid losing type info for small constants
            Some(TackyInstr::CopyToOffset {
                src: replace_operand_typed(src, reaching, types),
                dst_name: dst_name.clone(),
                offset: *offset,
            })
        }
        TackyInstr::CopyFromOffset {
            src_name,
            offset,
            dst,
        } => Some(TackyInstr::CopyFromOffset {
            src_name: replace_named_source(src_name, reaching),
            offset: *offset,
            dst: dst.clone(),
        }),
        TackyInstr::CopyStruct { src_name, dst_name } => {
            // Check if redundant (either direction)
            let fwd = CopyInstr {
                src: CopySrc::Var(src_name.clone()),
                dst: dst_name.clone(),
            };
            let rev = CopyInstr {
                src: CopySrc::Var(dst_name.clone()),
                dst: src_name.clone(),
            };
            if reaching.contains(&fwd) || reaching.contains(&rev) {
                return None; // Eliminate redundant struct copy
            }
            Some(TackyInstr::CopyStruct {
                src_name: replace_named_source(src_name, reaching),
                dst_name: dst_name.clone(),
            })
        }
        _ => Some(instr.clone()),
    }
}

// ============================================================
// Dead Store Elimination
// ============================================================

pub fn dead_store_elimination(
    cfg: &mut Cfg,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    if cfg
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instr| matches!(instr, TackyInstr::BuiltinSetjmp { .. }))
    {
        return false;
    }

    // An unknown pointer store or call can alias an aggregate parameter even
    // when this function contains no direct GetAddress for that aggregate.
    // Treat aggregates present in the function as externally observable in
    // that case, while retaining precise elimination for read-only functions.
    let mut effective_aliased_vars = aliased_vars.clone();
    if cfg
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(is_unknown_memory_write)
    {
        for instr in cfg.blocks.iter().flat_map(|block| &block.instructions) {
            match instr {
                TackyInstr::CopyToOffset { dst_name, .. }
                | TackyInstr::CopyStruct { dst_name, .. } => {
                    effective_aliased_vars.insert(dst_name.clone());
                }
                _ => {}
            }
        }
    }
    let aliased_vars = &effective_aliased_vars;

    // Liveness analysis (backward data-flow)
    let mut block_live_in: Vec<HashSet<String>> = vec![HashSet::new(); cfg.blocks.len()];

    // Phase 1: iterate liveness to a fixpoint WITHOUT deleting any instructions.
    // Filtering during the fixpoint is unsound: a store that is live across a
    // loop back-edge looks dead on the first visit (successors' live-in still
    // empty) and would be deleted before liveness converges.
    let mut worklist: VecDeque<usize> = (0..cfg.blocks.len()).rev().collect();
    let mut queued: HashSet<usize> = (0..cfg.blocks.len()).collect();

    while let Some(block_id) = worklist.pop_front() {
        queued.remove(&block_id);
        // Meet: union of all successors' live-in vars
        let end_live = meet_liveness(&cfg.blocks[block_id], &block_live_in, static_vars);

        // Transfer function (backward), no filtering. Instruction order is
        // preserved when filter=false, so we can move the vector back untouched.
        let instructions = std::mem::take(&mut cfg.blocks[block_id].instructions);
        let (new_live_in, instructions) =
            transfer_liveness_and_filter(instructions, end_live, aliased_vars, static_vars, false);
        cfg.blocks[block_id].instructions = instructions;

        if block_live_in[block_id] != new_live_in {
            block_live_in[block_id] = new_live_in;
            // Add predecessors to worklist
            for pred in &cfg.blocks[block_id].predecessors {
                if let NodeId::Block(pid) = pred {
                    if queued.insert(*pid) {
                        worklist.push_back(*pid);
                    }
                }
            }
        }
    }

    // Phase 2: with converged liveness, filter each block exactly once.
    let mut changed = false;
    for block_id in 0..cfg.blocks.len() {
        let end_live = meet_liveness(&cfg.blocks[block_id], &block_live_in, static_vars);
        let original_len = cfg.blocks[block_id].instructions.len();
        let instructions = std::mem::take(&mut cfg.blocks[block_id].instructions);
        let (_live_in, new_instrs) =
            transfer_liveness_and_filter(instructions, end_live, aliased_vars, static_vars, true);
        if new_instrs.len() != original_len {
            changed = true;
        }
        cfg.blocks[block_id].instructions = new_instrs;
    }

    changed
}

fn meet_liveness(
    block: &BasicBlock,
    block_live_in: &[HashSet<String>],
    static_vars: &HashSet<String>,
) -> HashSet<String> {
    let mut best_succ: Option<(Option<usize>, usize)> = None;
    for succ in &block.successors {
        match succ {
            NodeId::Exit => {
                let len = static_vars.len();
                if best_succ.is_none_or(|(_, best_len)| len > best_len) {
                    best_succ = Some((None, len));
                }
            }
            NodeId::Block(sid) => {
                if let Some(succ_live) = block_live_in.get(*sid) {
                    let len = succ_live.len();
                    if best_succ.is_none_or(|(_, best_len)| len > best_len) {
                        best_succ = Some((Some(*sid), len));
                    }
                }
            }
            _ => {}
        }
    }

    let Some((best_succ_id, _)) = best_succ else {
        return HashSet::new();
    };
    let mut live = match best_succ_id {
        None => static_vars.clone(),
        Some(sid) => block_live_in.get(sid).cloned().unwrap_or_default(),
    };
    for succ in &block.successors {
        match succ {
            NodeId::Exit if best_succ_id.is_some() => live.extend(static_vars.iter().cloned()),
            NodeId::Block(sid) if Some(*sid) != best_succ_id => {
                if let Some(succ_live) = block_live_in.get(*sid) {
                    live.extend(succ_live.iter().cloned());
                }
            }
            _ => {}
        }
    }
    live
}

fn transfer_liveness_and_filter(
    instructions: Vec<TackyInstr>,
    end_live: HashSet<String>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
    filter: bool,
) -> (HashSet<String>, Vec<TackyInstr>) {
    let mut current = end_live;
    let mut kept = Vec::with_capacity(instructions.len());

    // Process instructions in reverse
    for instr in instructions.into_iter().rev() {
        let instr_ref = &instr;
        // Only drop dead stores in the filtering pass. During the liveness
        // fixpoint (filter=false) every instruction contributes its normal
        // gen/kill so the computed live sets stay sound.
        if filter && is_dead_store(instr_ref, &current, aliased_vars, static_vars) {
            continue;
        }

        // Kill destination, generate sources
        match instr_ref {
            TackyInstr::FunCall {
                name,
                dst,
                args,
                indirect,
                ..
            } => {
                remove_live_var(&mut current, dst);
                if *indirect {
                    current.insert(name.clone());
                }
                for arg in args {
                    insert_live_var(&mut current, arg);
                }
                // Function calls may read/write any aliased var (static + address-taken)
                current.extend(aliased_vars.iter().cloned());
            }
            // CopyToOffset modifies a sub-field. If the aggregate is dead and
            // cannot be observed indirectly, the write and its source are dead.
            TackyInstr::CopyToOffset { src, dst_name, .. } => {
                if !aggregate_write_can_be_dead(dst_name, &current, aliased_vars, static_vars) {
                    current.insert(dst_name.clone());
                    insert_live_var(&mut current, src);
                }
            }
            // CopyFromOffset reads a sub-field — generates the struct, kills dst
            TackyInstr::CopyFromOffset { src_name, dst, .. } => {
                remove_live_var(&mut current, dst);
                current.insert(src_name.clone());
            }
            // CopyStruct overwrites entire struct. A dead, unobservable copy
            // does not read its source.
            TackyInstr::CopyStruct { src_name, dst_name } => {
                let dead_write =
                    aggregate_write_can_be_dead(dst_name, &current, aliased_vars, static_vars);
                current.remove(dst_name);
                if !dead_write {
                    current.insert(src_name.clone());
                }
            }
            // Store writes through a pointer — reads src and dst_ptr, but doesn't read aliased vars
            TackyInstr::Store { src, dst_ptr } => {
                insert_live_var(&mut current, src);
                insert_live_var(&mut current, dst_ptr);
            }
            // Load reads through a pointer — generates aliased vars
            TackyInstr::Load { src_ptr, dst } => {
                remove_live_var(&mut current, dst);
                insert_live_var(&mut current, src_ptr);
                // Load may read any aliased var
                current.extend(aliased_vars.iter().cloned());
            }
            TackyInstr::AtomicFetch { dst, ptr, arg, .. } => {
                remove_live_var(&mut current, dst);
                insert_live_var(&mut current, ptr);
                insert_live_var(&mut current, arg);
                current.extend(aliased_vars.iter().cloned());
            }
            TackyInstr::AtomicExchange { dst, ptr, value } => {
                remove_live_var(&mut current, dst);
                insert_live_var(&mut current, ptr);
                insert_live_var(&mut current, value);
                current.extend(aliased_vars.iter().cloned());
            }
            TackyInstr::AtomicCompareExchange {
                dst,
                ptr,
                expected,
                desired,
            }
            | TackyInstr::AtomicCompareSwap {
                dst,
                ptr,
                expected,
                desired,
                ..
            } => {
                remove_live_var(&mut current, dst);
                insert_live_var(&mut current, ptr);
                insert_live_var(&mut current, expected);
                insert_live_var(&mut current, desired);
                current.extend(aliased_vars.iter().cloned());
            }
            _ => {
                if let Some(d) = get_instr_dst(instr_ref) {
                    current.remove(d);
                }
                crate::optimize::for_each_instr_source_var(instr_ref, |src| {
                    current.insert(src.to_string());
                });
            }
        }

        kept.push(instr);
    }

    kept.reverse();
    (current, kept)
}

fn insert_live_var(live: &mut HashSet<String>, val: &TackyVal) {
    if let TackyVal::Var(name) = val {
        live.insert(name.clone());
    }
}

fn remove_live_var(live: &mut HashSet<String>, val: &TackyVal) {
    if let TackyVal::Var(name) = val {
        live.remove(name);
    }
}

fn aggregate_write_can_be_dead(
    dst_name: &str,
    live_after: &HashSet<String>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    !live_after.contains(dst_name)
        && !aliased_vars.contains(dst_name)
        && !static_vars.contains(dst_name)
}

fn is_unknown_memory_write(instr: &TackyInstr) -> bool {
    matches!(
        instr,
        TackyInstr::FunCall { .. }
            | TackyInstr::Store { .. }
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

fn is_dead_store(
    instr: &TackyInstr,
    live_after: &HashSet<String>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    // Never eliminate function calls (side effects), stores (write through pointer),
    // atomic operations, or GetAddress.
    if matches!(
        instr,
        TackyInstr::FunCall { .. }
            | TackyInstr::Store { .. }
            | TackyInstr::AtomicFence
            | TackyInstr::AtomicFetch { .. }
            | TackyInstr::AtomicExchange { .. }
            | TackyInstr::AtomicCompareExchange { .. }
            | TackyInstr::AtomicCompareSwap { .. }
            | TackyInstr::BuiltinSetjmp { .. }
            | TackyInstr::BuiltinLongjmp { .. }
            | TackyInstr::VaStart { .. }
    ) {
        return false;
    }
    // Never eliminate jumps, labels, returns
    if matches!(
        instr,
        TackyInstr::Jump(_)
            | TackyInstr::JumpIfZero(_, _)
            | TackyInstr::JumpIfNotZero(_, _)
            | TackyInstr::Return(_)
            | TackyInstr::Unreachable
            | TackyInstr::Label(_)
            | TackyInstr::Nop
    ) {
        return false;
    }
    if let TackyInstr::CopyToOffset { dst_name, .. } | TackyInstr::CopyStruct { dst_name, .. } =
        instr
    {
        return aggregate_write_can_be_dead(dst_name, live_after, aliased_vars, static_vars);
    }

    // If instruction has a destination and it's not live after, it's a dead store
    if let Some(dst) = get_instr_dst(instr) {
        if static_vars.contains(dst) {
            return false;
        }
        if !live_after.contains(dst) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str) -> TackyVal {
        TackyVal::Var(name.to_string())
    }

    fn typed_vars(vars: &[(&str, CType)]) -> IndexMap<String, CType> {
        vars.iter()
            .map(|(name, ty)| ((*name).to_string(), *ty))
            .collect()
    }

    #[test]
    fn cfg_treats_nonlocal_jump_as_terminator() {
        let cfg = Cfg::build(vec![
            TackyInstr::NonlocalJump("outer_label".to_string()),
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);

        assert_eq!(cfg.blocks.len(), 2);
        assert_eq!(cfg.blocks[0].successors, vec![NodeId::Exit]);
        assert!(cfg.blocks[1].predecessors.is_empty());
    }

    #[test]
    fn cfg_treats_builtin_longjmp_as_terminator() {
        let cfg = Cfg::build(vec![
            TackyInstr::BuiltinLongjmp {
                buf: v("env"),
                value: TackyVal::Constant(1),
            },
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);

        assert_eq!(cfg.blocks.len(), 2);
        assert_eq!(cfg.blocks[0].successors, vec![NodeId::Exit]);
        assert!(cfg.blocks[1].predecessors.is_empty());
    }

    #[test]
    fn conditional_jump_to_fallthrough_has_one_edge() {
        let cfg = Cfg::build(vec![
            TackyInstr::JumpIfZero(TackyVal::Constant(0), "next".to_string()),
            TackyInstr::Label("next".to_string()),
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);

        assert_eq!(cfg.blocks.len(), 2);
        assert_eq!(cfg.blocks[0].successors, vec![NodeId::Block(1)]);
        assert_eq!(cfg.blocks[1].predecessors, vec![NodeId::Block(0)]);
    }

    #[test]
    fn indirect_jump_targets_only_label_blocks() {
        let cfg = Cfg::build(vec![
            TackyInstr::JumpIndirect(TackyVal::Var("target".to_string())),
            TackyInstr::Copy {
                src: TackyVal::Constant(1),
                dst: TackyVal::Var("ordinary".to_string()),
            },
            TackyInstr::Label("target".to_string()),
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);

        assert_eq!(cfg.blocks.len(), 3);
        assert_eq!(cfg.blocks[0].successors, vec![NodeId::Block(2)]);
        assert!(cfg.blocks[1].predecessors.is_empty());
    }

    #[test]
    fn copy_propagation_rewrites_int128_constant_use() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: TackyVal::Int128Constant(42),
                dst: v("x"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: v("x"),
                right: TackyVal::Int128Constant(1),
                dst: v("y"),
            },
            TackyInstr::Return(v("y")),
        ]);
        let types = typed_vars(&[("x", CType::Int128), ("y", CType::Int128)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::Int128Constant(42),
            right: TackyVal::Int128Constant(1),
            dst: v("y"),
        }));
    }

    #[test]
    fn copy_propagation_rewrites_uint128_constant_use() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: TackyVal::UInt128Constant(42),
                dst: v("x"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: v("x"),
                right: TackyVal::UInt128Constant(1),
                dst: v("y"),
            },
            TackyInstr::Return(v("y")),
        ]);
        let types = typed_vars(&[("x", CType::UInt128), ("y", CType::UInt128)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: TackyVal::UInt128Constant(42),
            right: TackyVal::UInt128Constant(1),
            dst: v("y"),
        }));
    }

    #[test]
    fn copy_propagation_rejects_int128_constant_for_mismatched_dst_type() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: TackyVal::Int128Constant(42),
                dst: v("x"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: v("x"),
                right: TackyVal::Constant(1),
                dst: v("y"),
            },
            TackyInstr::Return(v("y")),
        ]);
        let types = typed_vars(&[("x", CType::Long), ("y", CType::Long)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("x"),
            right: TackyVal::Constant(1),
            dst: v("y"),
        }));
    }

    #[test]
    fn copy_propagation_transfers_rewritten_scalar_copy() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("b"),
            },
            TackyInstr::Copy {
                src: v("b"),
                dst: v("c"),
            },
            TackyInstr::Return(v("c")),
        ]);
        let types = typed_vars(&[("a", CType::Int), ("b", CType::Int), ("c", CType::Int)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Return(v("a"))));
    }

    #[test]
    fn copy_propagation_eliminates_rewritten_redundant_scalar_copy() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("c"),
            },
            TackyInstr::Copy {
                src: v("a"),
                dst: v("b"),
            },
            TackyInstr::Copy {
                src: v("b"),
                dst: v("c"),
            },
            TackyInstr::Return(v("c")),
        ]);
        let types = typed_vars(&[("a", CType::Int), ("b", CType::Int), ("c", CType::Int)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        let body = cfg.to_instructions();
        assert_eq!(
            body.iter()
                .filter(|instr| {
                    **instr
                        == TackyInstr::Copy {
                            src: v("a"),
                            dst: v("c"),
                        }
                })
                .count(),
            1
        );
        assert!(body.contains(&TackyInstr::Return(v("a"))));
    }

    #[test]
    fn copy_propagation_eliminates_rewritten_self_copy() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("b"),
            },
            TackyInstr::Copy {
                src: v("b"),
                dst: v("a"),
            },
            TackyInstr::Return(v("a")),
        ]);
        let types = typed_vars(&[("a", CType::Int), ("b", CType::Int)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert_eq!(cfg.to_instructions().len(), 2);
    }

    #[test]
    fn copy_propagation_kills_copy_when_label_address_redefines_dst() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("q"),
                dst: v("p"),
            },
            TackyInstr::LoadLabelAddress("target".to_string(), v("p")),
            TackyInstr::Return(v("p")),
        ]);
        let types = typed_vars(&[("p", CType::Pointer), ("q", CType::Pointer)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Return(v("p"))));
    }

    #[test]
    fn copy_propagation_rewrites_jump_indirect_target() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("target"),
                dst: v("tmp"),
            },
            TackyInstr::JumpIndirect(v("tmp")),
        ]);
        let types = typed_vars(&[("target", CType::Pointer), ("tmp", CType::Pointer)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg
            .to_instructions()
            .contains(&TackyInstr::JumpIndirect(v("target"))));
    }

    #[test]
    fn copy_propagation_rewrites_builtin_setjmp_buffer() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("env"),
                dst: v("tmp_env"),
            },
            TackyInstr::BuiltinSetjmp {
                buf: v("tmp_env"),
                dst: v("result"),
                label: "resume".to_string(),
                end_label: "done".to_string(),
            },
        ]);
        let types = typed_vars(&[
            ("env", CType::Pointer),
            ("tmp_env", CType::Pointer),
            ("result", CType::Int),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::BuiltinSetjmp {
            buf: v("env"),
            dst: v("result"),
            label: "resume".to_string(),
            end_label: "done".to_string(),
        }));
    }

    #[test]
    fn copy_propagation_rewrites_builtin_longjmp_operands() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("env"),
                dst: v("tmp_env"),
            },
            TackyInstr::Copy {
                src: v("code"),
                dst: v("tmp_code"),
            },
            TackyInstr::BuiltinLongjmp {
                buf: v("tmp_env"),
                value: v("tmp_code"),
            },
        ]);
        let types = typed_vars(&[
            ("env", CType::Pointer),
            ("tmp_env", CType::Pointer),
            ("code", CType::Int),
            ("tmp_code", CType::Int),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::BuiltinLongjmp {
            buf: v("env"),
            value: v("code"),
        }));
    }

    #[test]
    fn copy_propagation_rewrites_indirect_call_callee() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("callee"),
                dst: v("tmp_callee"),
            },
            TackyInstr::FunCall {
                name: "tmp_callee".to_string(),
                args: vec![v("arg")],
                dst: v("result"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 1,
                hidden_return: false,
                indirect: true,
            },
            TackyInstr::Return(v("result")),
        ]);
        let types = typed_vars(&[
            ("callee", CType::Pointer),
            ("tmp_callee", CType::Pointer),
            ("arg", CType::Int),
            ("result", CType::Int),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::FunCall {
            name: "callee".to_string(),
            args: vec![v("arg")],
            dst: v("result"),
            stack_arg_indices: HashSet::new(),
            memory_arg_blocks: Vec::new(),
            struct_arg_groups: Vec::new(),
            variadic: false,
            fixed_flat_arg_count: 1,
            hidden_return: false,
            indirect: true,
        }));
    }

    #[test]
    fn copy_propagation_clears_reaching_copies_after_setjmp() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("tmp"),
            },
            TackyInstr::Copy {
                src: v("env"),
                dst: v("tmp_env"),
            },
            TackyInstr::BuiltinSetjmp {
                buf: v("tmp_env"),
                dst: v("setjmp_result"),
                label: "resume".to_string(),
                end_label: "done".to_string(),
            },
            TackyInstr::Return(v("tmp")),
        ]);
        let types = typed_vars(&[
            ("a", CType::Int),
            ("tmp", CType::Int),
            ("env", CType::Pointer),
            ("tmp_env", CType::Pointer),
            ("setjmp_result", CType::Int),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        let body = cfg.to_instructions();
        assert!(body.contains(&TackyInstr::BuiltinSetjmp {
            buf: v("env"),
            dst: v("setjmp_result"),
            label: "resume".to_string(),
            end_label: "done".to_string(),
        }));
        assert!(body.contains(&TackyInstr::Return(v("tmp"))));
    }

    #[test]
    fn copy_propagation_clears_reaching_copies_after_longjmp() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("tmp"),
            },
            TackyInstr::BuiltinLongjmp {
                buf: v("env"),
                value: TackyVal::Constant(1),
            },
            TackyInstr::Return(v("tmp")),
        ]);
        let types = typed_vars(&[
            ("a", CType::Int),
            ("tmp", CType::Int),
            ("env", CType::Pointer),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg
            .to_instructions()
            .contains(&TackyInstr::Return(v("tmp"))));
    }

    #[test]
    fn copy_propagation_clears_reaching_copies_after_va_start() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("tmp"),
            },
            TackyInstr::VaStart { dst: v("ap") },
            TackyInstr::Return(v("tmp")),
        ]);
        let types = typed_vars(&[
            ("a", CType::Int),
            ("tmp", CType::Int),
            ("ap", CType::Pointer),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg
            .to_instructions()
            .contains(&TackyInstr::Return(v("tmp"))));
    }

    #[test]
    fn copy_propagation_clears_reaching_copies_after_unknown_store() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("tmp"),
            },
            TackyInstr::Store {
                src: TackyVal::Constant(1),
                dst_ptr: v("p"),
            },
            TackyInstr::Return(v("tmp")),
        ]);
        let types = typed_vars(&[
            ("a", CType::Int),
            ("tmp", CType::Int),
            ("p", CType::Pointer),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg
            .to_instructions()
            .contains(&TackyInstr::Return(v("tmp"))));
    }

    #[test]
    fn copy_propagation_rewrites_int128_constant_in_typed_comparison() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: TackyVal::Int128Constant(-5),
                dst: v("x"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: v("x"),
                right: TackyVal::Int128Constant(0),
                dst: v("ok"),
            },
            TackyInstr::Return(v("ok")),
        ]);
        let types = typed_vars(&[("x", CType::Int128), ("ok", CType::Int)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: TackyVal::Int128Constant(-5),
            right: TackyVal::Int128Constant(0),
            dst: v("ok"),
        }));
    }

    #[test]
    fn copy_propagation_rewrites_uint128_constant_in_typed_shift_right() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: TackyVal::UInt128Constant(1u128 << 127),
                dst: v("x"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::ShiftRight,
                left: v("x"),
                right: TackyVal::Constant(64),
                dst: v("y"),
            },
            TackyInstr::Return(v("y")),
        ]);
        let types = typed_vars(&[("x", CType::UInt128), ("y", CType::UInt128)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Binary {
            op: TackyBinaryOp::ShiftRight,
            left: TackyVal::UInt128Constant(1u128 << 127),
            right: TackyVal::Constant(64),
            dst: v("y"),
        }));
    }

    #[test]
    fn copy_propagation_treats_atomic_write_as_alias_barrier() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("a"),
                dst: v("y"),
            },
            TackyInstr::AtomicExchange {
                ptr: v("p"),
                value: TackyVal::Constant(7),
                dst: v("old"),
            },
            TackyInstr::Return(v("y")),
        ]);
        let types = typed_vars(&[
            ("a", CType::Int),
            ("y", CType::Int),
            ("p", CType::Pointer),
            ("old", CType::Int),
        ]);
        let aliased = HashSet::from(["a".to_string()]);

        copy_propagation(&mut cfg, &aliased, &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::Return(v("y"))));
    }

    #[test]
    fn copy_propagation_rewrites_atomic_fetch_operands() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: v("p"),
                dst: v("q"),
            },
            TackyInstr::Copy {
                src: v("x"),
                dst: v("arg"),
            },
            TackyInstr::AtomicFetch {
                op: TackyBinaryOp::Add,
                ptr: v("q"),
                arg: v("arg"),
                return_old: false,
                dst: v("result"),
            },
            TackyInstr::Return(v("result")),
        ]);
        let types = typed_vars(&[
            ("p", CType::Pointer),
            ("q", CType::Pointer),
            ("x", CType::Int),
            ("arg", CType::Int),
            ("result", CType::Int),
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::AtomicFetch {
            op: TackyBinaryOp::Add,
            ptr: v("p"),
            arg: v("x"),
            return_old: false,
            dst: v("result"),
        }));
    }

    #[test]
    fn copy_propagation_rewrites_copy_from_offset_struct_source() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::CopyStruct {
                src_name: "src".to_string(),
                dst_name: "tmp".to_string(),
            },
            TackyInstr::CopyFromOffset {
                src_name: "tmp".to_string(),
                offset: 8,
                dst: v("field"),
            },
            TackyInstr::Return(v("field")),
        ]);
        let types = typed_vars(&[("field", CType::Int)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::CopyFromOffset {
            src_name: "src".to_string(),
            offset: 8,
            dst: v("field"),
        }));
    }

    #[test]
    fn copy_propagation_rewrites_copy_struct_source() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::CopyStruct {
                src_name: "src".to_string(),
                dst_name: "tmp".to_string(),
            },
            TackyInstr::CopyStruct {
                src_name: "tmp".to_string(),
                dst_name: "dst".to_string(),
            },
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &IndexMap::new());

        assert!(cfg.to_instructions().contains(&TackyInstr::CopyStruct {
            src_name: "src".to_string(),
            dst_name: "dst".to_string(),
        }));
    }

    #[test]
    fn copy_propagation_transfers_rewritten_copy_struct() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::CopyStruct {
                src_name: "src".to_string(),
                dst_name: "tmp".to_string(),
            },
            TackyInstr::CopyStruct {
                src_name: "tmp".to_string(),
                dst_name: "dst".to_string(),
            },
            TackyInstr::CopyFromOffset {
                src_name: "dst".to_string(),
                offset: 8,
                dst: v("field"),
            },
        ]);
        let types = typed_vars(&[("field", CType::Int)]);

        copy_propagation(&mut cfg, &HashSet::new(), &types);

        assert!(cfg.to_instructions().contains(&TackyInstr::CopyFromOffset {
            src_name: "src".to_string(),
            offset: 8,
            dst: v("field"),
        }));
    }

    #[test]
    fn copy_propagation_eliminates_reverse_redundant_copy_struct() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::CopyStruct {
                src_name: "src".to_string(),
                dst_name: "tmp".to_string(),
            },
            TackyInstr::CopyStruct {
                src_name: "tmp".to_string(),
                dst_name: "src".to_string(),
            },
        ]);

        copy_propagation(&mut cfg, &HashSet::new(), &IndexMap::new());

        assert_eq!(cfg.to_instructions().len(), 1);
    }

    #[test]
    fn dead_store_elimination_keeps_store_read_by_atomic_exchange() {
        let store_to_aliased = TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: v("a"),
        };
        let mut cfg = Cfg::build(vec![
            store_to_aliased.clone(),
            TackyInstr::AtomicExchange {
                ptr: v("p"),
                value: TackyVal::Constant(2),
                dst: v("old"),
            },
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);
        let aliased = HashSet::from(["a".to_string()]);

        dead_store_elimination(&mut cfg, &aliased, &HashSet::new());

        assert!(cfg.to_instructions().contains(&store_to_aliased));
    }

    #[test]
    fn dead_store_elimination_keeps_setjmp_with_unused_result() {
        let setjmp = TackyInstr::BuiltinSetjmp {
            buf: v("env"),
            dst: v("unused_result"),
            label: "resume".to_string(),
            end_label: "done".to_string(),
        };
        let mut cfg = Cfg::build(vec![setjmp.clone(), TackyInstr::Return(v("value"))]);

        dead_store_elimination(&mut cfg, &HashSet::new(), &HashSet::new());

        assert!(cfg.to_instructions().contains(&setjmp));
    }

    #[test]
    fn dead_store_elimination_removes_dead_copy_to_offset_and_source() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::Copy {
                src: TackyVal::Constant(1),
                dst: v("field"),
            },
            TackyInstr::CopyToOffset {
                src: v("field"),
                dst_name: "agg".to_string(),
                offset: 0,
            },
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);

        dead_store_elimination(&mut cfg, &HashSet::new(), &HashSet::new());

        assert_eq!(
            cfg.to_instructions(),
            vec![TackyInstr::Return(TackyVal::Constant(0))]
        );
    }

    #[test]
    fn dead_store_elimination_keeps_copy_to_offset_read_later() {
        let write = TackyInstr::CopyToOffset {
            src: TackyVal::Constant(1),
            dst_name: "agg".to_string(),
            offset: 0,
        };
        let mut cfg = Cfg::build(vec![
            write.clone(),
            TackyInstr::CopyFromOffset {
                src_name: "agg".to_string(),
                offset: 0,
                dst: v("field"),
            },
            TackyInstr::Return(v("field")),
        ]);

        dead_store_elimination(&mut cfg, &HashSet::new(), &HashSet::new());

        assert!(cfg.to_instructions().contains(&write));
    }

    #[test]
    fn dead_store_elimination_keeps_copy_to_offset_to_aliased_aggregate() {
        let write = TackyInstr::CopyToOffset {
            src: TackyVal::Constant(1),
            dst_name: "agg".to_string(),
            offset: 0,
        };
        let mut cfg = Cfg::build(vec![
            write.clone(),
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);
        let aliased = HashSet::from(["agg".to_string()]);

        dead_store_elimination(&mut cfg, &aliased, &HashSet::new());

        assert!(cfg.to_instructions().contains(&write));
    }

    #[test]
    fn dead_store_elimination_keeps_copy_to_offset_to_static_aggregate() {
        let write = TackyInstr::CopyToOffset {
            src: TackyVal::Constant(1),
            dst_name: "agg".to_string(),
            offset: 0,
        };
        let mut cfg = Cfg::build(vec![
            write.clone(),
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);
        let statics = HashSet::from(["agg".to_string()]);

        dead_store_elimination(&mut cfg, &HashSet::new(), &statics);

        assert!(cfg.to_instructions().contains(&write));
    }

    #[test]
    fn dead_store_elimination_keeps_aggregate_write_before_unknown_store() {
        let write = TackyInstr::CopyToOffset {
            src: TackyVal::Constant(1),
            dst_name: "agg".to_string(),
            offset: 0,
        };
        let mut cfg = Cfg::build(vec![
            write.clone(),
            TackyInstr::Store {
                src: TackyVal::Constant(2),
                dst_ptr: v("p"),
            },
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);

        dead_store_elimination(&mut cfg, &HashSet::new(), &HashSet::new());

        assert!(cfg.to_instructions().contains(&write));
    }

    #[test]
    fn dead_store_elimination_removes_dead_copy_struct_and_source_write() {
        let mut cfg = Cfg::build(vec![
            TackyInstr::CopyToOffset {
                src: TackyVal::Constant(1),
                dst_name: "src".to_string(),
                offset: 0,
            },
            TackyInstr::CopyStruct {
                src_name: "src".to_string(),
                dst_name: "dst".to_string(),
            },
            TackyInstr::Return(TackyVal::Constant(0)),
        ]);

        dead_store_elimination(&mut cfg, &HashSet::new(), &HashSet::new());

        assert_eq!(
            cfg.to_instructions(),
            vec![TackyInstr::Return(TackyVal::Constant(0))]
        );
    }

    #[test]
    fn dead_store_elimination_keeps_copy_struct_read_later() {
        let copy = TackyInstr::CopyStruct {
            src_name: "src".to_string(),
            dst_name: "dst".to_string(),
        };
        let mut cfg = Cfg::build(vec![
            copy.clone(),
            TackyInstr::CopyFromOffset {
                src_name: "dst".to_string(),
                offset: 0,
                dst: v("field"),
            },
            TackyInstr::Return(v("field")),
        ]);

        dead_store_elimination(&mut cfg, &HashSet::new(), &HashSet::new());

        assert!(cfg.to_instructions().contains(&copy));
    }
}
