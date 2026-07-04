use crate::types::*;
use std::collections::{HashMap, HashSet};

use super::instr_utils::for_each_instr_defined_var;

#[derive(Debug, Clone)]
struct IpcpFunctionInfo {
    params: Vec<String>,
    constants: Vec<Option<TackyVal>>,
    blocked: Vec<bool>,
    saw_call: bool,
    address_escaped: bool,
}

pub(super) fn interprocedural_constant_propagation(program: &mut TackyProgram) {
    let mut functions = collect_ipcp_functions(program);
    if functions.is_empty() {
        return;
    }
    collect_ipcp_call_facts(&program.top_level, &mut functions);
    if !functions
        .values()
        .any(|info| ipcp_constants_to_apply(info).next().is_some())
    {
        return;
    }

    for top in &mut program.top_level {
        let TackyTopLevel::Function(func) = top else {
            continue;
        };
        let Some(info) = functions.get(&func.name) else {
            continue;
        };
        rewrite_ipcp_function(func, ipcp_constants_to_apply(info));
    }
}

fn collect_ipcp_functions(program: &TackyProgram) -> HashMap<String, IpcpFunctionInfo> {
    let mut functions = HashMap::with_capacity(program.top_level.len());
    for top in &program.top_level {
        let TackyTopLevel::Function(func) = top else {
            continue;
        };
        if func.global
            || func.params.is_empty()
            || !func.stack_params.is_empty()
            || !func.memory_param_blocks.is_empty()
            || !func.struct_param_groups.is_empty()
        {
            continue;
        }
        let mut blocked = Vec::with_capacity(func.params.len());
        for param in &func.params {
            blocked.push(matches!(
                program.symbol_types.get(param),
                Some(CType::Void | CType::Struct | CType::LongDouble) | None
            ));
        }
        functions.insert(
            func.name.clone(),
            IpcpFunctionInfo {
                params: func.params.clone(),
                constants: vec![None; func.params.len()],
                blocked,
                saw_call: false,
                address_escaped: false,
            },
        );
    }
    functions
}

fn collect_ipcp_call_facts(
    top_level: &[TackyTopLevel],
    functions: &mut HashMap<String, IpcpFunctionInfo>,
) {
    for top in top_level {
        match top {
            TackyTopLevel::Function(func) => {
                for instr in &func.body {
                    match instr {
                        TackyInstr::LoadLabelAddress(name, _) => {
                            mark_ipcp_address_escape(name, functions);
                        }
                        TackyInstr::GetAddress {
                            src: TackyVal::Var(name),
                            ..
                        } => {
                            mark_ipcp_address_escape(name, functions);
                        }
                        TackyInstr::FunCall {
                            name,
                            args,
                            variadic,
                            hidden_return,
                            indirect,
                            ..
                        } if !*indirect => {
                            let Some(info) = functions.get_mut(name) else {
                                continue;
                            };
                            if *variadic || *hidden_return || args.len() != info.params.len() {
                                info.blocked.fill(true);
                                continue;
                            }
                            info.saw_call = true;
                            for (idx, arg) in args.iter().enumerate() {
                                let Some(constant) = ipcp_constant_arg(arg) else {
                                    info.blocked[idx] = true;
                                    continue;
                                };
                                match &info.constants[idx] {
                                    Some(existing) if existing != &constant => {
                                        info.blocked[idx] = true
                                    }
                                    Some(_) => {}
                                    None => info.constants[idx] = Some(constant),
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            TackyTopLevel::StaticVar(var) => {
                for init in &var.init_values {
                    collect_ipcp_static_init_facts(init, functions);
                }
            }
            TackyTopLevel::StaticConstant(constant) => {
                collect_ipcp_static_init_facts(&constant.init, functions);
            }
            TackyTopLevel::Alias { target, .. } => {
                mark_ipcp_address_escape(target, functions);
            }
        }
    }
}

fn collect_ipcp_static_init_facts(
    init: &StaticInit,
    functions: &mut HashMap<String, IpcpFunctionInfo>,
) {
    match init {
        StaticInit::PointerInit(label) | StaticInit::PointerInitOffset(label, _) => {
            mark_ipcp_address_escape(label, functions);
        }
        StaticInit::LabelDiffInit(left, right, _) => {
            mark_ipcp_address_escape(left, functions);
            mark_ipcp_address_escape(right, functions);
        }
        _ => {}
    }
}

fn mark_ipcp_address_escape(name: &str, functions: &mut HashMap<String, IpcpFunctionInfo>) {
    if let Some(info) = functions.get_mut(name) {
        info.address_escaped = true;
    }
}

fn ipcp_constants_to_apply(
    info: &IpcpFunctionInfo,
) -> impl Iterator<Item = (String, TackyVal)> + '_ {
    info.params
        .iter()
        .cloned()
        .zip(info.constants.iter().cloned())
        .zip(info.blocked.iter().copied())
        .filter_map(move |((param, constant), blocked)| {
            if info.saw_call && !info.address_escaped && !blocked {
                constant.map(|constant| (param, constant))
            } else {
                None
            }
        })
}

fn ipcp_constant_arg(arg: &TackyVal) -> Option<TackyVal> {
    match arg {
        TackyVal::Var(_) => None,
        _ => Some(arg.clone()),
    }
}

fn rewrite_ipcp_function<I>(func: &mut TackyFunction, constants: I)
where
    I: IntoIterator<Item = (String, TackyVal)>,
{
    let mut active = constants.into_iter().collect::<HashMap<_, _>>();
    if active.is_empty() {
        return;
    }
    let mut params = HashSet::with_capacity(active.len());
    params.extend(active.keys().cloned());
    let mut rewritten = Vec::with_capacity(func.body.len());
    let old_body = std::mem::take(&mut func.body);
    let mut iter = old_body.into_iter();

    while let Some(instr) = iter.next() {
        let instr = rewrite_ipcp_instr(instr, &active);
        if let Some(escaped_param) = ipcp_address_taken_param(&instr, &params) {
            active.remove(escaped_param);
        }
        for_each_instr_defined_var(&instr, |def| {
            active.remove(def);
        });
        rewritten.push(instr);
        if active.is_empty() {
            rewritten.extend(iter);
            break;
        }
    }
    func.body = rewritten;
}

fn ipcp_address_taken_param<'a>(
    instr: &'a TackyInstr,
    params: &HashSet<String>,
) -> Option<&'a str> {
    match instr {
        TackyInstr::GetAddress {
            src: TackyVal::Var(name),
            ..
        } if params.contains(name) => Some(name.as_str()),
        _ => None,
    }
}

fn rewrite_ipcp_val(val: TackyVal, active: &HashMap<String, TackyVal>) -> TackyVal {
    match val {
        TackyVal::Var(name) => active.get(&name).cloned().unwrap_or(TackyVal::Var(name)),
        _ => val,
    }
}

fn rewrite_ipcp_instr(instr: TackyInstr, active: &HashMap<String, TackyVal>) -> TackyInstr {
    match instr {
        TackyInstr::Return(val) => TackyInstr::Return(rewrite_ipcp_val(val, active)),
        TackyInstr::Unary { op, src, dst } => TackyInstr::Unary {
            op,
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => TackyInstr::Binary {
            op,
            left: rewrite_ipcp_val(left, active),
            right: rewrite_ipcp_val(right, active),
            dst,
        },
        TackyInstr::Copy { src, dst } => TackyInstr::Copy {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::JumpIndirect(val) => TackyInstr::JumpIndirect(rewrite_ipcp_val(val, active)),
        TackyInstr::JumpIfZero(val, label) => {
            TackyInstr::JumpIfZero(rewrite_ipcp_val(val, active), label)
        }
        TackyInstr::JumpIfNotZero(val, label) => {
            TackyInstr::JumpIfNotZero(rewrite_ipcp_val(val, active), label)
        }
        TackyInstr::BuiltinSetjmp {
            buf,
            dst,
            label,
            end_label,
        } => TackyInstr::BuiltinSetjmp {
            buf: rewrite_ipcp_val(buf, active),
            dst,
            label,
            end_label,
        },
        TackyInstr::BuiltinLongjmp { buf, value } => TackyInstr::BuiltinLongjmp {
            buf: rewrite_ipcp_val(buf, active),
            value: rewrite_ipcp_val(value, active),
        },
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
        } => TackyInstr::FunCall {
            name,
            args: args
                .into_iter()
                .map(|arg| rewrite_ipcp_val(arg, active))
                .collect(),
            dst,
            stack_arg_indices,
            memory_arg_blocks,
            struct_arg_groups,
            variadic,
            fixed_flat_arg_count,
            hidden_return,
            indirect,
        },
        TackyInstr::SignExtend { src, dst } => TackyInstr::SignExtend {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::ZeroExtend { src, dst } => TackyInstr::ZeroExtend {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::Truncate { src, dst } => TackyInstr::Truncate {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::IntToDouble { src, dst } => TackyInstr::IntToDouble {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::IntToFloat { src, dst } => TackyInstr::IntToFloat {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::DoubleToInt { src, dst } => TackyInstr::DoubleToInt {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::FloatToInt { src, dst } => TackyInstr::FloatToInt {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::UIntToDouble { src, dst } => TackyInstr::UIntToDouble {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::UIntToFloat { src, dst } => TackyInstr::UIntToFloat {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::DoubleToUInt { src, dst } => TackyInstr::DoubleToUInt {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::FloatToUInt { src, dst } => TackyInstr::FloatToUInt {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::FloatToDouble { src, dst } => TackyInstr::FloatToDouble {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::DoubleToFloat { src, dst } => TackyInstr::DoubleToFloat {
            src: rewrite_ipcp_val(src, active),
            dst,
        },
        TackyInstr::AtomicFetch {
            op,
            ptr,
            arg,
            return_old,
            dst,
        } => TackyInstr::AtomicFetch {
            op,
            ptr: rewrite_ipcp_val(ptr, active),
            arg: rewrite_ipcp_val(arg, active),
            return_old,
            dst,
        },
        TackyInstr::AtomicExchange { ptr, value, dst } => TackyInstr::AtomicExchange {
            ptr: rewrite_ipcp_val(ptr, active),
            value: rewrite_ipcp_val(value, active),
            dst,
        },
        TackyInstr::AtomicCompareExchange {
            ptr,
            expected,
            desired,
            dst,
        } => TackyInstr::AtomicCompareExchange {
            ptr: rewrite_ipcp_val(ptr, active),
            expected: rewrite_ipcp_val(expected, active),
            desired: rewrite_ipcp_val(desired, active),
            dst,
        },
        TackyInstr::AtomicCompareSwap {
            ptr,
            expected,
            desired,
            return_old,
            dst,
        } => TackyInstr::AtomicCompareSwap {
            ptr: rewrite_ipcp_val(ptr, active),
            expected: rewrite_ipcp_val(expected, active),
            desired: rewrite_ipcp_val(desired, active),
            return_old,
            dst,
        },
        TackyInstr::Load { src_ptr, dst } => TackyInstr::Load {
            src_ptr: rewrite_ipcp_val(src_ptr, active),
            dst,
        },
        TackyInstr::Store { src, dst_ptr } => TackyInstr::Store {
            src: rewrite_ipcp_val(src, active),
            dst_ptr: rewrite_ipcp_val(dst_ptr, active),
        },
        TackyInstr::CopyToOffset {
            src,
            dst_name,
            offset,
        } => TackyInstr::CopyToOffset {
            src: rewrite_ipcp_val(src, active),
            dst_name,
            offset,
        },
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => TackyInstr::AddPtr {
            ptr: rewrite_ipcp_val(ptr, active),
            index: rewrite_ipcp_val(index, active),
            scale,
            dst,
        },
        _ => instr,
    }
}
