use crate::types::*;
use std::collections::{HashMap, HashSet};

use super::instr_utils::{for_each_instr_defined_var, for_each_instr_source_var};

#[derive(Debug, Clone)]
struct InlineCandidate {
    params: Vec<String>,
    body: Vec<TackyInstr>,
    return_value: TackyVal,
}

#[derive(Debug, Clone)]
struct InlineMetadataCopy {
    original: String,
    renamed: String,
}

struct InlineExpansion<'a> {
    call_id: usize,
    renames: HashMap<String, String>,
    static_vars: &'a HashSet<String>,
    symbols: &'a indexmap::IndexMap<String, CType>,
    metadata: Vec<InlineMetadataCopy>,
}

pub(super) fn inline_functions(program: &mut TackyProgram, static_vars: &HashSet<String>) {
    let candidates =
        collect_inline_candidates(&program.top_level, &program.symbol_types, static_vars);
    if candidates.is_empty() {
        return;
    }

    let symbols = &program.symbol_types;
    let alignments = &program.symbol_alignments;
    let array_sizes = &program.array_sizes;
    let var_struct_tags = &program.var_struct_tags;
    let mut metadata = Vec::with_capacity(program.top_level.len());
    let mut next_call_id = 0usize;

    for top in &mut program.top_level {
        let TackyTopLevel::Function(func) = top else {
            continue;
        };
        let original_body = std::mem::take(&mut func.body);
        let mut rewritten = Vec::with_capacity(original_body.len());
        for instr in original_body {
            if let Some(expansion) = inline_call_instr(
                &instr,
                &func.name,
                &candidates,
                static_vars,
                symbols,
                &mut metadata,
                &mut next_call_id,
            ) {
                rewritten.extend(expansion);
            } else {
                rewritten.push(instr);
            }
        }
        func.body = rewritten;
    }

    let mut metadata_updates = Vec::with_capacity(metadata.len());
    for copy in metadata {
        metadata_updates.push((
            copy.renamed,
            symbols.get(&copy.original).copied(),
            alignments.get(&copy.original).copied(),
            array_sizes.get(&copy.original).copied(),
            var_struct_tags.get(&copy.original).cloned(),
        ));
    }

    for (renamed, ty, alignment, size, tag) in metadata_updates {
        if let Some(ty) = ty {
            program.symbol_types.insert(renamed.clone(), ty);
        }
        if let Some(alignment) = alignment {
            program.symbol_alignments.insert(renamed.clone(), alignment);
        }
        if let Some(size) = size {
            program.array_sizes.insert(renamed.clone(), size);
        }
        if let Some(tag) = tag {
            program.var_struct_tags.insert(renamed, tag);
        }
    }
}

fn collect_inline_candidates(
    top_level: &[TackyTopLevel],
    symbols: &indexmap::IndexMap<String, CType>,
    static_vars: &HashSet<String>,
) -> HashMap<String, InlineCandidate> {
    let mut candidates = HashMap::with_capacity(top_level.len());
    for top in top_level {
        let TackyTopLevel::Function(func) = top else {
            continue;
        };
        if let Some(candidate) = inline_candidate(func, symbols, static_vars) {
            candidates.insert(func.name.clone(), candidate);
        }
    }
    candidates
}

fn inline_candidate(
    func: &TackyFunction,
    symbols: &indexmap::IndexMap<String, CType>,
    static_vars: &HashSet<String>,
) -> Option<InlineCandidate> {
    if func.global
        || matches!(
            func.return_type,
            CType::Void | CType::Struct | CType::LongDouble
        )
        || !func.stack_params.is_empty()
        || !func.memory_param_blocks.is_empty()
        || !func.struct_param_groups.is_empty()
    {
        return None;
    }

    let mut body = Vec::with_capacity(func.body.len());
    let mut return_value = None;
    for instr in &func.body {
        match instr {
            TackyInstr::Nop => {}
            TackyInstr::Return(value) if return_value.is_none() => {
                return_value = Some(value.clone());
            }
            TackyInstr::Return(_) => return None,
            _ if return_value.is_some() => return None,
            _ if is_inline_safe_instr(instr) => body.push(instr.clone()),
            _ => return None,
        }
    }
    if body.len() > 16 {
        return None;
    }
    let return_value = return_value?;
    if func
        .params
        .iter()
        .any(|name| !static_vars.contains(name) && !symbols.contains_key(name))
    {
        return None;
    }
    for instr in &body {
        let mut rejected = false;
        for_each_instr_defined_var(instr, |name| {
            if static_vars.contains(name) || !symbols.contains_key(name) {
                rejected = true;
            }
        });
        if rejected {
            return None;
        }
        for_each_instr_source_var(instr, |name| {
            if !static_vars.contains(name) && !symbols.contains_key(name) {
                rejected = true;
            }
        });
        if rejected {
            return None;
        }
    }
    if let TackyVal::Var(name) = &return_value {
        if !static_vars.contains(name) && !symbols.contains_key(name) {
            return None;
        }
    }

    Some(InlineCandidate {
        params: func.params.clone(),
        body,
        return_value,
    })
}

fn is_inline_safe_instr(instr: &TackyInstr) -> bool {
    matches!(
        instr,
        TackyInstr::Copy { .. }
            | TackyInstr::Unary { .. }
            | TackyInstr::Truncate { .. }
            | TackyInstr::SignExtend { .. }
            | TackyInstr::ZeroExtend { .. }
            | TackyInstr::DoubleToInt { .. }
            | TackyInstr::FloatToInt { .. }
            | TackyInstr::DoubleToUInt { .. }
            | TackyInstr::FloatToUInt { .. }
            | TackyInstr::IntToDouble { .. }
            | TackyInstr::IntToFloat { .. }
            | TackyInstr::UIntToDouble { .. }
            | TackyInstr::UIntToFloat { .. }
            | TackyInstr::FloatToDouble { .. }
            | TackyInstr::DoubleToFloat { .. }
            | TackyInstr::AddPtr { .. }
            | TackyInstr::Binary { .. }
    )
}

fn inline_call_instr(
    instr: &TackyInstr,
    caller_name: &str,
    candidates: &HashMap<String, InlineCandidate>,
    static_vars: &HashSet<String>,
    symbols: &indexmap::IndexMap<String, CType>,
    metadata: &mut Vec<InlineMetadataCopy>,
    next_call_id: &mut usize,
) -> Option<Vec<TackyInstr>> {
    let TackyInstr::FunCall {
        name,
        args,
        dst,
        stack_arg_indices,
        memory_arg_blocks,
        struct_arg_groups,
        variadic,
        hidden_return,
        indirect,
        ..
    } = instr
    else {
        return None;
    };
    if name == caller_name
        || *variadic
        || *hidden_return
        || *indirect
        || !stack_arg_indices.is_empty()
        || !memory_arg_blocks.is_empty()
        || !struct_arg_groups.is_empty()
    {
        return None;
    }
    let candidate = candidates.get(name)?;
    if candidate.params.len() != args.len() {
        return None;
    }
    let TackyVal::Var(dst_name) = dst else {
        return None;
    };
    if matches!(
        symbols.get(dst_name),
        Some(CType::Void | CType::Struct | CType::LongDouble)
    ) {
        return None;
    }

    let mut expansion = InlineExpansion {
        call_id: *next_call_id,
        renames: HashMap::new(),
        static_vars,
        symbols,
        metadata: Vec::with_capacity(candidate.params.len() + candidate.body.len()),
    };
    *next_call_id += 1;

    let mut rewritten = Vec::with_capacity(candidate.params.len() + candidate.body.len() + 1);
    for (param, arg) in candidate.params.iter().zip(args) {
        let renamed_param = expansion.rename_var(param);
        rewritten.push(TackyInstr::Copy {
            src: arg.clone(),
            dst: TackyVal::Var(renamed_param),
        });
    }
    for instr in &candidate.body {
        rewritten.push(rename_inline_instr(instr, &mut expansion));
    }
    rewritten.push(TackyInstr::Copy {
        src: rename_inline_val(&candidate.return_value, &mut expansion),
        dst: dst.clone(),
    });
    metadata.extend(expansion.metadata);
    Some(rewritten)
}

impl InlineExpansion<'_> {
    fn rename_var(&mut self, name: &str) -> String {
        if self.static_vars.contains(name) {
            return name.to_string();
        }
        if let Some(renamed) = self.renames.get(name) {
            return renamed.clone();
        }
        let renamed = format!("__rnqcc_inline.{}.{}", self.call_id, name);
        if self.symbols.contains_key(name) {
            self.metadata.push(InlineMetadataCopy {
                original: name.to_string(),
                renamed: renamed.clone(),
            });
        }
        self.renames.insert(name.to_string(), renamed.clone());
        renamed
    }
}

fn rename_inline_val(val: &TackyVal, expansion: &mut InlineExpansion<'_>) -> TackyVal {
    match val {
        TackyVal::Var(name) => TackyVal::Var(expansion.rename_var(name)),
        _ => val.clone(),
    }
}

fn rename_inline_instr(instr: &TackyInstr, expansion: &mut InlineExpansion<'_>) -> TackyInstr {
    match instr {
        TackyInstr::Copy { src, dst } => TackyInstr::Copy {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::Unary { op, src, dst } => TackyInstr::Unary {
            op: op.clone(),
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => TackyInstr::Binary {
            op: op.clone(),
            left: rename_inline_val(left, expansion),
            right: rename_inline_val(right, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::Truncate { src, dst } => TackyInstr::Truncate {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::SignExtend { src, dst } => TackyInstr::SignExtend {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::ZeroExtend { src, dst } => TackyInstr::ZeroExtend {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::DoubleToInt { src, dst } => TackyInstr::DoubleToInt {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::FloatToInt { src, dst } => TackyInstr::FloatToInt {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::DoubleToUInt { src, dst } => TackyInstr::DoubleToUInt {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::FloatToUInt { src, dst } => TackyInstr::FloatToUInt {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::IntToDouble { src, dst } => TackyInstr::IntToDouble {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::IntToFloat { src, dst } => TackyInstr::IntToFloat {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::UIntToDouble { src, dst } => TackyInstr::UIntToDouble {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::UIntToFloat { src, dst } => TackyInstr::UIntToFloat {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::FloatToDouble { src, dst } => TackyInstr::FloatToDouble {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::DoubleToFloat { src, dst } => TackyInstr::DoubleToFloat {
            src: rename_inline_val(src, expansion),
            dst: rename_inline_val(dst, expansion),
        },
        TackyInstr::AddPtr {
            ptr,
            index,
            scale,
            dst,
        } => TackyInstr::AddPtr {
            ptr: rename_inline_val(ptr, expansion),
            index: rename_inline_val(index, expansion),
            scale: *scale,
            dst: rename_inline_val(dst, expansion),
        },
        _ => instr.clone(),
    }
}
