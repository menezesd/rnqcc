use crate::types::*;
use std::collections::HashSet;

pub(super) fn referenced_static_vars(
    instructions: &[TackyInstr],
    static_var_names: &HashSet<String>,
) -> HashSet<String> {
    let mut referenced = HashSet::with_capacity(static_var_names.len());
    for instr in instructions {
        collect_static_refs(instr, static_var_names, &mut referenced);
    }
    referenced
}

fn collect_static_val_ref(
    val: &TackyVal,
    static_var_names: &HashSet<String>,
    referenced: &mut HashSet<String>,
) {
    if let TackyVal::Var(name) = val {
        if static_var_names.contains(name) {
            referenced.insert(name.clone());
        }
    }
}

fn collect_static_name_ref(
    name: &str,
    static_var_names: &HashSet<String>,
    referenced: &mut HashSet<String>,
) {
    if static_var_names.contains(name) {
        referenced.insert(name.to_string());
    }
}

fn collect_static_refs(
    instr: &TackyInstr,
    static_var_names: &HashSet<String>,
    referenced: &mut HashSet<String>,
) {
    match instr {
        TackyInstr::Copy { src, dst }
        | TackyInstr::Store { src, dst_ptr: dst }
        | TackyInstr::BuiltinLongjmp {
            buf: src,
            value: dst,
        } => {
            collect_static_val_ref(src, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::Unary { src, dst, .. }
        | TackyInstr::Truncate { src, dst }
        | TackyInstr::SignExtend { src, dst }
        | TackyInstr::ZeroExtend { src, dst }
        | TackyInstr::DoubleToInt { src, dst }
        | TackyInstr::FloatToInt { src, dst }
        | TackyInstr::DoubleToUInt { src, dst }
        | TackyInstr::FloatToUInt { src, dst }
        | TackyInstr::IntToDouble { src, dst }
        | TackyInstr::IntToFloat { src, dst }
        | TackyInstr::UIntToDouble { src, dst }
        | TackyInstr::UIntToFloat { src, dst }
        | TackyInstr::FloatToDouble { src, dst }
        | TackyInstr::DoubleToFloat { src, dst }
        | TackyInstr::Load { src_ptr: src, dst }
        | TackyInstr::GetAddress { src, dst } => {
            collect_static_val_ref(src, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::Binary {
            left, right, dst, ..
        }
        | TackyInstr::AddPtr {
            ptr: left,
            index: right,
            dst,
            ..
        } => {
            collect_static_val_ref(left, static_var_names, referenced);
            collect_static_val_ref(right, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::Return(val)
        | TackyInstr::JumpIndirect(val)
        | TackyInstr::JumpIfZero(val, _)
        | TackyInstr::JumpIfNotZero(val, _)
        | TackyInstr::VaStart { dst: val }
        | TackyInstr::FrameAddress { dst: val } => {
            collect_static_val_ref(val, static_var_names, referenced);
        }
        TackyInstr::BuiltinSetjmp { buf, dst, .. } => {
            collect_static_val_ref(buf, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::FunCall {
            name,
            args,
            dst,
            indirect,
            ..
        } => {
            if *indirect {
                collect_static_name_ref(name, static_var_names, referenced);
            }
            for arg in args {
                collect_static_val_ref(arg, static_var_names, referenced);
            }
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicFetch { ptr, arg, dst, .. } => {
            collect_static_val_ref(ptr, static_var_names, referenced);
            collect_static_val_ref(arg, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicExchange { ptr, value, dst } => {
            collect_static_val_ref(ptr, static_var_names, referenced);
            collect_static_val_ref(value, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicCompareExchange {
            ptr,
            expected,
            desired,
            dst,
        }
        | TackyInstr::AtomicCompareSwap {
            ptr,
            expected,
            desired,
            dst,
            ..
        } => {
            collect_static_val_ref(ptr, static_var_names, referenced);
            collect_static_val_ref(expected, static_var_names, referenced);
            collect_static_val_ref(desired, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::CopyToOffset { src, dst_name, .. } => {
            collect_static_val_ref(src, static_var_names, referenced);
            collect_static_name_ref(dst_name, static_var_names, referenced);
        }
        TackyInstr::CopyFromOffset { src_name, dst, .. } => {
            collect_static_name_ref(src_name, static_var_names, referenced);
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::CopyStruct { src_name, dst_name } => {
            collect_static_name_ref(src_name, static_var_names, referenced);
            collect_static_name_ref(dst_name, static_var_names, referenced);
        }
        TackyInstr::LoadLabelAddress(_, dst) => {
            collect_static_val_ref(dst, static_var_names, referenced);
        }
        TackyInstr::AtomicFence
        | TackyInstr::Jump(_)
        | TackyInstr::NonlocalJump(_)
        | TackyInstr::Label(_)
        | TackyInstr::Unreachable
        | TackyInstr::Nop => {}
    }
}

pub(crate) fn for_each_instr_defined_var(instr: &TackyInstr, mut f: impl FnMut(&str)) {
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
        | TackyInstr::Load {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::GetAddress {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::CopyFromOffset {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::AddPtr {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::LoadLabelAddress(_, TackyVal::Var(name))
        | TackyInstr::FunCall {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::VaStart {
            dst: TackyVal::Var(name),
        }
        | TackyInstr::FrameAddress {
            dst: TackyVal::Var(name),
        }
        | TackyInstr::BuiltinSetjmp {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::AtomicFetch {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::AtomicExchange {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::AtomicCompareExchange {
            dst: TackyVal::Var(name),
            ..
        }
        | TackyInstr::AtomicCompareSwap {
            dst: TackyVal::Var(name),
            ..
        } => f(name.as_str()),
        TackyInstr::CopyToOffset { dst_name, .. } | TackyInstr::CopyStruct { dst_name, .. } => {
            f(dst_name.as_str());
        }
        _ => {}
    }
}

pub(crate) fn for_each_instr_source_var(instr: &TackyInstr, mut f: impl FnMut(&str)) {
    match instr {
        TackyInstr::Copy { src, .. }
        | TackyInstr::Unary { src, .. }
        | TackyInstr::Truncate { src, .. }
        | TackyInstr::SignExtend { src, .. }
        | TackyInstr::ZeroExtend { src, .. }
        | TackyInstr::DoubleToInt { src, .. }
        | TackyInstr::FloatToInt { src, .. }
        | TackyInstr::DoubleToUInt { src, .. }
        | TackyInstr::FloatToUInt { src, .. }
        | TackyInstr::IntToDouble { src, .. }
        | TackyInstr::IntToFloat { src, .. }
        | TackyInstr::UIntToDouble { src, .. }
        | TackyInstr::UIntToFloat { src, .. }
        | TackyInstr::FloatToDouble { src, .. }
        | TackyInstr::DoubleToFloat { src, .. } => push_val_var_ref(src, &mut f),
        TackyInstr::Binary { left, right, .. }
        | TackyInstr::AddPtr {
            ptr: left,
            index: right,
            ..
        } => {
            push_val_var_ref(left, &mut f);
            push_val_var_ref(right, &mut f);
        }
        TackyInstr::Load {
            src_ptr: TackyVal::Var(name),
            ..
        } => f(name.as_str()),
        TackyInstr::Store { src, dst_ptr } => {
            push_val_var_ref(src, &mut f);
            push_val_var_ref(dst_ptr, &mut f);
        }
        TackyInstr::GetAddress { src, .. }
        | TackyInstr::Return(src)
        | TackyInstr::JumpIndirect(src)
        | TackyInstr::JumpIfZero(src, _)
        | TackyInstr::JumpIfNotZero(src, _) => push_val_var_ref(src, &mut f),
        TackyInstr::BuiltinLongjmp { buf, value } => {
            push_val_var_ref(buf, &mut f);
            push_val_var_ref(value, &mut f);
        }
        TackyInstr::BuiltinSetjmp {
            buf: TackyVal::Var(name),
            ..
        } => f(name.as_str()),
        TackyInstr::FunCall {
            name,
            args,
            indirect,
            ..
        } => {
            if *indirect {
                f(name.as_str());
            }
            for arg in args {
                push_val_var_ref(arg, &mut f);
            }
        }
        TackyInstr::AtomicFetch { ptr, arg, .. } => {
            push_val_var_ref(ptr, &mut f);
            push_val_var_ref(arg, &mut f);
        }
        TackyInstr::AtomicExchange { ptr, value, .. } => {
            push_val_var_ref(ptr, &mut f);
            push_val_var_ref(value, &mut f);
        }
        TackyInstr::AtomicCompareExchange {
            ptr,
            expected,
            desired,
            ..
        }
        | TackyInstr::AtomicCompareSwap {
            ptr,
            expected,
            desired,
            ..
        } => {
            push_val_var_ref(ptr, &mut f);
            push_val_var_ref(expected, &mut f);
            push_val_var_ref(desired, &mut f);
        }
        TackyInstr::CopyToOffset {
            src: TackyVal::Var(name),
            ..
        } => f(name.as_str()),
        TackyInstr::CopyFromOffset { src_name, .. } => f(src_name.as_str()),
        TackyInstr::CopyStruct { src_name, .. } => f(src_name.as_str()),
        _ => {}
    }
}

fn push_val_var_ref<'a>(val: &'a TackyVal, f: &mut impl FnMut(&'a str)) {
    if let TackyVal::Var(name) = val {
        f(name.as_str());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str) -> TackyVal {
        TackyVal::Var(name.to_string())
    }

    #[test]
    fn instr_source_vars_includes_builtin_longjmp_value() {
        let mut vars = Vec::new();
        for_each_instr_source_var(
            &TackyInstr::BuiltinLongjmp {
                buf: v("env"),
                value: v("code"),
            },
            |name| vars.push(name.to_string()),
        );
        assert_eq!(vars, vec!["env".to_string(), "code".to_string()]);
    }

    #[test]
    fn instr_source_vars_includes_indirect_call_callee() {
        let mut vars = Vec::new();
        for_each_instr_source_var(
            &TackyInstr::FunCall {
                name: "callee".to_string(),
                args: vec![v("arg")],
                dst: v("dst"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 1,
                hidden_return: false,
                indirect: true,
            },
            |name| vars.push(name.to_string()),
        );
        assert_eq!(vars, vec!["callee".to_string(), "arg".to_string()]);
    }
}
