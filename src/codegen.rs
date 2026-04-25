use crate::types::*;
use std::collections::HashMap;

const ARG_REGISTERS: [Reg; 6] = [Reg::DI, Reg::SI, Reg::DX, Reg::CX, Reg::R8, Reg::R9];

// ============================================================
// Phase 1: TACKY → Assembly (with pseudo-registers)
// ============================================================

fn convert_val(val: &TackyVal) -> AsmOperand {
    match val {
        TackyVal::Constant(c) => AsmOperand::Imm(*c),
        TackyVal::DoubleConstant(d) => {
            // Treat as integer bits when used in non-double context
            AsmOperand::Imm(d.to_bits() as i64)
        }
        TackyVal::Var(name) => AsmOperand::Pseudo(name.clone()),
    }
}

fn val_type(val: &TackyVal, types: &HashMap<String, CType>) -> AsmType {
    match val {
        TackyVal::Constant(c) => {
            if *c > i32::MAX as i64 || *c < i32::MIN as i64 {
                AsmType::Quadword
            } else {
                AsmType::Longword
            }
        }
        TackyVal::DoubleConstant(_) => AsmType::Double,
        TackyVal::Var(name) => {
            let ct = types.get(name).copied().unwrap_or(CType::Int);
            ct.into()
        }
    }
}

/// For doubles, we use unsigned condition codes (above/below) because
/// comisd sets CF/ZF like unsigned comparisons
fn is_comparison(op: &TackyBinaryOp, is_unsigned: bool) -> Option<CondCode> {
    match (op, is_unsigned) {
        (TackyBinaryOp::Equal, _) => Some(CondCode::E),
        (TackyBinaryOp::NotEqual, _) => Some(CondCode::NE),
        (TackyBinaryOp::LessThan, false) => Some(CondCode::L),
        (TackyBinaryOp::LessEqual, false) => Some(CondCode::LE),
        (TackyBinaryOp::GreaterThan, false) => Some(CondCode::G),
        (TackyBinaryOp::GreaterEqual, false) => Some(CondCode::GE),
        (TackyBinaryOp::LessThan, true) => Some(CondCode::B),
        (TackyBinaryOp::LessEqual, true) => Some(CondCode::BE),
        (TackyBinaryOp::GreaterThan, true) => Some(CondCode::A),
        (TackyBinaryOp::GreaterEqual, true) => Some(CondCode::AE),
        _ => None,
    }
}

// Double constants need to be emitted as static data and referenced by label
static DOUBLE_CONST_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn double_const_label() -> String {
    let n = DOUBLE_CONST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    format!("__double_const_{}", n)
}

/// Convert a TackyVal for doubles, emitting a data label for double constants
fn convert_double_val(val: &TackyVal, static_doubles: &mut Vec<(String, f64)>) -> AsmOperand {
    match val {
        TackyVal::DoubleConstant(d) => {
            let label = double_const_label();
            static_doubles.push((label.clone(), *d));
            AsmOperand::Data(label)
        }
        TackyVal::Constant(c) => AsmOperand::Imm(*c),
        TackyVal::Var(name) => AsmOperand::Pseudo(name.clone()),
    }
}

fn get_struct_classes(name: &str, var_struct_tags: &HashMap<String, String>, struct_defs: &HashMap<String, StructDef>) -> Option<Vec<ParamClass>> {
    if let Some(tag) = var_struct_tags.get(name) {
        if let Some(def) = struct_defs.get(tag) {
            return Some(def.classify_with(struct_defs));
        }
    }
    None
}

fn convert_instruction(instr: &TackyInstr, types: &HashMap<String, CType>, arr_sizes: &HashMap<String, usize>, out: &mut Vec<AsmInstr>, static_doubles: &mut Vec<(String, f64)>,
    var_struct_tags: &HashMap<String, String>, struct_defs: &HashMap<String, StructDef>) {
    match instr {
        TackyInstr::Nop => { /* skip */ }
        TackyInstr::Return(val) => {
            let t = val_type(val, types);
            // Check if returning a struct
            if let TackyVal::Var(ref name) = val {
                if types.get(name).copied() == Some(CType::Struct) {
                    if let Some(classes) = get_struct_classes(name, var_struct_tags, struct_defs) {
                        let struct_size = arr_sizes.get(name).copied()
                            .or_else(|| var_struct_tags.get(name).and_then(|t| struct_defs.get(t)).map(|d| d.size))
                            .unwrap_or(8);
                        let mut int_ret_idx = 0;
                        let mut sse_ret_idx = 0;
                        let int_ret_regs = [Reg::AX, Reg::DX];
                        let sse_ret_regs = [XmmReg::XMM0, XmmReg::XMM1];
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i32;
                            // Determine bytes remaining in this eightbyte
                            let remaining = struct_size as i32 - eb_offset;
                            let eb_size = std::cmp::min(remaining, 8);
                            match class {
                                ParamClass::Sse => {
                                    if sse_ret_idx < 2 {
                                        out.push(AsmInstr::Mov(AsmType::Double,
                                            AsmOperand::PseudoMem(name.clone(), eb_offset),
                                            AsmOperand::Xmm(sse_ret_regs[sse_ret_idx].clone())));
                                        sse_ret_idx += 1;
                                    }
                                }
                                ParamClass::Integer => {
                                    if int_ret_idx < 2 {
                                        out.push(AsmInstr::Mov(AsmType::Quadword,
                                            AsmOperand::PseudoMem(name.clone(), eb_offset),
                                            AsmOperand::Reg(int_ret_regs[int_ret_idx].clone())));
                                        int_ret_idx += 1;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    out.push(AsmInstr::Ret);
                    return;
                }
            }
            if t == AsmType::Double {
                let src = convert_double_val(val, static_doubles);
                out.push(AsmInstr::Mov(AsmType::Double, src, AsmOperand::Xmm(XmmReg::XMM0)));
            } else {
                // Use Quadword for return to preserve all 64 bits
                let ret_type = if t == AsmType::Longword { AsmType::Quadword } else { t };
                out.push(AsmInstr::Mov(ret_type, convert_val(val), AsmOperand::Reg(Reg::AX)));
            }
            out.push(AsmInstr::Ret);
        }
        TackyInstr::SignExtend { src, dst } => {
            let src_t = val_type(src, types);
            let dst_t = val_type(dst, types);
            match src {
                TackyVal::Constant(c) if dst_t != AsmType::Byte => {
                    out.push(AsmInstr::Mov(dst_t, AsmOperand::Imm(*c), convert_val(dst)));
                }
                _ => {
                    out.push(AsmInstr::Movsx(src_t, dst_t, convert_val(src), convert_val(dst)));
                }
            }
        }
        TackyInstr::ZeroExtend { src, dst } => {
            let src_t = val_type(src, types);
            let dst_t = val_type(dst, types);
            match src {
                TackyVal::Constant(c) if dst_t != AsmType::Byte => {
                    out.push(AsmInstr::Mov(dst_t, AsmOperand::Imm(*c), convert_val(dst)));
                }
                _ => {
                    out.push(AsmInstr::MovZeroExtend(src_t, dst_t, convert_val(src), convert_val(dst)));
                }
            }
        }
        TackyInstr::Truncate { src, dst } => {
            let dst_t = val_type(dst, types);
            out.push(AsmInstr::Mov(dst_t, convert_val(src), convert_val(dst)));
        }
        TackyInstr::Unary { op: TackyUnaryOp::LogicalNot, src, dst } => {
            let t = val_type(src, types);
            let dst_op = convert_val(dst);
            if t == AsmType::Double {
                out.push(AsmInstr::Binary(AsmType::Double, AsmBinaryOp::Xor, AsmOperand::Xmm(XmmReg::XMM14), AsmOperand::Xmm(XmmReg::XMM14)));
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Cmp(AsmType::Double, src_op, AsmOperand::Xmm(XmmReg::XMM14)));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(src)));
            }
            out.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(0), dst_op.clone()));
            out.push(AsmInstr::SetCC(CondCode::E, dst_op));
        }
        TackyInstr::Unary { op, src, dst } => {
            let t = val_type(dst, types);
            if t == AsmType::Double && matches!(op, TackyUnaryOp::Negate) {
                // Double negation: XOR with sign bit mask (bit 63)
                // Emit a static constant with just the sign bit set
                let sign_mask_label = double_const_label();
                let sign_bit: u64 = 1u64 << 63;
                static_doubles.push((sign_mask_label.clone(), f64::from_bits(sign_bit)));
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Mov(AsmType::Double, src_op, convert_val(dst)));
                out.push(AsmInstr::Binary(AsmType::Double, AsmBinaryOp::Xor, AsmOperand::Data(sign_mask_label), convert_val(dst)));
            } else {
                let asm_op = match op {
                    TackyUnaryOp::Negate => AsmUnaryOp::Neg,
                    TackyUnaryOp::Complement => AsmUnaryOp::Not,
                    _ => unreachable!(),
                };
                out.push(AsmInstr::Mov(t, convert_val(src), convert_val(dst)));
                out.push(AsmInstr::Unary(t, asm_op, convert_val(dst)));
            }
        }
        TackyInstr::Binary { op: TackyBinaryOp::Div, left, right, dst }
            if val_type(dst, types) == AsmType::Double =>
        {
            let left_op = convert_double_val(left, static_doubles);
            let right_op = convert_double_val(right, static_doubles);
            let dst_op = convert_val(dst);
            out.push(AsmInstr::Mov(AsmType::Double, left_op, dst_op.clone()));
            out.push(AsmInstr::Binary(AsmType::Double, AsmBinaryOp::DivDouble, right_op, dst_op));
        }
        TackyInstr::Binary { op: op @ (TackyBinaryOp::Div | TackyBinaryOp::Mod), left, right, dst } => {
            let t = val_type(dst, types);
            let dst_ctype = types.get(match dst { TackyVal::Var(n) => n.as_str(), _ => "" }).copied().unwrap_or(CType::Int);
            let is_unsigned = !dst_ctype.is_signed();
            out.push(AsmInstr::Mov(t, convert_val(left), AsmOperand::Reg(Reg::AX)));
            if is_unsigned {
                // Zero EDX/RDX for unsigned division
                out.push(AsmInstr::Mov(t, AsmOperand::Imm(0), AsmOperand::Reg(Reg::DX)));
                out.push(AsmInstr::Div(t, convert_val(right)));
            } else {
                out.push(AsmInstr::Cdq(t));
                out.push(AsmInstr::Idiv(t, convert_val(right)));
            }
            let result_reg = if matches!(op, TackyBinaryOp::Mod) { Reg::DX } else { Reg::AX };
            out.push(AsmInstr::Mov(t, AsmOperand::Reg(result_reg), convert_val(dst)));
        }
        TackyInstr::Binary { op: op @ (TackyBinaryOp::ShiftLeft | TackyBinaryOp::ShiftRight), left, right, dst } => {
            let t = val_type(dst, types);
            let dst_ctype = match dst { TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int), _ => CType::Int };
            let asm_op = match op {
                TackyBinaryOp::ShiftLeft => AsmBinaryOp::Sal,
                TackyBinaryOp::ShiftRight => if dst_ctype.is_signed() { AsmBinaryOp::Sar } else { AsmBinaryOp::Shr },
                _ => unreachable!(),
            };
            out.push(AsmInstr::Mov(t, convert_val(left), convert_val(dst)));
            out.push(AsmInstr::Mov(AsmType::Longword, convert_val(right), AsmOperand::Reg(Reg::CX)));
            out.push(AsmInstr::Binary(t, asm_op, AsmOperand::Reg(Reg::CX), convert_val(dst)));
        }
        TackyInstr::Binary { op, left, right, dst } => {
            let left_ctype = match left {
                TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int),
                TackyVal::Constant(_) => CType::Int,
                TackyVal::DoubleConstant(_) => CType::Double,
            };
            // Double comparisons use unsigned condition codes (comisd sets CF/ZF)
            let is_unsigned = !left_ctype.is_signed() || left_ctype == CType::Double;

            if let Some(cc) = is_comparison(op, is_unsigned) {
                let t = val_type(left, types);
                let cmp_type = if t == AsmType::Double || val_type(right, types) == AsmType::Double {
                    AsmType::Double
                } else if t == AsmType::Quadword || val_type(right, types) == AsmType::Quadword {
                    AsmType::Quadword
                } else {
                    AsmType::Longword
                };
                if cmp_type == AsmType::Double {
                    let l = convert_double_val(left, static_doubles);
                    let r = convert_double_val(right, static_doubles);
                    out.push(AsmInstr::Cmp(AsmType::Double, r, l));
                } else {
                    out.push(AsmInstr::Cmp(cmp_type, convert_val(right), convert_val(left)));
                }
                out.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(0), convert_val(dst)));
                out.push(AsmInstr::SetCC(cc, convert_val(dst)));
            } else {
                let t = val_type(dst, types);
                if t == AsmType::Double {
                    let asm_op = match op {
                        TackyBinaryOp::Add => AsmBinaryOp::Add,
                        TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                        TackyBinaryOp::Mul => AsmBinaryOp::Mul,
                        _ => unreachable!("Unsupported double binary op: {:?}", op),
                    };
                    let l = convert_double_val(left, static_doubles);
                    let r = convert_double_val(right, static_doubles);
                    out.push(AsmInstr::Mov(AsmType::Double, l, convert_val(dst)));
                    out.push(AsmInstr::Binary(AsmType::Double, asm_op, r, convert_val(dst)));
                } else {
                    let asm_op = match op {
                        TackyBinaryOp::Add => AsmBinaryOp::Add,
                        TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                        TackyBinaryOp::Mul => AsmBinaryOp::Mul,
                        TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                        TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                        TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                        _ => unreachable!(),
                    };
                    out.push(AsmInstr::Mov(t, convert_val(left), convert_val(dst)));
                    out.push(AsmInstr::Binary(t, asm_op, convert_val(right), convert_val(dst)));
                }
            }
        }
        TackyInstr::Copy { src, dst } => {
            let t = val_type(dst, types);
            let src_op = if t == AsmType::Double || matches!(src, TackyVal::DoubleConstant(_)) {
                convert_double_val(src, static_doubles)
            } else {
                convert_val(src)
            };
            out.push(AsmInstr::Mov(t, src_op, convert_val(dst)));
        }
        TackyInstr::IntToDouble { src, dst } => {
            let src_t = val_type(src, types);
            if src_t == AsmType::Byte {
                // char→double: sign-extend to int first, then cvtsi2sd
                out.push(AsmInstr::Movsx(AsmType::Byte, AsmType::Longword, convert_val(src), AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Cvtsi2sd(AsmType::Longword, AsmOperand::Reg(Reg::R10), convert_val(dst)));
            } else {
                out.push(AsmInstr::Cvtsi2sd(src_t, convert_val(src), convert_val(dst)));
            }
        }
        TackyInstr::DoubleToInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if dst_t == AsmType::Byte {
                // double→char: cvttsd2si to int, then truncate to byte
                out.push(AsmInstr::Cvttsd2si(AsmType::Longword, convert_val(src), AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Mov(AsmType::Byte, AsmOperand::Reg(Reg::R10), convert_val(dst)));
            } else {
                out.push(AsmInstr::Cvttsd2si(dst_t, convert_val(src), convert_val(dst)));
            }
        }
        TackyInstr::UIntToDouble { src, dst } => {
            let src_t = val_type(src, types);
            if src_t == AsmType::Byte {
                // unsigned char→double: zero-extend to int, then cvtsi2sd
                out.push(AsmInstr::MovZeroExtend(AsmType::Byte, AsmType::Longword, convert_val(src), AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Cvtsi2sd(AsmType::Longword, AsmOperand::Reg(Reg::R10), convert_val(dst)));
            } else if src_t == AsmType::Longword {
                // Unsigned int (32-bit): zero-extend to R10 (64-bit), then cvtsi2sdq
                out.push(AsmInstr::MovZeroExtend(AsmType::Longword, AsmType::Quadword, convert_val(src), AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Cvtsi2sd(AsmType::Quadword, AsmOperand::Reg(Reg::R10), convert_val(dst)));
            } else {
                // Unsigned long (64-bit): need to handle values > LONG_MAX
                // Algorithm: test if negative (as signed); if not, cvtsi2sdq directly
                // If so: shift right 1, save LSB, OR LSB into shifted value,
                // cvtsi2sdq, then addsd result to itself
                let ok_label = double_const_label();
                let end_label = double_const_label();
                out.push(AsmInstr::Cmp(AsmType::Quadword, AsmOperand::Imm(0), convert_val(src)));
                out.push(AsmInstr::JmpCC(CondCode::GE, ok_label.clone()));
                // Negative as signed = >= LONG_MAX as unsigned
                out.push(AsmInstr::Mov(AsmType::Quadword, convert_val(src), AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Mov(AsmType::Quadword, AsmOperand::Reg(Reg::R10), AsmOperand::Reg(Reg::R11)));
                // R11 = src & 1 (save LSB for rounding)
                out.push(AsmInstr::Binary(AsmType::Quadword, AsmBinaryOp::And, AsmOperand::Imm(1), AsmOperand::Reg(Reg::R11)));
                // R10 = src >> 1
                out.push(AsmInstr::Binary(AsmType::Quadword, AsmBinaryOp::Shr, AsmOperand::Imm(1), AsmOperand::Reg(Reg::R10)));
                // R10 = R10 | R11 (round-to-odd)
                out.push(AsmInstr::Binary(AsmType::Quadword, AsmBinaryOp::Or, AsmOperand::Reg(Reg::R11), AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Cvtsi2sd(AsmType::Quadword, AsmOperand::Reg(Reg::R10), convert_val(dst)));
                // Double the result: dst = dst + dst
                out.push(AsmInstr::Binary(AsmType::Double, AsmBinaryOp::Add, convert_val(dst), convert_val(dst)));
                out.push(AsmInstr::Jmp(end_label.clone()));
                out.push(AsmInstr::Label(ok_label));
                out.push(AsmInstr::Cvtsi2sd(AsmType::Quadword, convert_val(src), convert_val(dst)));
                out.push(AsmInstr::Label(end_label));
            }
        }
        TackyInstr::GetAddress { src, dst } => {
            out.push(AsmInstr::Lea(convert_val(src), convert_val(dst)));
        }
        TackyInstr::Load { src_ptr, dst } => {
            let dst_t = val_type(dst, types);
            // Load pointer value into R11, then load indirectly
            out.push(AsmInstr::Mov(AsmType::Quadword, convert_val(src_ptr), AsmOperand::Reg(Reg::R11)));
            out.push(AsmInstr::LoadIndirect(dst_t, Reg::R11, convert_val(dst)));
        }
        TackyInstr::Store { src, dst_ptr } => {
            let src_t = val_type(src, types);
            // Load pointer value into R11, then store indirectly
            out.push(AsmInstr::Mov(AsmType::Quadword, convert_val(dst_ptr), AsmOperand::Reg(Reg::R11)));
            let src_op = if src_t == AsmType::Double || matches!(src, TackyVal::DoubleConstant(_)) {
                convert_double_val(src, static_doubles)
            } else {
                convert_val(src)
            };
            out.push(AsmInstr::StoreIndirect(src_t, src_op, Reg::R11));
        }
        TackyInstr::CopyToOffset { src, dst_name, offset } => {
            let src_t = val_type(src, types);
            if src_t == AsmType::Double {
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Mov(AsmType::Double, src_op, AsmOperand::PseudoMem(dst_name.clone(), *offset as i32)));
            } else {
                out.push(AsmInstr::Mov(src_t, convert_val(src), AsmOperand::PseudoMem(dst_name.clone(), *offset as i32)));
            }
        }
        TackyInstr::CopyFromOffset { src_name, offset, dst } => {
            let dst_t = val_type(dst, types);
            if dst_t == AsmType::Double {
                out.push(AsmInstr::Mov(AsmType::Double, AsmOperand::PseudoMem(src_name.clone(), *offset as i32), convert_val(dst)));
            } else {
                out.push(AsmInstr::Mov(dst_t, AsmOperand::PseudoMem(src_name.clone(), *offset as i32), convert_val(dst)));
            }
        }
        TackyInstr::AddPtr { ptr, index, scale, dst } => {
            // ptr + index * scale → dst
            // For now: Mov ptr to AX, Mov index to DX, imulq $scale, %rdx, leaq (%rax,%rdx,1), dst
            // Or simpler: Mul + Add
            out.push(AsmInstr::Mov(AsmType::Quadword, convert_val(ptr), AsmOperand::Reg(Reg::AX)));
            out.push(AsmInstr::Mov(AsmType::Quadword, convert_val(index), AsmOperand::Reg(Reg::DX)));
            if *scale == 1 || *scale == 2 || *scale == 4 || *scale == 8 {
                out.push(AsmInstr::Lea(AsmOperand::Indexed(Reg::AX, Reg::DX, *scale as i32), convert_val(dst)));
            } else {
                out.push(AsmInstr::Binary(AsmType::Quadword, AsmBinaryOp::Mul, AsmOperand::Imm(*scale), AsmOperand::Reg(Reg::DX)));
                out.push(AsmInstr::Lea(AsmOperand::Indexed(Reg::AX, Reg::DX, 1), convert_val(dst)));
            }
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if dst_t == AsmType::Byte {
                // double→unsigned char: cvttsd2si to int, truncate to byte
                out.push(AsmInstr::Cvttsd2si(AsmType::Longword, convert_val(src), AsmOperand::Reg(Reg::R10)));
                out.push(AsmInstr::Mov(AsmType::Byte, AsmOperand::Reg(Reg::R10), convert_val(dst)));
            } else {
                out.push(AsmInstr::Cvttsd2si(AsmType::Quadword, convert_val(src), AsmOperand::Reg(Reg::R10)));
                if dst_t == AsmType::Longword {
                    out.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Reg(Reg::R10), convert_val(dst)));
                } else {
                    out.push(AsmInstr::Mov(AsmType::Quadword, AsmOperand::Reg(Reg::R10), convert_val(dst)));
                }
            }
        }
        TackyInstr::Jump(label) => {
            out.push(AsmInstr::Jmp(label.clone()));
        }
        TackyInstr::JumpIfZero(val, label) => {
            let t = val_type(val, types);
            if t == AsmType::Double {
                // xorpd zeroes an xmm; comisd compares
                out.push(AsmInstr::Binary(AsmType::Double, AsmBinaryOp::Xor, AsmOperand::Xmm(XmmReg::XMM14), AsmOperand::Xmm(XmmReg::XMM14)));
                out.push(AsmInstr::Cmp(AsmType::Double, convert_val(val), AsmOperand::Xmm(XmmReg::XMM14)));
                out.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(val)));
                out.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            }
        }
        TackyInstr::JumpIfNotZero(val, label) => {
            let t = val_type(val, types);
            if t == AsmType::Double {
                out.push(AsmInstr::Binary(AsmType::Double, AsmBinaryOp::Xor, AsmOperand::Xmm(XmmReg::XMM14), AsmOperand::Xmm(XmmReg::XMM14)));
                out.push(AsmInstr::Cmp(AsmType::Double, convert_val(val), AsmOperand::Xmm(XmmReg::XMM14)));
                out.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(val)));
                out.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            }
        }
        TackyInstr::Label(label) => {
            out.push(AsmInstr::Label(label.clone()));
        }
        TackyInstr::FunCall { name, args, dst, stack_arg_indices, struct_arg_groups } => {
            // Pre-compute which args must go on stack due to struct group overflow
            let mut force_stack_args: std::collections::HashSet<usize> = std::collections::HashSet::new();
            {
                let mut sim_int = 0usize;
                let mut sim_xmm = 0usize;
                for (arg_idx, arg) in args.iter().enumerate() {
                    if stack_arg_indices.contains(&arg_idx) {
                        force_stack_args.insert(arg_idx);
                        continue;
                    }
                    let group = struct_arg_groups.iter().find(|(start, count, _)| arg_idx >= *start && arg_idx < *start + *count);
                    if let Some((start, count, is_sse_vec)) = group {
                        if arg_idx == *start {
                            let int_needed: usize = is_sse_vec.iter().filter(|&&is_sse| !is_sse).count();
                            let sse_needed: usize = is_sse_vec.iter().filter(|&&is_sse| is_sse).count();
                            if sim_int + int_needed <= 6 && sim_xmm + sse_needed <= 8 {
                                sim_int += int_needed;
                                sim_xmm += sse_needed;
                            } else {
                                for j in *start..*start + *count {
                                    force_stack_args.insert(j);
                                }
                            }
                        }
                        continue;
                    }
                    let t = val_type(arg, types);
                    if t == AsmType::Double {
                        if sim_xmm < 8 { sim_xmm += 1; }
                    } else {
                        if sim_int < 6 { sim_int += 1; }
                    }
                }
            }

            // Classify args into int regs, xmm regs, and stack
            let mut int_reg_args = Vec::new();
            let mut xmm_reg_args = Vec::new();
            let mut stack_args_list = Vec::new();
            let mut int_idx = 0usize;
            let mut xmm_idx = 0usize;

            for (arg_idx, arg) in args.iter().enumerate() {
                if force_stack_args.contains(&arg_idx) {
                    stack_args_list.push(arg);
                    continue;
                }
                let t = val_type(arg, types);
                if t == AsmType::Double {
                    if xmm_idx < 8 {
                        xmm_reg_args.push((xmm_idx, arg));
                        xmm_idx += 1;
                    } else {
                        stack_args_list.push(arg);
                    }
                } else {
                    if int_idx < 6 {
                        int_reg_args.push((int_idx, arg));
                        int_idx += 1;
                    } else {
                        stack_args_list.push(arg);
                    }
                }
            }

            let stack_count = stack_args_list.len();
            let padding = if stack_count % 2 != 0 { 8 } else { 0 };
            if padding > 0 {
                out.push(AsmInstr::AllocateStack(padding as i32));
            }
            // Push stack args in reverse
            for arg in stack_args_list.iter().rev() {
                let t = val_type(arg, types);
                if t == AsmType::Double {
                    // For doubles on the stack, just pushq the memory location
                    // For double constants (Data), push via memory reference
                    let src = convert_double_val(arg, static_doubles);
                    // pushq works with memory operands (64-bit)
                    out.push(AsmInstr::Push(src));
                } else {
                    out.push(AsmInstr::Push(convert_val(arg)));
                }
            }
            // Move int register args
            for (i, arg) in &int_reg_args {
                let t = val_type(arg, types);
                out.push(AsmInstr::Mov(t, convert_val(arg), AsmOperand::Reg(ARG_REGISTERS[*i].clone())));
            }
            // Move xmm register args
            for (i, arg) in &xmm_reg_args {
                let src = convert_double_val(arg, static_doubles);
                out.push(AsmInstr::Mov(AsmType::Double, src, AsmOperand::Xmm(XMM_ARG_REGISTERS[*i].clone())));
            }

            out.push(AsmInstr::Call(name.clone()));
            let bytes_to_dealloc = (stack_count * 8 + padding) as i32;
            if bytes_to_dealloc > 0 {
                out.push(AsmInstr::DeallocateStack(bytes_to_dealloc));
            }
            let ret_t = val_type(dst, types);
            // Check if return value is a struct
            if let TackyVal::Var(ref dst_name) = dst {
                if types.get(dst_name).copied() == Some(CType::Struct) {
                    if let Some(classes) = get_struct_classes(dst_name, var_struct_tags, struct_defs) {
                        let struct_size = arr_sizes.get(dst_name).copied()
                            .or_else(|| var_struct_tags.get(dst_name).and_then(|t| struct_defs.get(t)).map(|d| d.size))
                            .unwrap_or(8);
                        let mut int_ret_idx = 0;
                        let mut sse_ret_idx = 0;
                        let int_ret_regs = [Reg::AX, Reg::DX];
                        let sse_ret_regs = [XmmReg::XMM0, XmmReg::XMM1];
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i32;
                            let remaining = struct_size as i32 - eb_offset;
                            let eb_size = std::cmp::min(remaining, 8);
                            match class {
                                ParamClass::Sse => {
                                    if sse_ret_idx < 2 {
                                        out.push(AsmInstr::Mov(AsmType::Double,
                                            AsmOperand::Xmm(sse_ret_regs[sse_ret_idx].clone()),
                                            AsmOperand::PseudoMem(dst_name.clone(), eb_offset)));
                                        sse_ret_idx += 1;
                                    }
                                }
                                ParamClass::Integer => {
                                    if int_ret_idx < 2 {
                                        out.push(AsmInstr::Mov(AsmType::Quadword,
                                            AsmOperand::Reg(int_ret_regs[int_ret_idx].clone()),
                                            AsmOperand::PseudoMem(dst_name.clone(), eb_offset)));
                                        int_ret_idx += 1;
                                    }
                                }
                                _ => {}
                            }
                        }
                        return;
                    }
                }
            }
            if ret_t == AsmType::Double {
                out.push(AsmInstr::Mov(AsmType::Double, AsmOperand::Xmm(XmmReg::XMM0), convert_val(dst)));
            } else {
                out.push(AsmInstr::Mov(ret_t, AsmOperand::Reg(Reg::AX), convert_val(dst)));
            }
        }
    }
}

const XMM_ARG_REGISTERS: [XmmReg; 8] = [
    XmmReg::XMM0, XmmReg::XMM1, XmmReg::XMM2, XmmReg::XMM3,
    XmmReg::XMM4, XmmReg::XMM5, XmmReg::XMM6, XmmReg::XMM7,
];

fn convert_function(func: &TackyFunction, types: &HashMap<String, CType>, arr_sizes: &HashMap<String, usize>, static_doubles: &mut Vec<(String, f64)>,
    var_struct_tags: &HashMap<String, String>, struct_defs: &HashMap<String, StructDef>) -> AsmFunction {
    let mut instructions = Vec::new();

    // System V ABI: integer args in DI,SI,DX,CX,R8,R9; double args in XMM0-XMM7
    let mut int_reg_idx = 0usize;
    let mut xmm_reg_idx = 0usize;
    let mut stack_arg_idx = 0usize;

    // Pre-compute which params must go on stack due to struct group overflow
    let mut force_stack: std::collections::HashSet<usize> = std::collections::HashSet::new();
    {
        let mut sim_int_idx = 0usize;
        let mut sim_xmm_idx = 0usize;
        // Account for hidden return pointer
        for (i, param) in func.params.iter().enumerate() {
            if func.stack_params.contains(param) {
                force_stack.insert(i);
                continue;
            }
            // Check if this param is part of a struct group
            let group = func.struct_param_groups.iter().find(|(start, count, _)| i >= *start && i < *start + *count);
            if let Some((start, count, is_sse_vec)) = group {
                if i == *start {
                    // First eightbyte in group — check if ALL fit
                    let int_needed: usize = is_sse_vec.iter().filter(|&&is_sse| !is_sse).count();
                    let sse_needed: usize = is_sse_vec.iter().filter(|&&is_sse| is_sse).count();
                    if sim_int_idx + int_needed <= 6 && sim_xmm_idx + sse_needed <= 8 {
                        // All fit — consume registers
                        sim_int_idx += int_needed;
                        sim_xmm_idx += sse_needed;
                    } else {
                        // Don't fit — force all to stack
                        for j in *start..*start + *count {
                            force_stack.insert(j);
                        }
                    }
                }
                // Skip non-first eightbytes (already handled)
                continue;
            }
            // Regular param
            let t: AsmType = types.get(param).copied().unwrap_or(CType::Int).into();
            if t == AsmType::Double {
                if sim_xmm_idx < 8 { sim_xmm_idx += 1; }
                // else overflow
            } else {
                if sim_int_idx < 6 { sim_int_idx += 1; }
            }
        }
    }

    for (i, param) in func.params.iter().enumerate() {
        if force_stack.contains(&i) || func.stack_params.contains(param) {
            let offset = 16 + (stack_arg_idx * 8) as i32;
            let t: AsmType = types.get(param).copied().unwrap_or(CType::Long).into();
            let mov_type = if t == AsmType::Double { AsmType::Double } else { t };
            instructions.push(AsmInstr::Mov(mov_type, AsmOperand::Stack(offset), AsmOperand::Pseudo(param.clone())));
            stack_arg_idx += 1;
            continue;
        }
        let t: AsmType = types.get(param).copied().unwrap_or(CType::Int).into();
        if t == AsmType::Double {
            if xmm_reg_idx < 8 {
                instructions.push(AsmInstr::Mov(AsmType::Double, AsmOperand::Xmm(XMM_ARG_REGISTERS[xmm_reg_idx].clone()), AsmOperand::Pseudo(param.clone())));
                xmm_reg_idx += 1;
            } else {
                let offset = 16 + (stack_arg_idx * 8) as i32;
                instructions.push(AsmInstr::Mov(AsmType::Double, AsmOperand::Stack(offset), AsmOperand::Pseudo(param.clone())));
                stack_arg_idx += 1;
            }
        } else {
            if int_reg_idx < 6 {
                instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(ARG_REGISTERS[int_reg_idx].clone()), AsmOperand::Pseudo(param.clone())));
                int_reg_idx += 1;
            } else {
                let offset = 16 + (stack_arg_idx * 8) as i32;
                instructions.push(AsmInstr::Mov(t, AsmOperand::Stack(offset), AsmOperand::Pseudo(param.clone())));
                stack_arg_idx += 1;
            }
        }
    }

    for instr in &func.body {
        convert_instruction(instr, types, arr_sizes, &mut instructions, static_doubles, var_struct_tags, struct_defs);
    }
    AsmFunction { name: func.name.clone(), global: func.global, instructions }
}

// ============================================================
// Phase 2: Replace pseudo-registers with stack slots
// ============================================================

fn replace_pseudos(
    func: &mut AsmFunction,
    static_vars: &std::collections::HashSet<String>,
    types: &HashMap<String, CType>,
    arr_sizes: &HashMap<String, usize>,
) -> i32 {
    let mut pseudo_map: HashMap<String, i32> = HashMap::new();
    let mut stack_offset: i32 = 0;

    fn replace_operand(
        op: &mut AsmOperand,
        map: &mut HashMap<String, i32>,
        offset: &mut i32,
        statics: &std::collections::HashSet<String>,
        types: &HashMap<String, CType>,
        arr_sizes: &HashMap<String, usize>,
    ) {
        match op {
            AsmOperand::Pseudo(name) => {
                let name = name.clone();
                if statics.contains(&name) {
                    *op = AsmOperand::Data(name);
                } else {
                    let off = if let Some(&o) = map.get(&name) {
                        o
                    } else {
                        let size = if let Some(&arr_size) = arr_sizes.get(&name) {
                            arr_size as i32
                        } else {
                            let ct = types.get(&name).copied().unwrap_or(CType::Int);
                            // Void function results get stored as Longword (movl %eax, ...)
                            // so ensure at least 4 bytes
                            if ct == CType::Void { 4 } else { std::cmp::max(ct.size(), 1) }
                        };
                        let align = if let Some(&arr_size) = arr_sizes.get(&name) {
                            if arr_size >= 16 { 16 } else { std::cmp::max(size.min(16), 1) }
                        } else {
                            std::cmp::max(size.min(16), 1)
                        };
                        *offset -= size;
                        *offset &= -(align as i32);
                        map.insert(name, *offset);
                        *offset
                    };
                    *op = AsmOperand::Stack(off);
                }
            }
            AsmOperand::PseudoMem(name, mem_offset) => {
                let name = name.clone();
                let mem_off = *mem_offset;
                if statics.contains(&name) {
                    if mem_off != 0 {
                        // Static var with offset: name+offset(%rip)
                        *op = AsmOperand::Data(format!("{}+{}", name, mem_off));
                    } else {
                        *op = AsmOperand::Data(name);
                    }
                } else {
                    // Allocate if not yet allocated
                    let base_off = if let Some(&o) = map.get(&name) {
                        o
                    } else {
                        let size = if let Some(&arr_size) = arr_sizes.get(&name) {
                            arr_size as i32
                        } else {
                            types.get(&name).copied().unwrap_or(CType::Int).size()
                        };
                        let align = if size >= 16 { 16 } else { std::cmp::max(size.min(16), 1) };
                        *offset -= size;
                        *offset &= -(align as i32);
                        map.insert(name, *offset);
                        *offset
                    };
                    *op = AsmOperand::Stack(base_off + mem_off);
                }
            }
            _ => {}
        }
    }

    let r = |op: &mut AsmOperand, map: &mut HashMap<String, i32>, off: &mut i32| {
        replace_operand(op, map, off, static_vars, types, arr_sizes);
    };
    let _ = r; // suppress unused — we use the closure pattern below

    for instr in &mut func.instructions {
        match instr {
            AsmInstr::Mov(_, src, dst) | AsmInstr::Cmp(_, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::Binary(_, _, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::Unary(_, _, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::Idiv(_, op) | AsmInstr::Div(_, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::SetCC(_, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::Push(op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::Cvtsi2sd(_, src, dst) | AsmInstr::Cvttsd2si(_, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::Lea(src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::LoadIndirect(_, _, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            AsmInstr::StoreIndirect(_, src, _) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, static_vars, types, arr_sizes);
            }
            _ => {}
        }
    }

    -stack_offset
}

// ============================================================
// Phase 3: Fix up invalid instructions
// ============================================================

fn is_memory(op: &AsmOperand) -> bool {
    matches!(op, AsmOperand::Stack(_) | AsmOperand::Data(_))
}

fn fixup_instructions(func: &mut AsmFunction, stack_size: i32) {
    let aligned_stack = (stack_size + 15) & !15;
    let old_instructions = std::mem::take(&mut func.instructions);
    let mut new_instructions = Vec::new();

    // Prologue placeholder
    new_instructions.push(AsmInstr::Push(AsmOperand::Reg(Reg::AX)));
    new_instructions.push(AsmInstr::AllocateStack(aligned_stack));

    for instr in old_instructions {
        match instr {
            // mov mem, mem
            AsmInstr::Mov(AsmType::Double, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(AsmType::Double, src.clone(), AsmOperand::Xmm(XmmReg::XMM14)));
                new_instructions.push(AsmInstr::Mov(AsmType::Double, AsmOperand::Xmm(XmmReg::XMM14), dst.clone()));
            }
            AsmInstr::Mov(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // movsx mem, mem
            AsmInstr::Movsx(st, dt, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Movsx(st, dt, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(dt, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // movzx mem, mem
            AsmInstr::MovZeroExtend(st, dt, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::MovZeroExtend(st, dt, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(dt, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // idiv imm / div imm
            AsmInstr::Idiv(t, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Imm(val), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Idiv(t, AsmOperand::Reg(Reg::R10)));
            }
            AsmInstr::Div(t, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Imm(val), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Div(t, AsmOperand::Reg(Reg::R10)));
            }
            // mul with memory dst (integer only)
            AsmInstr::Binary(t, AsmBinaryOp::Mul, ref src, ref dst) if is_memory(dst) && t != AsmType::Double => {
                if is_memory(src) {
                    new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                    new_instructions.push(AsmInstr::Mov(t, dst.clone(), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Binary(t, AsmBinaryOp::Mul, AsmOperand::Reg(Reg::R10), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R11), dst.clone()));
                } else {
                    new_instructions.push(AsmInstr::Mov(t, dst.clone(), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Binary(t, AsmBinaryOp::Mul, src.clone(), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R11), dst.clone()));
                }
            }
            // double binary mem, mem
            // double binary: dst must be XMM register
            AsmInstr::Binary(AsmType::Double, ref op, ref src, ref dst) if is_memory(dst) || is_memory(src) => {
                // Load dst into XMM14 (for operations like addsd, dst is both src and dest)
                let dst_xmm = AsmOperand::Xmm(XmmReg::XMM14);
                new_instructions.push(AsmInstr::Mov(AsmType::Double, dst.clone(), dst_xmm.clone()));
                let src_op = if is_memory(src) && !matches!(src, AsmOperand::Xmm(_)) {
                    // src can be memory for SSE ops like addsd mem, xmm
                    src.clone()
                } else {
                    src.clone()
                };
                new_instructions.push(AsmInstr::Binary(AsmType::Double, op.clone(), src_op, dst_xmm.clone()));
                new_instructions.push(AsmInstr::Mov(AsmType::Double, dst_xmm, dst.clone()));
            }
            // binary mem, mem (integer)
            AsmInstr::Binary(t, ref op, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Binary(t, op.clone(), AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // double cmp: comisd src, dst — dst MUST be xmm register
            AsmInstr::Cmp(AsmType::Double, ref src, ref dst) if !matches!(dst, AsmOperand::Xmm(_)) => {
                new_instructions.push(AsmInstr::Mov(AsmType::Double, dst.clone(), AsmOperand::Xmm(XmmReg::XMM14)));
                new_instructions.push(AsmInstr::Cmp(AsmType::Double, src.clone(), AsmOperand::Xmm(XmmReg::XMM14)));
            }
            // cmp mem, mem
            AsmInstr::Cmp(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Cmp(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // cmp src, imm (dst can't be immediate)
            AsmInstr::Cmp(t, ref src, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Imm(val), AsmOperand::Reg(Reg::R11)));
                new_instructions.push(AsmInstr::Cmp(t, src.clone(), AsmOperand::Reg(Reg::R11)));
            }
            // cmp with large immediate src that doesn't fit in 32 bits
            AsmInstr::Cmp(AsmType::Quadword, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) =>
            {
                new_instructions.push(AsmInstr::Mov(AsmType::Quadword, AsmOperand::Imm(val), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Cmp(AsmType::Quadword, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // binary with large immediate for quadword
            AsmInstr::Binary(AsmType::Quadword, ref op, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) =>
            {
                new_instructions.push(AsmInstr::Mov(AsmType::Quadword, AsmOperand::Imm(val), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Binary(AsmType::Quadword, op.clone(), AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // mov with large immediate for quadword
            AsmInstr::Mov(AsmType::Quadword, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) && is_memory(dst) =>
            {
                new_instructions.push(AsmInstr::Mov(AsmType::Quadword, AsmOperand::Imm(val), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(AsmType::Quadword, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // cvtsi2sd with immediate src
            AsmInstr::Cvtsi2sd(t, AsmOperand::Imm(val), ref dst) => {
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Imm(val), AsmOperand::Reg(Reg::R10)));
                if is_memory(dst) {
                    new_instructions.push(AsmInstr::Cvtsi2sd(t, AsmOperand::Reg(Reg::R10), AsmOperand::Xmm(XmmReg::XMM14)));
                    new_instructions.push(AsmInstr::Mov(AsmType::Double, AsmOperand::Xmm(XmmReg::XMM14), dst.clone()));
                } else {
                    new_instructions.push(AsmInstr::Cvtsi2sd(t, AsmOperand::Reg(Reg::R10), dst.clone()));
                }
            }
            // cvtsi2sd with memory dst
            AsmInstr::Cvtsi2sd(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvtsi2sd(t, src.clone(), AsmOperand::Xmm(XmmReg::XMM14)));
                new_instructions.push(AsmInstr::Mov(AsmType::Double, AsmOperand::Xmm(XmmReg::XMM14), dst.clone()));
            }
            // cvttsd2si with memory src AND memory dst
            AsmInstr::Cvttsd2si(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(AsmType::Double, src.clone(), AsmOperand::Xmm(XmmReg::XMM14)));
                new_instructions.push(AsmInstr::Cvttsd2si(t, AsmOperand::Xmm(XmmReg::XMM14), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            AsmInstr::Cvttsd2si(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvttsd2si(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // lea with memory dst
            AsmInstr::Lea(ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Lea(src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(AsmType::Quadword, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // LoadIndirect with memory dst
            AsmInstr::LoadIndirect(t, ref reg, ref dst) if is_memory(dst) => {
                if t == AsmType::Double {
                    new_instructions.push(AsmInstr::LoadIndirect(t, reg.clone(), AsmOperand::Xmm(XmmReg::XMM14)));
                    new_instructions.push(AsmInstr::Mov(AsmType::Double, AsmOperand::Xmm(XmmReg::XMM14), dst.clone()));
                } else {
                    new_instructions.push(AsmInstr::LoadIndirect(t, reg.clone(), AsmOperand::Reg(Reg::R10)));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
                }
            }
            // StoreIndirect with memory src
            AsmInstr::StoreIndirect(t, ref src, ref reg) if is_memory(src) => {
                if t == AsmType::Double {
                    new_instructions.push(AsmInstr::Mov(AsmType::Double, src.clone(), AsmOperand::Xmm(XmmReg::XMM14)));
                    new_instructions.push(AsmInstr::StoreIndirect(t, AsmOperand::Xmm(XmmReg::XMM14), reg.clone()));
                } else {
                    new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                    new_instructions.push(AsmInstr::StoreIndirect(t, AsmOperand::Reg(Reg::R10), reg.clone()));
                }
            }
            // Ret → epilogue (movq %rbp, %rsp in Ret handles stack cleanup)
            AsmInstr::Ret => {
                new_instructions.push(AsmInstr::Ret);
            }
            other => {
                new_instructions.push(other);
            }
        }
    }

    func.instructions = new_instructions;
}

// ============================================================
// Public API
// ============================================================

pub fn gen(program: &TackyProgram) -> AsmProgram {
    let static_vars = &program.global_vars;
    let types = &program.symbol_types;
    let array_sizes = &program.array_sizes;
    let mut top_level = Vec::new();
    let mut static_doubles = Vec::new();

    for tl in &program.top_level {
        match tl {
            TackyTopLevel::Function(tf) => {
                let mut asm_func = convert_function(tf, types, array_sizes, &mut static_doubles, &program.var_struct_tags, &program.struct_defs);
                let stack_size = replace_pseudos(&mut asm_func, static_vars, types, array_sizes);
                fixup_instructions(&mut asm_func, stack_size);
                top_level.push(AsmTopLevel::Function(asm_func));
            }
            TackyTopLevel::StaticVar(sv) => {
                top_level.push(AsmTopLevel::StaticVar(AsmStaticVar {
                    name: sv.name.clone(),
                    global: sv.global,
                    alignment: sv.alignment,
                    init_values: sv.init_values.clone(),
                }));
            }
            TackyTopLevel::StaticConstant(sc) => {
                top_level.push(AsmTopLevel::StaticConstant(AsmStaticConstant {
                    name: sc.name.clone(),
                    alignment: sc.alignment,
                    init: sc.init.clone(),
                }));
            }
        }
    }

    // Emit double constants as static data
    for (label, value) in static_doubles {
        top_level.push(AsmTopLevel::StaticVar(AsmStaticVar {
            name: label,
            global: false,
            alignment: 16,
            init_values: vec![StaticInit::DoubleInit(value)],
        }));
    }

    AsmProgram { top_level }
}
