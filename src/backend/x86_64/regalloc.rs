use crate::types::*;
use indexmap::IndexMap;
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
const COALESCING_INSTRUCTION_LIMIT: usize = 50_000;
const COALESCING_MOVE_LIMIT: usize = 20_000;

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

const AARCH64_GP_COLOR_ORDER: [Reg; 7] = [
    Reg::AX,
    Reg::DI,
    Reg::SI,
    Reg::DX,
    Reg::CX,
    Reg::R8,
    Reg::R9,
];

const AARCH64_CALLER_SAVED_GP: [Reg; 13] = [
    Reg::AX,
    Reg::DI,
    Reg::SI,
    Reg::DX,
    Reg::CX,
    Reg::R8,
    Reg::R9,
    Reg::R12,
    Reg::R10,
    Reg::R11,
    Reg::R13,
    Reg::R14,
    Reg::R15,
];

const AARCH64_ARG_INT_REGS: [Reg; 8] = [
    Reg::AX,
    Reg::DI,
    Reg::SI,
    Reg::DX,
    Reg::CX,
    Reg::R8,
    Reg::R9,
    Reg::R12,
];

pub struct RegAllocProfile {
    gp_color_order: &'static [Reg],
    xmm_color_order: &'static [XmmReg],
    gp_callee_saved: &'static [Reg],
    arg_int_regs: &'static [Reg],
    arg_sse_regs: &'static [XmmReg],
    caller_saved_gp: &'static [Reg],
    caller_saved_xmm: &'static [XmmReg],
}

pub const X86_64_REG_ALLOC_PROFILE: RegAllocProfile = RegAllocProfile {
    gp_color_order: &GP_COLOR_ORDER,
    xmm_color_order: &XMM_COLOR_ORDER,
    gp_callee_saved: &GP_CALLEE_SAVED,
    arg_int_regs: &ARG_INT_REGS,
    arg_sse_regs: &ARG_SSE_REGS,
    caller_saved_gp: &CALLER_SAVED_GP,
    caller_saved_xmm: &ALL_XMM,
};

pub const AARCH64_REG_ALLOC_PROFILE: RegAllocProfile = RegAllocProfile {
    gp_color_order: &AARCH64_GP_COLOR_ORDER,
    xmm_color_order: &ARG_SSE_REGS,
    gp_callee_saved: &[],
    arg_int_regs: &AARCH64_ARG_INT_REGS,
    arg_sse_regs: &ARG_SSE_REGS,
    caller_saved_gp: &AARCH64_CALLER_SAVED_GP,
    caller_saved_xmm: &ARG_SSE_REGS,
};

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
    let mut leaders: Vec<usize> = Vec::with_capacity(instrs.len() / 2 + 1);
    leaders.push(0); // first instruction is always a leader
    let mut leader_set: HashSet<usize> = HashSet::with_capacity(instrs.len());
    leader_set.insert(0);
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
    let mut blocks: Vec<AsmBlock> = Vec::with_capacity(leaders.len());
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
    let mut label_to_block: HashMap<&str, usize> = HashMap::with_capacity(blocks.len());
    for (bi, block) in blocks.iter().enumerate() {
        if let AsmInstr::Label(label) = &instrs[block.start] {
            label_to_block.insert(label.as_str(), bi);
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
                if let Some(&ti) = label_to_block.get(label.as_str()) {
                    blocks[bi].succs.push(ti);
                    blocks[ti].preds.push(bi);
                }
            }
            AsmInstr::JmpCC(_, label) => {
                if let Some(&ti) = label_to_block.get(label.as_str()) {
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

fn push_operand_reads(out: &mut Vec<RegId>, op: &AsmOperand) {
    match op {
        AsmOperand::Reg(r) => out.push(RegId::Gp(*r)),
        AsmOperand::Xmm(x) => out.push(RegId::Xmm(*x)),
        AsmOperand::Pseudo(name) => out.push(RegId::Pseudo(name.clone())),
        AsmOperand::Indexed(base, idx, _) => {
            out.push(RegId::Gp(*base));
            out.push(RegId::Gp(*idx));
        }
        _ => {}
    }
}

fn push_operand_writes(out: &mut Vec<RegId>, op: &AsmOperand) {
    match op {
        AsmOperand::Reg(r) => out.push(RegId::Gp(*r)),
        AsmOperand::Xmm(x) => out.push(RegId::Xmm(*x)),
        AsmOperand::Pseudo(name) => out.push(RegId::Pseudo(name.clone())),
        _ => {}
    }
}

struct RegEffects {
    used: Vec<RegId>,
    updated: Vec<RegId>,
}

fn reg_effects(used: Vec<RegId>, updated: Vec<RegId>) -> RegEffects {
    RegEffects { used, updated }
}

fn find_used_and_updated_with_profile(instr: &AsmInstr, profile: &RegAllocProfile) -> RegEffects {
    match instr {
        AsmInstr::Mov(_, src, dst) => {
            let mut used = Vec::with_capacity(3);
            push_operand_reads(&mut used, src);
            // If dst is Indexed, base/idx are used for addressing
            if let AsmOperand::Indexed(base, idx, _) = dst {
                used.push(RegId::Gp(*base));
                used.push(RegId::Gp(*idx));
            }
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, src);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::Binary(_, _, src, dst) => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, src);
            push_operand_reads(&mut used, dst);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::And(_, src, dst)
        | AsmInstr::Or(_, src, dst)
        | AsmInstr::Xor(_, src, dst)
        | AsmInstr::Shl(_, src, dst)
        | AsmInstr::Shr(_, src, dst)
        | AsmInstr::Sar(_, src, dst)
        | AsmInstr::Ror(_, src, dst)
        | AsmInstr::Rol(_, src, dst) => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, src);
            push_operand_reads(&mut used, dst);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::Test(_, src, dst) => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, src);
            push_operand_reads(&mut used, dst);
            reg_effects(used, vec![])
        }
        AsmInstr::Unary(_, _, dst) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, dst);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::Cmp(_, src, dst) => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, src);
            push_operand_reads(&mut used, dst);
            reg_effects(used, vec![])
        }
        AsmInstr::SetCC(_, dst) => {
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(vec![], updated)
        }
        AsmInstr::Push(val) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, val);
            reg_effects(used, vec![])
        }
        AsmInstr::Pop(reg) => reg_effects(vec![], vec![RegId::Gp(*reg)]),
        AsmInstr::MulFull(_, divisor) | AsmInstr::Idiv(_, divisor) | AsmInstr::Div(_, divisor) => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, divisor);
            used.push(RegId::Gp(Reg::AX));
            used.push(RegId::Gp(Reg::DX));
            reg_effects(used, vec![RegId::Gp(Reg::AX), RegId::Gp(Reg::DX)])
        }
        AsmInstr::Cdq(_) => reg_effects(vec![RegId::Gp(Reg::AX)], vec![RegId::Gp(Reg::DX)]),
        AsmInstr::Call(_, int_regs, sse_regs, _, _) => {
            let mut used = Vec::with_capacity(int_regs + sse_regs);
            for reg in profile.arg_int_regs.iter().take(*int_regs) {
                used.push(RegId::Gp(*reg));
            }
            for reg in profile.arg_sse_regs.iter().take(*sse_regs) {
                used.push(RegId::Xmm(*reg));
            }
            let mut updated =
                Vec::with_capacity(profile.caller_saved_gp.len() + profile.caller_saved_xmm.len());
            for r in profile.caller_saved_gp {
                updated.push(RegId::Gp(*r));
            }
            for x in profile.caller_saved_xmm {
                updated.push(RegId::Xmm(*x));
            }
            reg_effects(used, updated)
        }
        AsmInstr::X86SetVarargsXmmCount(_) => reg_effects(vec![], vec![RegId::Gp(Reg::AX)]),
        AsmInstr::JmpIndirect(target) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, target);
            reg_effects(used, vec![])
        }
        AsmInstr::LoadLabelAddress(_, dst) => {
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(vec![], updated)
        }
        AsmInstr::Cvtsi2sd(_, src, dst)
        | AsmInstr::Cvtsi2ss(_, src, dst)
        | AsmInstr::Cvttsd2si(_, src, dst)
        | AsmInstr::Cvttss2si(_, src, dst)
        | AsmInstr::AArch64UIntToDouble(_, src, dst)
        | AsmInstr::AArch64UIntToFloat(_, src, dst)
        | AsmInstr::AArch64DoubleToUInt(_, src, dst)
        | AsmInstr::AArch64FloatToUInt(_, src, dst) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, src);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::Cvtss2sd(src, dst)
        | AsmInstr::Cvtsd2ss(src, dst)
        | AsmInstr::AArch64FloatToDouble(src, dst)
        | AsmInstr::AArch64DoubleToFloat(src, dst) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, src);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::X87Load(_, src) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, src);
            reg_effects(used, vec![])
        }
        AsmInstr::X87Store(dst) => {
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(vec![], updated)
        }
        AsmInstr::X87StoreFloat(_, _) => reg_effects(vec![], vec![]),
        AsmInstr::X87StoreInt(_, dst) => {
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(vec![], updated)
        }
        AsmInstr::X87LoadIndirect(_, reg) | AsmInstr::X87StoreIndirect(reg) => {
            reg_effects(vec![RegId::Gp(*reg)], vec![])
        }
        AsmInstr::X87UnaryNeg | AsmInstr::X87Binary(_) | AsmInstr::X87Compare => {
            reg_effects(vec![], vec![])
        }
        AsmInstr::Lea(src, dst) => {
            // Lea reads address components from src, writes result to dst
            let mut used = Vec::with_capacity(3);
            match src {
                AsmOperand::Pseudo(name) => used.push(RegId::Pseudo(name.clone())),
                AsmOperand::Indexed(base, idx, _) => {
                    used.push(RegId::Gp(*base));
                    used.push(RegId::Gp(*idx));
                }
                AsmOperand::Stack(_) => {}
                _ => push_operand_reads(&mut used, src),
            }
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::LoadIndirect(_, reg, dst) => {
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(vec![RegId::Gp(*reg)], updated)
        }
        AsmInstr::StoreIndirect(_, src, reg) => {
            let mut used = Vec::with_capacity(3);
            push_operand_reads(&mut used, src);
            used.push(RegId::Gp(*reg));
            reg_effects(used, vec![])
        }
        AsmInstr::AArch64AddPtr(ptr, index, _, dst) => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, ptr);
            push_operand_reads(&mut used, index);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::AArch64LoadAdjusted(_, src, dst, _) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, src);
            reg_effects(used, vec![RegId::Gp(*dst)])
        }
        AsmInstr::AArch64StoreOutgoingArg(_, src, _, _) => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, src);
            reg_effects(used, vec![])
        }
        AsmInstr::AArch64Rem(_, _, left, right, dst) => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, left);
            push_operand_reads(&mut used, right);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::CopyToStackArg { src_ptr, .. } => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, src_ptr);
            reg_effects(
                used,
                vec![RegId::Gp(Reg::SI), RegId::Gp(Reg::DI), RegId::Gp(Reg::CX)],
            )
        }
        AsmInstr::CopyFromStackArg { .. } => reg_effects(
            vec![],
            vec![RegId::Gp(Reg::SI), RegId::Gp(Reg::DI), RegId::Gp(Reg::CX)],
        ),
        AsmInstr::BuiltinSetjmp { buf, dst, .. } => {
            let mut used = Vec::with_capacity(2);
            push_operand_reads(&mut used, buf);
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::BuiltinLongjmp { buf, value } => {
            let mut used = Vec::with_capacity(4);
            push_operand_reads(&mut used, buf);
            push_operand_reads(&mut used, value);
            reg_effects(used, vec![])
        }
        AsmInstr::AtomicRmw(_, _, return_old, dst) => {
            let used = vec![RegId::Gp(Reg::R10), RegId::Gp(Reg::R11)];
            let mut updated = Vec::with_capacity(3);
            push_operand_writes(&mut updated, dst);
            if *return_old {
                updated.push(RegId::Gp(Reg::AX));
                updated.push(RegId::Gp(Reg::R12));
            }
            reg_effects(used, updated)
        }
        AsmInstr::AtomicExchange(_, dst) => {
            let used = vec![RegId::Gp(Reg::R10), RegId::Gp(Reg::R11)];
            let mut updated = Vec::with_capacity(2);
            push_operand_writes(&mut updated, dst);
            reg_effects(used, updated)
        }
        AsmInstr::AtomicCompareExchange(_, dst) => {
            let used = vec![
                RegId::Gp(Reg::R10),
                RegId::Gp(Reg::R11),
                RegId::Gp(Reg::R12),
            ];
            let mut updated = Vec::with_capacity(3);
            push_operand_writes(&mut updated, dst);
            updated.push(RegId::Gp(Reg::AX));
            updated.push(RegId::Gp(Reg::R10));
            reg_effects(used, updated)
        }
        AsmInstr::AtomicCompareSwap(_, return_old, dst) => {
            let used = vec![
                RegId::Gp(Reg::R10),
                RegId::Gp(Reg::R11),
                RegId::Gp(Reg::R12),
            ];
            let mut updated = Vec::with_capacity(4);
            push_operand_writes(&mut updated, dst);
            updated.push(RegId::Gp(Reg::AX));
            if !return_old {
                updated.push(RegId::Gp(Reg::R10));
            }
            reg_effects(used, updated)
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
        | AsmInstr::AArch64SaveLink(..)
        | AsmInstr::AArch64RestoreLink(..)
        | AsmInstr::AArch64AllocateLargeStack(..)
        | AsmInstr::AArch64DeallocateLargeStack(..)
        | AsmInstr::AArch64StoreLargeLocalBase { .. }
        | AsmInstr::AtomicFence
        | AsmInstr::Fld(_, _)
        | AsmInstr::Fstp(_, _)
        | AsmInstr::Fisttp(_, _)
        | AsmInstr::Fxch
        | AsmInstr::FstpQ
        | AsmInstr::FldQ(_)
        | AsmInstr::X87Push(_, _)
        | AsmInstr::X87Pop(_, _) => reg_effects(vec![], vec![]),
    }
}

#[derive(Debug)]
struct MovOperands {
    src: RegId,
    dst: RegId,
}

/// Check if instruction is a plain Mov between register-allocatable operands.
fn mov_operands(instr: &AsmInstr) -> Option<MovOperands> {
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
                return Some(MovOperands { src: s, dst: d });
            }
        }
    }
    None
}

// ============================================================
// Liveness Analysis (backward dataflow)
// ============================================================

#[cfg(test)]
fn liveness_analysis(instrs: &[AsmInstr], exit_live: &HashSet<RegId>) -> Vec<HashSet<RegId>> {
    liveness_analysis_with_profile(instrs, exit_live, &X86_64_REG_ALLOC_PROFILE)
}

fn liveness_analysis_with_profile(
    instrs: &[AsmInstr],
    exit_live: &HashSet<RegId>,
    profile: &RegAllocProfile,
) -> Vec<HashSet<RegId>> {
    let blocks = build_asm_cfg(instrs);
    if blocks.is_empty() {
        return vec![HashSet::new(); instrs.len()];
    }

    let num_blocks = blocks.len();
    let mut block_live_in: Vec<HashSet<RegId>> = vec![HashSet::new(); num_blocks];
    let mut worklist: VecDeque<usize> = VecDeque::with_capacity(num_blocks);
    worklist.extend((0..num_blocks).rev());
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
            let effects = find_used_and_updated_with_profile(&instrs[i], profile);
            for u in &effects.updated {
                live.remove(u);
            }
            for u in &effects.used {
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
            let effects = find_used_and_updated_with_profile(&instrs[i], profile);
            for u in &effects.updated {
                live.remove(u);
            }
            for u in &effects.used {
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
    profile: &RegAllocProfile,
) -> Graph {
    let mut graph = Graph::new();
    let node_capacity = hard_reg_ids.len() + candidates.len();
    graph.adj.reserve(node_capacity);
    graph.spill_cost.reserve(node_capacity);
    graph.color.reserve(node_capacity);

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
    // Count occurrences for spill cost.
    let candidate_names: Vec<&String> = candidates.iter().collect();
    let mut candidate_indices: HashMap<&str, usize> = HashMap::with_capacity(candidates.len());
    for (idx, name) in candidate_names.iter().enumerate() {
        candidate_indices.insert(name.as_str(), idx);
    }
    let mut occurrence_count = vec![0.0f64; candidate_names.len()];
    for instr in instrs {
        let effects = find_used_and_updated_with_profile(instr, profile);
        for id in effects.used.iter().chain(effects.updated.iter()) {
            if let RegId::Pseudo(name) = id {
                if let Some(&idx) = candidate_indices.get(name.as_str()) {
                    occurrence_count[idx] += 1.0;
                }
            }
        }
        if let AsmInstr::LoadLabelAddress(_, AsmOperand::Pseudo(name)) = instr {
            if let Some(&idx) = candidate_indices.get(name.as_str()) {
                occurrence_count[idx] += 1000.0;
            }
        }
    }
    for (idx, name) in candidate_names.iter().enumerate() {
        graph.add_node(RegId::Pseudo((*name).clone()), occurrence_count[idx]);
    }

    // Add interference edges from liveness
    for (i, instr) in instrs.iter().enumerate() {
        let effects = find_used_and_updated_with_profile(instr, profile);
        // Mov exception: a move whose src and dst can share a register need not
        // interfere. This only holds when the move copies the *whole* value.
        // A Longword move is value-preserving only when its source is dead
        // afterwards (nothing reads the source's upper 32 bits): on x86-64 a
        // 32-bit write zero-extends into the full register, so if the source is
        // still live, coalescing dst onto src would collapse the move into
        // `movl %r, %r` (kept by `should_keep_mov`) and clobber the source's
        // upper half — see the `(int)` truncation of a live `long long`. Since
        // the exception only ever drops the edge when the source *is* live, we
        // simply never apply it to Longword moves; coalescing still fires for
        // dead-source Longword moves, which don't get an edge in the first place.
        let is_mov = mov_operands(instr);
        let mov_src = is_mov.as_ref().and_then(|mov| {
            if matches!(instr, AsmInstr::Mov(AsmType::Longword, _, _)) {
                None
            } else {
                Some(&mov.src)
            }
        });

        for u in &effects.updated {
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
    // Collect pseudo nodes
    let mut pseudo_nodes = Vec::with_capacity(graph.adj.len().saturating_sub(hard_reg_ids.len()));
    for id in graph.adj.keys() {
        if !hard_reg_ids.contains(id) {
            pseudo_nodes.push(id.clone());
        }
    }

    if pseudo_nodes.is_empty() {
        return;
    }

    // Simplify: push nodes to stack. Keep mutable degrees so each prune is
    // proportional to the removed node's neighbors, not to all remaining nodes.
    let mut stack: Vec<RegId> = Vec::with_capacity(pseudo_nodes.len());
    let mut remaining: HashSet<RegId> = HashSet::with_capacity(pseudo_nodes.len());
    let mut degree: HashMap<RegId, usize> = HashMap::with_capacity(pseudo_nodes.len());
    let mut low_degree: VecDeque<RegId> = VecDeque::with_capacity(pseudo_nodes.len());
    for node in &pseudo_nodes {
        remaining.insert(node.clone());
        let deg = graph.adj.get(node).map(|nbrs| nbrs.len()).unwrap_or(0);
        degree.insert(node.clone(), deg);
        if deg < k {
            low_degree.push_back(node.clone());
        }
    }

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

    fn find(&mut self, x: &RegId) -> RegId {
        match self.parent.get(x).cloned() {
            Some(parent) if parent != *x => {
                let root = self.find(&parent);
                self.parent.insert(x.clone(), root.clone());
                root
            }
            Some(parent) => parent,
            None => x.clone(),
        }
    }

    // Callers rely on `find(x)` resolving to `y` after `union(x, y)` — in
    // particular `coalesce_pass` merges a pseudo (`x`) into what may be a
    // pre-colored hard register (`y`), which must remain the canonical root.
    // So `x`'s root always gets attached under `y`'s root, never the reverse.
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

#[derive(Debug)]
struct CoalesceCandidate {
    idx: usize,
    mov: MovOperands,
    hard_priority: bool,
    spill_score: f64,
}

fn collect_coalesce_candidates(instrs: &[AsmInstr], graph: &Graph) -> Vec<CoalesceCandidate> {
    let mut candidates = Vec::new();
    for (idx, instr) in instrs.iter().enumerate() {
        if let Some(mov) = mov_operands(instr) {
            let spill_score = graph.spill_cost.get(&mov.src).copied().unwrap_or(0.0)
                + graph.spill_cost.get(&mov.dst).copied().unwrap_or(0.0);
            candidates.push(CoalesceCandidate {
                idx,
                hard_priority: is_hard_reg(&mov.src) || is_hard_reg(&mov.dst),
                spill_score,
                mov,
            });
        }
    }
    candidates.sort_by(|a, b| {
        b.hard_priority
            .cmp(&a.hard_priority)
            .then_with(|| {
                b.spill_score
                    .partial_cmp(&a.spill_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then(a.idx.cmp(&b.idx))
    });
    candidates
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

    for candidate in collect_coalesce_candidates(instrs, graph) {
        let mov = candidate.mov;
        let src_r = uf.find(&mov.src);
        let dst_r = uf.find(&mov.dst);
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
                .map(|nbrs| nbrs.iter().cloned().collect())
                .unwrap_or_default();
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

    if coalesced_any {
        Some(uf)
    } else {
        None
    }
}

fn rewrite_coalesced(instrs: &mut Vec<AsmInstr>, uf: &mut UnionFind) {
    fn rewrite_op(op: &mut AsmOperand, uf: &mut UnionFind) {
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

    let old_instrs = std::mem::take(instrs);
    let mut new_instrs = Vec::with_capacity(old_instrs.len());

    for mut instr in old_instrs {
        match &mut instr {
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

        if should_keep_mov(&instr) {
            new_instrs.push(instr);
        }
    }

    *instrs = new_instrs;
}

fn is_self_move(a: &AsmOperand, b: &AsmOperand) -> bool {
    match (a, b) {
        (AsmOperand::Reg(r1), AsmOperand::Reg(r2)) => r1 == r2,
        (AsmOperand::Xmm(x1), AsmOperand::Xmm(x2)) => x1 == x2,
        (AsmOperand::Pseudo(n1), AsmOperand::Pseudo(n2)) => n1 == n2,
        _ => false,
    }
}

fn should_keep_mov(instr: &AsmInstr) -> bool {
    match instr {
        AsmInstr::Mov(AsmType::Longword, AsmOperand::Reg(src), AsmOperand::Reg(dst))
            if src == dst =>
        {
            true
        }
        AsmInstr::Mov(_, src, dst) => !is_self_move(src, dst),
        _ => true,
    }
}

// ============================================================
// Apply Coloring: replace pseudos with hard registers
// ============================================================

fn build_color_map(graph: &Graph, hard_reg_ids: &[RegId]) -> HashMap<String, RegId> {
    // Map color index → hard register
    let mut color_to_reg: HashMap<usize, RegId> = HashMap::with_capacity(hard_reg_ids.len());
    for (i, hr) in hard_reg_ids.iter().enumerate() {
        color_to_reg.insert(i, hr.clone());
    }

    let mut map = HashMap::with_capacity(graph.color.len());
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

    let old_instrs = std::mem::take(instrs);
    let mut new_instrs = Vec::with_capacity(old_instrs.len());

    for mut instr in old_instrs {
        match &mut instr {
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
            AsmInstr::AArch64AddPtr(ptr, index, _, dst) => {
                replace_op(ptr, map);
                replace_op(index, map);
                replace_op(dst, map);
            }
            AsmInstr::AArch64LoadAdjusted(_, src, _, _) => {
                replace_op(src, map);
            }
            AsmInstr::AArch64StoreOutgoingArg(_, src, _, _) => {
                replace_op(src, map);
            }
            AsmInstr::AArch64Rem(_, _, left, right, dst) => {
                replace_op(left, map);
                replace_op(right, map);
                replace_op(dst, map);
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

        if should_keep_mov(&instr) {
            new_instrs.push(instr);
        }
    }

    *instrs = new_instrs;
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
    types: &IndexMap<String, CType>,
    arr_sizes: &IndexMap<String, usize>,
    ret_regs: &[RegId],
    no_coalescing: bool,
) -> RegAllocResult {
    allocate_registers_with_profile(
        func,
        aliased,
        types,
        arr_sizes,
        ret_regs,
        &X86_64_REG_ALLOC_PROFILE,
        no_coalescing,
    )
}

pub fn allocate_registers_with_profile(
    func: &mut AsmFunction,
    aliased: &HashSet<String>,
    types: &IndexMap<String, CType>,
    arr_sizes: &IndexMap<String, usize>,
    ret_regs: &[RegId],
    profile: &RegAllocProfile,
    no_coalescing: bool,
) -> RegAllocResult {
    let mut exit_live: HashSet<RegId> = HashSet::with_capacity(ret_regs.len());
    for reg in ret_regs {
        exit_live.insert(reg.clone());
    }

    // Determine candidate pseudo-registers
    let mut gp_candidates: HashSet<String> = HashSet::with_capacity(func.instructions.len());
    let mut xmm_candidates: HashSet<String> = HashSet::with_capacity(func.instructions.len());
    let mut x87_float_store_dsts: HashSet<String> = HashSet::with_capacity(func.instructions.len());

    for instr in &func.instructions {
        if let AsmInstr::X87StoreFloat(_, AsmOperand::Pseudo(name)) = instr {
            x87_float_store_dsts.insert(name.clone());
        }
    }

    // Scan instructions for all pseudo names
    for instr in &func.instructions {
        let effects = find_used_and_updated_with_profile(instr, profile);
        for id in effects.used.iter().chain(effects.updated.iter()) {
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
    let mut gp_hard_ids: Vec<RegId> = Vec::with_capacity(profile.gp_color_order.len());
    for r in profile.gp_color_order {
        gp_hard_ids.push(RegId::Gp(*r));
    }
    allocate_one_pass(
        &mut func.instructions,
        &exit_live,
        &gp_candidates,
        &gp_hard_ids,
        profile.gp_color_order.len(),
        profile,
        no_coalescing,
    );

    // --- XMM Register Allocation ---
    let mut xmm_hard_ids: Vec<RegId> = Vec::with_capacity(profile.xmm_color_order.len());
    for r in profile.xmm_color_order {
        xmm_hard_ids.push(RegId::Xmm(*r));
    }
    allocate_one_pass(
        &mut func.instructions,
        &exit_live,
        &xmm_candidates,
        &xmm_hard_ids,
        profile.xmm_color_order.len(),
        profile,
        no_coalescing,
    );

    // Determine which callee-saved GP registers were used
    let mut callee_saved_used: Vec<Reg> = Vec::new();
    let mut callee_saved_seen: HashSet<Reg> = HashSet::with_capacity(profile.gp_callee_saved.len());
    for instr in &func.instructions {
        visit_operands(instr, |op| {
            if let AsmOperand::Reg(r) = op {
                if profile.gp_callee_saved.contains(r) && callee_saved_seen.insert(*r) {
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
    profile: &RegAllocProfile,
    no_coalescing: bool,
) {
    if candidates.is_empty() {
        return;
    }

    let skip_coalescing =
        no_coalescing || coalescing_would_be_too_expensive(instrs, COALESCING_MOVE_LIMIT);

    // Build-coalesce loop
    let mut graph;
    loop {
        let live_after = liveness_analysis_with_profile(instrs, exit_live, profile);
        graph = build_interference_graph(instrs, &live_after, candidates, hard_reg_ids, k, profile);

        if skip_coalescing {
            break;
        }

        if let Some(uf) = coalesce_pass(instrs, &mut graph, hard_reg_ids, k) {
            let mut uf = uf;
            rewrite_coalesced(instrs, &mut uf);
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

fn coalescing_would_be_too_expensive(instrs: &[AsmInstr], move_limit: usize) -> bool {
    instrs.len() > COALESCING_INSTRUCTION_LIMIT
        || instrs
            .iter()
            .filter(|instr| mov_operands(instr).is_some())
            .take(move_limit + 1)
            .count()
            > move_limit
}

#[cfg(test)]
mod candidate_tests {
    use super::*;

    #[test]
    fn hard_register_move_candidates_sort_first() {
        let instrs = vec![
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Pseudo("a".to_string()),
                AsmOperand::Pseudo("b".to_string()),
            ),
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Pseudo("c".to_string()),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Pseudo("d".to_string()),
                AsmOperand::Pseudo("e".to_string()),
            ),
        ];

        let mut graph = Graph::new();
        graph.add_node(RegId::Pseudo("a".to_string()), 1.0);
        graph.add_node(RegId::Pseudo("b".to_string()), 1.0);
        graph.add_node(RegId::Pseudo("c".to_string()), 10.0);
        graph.add_node(RegId::Gp(Reg::AX), f64::INFINITY);
        graph.add_node(RegId::Pseudo("d".to_string()), 2.0);
        graph.add_node(RegId::Pseudo("e".to_string()), 2.0);

        let candidates = collect_coalesce_candidates(&instrs, &graph);

        assert!(candidates[0].hard_priority);
        assert_eq!(candidates[0].idx, 1);
        assert_eq!(candidates[1].idx, 2);
        assert_eq!(candidates[2].idx, 0);
    }
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
        AsmInstr::AArch64AddPtr(ptr, index, _, dst) => {
            f(ptr);
            f(index);
            f(dst);
        }
        AsmInstr::AArch64LoadAdjusted(_, src, _, _) => {
            f(src);
        }
        AsmInstr::AArch64StoreOutgoingArg(_, src, _, _) => {
            f(src);
        }
        AsmInstr::AArch64Rem(_, _, left, right, dst) => {
            f(left);
            f(right);
            f(dst);
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

    #[test]
    fn coalescing_cutoff_keeps_ordinary_functions_enabled() {
        let instrs = vec![AsmInstr::Mov(
            AsmType::Longword,
            AsmOperand::Pseudo("a".to_string()),
            AsmOperand::Pseudo("b".to_string()),
        )];

        assert!(!coalescing_would_be_too_expensive(
            &instrs,
            COALESCING_MOVE_LIMIT
        ));
    }

    #[test]
    fn coalescing_cutoff_trips_on_large_instruction_streams() {
        let instrs = vec![
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Pseudo("a".to_string()),
            );
            COALESCING_INSTRUCTION_LIMIT + 1
        ];

        assert!(coalescing_would_be_too_expensive(
            &instrs,
            COALESCING_MOVE_LIMIT
        ));
    }

    #[test]
    fn coalescing_cutoff_trips_on_many_move_candidates() {
        let instrs = vec![
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Pseudo("a".to_string()),
                AsmOperand::Pseudo("b".to_string()),
            );
            3
        ];

        assert!(coalescing_would_be_too_expensive(&instrs, 2));
    }
}
