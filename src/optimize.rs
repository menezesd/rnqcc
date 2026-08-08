use crate::types::*;
use std::collections::HashSet;

mod constant_folding;
mod cse;
mod inline;
mod instr_utils;
mod ipcp;
mod licm;
mod unreachable;
use self::constant_folding::constant_folding;
use self::cse::{common_subexpression_elimination, cse_copy_from_offset};
use self::inline::inline_functions;
pub(crate) use self::instr_utils::for_each_instr_source_var;
use self::instr_utils::referenced_static_vars;
use self::ipcp::interprocedural_constant_propagation;
use self::licm::loop_invariant_code_motion;
use self::unreachable::unreachable_code_elimination;

const MAX_CLEANUP_ITERATIONS: usize = 8;

#[derive(Debug, Clone)]
pub struct OptimizationFlags {
    pub fold_constants: bool,
    pub eliminate_unreachable_code: bool,
    pub propagate_copies: bool,
    pub eliminate_dead_stores: bool,
    pub licm: bool,
    pub eliminate_common_subexpressions: bool,
    pub inline_functions: bool,
    pub interprocedural_constant_propagation: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OptimizationFlagSelections {
    pub fold_constants: bool,
    pub eliminate_unreachable_code: bool,
    pub propagate_copies: bool,
    pub eliminate_dead_stores: bool,
    pub licm: bool,
    pub eliminate_common_subexpressions: bool,
    pub inline_functions: bool,
    pub interprocedural_constant_propagation: bool,
}

impl OptimizationFlags {
    pub fn any_enabled(&self) -> bool {
        self.fold_constants
            || self.eliminate_unreachable_code
            || self.propagate_copies
            || self.eliminate_dead_stores
            || self.licm
            || self.eliminate_common_subexpressions
            || self.inline_functions
            || self.interprocedural_constant_propagation
    }

    pub fn all_enabled() -> Self {
        Self {
            fold_constants: true,
            eliminate_unreachable_code: true,
            propagate_copies: true,
            eliminate_dead_stores: true,
            licm: true,
            eliminate_common_subexpressions: true,
            inline_functions: true,
            interprocedural_constant_propagation: true,
        }
    }

    pub fn from_cli(all_opts: bool, selections: OptimizationFlagSelections) -> Self {
        Self {
            fold_constants: all_opts || selections.fold_constants,
            eliminate_unreachable_code: all_opts || selections.eliminate_unreachable_code,
            propagate_copies: all_opts || selections.propagate_copies,
            eliminate_dead_stores: all_opts || selections.eliminate_dead_stores,
            licm: all_opts || selections.licm,
            eliminate_common_subexpressions: all_opts || selections.eliminate_common_subexpressions,
            inline_functions: all_opts || selections.inline_functions,
            interprocedural_constant_propagation: all_opts
                || selections.interprocedural_constant_propagation,
        }
    }
}

pub fn optimize_program(program: &mut TackyProgram, flags: &OptimizationFlags) {
    if !flags.any_enabled() {
        return;
    }
    // Collect static/global variable names
    let mut static_var_names = program.global_vars.clone();
    for top in &program.top_level {
        if let TackyTopLevel::StaticVar(sv) = top {
            static_var_names.insert(sv.name.clone());
        }
    }
    if flags.inline_functions {
        inline_functions(program, &static_var_names);
    }
    if flags.interprocedural_constant_propagation {
        interprocedural_constant_propagation(program);
    }
    let types = &program.symbol_types;
    for top in &mut program.top_level {
        if let TackyTopLevel::Function(func) = top {
            optimize_function(func, flags, types, &static_var_names);
        }
    }
}

fn optimize_function(
    func: &mut TackyFunction,
    flags: &OptimizationFlags,
    types: &indexmap::IndexMap<String, CType>,
    static_var_names: &HashSet<String>,
) {
    if func.body.is_empty() {
        return;
    }

    let static_vars = referenced_static_vars(&func.body, static_var_names);

    run_post_transform_cleanup(func, flags, types);

    if flags.propagate_copies && !func.body.is_empty() {
        let aliased_vars = function_aliased_vars(func, &static_vars);
        run_copy_propagation(func, types, &aliased_vars);
        func.body = cse_copy_from_offset(std::mem::take(&mut func.body));

        run_post_transform_cleanup(func, flags, types);
    }

    if flags.eliminate_common_subexpressions && !func.body.is_empty() {
        let aliased_vars = function_aliased_vars(func, &static_vars);
        let mut cfg = crate::cfg::Cfg::build(std::mem::take(&mut func.body));
        common_subexpression_elimination(&mut cfg, types, &aliased_vars, &static_vars);
        func.body = cfg.into_instructions();

        if flags.propagate_copies && !func.body.is_empty() {
            let aliased_vars = function_aliased_vars(func, &static_vars);
            run_copy_propagation(func, types, &aliased_vars);
        }
        run_post_transform_cleanup(func, flags, types);
    }

    if flags.licm && !func.body.is_empty() {
        let aliased_vars = function_aliased_vars(func, &static_vars);
        let mut cfg = crate::cfg::Cfg::build(std::mem::take(&mut func.body));
        loop_invariant_code_motion(&mut cfg, types, &aliased_vars, &static_vars);
        func.body = cfg.into_instructions();

        if flags.propagate_copies && !func.body.is_empty() {
            let aliased_vars = function_aliased_vars(func, &static_vars);
            run_copy_propagation(func, types, &aliased_vars);
        }
        run_post_transform_cleanup(func, flags, types);
    }

    if flags.eliminate_dead_stores && !func.body.is_empty() {
        let aliased_vars = function_aliased_vars(func, &static_vars);
        let dse_changed = run_dead_store_elimination(func, &aliased_vars, &static_vars);
        if dse_changed && (flags.fold_constants || flags.eliminate_unreachable_code) {
            run_post_transform_cleanup(func, flags, types);
        }
    }

    if flags.propagate_copies && !func.body.is_empty() {
        let aliased_vars = function_aliased_vars(func, &static_vars);
        run_copy_propagation(func, types, &aliased_vars);

        if flags.fold_constants || flags.eliminate_unreachable_code {
            run_post_transform_cleanup(func, flags, types);
        }
    }
}

fn function_aliased_vars(func: &TackyFunction, static_vars: &HashSet<String>) -> HashSet<String> {
    let mut aliased = crate::cfg::find_aliased_vars(&func.body, static_vars);
    // Function parameters may alias one another at the call site (for
    // example, an aggregate parameter and a pointer parameter can refer to
    // the same object). Keep memory-sensitive optimizations conservative even
    // when the callee contains no direct GetAddress instruction.
    aliased.extend(func.params.iter().cloned());
    aliased
}

fn run_post_transform_cleanup(
    func: &mut TackyFunction,
    flags: &OptimizationFlags,
    types: &indexmap::IndexMap<String, CType>,
) {
    if !flags.fold_constants && !flags.eliminate_unreachable_code {
        return;
    }

    for _ in 0..MAX_CLEANUP_ITERATIONS {
        let mut changed = false;

        if flags.fold_constants && !func.body.is_empty() {
            let (body, did_change) = constant_folding(std::mem::take(&mut func.body), types);
            func.body = body;
            changed |= did_change;
        }
        if flags.eliminate_unreachable_code && !func.body.is_empty() {
            let (body, did_change) = unreachable_code_elimination(std::mem::take(&mut func.body));
            func.body = body;
            changed |= did_change;
        }

        if !changed || func.body.is_empty() {
            break;
        }
    }
}

fn run_copy_propagation(
    func: &mut TackyFunction,
    types: &indexmap::IndexMap<String, CType>,
    aliased_vars: &HashSet<String>,
) {
    let mut cfg = crate::cfg::Cfg::build(std::mem::take(&mut func.body));
    crate::cfg::copy_propagation(&mut cfg, aliased_vars, types);
    func.body = cfg.into_instructions();
}

fn run_dead_store_elimination(
    func: &mut TackyFunction,
    aliased_vars: &HashSet<String>,
    static_vars: &HashSet<String>,
) -> bool {
    let mut changed = false;
    let mut cfg = crate::cfg::Cfg::build(std::mem::take(&mut func.body));
    loop {
        let did_change = crate::cfg::dead_store_elimination(&mut cfg, aliased_vars, static_vars);
        if did_change {
            changed = true;
        }
        if !did_change || cfg.blocks.is_empty() {
            break;
        }
    }
    func.body = cfg.into_instructions();
    changed
}

#[cfg(test)]
mod tests;
