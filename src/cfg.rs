use crate::types::*;
use std::collections::{HashMap, HashSet, VecDeque};

// ============================================================
// Control-Flow Graph
// ============================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
pub struct CFG {
    pub blocks: Vec<BasicBlock>,
    pub entry_successors: Vec<NodeId>,
    pub exit_predecessors: Vec<NodeId>,
}

impl CFG {
    pub fn build(instructions: Vec<TackyInstr>) -> Self {
        let partitioned = partition_into_basic_blocks(instructions);
        let mut blocks: Vec<BasicBlock> = partitioned.into_iter().enumerate()
            .map(|(i, instrs)| BasicBlock {
                id: i,
                instructions: instrs,
                successors: Vec::new(),
                predecessors: Vec::new(),
            }).collect();

        // Build label → block_id map
        let mut label_to_block: HashMap<String, usize> = HashMap::new();
        for block in &blocks {
            if let Some(TackyInstr::Label(label)) = block.instructions.first() {
                label_to_block.insert(label.clone(), block.id);
            }
        }

        // Add edges
        let num_blocks = blocks.len();
        let mut entry_succs = Vec::new();
        let mut exit_preds = Vec::new();

        if num_blocks > 0 {
            entry_succs.push(NodeId::Block(0));
            blocks[0].predecessors.push(NodeId::Entry);
        }

        for i in 0..num_blocks {
            let last = blocks[i].instructions.last().cloned();
            let next_id = if i + 1 < num_blocks {
                NodeId::Block(i + 1)
            } else {
                NodeId::Exit
            };

            match last {
                Some(TackyInstr::Return(_)) => {
                    blocks[i].successors.push(NodeId::Exit);
                    exit_preds.push(NodeId::Block(i));
                }
                Some(TackyInstr::Jump(ref target)) => {
                    if let Some(&target_id) = label_to_block.get(target) {
                        blocks[i].successors.push(NodeId::Block(target_id));
                        blocks[target_id].predecessors.push(NodeId::Block(i));
                    }
                }
                Some(TackyInstr::JumpIfZero(_, ref target)) |
                Some(TackyInstr::JumpIfNotZero(_, ref target)) => {
                    if let Some(&target_id) = label_to_block.get(target) {
                        blocks[i].successors.push(NodeId::Block(target_id));
                        blocks[target_id].predecessors.push(NodeId::Block(i));
                    }
                    // Fall-through to next block
                    blocks[i].successors.push(next_id.clone());
                    match &next_id {
                        NodeId::Block(j) => blocks[*j].predecessors.push(NodeId::Block(i)),
                        NodeId::Exit => exit_preds.push(NodeId::Block(i)),
                        _ => {}
                    }
                }
                _ => {
                    // Fall-through
                    blocks[i].successors.push(next_id.clone());
                    match &next_id {
                        NodeId::Block(j) => blocks[*j].predecessors.push(NodeId::Block(i)),
                        NodeId::Exit => exit_preds.push(NodeId::Block(i)),
                        _ => {}
                    }
                }
            }
        }

        CFG { blocks, entry_successors: entry_succs, exit_predecessors: exit_preds }
    }

    pub fn to_instructions(&self) -> Vec<TackyInstr> {
        let mut sorted: Vec<&BasicBlock> = self.blocks.iter().collect();
        sorted.sort_by_key(|b| b.id);
        sorted.into_iter().flat_map(|b| b.instructions.clone()).collect()
    }
}

fn partition_into_basic_blocks(instructions: Vec<TackyInstr>) -> Vec<Vec<TackyInstr>> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();

    for instr in instructions {
        match &instr {
            TackyInstr::Label(_) => {
                if !current.is_empty() {
                    blocks.push(current);
                    current = Vec::new();
                }
                current.push(instr);
            }
            TackyInstr::Jump(_) | TackyInstr::JumpIfZero(_, _) |
            TackyInstr::JumpIfNotZero(_, _) | TackyInstr::Return(_) => {
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

// ============================================================
// Address-Taken Analysis
// ============================================================

pub fn find_aliased_vars(instructions: &[TackyInstr], static_vars: &HashSet<String>) -> HashSet<String> {
    let mut aliased = static_vars.clone();
    for instr in instructions {
        if let TackyInstr::GetAddress { src: TackyVal::Var(name), .. } = instr {
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
    DoubleConstant(u64), // store as bits for Eq/Hash
}

pub fn copy_propagation(cfg: &mut CFG, aliased_vars: &HashSet<String>, types: &HashMap<String, CType>) {
    // Collect all copy instructions in the function
    let all_copies: HashSet<CopyInstr> = collect_all_copies(cfg, types);

    // Initialize block annotations with all_copies
    let mut block_out: HashMap<usize, HashSet<CopyInstr>> = HashMap::new();
    for block in &cfg.blocks {
        block_out.insert(block.id, all_copies.clone());
    }

    // Per-instruction annotations
    let mut instr_reaching: HashMap<(usize, usize), HashSet<CopyInstr>> = HashMap::new();

    // Worklist algorithm
    let mut worklist: VecDeque<usize> = (0..cfg.blocks.len()).collect();

    while let Some(block_id) = worklist.pop_front() {
        // Meet: intersection of all predecessors' out-copies
        let incoming = meet_copies(&cfg.blocks[block_id], &block_out, &all_copies);

        // Transfer function
        let (new_out, new_instr_reaching) = transfer_copies(
            &cfg.blocks[block_id], &incoming, aliased_vars, types
        );

        let old_out = block_out.get(&block_id).cloned().unwrap_or_default();
        if new_out != old_out {
            block_out.insert(block_id, new_out);
            // Add successors to worklist
            for succ in &cfg.blocks[block_id].successors {
                if let NodeId::Block(sid) = succ {
                    if !worklist.contains(sid) {
                        worklist.push_back(*sid);
                    }
                }
            }
        }
        // Update instruction annotations
        for ((bid, iid), copies) in new_instr_reaching {
            instr_reaching.insert((bid, iid), copies);
        }
    }

    // Rewrite instructions using reaching copies
    for block in &mut cfg.blocks {
        let mut new_instrs = Vec::new();
        for (i, instr) in block.instructions.iter().enumerate() {
            let reaching = instr_reaching.get(&(block.id, i)).cloned().unwrap_or_default();
            if let Some(rewritten) = rewrite_instruction(instr, &reaching, types) {
                new_instrs.push(rewritten);
            }
            // else: instruction was eliminated (redundant copy)
        }
        block.instructions = new_instrs;
    }
}

fn collect_all_copies(cfg: &CFG, types: &HashMap<String, CType>) -> HashSet<CopyInstr> {
    let mut copies = HashSet::new();
    for block in &cfg.blocks {
        for instr in &block.instructions {
            match instr {
                TackyInstr::Copy { src: TackyVal::Var(s), dst: TackyVal::Var(d) } => {
                    let st = types.get(s).copied().unwrap_or(CType::Int);
                    let dt = types.get(d).copied().unwrap_or(CType::Int);
                    if st == dt || (st.is_signed() == dt.is_signed() && st.size() == dt.size()) {
                        copies.insert(CopyInstr { src: CopySrc::Var(s.clone()), dst: d.clone() });
                    }
                }
                TackyInstr::Copy { src: TackyVal::Constant(c), dst: TackyVal::Var(d) } => {
                    copies.insert(CopyInstr { src: CopySrc::Constant(*c), dst: d.clone() });
                }
                TackyInstr::Copy { src: TackyVal::DoubleConstant(c), dst: TackyVal::Var(d) } => {
                    copies.insert(CopyInstr { src: CopySrc::DoubleConstant(c.to_bits()), dst: d.clone() });
                }
                TackyInstr::CopyStruct { src_name, dst_name } => {
                    copies.insert(CopyInstr { src: CopySrc::Var(src_name.clone()), dst: dst_name.clone() });
                }
                _ => {}
            }
        }
    }
    copies
}

fn meet_copies(block: &BasicBlock, block_out: &HashMap<usize, HashSet<CopyInstr>>, all_copies: &HashSet<CopyInstr>) -> HashSet<CopyInstr> {
    let mut incoming = all_copies.clone();
    for pred in &block.predecessors {
        match pred {
            NodeId::Entry => return HashSet::new(), // No copies reach from entry
            NodeId::Block(pid) => {
                if let Some(pred_out) = block_out.get(pid) {
                    incoming = incoming.intersection(pred_out).cloned().collect();
                }
            }
            _ => {}
        }
    }
    if block.predecessors.is_empty() {
        return all_copies.clone(); // Unreachable block
    }
    incoming
}

fn transfer_copies(
    block: &BasicBlock,
    initial: &HashSet<CopyInstr>,
    aliased: &HashSet<String>,
    types: &HashMap<String, CType>,
) -> (HashSet<CopyInstr>, HashMap<(usize, usize), HashSet<CopyInstr>>) {
    let mut current = initial.clone();
    let mut annotations = HashMap::new();

    for (i, instr) in block.instructions.iter().enumerate() {
        annotations.insert((block.id, i), current.clone());

        match instr {
            TackyInstr::Copy { src: TackyVal::Var(s), dst: TackyVal::Var(d) } => {
                let st = types.get(s).copied().unwrap_or(CType::Int);
                let dt = types.get(d).copied().unwrap_or(CType::Int);
                let same_type = st == dt || (st.is_signed() == dt.is_signed() && st.size() == dt.size());
                if same_type {
                    let copy = CopyInstr { src: CopySrc::Var(s.clone()), dst: d.clone() };
                    let reverse_redundant = current.contains(&CopyInstr { src: CopySrc::Var(d.clone()), dst: s.clone() });
                    if current.contains(&copy) || reverse_redundant {
                        // Redundant — don't modify reaching copies
                    } else {
                        // Kill copies to/from dst
                        current.retain(|c| {
                            let src_match = match &c.src { CopySrc::Var(v) => v == d, _ => false };
                            c.dst != *d && !src_match
                        });
                        current.insert(copy);
                    }
                } else {
                    // Type conversion copy — kill copies to/from dst
                    current.retain(|c| {
                        let src_match = match &c.src { CopySrc::Var(v) => v == d, _ => false };
                        c.dst != *d && !src_match
                    });
                }
            }
            TackyInstr::Copy { src: TackyVal::Constant(c), dst: TackyVal::Var(d) } => {
                let copy = CopyInstr { src: CopySrc::Constant(*c), dst: d.clone() };
                if !current.contains(&copy) {
                    current.retain(|c| {
                        let src_match = match &c.src { CopySrc::Var(v) => v == d, _ => false };
                        c.dst != *d && !src_match
                    });
                    current.insert(copy);
                }
            }
            TackyInstr::Copy { src: TackyVal::DoubleConstant(c), dst: TackyVal::Var(d) } => {
                let copy = CopyInstr { src: CopySrc::DoubleConstant(c.to_bits()), dst: d.clone() };
                if !current.contains(&copy) {
                    current.retain(|c_instr| {
                        let src_match = match &c_instr.src { CopySrc::Var(v) => v == d, _ => false };
                        c_instr.dst != *d && !src_match
                    });
                    current.insert(copy);
                }
            }
            TackyInstr::Copy { dst: TackyVal::Var(d), .. } => {
                // Type conversion copy — kill copies to/from dst
                current.retain(|c| {
                    let src_match = match &c.src { CopySrc::Var(v) => v == d, _ => false };
                    c.dst != *d && !src_match
                });
            }
            TackyInstr::FunCall { dst, .. } => {
                // Kill copies involving aliased vars or dst
                current.retain(|c| {
                    let src_aliased = match &c.src { CopySrc::Var(v) => aliased.contains(v), _ => false };
                    !src_aliased && !aliased.contains(&c.dst)
                });
                if let TackyVal::Var(d) = dst {
                    current.retain(|c| {
                        let src_match = match &c.src { CopySrc::Var(v) => v == d, _ => false };
                        c.dst != *d && !src_match
                    });
                }
            }
            TackyInstr::Store { .. } => {
                // Kill copies involving aliased vars
                current.retain(|c| {
                    let src_aliased = match &c.src { CopySrc::Var(v) => aliased.contains(v), _ => false };
                    !src_aliased && !aliased.contains(&c.dst)
                });
            }
            TackyInstr::CopyStruct { src_name, dst_name } => {
                let copy = CopyInstr { src: CopySrc::Var(src_name.clone()), dst: dst_name.clone() };
                let reverse_redundant = current.contains(&CopyInstr { src: CopySrc::Var(dst_name.clone()), dst: src_name.clone() });
                if !current.contains(&copy) && !reverse_redundant {
                    current.retain(|c| {
                        let src_match = match &c.src { CopySrc::Var(v) => v == dst_name, _ => false };
                        c.dst != *dst_name && !src_match
                    });
                    current.insert(copy);
                }
            }
            _ => {
                // Kill copies to/from dst if instruction writes to a variable
                if let Some(d) = get_instr_dst(instr) {
                    current.retain(|c| {
                        let src_match = match &c.src { CopySrc::Var(v) => v == &d, _ => false };
                        c.dst != d && !src_match
                    });
                }
            }
        }
    }

    (current, annotations)
}

fn get_instr_dst(instr: &TackyInstr) -> Option<String> {
    match instr {
        TackyInstr::Copy { dst: TackyVal::Var(n), .. } |
        TackyInstr::Binary { dst: TackyVal::Var(n), .. } |
        TackyInstr::Unary { dst: TackyVal::Var(n), .. } |
        TackyInstr::Truncate { dst: TackyVal::Var(n), .. } |
        TackyInstr::SignExtend { dst: TackyVal::Var(n), .. } |
        TackyInstr::ZeroExtend { dst: TackyVal::Var(n), .. } |
        TackyInstr::DoubleToInt { dst: TackyVal::Var(n), .. } |
        TackyInstr::DoubleToUInt { dst: TackyVal::Var(n), .. } |
        TackyInstr::IntToDouble { dst: TackyVal::Var(n), .. } |
        TackyInstr::UIntToDouble { dst: TackyVal::Var(n), .. } |
        TackyInstr::Load { dst: TackyVal::Var(n), .. } |
        TackyInstr::GetAddress { dst: TackyVal::Var(n), .. } |
        TackyInstr::CopyFromOffset { dst: TackyVal::Var(n), .. } |
        TackyInstr::AddPtr { dst: TackyVal::Var(n), .. } |
        TackyInstr::FunCall { dst: TackyVal::Var(n), .. } => Some(n.clone()),
        TackyInstr::CopyToOffset { dst_name, .. } |
        TackyInstr::CopyStruct { dst_name, .. } => Some(dst_name.clone()),
        _ => None,
    }
}

fn replace_operand(val: &TackyVal, reaching: &HashSet<CopyInstr>) -> TackyVal {
    if let TackyVal::Var(name) = val {
        // Find matching copy, prefer variable sources over constants
        let mut best: Option<&CopyInstr> = None;
        for copy in reaching {
            if copy.dst == *name {
                match (&best, &copy.src) {
                    (None, _) => best = Some(copy),
                    (Some(b), CopySrc::Var(_)) if !matches!(&b.src, CopySrc::Var(_)) => best = Some(copy),
                    (Some(b), CopySrc::Var(s)) if matches!(&b.src, CopySrc::Var(_)) => {
                        // Both are variables — pick deterministically (alphabetically)
                        if let CopySrc::Var(bs) = &b.src {
                            if s < bs { best = Some(copy); }
                        }
                    }
                    _ => {} // Keep existing best
                }
            }
        }
        if let Some(copy) = best {
            return match &copy.src {
                CopySrc::Var(s) => TackyVal::Var(s.clone()),
                CopySrc::Constant(c) => TackyVal::Constant(*c),
                CopySrc::DoubleConstant(bits) => TackyVal::DoubleConstant(f64::from_bits(*bits)),
            };
        }
    }
    val.clone()
}

fn replace_operand_typed(val: &TackyVal, reaching: &HashSet<CopyInstr>, types: &HashMap<String, CType>) -> TackyVal {
    // Like replace_operand, but for constants, check that the type would be preserved
    if let TackyVal::Var(name) = val {
        let orig_type = types.get(name).copied().unwrap_or(CType::Int);
        let mut best: Option<&CopyInstr> = None;
        for copy in reaching {
            if copy.dst == *name {
                match (&best, &copy.src) {
                    (None, CopySrc::Var(_)) => best = Some(copy),
                    (None, CopySrc::Constant(c)) => {
                        // Only use constant if it produces same AsmType as the variable
                        let const_size = if *c > i32::MAX as i64 || *c < i32::MIN as i64 { 8 } else { 4 };
                        let var_size = orig_type.size();
                        if const_size == var_size { best = Some(copy); }
                    }
                    (Some(b), CopySrc::Var(_)) if !matches!(&b.src, CopySrc::Var(_)) => best = Some(copy),
                    _ => {}
                }
            }
        }
        if let Some(copy) = best {
            return match &copy.src {
                CopySrc::Var(s) => TackyVal::Var(s.clone()),
                CopySrc::Constant(c) => TackyVal::Constant(*c),
                CopySrc::DoubleConstant(bits) => TackyVal::DoubleConstant(f64::from_bits(*bits)),
            };
        }
    }
    val.clone()
}

fn replace_operand_var_only(val: &TackyVal, reaching: &HashSet<CopyInstr>) -> TackyVal {
    if let TackyVal::Var(name) = val {
        for copy in reaching {
            if copy.dst == *name {
                if let CopySrc::Var(s) = &copy.src {
                    return TackyVal::Var(s.clone());
                }
            }
        }
    }
    val.clone()
}

fn rewrite_instruction(instr: &TackyInstr, reaching: &HashSet<CopyInstr>, types: &HashMap<String, CType>) -> Option<TackyInstr> {
    match instr {
        TackyInstr::Copy { src, dst } => {
            if let (TackyVal::Var(s), TackyVal::Var(d)) = (src, dst) {
                // Check if this copy is redundant (either direction)
                let fwd = CopyInstr { src: CopySrc::Var(s.clone()), dst: d.clone() };
                let rev = CopyInstr { src: CopySrc::Var(d.clone()), dst: s.clone() };
                if reaching.contains(&fwd) || reaching.contains(&rev) {
                    return None; // Eliminate redundant copy
                }
            } else if let TackyVal::Var(d) = dst {
                let copy_src = match src {
                    TackyVal::Constant(c) => Some(CopySrc::Constant(*c)),
                    TackyVal::DoubleConstant(c) => Some(CopySrc::DoubleConstant(c.to_bits())),
                    _ => None,
                };
                if let Some(cs) = copy_src {
                    let fwd = CopyInstr { src: cs, dst: d.clone() };
                    if reaching.contains(&fwd) {
                        return None; // Eliminate redundant copy
                    }
                }
            }
            let new_src = replace_operand(src, reaching);
            Some(TackyInstr::Copy { src: new_src, dst: dst.clone() })
        }
        TackyInstr::Return(val) => {
            Some(TackyInstr::Return(replace_operand(val, reaching)))
        }
        TackyInstr::Unary { op, src, dst } => {
            Some(TackyInstr::Unary { op: op.clone(), src: replace_operand(src, reaching), dst: dst.clone() })
        }
        TackyInstr::Binary { op, left, right, dst } => {
            // For comparisons, use typed replacement to preserve signedness
            let is_cmp = matches!(op, TackyBinaryOp::Equal | TackyBinaryOp::NotEqual |
                TackyBinaryOp::LessThan | TackyBinaryOp::LessEqual |
                TackyBinaryOp::GreaterThan | TackyBinaryOp::GreaterEqual);
            let new_left = if is_cmp { replace_operand_typed(left, reaching, types) } else { replace_operand(left, reaching) };
            let new_right = if is_cmp { replace_operand_typed(right, reaching, types) } else { replace_operand(right, reaching) };
            Some(TackyInstr::Binary {
                op: op.clone(),
                left: new_left,
                right: new_right,
                dst: dst.clone(),
            })
        }
        TackyInstr::JumpIfZero(val, target) => {
            Some(TackyInstr::JumpIfZero(replace_operand(val, reaching), target.clone()))
        }
        TackyInstr::JumpIfNotZero(val, target) => {
            Some(TackyInstr::JumpIfNotZero(replace_operand(val, reaching), target.clone()))
        }
        TackyInstr::Truncate { src, dst } => {
            Some(TackyInstr::Truncate { src: replace_operand(src, reaching), dst: dst.clone() })
        }
        TackyInstr::SignExtend { src, dst } => {
            Some(TackyInstr::SignExtend { src: replace_operand(src, reaching), dst: dst.clone() })
        }
        TackyInstr::ZeroExtend { src, dst } => {
            Some(TackyInstr::ZeroExtend { src: replace_operand(src, reaching), dst: dst.clone() })
        }
        TackyInstr::Store { src, dst_ptr } => {
            // Replace Store src, but only with constants if the type is preserved
            let new_src = replace_operand_typed(src, reaching, types);
            Some(TackyInstr::Store { src: new_src, dst_ptr: replace_operand(dst_ptr, reaching) })
        }
        TackyInstr::Load { src_ptr, dst } => {
            Some(TackyInstr::Load { src_ptr: replace_operand(src_ptr, reaching), dst: dst.clone() })
        }
        TackyInstr::DoubleToInt { src, dst } => {
            Some(TackyInstr::DoubleToInt { src: replace_operand(src, reaching), dst: dst.clone() })
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            Some(TackyInstr::DoubleToUInt { src: replace_operand(src, reaching), dst: dst.clone() })
        }
        TackyInstr::IntToDouble { src, dst } => {
            Some(TackyInstr::IntToDouble { src: replace_operand(src, reaching), dst: dst.clone() })
        }
        TackyInstr::UIntToDouble { src, dst } => {
            Some(TackyInstr::UIntToDouble { src: replace_operand(src, reaching), dst: dst.clone() })
        }
        // Don't rewrite GetAddress (uses address, not value; changing address breaks aliasing)
        TackyInstr::GetAddress { .. } => Some(instr.clone()),
        TackyInstr::FunCall { name, args, dst, stack_arg_indices, struct_arg_groups, indirect } => {
            let new_args: Vec<TackyVal> = args.iter().map(|a| replace_operand(a, reaching)).collect();
            Some(TackyInstr::FunCall { name: name.clone(), args: new_args, dst: dst.clone(), stack_arg_indices: stack_arg_indices.clone(), struct_arg_groups: struct_arg_groups.clone(), indirect: *indirect })
        }
        TackyInstr::AddPtr { ptr, index, scale, dst } => {
            Some(TackyInstr::AddPtr { ptr: replace_operand(ptr, reaching), index: replace_operand(index, reaching), scale: *scale, dst: dst.clone() })
        }
        TackyInstr::CopyToOffset { src, dst_name, offset } => {
            // Use typed replacement to avoid losing type info for small constants
            Some(TackyInstr::CopyToOffset { src: replace_operand_typed(src, reaching, types), dst_name: dst_name.clone(), offset: *offset })
        }
        TackyInstr::CopyFromOffset { src_name, offset, dst } => {
            // Check if src_name has a reaching copy (struct-to-struct copy)
            let new_src_name = {
                let mut result = src_name.clone();
                for copy in reaching.iter() {
                    if copy.dst == *src_name {
                        if let CopySrc::Var(s) = &copy.src {
                            result = s.clone();
                            break;
                        }
                    }
                }
                result
            };
            Some(TackyInstr::CopyFromOffset { src_name: new_src_name, offset: *offset, dst: dst.clone() })
        }
        TackyInstr::CopyStruct { src_name, dst_name } => {
            // Check if redundant (either direction)
            let fwd = CopyInstr { src: CopySrc::Var(src_name.clone()), dst: dst_name.clone() };
            let rev = CopyInstr { src: CopySrc::Var(dst_name.clone()), dst: src_name.clone() };
            if reaching.contains(&fwd) || reaching.contains(&rev) {
                return None; // Eliminate redundant struct copy
            }
            Some(instr.clone())
        }
        _ => Some(instr.clone()),
    }
}

// ============================================================
// Dead Store Elimination
// ============================================================

pub fn dead_store_elimination(cfg: &mut CFG, aliased_vars: &HashSet<String>, static_vars: &HashSet<String>) {
    // Liveness analysis (backward data-flow)
    let mut block_live_in: HashMap<usize, HashSet<String>> = HashMap::new();
    for block in &cfg.blocks {
        block_live_in.insert(block.id, HashSet::new());
    }

    let mut instr_live_after: HashMap<(usize, usize), HashSet<String>> = HashMap::new();

    // Worklist (process in reverse order for backward analysis)
    let mut worklist: VecDeque<usize> = (0..cfg.blocks.len()).rev().collect();

    while let Some(block_id) = worklist.pop_front() {
        // Meet: union of all successors' live-in vars
        let end_live = meet_liveness(&cfg.blocks[block_id], &block_live_in, static_vars);

        // Transfer function (backward)
        let (new_live_in, new_instr_live) = transfer_liveness(
            &cfg.blocks[block_id], &end_live, aliased_vars, static_vars
        );

        let old_live_in = block_live_in.get(&block_id).cloned().unwrap_or_default();
        if new_live_in != old_live_in {
            block_live_in.insert(block_id, new_live_in);
            // Add predecessors to worklist
            for pred in &cfg.blocks[block_id].predecessors {
                if let NodeId::Block(pid) = pred {
                    if !worklist.contains(pid) {
                        worklist.push_back(*pid);
                    }
                }
            }
        }
        for ((bid, iid), live) in new_instr_live {
            instr_live_after.insert((bid, iid), live);
        }
    }

    // Remove dead stores
    for block in &mut cfg.blocks {
        let mut new_instrs = Vec::new();
        for (i, instr) in block.instructions.iter().enumerate() {
            if is_dead_store(instr, &instr_live_after.get(&(block.id, i)).cloned().unwrap_or_default()) {
                continue; // Remove dead store
            }
            new_instrs.push(instr.clone());
        }
        block.instructions = new_instrs;
    }
}

fn meet_liveness(block: &BasicBlock, block_live_in: &HashMap<usize, HashSet<String>>, static_vars: &HashSet<String>) -> HashSet<String> {
    let mut live = HashSet::new();
    for succ in &block.successors {
        match succ {
            NodeId::Exit => {
                // Static vars are live at exit
                live.extend(static_vars.iter().cloned());
            }
            NodeId::Block(sid) => {
                if let Some(succ_live) = block_live_in.get(sid) {
                    live.extend(succ_live.iter().cloned());
                }
            }
            _ => {}
        }
    }
    live
}

fn transfer_liveness(
    block: &BasicBlock,
    end_live: &HashSet<String>,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> (HashSet<String>, HashMap<(usize, usize), HashSet<String>>) {
    let mut current = end_live.clone();
    let mut annotations = HashMap::new();

    // Process instructions in reverse
    for (i, instr) in block.instructions.iter().enumerate().rev() {
        annotations.insert((block.id, i), current.clone());

        // Kill destination, generate sources
        match instr {
            TackyInstr::FunCall { dst, args, .. } => {
                if let TackyVal::Var(d) = dst {
                    current.remove(d);
                }
                for arg in args {
                    if let TackyVal::Var(a) = arg { current.insert(a.clone()); }
                }
                // Function calls may read/write any aliased var (static + address-taken)
                current.extend(aliased_vars.iter().cloned());
            }
            // CopyToOffset modifies a sub-field — generates (not kills) the struct
            TackyInstr::CopyToOffset { src, dst_name, .. } => {
                // Generate dst_name (struct is still needed)
                current.insert(dst_name.clone());
                // Generate source
                if let TackyVal::Var(s) = src { current.insert(s.clone()); }
            }
            // CopyFromOffset reads a sub-field — generates the struct, kills dst
            TackyInstr::CopyFromOffset { src_name, dst, .. } => {
                if let TackyVal::Var(d) = dst { current.remove(d); }
                current.insert(src_name.clone());
            }
            // CopyStruct overwrites entire struct — kills dst, generates src
            TackyInstr::CopyStruct { src_name, dst_name } => {
                current.remove(dst_name);
                current.insert(src_name.clone());
            }
            // Store writes through a pointer — reads src and dst_ptr, but doesn't read aliased vars
            TackyInstr::Store { src, dst_ptr } => {
                if let TackyVal::Var(s) = src { current.insert(s.clone()); }
                if let TackyVal::Var(p) = dst_ptr { current.insert(p.clone()); }
            }
            // Load reads through a pointer — generates aliased vars
            TackyInstr::Load { src_ptr, dst } => {
                if let TackyVal::Var(d) = dst { current.remove(d); }
                if let TackyVal::Var(p) = src_ptr { current.insert(p.clone()); }
                // Load may read any aliased var
                current.extend(aliased_vars.iter().cloned());
            }
            _ => {
                if let Some(d) = get_instr_dst(instr) {
                    current.remove(&d);
                }
                // Generate: add all source variables
                for src in get_instr_sources(instr) {
                    current.insert(src);
                }
            }
        }
    }

    (current, annotations)
}

fn get_instr_sources(instr: &TackyInstr) -> Vec<String> {
    let mut srcs = Vec::new();
    let add_var = |v: &TackyVal, s: &mut Vec<String>| {
        if let TackyVal::Var(n) = v { s.push(n.clone()); }
    };
    match instr {
        TackyInstr::Copy { src, .. } => add_var(src, &mut srcs),
        TackyInstr::Unary { src, .. } => add_var(src, &mut srcs),
        TackyInstr::Binary { left, right, .. } => {
            add_var(left, &mut srcs);
            add_var(right, &mut srcs);
        }
        TackyInstr::Return(val) => add_var(val, &mut srcs),
        TackyInstr::JumpIfZero(val, _) | TackyInstr::JumpIfNotZero(val, _) => add_var(val, &mut srcs),
        TackyInstr::Truncate { src, .. } | TackyInstr::SignExtend { src, .. } |
        TackyInstr::ZeroExtend { src, .. } | TackyInstr::DoubleToInt { src, .. } |
        TackyInstr::DoubleToUInt { src, .. } | TackyInstr::IntToDouble { src, .. } |
        TackyInstr::UIntToDouble { src, .. } => add_var(src, &mut srcs),
        TackyInstr::Store { src, dst_ptr } => { add_var(src, &mut srcs); add_var(dst_ptr, &mut srcs); }
        TackyInstr::Load { src_ptr, .. } => add_var(src_ptr, &mut srcs),
        TackyInstr::GetAddress { .. } => {} // GetAddress takes address, doesn't read value
        TackyInstr::AddPtr { ptr, index, .. } => { add_var(ptr, &mut srcs); add_var(index, &mut srcs); }
        TackyInstr::CopyToOffset { src, .. } => add_var(src, &mut srcs),
        TackyInstr::CopyFromOffset { dst: _, .. } => {} // src_name is just a name, not a TackyVal
        TackyInstr::CopyStruct { src_name, .. } => srcs.push(src_name.clone()),
        _ => {}
    }
    srcs
}

fn is_dead_store(instr: &TackyInstr, live_after: &HashSet<String>) -> bool {
    // Never eliminate function calls (side effects), stores (write through pointer),
    // CopyToOffset (partial struct update), or GetAddress
    if matches!(instr, TackyInstr::FunCall { .. } | TackyInstr::Store { .. }) {
        return false;
    }
    // Never eliminate jumps, labels, returns
    if matches!(instr, TackyInstr::Jump(_) | TackyInstr::JumpIfZero(_, _) |
        TackyInstr::JumpIfNotZero(_, _) | TackyInstr::Return(_) |
        TackyInstr::Label(_) | TackyInstr::Nop) {
        return false;
    }
    // If instruction has a destination and it's not live after, it's a dead store
    if let Some(dst) = get_instr_dst(instr) {
        if !live_after.contains(&dst) {
            return true;
        }
    }
    false
}
