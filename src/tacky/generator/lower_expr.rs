//! TACKY lowering for function calls, expressions, and statements.
//! Continuation of `impl TackyGen` (see mod.rs).

use super::*;

impl TackyGen {
    pub(super) fn emit_function_call(
        &mut self,
        name: String,
        args: Vec<Exp>,
        no_visible_prototype: bool,
    ) -> TackyResult<(TackyVal, CType)> {
        if let Some(inlined) = self.expand_inline_va_arg_pack_call(&name, &args) {
            return self.emit_exp(inlined);
        }
        self.emit_nested_capture_updates(&name);
        if name == "__builtin_apply_args" && args.is_empty() {
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            self.emit(TackyInstr::FrameAddress { dst: dst.clone() });
            return Ok((dst, CType::Pointer));
        }
        if name == "__builtin_apply" && args.len() == 3 {
            let Some(target) = Self::builtin_apply_target_name(&args[0]) else {
                return Err("__builtin_apply requires a direct function target".to_string());
            };
            if !matches!(&args[1], Exp::FunctionCall(inner, inner_args) | Exp::ImplicitFunctionCall(inner, inner_args)
                if inner == "__builtin_apply_args" && inner_args.is_empty())
            {
                let dst = self.fresh_tmp_full(&Self::void_pointer_type());
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(0),
                    dst: dst.clone(),
                });
                return Ok((dst, CType::Pointer));
            }
            let forwarded_args = self
                .current_function_params
                .iter()
                .cloned()
                .map(Exp::Var)
                .collect();
            let _ = self.emit_function_call(target, forwarded_args, false)?;
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }
        if name == "__builtin_return_address" && args.len() == 1 {
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }
        if name == "__builtin_extract_return_addr" && args.len() == 1 {
            let Some(arg_exp) = args.into_iter().next() else {
                return Err("__builtin_extract_return_addr requires an argument".to_string());
            };
            let (arg, arg_type) = self.emit_exp(arg_exp)?;
            let converted = self.convert_to(arg, arg_type, CType::Pointer);
            return Ok((converted, CType::Pointer));
        }
        if name == "__builtin_frame_address" && args.len() == 1 {
            let dst = self.fresh_tmp_full(&Self::void_pointer_type());
            if matches!(args.first(), Some(Exp::Constant(0))) {
                self.emit(TackyInstr::FrameAddress { dst: dst.clone() });
            } else {
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(0),
                    dst: dst.clone(),
                });
            }
            return Ok((dst, CType::Pointer));
        }
        if args.len() == 1 {
            if let Some(signature) = Self::bit_builtin_signature(&name) {
                let Some(arg_exp) = args.into_iter().next() else {
                    return Err(format!("{} requires an argument", name));
                };
                return self.emit_bit_builtin(
                    signature.kind,
                    signature.arg_type,
                    signature.width,
                    arg_exp,
                );
            }
        }
        if matches!(
            name.as_str(),
            "__builtin_conjf" | "__builtin_conj" | "__builtin_conjl"
        ) && args.len() == 1
        {
            let Some(arg_exp) = args.into_iter().next() else {
                return Err(format!("{} requires an argument", name));
            };
            if self.typeof_exp(&arg_exp).is_complex() {
                return self.emit_unary(UnaryOp::Complement, arg_exp);
            }
            return self.emit_exp(arg_exp);
        }
        if matches!(
            name.as_str(),
            "__builtin_inf"
                | "__builtin_inff"
                | "__builtin_infl"
                | "__builtin_huge_val"
                | "__builtin_huge_valf"
                | "__builtin_huge_vall"
        ) && args.is_empty()
        {
            let ret_type = match name.as_str() {
                "__builtin_inff" | "__builtin_huge_valf" => CType::Float,
                "__builtin_infl" | "__builtin_huge_vall" => self.long_double_ctype(),
                _ => CType::Double,
            };
            let raw_inf = self.fresh_tmp(CType::Double);
            self.emit(TackyInstr::Copy {
                src: TackyVal::DoubleConstant(f64::INFINITY),
                dst: raw_inf.clone(),
            });
            let dst = self.convert_to(raw_inf, CType::Double, ret_type);
            return Ok((dst, ret_type));
        }
        if matches!(
            name.as_str(),
            "__builtin_isinf" | "__builtin_isinff" | "__builtin_isinfl"
        ) && args.len() == 1
        {
            let arg_type = match name.as_str() {
                "__builtin_isinff" => CType::Float,
                "__builtin_isinfl" => self.long_double_ctype(),
                _ => CType::Double,
            };
            let Some(arg_exp) = args.into_iter().next() else {
                return Err(format!("{} requires an argument", name));
            };
            let (arg, from_type) = self.emit_exp(arg_exp)?;
            let value = self.convert_to(arg, from_type, arg_type);
            let (high_op, low_op, limit) = match arg_type {
                CType::Float => (
                    TackyBinaryOp::GreaterEqual,
                    TackyBinaryOp::LessEqual,
                    f32::MAX as f64,
                ),
                CType::Double => (
                    TackyBinaryOp::GreaterEqual,
                    TackyBinaryOp::LessEqual,
                    f64::MAX,
                ),
                _ => (TackyBinaryOp::Equal, TackyBinaryOp::Equal, f64::INFINITY),
            };
            let raw_high_limit = self.fresh_tmp(CType::Double);
            self.emit(TackyInstr::Copy {
                src: TackyVal::DoubleConstant(limit),
                dst: raw_high_limit.clone(),
            });
            let high_limit = self.convert_to(raw_high_limit, CType::Double, arg_type);
            let high = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: high_op,
                left: value.clone(),
                right: high_limit,
                dst: high.clone(),
            });
            let raw_low_limit = self.fresh_tmp(CType::Double);
            self.emit(TackyInstr::Copy {
                src: TackyVal::DoubleConstant(-limit),
                dst: raw_low_limit.clone(),
            });
            let low_limit = self.convert_to(raw_low_limit, CType::Double, arg_type);
            let low = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: low_op,
                left: value,
                right: low_limit,
                dst: low.clone(),
            });
            let result = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseOr,
                left: high,
                right: low,
                dst: result.clone(),
            });
            return Ok((result, CType::Int));
        }
        if name == "__builtin_shuffle" && args.len() == 2 {
            let src_exp = args[0].clone();
            let mask_exp = args[1].clone();
            let src_ft = self.typeof_exp(&src_exp);
            let FullType::Vector { elem, lanes, .. } = src_ft.clone() else {
                return Err("__builtin_shuffle requires a vector source".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&src_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("__builtin_shuffle result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, src_ft.byte_size_with(&self.struct_defs));
            for lane in 0..lanes {
                let (mask_val, mask_type) =
                    self.emit_subscript(mask_exp.clone(), Exp::Constant(lane as i64))?;
                let mut index = self.convert_to(mask_val, mask_type, CType::Long);
                if lanes.is_power_of_two() {
                    let masked = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::BitwiseAnd,
                        left: index,
                        right: TackyVal::Constant((lanes - 1) as i64),
                        dst: masked.clone(),
                    });
                    index = masked;
                }
                let index_name = match index {
                    TackyVal::Var(name) => name,
                    other => {
                        let tmp = self.fresh_tmp(CType::Long);
                        self.emit(TackyInstr::Copy {
                            src: other,
                            dst: tmp.clone(),
                        });
                        match tmp {
                            TackyVal::Var(name) => name,
                            _ => {
                                return Err(
                                    "internal error: expected temporary variable".to_string()
                                )
                            }
                        }
                    }
                };
                let (src_ptr, _, _) =
                    self.emit_subscript_addr(src_exp.clone(), Exp::Var(index_name))?;
                let lane_val = self.fresh_tmp(elem_type);
                self.emit(TackyInstr::Load {
                    src_ptr,
                    dst: lane_val.clone(),
                });
                self.emit(TackyInstr::CopyToOffset {
                    src: lane_val,
                    dst_name: result_name.clone(),
                    offset: (lane * elem_size) as i64,
                });
            }
            return Ok((result, src_ft.to_ctype()));
        }
        if name == "__builtin_setjmp" && args.len() == 1 {
            let (buf, buf_ty) = self.emit_exp(args[0].clone())?;
            let buf = self.convert_to(buf, buf_ty, CType::Pointer);
            let dst = self.fresh_tmp(CType::Int);
            let label = self.fresh_label("builtin_setjmp_resume");
            let end_label = self.fresh_label("builtin_setjmp_end");
            self.emit(TackyInstr::BuiltinSetjmp {
                buf,
                dst: dst.clone(),
                label,
                end_label,
            });
            return Ok((dst, CType::Int));
        }
        if name == "__builtin_longjmp" && args.len() == 2 {
            let (buf, buf_ty) = self.emit_exp(args[0].clone())?;
            let buf = self.convert_to(buf, buf_ty, CType::Pointer);
            let (value, value_ty) = self.emit_exp(args[1].clone())?;
            let value = self.convert_to(value, value_ty, CType::Int);
            self.emit(TackyInstr::BuiltinLongjmp { buf, value });
            return Ok((TackyVal::Constant(0), CType::Void));
        }
        if name == "__builtin_va_start" && !args.is_empty() {
            match &args[0] {
                Exp::Var(ap_name) => {
                    self.emit(TackyInstr::VaStart {
                        dst: TackyVal::Var(ap_name.clone()),
                    });
                }
                _ => {
                    let Some((ap_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())? else {
                        return Err("__builtin_va_start requires a va_list object".to_string());
                    };
                    let current = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::VaStart {
                        dst: current.clone(),
                    });
                    self.emit(TackyInstr::Store {
                        src: current,
                        dst_ptr: ap_addr,
                    });
                }
            }
            return Ok((TackyVal::Constant(0), CType::Int));
        }
        if matches!(name.as_str(), "__builtin_va_copy" | "__va_copy") && args.len() == 2 {
            let (src, _) = self.emit_exp(args[1].clone())?;
            match &args[0] {
                Exp::Var(dst_name) => {
                    self.emit(TackyInstr::Copy {
                        src,
                        dst: TackyVal::Var(dst_name.clone()),
                    });
                }
                _ => {
                    let Some((dst_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())?
                    else {
                        return Err("__builtin_va_copy requires a va_list destination".to_string());
                    };
                    self.emit(TackyInstr::Store {
                        src,
                        dst_ptr: dst_addr,
                    });
                }
            }
            return Ok((TackyVal::Constant(0), CType::Int));
        }
        if name == "__builtin_bswap16" && args.len() == 1 {
            let (value, value_ty) = self.emit_exp(args[0].clone())?;
            let value = self.convert_to(value, value_ty, CType::UInt);
            let low = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: value.clone(),
                right: TackyVal::Constant(0xff),
                dst: low.clone(),
            });
            let low_shifted = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: low,
                right: TackyVal::Constant(8),
                dst: low_shifted.clone(),
            });
            let high = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftRight,
                left: value,
                right: TackyVal::Constant(8),
                dst: high.clone(),
            });
            let high_masked = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: high,
                right: TackyVal::Constant(0xff),
                dst: high_masked.clone(),
            });
            let result = self.fresh_tmp(CType::UInt);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseOr,
                left: low_shifted,
                right: high_masked,
                dst: result.clone(),
            });
            return Ok((result, CType::UInt));
        }
        if let Some(tag) = name.strip_prefix("__rnqcc_va_arg_struct_") {
            if args.len() != 1 {
                return Err("__builtin_va_arg requires one va_list argument".to_string());
            }
            let tag = self.resolve_struct_tag_name(tag);
            let ft = FullType::Struct(tag.to_string());
            let struct_size = ft.byte_size_with(&self.struct_defs);
            let (slot_ptr, ap_store) = match &args[0] {
                Exp::Var(ap_name) => (
                    self.emit_exp(args[0].clone())?.0,
                    TackyVal::Var(ap_name.clone()),
                ),
                _ => {
                    let Some((ap_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())? else {
                        return Err("__builtin_va_arg requires a va_list object".to_string());
                    };
                    let current = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::Load {
                        src_ptr: ap_addr.clone(),
                        dst: current.clone(),
                    });
                    (current, ap_addr)
                }
            };
            let dst = self.fresh_tmp_full(&ft);
            let slot_ptr =
                self.emit_aligned_pointer(slot_ptr, ft.alignment_with(&self.struct_defs).min(16));
            if let TackyVal::Var(ref dst_name) = dst {
                self.emit_struct_copy_to(slot_ptr.clone(), dst_name, struct_size);
            }
            let next = self.fresh_tmp(CType::Pointer);
            let slot_size = struct_size.next_multiple_of(8);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: slot_ptr,
                right: TackyVal::Constant(slot_size as i64),
                dst: next.clone(),
            });
            match args[0] {
                Exp::Var(_) => self.emit(TackyInstr::Copy {
                    src: next,
                    dst: ap_store,
                }),
                _ => self.emit(TackyInstr::Store {
                    src: next,
                    dst_ptr: ap_store,
                }),
            }
            return Ok((dst, CType::Struct));
        }
        if let Some(arg_type) = name
            .strip_prefix("__rnqcc_va_arg_")
            .and_then(Self::va_arg_helper_type)
        {
            if args.len() != 1 {
                return Err("__builtin_va_arg requires one va_list argument".to_string());
            }
            let (slot_ptr, ap_store) = match &args[0] {
                Exp::Var(ap_name) => (
                    self.emit_exp(args[0].clone())?.0,
                    TackyVal::Var(ap_name.clone()),
                ),
                _ => {
                    let Some((ap_addr, _, _)) = self.scalar_lvalue_address(args[0].clone())? else {
                        return Err("__builtin_va_arg requires a va_list object".to_string());
                    };
                    let current = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::Load {
                        src_ptr: ap_addr.clone(),
                        dst: current.clone(),
                    });
                    (current, ap_addr)
                }
            };
            let dst = self.fresh_tmp(arg_type);
            let slot_ptr =
                self.emit_aligned_pointer(slot_ptr, std::cmp::min(arg_type.size() as usize, 16));
            self.emit(TackyInstr::Load {
                src_ptr: slot_ptr.clone(),
                dst: dst.clone(),
            });
            let next = self.fresh_tmp(CType::Pointer);
            let size = arg_type.size();
            let slot_size = std::cmp::max(8, ((size + 7) / 8) * 8);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: slot_ptr,
                right: TackyVal::Constant(slot_size as i64),
                dst: next.clone(),
            });
            match args[0] {
                Exp::Var(_) => self.emit(TackyInstr::Copy {
                    src: next,
                    dst: ap_store,
                }),
                _ => self.emit(TackyInstr::Store {
                    src: next,
                    dst_ptr: ap_store,
                }),
            }
            return Ok((dst, arg_type));
        }
        if name == "__builtin___sprintf_chk" && args.len() >= 4 {
            self.func_types.insert(
                "sprintf".to_string(),
                (CType::Int, vec![CType::Pointer, CType::Pointer], None, true),
            );
            self.func_full_types
                .insert("sprintf".to_string(), FullType::Scalar(CType::Int));
            self.func_param_full_types.insert(
                "sprintf".to_string(),
                vec![Self::char_pointer_type(), Self::char_pointer_type()],
            );
            let mut lowered_args = Vec::with_capacity(args.len() - 2);
            lowered_args.push(args[0].clone());
            lowered_args.push(args[3].clone());
            lowered_args.extend(args.into_iter().skip(4));
            return self.emit_function_call("sprintf".to_string(), lowered_args, false);
        }
        if name == "mempcpy" && args.len() == 3 {
            self.emit_function_call("memcpy".to_string(), args.clone(), false)?;
            let (dst, dst_ty) = self.emit_exp(args[0].clone())?;
            let (count, count_ty) = self.emit_exp(args[2].clone())?;
            let count = self.convert_to(count, count_ty, CType::Long);
            let dst = self.convert_to(dst, dst_ty, CType::Pointer);
            let result = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst,
                right: count,
                dst: result.clone(),
            });
            return Ok((result, CType::Pointer));
        }
        if name == "__builtin_mul_overflow" && args.len() == 3 {
            let out_ft = self.typeof_exp(&args[2]);
            let target_type = match out_ft {
                FullType::Pointer(inner) => inner.to_ctype(),
                _ => CType::Long,
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let (out_ptr, _) = self.emit_exp(args[2].clone())?;
            if let Some((min, max)) = match target_type {
                CType::Char | CType::SChar => Some((i8::MIN as i128, i8::MAX as i128)),
                CType::Short => Some((i16::MIN as i128, i16::MAX as i128)),
                CType::Int => Some((i32::MIN as i128, i32::MAX as i128)),
                CType::Long => Some((i64::MIN as i128, i64::MAX as i128)),
                CType::UChar => Some((0, u8::MAX as i128)),
                CType::UShort => Some((0, u16::MAX as i128)),
                CType::UInt => Some((0, u32::MAX as i128)),
                CType::ULong => Some((0, u64::MAX as i128)),
                _ => None,
            } {
                let left_wide = self.convert_to(left, left_ty, CType::Int128);
                let right_wide = self.convert_to(right, right_ty, CType::Int128);
                let product_wide = self.fresh_tmp(CType::Int128);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Mul,
                    left: left_wide,
                    right: right_wide,
                    dst: product_wide.clone(),
                });
                let high = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterThan,
                    left: product_wide.clone(),
                    right: TackyVal::Int128Constant(max),
                    dst: high.clone(),
                });
                let low = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::LessThan,
                    left: product_wide.clone(),
                    right: TackyVal::Int128Constant(min),
                    dst: low.clone(),
                });
                let overflow = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseOr,
                    left: high,
                    right: low,
                    dst: overflow.clone(),
                });
                let product = self.convert_to(product_wide, CType::Int128, target_type);
                self.emit(TackyInstr::Store {
                    src: product,
                    dst_ptr: out_ptr,
                });
                return Ok((overflow, CType::Int));
            }
            let left = self.convert_to(left, left_ty, target_type);
            let right = self.convert_to(right, right_ty, target_type);
            let product = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Mul,
                left: left.clone(),
                right: right.clone(),
                dst: product.clone(),
            });
            self.emit(TackyInstr::Store {
                src: product.clone(),
                dst_ptr: out_ptr,
            });

            if matches!(target_type, CType::Int128 | CType::UInt128) {
                return Ok((TackyVal::Constant(0), CType::Int));
            }

            let overflow = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: overflow.clone(),
            });
            let zero = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Equal,
                left: left.clone(),
                right: TackyVal::Constant(0),
                dst: zero.clone(),
            });
            let end_label = self.fresh_label("builtin_mul_overflow_end");
            self.emit(TackyInstr::JumpIfNotZero(zero, end_label.clone()));
            let quotient = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Div,
                left: product,
                right: left,
                dst: quotient.clone(),
            });
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::NotEqual,
                left: quotient,
                right,
                dst: overflow.clone(),
            });
            self.emit(TackyInstr::Label(end_label));
            return Ok((overflow, CType::Int));
        }
        if name == "__builtin_add_overflow" && args.len() == 3 {
            let out_ft = self.typeof_exp(&args[2]);
            let target_type = match out_ft {
                FullType::Pointer(inner) => inner.to_ctype(),
                _ => CType::Long,
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let (out_ptr, _) = self.emit_exp(args[2].clone())?;
            if let Some((min, max)) = match target_type {
                CType::Char | CType::SChar => Some((i8::MIN as i128, i8::MAX as i128)),
                CType::Short => Some((i16::MIN as i128, i16::MAX as i128)),
                CType::Int => Some((i32::MIN as i128, i32::MAX as i128)),
                CType::Long => Some((i64::MIN as i128, i64::MAX as i128)),
                CType::UChar => Some((0, u8::MAX as i128)),
                CType::UShort => Some((0, u16::MAX as i128)),
                CType::UInt => Some((0, u32::MAX as i128)),
                CType::ULong => Some((0, u64::MAX as i128)),
                _ => None,
            } {
                let left_wide = self.convert_to(left, left_ty, CType::Int128);
                let right_wide = self.convert_to(right, right_ty, CType::Int128);
                let sum_wide = self.fresh_tmp(CType::Int128);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: left_wide,
                    right: right_wide,
                    dst: sum_wide.clone(),
                });
                let high = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterThan,
                    left: sum_wide.clone(),
                    right: TackyVal::Int128Constant(max),
                    dst: high.clone(),
                });
                let low = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::LessThan,
                    left: sum_wide.clone(),
                    right: TackyVal::Int128Constant(min),
                    dst: low.clone(),
                });
                let overflow = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseOr,
                    left: high,
                    right: low,
                    dst: overflow.clone(),
                });
                let sum = self.convert_to(sum_wide, CType::Int128, target_type);
                self.emit(TackyInstr::Store {
                    src: sum,
                    dst_ptr: out_ptr,
                });
                return Ok((overflow, CType::Int));
            }
            let left = self.convert_to(left, left_ty, target_type);
            let right = self.convert_to(right, right_ty, target_type);
            let sum = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: left.clone(),
                right,
                dst: sum.clone(),
            });
            let overflow = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: sum.clone(),
                right: left,
                dst: overflow.clone(),
            });
            self.emit(TackyInstr::Store {
                src: sum,
                dst_ptr: out_ptr,
            });
            return Ok((overflow, CType::Int));
        }
        if name == "__builtin_sub_overflow" && args.len() == 3 {
            let out_ft = if let Exp::Unary(UnaryOp::AddrOf, inner) = &args[2] {
                FullType::Pointer(Box::new(self.typeof_exp(inner)))
            } else {
                self.typeof_exp(&args[2])
            };
            let target_type = match out_ft {
                FullType::Pointer(inner) => inner.to_ctype(),
                _ => CType::Long,
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let (out_ptr, _) = self.emit_exp(args[2].clone())?;
            if let Some((min, max)) = match target_type {
                CType::Char | CType::SChar => Some((i8::MIN as i128, i8::MAX as i128)),
                CType::Short => Some((i16::MIN as i128, i16::MAX as i128)),
                CType::Int => Some((i32::MIN as i128, i32::MAX as i128)),
                CType::Long => Some((i64::MIN as i128, i64::MAX as i128)),
                CType::UChar => Some((0, u8::MAX as i128)),
                CType::UShort => Some((0, u16::MAX as i128)),
                CType::UInt => Some((0, u32::MAX as i128)),
                CType::ULong => Some((0, u64::MAX as i128)),
                _ => None,
            } {
                let left_wide = self.convert_to(left, left_ty, CType::Int128);
                let right_wide = self.convert_to(right, right_ty, CType::Int128);
                let diff_wide = self.fresh_tmp(CType::Int128);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Sub,
                    left: left_wide,
                    right: right_wide,
                    dst: diff_wide.clone(),
                });
                let high = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::GreaterThan,
                    left: diff_wide.clone(),
                    right: TackyVal::Int128Constant(max),
                    dst: high.clone(),
                });
                let low = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::LessThan,
                    left: diff_wide.clone(),
                    right: TackyVal::Int128Constant(min),
                    dst: low.clone(),
                });
                let overflow = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseOr,
                    left: high,
                    right: low,
                    dst: overflow.clone(),
                });
                let diff = self.convert_to(diff_wide, CType::Int128, target_type);
                self.emit(TackyInstr::Store {
                    src: diff,
                    dst_ptr: out_ptr,
                });
                return Ok((overflow, CType::Int));
            }
            let left = self.convert_to(left, left_ty, target_type);
            let right = self.convert_to(right, right_ty, target_type);
            let diff = self.fresh_tmp(target_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Sub,
                left: left.clone(),
                right: right.clone(),
                dst: diff.clone(),
            });
            let overflow = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: left.clone(),
                right,
                dst: overflow.clone(),
            });
            self.emit(TackyInstr::Store {
                src: diff,
                dst_ptr: out_ptr,
            });
            return Ok((overflow, CType::Int));
        }
        if name == "__builtin_mul_overflow_p" && args.len() == 3 {
            let target_type = self.typeof_exp(&args[2]).to_ctype();
            let Some(range) = Self::integer_range_for_type(target_type) else {
                return Ok((TackyVal::Constant(0), CType::Int));
            };
            let (left, left_ty) = self.emit_exp(args[0].clone())?;
            let (right, right_ty) = self.emit_exp(args[1].clone())?;
            let left_wide = self.convert_to(left, left_ty, CType::Int128);
            let right_wide = self.convert_to(right, right_ty, CType::Int128);
            let product_wide = self.fresh_tmp(CType::Int128);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Mul,
                left: left_wide,
                right: right_wide,
                dst: product_wide.clone(),
            });
            let high = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::GreaterThan,
                left: product_wide.clone(),
                right: TackyVal::Int128Constant(range.max),
                dst: high.clone(),
            });
            let low = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: product_wide,
                right: TackyVal::Int128Constant(range.min),
                dst: low.clone(),
            });
            let dst = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseOr,
                left: high,
                right: low,
                dst: dst.clone(),
            });
            return Ok((dst, CType::Int));
        }
        if matches!(name.as_str(), "__builtin_ctz" | "__builtin_clz") && args.len() == 1 {
            let (arg, arg_ty) = self.emit_exp(args[0].clone())?;
            let value = self.convert_to(arg, arg_ty, CType::UInt);
            let count = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Copy {
                src: TackyVal::Constant(0),
                dst: count.clone(),
            });
            if name == "__builtin_ctz" {
                let current = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Copy {
                    src: value,
                    dst: current.clone(),
                });
                let loop_label = self.fresh_label("builtin_ctz_loop");
                let end_label = self.fresh_label("builtin_ctz_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let bit = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: current.clone(),
                    right: TackyVal::Constant(1),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(bit, end_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: count.clone(),
                    right: TackyVal::Constant(1),
                    dst: count.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: current.clone(),
                    right: TackyVal::Constant(1),
                    dst: current,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            } else {
                let mask = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Copy {
                    src: TackyVal::Constant(1_i64 << 31),
                    dst: mask.clone(),
                });
                let loop_label = self.fresh_label("builtin_clz_loop");
                let end_label = self.fresh_label("builtin_clz_end");
                self.emit(TackyInstr::Label(loop_label.clone()));
                let bit = self.fresh_tmp(CType::UInt);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: value.clone(),
                    right: mask.clone(),
                    dst: bit.clone(),
                });
                self.emit(TackyInstr::JumpIfNotZero(bit, end_label.clone()));
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: count.clone(),
                    right: TackyVal::Constant(1),
                    dst: count.clone(),
                });
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::ShiftRight,
                    left: mask.clone(),
                    right: TackyVal::Constant(1),
                    dst: mask,
                });
                self.emit(TackyInstr::Jump(loop_label));
                self.emit(TackyInstr::Label(end_label));
            }
            return Ok((count, CType::Int));
        }
        let pointer_sig = self
            .full_types
            .get(&name)
            .and_then(Self::function_signature_from_full);
        let user_declares_function = self.func_types.contains_key(&name) || pointer_sig.is_some();
        let builtin_info = if user_declares_function && !name.starts_with("__builtin_") {
            None
        } else {
            self.builtin_function_info(&name)
        };
        let call_name = builtin_info
            .as_ref()
            .map(|(call_name, _, _, _, _)| (*call_name).to_string())
            .unwrap_or_else(|| name.clone());
        if matches!(name.as_str(), "alloca" | "__builtin_alloca") && args.len() == 1 {
            let size = eval_static_integer_constant_exp_with_context(
                &args[0],
                &self.struct_defs,
                &self.full_types,
            )
            .and_then(|constant| usize::try_from(constant.value).ok());
            let Some(size) = size else {
                let _ = self.emit_exp(args[0].clone())?;
                let name = format!("__alloca_dynamic.{}", self.current_function);
                let size = 1024 * 1024;
                if !self.array_sizes.contains_key(&name) {
                    let ft = FullType::Array {
                        elem: Box::new(FullType::Scalar(CType::Char)),
                        size,
                    };
                    self.register_var(&name, ft);
                    self.array_sizes.insert(name.clone(), size);
                }
                let dst = self.fresh_tmp_full(&Self::void_pointer_type());
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(name),
                    dst: dst.clone(),
                });
                return Ok((dst, CType::Pointer));
            };
            if size == 0 {
                let dst = self.fresh_tmp_full(&Self::void_pointer_type());
                self.emit(TackyInstr::FrameAddress { dst: dst.clone() });
                return Ok((dst, CType::Pointer));
            }
            let name = self.fresh_var_name();
            let ft = FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size,
            };
            self.register_var(&name, ft);
            self.array_sizes.insert(name.clone(), size);
            let ptr_ft = Self::void_pointer_type();
            let dst = self.fresh_tmp_full(&ptr_ft);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(name),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }
        let uses_standard_abs_builtin = builtin_info.is_some()
            || self
                .func_types
                .get(&name)
                .is_some_and(|(ret, params, _, _)| {
                    params.len() == 1
                        && match name.as_str() {
                            "abs" | "__builtin_abs" => {
                                *ret == CType::Int && params[0] == CType::Int
                            }
                            "labs" | "__builtin_labs" | "llabs" | "__builtin_llabs" => {
                                *ret == CType::Long && params[0] == CType::Long
                            }
                            _ => false,
                        }
                });
        if matches!(
            name.as_str(),
            "abs" | "__builtin_abs" | "labs" | "__builtin_labs" | "llabs" | "__builtin_llabs"
        ) && uses_standard_abs_builtin
            && args.len() == 1
        {
            let ret_type = match name.as_str() {
                "abs" | "__builtin_abs" => CType::Int,
                _ => CType::Long,
            };
            let Some(arg_exp) = args.into_iter().next() else {
                return Err(format!("{} requires an argument", name));
            };
            let (arg, arg_type) = self.emit_exp(arg_exp)?;
            let arg = self.convert_to(arg, arg_type, ret_type);
            let dst = self.fresh_tmp(ret_type);
            self.emit(TackyInstr::Copy {
                src: arg,
                dst: dst.clone(),
            });
            let is_negative = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::LessThan,
                left: dst.clone(),
                right: TackyVal::Constant(0),
                dst: is_negative.clone(),
            });
            let end_label = self.fresh_label("abs_end");
            self.emit(TackyInstr::JumpIfZero(is_negative, end_label.clone()));
            self.emit(TackyInstr::Unary {
                op: TackyUnaryOp::Negate,
                src: dst.clone(),
                dst: dst.clone(),
            });
            self.emit(TackyInstr::Label(end_label));
            return Ok((dst, ret_type));
        }
        let (ret_type, param_types, ret_pi, variadic) =
            if no_visible_prototype && builtin_info.is_none() {
                (CType::Int, Vec::new(), None, false)
            } else if let Some((_, ret_type, _, param_types, ret_pi)) = builtin_info.as_ref() {
                (
                    *ret_type,
                    param_types.clone(),
                    *ret_pi,
                    matches!(
                        name.as_str(),
                        "__builtin_printf" | "__builtin_sprintf" | "__builtin_snprintf"
                    ),
                )
            } else {
                self.func_types
                    .get(&name)
                    .cloned()
                    .or_else(|| {
                        pointer_sig.as_ref().map(|signature| {
                            let ret_pi = match &signature.return_type {
                                FullType::Pointer(inner) => Some(Self::ptr_info_from_full(inner)),
                                _ => None,
                            };
                            (
                                signature.return_type.to_ctype(),
                                signature.params.iter().map(FullType::to_ctype).collect(),
                                ret_pi,
                                signature.variadic,
                            )
                        })
                    })
                    .unwrap_or((CType::Int, Vec::new(), None, false))
            };
        let direct_old_style_call = no_visible_prototype
            || (self.old_style_functions.contains(&name)
                && builtin_info.is_none()
                && pointer_sig.is_none());
        let param_types = if direct_old_style_call {
            Vec::new()
        } else {
            param_types
        };
        let has_prototype = !no_visible_prototype
            && (builtin_info.is_some()
                || pointer_sig.is_some()
                || (self.func_types.contains_key(&name) && !direct_old_style_call));
        if has_prototype && !variadic && args.len() != param_types.len() {
            return Err(format!(
                "function '{}' called with {} argument(s), but prototype expects {}",
                name,
                args.len(),
                param_types.len()
            ));
        }
        if has_prototype && variadic && args.len() < param_types.len() {
            return Err(format!(
                "variadic function '{}' called with {} argument(s), but prototype requires at least {}",
                name,
                args.len(),
                param_types.len()
            ));
        }

        let ret_ft = if no_visible_prototype && builtin_info.is_none() {
            None
        } else {
            builtin_info
                .as_ref()
                .map(|(_, _, ret_ft, _, _)| ret_ft.clone())
                .or_else(|| {
                    self.func_full_types.get(&name).cloned().or_else(|| {
                        pointer_sig
                            .as_ref()
                            .map(|signature| signature.return_type.clone())
                    })
                })
        };
        let param_full_types: Vec<FullType> = if direct_old_style_call {
            Vec::new()
        } else {
            self.func_param_full_types
                .get(&name)
                .cloned()
                .or_else(|| {
                    pointer_sig
                        .as_ref()
                        .map(|signature| signature.params.clone())
                })
                .unwrap_or_default()
        };
        let param_full_types = self.canonical_param_full_types(&param_full_types);
        let param_types = if param_full_types.is_empty() {
            param_types
        } else {
            param_full_types
                .iter()
                .map(FullType::to_ctype)
                .collect::<Vec<_>>()
        };

        let mut tacky_args = Vec::new();
        let stack_arg_indices = std::collections::HashSet::new();
        let mut memory_arg_blocks = Vec::new();
        let mut struct_arg_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut fixed_flat_arg_count = 0usize;
        for (i, arg) in args.into_iter().enumerate() {
            let arg_for_type = arg.clone();
            let (val, val_type) = self.emit_exp(arg)?;
            let val_ft = self.val_full_type(&val);
            let expected = param_full_types
                .get(i)
                .map(|ft| self.storage_ctype_for_full(ft))
                .or_else(|| param_types.get(i).copied())
                .unwrap_or_else(|| val_type.promote());
            if let Some(expected_ft) = param_full_types.get(i) {
                let context = format!("function call to {} argument {}", name, i + 1);
                self.assert_assignable_exp_full_type(
                    expected_ft,
                    &val_ft,
                    &arg_for_type,
                    &context,
                )?;
            }

            let source_ft = self.typeof_exp(&arg_for_type);
            let memory_vector_ft = param_full_types
                .get(i)
                .filter(|ft| self.vector_requires_memory_abi(ft))
                .or_else(|| self.vector_requires_memory_abi(&val_ft).then_some(&val_ft))
                .or_else(|| {
                    self.vector_requires_memory_abi(&source_ft)
                        .then_some(&source_ft)
                });
            if let Some(vector_ft) = memory_vector_ft {
                let arg_idx = tacky_args.len();
                tacky_args.push(self.get_struct_addr(val));
                memory_arg_blocks.push((
                    arg_idx,
                    vector_ft.byte_size_with(&self.struct_defs),
                    vector_ft.alignment_with(&self.struct_defs).max(16),
                ));
                if i + 1 == param_types.len() {
                    fixed_flat_arg_count = tacky_args.len();
                }
                continue;
            }
            let expected_ft_is_complex = param_full_types.get(i).is_some_and(FullType::is_complex);
            let is_complex_arg =
                expected_ft_is_complex || val_ft.is_complex() || source_ft.is_complex();
            if is_complex_arg {
                let complex_ft = param_full_types
                    .get(i)
                    .filter(|ft| ft.is_complex())
                    .cloned()
                    .unwrap_or_else(|| {
                        if val_ft.is_complex() {
                            val_ft.clone()
                        } else {
                            source_ft.clone()
                        }
                    });
                let FullType::Vector { elem, .. } = complex_ft.clone() else {
                    return Err("internal error: expected complex vector type".to_string());
                };
                let elem_type = elem.to_ctype();
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let group_start = tacky_args.len();
                let real = self.emit_complex_component_value(
                    val.clone(),
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    0,
                )?;
                let imag = self.emit_complex_component_value(
                    val,
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    1,
                )?;
                tacky_args.push(real);
                tacky_args.push(imag);
                let lane_is_float = elem_type.is_floating();
                struct_arg_groups.push((group_start, 2, vec![lane_is_float, lane_is_float]));
                if i + 1 == param_types.len() {
                    fixed_flat_arg_count = tacky_args.len();
                }
                continue;
            }
            let is_struct_arg =
                source_ft.is_struct() || val_ft.is_struct() || val_type == CType::Struct;
            if is_struct_arg || expected == CType::Struct {
                let tag = match &val_ft {
                    FullType::Struct(t) => t.clone(),
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => {
                            tacky_args.push(val);
                            if i + 1 == param_types.len() {
                                fixed_flat_arg_count = tacky_args.len();
                            }
                            continue;
                        }
                    },
                    _ => {
                        tacky_args.push(val);
                        if i + 1 == param_types.len() {
                            fixed_flat_arg_count = tacky_args.len();
                        }
                        continue;
                    }
                };

                if let Some(def) = self.struct_defs.get(&tag).cloned() {
                    let classes = def.classify_with(&self.struct_defs);
                    let is_variadic_extra = variadic && i >= param_types.len();
                    if is_variadic_extra || (classes.len() == 1 && classes[0] == ParamClass::Memory)
                    {
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let arg_idx = tacky_args.len();
                        tacky_args.push(struct_addr);
                        memory_arg_blocks.push((arg_idx, def.size, def.alignment));
                    } else {
                        let struct_var_name = if val_ft.is_struct() {
                            if let TackyVal::Var(ref n) = val {
                                Some(n.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let group_start = tacky_args.len();
                        let is_sse_vec: Vec<bool> =
                            classes.iter().map(|c| *c == ParamClass::Sse).collect();
                        if !classes.is_empty() {
                            struct_arg_groups.push((group_start, classes.len(), is_sse_vec));
                        }
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i64;
                            match class {
                                ParamClass::Sse => {
                                    let tmp = self.fresh_tmp(CType::Double);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                ParamClass::Integer => {
                                    let tmp = self.fresh_tmp(CType::Long);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                _ => tacky_args.push(TackyVal::Constant(0)),
                            }
                        }
                    }
                } else if param_full_types.get(i).is_some_and(FullType::is_vector) {
                    tacky_args.push(val);
                } else {
                    tacky_args.push(self.convert_to(val, val_type, expected));
                }
            } else if param_full_types.get(i).is_some_and(FullType::is_vector) {
                tacky_args.push(val);
            } else {
                tacky_args.push(self.convert_to(val, val_type, expected));
            }

            if i + 1 == param_types.len() {
                fixed_flat_arg_count = tacky_args.len();
            }
        }
        if param_types.is_empty() {
            fixed_flat_arg_count =
                if variadic && !self.zero_fixed_variadic_functions.contains(&name) {
                    tacky_args.len()
                } else {
                    0
                };
        }

        let uses_hidden_ptr = ret_ft
            .as_ref()
            .is_some_and(|ft| self.return_requires_hidden_pointer(ft));
        let is_indirect = builtin_info.is_none()
            && !self.func_types.contains_key(&name)
            && !self.function_symbols.contains(&name)
            && !name.starts_with("__builtin_");
        if uses_hidden_ptr {
            let rft = ret_ft
                .as_ref()
                .ok_or_else(|| "missing struct return type".to_string())?;
            let tmp = self.fresh_tmp_full(rft);
            if let FullType::Struct(ref tag) = rft {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    if let Some(def) = self.struct_defs.get(tag) {
                        self.array_sizes.insert(tmp_name.clone(), def.size);
                    }
                }
            } else if rft.is_complex() {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    self.array_sizes
                        .insert(tmp_name.clone(), rft.byte_size_with(&self.struct_defs));
                }
            }
            let ret_addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: tmp.clone(),
                dst: ret_addr.clone(),
            });
            tacky_args.insert(0, ret_addr);
            let shifted_stack = stack_arg_indices.iter().map(|&i| i + 1).collect();
            let shifted_memory_blocks: Vec<(usize, usize, usize)> = memory_arg_blocks
                .iter()
                .map(|(index, size, align)| (index + 1, *size, *align))
                .collect();
            let shifted_groups: Vec<(usize, usize, Vec<bool>)> = struct_arg_groups
                .iter()
                .map(|(start, count, classes)| (start + 1, *count, classes.clone()))
                .collect();
            self.emit(TackyInstr::FunCall {
                name: call_name,
                args: tacky_args,
                dst: tmp.clone(),
                stack_arg_indices: shifted_stack,
                memory_arg_blocks: shifted_memory_blocks,
                struct_arg_groups: shifted_groups,
                variadic,
                fixed_flat_arg_count: fixed_flat_arg_count + 1,
                hidden_return: true,
                indirect: is_indirect,
            });
            if let Some(pi) = ret_pi {
                if let TackyVal::Var(ref dst_name) = tmp {
                    self.ptr_info.insert(dst_name.clone(), pi);
                }
            }
            return Ok((tmp, ret_type));
        }

        let dst = if let Some(ref rft) = ret_ft {
            let tmp = self.fresh_tmp_full(rft);
            if let FullType::Struct(ref tag) = rft {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    if let Some(def) = self.struct_defs.get(tag) {
                        let classes = def.classify_with(&self.struct_defs);
                        let alloc_size = std::cmp::max(def.size, classes.len() * 8);
                        self.array_sizes.insert(tmp_name.clone(), alloc_size);
                    }
                }
            } else if rft.is_complex() {
                if let TackyVal::Var(ref tmp_name) = tmp {
                    self.array_sizes
                        .insert(tmp_name.clone(), rft.byte_size_with(&self.struct_defs));
                }
            }
            tmp
        } else {
            self.fresh_tmp(ret_type)
        };
        self.emit(TackyInstr::FunCall {
            name: call_name,
            args: tacky_args,
            dst: dst.clone(),
            stack_arg_indices,
            memory_arg_blocks,
            struct_arg_groups,
            variadic,
            fixed_flat_arg_count,
            hidden_return: false,
            indirect: is_indirect,
        });
        if let Some(pi) = ret_pi {
            if let TackyVal::Var(ref dst_name) = dst {
                self.ptr_info.insert(dst_name.clone(), pi);
            }
        }
        Ok((dst, ret_type))
    }

    pub(super) fn emit_indirect_call(
        &mut self,
        callee: Exp,
        args: Vec<Exp>,
    ) -> TackyResult<(TackyVal, CType)> {
        let callee_ft = self.typeof_exp(&callee);
        let pointer_sig = Self::function_signature_from_full(&callee_ft);
        let (ret_ft, param_types, variadic) = pointer_sig
            .as_ref()
            .map(|signature| {
                (
                    signature.return_type.clone(),
                    signature
                        .params
                        .iter()
                        .map(FullType::to_ctype)
                        .collect::<Vec<_>>(),
                    signature.variadic,
                )
            })
            .unwrap_or((FullType::Scalar(CType::Int), Vec::new(), false));
        let param_full_types: Vec<FullType> = pointer_sig
            .as_ref()
            .map(|signature| signature.params.clone())
            .unwrap_or_default();
        let param_full_types = self.canonical_param_full_types(&param_full_types);
        let param_types = if param_full_types.is_empty() {
            param_types
        } else {
            param_full_types
                .iter()
                .map(FullType::to_ctype)
                .collect::<Vec<_>>()
        };
        let has_prototype = pointer_sig.is_some();
        if has_prototype && !variadic && args.len() != param_types.len() {
            return Err(format!(
                "function pointer called with {} argument(s), but prototype expects {}",
                args.len(),
                param_types.len()
            ));
        }
        if has_prototype && variadic && args.len() < param_types.len() {
            return Err(format!(
                "variadic function pointer called with {} argument(s), but prototype requires at least {}",
                args.len(),
                param_types.len()
            ));
        }
        let (ptr_val, _ptr_type) = self.emit_exp(callee)?;
        let ptr_name = if let TackyVal::Var(ref n) = ptr_val {
            n.clone()
        } else {
            let tmp = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Copy {
                src: ptr_val,
                dst: tmp.clone(),
            });
            match tmp {
                TackyVal::Var(n) => n,
                _ => return Err("indirect call callee did not lower to a pointer".to_string()),
            }
        };

        let mut tacky_args = Vec::new();
        let stack_arg_indices = std::collections::HashSet::new();
        let mut memory_arg_blocks = Vec::new();
        let mut struct_arg_groups: Vec<(usize, usize, Vec<bool>)> = Vec::new();
        let mut fixed_flat_arg_count = 0usize;
        for (i, arg) in args.into_iter().enumerate() {
            let arg_for_type = arg.clone();
            let (val, val_type) = self.emit_exp(arg)?;
            let val_ft = self.val_full_type(&val);
            let expected = param_full_types
                .get(i)
                .map(|ft| self.storage_ctype_for_full(ft))
                .or_else(|| param_types.get(i).copied())
                .unwrap_or_else(|| val_type.promote());
            if let Some(expected_ft) = param_full_types.get(i) {
                self.assert_assignable_exp_full_type(
                    expected_ft,
                    &val_ft,
                    &arg_for_type,
                    "function pointer call",
                )?;
            }

            let source_ft = self.typeof_exp(&arg_for_type);
            let memory_vector_ft = param_full_types
                .get(i)
                .filter(|ft| self.vector_requires_memory_abi(ft))
                .or_else(|| self.vector_requires_memory_abi(&val_ft).then_some(&val_ft))
                .or_else(|| {
                    self.vector_requires_memory_abi(&source_ft)
                        .then_some(&source_ft)
                });
            if let Some(vector_ft) = memory_vector_ft {
                let arg_idx = tacky_args.len();
                tacky_args.push(self.get_struct_addr(val));
                memory_arg_blocks.push((
                    arg_idx,
                    vector_ft.byte_size_with(&self.struct_defs),
                    vector_ft.alignment_with(&self.struct_defs).max(16),
                ));
                if i + 1 == param_types.len() {
                    fixed_flat_arg_count = tacky_args.len();
                }
                continue;
            }
            let expected_ft_is_complex = param_full_types.get(i).is_some_and(FullType::is_complex);
            let is_complex_arg =
                expected_ft_is_complex || val_ft.is_complex() || source_ft.is_complex();
            if is_complex_arg {
                let complex_ft = param_full_types
                    .get(i)
                    .filter(|ft| ft.is_complex())
                    .cloned()
                    .unwrap_or_else(|| {
                        if val_ft.is_complex() {
                            val_ft.clone()
                        } else {
                            source_ft.clone()
                        }
                    });
                let FullType::Vector { elem, .. } = complex_ft.clone() else {
                    return Err("internal error: expected complex vector type".to_string());
                };
                let elem_type = elem.to_ctype();
                let elem_size = elem.byte_size_with(&self.struct_defs);
                let group_start = tacky_args.len();
                let real = self.emit_complex_component_value(
                    val.clone(),
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    0,
                )?;
                let imag = self.emit_complex_component_value(
                    val,
                    val_ft.clone(),
                    elem_type,
                    elem_size,
                    1,
                )?;
                tacky_args.push(real);
                tacky_args.push(imag);
                let lane_is_float = elem_type.is_floating();
                struct_arg_groups.push((group_start, 2, vec![lane_is_float, lane_is_float]));
                if i + 1 == param_types.len() {
                    fixed_flat_arg_count = tacky_args.len();
                }
                continue;
            }
            let is_struct_arg =
                source_ft.is_struct() || val_ft.is_struct() || val_type == CType::Struct;
            if is_struct_arg || expected == CType::Struct {
                let tag = match &val_ft {
                    FullType::Struct(t) => t.clone(),
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => {
                            tacky_args.push(val);
                            if i + 1 == param_types.len() {
                                fixed_flat_arg_count = tacky_args.len();
                            }
                            continue;
                        }
                    },
                    _ => {
                        tacky_args.push(val);
                        if i + 1 == param_types.len() {
                            fixed_flat_arg_count = tacky_args.len();
                        }
                        continue;
                    }
                };

                if let Some(def) = self.struct_defs.get(&tag).cloned() {
                    let classes = def.classify_with(&self.struct_defs);
                    let is_variadic_extra = variadic && i >= param_types.len();
                    if is_variadic_extra || (classes.len() == 1 && classes[0] == ParamClass::Memory)
                    {
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let arg_idx = tacky_args.len();
                        tacky_args.push(struct_addr);
                        memory_arg_blocks.push((arg_idx, def.size, def.alignment));
                    } else {
                        let struct_var_name = if val_ft.is_struct() {
                            if let TackyVal::Var(ref n) = val {
                                Some(n.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        let struct_addr = self.fresh_tmp(CType::Pointer);
                        if val_ft.is_struct() {
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Copy {
                                src: val,
                                dst: struct_addr.clone(),
                            });
                        }
                        let group_start = tacky_args.len();
                        let is_sse_vec: Vec<bool> =
                            classes.iter().map(|c| *c == ParamClass::Sse).collect();
                        if !classes.is_empty() {
                            struct_arg_groups.push((group_start, classes.len(), is_sse_vec));
                        }
                        for (eb_idx, class) in classes.iter().enumerate() {
                            let eb_offset = (eb_idx * 8) as i64;
                            match class {
                                ParamClass::Sse => {
                                    let tmp = self.fresh_tmp(CType::Double);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                ParamClass::Integer => {
                                    let tmp = self.fresh_tmp(CType::Long);
                                    if let Some(ref sname) = struct_var_name {
                                        self.emit(TackyInstr::CopyFromOffset {
                                            src_name: sname.clone(),
                                            offset: eb_offset,
                                            dst: tmp.clone(),
                                        });
                                    } else {
                                        let ptr = self.fresh_tmp(CType::Pointer);
                                        if eb_offset > 0 {
                                            self.emit(TackyInstr::Binary {
                                                op: TackyBinaryOp::Add,
                                                left: struct_addr.clone(),
                                                right: TackyVal::Constant(eb_offset),
                                                dst: ptr.clone(),
                                            });
                                        } else {
                                            self.emit(TackyInstr::Copy {
                                                src: struct_addr.clone(),
                                                dst: ptr.clone(),
                                            });
                                        }
                                        self.emit(TackyInstr::Load {
                                            src_ptr: ptr,
                                            dst: tmp.clone(),
                                        });
                                    }
                                    tacky_args.push(tmp);
                                }
                                _ => tacky_args.push(TackyVal::Constant(0)),
                            }
                        }
                    }
                } else {
                    tacky_args.push(self.convert_to(val, val_type, expected));
                }
            } else {
                tacky_args.push(self.convert_to(val, val_type, expected));
            }

            if i + 1 == param_types.len() {
                fixed_flat_arg_count = tacky_args.len();
            }
        }
        if param_types.is_empty() {
            fixed_flat_arg_count = 0;
        }
        let ret_type = ret_ft.to_ctype();
        let uses_hidden_ptr = self.return_requires_hidden_pointer(&ret_ft);
        if uses_hidden_ptr {
            let dst = self.fresh_tmp_full(&ret_ft);
            if let FullType::Struct(ref tag) = ret_ft {
                if let TackyVal::Var(ref dst_name) = dst {
                    if let Some(def) = self.struct_defs.get(tag) {
                        self.array_sizes.insert(dst_name.clone(), def.size);
                    }
                }
            } else if ret_ft.is_complex() {
                if let TackyVal::Var(ref dst_name) = dst {
                    self.array_sizes
                        .insert(dst_name.clone(), ret_ft.byte_size_with(&self.struct_defs));
                }
            }
            let ret_addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: dst.clone(),
                dst: ret_addr.clone(),
            });
            tacky_args.insert(0, ret_addr);
            let shifted_stack = stack_arg_indices.iter().map(|&i| i + 1).collect();
            let shifted_memory_blocks: Vec<(usize, usize, usize)> = memory_arg_blocks
                .iter()
                .map(|(index, size, align)| (index + 1, *size, *align))
                .collect();
            let shifted_groups: Vec<(usize, usize, Vec<bool>)> = struct_arg_groups
                .iter()
                .map(|(start, count, classes)| (start + 1, *count, classes.clone()))
                .collect();
            self.emit(TackyInstr::FunCall {
                name: ptr_name,
                fixed_flat_arg_count: fixed_flat_arg_count + 1,
                args: tacky_args,
                dst: dst.clone(),
                stack_arg_indices: shifted_stack,
                memory_arg_blocks: shifted_memory_blocks,
                struct_arg_groups: shifted_groups,
                variadic,
                hidden_return: true,
                indirect: true,
            });
            return Ok((dst, ret_type));
        }

        let dst = self.fresh_tmp_full(&ret_ft);
        if let FullType::Struct(ref tag) = ret_ft {
            if let TackyVal::Var(ref dst_name) = dst {
                if let Some(def) = self.struct_defs.get(tag) {
                    let classes = def.classify_with(&self.struct_defs);
                    let alloc_size = std::cmp::max(def.size, classes.len() * 8);
                    self.array_sizes.insert(dst_name.clone(), alloc_size);
                }
            }
        } else if ret_ft.is_complex() {
            if let TackyVal::Var(ref dst_name) = dst {
                self.array_sizes
                    .insert(dst_name.clone(), ret_ft.byte_size_with(&self.struct_defs));
            }
        }
        self.emit(TackyInstr::FunCall {
            name: ptr_name,
            fixed_flat_arg_count,
            args: tacky_args,
            dst: dst.clone(),
            stack_arg_indices,
            memory_arg_blocks,
            struct_arg_groups,
            variadic,
            hidden_return: false,
            indirect: true,
        });
        Ok((dst, ret_type))
    }

    pub(super) fn emit_addr_of(&mut self, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
            return self.emit_exp(*ptr_exp);
        }

        if let Exp::Unary(op @ (UnaryOp::RealPart | UnaryOp::ImagPart), component_inner) = inner {
            return self.emit_complex_lane_address(op, *component_inner);
        }

        if let Exp::StringLiteral(s) = inner {
            let label = self.make_string_constant(&s);
            let str_size = c_string_byte_len(&s) + 1;
            let str_ft = FullType::Array {
                elem: Box::new(FullType::Scalar(CType::Char)),
                size: str_size,
            };
            let addr_ft = FullType::Pointer(Box::new(str_ft));
            let dst = self.fresh_tmp_full(&addr_ft);
            self.emit(TackyInstr::GetAddress {
                src: TackyVal::Var(label),
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }

        if matches!(inner, Exp::Dot(_, _) | Exp::Arrow(_, _)) {
            let pointee_ft = self.typeof_exp(&inner);
            let addr = self.emit_dot_address(&inner)?;
            let addr_ft = FullType::Pointer(Box::new(pointee_ft));
            let dst = self.fresh_tmp_full(&addr_ft);
            self.emit(TackyInstr::Copy {
                src: addr,
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }

        if let Exp::Subscript(first, second) = inner {
            let (ptr, _elem_type, _elem_ft) = self.emit_subscript_addr(*first, *second)?;
            return Ok((ptr, CType::Pointer));
        }

        if let Exp::Cast(target_type, Some(ft), boxed) = inner.clone() {
            if ft.is_scalar() && matches!(boxed.as_ref(), Exp::ArrayInit(_)) {
                let tmp_name = self.fresh_var_name();
                self.register_var(&tmp_name, ft.clone());
                let (value, value_type) =
                    self.emit_compound_literal_cast(target_type, Some(ft.clone()), *boxed)?;
                let converted = self.convert_to(value, value_type, target_type);
                self.emit(TackyInstr::Copy {
                    src: converted,
                    dst: TackyVal::Var(tmp_name.clone()),
                });
                let dst = self.fresh_tmp_full(&FullType::Pointer(Box::new(ft)));
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(tmp_name),
                    dst: dst.clone(),
                });
                return Ok((dst, CType::Pointer));
            }
        }

        let is_aggregate_compound_literal = matches!(
            &inner,
            Exp::Cast(_, Some(ft), boxed) if (ft.is_struct() || ft.is_vector()) && matches!(boxed.as_ref(), Exp::ArrayInit(_))
        );
        if is_aggregate_compound_literal {
            let Exp::Cast(target_type, Some(ft), boxed) = inner else {
                return Err("internal error: expected aggregate compound literal".to_string());
            };
            let pointee_ft = ft.clone();
            let (var, _) = self.emit_compound_literal_cast(target_type, Some(ft), *boxed)?;
            let dst = self.fresh_tmp_full(&FullType::Pointer(Box::new(pointee_ft)));
            self.emit(TackyInstr::GetAddress {
                src: var,
                dst: dst.clone(),
            });
            return Ok((dst, CType::Pointer));
        }

        if let Exp::Var(ref name) = inner {
            if self.function_symbols.contains(name) {
                self.emit_nested_capture_updates(name);
                let dst = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(name.clone()),
                    dst: dst.clone(),
                });
                self.extern_vars.push(name.clone());
                return Ok((dst, CType::Pointer));
            }
        }

        let pointee_type = self.lvalue_type(&inner);
        let var = self.emit_lvalue(inner)?;
        let var_ft = self.val_full_type(&var);
        let addr_ft = FullType::Pointer(Box::new(var_ft));
        let dst = self.fresh_tmp_full(&addr_ft);
        if let TackyVal::Var(ref dst_name) = dst {
            let info = match &var {
                TackyVal::Var(vname) => {
                    if let Some(&(base, depth)) = self.ptr_info.get(vname) {
                        (base, depth + 1)
                    } else {
                        (pointee_type, 1)
                    }
                }
                _ => (pointee_type, 1),
            };
            self.ptr_info.insert(dst_name.clone(), info);
        }
        self.emit(TackyInstr::GetAddress {
            src: var,
            dst: dst.clone(),
        });
        Ok((dst, CType::Pointer))
    }

    pub(super) fn emit_deref(&mut self, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        let (ptr, _) = self.emit_exp(inner)?;
        let ptr_full = self.val_full_type(&ptr);
        if let FullType::Pointer(ref inner_ft) = ptr_full {
            if matches!(inner_ft.as_ref(), FullType::Function { .. }) {
                return Ok((ptr, CType::Pointer));
            }
            if inner_ft.is_array() {
                let decayed = inner_ft.decay();
                let result = self.fresh_tmp_full(&decayed);
                self.emit(TackyInstr::Copy {
                    src: ptr,
                    dst: result.clone(),
                });
                return Ok((result, decayed.to_ctype()));
            }
            if inner_ft.is_struct() {
                let ptr_ft = FullType::Pointer(Box::new(inner_ft.as_ref().clone()));
                let result = self.fresh_tmp_full(&ptr_ft);
                self.emit(TackyInstr::Copy {
                    src: ptr,
                    dst: result.clone(),
                });
                return Ok((result, CType::Pointer));
            }
            if inner_ft.is_vector() {
                let result = self.fresh_tmp_full(inner_ft);
                if let TackyVal::Var(ref dst_name) = result {
                    self.emit_struct_copy_to(
                        ptr,
                        dst_name,
                        inner_ft.byte_size_with(&self.struct_defs),
                    );
                }
                return Ok((result, inner_ft.to_ctype()));
            }
        }
        let pointee_type = if let TackyVal::Var(ref name) = ptr {
            self.deref_type(name)
        } else {
            CType::Int
        };
        let pointee_full = if let FullType::Pointer(ref inner_ft) = ptr_full {
            inner_ft.as_ref().clone()
        } else {
            FullType::Scalar(pointee_type)
        };
        let dst = self.fresh_tmp_full(&pointee_full);
        self.emit(TackyInstr::Load {
            src_ptr: ptr.clone(),
            dst: dst.clone(),
        });
        if pointee_type == CType::Pointer {
            if let TackyVal::Var(ref ptr_name) = ptr {
                if let Some(info) = self.deref_info(ptr_name) {
                    if let TackyVal::Var(ref dst_name) = dst {
                        self.ptr_info.insert(dst_name.clone(), info);
                    }
                }
            }
        }
        Ok((dst, pointee_type))
    }

    pub(super) fn emit_subscript(&mut self, arr: Exp, idx: Exp) -> TackyResult<(TackyVal, CType)> {
        if let FullType::Scalar(elem_type) = self.typeof_exp(&arr) {
            if let Some(index) = eval_static_integer_constant_exp_with_context(
                &idx,
                &self.struct_defs,
                &self.full_types,
            )
            .map(|constant| constant.value)
            {
                let (arr_val, arr_type) = self.emit_exp(arr)?;
                if index == 0 {
                    return Ok((self.convert_to(arr_val, arr_type, elem_type), elem_type));
                }
                let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                return Ok((zero, elem_type));
            }
        }

        let (ptr, _elem_type, elem_full) = self.emit_subscript_addr(arr, idx)?;

        if elem_full.is_array() {
            let decayed = elem_full.decay();
            let decayed_ptr = self.fresh_tmp_full(&decayed);
            self.emit(TackyInstr::Copy {
                src: ptr,
                dst: decayed_ptr.clone(),
            });
            return Ok((decayed_ptr, decayed.to_ctype()));
        }

        if elem_full.is_struct() {
            let ptr_ft = FullType::Pointer(Box::new(elem_full));
            let result = self.fresh_tmp_full(&ptr_ft);
            self.emit(TackyInstr::Copy {
                src: ptr,
                dst: result.clone(),
            });
            return Ok((result, CType::Pointer));
        }

        if elem_full.is_vector() {
            let result = self.fresh_tmp_full(&elem_full);
            if let TackyVal::Var(ref name) = result {
                self.emit_struct_copy_to(ptr, name, elem_full.byte_size_with(&self.struct_defs));
            }
            return Ok((result, elem_full.to_ctype()));
        }

        let elem_ctype = elem_full.to_ctype();
        let result = self.fresh_tmp_full(&elem_full);
        self.emit(TackyInstr::Load {
            src_ptr: ptr,
            dst: result.clone(),
        });
        Ok((result, elem_ctype))
    }

    pub(super) fn emit_conditional(
        &mut self,
        cond: Exp,
        then_exp: Exp,
        else_exp: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let cond_val = self.emit_condition_value(cond)?;
        let else_label = self.fresh_label("cond_else");
        let end_label = self.fresh_label("cond_end");
        self.emit(TackyInstr::JumpIfZero(cond_val, else_label.clone()));
        let (then_val, then_type) = self.emit_exp(then_exp)?;

        if then_type == CType::Void {
            self.emit(TackyInstr::Jump(end_label.clone()));
            self.emit(TackyInstr::Label(else_label));
            self.emit_exp(else_exp)?;
            self.emit(TackyInstr::Label(end_label));
            return Ok((TackyVal::Constant(0), CType::Void));
        }

        if then_type == CType::Struct {
            let then_ft = self.val_full_type(&then_val);
            let tag = match &then_ft {
                FullType::Struct(t) => t.clone(),
                FullType::Pointer(inner) => match inner.as_ref() {
                    FullType::Struct(t) => t.clone(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            let struct_size = self.struct_defs.get(&tag).map(|d| d.size).unwrap_or(0);
            let result = self.fresh_tmp_full(&FullType::Struct(tag.clone()));
            if let TackyVal::Var(ref rn) = result {
                self.array_sizes.insert(rn.clone(), struct_size);
            }
            let then_addr = if then_ft.is_struct() {
                if let TackyVal::Var(ref n) = then_val {
                    if self.array_sizes.contains_key(n) {
                        let a = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: then_val,
                            dst: a.clone(),
                        });
                        a
                    } else {
                        then_val
                    }
                } else {
                    then_val
                }
            } else {
                then_val
            };
            if let TackyVal::Var(ref rn) = result {
                self.emit_struct_copy_to(then_addr, rn, struct_size);
            }
            self.emit(TackyInstr::Jump(end_label.clone()));
            self.emit(TackyInstr::Label(else_label));
            let (else_val, _) = self.emit_exp(else_exp)?;
            let else_ft = self.val_full_type(&else_val);
            let else_addr = if else_ft.is_struct() {
                if let TackyVal::Var(ref n) = else_val {
                    if self.array_sizes.contains_key(n) {
                        let a = self.fresh_tmp(CType::Pointer);
                        self.emit(TackyInstr::GetAddress {
                            src: else_val,
                            dst: a.clone(),
                        });
                        a
                    } else {
                        else_val
                    }
                } else {
                    else_val
                }
            } else {
                else_val
            };
            if let TackyVal::Var(ref rn) = result {
                self.emit_struct_copy_to(else_addr, rn, struct_size);
            }
            self.emit(TackyInstr::Label(end_label));
            return Ok((result, CType::Struct));
        }

        let then_ft = self.val_full_type(&then_val);
        if then_type == CType::Pointer && matches!(then_ft, FullType::Pointer(_)) {
            let else_exp_for_type = else_exp.clone();
            let result = self.fresh_tmp_full(&then_ft);
            self.emit(TackyInstr::Copy {
                src: then_val,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Jump(end_label.clone()));
            self.emit(TackyInstr::Label(else_label));
            let (else_val, else_type) = self.emit_exp(else_exp)?;
            if else_type != CType::Pointer {
                if Self::is_null_pointer_constant(&else_exp_for_type) {
                    self.emit(TackyInstr::Copy {
                        src: TackyVal::Constant(0),
                        dst: result.clone(),
                    });
                    self.emit(TackyInstr::Label(end_label));
                    return Ok((result, CType::Pointer));
                }
                return Err(format!(
                    "conditional pointer arm has incompatible type {:?}",
                    else_type
                ));
            }
            self.emit(TackyInstr::Copy {
                src: else_val,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Label(end_label));
            return Ok((result, CType::Pointer));
        }

        let then_tmp = self.fresh_tmp(then_type);
        self.emit(TackyInstr::Copy {
            src: then_val,
            dst: then_tmp.clone(),
        });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(else_label));
        let (else_val, else_type) = self.emit_exp(else_exp)?;
        let common = CType::common(then_type, else_type);
        let result = self.fresh_tmp(common);
        let else_conv = self.convert_to(else_val, else_type, common);
        self.emit(TackyInstr::Copy {
            src: else_conv,
            dst: result.clone(),
        });
        let end2_label = self.fresh_label("cond_end2");
        self.emit(TackyInstr::Jump(end2_label.clone()));
        self.emit(TackyInstr::Label(end_label));
        let then_conv = self.convert_to(then_tmp, then_type, common);
        self.emit(TackyInstr::Copy {
            src: then_conv,
            dst: result.clone(),
        });
        self.emit(TackyInstr::Label(end2_label));
        Ok((result, common))
    }

    pub(super) fn emit_condition_value(&mut self, condition: Exp) -> TackyResult<TackyVal> {
        let (cond_val, cond_type) = self.emit_exp(condition)?;
        if cond_type == CType::Void {
            return Err("void value not ignored as it ought to be".to_string());
        }
        Ok(cond_val)
    }

    pub(super) fn emit_complex_lane_value(
        &mut self,
        op: UnaryOp,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let (src, src_type) = self.emit_exp(inner)?;
        let src_ft = self.val_full_type(&src);
        if src_ft.is_complex() {
            let FullType::Vector { elem, .. } = src_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let value =
                self.emit_complex_component_value(src, src_ft, elem_type, elem_size, lane)?;
            Ok((value, elem_type))
        } else if lane == 0 {
            Ok((src, src_type))
        } else {
            Ok((
                self.convert_to(TackyVal::Constant(0), CType::Int, src_type),
                src_type,
            ))
        }
    }

    pub(super) fn emit_complex_lane_assignment(
        &mut self,
        op: UnaryOp,
        inner: Exp,
        right: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let lhs_ft = self.typeof_exp(&inner);
        if !lhs_ft.is_complex() {
            if lane == 0 {
                return self.emit_exp(Exp::Assign(Box::new(inner), Box::new(right)));
            }
            return Err("assignment to scalar imaginary component".to_string());
        }
        let FullType::Vector { elem, .. } = lhs_ft.clone() else {
            return Err("internal error: expected complex vector type".to_string());
        };
        let elem_type = elem.to_ctype();
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let right_for_type = right.clone();
        let (rhs, rhs_type) = self.emit_exp(right)?;
        let rhs_ft = self.val_full_type(&rhs);
        self.assert_assignable_exp_full_type(&elem, &rhs_ft, &right_for_type, "assignment")?;
        let rhs_conv = self.convert_to(rhs, rhs_type, elem_type);
        let offset = (lane * elem_size) as i64;

        if let Exp::Var(name) = inner {
            self.emit(TackyInstr::CopyToOffset {
                src: rhs_conv.clone(),
                dst_name: name,
                offset,
            });
            return Ok((rhs_conv, elem_type));
        }

        let Some((mut ptr, _, _)) = self.scalar_lvalue_address(inner)? else {
            return Err("Expression is not a simple lvalue".to_string());
        };
        if offset != 0 {
            let lane_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: ptr,
                right: TackyVal::Constant(offset),
                dst: lane_ptr.clone(),
            });
            ptr = lane_ptr;
        }
        self.emit(TackyInstr::Store {
            src: rhs_conv.clone(),
            dst_ptr: ptr,
        });
        Ok((rhs_conv, elem_type))
    }

    pub(super) fn emit_complex_lane_compound_assignment(
        &mut self,
        component_op: UnaryOp,
        op: BinaryOp,
        inner: Exp,
        right: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match component_op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let lhs_ft = self.typeof_exp(&inner);
        if !lhs_ft.is_complex() {
            if lane == 0 {
                return self.emit_exp(Exp::CompoundAssign(op, Box::new(inner), Box::new(right)));
            }
            return Err("compound assignment to scalar imaginary component".to_string());
        }
        let FullType::Vector { elem, .. } = lhs_ft.clone() else {
            return Err("internal error: expected complex vector type".to_string());
        };
        let elem_type = elem.to_ctype();
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let offset = (lane * elem_size) as i64;

        let (current, dst_ptr, dst_name) = if let Exp::Var(name) = inner {
            let current = self.fresh_tmp(elem_type);
            self.emit(TackyInstr::CopyFromOffset {
                src_name: name.clone(),
                offset,
                dst: current.clone(),
            });
            (current, None, Some(name))
        } else {
            let Some((mut ptr, _, _)) = self.scalar_lvalue_address(inner)? else {
                return Err("Expression is not a simple lvalue".to_string());
            };
            if offset != 0 {
                let lane_ptr = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: ptr,
                    right: TackyVal::Constant(offset),
                    dst: lane_ptr.clone(),
                });
                ptr = lane_ptr;
            }
            let current = self.fresh_tmp(elem_type);
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            (current, Some(ptr), None)
        };

        let (rhs, rhs_type) = self.emit_exp(right)?;
        let common = CType::common(elem_type, rhs_type);
        let lhs_conv = self.convert_to(current, elem_type, common);
        let rhs_conv = self.convert_to(rhs, rhs_type, common);
        let result = self.fresh_tmp(common);
        let tacky_op = Self::convert_binop(op)?;
        self.emit(TackyInstr::Binary {
            op: tacky_op,
            left: lhs_conv,
            right: rhs_conv,
            dst: result.clone(),
        });
        let result_conv = self.convert_to(result, common, elem_type);
        if let Some(ptr) = dst_ptr {
            self.emit(TackyInstr::Store {
                src: result_conv.clone(),
                dst_ptr: ptr,
            });
        } else if let Some(name) = dst_name {
            self.emit(TackyInstr::CopyToOffset {
                src: result_conv.clone(),
                dst_name: name,
                offset,
            });
        }
        Ok((result_conv, elem_type))
    }

    pub(super) fn emit_complex_lane_address(
        &mut self,
        op: UnaryOp,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let lane = match op {
            UnaryOp::RealPart => 0,
            UnaryOp::ImagPart => 1,
            _ => return Err("internal error: expected complex component operator".to_string()),
        };
        let inner_ft = self.typeof_exp(&inner);
        if !inner_ft.is_complex() {
            if lane == 0 {
                return self.emit_addr_of(inner);
            }
            return Err("cannot take address of scalar imaginary component".to_string());
        }
        let FullType::Vector { elem, .. } = inner_ft else {
            return Err("internal error: expected complex vector type".to_string());
        };
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let offset = (lane * elem_size) as i64;
        let ptr_ft = FullType::Pointer(elem);
        let mut ptr = match inner {
            Exp::Var(name) => {
                let base = self.fresh_tmp_full(&ptr_ft);
                self.emit(TackyInstr::GetAddress {
                    src: TackyVal::Var(name),
                    dst: base.clone(),
                });
                base
            }
            other => {
                let Some((base, _, _)) = self.scalar_lvalue_address(other)? else {
                    return Err("Expression is not a simple lvalue".to_string());
                };
                let typed_base = self.fresh_tmp_full(&ptr_ft);
                self.emit(TackyInstr::Copy {
                    src: base,
                    dst: typed_base.clone(),
                });
                typed_base
            }
        };
        if offset != 0 {
            let lane_ptr = self.fresh_tmp_full(&ptr_ft);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: ptr,
                right: TackyVal::Constant(offset),
                dst: lane_ptr.clone(),
            });
            ptr = lane_ptr;
        }
        Ok((ptr, CType::Pointer))
    }

    pub(super) fn emit_complex_value_parts(
        &mut self,
        target_ft: &FullType,
        value: TackyVal,
        value_type: CType,
        value_ft: FullType,
    ) -> TackyResult<(TackyVal, TackyVal, CType, usize)> {
        let FullType::Vector { elem, .. } = target_ft else {
            return Err("internal error: expected complex target type".to_string());
        };
        let elem_type = elem.to_ctype();
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let real = if value_ft.is_complex() {
            self.emit_complex_component_value(
                value.clone(),
                value_ft.clone(),
                elem_type,
                elem_size,
                0,
            )?
        } else {
            self.convert_to(value.clone(), value_type, elem_type)
        };
        let imag = if value_ft.is_complex() {
            self.emit_complex_component_value(value, value_ft, elem_type, elem_size, 1)?
        } else {
            self.convert_to(TackyVal::Constant(0), CType::Int, elem_type)
        };
        Ok((real, imag, elem_type, elem_size))
    }

    pub(super) fn emit_complex_value_to_offset(
        &mut self,
        dst_name: &str,
        target_ft: &FullType,
        value: TackyVal,
        value_type: CType,
        value_ft: FullType,
        offset: i64,
    ) -> TackyResult<()> {
        let (real, imag, _elem_type, elem_size) =
            self.emit_complex_value_parts(target_ft, value, value_type, value_ft)?;
        self.emit(TackyInstr::CopyToOffset {
            src: real,
            dst_name: dst_name.to_string(),
            offset,
        });
        self.emit(TackyInstr::CopyToOffset {
            src: imag,
            dst_name: dst_name.to_string(),
            offset: offset + elem_size as i64,
        });
        Ok(())
    }

    pub(super) fn emit_complex_value_to_ptr(
        &mut self,
        dst_ptr: TackyVal,
        target_ft: &FullType,
        value: TackyVal,
        value_type: CType,
        value_ft: FullType,
    ) -> TackyResult<()> {
        let (real, imag, _elem_type, elem_size) =
            self.emit_complex_value_parts(target_ft, value, value_type, value_ft)?;
        self.emit(TackyInstr::Store {
            src: real,
            dst_ptr: dst_ptr.clone(),
        });
        let imag_ptr = self.fresh_tmp(CType::Pointer);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: dst_ptr,
            right: TackyVal::Constant(elem_size as i64),
            dst: imag_ptr.clone(),
        });
        self.emit(TackyInstr::Store {
            src: imag,
            dst_ptr: imag_ptr,
        });
        Ok(())
    }

    #[allow(dead_code)]
    pub(super) fn emit_scalar_unary(
        &mut self,
        op: UnaryOp,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let (src, src_type) = self.emit_exp(inner)?;
        if src_type.is_floating() && matches!(op, UnaryOp::Negate) {
            let dst = self.fresh_tmp(src_type);
            self.emit(TackyInstr::Unary {
                op: TackyUnaryOp::Negate,
                src,
                dst: dst.clone(),
            });
            return Ok((dst, src_type));
        }

        let promoted = src_type.promote();
        let src_conv = self.convert_to(src, src_type, promoted);
        let dst = self.fresh_tmp(promoted);
        let tacky_op = match op {
            UnaryOp::Negate => TackyUnaryOp::Negate,
            UnaryOp::Complement => TackyUnaryOp::Complement,
            _ => return Err(format!("invalid scalar unary operator: {:?}", op)),
        };
        self.emit(TackyInstr::Unary {
            op: tacky_op,
            src: src_conv,
            dst: dst.clone(),
        });
        Ok((dst, promoted))
    }

    pub(super) fn emit_unary(&mut self, op: UnaryOp, inner: Exp) -> TackyResult<(TackyVal, CType)> {
        let (src, src_type) = self.emit_exp(inner)?;
        let value_ft = self.val_full_type(&src);
        if value_ft.is_complex() {
            let FullType::Vector { elem, .. } = value_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&value_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("complex unary result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, value_ft.byte_size_with(&self.struct_defs));
            match op {
                UnaryOp::Negate => {
                    let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                    let real = self.emit_complex_component_value(
                        src.clone(),
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        0,
                    )?;
                    let imag = self.emit_complex_component_value(
                        src,
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        1,
                    )?;
                    let neg_real = self.fresh_tmp(elem_type);
                    let neg_imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: zero.clone(),
                        right: real,
                        dst: neg_real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: zero,
                        right: imag,
                        dst: neg_imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: neg_real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: neg_imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, value_ft.to_ctype()));
                }
                UnaryOp::Complement => {
                    let real = self.emit_complex_component_value(
                        src.clone(),
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        0,
                    )?;
                    let imag = self.emit_complex_component_value(
                        src,
                        value_ft.clone(),
                        elem_type,
                        elem_size,
                        1,
                    )?;
                    let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                    let neg_imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: zero,
                        right: imag,
                        dst: neg_imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: neg_imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, value_ft.to_ctype()));
                }
                _ => return Err("invalid complex unary operator".to_string()),
            }
        }
        if !value_ft.is_vector() {
            if src_type.is_floating() && matches!(op, UnaryOp::Negate) {
                let dst = self.fresh_tmp(src_type);
                self.emit(TackyInstr::Unary {
                    op: TackyUnaryOp::Negate,
                    src,
                    dst: dst.clone(),
                });
                return Ok((dst, src_type));
            }

            let promoted = src_type.promote();
            let src_conv = self.convert_to(src, src_type, promoted);
            let dst = self.fresh_tmp(promoted);
            let tacky_op = match op {
                UnaryOp::Negate => TackyUnaryOp::Negate,
                UnaryOp::Complement => TackyUnaryOp::Complement,
                _ => return Err(format!("invalid scalar unary operator: {:?}", op)),
            };
            self.emit(TackyInstr::Unary {
                op: tacky_op,
                src: src_conv,
                dst: dst.clone(),
            });
            return Ok((dst, promoted));
        }

        let FullType::Vector { elem, lanes, .. } = value_ft.clone() else {
            return Err("internal error: expected vector type".to_string());
        };
        let elem_type = elem.to_ctype();
        let calc_type = match elem_type {
            CType::UChar | CType::UShort => CType::UInt,
            CType::Char | CType::SChar | CType::Short | CType::Bool => CType::Int,
            _ => elem_type,
        };
        let elem_size = elem.byte_size_with(&self.struct_defs);
        let result = self.fresh_tmp_full(&value_ft);
        let TackyVal::Var(result_name) = result.clone() else {
            return Err("vector unary result must be addressable".to_string());
        };
        self.zero_init_local(&result_name, value_ft.byte_size_with(&self.struct_defs));
        let tacky_op = match op {
            UnaryOp::Negate => TackyUnaryOp::Negate,
            UnaryOp::Complement => TackyUnaryOp::Complement,
            _ => return Err(format!("invalid vector unary operator: {:?}", op)),
        };
        for lane in 0..lanes {
            let lane_value = self.emit_vector_lane_value(
                src.clone(),
                value_ft.clone(),
                elem_type,
                elem_size,
                lane,
            )?;
            let lane_value = self.convert_to(lane_value, elem_type, calc_type);
            let dst = self.fresh_tmp(calc_type);
            self.emit(TackyInstr::Unary {
                op: tacky_op.clone(),
                src: lane_value,
                dst: dst.clone(),
            });
            let stored = self.convert_to(dst, calc_type, elem_type);
            self.emit(TackyInstr::CopyToOffset {
                src: stored,
                dst_name: result_name.clone(),
                offset: (lane * elem_size) as i64,
            });
        }
        Ok((result, value_ft.to_ctype()))
    }

    pub(super) fn lvalue_type(&self, exp: &Exp) -> CType {
        match exp {
            Exp::Var(name) => self
                .var_types
                .get(name)
                .copied()
                .or_else(|| self.symbol_types.get(name).copied())
                .unwrap_or(CType::Int),
            Exp::Unary(UnaryOp::Deref, inner) => {
                if let Exp::Var(name) = inner.as_ref() {
                    self.deref_type(name)
                } else {
                    CType::Int
                }
            }
            Exp::Subscript(arr, _) => {
                if let Exp::Var(name) = arr.as_ref() {
                    self.deref_type(name)
                } else {
                    CType::Int
                }
            }
            Exp::Dot(_, _) => self.typeof_exp(exp).to_ctype(),
            Exp::Arrow(_, _) => self.typeof_exp(exp).to_ctype(),
            _ => CType::Int,
        }
    }

    pub(super) fn emit_lvalue(&self, exp: Exp) -> TackyResult<TackyVal> {
        match exp {
            Exp::Var(name) => Ok(TackyVal::Var(name)),
            _ => Err("Expression is not a simple lvalue".to_string()),
        }
    }

    pub(super) fn scalar_lvalue_address(
        &mut self,
        exp: Exp,
    ) -> TackyResult<Option<(TackyVal, CType, FullType)>> {
        match exp {
            Exp::Subscript(arr, idx) => {
                let (ptr, elem_type, elem_ft) = self.emit_subscript_addr(*arr, *idx)?;
                Ok(Some((ptr, elem_type, elem_ft)))
            }
            Exp::Unary(UnaryOp::Deref, ptr_exp) => {
                let (ptr, _) = self.emit_exp(*ptr_exp)?;
                let ptr_ft = self.val_full_type(&ptr);
                let elem_ft = match ptr_ft {
                    FullType::Pointer(inner) => *inner,
                    _ => FullType::Scalar(CType::Int),
                };
                Ok(Some((ptr, elem_ft.to_ctype(), elem_ft)))
            }
            Exp::Dot(_, _) | Exp::Arrow(_, _) => {
                let ft = self.typeof_exp(&exp);
                let ptr = self.emit_dot_address(&exp)?;
                Ok(Some((ptr, ft.to_ctype(), ft)))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn emit_inc_dec(
        &mut self,
        op: UnaryOp,
        inner: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let is_pre = matches!(op, UnaryOp::PreIncrement | UnaryOp::PreDecrement);
        let binop = if matches!(op, UnaryOp::PreIncrement | UnaryOp::PostIncrement) {
            TackyBinaryOp::Add
        } else {
            TackyBinaryOp::Sub
        };

        if matches!(inner, Exp::Dot(_, _) | Exp::Arrow(_, _)) {
            let mem = match &inner {
                Exp::Dot(base, member) | Exp::Arrow(base, member) => {
                    let tag = self.dot_inner_tag(base)?;
                    self.struct_member(&tag, member)?
                }
                _ => return Err("internal error: expected dot or arrow expression".to_string()),
            };
            if let Some(width) = mem.bit_width {
                let ptr = self.emit_dot_address(&inner)?;
                let unit = self.fresh_tmp(mem.member_type);
                self.emit(TackyInstr::Load {
                    src_ptr: ptr.clone(),
                    dst: unit.clone(),
                });
                let current = self.extract_bit_field(unit, &mem)?;
                let promoted_type = Self::bit_field_promoted_type(&mem, width);
                let increment = self.convert_to(TackyVal::Constant(1), CType::Int, promoted_type);
                let result = self.fresh_tmp(promoted_type);
                self.emit(TackyInstr::Binary {
                    op: binop,
                    left: current.clone(),
                    right: increment,
                    dst: result.clone(),
                });
                let result_for_store =
                    self.convert_to(result.clone(), promoted_type, mem.member_type);
                let stored = self.store_bit_field_to_ptr(ptr, &mem, result_for_store)?;
                let stored = self.convert_to(stored, mem.member_type, promoted_type);
                if !mem.member_type.is_signed() {
                    self.mark_bit_precision(&stored, width);
                }
                return Ok((if is_pre { stored } else { current }, promoted_type));
            }
        }

        if let Exp::Subscript(arr, idx) = inner {
            let (ptr, pt, pt_ft) = self.emit_subscript_addr(*arr, *idx)?;
            let current = if pt == CType::Pointer {
                self.fresh_tmp_full(&pt_ft)
            } else {
                self.fresh_tmp(pt)
            };
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let increment = if pt == CType::Pointer {
                let elem_size = match &pt_ft {
                    FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                    _ => 1,
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, pt)
            };
            let result = if pt == CType::Pointer {
                self.fresh_tmp_full(&pt_ft)
            } else {
                self.fresh_tmp(pt)
            };
            self.emit(TackyInstr::Binary {
                op: binop,
                left: current.clone(),
                right: increment,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Store {
                src: result.clone(),
                dst_ptr: ptr,
            });
            return Ok((if is_pre { result } else { current }, pt));
        }

        if let Exp::Unary(UnaryOp::Deref, ptr_exp) = inner {
            let (ptr, _) = self.emit_exp(*ptr_exp)?;
            let elem_ft = match self.val_full_type(&ptr) {
                FullType::Pointer(inner) => *inner,
                _ => FullType::Scalar(CType::Int),
            };
            let pt = elem_ft.to_ctype();
            let current = if pt == CType::Pointer {
                self.fresh_tmp_full(&elem_ft)
            } else {
                self.fresh_tmp(pt)
            };
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let increment = if pt == CType::Pointer {
                let elem_size = match &elem_ft {
                    FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                    _ => 1,
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, pt)
            };
            let result = if pt == CType::Pointer {
                self.fresh_tmp_full(&elem_ft)
            } else {
                self.fresh_tmp(pt)
            };
            self.emit(TackyInstr::Binary {
                op: binop,
                left: current.clone(),
                right: increment,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Store {
                src: result.clone(),
                dst_ptr: ptr,
            });
            return Ok((if is_pre { result } else { current }, pt));
        }

        if let Some((ptr, var_type, var_ft)) = self.scalar_lvalue_address(inner.clone())? {
            let current = self.fresh_tmp_full(&var_ft);
            self.emit(TackyInstr::Load {
                src_ptr: ptr.clone(),
                dst: current.clone(),
            });
            let increment = if var_type == CType::Pointer {
                let elem_size = match &var_ft {
                    FullType::Pointer(inner) => inner.byte_size_with(&self.struct_defs) as i64,
                    _ => 1,
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, var_type)
            };
            let result = self.fresh_tmp_full(&var_ft);
            self.emit(TackyInstr::Binary {
                op: binop,
                left: current.clone(),
                right: increment,
                dst: result.clone(),
            });
            self.emit(TackyInstr::Store {
                src: result.clone(),
                dst_ptr: ptr,
            });
            return Ok((if is_pre { result } else { current }, var_type));
        }

        let var_type = self.lvalue_type(&inner);
        let var = self.emit_lvalue(inner)?;
        let var_ft = self.val_full_type(&var);

        if is_pre {
            let increment = if var_type == CType::Pointer {
                let elem_size = if let TackyVal::Var(ref n) = var {
                    self.ptr_elem_size(n)
                } else {
                    1
                };
                TackyVal::Constant(elem_size)
            } else {
                self.convert_to(TackyVal::Constant(1), CType::Int, var_type)
            };
            let dst = if var_type == CType::Pointer {
                self.fresh_tmp_full(&var_ft)
            } else {
                self.fresh_tmp(var_type)
            };
            self.emit(TackyInstr::Binary {
                op: binop,
                left: var.clone(),
                right: increment,
                dst: dst.clone(),
            });
            if var_type == CType::Pointer {
                if let TackyVal::Var(ref vn) = var {
                    if let Some(&info) = self.ptr_info.get(vn) {
                        if let TackyVal::Var(ref dn) = dst {
                            self.ptr_info.insert(dn.clone(), info);
                        }
                    }
                }
            }
            self.emit(TackyInstr::Copy {
                src: dst.clone(),
                dst: var,
            });
            return Ok((dst, var_type));
        }

        let old_val = if var_type == CType::Pointer {
            self.fresh_tmp_full(&var_ft)
        } else {
            self.fresh_tmp(var_type)
        };
        self.emit(TackyInstr::Copy {
            src: var.clone(),
            dst: old_val.clone(),
        });
        let increment = if var_type == CType::Pointer {
            let elem_size = if let TackyVal::Var(ref n) = var {
                self.ptr_elem_size(n)
            } else {
                1
            };
            TackyVal::Constant(elem_size)
        } else {
            self.convert_to(TackyVal::Constant(1), CType::Int, var_type)
        };
        let new_val = self.fresh_tmp(var_type);
        self.emit(TackyInstr::Binary {
            op: binop,
            left: var.clone(),
            right: increment,
            dst: new_val.clone(),
        });
        self.emit(TackyInstr::Copy {
            src: new_val,
            dst: var,
        });
        Ok((old_val, var_type))
    }

    /// Compute the address of a Dot/Arrow lvalue expression
    pub(super) fn emit_dot_address(&mut self, exp: &Exp) -> TackyResult<TackyVal> {
        match exp {
            Exp::Dot(inner, member) => {
                let base_addr = if let Exp::Var(n) = inner.as_ref() {
                    let addr = self.fresh_tmp(CType::Pointer);
                    self.emit(TackyInstr::GetAddress {
                        src: TackyVal::Var(n.clone()),
                        dst: addr.clone(),
                    });
                    addr
                } else if let Exp::Dot(_, _) = inner.as_ref() {
                    self.emit_dot_address(inner)?
                } else if let Exp::Arrow(ptr_exp, mem) = inner.as_ref() {
                    let (ptr, _) = self.emit_exp((**ptr_exp).clone())?;
                    let ptr_ft = self.val_full_type(&ptr);
                    let tag = match &ptr_ft {
                        FullType::Pointer(inner) => match inner.as_ref() {
                            FullType::Struct(t) => t.clone(),
                            _ => {
                                return Err(format!(
                                    "emit_dot_address Arrow: inner is {:?}, expected Struct",
                                    inner
                                ))
                            }
                        },
                        _ => {
                            return Err(format!(
                                "emit_dot_address Arrow: ft is {:?}, expected Pointer",
                                ptr_ft
                            ))
                        }
                    };
                    let m = self.struct_member(&tag, mem)?;
                    let result = self.fresh_tmp(CType::Pointer);
                    if m.offset > 0 {
                        self.emit(TackyInstr::Binary {
                            op: TackyBinaryOp::Add,
                            left: ptr,
                            right: TackyVal::Constant(m.offset as i64),
                            dst: result.clone(),
                        });
                    } else {
                        self.emit(TackyInstr::Copy {
                            src: ptr,
                            dst: result.clone(),
                        });
                    }
                    result
                } else {
                    let (val, _) = self.emit_exp((**inner).clone())?;
                    let val_ft = self.val_full_type(&val);
                    match &val_ft {
                        FullType::Struct(_) => {
                            let addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: addr.clone(),
                            });
                            addr
                        }
                        FullType::Pointer(_) => val,
                        _ => {
                            let addr = self.fresh_tmp(CType::Pointer);
                            self.emit(TackyInstr::GetAddress {
                                src: val,
                                dst: addr.clone(),
                            });
                            addr
                        }
                    }
                };
                // Get the struct tag using typeof_exp
                let tag = self.dot_inner_tag(inner)?;
                let mem = self.struct_member(&tag, member)?;
                let result = self.fresh_tmp(CType::Pointer);
                if mem.offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: base_addr,
                        right: TackyVal::Constant(mem.offset as i64),
                        dst: result.clone(),
                    });
                } else {
                    self.emit(TackyInstr::Copy {
                        src: base_addr,
                        dst: result.clone(),
                    });
                }
                Ok(result)
            }
            Exp::Arrow(inner, member) => {
                let (ptr, _) = self.emit_exp((**inner).clone())?;
                let ptr_ft = self.val_full_type(&ptr);
                // Try to get struct tag from FullType; fall back to looking up ptr_info
                let tag = match &ptr_ft {
                    FullType::Pointer(inner) => match inner.as_ref() {
                        FullType::Struct(t) => t.clone(),
                        _ => {
                            // Fallback: try ptr_info
                            if let TackyVal::Var(ref name) = ptr {
                                if let Some(&(base_t, _)) = self.ptr_info.get(name) {
                                    if base_t == CType::Struct {
                                        // Can't determine tag from ptr_info alone
                                    }
                                }
                            }
                            return Err(format!(
                                "emit_dot_address Arrow: inner is {:?}, expected Struct",
                                inner
                            ));
                        }
                    },
                    _ => {
                        return Err(format!(
                            "emit_dot_address Arrow: ft is {:?}, expected Pointer",
                            ptr_ft
                        ))
                    }
                };
                let mem = self.struct_member(&tag, member)?;
                let result = self.fresh_tmp(CType::Pointer);
                if mem.offset > 0 {
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ptr,
                        right: TackyVal::Constant(mem.offset as i64),
                        dst: result.clone(),
                    });
                } else {
                    self.emit(TackyInstr::Copy {
                        src: ptr,
                        dst: result.clone(),
                    });
                }
                Ok(result)
            }
            _ => Err("emit_dot_address called on non-Dot/Arrow expression".to_string()),
        }
    }

    pub(super) fn dot_inner_tag(&self, exp: &Exp) -> TackyResult<String> {
        let ft = self.typeof_exp(exp);
        match ft {
            FullType::Struct(t) => Ok(t),
            FullType::Pointer(inner) => match *inner {
                FullType::Struct(t) => Ok(t),
                _ => Err(format!("dot_inner_tag: pointer to non-struct: {:?}", inner)),
            },
            _ => Err(format!(
                "dot_inner_tag: non-struct type {:?} for {:?}",
                ft, exp
            )),
        }
    }

    pub(super) fn struct_member(&self, tag: &str, member: &str) -> TackyResult<StructMember> {
        let def = self
            .struct_defs
            .get(tag)
            .cloned()
            .ok_or_else(|| format!("Undefined struct: {}", tag))?;
        def.find_member(member)
            .cloned()
            .ok_or_else(|| format!("No member '{}' in struct {}", member, tag))
    }

    pub(super) fn bit_mask(width: u8) -> i64 {
        if width >= 63 {
            i64::MAX
        } else {
            (1_i64 << width) - 1
        }
    }

    pub(super) fn bit_field_promoted_type(mem: &StructMember, width: u8) -> CType {
        if width < 32 {
            CType::Int
        } else if width == 32 {
            if mem.member_type.is_signed() {
                CType::Int
            } else {
                CType::UInt
            }
        } else if mem.member_type.is_signed() {
            CType::Long
        } else {
            CType::ULong
        }
    }

    pub(super) fn mark_bit_precision(&mut self, value: &TackyVal, width: u8) {
        if let TackyVal::Var(name) = value {
            if width > 32 && width < 64 {
                self.bit_precisions.insert(name.clone(), width);
            }
        }
    }

    pub(super) fn bit_precision(&self, value: &TackyVal) -> Option<u8> {
        let TackyVal::Var(name) = value else {
            return None;
        };
        self.bit_precisions.get(name).copied()
    }

    pub(super) fn sign_extend_bit_field_value(
        &mut self,
        value: TackyVal,
        mem: &StructMember,
        width: u8,
    ) -> TackyVal {
        if !mem.member_type.is_signed() || width == 0 || width as i32 >= mem.member_type.size() * 8
        {
            return value;
        }
        let sign_bit = 1_i64 << (width - 1);
        let xored = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseXor,
            left: value,
            right: TackyVal::Constant(sign_bit),
            dst: xored.clone(),
        });
        let extended = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::Sub,
            left: xored,
            right: TackyVal::Constant(sign_bit),
            dst: extended.clone(),
        });
        extended
    }

    pub(super) fn extract_bit_field(
        &mut self,
        unit: TackyVal,
        mem: &StructMember,
    ) -> TackyResult<TackyVal> {
        let Some(width) = mem.bit_width else {
            return Ok(unit);
        };
        let mut value = if mem.reverse_storage_order {
            self.byteswap_storage_value(unit, mem.member_type)
        } else {
            unit
        };
        if mem.bit_offset > 0 {
            let shifted = self.fresh_tmp(mem.member_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftRight,
                left: value,
                right: TackyVal::Constant(mem.bit_offset as i64),
                dst: shifted.clone(),
            });
            value = shifted;
        }
        if width as i32 == mem.member_type.size() * 8 {
            let promoted_type = Self::bit_field_promoted_type(mem, width);
            let value = self.convert_to(value, mem.member_type, promoted_type);
            if !mem.member_type.is_signed() {
                self.mark_bit_precision(&value, width);
            }
            return Ok(value);
        }
        let masked = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: value,
            right: TackyVal::Constant(Self::bit_mask(width)),
            dst: masked.clone(),
        });
        let value = self.sign_extend_bit_field_value(masked, mem, width);
        let promoted_type = Self::bit_field_promoted_type(mem, width);
        let value = self.convert_to(value, mem.member_type, promoted_type);
        if !mem.member_type.is_signed() {
            self.mark_bit_precision(&value, width);
        }
        Ok(value)
    }

    pub(super) fn byteswap_storage_value(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        match ty.size() {
            2 => self.byteswap_2(value, ty),
            4 => self.byteswap_4(value, ty),
            8 => self.byteswap_8(value, ty),
            _ => value,
        }
    }

    pub(super) fn bitwise_and_const(&mut self, value: TackyVal, ty: CType, mask: i64) -> TackyVal {
        let dst = self.fresh_tmp(ty);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: value,
            right: TackyVal::Constant(mask),
            dst: dst.clone(),
        });
        dst
    }

    pub(super) fn shift_const(
        &mut self,
        op: TackyBinaryOp,
        value: TackyVal,
        ty: CType,
        bits: i64,
    ) -> TackyVal {
        let dst = self.fresh_tmp(ty);
        self.emit(TackyInstr::Binary {
            op,
            left: value,
            right: TackyVal::Constant(bits),
            dst: dst.clone(),
        });
        dst
    }

    pub(super) fn bitwise_or(&mut self, left: TackyVal, right: TackyVal, ty: CType) -> TackyVal {
        let dst = self.fresh_tmp(ty);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left,
            right,
            dst: dst.clone(),
        });
        dst
    }

    pub(super) fn byteswap_2(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        let lo = self.bitwise_and_const(value.clone(), ty, 0x00ff);
        let lo = self.shift_const(TackyBinaryOp::ShiftLeft, lo, ty, 8);
        let hi = self.shift_const(TackyBinaryOp::ShiftRight, value, ty, 8);
        let hi = self.bitwise_and_const(hi, ty, 0x00ff);
        self.bitwise_or(lo, hi, ty)
    }

    pub(super) fn byteswap_4(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        let b0 = self.bitwise_and_const(value.clone(), ty, 0x000000ff);
        let b0 = self.shift_const(TackyBinaryOp::ShiftLeft, b0, ty, 24);
        let b1 = self.bitwise_and_const(value.clone(), ty, 0x0000ff00);
        let b1 = self.shift_const(TackyBinaryOp::ShiftLeft, b1, ty, 8);
        let b2 = self.shift_const(TackyBinaryOp::ShiftRight, value.clone(), ty, 8);
        let b2 = self.bitwise_and_const(b2, ty, 0x0000ff00);
        let b3 = self.shift_const(TackyBinaryOp::ShiftRight, value, ty, 24);
        let b3 = self.bitwise_and_const(b3, ty, 0x000000ff);
        let lo = self.bitwise_or(b0, b1, ty);
        let hi = self.bitwise_or(b2, b3, ty);
        self.bitwise_or(lo, hi, ty)
    }

    pub(super) fn byteswap_8(&mut self, value: TackyVal, ty: CType) -> TackyVal {
        let mut out = self.fresh_tmp(ty);
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: out.clone(),
        });
        for byte in 0..8 {
            let shifted = self.shift_const(TackyBinaryOp::ShiftRight, value.clone(), ty, byte * 8);
            let masked = self.bitwise_and_const(shifted, ty, 0xff);
            let moved = self.shift_const(TackyBinaryOp::ShiftLeft, masked, ty, (7 - byte) * 8);
            out = self.bitwise_or(out, moved, ty);
        }
        out
    }

    pub(super) fn store_bit_field_to_offset(
        &mut self,
        dst_name: String,
        mem: &StructMember,
        rhs: TackyVal,
    ) -> TackyResult<TackyVal> {
        self.store_bit_field_to_absolute_offset(dst_name, mem, mem.offset as i64, rhs)
    }

    pub(super) fn store_bit_field_to_absolute_offset(
        &mut self,
        dst_name: String,
        mem: &StructMember,
        offset: i64,
        rhs: TackyVal,
    ) -> TackyResult<TackyVal> {
        let Some(width) = mem.bit_width else {
            return Err("store_bit_field_to_offset called for non-bit-field".to_string());
        };
        let unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::CopyFromOffset {
            src_name: dst_name.clone(),
            offset,
            dst: unit.clone(),
        });
        let unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(unit, mem.member_type)
        } else {
            unit
        };
        let rhs_masked = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: rhs,
            right: TackyVal::Constant(Self::bit_mask(width)),
            dst: rhs_masked.clone(),
        });
        let inserted = if mem.bit_offset > 0 {
            let shifted = self.fresh_tmp(mem.member_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: rhs_masked.clone(),
                right: TackyVal::Constant(mem.bit_offset as i64),
                dst: shifted.clone(),
            });
            shifted
        } else {
            rhs_masked.clone()
        };
        let field_mask = Self::bit_mask(width) << mem.bit_offset;
        let cleared = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: unit,
            right: TackyVal::Constant(!field_mask),
            dst: cleared.clone(),
        });
        let new_unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left: cleared,
            right: inserted,
            dst: new_unit.clone(),
        });
        let stored_unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(new_unit, mem.member_type)
        } else {
            new_unit
        };
        self.emit(TackyInstr::CopyToOffset {
            src: stored_unit,
            dst_name,
            offset,
        });
        Ok(self.sign_extend_bit_field_value(rhs_masked, mem, width))
    }

    pub(super) fn store_bit_field_to_ptr(
        &mut self,
        dst_ptr: TackyVal,
        mem: &StructMember,
        rhs: TackyVal,
    ) -> TackyResult<TackyVal> {
        let Some(width) = mem.bit_width else {
            return Err("store_bit_field_to_ptr called for non-bit-field".to_string());
        };
        let unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Load {
            src_ptr: dst_ptr.clone(),
            dst: unit.clone(),
        });
        let unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(unit, mem.member_type)
        } else {
            unit
        };
        let rhs_masked = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: rhs,
            right: TackyVal::Constant(Self::bit_mask(width)),
            dst: rhs_masked.clone(),
        });
        let inserted = if mem.bit_offset > 0 {
            let shifted = self.fresh_tmp(mem.member_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: rhs_masked.clone(),
                right: TackyVal::Constant(mem.bit_offset as i64),
                dst: shifted.clone(),
            });
            shifted
        } else {
            rhs_masked.clone()
        };
        let field_mask = Self::bit_mask(width) << mem.bit_offset;
        let cleared = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: unit,
            right: TackyVal::Constant(!field_mask),
            dst: cleared.clone(),
        });
        let new_unit = self.fresh_tmp(mem.member_type);
        self.emit(TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left: cleared,
            right: inserted,
            dst: new_unit.clone(),
        });
        let stored_unit = if mem.reverse_storage_order {
            self.byteswap_storage_value(new_unit, mem.member_type)
        } else {
            new_unit
        };
        self.emit(TackyInstr::Store {
            src: stored_unit,
            dst_ptr,
        });
        Ok(self.sign_extend_bit_field_value(rhs_masked, mem, width))
    }

    pub(super) fn access_struct_member(
        &mut self,
        struct_addr: TackyVal,
        tag: String,
        member: &str,
    ) -> TackyResult<(TackyVal, CType)> {
        let mem = self.struct_member(&tag, member)?;
        let mem_type = mem.member_type;
        let mem_offset = mem.offset;
        let mem_ft = mem.member_full_type.clone();

        let mem_ptr = self.fresh_tmp(CType::Pointer);
        if mem_offset > 0 {
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: struct_addr,
                right: TackyVal::Constant(mem_offset as i64),
                dst: mem_ptr.clone(),
            });
        } else {
            self.emit(TackyInstr::Copy {
                src: struct_addr,
                dst: mem_ptr.clone(),
            });
        }

        if mem_ft.is_array() {
            // Array member: return pointer (decayed)
            let result_ft = FullType::Pointer(Box::new(match &mem_ft {
                FullType::Array { elem, .. } => *elem.clone(),
                _ => mem_ft.clone(),
            }));
            let result = self.fresh_tmp_full(&result_ft);
            self.emit(TackyInstr::Copy {
                src: mem_ptr,
                dst: result.clone(),
            });
            Ok((result, CType::Pointer))
        } else if mem_ft.is_struct() {
            // Struct member: return pointer to it (not loaded)
            let result_ft = FullType::Pointer(Box::new(mem_ft));
            let result = self.fresh_tmp_full(&result_ft);
            self.emit(TackyInstr::Copy {
                src: mem_ptr,
                dst: result.clone(),
            });
            Ok((result, CType::Pointer))
        } else {
            // Scalar member: load the value
            let result = if mem.bit_width.is_some() {
                self.fresh_tmp(mem.member_type)
            } else {
                self.fresh_tmp_full(&mem_ft)
            };
            self.emit(TackyInstr::Load {
                src_ptr: mem_ptr,
                dst: result.clone(),
            });
            let result = self.extract_bit_field(result, &mem)?;
            let result_type = mem
                .bit_width
                .map(|width| Self::bit_field_promoted_type(&mem, width))
                .unwrap_or(mem_type);
            Ok((result, result_type))
        }
    }

    /// Get the address of a struct value, handling deref temps correctly
    pub(super) fn get_struct_addr(&mut self, val: TackyVal) -> TackyVal {
        if let TackyVal::Var(ref n) = val {
            if self.array_sizes.contains_key(n) || self.get_full_type(n).is_vector() {
                // Proper aggregate/vector variable — take its address
                let a = self.fresh_tmp(CType::Pointer);
                self.emit(TackyInstr::GetAddress {
                    src: val,
                    dst: a.clone(),
                });
                return a;
            }
        }
        // Deref temp or pointer — use directly
        val
    }

    /// Emit a word-by-word struct copy from src address to dst name
    pub(super) fn emit_struct_copy_to(
        &mut self,
        src_addr: TackyVal,
        dst_name: &str,
        struct_size: usize,
    ) {
        let mut off = 0usize;
        while off + 8 <= struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Long);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: tmp.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: tmp,
                dst_name: dst_name.to_string(),
                offset: off as i64,
            });
            off += 8;
        }
        while off + 4 <= struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: tmp.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: tmp,
                dst_name: dst_name.to_string(),
                offset: off as i64,
            });
            off += 4;
        }
        while off < struct_size {
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Char);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: tmp.clone(),
            });
            self.emit(TackyInstr::CopyToOffset {
                src: tmp,
                dst_name: dst_name.to_string(),
                offset: off as i64,
            });
            off += 1;
        }
    }

    /// Emit struct copy from src address to dst address (both pointers)
    pub(super) fn emit_struct_copy_ptr_to_ptr(
        &mut self,
        src_addr: TackyVal,
        dst_addr: TackyVal,
        struct_size: usize,
    ) {
        let mut off = 0usize;
        while off + 8 <= struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: src_ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Long);
            self.emit(TackyInstr::Load {
                src_ptr,
                dst: tmp.clone(),
            });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: dst_ptr.clone(),
            });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr });
            off += 8;
        }
        while off + 4 <= struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: src_ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Int);
            self.emit(TackyInstr::Load {
                src_ptr,
                dst: tmp.clone(),
            });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: dst_ptr.clone(),
            });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr });
            off += 4;
        }
        while off < struct_size {
            let src_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: src_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: src_ptr.clone(),
            });
            let tmp = self.fresh_tmp(CType::Char);
            self.emit(TackyInstr::Load {
                src_ptr,
                dst: tmp.clone(),
            });
            let dst_ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: dst_addr.clone(),
                right: TackyVal::Constant(off as i64),
                dst: dst_ptr.clone(),
            });
            self.emit(TackyInstr::Store { src: tmp, dst_ptr });
            off += 1;
        }
    }

    /// Compute the address for a subscript expression a[i].
    /// Returns (pointer_to_element, element_type)
    /// Returns (pointer_to_element, element_ctype, element_full_type)
    pub(super) fn emit_subscript_addr(
        &mut self,
        arr: Exp,
        idx: Exp,
    ) -> TackyResult<(TackyVal, CType, FullType)> {
        let (first_val, first_type) = self.emit_exp(arr)?;
        let (second_val, second_type) = self.emit_exp(idx)?;

        // Normalize: pointer first, index second
        let first_full = self.val_full_type(&first_val);
        if let FullType::Vector { elem, .. } = first_full.clone() {
            let elem_full = elem.as_ref().clone();
            let elem_type = elem_full.to_ctype();
            let idx_long = self.convert_to(second_val, second_type, CType::Long);
            let base_ptr = self.fresh_tmp_full(&FullType::Pointer(Box::new(elem_full.clone())));
            self.emit(TackyInstr::GetAddress {
                src: first_val,
                dst: base_ptr.clone(),
            });
            let ptr = self.fresh_tmp_full(&FullType::Pointer(Box::new(elem_full.clone())));
            self.emit(TackyInstr::AddPtr {
                ptr: base_ptr,
                index: idx_long,
                scale: elem_full.byte_size_with(&self.struct_defs) as i64,
                dst: ptr.clone(),
            });
            return Ok((ptr, elem_type, elem_full));
        }
        let (arr_val, idx_val, idx_type, arr_full) =
            if first_full.is_pointer() || first_type == CType::Pointer {
                (first_val, second_val, second_type, first_full)
            } else {
                let second_full = self.val_full_type(&second_val);
                (second_val, first_val, first_type, second_full)
            };
        let (elem_full, scale) = match &arr_full {
            FullType::Pointer(inner) => (
                inner.as_ref().clone(),
                inner.byte_size_with(&self.struct_defs) as i64,
            ),
            _ => {
                // Fallback to old approach
                let elem_type = if let TackyVal::Var(ref name) = arr_val {
                    self.deref_type(name)
                } else {
                    CType::Int
                };
                (FullType::Scalar(elem_type), elem_type.size() as i64)
            }
        };
        let elem_type = elem_full.to_ctype();

        let idx_long = self.convert_to(idx_val, idx_type, CType::Long);
        if let TackyVal::Var(ref arr_name) = arr_val {
            if let Some(scale_exp) = self
                .dynamic_sizes
                .get(arr_name)
                .cloned()
                .or_else(|| self.vla_param_bounds.get(arr_name).cloned())
                .filter(|_| elem_full.is_array() || elem_full.is_struct())
            {
                let (scale_val, scale_type) = self.emit_exp(scale_exp)?;
                let scale_long = self.convert_to(scale_val, scale_type, CType::Long);
                let byte_offset = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Mul,
                    left: idx_long,
                    right: scale_long,
                    dst: byte_offset.clone(),
                });
                let result_ptr_type = FullType::Pointer(Box::new(elem_full.clone()));
                let ptr = self.fresh_tmp_full(&result_ptr_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Add,
                    left: arr_val.clone(),
                    right: byte_offset,
                    dst: ptr.clone(),
                });
                if let TackyVal::Var(ref pname) = ptr {
                    if let Some(info) = self.deref_info(arr_name) {
                        self.ptr_info.insert(pname.clone(), info);
                    } else {
                        self.ptr_info.insert(pname.clone(), (elem_type, 1));
                    }
                    if let Some(size) = self
                        .dynamic_sizes
                        .get(arr_name)
                        .cloned()
                        .filter(|_| elem_full.is_array() || elem_full.is_struct())
                    {
                        self.dynamic_sizes.insert(pname.clone(), size);
                    }
                }
                return Ok((ptr, elem_type, elem_full));
            }
        }
        let result_ptr_type = FullType::Pointer(Box::new(elem_full.clone()));
        let ptr = self.fresh_tmp_full(&result_ptr_type);
        self.emit(TackyInstr::AddPtr {
            ptr: arr_val.clone(),
            index: idx_long,
            scale,
            dst: ptr.clone(),
        });

        // Propagate pointee metadata to the derived pointer value.
        if let TackyVal::Var(ref pname) = ptr {
            if let TackyVal::Var(ref aname) = arr_val {
                if let Some(info) = self.deref_info(aname) {
                    self.ptr_info.insert(pname.clone(), info);
                } else {
                    self.ptr_info.insert(pname.clone(), (elem_type, 1));
                }
            }
        }

        Ok((ptr, elem_type, elem_full))
    }

    pub(super) fn emit_vector_lane_value(
        &mut self,
        value: TackyVal,
        value_ft: FullType,
        elem_type: CType,
        elem_size: usize,
        lane: usize,
    ) -> TackyResult<TackyVal> {
        if value_ft.is_vector() {
            if let TackyVal::Var(name) = value {
                let lane_value = self.fresh_tmp(elem_type);
                self.emit(TackyInstr::CopyFromOffset {
                    src_name: name,
                    offset: (lane * elem_size) as i64,
                    dst: lane_value.clone(),
                });
                return Ok(lane_value);
            }
            let addr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::GetAddress {
                src: value,
                dst: addr.clone(),
            });
            let ptr = self.fresh_tmp(CType::Pointer);
            self.emit(TackyInstr::AddPtr {
                ptr: addr,
                index: TackyVal::Constant(lane as i64),
                scale: elem_size as i64,
                dst: ptr.clone(),
            });
            let lane_value = self.fresh_tmp(elem_type);
            self.emit(TackyInstr::Load {
                src_ptr: ptr,
                dst: lane_value.clone(),
            });
            Ok(lane_value)
        } else {
            Ok(self.convert_to(value, value_ft.to_ctype(), elem_type))
        }
    }

    pub(super) fn emit_complex_component_value(
        &mut self,
        value: TackyVal,
        value_ft: FullType,
        elem_type: CType,
        _elem_size: usize,
        lane: usize,
    ) -> TackyResult<TackyVal> {
        if value_ft.is_complex() {
            let TackyVal::Var(name) = value else {
                return Err("complex values must lower to addressable temporaries".to_string());
            };
            let FullType::Vector { elem, .. } = value_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let source_elem_type = elem.to_ctype();
            let source_elem_size = elem.byte_size_with(&self.struct_defs);
            let dst = self.fresh_tmp(source_elem_type);
            self.emit(TackyInstr::CopyFromOffset {
                src_name: name,
                offset: (lane * source_elem_size) as i64,
                dst: dst.clone(),
            });
            Ok(self.convert_to(dst, source_elem_type, elem_type))
        } else if lane == 0 {
            Ok(self.convert_to(value, value_ft.to_ctype(), elem_type))
        } else {
            Ok(self.convert_to(TackyVal::Constant(0), CType::Int, elem_type))
        }
    }

    pub(super) fn emit_binary(
        &mut self,
        op: BinaryOp,
        left: Exp,
        right: Exp,
    ) -> TackyResult<(TackyVal, CType)> {
        let vector_comparison_elem = if is_comparison_op(&op) {
            match (self.typeof_exp(&left), self.typeof_exp(&right)) {
                (FullType::Vector { elem, .. }, _) | (_, FullType::Vector { elem, .. }) => {
                    Some(elem.to_ctype())
                }
                _ => None,
            }
        } else {
            None
        };
        let (l, l_type) = self.emit_exp(left)?;
        let (r, r_type) = self.emit_exp(right)?;
        let bit_precision = self.bit_precision(&l).max(self.bit_precision(&r));
        let l_full = self.val_full_type(&l);
        let r_full = self.val_full_type(&r);

        if l_full.is_complex() || r_full.is_complex() {
            let complex_ft = if l_full.is_complex() {
                l_full.clone()
            } else {
                r_full.clone()
            };
            let FullType::Vector { elem, .. } = complex_ft.clone() else {
                return Err("internal error: expected complex vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&complex_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("complex result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, complex_ft.byte_size_with(&self.struct_defs));

            let left_real = self.emit_complex_component_value(
                l.clone(),
                l_full.clone(),
                elem_type,
                elem_size,
                0,
            )?;
            let left_imag = self.emit_complex_component_value(
                l.clone(),
                l_full.clone(),
                elem_type,
                elem_size,
                1,
            )?;
            let right_real = self.emit_complex_component_value(
                r.clone(),
                r_full.clone(),
                elem_type,
                elem_size,
                0,
            )?;
            let right_imag = self.emit_complex_component_value(
                r.clone(),
                r_full.clone(),
                elem_type,
                elem_size,
                1,
            )?;

            match op {
                BinaryOp::Add | BinaryOp::Sub => {
                    let tacky_op = Self::convert_binop(op)?;
                    let real = self.fresh_tmp(elem_type);
                    let imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: tacky_op.clone(),
                        left: left_real,
                        right: right_real,
                        dst: real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: tacky_op,
                        left: left_imag,
                        right: right_imag,
                        dst: imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, elem_type));
                }
                BinaryOp::Mul => {
                    let ar_br = self.fresh_tmp(elem_type);
                    let ai_bi = self.fresh_tmp(elem_type);
                    let ar_bi = self.fresh_tmp(elem_type);
                    let ai_br = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real.clone(),
                        right: right_real.clone(),
                        dst: ar_br.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag.clone(),
                        right: right_imag.clone(),
                        dst: ai_bi.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real,
                        right: right_imag,
                        dst: ar_bi.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag,
                        right: right_real,
                        dst: ai_br.clone(),
                    });
                    let real = self.fresh_tmp(elem_type);
                    let imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: ar_br,
                        right: ai_bi,
                        dst: real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ar_bi,
                        right: ai_br,
                        dst: imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, elem_type));
                }
                BinaryOp::Div => {
                    let br2 = self.fresh_tmp(elem_type);
                    let bi2 = self.fresh_tmp(elem_type);
                    let denom = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: right_real.clone(),
                        right: right_real.clone(),
                        dst: br2.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: right_imag.clone(),
                        right: right_imag.clone(),
                        dst: bi2.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: br2,
                        right: bi2,
                        dst: denom.clone(),
                    });
                    let ar_br = self.fresh_tmp(elem_type);
                    let ai_bi = self.fresh_tmp(elem_type);
                    let ai_br = self.fresh_tmp(elem_type);
                    let ar_bi = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real.clone(),
                        right: right_real.clone(),
                        dst: ar_br.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag.clone(),
                        right: right_imag.clone(),
                        dst: ai_bi.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_imag,
                        right: right_real,
                        dst: ai_br.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Mul,
                        left: left_real,
                        right: right_imag,
                        dst: ar_bi.clone(),
                    });
                    let real_num = self.fresh_tmp(elem_type);
                    let imag_num = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Add,
                        left: ar_br,
                        right: ai_bi,
                        dst: real_num.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Sub,
                        left: ai_br,
                        right: ar_bi,
                        dst: imag_num.clone(),
                    });
                    let real = self.fresh_tmp(elem_type);
                    let imag = self.fresh_tmp(elem_type);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Div,
                        left: real_num,
                        right: denom.clone(),
                        dst: real.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Div,
                        left: imag_num,
                        right: denom,
                        dst: imag.clone(),
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: real,
                        dst_name: result_name.clone(),
                        offset: 0,
                    });
                    self.emit(TackyInstr::CopyToOffset {
                        src: imag,
                        dst_name: result_name.clone(),
                        offset: elem_size as i64,
                    });
                    return Ok((result, elem_type));
                }
                BinaryOp::Equal | BinaryOp::NotEqual => {
                    let real_cmp = self.fresh_tmp(CType::Int);
                    let imag_cmp = self.fresh_tmp(CType::Int);
                    let is_equal = matches!(op, BinaryOp::Equal);
                    let cmp_op = Self::convert_binop(op.clone())?;
                    self.emit(TackyInstr::Binary {
                        op: cmp_op.clone(),
                        left: left_real,
                        right: right_real,
                        dst: real_cmp.clone(),
                    });
                    self.emit(TackyInstr::Binary {
                        op: cmp_op,
                        left: left_imag,
                        right: right_imag,
                        dst: imag_cmp.clone(),
                    });
                    let combine_op = if is_equal {
                        TackyBinaryOp::BitwiseAnd
                    } else {
                        TackyBinaryOp::BitwiseOr
                    };
                    let dst = self.fresh_tmp(CType::Int);
                    self.emit(TackyInstr::Binary {
                        op: combine_op,
                        left: real_cmp,
                        right: imag_cmp,
                        dst: dst.clone(),
                    });
                    return Ok((dst, CType::Int));
                }
                _ => {
                    return Err("unsupported complex operator".to_string());
                }
            }
        }

        if is_comparison_op(&op) && (l_full.is_vector() || r_full.is_vector()) {
            let vector_ft = if l_full.is_vector() {
                l_full.clone()
            } else {
                r_full.clone()
            };
            let FullType::Vector { elem, lanes, .. } = vector_ft.clone() else {
                return Err("internal error: expected vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&vector_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("vector comparison result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, vector_ft.byte_size_with(&self.struct_defs));
            let tacky_op = Self::convert_binop(op)?;
            for lane in 0..lanes {
                let left_lane = self.emit_vector_lane_value(
                    l.clone(),
                    l_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let right_lane = self.emit_vector_lane_value(
                    r.clone(),
                    r_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let cmp = self.fresh_tmp(CType::Int);
                self.emit(TackyInstr::Binary {
                    op: tacky_op.clone(),
                    left: left_lane,
                    right: right_lane,
                    dst: cmp.clone(),
                });
                let bool_as_elem = self.convert_to(cmp, CType::Int, elem_type);
                let lane_mask = self.fresh_tmp(elem_type);
                let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Sub,
                    left: zero,
                    right: bool_as_elem,
                    dst: lane_mask.clone(),
                });
                self.emit(TackyInstr::CopyToOffset {
                    src: lane_mask,
                    dst_name: result_name.clone(),
                    offset: (lane * elem_size) as i64,
                });
            }
            return Ok((result, vector_ft.to_ctype()));
        }

        if !is_comparison_op(&op) && (l_full.is_vector() || r_full.is_vector()) {
            let vector_ft = if l_full.is_vector() {
                l_full.clone()
            } else {
                r_full.clone()
            };
            let FullType::Vector { elem, lanes, .. } = vector_ft.clone() else {
                return Err("internal error: expected vector type".to_string());
            };
            let elem_type = elem.to_ctype();
            let calc_type = match elem_type {
                CType::UChar | CType::UShort => CType::UInt,
                CType::Char | CType::SChar | CType::Short | CType::Bool => CType::Int,
                _ => elem_type,
            };
            let elem_size = elem.byte_size_with(&self.struct_defs);
            let result = self.fresh_tmp_full(&vector_ft);
            let TackyVal::Var(result_name) = result.clone() else {
                return Err("vector binary result must be addressable".to_string());
            };
            self.zero_init_local(&result_name, vector_ft.byte_size_with(&self.struct_defs));
            let tacky_op = Self::convert_binop(op)?;
            for lane in 0..lanes {
                let left_lane = self.emit_vector_lane_value(
                    l.clone(),
                    l_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let right_lane = self.emit_vector_lane_value(
                    r.clone(),
                    r_full.clone(),
                    elem_type,
                    elem_size,
                    lane,
                )?;
                let left_lane = self.convert_to(left_lane, elem_type, calc_type);
                let right_lane = self.convert_to(right_lane, elem_type, calc_type);
                let dst = self.fresh_tmp(calc_type);
                self.emit(TackyInstr::Binary {
                    op: tacky_op.clone(),
                    left: left_lane,
                    right: right_lane,
                    dst: dst.clone(),
                });
                let stored = self.convert_to(dst, calc_type, elem_type);
                self.emit(TackyInstr::CopyToOffset {
                    src: stored,
                    dst_name: result_name.clone(),
                    offset: (lane * elem_size) as i64,
                });
            }
            return Ok((result, vector_ft.to_ctype()));
        }

        if matches!(op, BinaryOp::Add | BinaryOp::Sub) {
            let (is_ptr_arith, ptr_val, int_val, elem_size, int_type) =
                if l_type == CType::Pointer && !r_type.is_pointer() {
                    let es = if let TackyVal::Var(ref n) = l {
                        self.ptr_elem_size(n)
                    } else {
                        1
                    };
                    (true, l.clone(), r.clone(), es, r_type)
                } else if r_type == CType::Pointer
                    && !l_type.is_pointer()
                    && matches!(op, BinaryOp::Add)
                {
                    let es = if let TackyVal::Var(ref n) = r {
                        self.ptr_elem_size(n)
                    } else {
                        1
                    };
                    (true, r.clone(), l.clone(), es, l_type)
                } else {
                    (false, l.clone(), r.clone(), 1, r_type)
                };

            if is_ptr_arith && elem_size > 1 {
                let int_long = self.convert_to(int_val, int_type, CType::Long);
                let scaled = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Mul,
                    left: int_long,
                    right: TackyVal::Constant(elem_size),
                    dst: scaled.clone(),
                });
                let ptr_ft = self.val_full_type(&ptr_val);
                let dst = self.fresh_tmp_full(&ptr_ft);
                let tacky_op = Self::convert_binop(op.clone())?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: ptr_val.clone(),
                    right: scaled,
                    dst: dst.clone(),
                });
                if let TackyVal::Var(ref pname) = ptr_val {
                    if let Some(&info) = self.ptr_info.get(pname) {
                        if let TackyVal::Var(ref dname) = dst {
                            self.ptr_info.insert(dname.clone(), info);
                        }
                    }
                }
                return Ok((dst, CType::Pointer));
            } else if is_ptr_arith {
                let int_long = self.convert_to(int_val, int_type, CType::Long);
                let ptr_ft = self.val_full_type(&ptr_val);
                let dst = self.fresh_tmp_full(&ptr_ft);
                let tacky_op = Self::convert_binop(op)?;
                self.emit(TackyInstr::Binary {
                    op: tacky_op,
                    left: ptr_val.clone(),
                    right: int_long,
                    dst: dst.clone(),
                });
                if let TackyVal::Var(ref pname) = ptr_val {
                    if let Some(&info) = self.ptr_info.get(pname) {
                        if let TackyVal::Var(ref dname) = dst {
                            self.ptr_info.insert(dname.clone(), info);
                        }
                    }
                }
                return Ok((dst, CType::Pointer));
            }

            if l_type == CType::Pointer && r_type == CType::Pointer && matches!(op, BinaryOp::Sub) {
                let raw_diff = self.fresh_tmp(CType::Long);
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::Sub,
                    left: l,
                    right: r,
                    dst: raw_diff.clone(),
                });
                let es = if let TackyVal::Var(ref n) = ptr_val {
                    self.ptr_elem_size(n)
                } else {
                    1
                };
                if es > 1 {
                    let result = self.fresh_tmp(CType::Long);
                    self.emit(TackyInstr::Binary {
                        op: TackyBinaryOp::Div,
                        left: raw_diff,
                        right: TackyVal::Constant(es),
                        dst: result.clone(),
                    });
                    return Ok((result, CType::Long));
                }
                return Ok((raw_diff, CType::Long));
            }
        }

        let is_shift = matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight);
        if is_shift {
            let promoted = l_type.promote();
            let l_conv = self.convert_to(l, l_type, promoted);
            let dst = self.fresh_tmp(promoted);
            let tacky_op = Self::convert_binop(op)?;
            self.emit(TackyInstr::Binary {
                op: tacky_op,
                left: l_conv,
                right: r,
                dst: dst.clone(),
            });
            if let Some(width) = bit_precision {
                self.emit(TackyInstr::Binary {
                    op: TackyBinaryOp::BitwiseAnd,
                    left: dst.clone(),
                    right: TackyVal::Constant(Self::bit_mask(width)),
                    dst: dst.clone(),
                });
                self.mark_bit_precision(&dst, width);
            }
            return Ok((dst, promoted));
        }

        let common = CType::common(l_type, r_type);
        let l_conv = self.convert_to(l, l_type, common);
        let r_conv = self.convert_to(r, r_type, common);
        if is_comparison_op(&op)
            && Self::should_warn_compare_distinct_pointer_types(&op, &l_full, &r_full)
        {
            self.warn_compare_distinct_pointer_types();
        }
        let result_type = if is_comparison_op(&op) {
            CType::Int
        } else {
            common
        };
        let dst = self.fresh_tmp(result_type);
        let is_comparison = is_comparison_op(&op);
        let tacky_op = Self::convert_binop(op)?;
        self.emit(TackyInstr::Binary {
            op: tacky_op,
            left: l_conv,
            right: r_conv,
            dst: dst.clone(),
        });
        if let Some(elem_type) = vector_comparison_elem {
            let bool_as_elem = self.convert_to(dst, CType::Int, elem_type);
            let result = self.fresh_tmp(elem_type);
            let zero = self.convert_to(TackyVal::Constant(0), CType::Int, elem_type);
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::Sub,
                left: zero,
                right: bool_as_elem,
                dst: result.clone(),
            });
            return Ok((result, elem_type));
        }
        if let Some(width) = bit_precision.filter(|_| !is_comparison) {
            self.emit(TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: dst.clone(),
                right: TackyVal::Constant(Self::bit_mask(width)),
                dst: dst.clone(),
            });
        }
        Ok((dst, result_type))
    }

    pub(super) fn emit_logical_and(&mut self, left: Exp, right: Exp) -> TackyResult<TackyVal> {
        let false_label = self.fresh_label("and_false");
        let end_label = self.fresh_label("and_end");
        let result = self.fresh_tmp(CType::Int);
        let (l, _) = self.emit_exp(left)?;
        self.emit(TackyInstr::JumpIfZero(l, false_label.clone()));
        let (r, _) = self.emit_exp(right)?;
        self.emit(TackyInstr::JumpIfZero(r, false_label.clone()));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(false_label));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Label(end_label));
        Ok(result)
    }

    pub(super) fn emit_logical_or(&mut self, left: Exp, right: Exp) -> TackyResult<TackyVal> {
        let true_label = self.fresh_label("or_true");
        let end_label = self.fresh_label("or_end");
        let result = self.fresh_tmp(CType::Int);
        let (l, _) = self.emit_exp(left)?;
        self.emit(TackyInstr::JumpIfNotZero(l, true_label.clone()));
        let (r, _) = self.emit_exp(right)?;
        self.emit(TackyInstr::JumpIfNotZero(r, true_label.clone()));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Jump(end_label.clone()));
        self.emit(TackyInstr::Label(true_label));
        self.emit(TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: result.clone(),
        });
        self.emit(TackyInstr::Label(end_label));
        Ok(result)
    }

    pub(super) fn convert_binop(op: BinaryOp) -> TackyResult<TackyBinaryOp> {
        match op {
            BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                Err(format!("invalid scalar binary operator: {:?}", op))
            }
            BinaryOp::Add => Ok(TackyBinaryOp::Add),
            BinaryOp::Sub => Ok(TackyBinaryOp::Sub),
            BinaryOp::Mul => Ok(TackyBinaryOp::Mul),
            BinaryOp::Div => Ok(TackyBinaryOp::Div),
            BinaryOp::Mod => Ok(TackyBinaryOp::Mod),
            BinaryOp::BitwiseAnd => Ok(TackyBinaryOp::BitwiseAnd),
            BinaryOp::BitwiseNand => Ok(TackyBinaryOp::BitwiseNand),
            BinaryOp::BitwiseOr => Ok(TackyBinaryOp::BitwiseOr),
            BinaryOp::BitwiseXor => Ok(TackyBinaryOp::BitwiseXor),
            BinaryOp::ShiftLeft => Ok(TackyBinaryOp::ShiftLeft),
            BinaryOp::ShiftRight => Ok(TackyBinaryOp::ShiftRight),
            BinaryOp::Equal => Ok(TackyBinaryOp::Equal),
            BinaryOp::NotEqual => Ok(TackyBinaryOp::NotEqual),
            BinaryOp::LessThan => Ok(TackyBinaryOp::LessThan),
            BinaryOp::GreaterThan => Ok(TackyBinaryOp::GreaterThan),
            BinaryOp::LessEqual => Ok(TackyBinaryOp::LessEqual),
            BinaryOp::GreaterEqual => Ok(TackyBinaryOp::GreaterEqual),
        }
    }

    // --------------------------------------------------------
    // Statement emission
    // --------------------------------------------------------

    pub(super) fn emit_statement(&mut self, stmt: Statement) -> TackyResult<()> {
        match stmt {
            Statement::Return(exp) => {
                let ret_type = self
                    .func_types
                    .get(&self.current_function)
                    .map(|(rt, _, _, _)| *rt)
                    .unwrap_or(CType::Int);
                if let Some(exp) = exp {
                    let exp_for_type = exp.clone();
                    let (val, val_type) = self.emit_exp(exp)?;
                    if ret_type == CType::Void {
                        self.emit(TackyInstr::Return(TackyVal::Constant(0)));
                    } else if let Some(ref ret_ptr) = self.hidden_ret_ptr.clone() {
                        // Large struct return via hidden pointer
                        let ret_ptr_val = TackyVal::Var(ret_ptr.clone());
                        let ret_ft = self.func_full_types.get(&self.current_function).cloned();
                        let val_ft = self.val_full_type(&val);
                        let src_addr = if let Some(ref ret_ft) = ret_ft {
                            if ret_ft.is_complex() {
                                if val_ft.is_complex() {
                                    self.get_struct_addr(val)
                                } else {
                                    let tmp = self.fresh_tmp_full(ret_ft);
                                    let TackyVal::Var(ref tmp_name) = tmp else {
                                        return Err(
                                            "internal error: hidden return value must be addressable"
                                                .to_string(),
                                        );
                                    };
                                    self.zero_init_local(
                                        tmp_name,
                                        ret_ft.byte_size_with(&self.struct_defs),
                                    );
                                    let FullType::Vector { elem, .. } = ret_ft.clone() else {
                                        return Err("internal error: expected complex return type"
                                            .to_string());
                                    };
                                    let elem_type = elem.to_ctype();
                                    let real = self.convert_to(val, val_type, elem_type);
                                    self.emit(TackyInstr::CopyToOffset {
                                        src: real,
                                        dst_name: tmp_name.clone(),
                                        offset: 0,
                                    });
                                    self.get_struct_addr(tmp)
                                }
                            } else if val_type == CType::Struct {
                                if let TackyVal::Var(ref name) = val {
                                    if self.array_sizes.contains_key(name) {
                                        let a = self.fresh_tmp(CType::Pointer);
                                        self.emit(TackyInstr::GetAddress {
                                            src: val,
                                            dst: a.clone(),
                                        });
                                        a
                                    } else {
                                        val
                                    }
                                } else {
                                    val
                                }
                            } else {
                                if let FullType::Struct(ret_tag) = ret_ft {
                                    if let FullType::Pointer(pointee) = &val_ft {
                                        if matches!(pointee.as_ref(), FullType::Struct(src_tag) if src_tag == ret_tag)
                                        {
                                            val
                                        } else {
                                            let a = self.fresh_tmp(CType::Pointer);
                                            self.emit(TackyInstr::GetAddress {
                                                src: val,
                                                dst: a.clone(),
                                            });
                                            a
                                        }
                                    } else {
                                        let a = self.fresh_tmp(CType::Pointer);
                                        self.emit(TackyInstr::GetAddress {
                                            src: val,
                                            dst: a.clone(),
                                        });
                                        a
                                    }
                                } else {
                                    let a = self.fresh_tmp(CType::Pointer);
                                    self.emit(TackyInstr::GetAddress {
                                        src: val,
                                        dst: a.clone(),
                                    });
                                    a
                                }
                            }
                        } else {
                            val
                        };
                        // Copy aggregate/complex result to hidden return pointer location
                        let ret_size = ret_ft
                            .as_ref()
                            .map(|ft| ft.byte_size_with(&self.struct_defs))
                            .unwrap_or(0);
                        self.emit_struct_copy_ptr_to_ptr(src_addr, ret_ptr_val.clone(), ret_size);
                        self.emit(TackyInstr::Return(ret_ptr_val));
                    } else {
                        let ret_ft = self
                            .func_full_types
                            .get(&self.current_function)
                            .cloned()
                            .unwrap_or(FullType::Scalar(ret_type));
                        let val_ft = self.val_full_type(&val);
                        if let (FullType::Struct(ref ret_tag), FullType::Pointer(ref pointee)) =
                            (&ret_ft, &val_ft)
                        {
                            if matches!(pointee.as_ref(), FullType::Struct(tag) if tag == ret_tag) {
                                let struct_size = self
                                    .struct_defs
                                    .get(ret_tag)
                                    .map(|def| def.size)
                                    .unwrap_or(0);
                                let result = self.fresh_tmp_full(&ret_ft);
                                if let TackyVal::Var(ref result_name) = result {
                                    self.array_sizes.insert(result_name.clone(), struct_size);
                                    self.emit_struct_copy_to(val, result_name, struct_size);
                                }
                                self.emit(TackyInstr::Return(result));
                                return Ok(());
                            }
                        }
                        self.assert_assignable_exp_full_type(
                            &ret_ft,
                            &val_ft,
                            &exp_for_type,
                            "return",
                        )?;
                        let val_conv = self.convert_to(val, val_type, ret_type);
                        self.emit(TackyInstr::Return(val_conv));
                    }
                } else {
                    self.emit(TackyInstr::Return(TackyVal::Constant(0)));
                }
            }
            Statement::Expression(exp) => {
                self.emit_exp(exp)?;
            }
            Statement::If(cond, then_stmt, else_stmt) => {
                let branch_has_label = Self::statement_contains_label(then_stmt.as_ref())
                    || else_stmt
                        .as_deref()
                        .is_some_and(Self::statement_contains_label);
                if !branch_has_label {
                    if let Some(value) = eval_static_integer_constant_exp_with_context(
                        &cond,
                        &self.struct_defs,
                        &self.full_types,
                    )
                    .map(|constant| constant.value)
                    {
                        if value != 0 {
                            self.emit_statement(*then_stmt)?;
                        } else if let Some(else_s) = else_stmt {
                            self.emit_statement(*else_s)?;
                        }
                        return Ok(());
                    }
                } else if let Some(value) = eval_static_integer_constant_exp_with_context(
                    &cond,
                    &self.struct_defs,
                    &self.full_types,
                )
                .map(|constant| constant.value)
                {
                    if value == 0 && else_stmt.is_none() {
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::Jump(end_label.clone()));
                        if let Some(pruned_then) =
                            Self::prune_unreachable_prefix_to_label(then_stmt.as_ref().clone())
                        {
                            self.emit_statement(pruned_then)?;
                        }
                        self.emit(TackyInstr::Label(end_label));
                        return Ok(());
                    }
                }
                let cond_val = self.emit_condition_value(cond)?;
                match else_stmt {
                    None => {
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::JumpIfZero(cond_val, end_label.clone()));
                        self.emit_statement(*then_stmt)?;
                        self.emit(TackyInstr::Label(end_label));
                    }
                    Some(else_s) => {
                        let else_label = self.fresh_label("if_else");
                        let end_label = self.fresh_label("if_end");
                        self.emit(TackyInstr::JumpIfZero(cond_val, else_label.clone()));
                        self.emit_statement(*then_stmt)?;
                        self.emit(TackyInstr::Jump(end_label.clone()));
                        self.emit(TackyInstr::Label(else_label));
                        self.emit_statement(*else_s)?;
                        self.emit(TackyInstr::Label(end_label));
                    }
                }
            }
            Statement::Block(block) => {
                self.emit_block(block)?;
            }
            Statement::While {
                condition,
                body,
                label,
            } => {
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                self.emit(TackyInstr::Label(continue_label.clone()));
                let cond_val = self.emit_condition_value(condition)?;
                self.emit(TackyInstr::JumpIfZero(cond_val, break_label.clone()));
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Jump(continue_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::DoWhile {
                body,
                condition,
                label,
            } => {
                let start_label = format!("start_{}", label);
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                self.emit(TackyInstr::Label(start_label.clone()));
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Label(continue_label));
                let cond_val = self.emit_condition_value(condition)?;
                self.emit(TackyInstr::JumpIfNotZero(cond_val, start_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::For {
                init,
                condition,
                post,
                body,
                label,
            } => {
                let start_label = format!("start_{}", label);
                let continue_label = format!("continue_{}", label);
                let break_label = format!("break_{}", label);
                match *init {
                    ForInit::Declaration(vd) => {
                        // Delegate to emit_var_decl which handles arrays, scalars, etc.
                        self.emit_var_decl(vd)?;
                    }
                    ForInit::Expression(Some(exp)) => {
                        self.emit_exp(exp)?;
                    }
                    ForInit::Expression(None) => {}
                }
                self.emit(TackyInstr::Label(start_label.clone()));
                if let Some(cond) = condition {
                    let cond_val = self.emit_condition_value(cond)?;
                    self.emit(TackyInstr::JumpIfZero(cond_val, break_label.clone()));
                }
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Label(continue_label));
                if let Some(post_exp) = post {
                    self.emit_exp(post_exp)?;
                }
                self.emit(TackyInstr::Jump(start_label));
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::Break(label) => {
                self.emit(TackyInstr::Jump(format!("break_{}", label)));
            }
            Statement::Continue(label) => {
                self.emit(TackyInstr::Jump(format!("continue_{}", label)));
            }
            Statement::Goto(label) => {
                let local = self
                    .local_label_stack
                    .last()
                    .is_none_or(|labels| labels.contains(&label));
                let target_function = if local {
                    self.current_function.clone()
                } else if let Some(parent) = self.label_address_function.as_ref() {
                    parent.clone()
                } else {
                    self.current_function.clone()
                };
                let target = format!("label.{}.{}", target_function, label);
                if local || target_function == self.current_function {
                    self.emit(TackyInstr::Jump(target));
                } else if let Some(slot) = self.current_parent_label_env_slots.get(&label) {
                    self.emit(TackyInstr::BuiltinLongjmp {
                        buf: TackyVal::Var(slot.clone()),
                        value: TackyVal::Constant(1),
                    });
                } else {
                    self.emit(TackyInstr::NonlocalJump(target));
                }
            }
            Statement::IndirectGoto(exp) => {
                let (target, _) = self.emit_exp(exp)?;
                self.emit(TackyInstr::JumpIndirect(target));
            }
            Statement::Label(name, body) => {
                self.emit(TackyInstr::Label(format!(
                    "label.{}.{}",
                    self.current_function, name
                )));
                self.emit_statement(*body)?;
            }
            Statement::Switch {
                control,
                body,
                label,
                cases,
            } => {
                let break_label = format!("break_{}", label);
                let (control_val, ctrl_type) = self.emit_exp(control)?;
                // Integer promotion for switch control
                let promoted_type = ctrl_type.promote();
                let control_val = self.convert_to(control_val, ctrl_type, promoted_type);
                for case in &cases {
                    if let Some(val) = case.value {
                        let cmp_value = self.fresh_tmp(CType::Int);
                        let low = self.switch_case_constant(val, promoted_type);
                        if let Some(end_val) = case.end_value {
                            let ge_low = self.fresh_tmp(CType::Int);
                            let le_high = self.fresh_tmp(CType::Int);
                            let high = self.switch_case_constant(end_val, promoted_type);
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::GreaterEqual,
                                left: control_val.clone(),
                                right: low,
                                dst: ge_low.clone(),
                            });
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::LessEqual,
                                left: control_val.clone(),
                                right: high,
                                dst: le_high.clone(),
                            });
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::BitwiseAnd,
                                left: ge_low,
                                right: le_high,
                                dst: cmp_value.clone(),
                            });
                        } else {
                            self.emit(TackyInstr::Binary {
                                op: TackyBinaryOp::Equal,
                                left: control_val.clone(),
                                right: low,
                                dst: cmp_value.clone(),
                            });
                        }
                        self.emit(TackyInstr::JumpIfNotZero(cmp_value, case.label.clone()));
                    }
                }
                let default_label = cases
                    .iter()
                    .find(|c| c.value.is_none())
                    .map(|c| c.label.clone());
                match default_label {
                    Some(dl) => self.emit(TackyInstr::Jump(dl)),
                    None => self.emit(TackyInstr::Jump(break_label.clone())),
                }
                self.emit_statement(*body)?;
                self.emit(TackyInstr::Label(break_label));
            }
            Statement::Case { body, label, .. } => {
                self.emit(TackyInstr::Label(label));
                self.emit_statement(*body)?;
            }
            Statement::Default { body, label } => {
                self.emit(TackyInstr::Label(label));
                self.emit_statement(*body)?;
            }
            Statement::Null => {}
        }
        Ok(())
    }
}
