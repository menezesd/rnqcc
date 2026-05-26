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
fn double_const_label(static_doubles: &[(String, f64)]) -> String {
    format!("__double_const_{}", static_doubles.len())
}

/// Convert a TackyVal for doubles, emitting a data label for double constants
fn convert_double_val(val: &TackyVal, static_doubles: &mut Vec<(String, f64)>) -> AsmOperand {
    match val {
        TackyVal::DoubleConstant(d) => {
            let label = double_const_label(static_doubles);
            static_doubles.push((label.clone(), *d));
            AsmOperand::Data(label)
        }
        TackyVal::Constant(c) => AsmOperand::Imm(*c),
        TackyVal::Var(name) => AsmOperand::Pseudo(name.clone()),
    }
}

fn get_struct_def<'a>(
    name: &str,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &'a HashMap<String, StructDef>,
) -> Option<&'a StructDef> {
    var_struct_tags
        .get(name)
        .and_then(|tag| struct_defs.get(tag))
}

fn get_struct_classes(
    name: &str,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Option<Vec<ParamClass>> {
    if let Some(tag) = var_struct_tags.get(name) {
        if let Some(def) = struct_defs.get(tag) {
            return Some(def.classify_with(struct_defs));
        }
    }
    None
}

fn convert_instruction(
    instr: &TackyInstr,
    types: &HashMap<String, CType>,
    _arr_sizes: &HashMap<String, usize>,
    out: &mut Vec<AsmInstr>,
    static_doubles: &mut Vec<(String, f64)>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Result<(), String> {
    match instr {
        TackyInstr::Nop => { /* skip */ }
        TackyInstr::Unreachable => {
            out.push(AsmInstr::Unreachable);
        }
        TackyInstr::AtomicFence => {
            out.push(AsmInstr::AtomicFence);
        }
        TackyInstr::AtomicFetch {
            op,
            ptr,
            arg,
            return_old,
            dst,
        } => {
            let t = val_type(dst, types);
            if matches!(t, AsmType::Float | AsmType::Double) {
                return Err("x86-64 backend cannot atomic-fetch floating values".to_string());
            }
            let asm_op = match op {
                TackyBinaryOp::Add => AsmBinaryOp::Add,
                TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                TackyBinaryOp::BitwiseNand => AsmBinaryOp::Nand,
                TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                _ => return Err(format!("unsupported x86-64 atomic fetch op: {:?}", op)),
            };
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                t,
                convert_val(arg),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicRmw(
                t,
                asm_op,
                *return_old,
                convert_val(dst),
            ));
        }
        TackyInstr::AtomicExchange { ptr, value, dst } => {
            let t = val_type(dst, types);
            if matches!(t, AsmType::Float | AsmType::Double) {
                return Err("x86-64 backend cannot atomic-exchange floating values".to_string());
            }
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                t,
                convert_val(value),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicExchange(t, convert_val(dst)));
        }
        TackyInstr::AtomicCompareExchange {
            ptr,
            expected,
            desired,
            dst,
        } => {
            let desired_ty = val_type(desired, types);
            if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                return Err(
                    "x86-64 backend cannot atomic-compare-exchange floating values".to_string(),
                );
            }
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(expected),
                AsmOperand::Reg(Reg::R12),
            ));
            out.push(AsmInstr::Mov(
                desired_ty,
                convert_val(desired),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicCompareExchange(
                desired_ty,
                convert_val(dst),
            ));
        }
        TackyInstr::AtomicCompareSwap {
            ptr,
            expected,
            desired,
            return_old,
            dst,
        } => {
            let desired_ty = val_type(desired, types);
            if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                return Err(
                    "x86-64 backend cannot sync compare-and-swap floating values".to_string(),
                );
            }
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::Mov(
                desired_ty,
                convert_val(expected),
                AsmOperand::Reg(Reg::R12),
            ));
            out.push(AsmInstr::Mov(
                desired_ty,
                convert_val(desired),
                AsmOperand::Reg(Reg::R10),
            ));
            out.push(AsmInstr::AtomicCompareSwap(
                desired_ty,
                *return_old,
                convert_val(dst),
            ));
        }
        TackyInstr::CopyStruct { src_name, dst_name } => {
            // Emit bytewise copy for struct-to-struct assignment
            let struct_size = get_struct_def(dst_name, var_struct_tags, struct_defs)
                .map(|d| d.size)
                .unwrap_or(0);
            let mut off = 0i32;
            while (off as usize) + 8 <= struct_size {
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::PseudoMem(src_name.clone(), off),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::PseudoMem(dst_name.clone(), off),
                ));
                off += 8;
            }
            while (off as usize) + 4 <= struct_size {
                out.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::PseudoMem(src_name.clone(), off),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::PseudoMem(dst_name.clone(), off),
                ));
                off += 4;
            }
            while (off as usize) < struct_size {
                out.push(AsmInstr::Mov(
                    AsmType::Byte,
                    AsmOperand::PseudoMem(src_name.clone(), off),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Byte,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::PseudoMem(dst_name.clone(), off),
                ));
                off += 1;
            }
        }
        TackyInstr::Return(val) => {
            let t = val_type(val, types);
            // Check if returning a struct
            if let TackyVal::Var(ref name) = val {
                if types.get(name).copied() == Some(CType::Struct) {
                    if let Some(classes) = get_struct_classes(name, var_struct_tags, struct_defs) {
                        let mut int_ret_idx = 0;
                        let mut sse_ret_idx = 0;
                        let int_ret_regs = [Reg::AX, Reg::DX];
                        let sse_ret_regs = [XmmReg::XMM0, XmmReg::XMM1];
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i32;
                            match class {
                                ParamClass::Sse if sse_ret_idx < 2 => {
                                    out.push(AsmInstr::Mov(
                                        AsmType::Double,
                                        AsmOperand::PseudoMem(name.clone(), eb_offset),
                                        AsmOperand::Xmm(sse_ret_regs[sse_ret_idx]),
                                    ));
                                    sse_ret_idx += 1;
                                }
                                ParamClass::Integer if int_ret_idx < 2 => {
                                    out.push(AsmInstr::Mov(
                                        AsmType::Quadword,
                                        AsmOperand::PseudoMem(name.clone(), eb_offset),
                                        AsmOperand::Reg(int_ret_regs[int_ret_idx]),
                                    ));
                                    int_ret_idx += 1;
                                }
                                _ => {}
                            }
                        }
                    }
                    out.push(AsmInstr::Ret);
                    return Ok(());
                }
            }
            if matches!(t, AsmType::Float | AsmType::Double) {
                let src = convert_double_val(val, static_doubles);
                out.push(AsmInstr::Mov(
                    AsmType::Double,
                    src,
                    AsmOperand::Xmm(XmmReg::XMM0),
                ));
            } else {
                // Use Quadword for return to preserve all 64 bits
                let ret_type = if t == AsmType::Longword {
                    AsmType::Quadword
                } else {
                    t
                };
                out.push(AsmInstr::Mov(
                    ret_type,
                    convert_val(val),
                    AsmOperand::Reg(Reg::AX),
                ));
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
                    out.push(AsmInstr::Movsx(
                        src_t,
                        dst_t,
                        convert_val(src),
                        convert_val(dst),
                    ));
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
                    out.push(AsmInstr::MovZeroExtend(
                        src_t,
                        dst_t,
                        convert_val(src),
                        convert_val(dst),
                    ));
                }
            }
        }
        TackyInstr::Truncate { src, dst } => {
            let dst_t = val_type(dst, types);
            out.push(AsmInstr::Mov(dst_t, convert_val(src), convert_val(dst)));
        }
        TackyInstr::Unary {
            op: TackyUnaryOp::LogicalNot,
            src,
            dst,
        } => {
            let t = val_type(src, types);
            let dst_op = convert_val(dst);
            if matches!(t, AsmType::Float | AsmType::Double) {
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Xor,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Cmp(
                    AsmType::Double,
                    src_op,
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(src)));
            }
            out.push(AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                dst_op.clone(),
            ));
            out.push(AsmInstr::SetCC(CondCode::E, dst_op));
        }
        TackyInstr::Unary { op, src, dst } => {
            let t = val_type(dst, types);
            if matches!(t, AsmType::Float | AsmType::Double) && matches!(op, TackyUnaryOp::Negate) {
                // Double negation: XOR with sign bit mask (bit 63)
                // Emit a static constant with just the sign bit set
                let sign_mask_label = double_const_label(static_doubles);
                let sign_bit: u64 = if t == AsmType::Float {
                    (1u32 << 31) as u64
                } else {
                    1u64 << 63
                };
                static_doubles.push((sign_mask_label.clone(), f64::from_bits(sign_bit)));
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Mov(t, src_op, convert_val(dst)));
                out.push(AsmInstr::Binary(
                    t,
                    AsmBinaryOp::Xor,
                    AsmOperand::Data(sign_mask_label),
                    convert_val(dst),
                ));
            } else {
                let asm_op = match op {
                    TackyUnaryOp::Negate => AsmUnaryOp::Neg,
                    TackyUnaryOp::Complement => AsmUnaryOp::Not,
                    TackyUnaryOp::LogicalNot => {
                        return Err("logical-not should be lowered before x86-64 unary emission"
                            .to_string())
                    }
                };
                out.push(AsmInstr::Mov(t, convert_val(src), convert_val(dst)));
                out.push(AsmInstr::Unary(t, asm_op, convert_val(dst)));
            }
        }
        TackyInstr::Binary {
            op: TackyBinaryOp::Div,
            left,
            right,
            dst,
        } if matches!(val_type(dst, types), AsmType::Float | AsmType::Double) => {
            let t = val_type(dst, types);
            let left_op = convert_double_val(left, static_doubles);
            let right_op = convert_double_val(right, static_doubles);
            let dst_op = convert_val(dst);
            out.push(AsmInstr::Mov(t, left_op, dst_op.clone()));
            out.push(AsmInstr::Binary(
                t,
                AsmBinaryOp::DivDouble,
                right_op,
                dst_op,
            ));
        }
        TackyInstr::Binary {
            op: op @ (TackyBinaryOp::Div | TackyBinaryOp::Mod),
            left,
            right,
            dst,
        } => {
            let t = val_type(dst, types);
            let dst_ctype = types
                .get(match dst {
                    TackyVal::Var(n) => n.as_str(),
                    _ => "",
                })
                .copied()
                .unwrap_or(CType::Int);
            let is_unsigned = !dst_ctype.is_signed();
            out.push(AsmInstr::Mov(
                t,
                convert_val(left),
                AsmOperand::Reg(Reg::AX),
            ));
            if is_unsigned {
                // Zero EDX/RDX for unsigned division
                out.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(0),
                    AsmOperand::Reg(Reg::DX),
                ));
                out.push(AsmInstr::Div(t, convert_val(right)));
            } else {
                out.push(AsmInstr::Cdq(t));
                out.push(AsmInstr::Idiv(t, convert_val(right)));
            }
            let result_reg = if matches!(op, TackyBinaryOp::Mod) {
                Reg::DX
            } else {
                Reg::AX
            };
            out.push(AsmInstr::Mov(
                t,
                AsmOperand::Reg(result_reg),
                convert_val(dst),
            ));
        }
        TackyInstr::Binary {
            op: op @ (TackyBinaryOp::ShiftLeft | TackyBinaryOp::ShiftRight),
            left,
            right,
            dst,
        } => {
            let t = val_type(dst, types);
            let dst_ctype = match dst {
                TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int),
                _ => CType::Int,
            };
            let asm_op = match op {
                TackyBinaryOp::ShiftLeft => AsmBinaryOp::Sal,
                TackyBinaryOp::ShiftRight => {
                    if dst_ctype.is_signed() {
                        AsmBinaryOp::Sar
                    } else {
                        AsmBinaryOp::Shr
                    }
                }
                _ => return Err(format!("unsupported x86-64 shift op: {:?}", op)),
            };
            out.push(AsmInstr::Mov(t, convert_val(left), convert_val(dst)));
            out.push(AsmInstr::Mov(
                AsmType::Longword,
                convert_val(right),
                AsmOperand::Reg(Reg::CX),
            ));
            out.push(AsmInstr::Binary(
                t,
                asm_op,
                AsmOperand::Reg(Reg::CX),
                convert_val(dst),
            ));
        }
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            convert_binary(op, left, right, dst, types, out, static_doubles)?;
        }
        TackyInstr::Copy { src, dst } => {
            let t = val_type(dst, types);
            let src_op = if matches!(t, AsmType::Float | AsmType::Double)
                || matches!(src, TackyVal::DoubleConstant(_))
            {
                convert_double_val(src, static_doubles)
            } else {
                convert_val(src)
            };
            out.push(AsmInstr::Mov(t, src_op, convert_val(dst)));
        }
        TackyInstr::IntToDouble { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                // narrow integer→double: extend to int first, then cvtsi2sd
                out.push(AsmInstr::Movsx(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvtsi2sd(
                    src_t,
                    convert_val(src),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::IntToFloat { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::Movsx(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvtsi2ss(
                    src_t,
                    convert_val(src),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::DoubleToInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                // double→narrow integer: cvttsd2si to int, then truncate
                out.push(AsmInstr::Cvttsd2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttsd2si(
                    dst_t,
                    convert_val(src),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::FloatToInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::Cvttss2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttss2si(
                    dst_t,
                    convert_val(src),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::UIntToDouble { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                // narrow unsigned integer→double: zero-extend to int, then cvtsi2sd
                out.push(AsmInstr::MovZeroExtend(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else if src_t == AsmType::Longword {
                // Unsigned int (32-bit): zero-extend to R10 (64-bit), then cvtsi2sdq
                out.push(AsmInstr::MovZeroExtend(
                    AsmType::Longword,
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                // Unsigned long (64-bit): need to handle values > LONG_MAX
                // Algorithm: test if negative (as signed); if not, cvtsi2sdq directly
                // If so: shift right 1, save LSB, OR LSB into shifted value,
                // cvtsi2sdq, then addsd result to itself
                let ok_label = double_const_label(static_doubles);
                let end_label = double_const_label(static_doubles);
                out.push(AsmInstr::Cmp(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    convert_val(src),
                ));
                out.push(AsmInstr::JmpCC(CondCode::GE, ok_label.clone()));
                // Negative as signed = >= LONG_MAX as unsigned
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    AsmOperand::Reg(Reg::R11),
                ));
                // R11 = src & 1 (save LSB for rounding)
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::And,
                    AsmOperand::Imm(1),
                    AsmOperand::Reg(Reg::R11),
                ));
                // R10 = src >> 1
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Shr,
                    AsmOperand::Imm(1),
                    AsmOperand::Reg(Reg::R10),
                ));
                // R10 = R10 | R11 (round-to-odd)
                out.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Or,
                    AsmOperand::Reg(Reg::R11),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
                // Double the result: dst = dst + dst
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Add,
                    convert_val(dst),
                    convert_val(dst),
                ));
                out.push(AsmInstr::Jmp(end_label.clone()));
                out.push(AsmInstr::Label(ok_label));
                out.push(AsmInstr::Cvtsi2sd(
                    AsmType::Quadword,
                    convert_val(src),
                    convert_val(dst),
                ));
                out.push(AsmInstr::Label(end_label));
            }
        }
        TackyInstr::UIntToFloat { src, dst } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::MovZeroExtend(
                    src_t,
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Longword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else if src_t == AsmType::Longword {
                out.push(AsmInstr::MovZeroExtend(
                    AsmType::Longword,
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvtsi2ss(
                    AsmType::Quadword,
                    convert_val(src),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::GetAddress { src, dst } => {
            out.push(AsmInstr::Lea(convert_val(src), convert_val(dst)));
        }
        TackyInstr::Load { src_ptr, dst } => {
            let dst_t = val_type(dst, types);
            // Load pointer value into R11, then load indirectly
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(src_ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            out.push(AsmInstr::LoadIndirect(dst_t, Reg::R11, convert_val(dst)));
        }
        TackyInstr::Store { src, dst_ptr } => {
            let src_t = val_type(src, types);
            // Load pointer value into R11, then store indirectly
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                convert_val(dst_ptr),
                AsmOperand::Reg(Reg::R11),
            ));
            let src_op = if matches!(src_t, AsmType::Float | AsmType::Double)
                || matches!(src, TackyVal::DoubleConstant(_))
            {
                convert_double_val(src, static_doubles)
            } else {
                convert_val(src)
            };
            out.push(AsmInstr::StoreIndirect(src_t, src_op, Reg::R11));
        }
        TackyInstr::CopyToOffset {
            src,
            dst_name,
            offset,
        } => {
            let src_t = val_type(src, types);
            if matches!(src_t, AsmType::Float | AsmType::Double) {
                let src_op = convert_double_val(src, static_doubles);
                out.push(AsmInstr::Mov(
                    src_t,
                    src_op,
                    AsmOperand::PseudoMem(dst_name.clone(), *offset as i32),
                ));
            } else {
                out.push(AsmInstr::Mov(
                    src_t,
                    convert_val(src),
                    AsmOperand::PseudoMem(dst_name.clone(), *offset as i32),
                ));
            }
        }
        TackyInstr::CopyFromOffset {
            src_name,
            offset,
            dst,
        } => {
            let dst_t = val_type(dst, types);
            out.push(AsmInstr::Mov(
                dst_t,
                AsmOperand::PseudoMem(src_name.clone(), *offset as i32),
                convert_val(dst),
            ));
        }
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => {
            // ptr + index * scale → dst
            // If index is a constant, compute offset at compile time
            if let TackyVal::Constant(idx) = index {
                let offset = *idx * *scale;
                if offset == 0 {
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        convert_val(ptr),
                        convert_val(dst),
                    ));
                } else {
                    // Use lea to avoid add instruction
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        convert_val(ptr),
                        AsmOperand::Reg(Reg::AX),
                    ));
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Imm(offset),
                        AsmOperand::Reg(Reg::DX),
                    ));
                    out.push(AsmInstr::Lea(
                        AsmOperand::Indexed(Reg::AX, Reg::DX, 1),
                        convert_val(dst),
                    ));
                }
            } else {
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(ptr),
                    AsmOperand::Reg(Reg::AX),
                ));
                out.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    convert_val(index),
                    AsmOperand::Reg(Reg::DX),
                ));
                if *scale == 1 || *scale == 2 || *scale == 4 || *scale == 8 {
                    out.push(AsmInstr::Lea(
                        AsmOperand::Indexed(Reg::AX, Reg::DX, *scale as i32),
                        convert_val(dst),
                    ));
                } else {
                    out.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        AsmBinaryOp::Mul,
                        AsmOperand::Imm(*scale),
                        AsmOperand::Reg(Reg::DX),
                    ));
                    out.push(AsmInstr::Lea(
                        AsmOperand::Indexed(Reg::AX, Reg::DX, 1),
                        convert_val(dst),
                    ));
                }
            }
        }
        TackyInstr::DoubleToUInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                // double→narrow unsigned integer: cvttsd2si to int, truncate
                out.push(AsmInstr::Cvttsd2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttsd2si(
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                if dst_t == AsmType::Longword {
                    out.push(AsmInstr::Mov(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        convert_val(dst),
                    ));
                } else {
                    out.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::R10),
                        convert_val(dst),
                    ));
                }
            }
        }
        TackyInstr::FloatToUInt { src, dst } => {
            let dst_t = val_type(dst, types);
            if matches!(dst_t, AsmType::Byte | AsmType::Word) {
                out.push(AsmInstr::Cvttss2si(
                    AsmType::Longword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            } else {
                out.push(AsmInstr::Cvttss2si(
                    AsmType::Quadword,
                    convert_val(src),
                    AsmOperand::Reg(Reg::R10),
                ));
                out.push(AsmInstr::Mov(
                    dst_t,
                    AsmOperand::Reg(Reg::R10),
                    convert_val(dst),
                ));
            }
        }
        TackyInstr::FloatToDouble { src, dst } => {
            out.push(AsmInstr::Cvtss2sd(convert_val(src), convert_val(dst)));
        }
        TackyInstr::DoubleToFloat { src, dst } => {
            out.push(AsmInstr::Cvtsd2ss(
                convert_double_val(src, static_doubles),
                convert_val(dst),
            ));
        }
        TackyInstr::Jump(label) => {
            out.push(AsmInstr::Jmp(label.clone()));
        }
        TackyInstr::JumpIfZero(val, label) => {
            let t = val_type(val, types);
            if matches!(t, AsmType::Float | AsmType::Double) {
                // xorpd zeroes an xmm; comisd compares
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Xor,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                out.push(AsmInstr::Cmp(
                    AsmType::Double,
                    convert_val(val),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                out.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(val)));
                out.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            }
        }
        TackyInstr::JumpIfNotZero(val, label) => {
            let t = val_type(val, types);
            if t == AsmType::Double {
                out.push(AsmInstr::Binary(
                    AsmType::Double,
                    AsmBinaryOp::Xor,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                out.push(AsmInstr::Cmp(
                    AsmType::Double,
                    convert_val(val),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                out.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            } else {
                out.push(AsmInstr::Cmp(t, AsmOperand::Imm(0), convert_val(val)));
                out.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            }
        }
        TackyInstr::Label(label) => {
            out.push(AsmInstr::Label(label.clone()));
        }
        TackyInstr::FunCall {
            name,
            args,
            dst,
            stack_arg_indices,
            struct_arg_groups,
            variadic,
            fixed_flat_arg_count: _,
            indirect,
        } => {
            let mut ctx = FuncallContext {
                types,
                out,
                static_doubles,
                var_struct_tags,
                struct_defs,
                variadic: *variadic,
            };
            convert_funcall(
                name,
                args,
                dst,
                stack_arg_indices,
                struct_arg_groups,
                *indirect,
                &mut ctx,
            );
        }
    };
    Ok(())
}

struct FuncallContext<'a> {
    types: &'a HashMap<String, CType>,
    out: &'a mut Vec<AsmInstr>,
    static_doubles: &'a mut Vec<(String, f64)>,
    var_struct_tags: &'a HashMap<String, String>,
    struct_defs: &'a HashMap<String, StructDef>,
    variadic: bool,
}

fn convert_funcall(
    name: &str,
    args: &[TackyVal],
    dst: &TackyVal,
    stack_arg_indices: &std::collections::HashSet<usize>,
    struct_arg_groups: &[(usize, usize, Vec<bool>)],
    indirect: bool,
    ctx: &mut FuncallContext<'_>,
) {
    let types = ctx.types;
    let out = &mut *ctx.out;
    let static_doubles = &mut *ctx.static_doubles;
    let var_struct_tags = ctx.var_struct_tags;
    let struct_defs = ctx.struct_defs;

    {
        // Pre-compute which args must go on stack due to struct group overflow
        let mut force_stack_args: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        {
            let mut sim_int = 0usize;
            let mut sim_xmm = 0usize;
            for (arg_idx, arg) in args.iter().enumerate() {
                if stack_arg_indices.contains(&arg_idx) {
                    force_stack_args.insert(arg_idx);
                    continue;
                }
                let group = struct_arg_groups
                    .iter()
                    .find(|(start, count, _)| arg_idx >= *start && arg_idx < *start + *count);
                if let Some((start, count, is_sse_vec)) = group {
                    if arg_idx == *start {
                        let int_needed: usize =
                            is_sse_vec.iter().filter(|&&is_sse| !is_sse).count();
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
                if matches!(t, AsmType::Float | AsmType::Double) {
                    if sim_xmm < 8 {
                        sim_xmm += 1;
                    }
                } else {
                    if sim_int < 6 {
                        sim_int += 1;
                    }
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
            if matches!(t, AsmType::Float | AsmType::Double) {
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
            // For constants going into registers: use Quadword when value is negative
            // (movl zero-extends, which changes the meaning of negative values)
            let t = match arg {
                TackyVal::Constant(v) if *v < 0 && t == AsmType::Longword => AsmType::Quadword,
                _ => t,
            };
            out.push(AsmInstr::Mov(
                t,
                convert_val(arg),
                AsmOperand::Reg(ARG_REGISTERS[*i]),
            ));
        }
        // Move xmm register args
        for (i, arg) in &xmm_reg_args {
            let t = val_type(arg, types);
            let src = convert_double_val(arg, static_doubles);
            out.push(AsmInstr::Mov(
                t,
                src,
                AsmOperand::Xmm(XMM_ARG_REGISTERS[*i]),
            ));
        }

        if indirect {
            out.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Pseudo(name.to_string()),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        if ctx.variadic {
            out.push(AsmInstr::X86SetVarargsXmmCount(xmm_reg_args.len()));
        }
        out.push(AsmInstr::Call(
            name.to_string(),
            int_reg_args.len(),
            xmm_reg_args.len(),
            indirect,
        ));
        let bytes_to_dealloc = (stack_count * 8 + padding) as i32;
        if bytes_to_dealloc > 0 {
            out.push(AsmInstr::DeallocateStack(bytes_to_dealloc));
        }
        let ret_t = val_type(dst, types);
        // Check if return value is a struct
        if let TackyVal::Var(ref dst_name) = dst {
            if types.get(dst_name).copied() == Some(CType::Struct) {
                if let Some(classes) = get_struct_classes(dst_name, var_struct_tags, struct_defs) {
                    let mut int_ret_idx = 0;
                    let mut sse_ret_idx = 0;
                    let int_ret_regs = [Reg::AX, Reg::DX];
                    let sse_ret_regs = [XmmReg::XMM0, XmmReg::XMM1];
                    for (eb_idx, class) in classes.iter().enumerate() {
                        let eb_offset = (eb_idx * 8) as i32;
                        match class {
                            ParamClass::Sse if sse_ret_idx < 2 => {
                                out.push(AsmInstr::Mov(
                                    AsmType::Double,
                                    AsmOperand::Xmm(sse_ret_regs[sse_ret_idx]),
                                    AsmOperand::PseudoMem(dst_name.clone(), eb_offset),
                                ));
                                sse_ret_idx += 1;
                            }
                            ParamClass::Integer if int_ret_idx < 2 => {
                                out.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    AsmOperand::Reg(int_ret_regs[int_ret_idx]),
                                    AsmOperand::PseudoMem(dst_name.clone(), eb_offset),
                                ));
                                int_ret_idx += 1;
                            }
                            _ => {}
                        }
                    }
                    return;
                }
            }
        }
        if matches!(ret_t, AsmType::Float | AsmType::Double) {
            out.push(AsmInstr::Mov(
                ret_t,
                AsmOperand::Xmm(XmmReg::XMM0),
                convert_val(dst),
            ));
        } else {
            out.push(AsmInstr::Mov(
                ret_t,
                AsmOperand::Reg(Reg::AX),
                convert_val(dst),
            ));
        }
    }
}

fn convert_binary(
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    types: &HashMap<String, CType>,
    out: &mut Vec<AsmInstr>,
    static_doubles: &mut Vec<(String, f64)>,
) -> Result<(), String> {
    let left_ctype = match left {
        TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int),
        TackyVal::Constant(_) => CType::Int,
        TackyVal::DoubleConstant(_) => CType::Double,
    };
    let right_ctype = match right {
        TackyVal::Var(n) => types.get(n).copied().unwrap_or(CType::Int),
        TackyVal::Constant(_) => CType::Int,
        TackyVal::DoubleConstant(_) => CType::Double,
    };
    let is_unsigned = !left_ctype.is_signed()
        || !right_ctype.is_signed()
        || left_ctype.is_floating()
        || right_ctype.is_floating();

    if let Some(cc) = is_comparison(op, is_unsigned) {
        let var_type = |v: &TackyVal| -> Option<AsmType> {
            match v {
                TackyVal::Var(n) => Some(match types.get(n).copied().unwrap_or(CType::Int) {
                    CType::Long | CType::ULong | CType::Pointer => AsmType::Quadword,
                    CType::Float => AsmType::Float,
                    CType::Double => AsmType::Double,
                    _ => AsmType::Longword,
                }),
                TackyVal::DoubleConstant(_) => Some(AsmType::Double),
                _ => None,
            }
        };
        let cmp_type = match (var_type(left), var_type(right)) {
            (Some(AsmType::Double), _) | (_, Some(AsmType::Double)) => AsmType::Double,
            (Some(AsmType::Float), _) | (_, Some(AsmType::Float)) => AsmType::Float,
            (Some(AsmType::Quadword), _) | (_, Some(AsmType::Quadword)) => AsmType::Quadword,
            (Some(t), _) => t,
            (_, Some(t)) => t,
            _ => {
                let lt = val_type(left, types);
                let rt = val_type(right, types);
                if lt == AsmType::Quadword || rt == AsmType::Quadword {
                    AsmType::Quadword
                } else {
                    AsmType::Longword
                }
            }
        };
        if matches!(cmp_type, AsmType::Float | AsmType::Double) {
            let l = convert_double_val(left, static_doubles);
            let r = convert_double_val(right, static_doubles);
            out.push(AsmInstr::Cmp(cmp_type, r, l));
        } else {
            out.push(AsmInstr::Cmp(
                cmp_type,
                convert_val(right),
                convert_val(left),
            ));
        }
        out.push(AsmInstr::Mov(
            AsmType::Longword,
            AsmOperand::Imm(0),
            convert_val(dst),
        ));
        out.push(AsmInstr::SetCC(cc, convert_val(dst)));
    } else {
        let t = val_type(dst, types);
        if matches!(t, AsmType::Float | AsmType::Double) {
            let asm_op = match op {
                TackyBinaryOp::Add => AsmBinaryOp::Add,
                TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                TackyBinaryOp::Mul => AsmBinaryOp::Mul,
                _ => return Err(format!("Unsupported floating binary op: {:?}", op)),
            };
            let l = convert_double_val(left, static_doubles);
            let r = convert_double_val(right, static_doubles);
            out.push(AsmInstr::Mov(t, l, convert_val(dst)));
            out.push(AsmInstr::Binary(t, asm_op, r, convert_val(dst)));
        } else {
            let asm_op = match op {
                TackyBinaryOp::Add => AsmBinaryOp::Add,
                TackyBinaryOp::Sub => AsmBinaryOp::Sub,
                TackyBinaryOp::Mul => AsmBinaryOp::Mul,
                TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                _ => return Err(format!("unsupported x86-64 binary op: {:?}", op)),
            };
            out.push(AsmInstr::Mov(t, convert_val(left), convert_val(dst)));
            out.push(AsmInstr::Binary(
                t,
                asm_op,
                convert_val(right),
                convert_val(dst),
            ));
        }
    }
    Ok(())
}

const XMM_ARG_REGISTERS: [XmmReg; 8] = [
    XmmReg::XMM0,
    XmmReg::XMM1,
    XmmReg::XMM2,
    XmmReg::XMM3,
    XmmReg::XMM4,
    XmmReg::XMM5,
    XmmReg::XMM6,
    XmmReg::XMM7,
];

fn convert_function(
    func: &TackyFunction,
    types: &HashMap<String, CType>,
    arr_sizes: &HashMap<String, usize>,
    static_doubles: &mut Vec<(String, f64)>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Result<AsmFunction, String> {
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
            let group = func
                .struct_param_groups
                .iter()
                .find(|(start, count, _)| i >= *start && i < *start + *count);
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
            if matches!(t, AsmType::Float | AsmType::Double) {
                if sim_xmm_idx < 8 {
                    sim_xmm_idx += 1;
                }
                // else overflow
            } else {
                if sim_int_idx < 6 {
                    sim_int_idx += 1;
                }
            }
        }
    }

    for (i, param) in func.params.iter().enumerate() {
        if force_stack.contains(&i) || func.stack_params.contains(param) {
            let offset = 16 + (stack_arg_idx * 8) as i32;
            let t: AsmType = types.get(param).copied().unwrap_or(CType::Long).into();
            instructions.push(AsmInstr::Mov(
                t,
                AsmOperand::Stack(offset),
                AsmOperand::Pseudo(param.clone()),
            ));
            stack_arg_idx += 1;
            continue;
        }
        let t: AsmType = types.get(param).copied().unwrap_or(CType::Int).into();
        if matches!(t, AsmType::Float | AsmType::Double) {
            if xmm_reg_idx < 8 {
                instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Xmm(XMM_ARG_REGISTERS[xmm_reg_idx]),
                    AsmOperand::Pseudo(param.clone()),
                ));
                xmm_reg_idx += 1;
            } else {
                let offset = 16 + (stack_arg_idx * 8) as i32;
                instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Stack(offset),
                    AsmOperand::Pseudo(param.clone()),
                ));
                stack_arg_idx += 1;
            }
        } else {
            if int_reg_idx < 6 {
                instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Reg(ARG_REGISTERS[int_reg_idx]),
                    AsmOperand::Pseudo(param.clone()),
                ));
                int_reg_idx += 1;
            } else {
                let offset = 16 + (stack_arg_idx * 8) as i32;
                instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Stack(offset),
                    AsmOperand::Pseudo(param.clone()),
                ));
                stack_arg_idx += 1;
            }
        }
    }

    for instr in &func.body {
        convert_instruction(
            instr,
            types,
            arr_sizes,
            &mut instructions,
            static_doubles,
            var_struct_tags,
            struct_defs,
        )?;
    }
    Ok(AsmFunction {
        name: func.name.clone(),
        global: func.global,
        instructions,
    })
}

// ============================================================
// Phase 2: Replace pseudo-registers with stack slots
// ============================================================

fn replace_pseudos(
    func: &mut AsmFunction,
    static_vars: &std::collections::HashSet<String>,
    thread_local_vars: &std::collections::HashSet<String>,
    types: &HashMap<String, CType>,
    arr_sizes: &HashMap<String, usize>,
    alignments: &HashMap<String, usize>,
) -> i32 {
    let mut pseudo_map: HashMap<String, i32> = HashMap::new();
    let mut stack_offset: i32 = 0;
    struct ReplaceCtx<'a> {
        statics: &'a std::collections::HashSet<String>,
        tls_vars: &'a std::collections::HashSet<String>,
        types: &'a HashMap<String, CType>,
        arr_sizes: &'a HashMap<String, usize>,
        alignments: &'a HashMap<String, usize>,
    }
    let ctx = ReplaceCtx {
        statics: static_vars,
        tls_vars: thread_local_vars,
        types,
        arr_sizes,
        alignments,
    };

    fn replace_operand(
        op: &mut AsmOperand,
        map: &mut HashMap<String, i32>,
        offset: &mut i32,
        ctx: &ReplaceCtx,
    ) {
        match op {
            AsmOperand::Pseudo(name) => {
                let name = name.clone();
                if ctx.tls_vars.contains(&name) {
                    *op = AsmOperand::TlsData(name, 0);
                } else if ctx.statics.contains(&name) {
                    *op = AsmOperand::Data(name);
                } else {
                    let off = if let Some(&o) = map.get(&name) {
                        o
                    } else {
                        let size = if let Some(&arr_size) = ctx.arr_sizes.get(&name) {
                            arr_size as i32
                        } else {
                            let ct = ctx.types.get(&name).copied().unwrap_or(CType::Int);
                            // Void function results get stored as Longword (movl %eax, ...)
                            // so ensure at least 4 bytes
                            if ct == CType::Void {
                                4
                            } else {
                                std::cmp::max(ct.size(), 1)
                            }
                        };
                        let align = if let Some(&decl_align) = ctx.alignments.get(&name) {
                            decl_align
                        } else if let Some(&arr_size) = ctx.arr_sizes.get(&name) {
                            if arr_size >= 16 {
                                16
                            } else {
                                std::cmp::max(size.min(16) as usize, 1)
                            }
                        } else {
                            std::cmp::max(size.min(16) as usize, 1)
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
                if ctx.tls_vars.contains(&name) {
                    *op = AsmOperand::TlsData(name, mem_off);
                } else if ctx.statics.contains(&name) {
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
                        let size = if let Some(&arr_size) = ctx.arr_sizes.get(&name) {
                            arr_size as i32
                        } else {
                            ctx.types.get(&name).copied().unwrap_or(CType::Int).size()
                        };
                        let align = if let Some(&decl_align) = ctx.alignments.get(&name) {
                            decl_align
                        } else if size >= 16 {
                            16
                        } else {
                            std::cmp::max(size.min(16) as usize, 1)
                        };
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
        replace_operand(op, map, off, &ctx);
    };
    let _ = r; // suppress unused — we use the closure pattern below

    for instr in &mut func.instructions {
        match instr {
            AsmInstr::Mov(_, src, dst) | AsmInstr::Cmp(_, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, &ctx);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, &ctx);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Binary(_, _, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, &ctx);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Unary(_, _, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Idiv(_, op) | AsmInstr::Div(_, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::SetCC(_, op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Push(op) => {
                replace_operand(op, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvtsi2ss(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Cvttss2si(_, src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, &ctx);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Cvtss2sd(src, dst) | AsmInstr::Cvtsd2ss(src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, &ctx);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::Lea(src, dst) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, &ctx);
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::LoadIndirect(_, _, dst) => {
                replace_operand(dst, &mut pseudo_map, &mut stack_offset, &ctx);
            }
            AsmInstr::StoreIndirect(_, src, _) => {
                replace_operand(src, &mut pseudo_map, &mut stack_offset, &ctx);
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
    matches!(
        op,
        AsmOperand::Stack(_) | AsmOperand::Data(_) | AsmOperand::TlsData(_, _)
    )
}

fn fixup_instructions(func: &mut AsmFunction, stack_size: i32, callee_saved: &[Reg]) {
    let num_cs = callee_saved.len() as i32;
    let total_aligned = (stack_size + 8 * num_cs + 15) & !15;
    let adjusted_stack = total_aligned - 8 * num_cs;
    let old_instructions = std::mem::take(&mut func.instructions);
    let mut new_instructions = Vec::new();

    // Prologue placeholder
    new_instructions.push(AsmInstr::Push(AsmOperand::Reg(Reg::AX)));
    new_instructions.push(AsmInstr::AllocateStack(adjusted_stack));
    // Push callee-saved registers
    for reg in callee_saved {
        new_instructions.push(AsmInstr::Push(AsmOperand::Reg(*reg)));
    }

    for instr in old_instructions {
        match instr {
            // mov mem, mem
            AsmInstr::Mov(t @ (AsmType::Float | AsmType::Double), ref src, ref dst)
                if is_memory(src) && is_memory(dst) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            AsmInstr::Mov(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // movsx with memory dst (movslq can't write to memory)
            AsmInstr::Movsx(st, dt, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Movsx(
                    st,
                    dt,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(dt, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // movzx with memory dst
            AsmInstr::MovZeroExtend(st, dt, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::MovZeroExtend(
                    st,
                    dt,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(dt, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // idiv imm / div imm
            AsmInstr::Idiv(t, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Idiv(t, AsmOperand::Reg(Reg::R10)));
            }
            AsmInstr::Div(t, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Div(t, AsmOperand::Reg(Reg::R10)));
            }
            // mul with memory dst (integer only)
            AsmInstr::Binary(t, AsmBinaryOp::Mul, ref src, ref dst)
                if is_memory(dst) && !matches!(t, AsmType::Float | AsmType::Double) =>
            {
                if is_memory(src) {
                    new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                    new_instructions.push(AsmInstr::Mov(t, dst.clone(), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Binary(
                        t,
                        AsmBinaryOp::Mul,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Reg(Reg::R11),
                    ));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R11), dst.clone()));
                } else {
                    new_instructions.push(AsmInstr::Mov(t, dst.clone(), AsmOperand::Reg(Reg::R11)));
                    new_instructions.push(AsmInstr::Binary(
                        t,
                        AsmBinaryOp::Mul,
                        src.clone(),
                        AsmOperand::Reg(Reg::R11),
                    ));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R11), dst.clone()));
                }
            }
            // double binary mem, mem
            // double binary: dst must be XMM register
            AsmInstr::Binary(t @ (AsmType::Float | AsmType::Double), ref op, ref src, ref dst)
                if is_memory(dst) || is_memory(src) =>
            {
                // Load dst into XMM14 (for operations like addsd, dst is both src and dest)
                let dst_xmm = AsmOperand::Xmm(XmmReg::XMM14);
                new_instructions.push(AsmInstr::Mov(t, dst.clone(), dst_xmm.clone()));
                let src_op = if is_memory(src) && !matches!(src, AsmOperand::Xmm(_)) {
                    // src can be memory for SSE ops like addsd mem, xmm
                    src.clone()
                } else {
                    src.clone()
                };
                new_instructions.push(AsmInstr::Binary(t, op.clone(), src_op, dst_xmm.clone()));
                new_instructions.push(AsmInstr::Mov(t, dst_xmm, dst.clone()));
            }
            // binary mem, mem (integer)
            AsmInstr::Binary(t, ref op, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Binary(
                    t,
                    op.clone(),
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // double cmp: comisd src, dst — dst MUST be xmm register
            AsmInstr::Cmp(t @ (AsmType::Float | AsmType::Double), ref src, ref dst)
                if !matches!(dst, AsmOperand::Xmm(_)) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    dst.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Cmp(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
            }
            AsmInstr::Cvtss2sd(ref src, ref dst) if !matches!(dst, AsmOperand::Xmm(_)) => {
                new_instructions.push(AsmInstr::Cvtss2sd(
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            AsmInstr::Cvtsd2ss(ref src, ref dst) if !matches!(dst, AsmOperand::Xmm(_)) => {
                new_instructions.push(AsmInstr::Cvtsd2ss(
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Float,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            // cmp mem, mem
            AsmInstr::Cmp(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Cmp(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // cmp src, imm (dst can't be immediate)
            AsmInstr::Cmp(t, ref src, AsmOperand::Imm(val)) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R11),
                ));
                new_instructions.push(AsmInstr::Cmp(t, src.clone(), AsmOperand::Reg(Reg::R11)));
            }
            // cmp with large immediate src that doesn't fit in 32 bits
            AsmInstr::Cmp(AsmType::Quadword, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Cmp(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // binary with large immediate for quadword
            AsmInstr::Binary(AsmType::Quadword, ref op, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Binary(
                    AsmType::Quadword,
                    op.clone(),
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // mov with large immediate for quadword
            AsmInstr::Mov(AsmType::Quadword, AsmOperand::Imm(val), ref dst)
                if (val > i32::MAX as i64 || val < i32::MIN as i64) && is_memory(dst) =>
            {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // cvtsi2sd with immediate src
            AsmInstr::Cvtsi2sd(t, AsmOperand::Imm(val), ref dst) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                if is_memory(dst) {
                    new_instructions.push(AsmInstr::Cvtsi2sd(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::Mov(
                        AsmType::Double,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        dst.clone(),
                    ));
                } else {
                    new_instructions.push(AsmInstr::Cvtsi2sd(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        dst.clone(),
                    ));
                }
            }
            // cvtsi2ss with immediate src
            AsmInstr::Cvtsi2ss(t, AsmOperand::Imm(val), ref dst) => {
                new_instructions.push(AsmInstr::Mov(
                    t,
                    AsmOperand::Imm(val),
                    AsmOperand::Reg(Reg::R10),
                ));
                if is_memory(dst) {
                    new_instructions.push(AsmInstr::Cvtsi2ss(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::Mov(
                        AsmType::Float,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        dst.clone(),
                    ));
                } else {
                    new_instructions.push(AsmInstr::Cvtsi2ss(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        dst.clone(),
                    ));
                }
            }
            // cvtsi2sd with memory dst
            AsmInstr::Cvtsi2sd(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvtsi2sd(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            // cvtsi2ss with memory dst
            AsmInstr::Cvtsi2ss(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvtsi2ss(
                    t,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Float,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    dst.clone(),
                ));
            }
            // cvttsd2si with memory src AND memory dst
            AsmInstr::Cvttsd2si(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Cvttsd2si(
                    t,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // cvttss2si with memory src AND memory dst
            AsmInstr::Cvttss2si(t, ref src, ref dst) if is_memory(src) && is_memory(dst) => {
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Float,
                    src.clone(),
                    AsmOperand::Xmm(XmmReg::XMM14),
                ));
                new_instructions.push(AsmInstr::Cvttss2si(
                    t,
                    AsmOperand::Xmm(XmmReg::XMM14),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            AsmInstr::Cvttsd2si(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvttsd2si(
                    t,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            AsmInstr::Cvttss2si(t, ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Cvttss2si(
                    t,
                    src.clone(),
                    AsmOperand::Reg(Reg::R10),
                ));
                new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
            }
            // lea with memory dst
            AsmInstr::Lea(ref src, ref dst) if is_memory(dst) => {
                new_instructions.push(AsmInstr::Lea(src.clone(), AsmOperand::Reg(Reg::R10)));
                new_instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(Reg::R10),
                    dst.clone(),
                ));
            }
            // LoadIndirect with memory dst
            AsmInstr::LoadIndirect(t, ref reg, ref dst) if is_memory(dst) => {
                if t == AsmType::Double {
                    new_instructions.push(AsmInstr::LoadIndirect(
                        t,
                        *reg,
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::Mov(
                        AsmType::Double,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        dst.clone(),
                    ));
                } else {
                    new_instructions.push(AsmInstr::LoadIndirect(
                        t,
                        *reg,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    new_instructions.push(AsmInstr::Mov(t, AsmOperand::Reg(Reg::R10), dst.clone()));
                }
            }
            // StoreIndirect with memory src
            AsmInstr::StoreIndirect(t, ref src, ref reg) if is_memory(src) => {
                if t == AsmType::Double {
                    new_instructions.push(AsmInstr::Mov(
                        AsmType::Double,
                        src.clone(),
                        AsmOperand::Xmm(XmmReg::XMM14),
                    ));
                    new_instructions.push(AsmInstr::StoreIndirect(
                        t,
                        AsmOperand::Xmm(XmmReg::XMM14),
                        *reg,
                    ));
                } else {
                    new_instructions.push(AsmInstr::Mov(t, src.clone(), AsmOperand::Reg(Reg::R10)));
                    new_instructions.push(AsmInstr::StoreIndirect(
                        t,
                        AsmOperand::Reg(Reg::R10),
                        *reg,
                    ));
                }
            }
            // Ret → pop callee-saved registers (reverse order), then epilogue
            instr @ (AsmInstr::Ret | AsmInstr::Unreachable) => {
                for reg in callee_saved.iter().rev() {
                    new_instructions.push(AsmInstr::Pop(*reg));
                }
                new_instructions.push(instr);
            }
            other => {
                new_instructions.push(other);
            }
        }
    }

    func.instructions = new_instructions;
}

fn assert_no_pseudo_operand(op: &AsmOperand, instr: &AsmInstr) -> Result<(), String> {
    if matches!(op, AsmOperand::Pseudo(_) | AsmOperand::PseudoMem(_, _)) {
        return Err(format!(
            "unlowered pseudo operand in final assembly: {:?} in {:?}",
            op, instr
        ));
    }
    Ok(())
}

fn verify_final_function(func: &AsmFunction) -> Result<(), String> {
    for instr in &func.instructions {
        match instr {
            AsmInstr::Mov(_, src, dst)
            | AsmInstr::Binary(_, _, src, dst)
            | AsmInstr::Cmp(_, src, dst)
            | AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Lea(src, dst) => {
                assert_no_pseudo_operand(src, instr)?;
                assert_no_pseudo_operand(dst, instr)?;
            }
            AsmInstr::Movsx(_, _, src, dst) | AsmInstr::MovZeroExtend(_, _, src, dst) => {
                assert_no_pseudo_operand(src, instr)?;
                assert_no_pseudo_operand(dst, instr)?;
                if is_memory(dst) {
                    return Err(format!(
                        "extension instruction has memory destination after fixup: {:?}",
                        instr
                    ));
                }
            }
            AsmInstr::Unary(_, _, op)
            | AsmInstr::Idiv(_, op)
            | AsmInstr::Div(_, op)
            | AsmInstr::SetCC(_, op)
            | AsmInstr::Push(op) => {
                assert_no_pseudo_operand(op, instr)?;
            }
            AsmInstr::LoadIndirect(_, _, dst)
            | AsmInstr::StoreIndirect(_, dst, _)
            | AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => {
                assert_no_pseudo_operand(dst, instr)?;
            }
            _ => {}
        }

        match instr {
            AsmInstr::Mov(_, src, dst) if is_memory(src) && is_memory(dst) => {
                return Err(format!(
                    "memory-to-memory mov after fixup in {}: {:?}",
                    func.name, instr
                ));
            }
            AsmInstr::Binary(AsmType::Double, _, _, dst) if !matches!(dst, AsmOperand::Xmm(_)) => {
                return Err(format!(
                    "double binary destination is not an XMM register in {}: {:?}",
                    func.name, instr
                ));
            }
            AsmInstr::Binary(_, _, src, dst) if is_memory(src) && is_memory(dst) => {
                return Err(format!(
                    "memory-to-memory binary instruction after fixup in {}: {:?}",
                    func.name, instr
                ));
            }
            AsmInstr::Cmp(_, src, dst) if is_memory(src) && is_memory(dst) => {
                return Err(format!(
                    "memory-to-memory cmp after fixup in {}: {:?}",
                    func.name, instr
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

// ============================================================
// Public API
// ============================================================

fn compute_aliased(
    body: &[TackyInstr],
    static_vars: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let mut aliased = static_vars.clone();
    for instr in body {
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

fn compute_ret_regs(
    body: &[TackyInstr],
    types: &HashMap<String, CType>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Vec<super::regalloc::RegId> {
    use super::regalloc::RegId;
    for instr in body {
        match instr {
            TackyInstr::Return(TackyVal::DoubleConstant(_)) => {
                return vec![RegId::Xmm(XmmReg::XMM0)];
            }
            TackyInstr::Return(TackyVal::Constant(_)) => {
                return vec![RegId::Gp(Reg::AX)];
            }
            TackyInstr::Return(TackyVal::Var(name)) => {
                let ct = types.get(name).copied().unwrap_or(CType::Int);
                return match ct {
                    CType::Float | CType::Double => vec![RegId::Xmm(XmmReg::XMM0)],
                    CType::Void => vec![],
                    CType::Struct => {
                        let mut regs = Vec::new();
                        if let Some(tag) = var_struct_tags.get(name) {
                            if let Some(def) = struct_defs.get(tag) {
                                let classes = def.classify_with(struct_defs);
                                let int_rets = [Reg::AX, Reg::DX];
                                let sse_rets = [XmmReg::XMM0, XmmReg::XMM1];
                                let (mut ir, mut sr) = (0usize, 0usize);
                                for c in &classes {
                                    match c {
                                        ParamClass::Integer => {
                                            regs.push(RegId::Gp(int_rets[ir]));
                                            ir += 1;
                                        }
                                        ParamClass::Sse => {
                                            regs.push(RegId::Xmm(sse_rets[sr]));
                                            sr += 1;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        if regs.is_empty() {
                            vec![RegId::Gp(Reg::AX)]
                        } else {
                            regs
                        }
                    }
                    _ => vec![RegId::Gp(Reg::AX)],
                };
            }
            _ => {}
        }
    }
    vec![] // void function
}

pub fn gen(program: &TackyProgram, no_coalescing: bool) -> Result<AsmProgram, String> {
    let static_vars = &program.global_vars;
    let types = &program.symbol_types;
    let array_sizes = &program.array_sizes;
    let alignments = &program.symbol_alignments;
    let mut top_level = Vec::new();
    let mut static_doubles = Vec::new();

    for tl in &program.top_level {
        match tl {
            TackyTopLevel::Function(tf) => {
                let mut asm_func = convert_function(
                    tf,
                    types,
                    array_sizes,
                    &mut static_doubles,
                    &program.var_struct_tags,
                    &program.struct_defs,
                )?;

                // Compute aliased variables (address-taken + static)
                let aliased = compute_aliased(&tf.body, static_vars);

                // Compute return value registers for EXIT node liveness
                let ret_regs = compute_ret_regs(
                    &tf.body,
                    types,
                    &program.var_struct_tags,
                    &program.struct_defs,
                );

                // Register allocation
                let allocation = super::regalloc::allocate_registers(
                    &mut asm_func,
                    &aliased,
                    types,
                    array_sizes,
                    &ret_regs,
                    no_coalescing,
                );

                // Phase 2: replace remaining pseudos with stack slots
                let stack_size = replace_pseudos(
                    &mut asm_func,
                    static_vars,
                    &program.thread_local_vars,
                    types,
                    array_sizes,
                    alignments,
                );

                // Phase 3: fix up instructions + callee-saved register handling
                fixup_instructions(&mut asm_func, stack_size, &allocation.callee_saved);
                verify_final_function(&asm_func)?;
                top_level.push(AsmTopLevel::Function(asm_func));
            }
            TackyTopLevel::StaticVar(sv) => {
                top_level.push(AsmTopLevel::StaticVar(AsmStaticVar {
                    name: sv.name.clone(),
                    global: sv.global,
                    thread_local: sv.thread_local,
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
            thread_local: false,
            alignment: 16,
            init_values: vec![StaticInit::DoubleInit(value)],
        }));
    }

    Ok(AsmProgram { top_level })
}
