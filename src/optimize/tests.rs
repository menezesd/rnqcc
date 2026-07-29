use super::*;
use indexmap::IndexMap;
use std::collections::HashMap;

fn v(name: &str) -> TackyVal {
    TackyVal::Var(name.to_string())
}

fn flags_with_licm() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: false,
        propagate_copies: false,
        eliminate_dead_stores: false,
        licm: true,
        eliminate_common_subexpressions: false,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_copy_propagation_and_licm() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: false,
        propagate_copies: true,
        eliminate_dead_stores: false,
        licm: true,
        eliminate_common_subexpressions: false,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_cse() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: false,
        propagate_copies: false,
        eliminate_dead_stores: false,
        licm: false,
        eliminate_common_subexpressions: true,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_copy_propagation_and_cse() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: false,
        propagate_copies: true,
        eliminate_dead_stores: false,
        licm: false,
        eliminate_common_subexpressions: true,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_inlining() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: false,
        propagate_copies: false,
        eliminate_dead_stores: false,
        licm: false,
        eliminate_common_subexpressions: false,
        inline_functions: true,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_ipcp() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: false,
        propagate_copies: false,
        eliminate_dead_stores: false,
        licm: false,
        eliminate_common_subexpressions: false,
        inline_functions: false,
        interprocedural_constant_propagation: true,
    }
}

fn flags_with_constant_folding() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: true,
        eliminate_unreachable_code: false,
        propagate_copies: false,
        eliminate_dead_stores: false,
        licm: false,
        eliminate_common_subexpressions: false,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_unreachable_cleanup() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: true,
        propagate_copies: false,
        eliminate_dead_stores: false,
        licm: false,
        eliminate_common_subexpressions: false,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_copy_folding_and_unreachable_cleanup() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: true,
        eliminate_unreachable_code: true,
        propagate_copies: true,
        eliminate_dead_stores: false,
        licm: false,
        eliminate_common_subexpressions: false,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn flags_with_copy_propagation_and_dead_store_elimination() -> OptimizationFlags {
    OptimizationFlags {
        fold_constants: false,
        eliminate_unreachable_code: false,
        propagate_copies: true,
        eliminate_dead_stores: true,
        licm: false,
        eliminate_common_subexpressions: false,
        inline_functions: false,
        interprocedural_constant_propagation: false,
    }
}

fn empty_function(body: Vec<TackyInstr>) -> TackyFunction {
    TackyFunction {
        name: "f".to_string(),
        return_type: CType::Int,
        params: Vec::new(),
        global: false,
        body,
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    }
}

fn int_types(names: &[&str]) -> IndexMap<String, CType> {
    names
        .iter()
        .map(|name| ((*name).to_string(), CType::Int))
        .collect()
}

fn typed_vars(vars: &[(&str, CType)]) -> IndexMap<String, CType> {
    vars.iter()
        .map(|(name, ty)| ((*name).to_string(), *ty))
        .collect()
}

fn program_with_functions(
    functions: Vec<TackyFunction>,
    symbols: IndexMap<String, CType>,
) -> TackyProgram {
    TackyProgram {
        top_level: functions.into_iter().map(TackyTopLevel::Function).collect(),
        global_vars: HashSet::new(),
        thread_local_vars: HashSet::new(),
        symbol_types: symbols,
        symbol_alignments: IndexMap::new(),
        array_sizes: IndexMap::new(),
        struct_defs: IndexMap::new(),
        var_struct_tags: HashMap::new(),
    }
}

fn function_at(program: &TackyProgram, index: usize) -> &TackyFunction {
    let Some(TackyTopLevel::Function(function)) = program.top_level.get(index) else {
        panic!("expected function at index {index}");
    };
    function
}

fn instr_index(body: &[TackyInstr], needle: &TackyInstr) -> usize {
    body.iter()
        .position(|instr| instr == needle)
        .expect("instruction not found")
}

#[test]
fn constant_folding_simplifies_integer_identity_ops() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("a"),
            right: TackyVal::Constant(0),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Mul,
            left: TackyVal::Constant(1),
            right: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::ShiftLeft,
            left: v("__rnqcc_tmp.1"),
            right: TackyVal::Constant(0),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: v("__rnqcc_tmp.2"),
            right: TackyVal::Constant(0),
            dst: v("__rnqcc_tmp.3"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.3")),
    ]);
    let types = int_types(&[
        "a",
        "__rnqcc_tmp.0",
        "__rnqcc_tmp.1",
        "__rnqcc_tmp.2",
        "__rnqcc_tmp.3",
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("a"),
        dst: v("__rnqcc_tmp.0"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.1"),
        dst: v("__rnqcc_tmp.2"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::Constant(0),
        dst: v("__rnqcc_tmp.3"),
    }));
}

#[test]
fn constant_folding_strength_reduces_unsigned_power_of_two_arithmetic() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Mul,
            left: v("x"),
            right: TackyVal::Constant(8),
            dst: v("mul"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Div,
            left: v("mul"),
            right: TackyVal::Constant(16),
            dst: v("div"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Mod,
            left: v("div"),
            right: TackyVal::Constant(32),
            dst: v("rem"),
        },
        TackyInstr::Return(v("rem")),
    ]);
    let types = typed_vars(&[
        ("x", CType::UInt),
        ("mul", CType::UInt),
        ("div", CType::UInt),
        ("rem", CType::UInt),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert_eq!(
        func.body,
        vec![
            TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: v("x"),
                right: TackyVal::Constant(3),
                dst: v("mul"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::ShiftRight,
                left: v("mul"),
                right: TackyVal::Constant(4),
                dst: v("div"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: v("div"),
                right: TackyVal::Constant(31),
                dst: v("rem"),
            },
            TackyInstr::Return(v("rem")),
        ]
    );
}

#[test]
fn constant_folding_keeps_signed_power_of_two_multiplication() {
    let original = TackyInstr::Binary {
        op: TackyBinaryOp::Mul,
        left: v("x"),
        right: TackyVal::Constant(8),
        dst: v("result"),
    };
    let mut func = empty_function(vec![original.clone(), TackyInstr::Return(v("result"))]);
    let types = int_types(&["x", "result"]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&original));
}

#[test]
fn constant_folding_strength_reduces_uint128_power_of_two_arithmetic() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Mul,
            left: v("x"),
            right: TackyVal::UInt128Constant(1_u128 << 96),
            dst: v("mul"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Div,
            left: v("mul"),
            right: TackyVal::UInt128Constant(1_u128 << 96),
            dst: v("div"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Mod,
            left: v("div"),
            right: TackyVal::UInt128Constant(1_u128 << 96),
            dst: v("rem"),
        },
        TackyInstr::Return(v("rem")),
    ]);
    let types = typed_vars(&[
        ("x", CType::UInt128),
        ("mul", CType::UInt128),
        ("div", CType::UInt128),
        ("rem", CType::UInt128),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert_eq!(
        func.body,
        vec![
            TackyInstr::Binary {
                op: TackyBinaryOp::ShiftLeft,
                left: v("x"),
                right: TackyVal::Constant(96),
                dst: v("mul"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::ShiftRight,
                left: v("mul"),
                right: TackyVal::Constant(96),
                dst: v("div"),
            },
            TackyInstr::Binary {
                op: TackyBinaryOp::BitwiseAnd,
                left: v("div"),
                right: TackyVal::UInt128Constant((1_u128 << 96) - 1),
                dst: v("rem"),
            },
            TackyInstr::Return(v("rem")),
        ]
    );
}

#[test]
fn constant_folding_simplifies_integer_all_ones_bitwise_ops() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: v("a"),
            right: TackyVal::Constant(255),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: TackyVal::Constant(-1),
            right: v("a"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left: v("a"),
            right: TackyVal::Constant(255),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseXor,
            left: v("a"),
            right: TackyVal::Constant(255),
            dst: v("__rnqcc_tmp.3"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseNand,
            left: TackyVal::Constant(255),
            right: v("a"),
            dst: v("__rnqcc_tmp.4"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.4")),
    ]);
    let types = typed_vars(&[
        ("a", CType::UChar),
        ("__rnqcc_tmp.0", CType::UChar),
        ("__rnqcc_tmp.1", CType::UChar),
        ("__rnqcc_tmp.2", CType::UChar),
        ("__rnqcc_tmp.3", CType::UChar),
        ("__rnqcc_tmp.4", CType::UChar),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    for dst in ["__rnqcc_tmp.0", "__rnqcc_tmp.1"] {
        assert!(func.body.contains(&TackyInstr::Copy {
            src: v("a"),
            dst: v(dst),
        }));
    }
    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::Constant(255),
        dst: v("__rnqcc_tmp.2"),
    }));
    for dst in ["__rnqcc_tmp.3", "__rnqcc_tmp.4"] {
        assert!(func.body.contains(&TackyInstr::Unary {
            op: TackyUnaryOp::Complement,
            src: v("a"),
            dst: v(dst),
        }));
    }
}

#[test]
fn constant_folding_simplifies_integer_self_comparisons() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Equal,
            left: v("a"),
            right: v("a"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("a"),
            right: v("a"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = int_types(&["a", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::Constant(1),
        dst: v("__rnqcc_tmp.0"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::Constant(0),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn constant_folding_simplifies_integer_same_operand_ops() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Sub,
            left: v("a"),
            right: v("a"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseXor,
            left: v("a"),
            right: v("a"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseAnd,
            left: v("a"),
            right: v("a"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseOr,
            left: v("a"),
            right: v("a"),
            dst: v("__rnqcc_tmp.3"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseNand,
            left: v("a"),
            right: v("a"),
            dst: v("__rnqcc_tmp.4"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.4")),
    ]);
    let types = int_types(&[
        "a",
        "__rnqcc_tmp.0",
        "__rnqcc_tmp.1",
        "__rnqcc_tmp.2",
        "__rnqcc_tmp.3",
        "__rnqcc_tmp.4",
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::Constant(0),
        dst: v("__rnqcc_tmp.0"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::Constant(0),
        dst: v("__rnqcc_tmp.1"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("a"),
        dst: v("__rnqcc_tmp.2"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("a"),
        dst: v("__rnqcc_tmp.3"),
    }));
    assert!(func.body.contains(&TackyInstr::Unary {
        op: TackyUnaryOp::Complement,
        src: v("a"),
        dst: v("__rnqcc_tmp.4"),
    }));
}

#[test]
fn constant_folding_simplifies_integer_zero_left_ops() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Div,
            left: TackyVal::Constant(0),
            right: v("a"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Mod,
            left: TackyVal::Constant(0),
            right: v("a"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::ShiftLeft,
            left: TackyVal::Constant(0),
            right: v("a"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::ShiftRight,
            left: TackyVal::Constant(0),
            right: v("a"),
            dst: v("__rnqcc_tmp.3"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::BitwiseNand,
            left: TackyVal::Constant(0),
            right: v("a"),
            dst: v("__rnqcc_tmp.4"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.4")),
    ]);
    let types = int_types(&[
        "a",
        "__rnqcc_tmp.0",
        "__rnqcc_tmp.1",
        "__rnqcc_tmp.2",
        "__rnqcc_tmp.3",
        "__rnqcc_tmp.4",
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    for dst in [
        "__rnqcc_tmp.0",
        "__rnqcc_tmp.1",
        "__rnqcc_tmp.2",
        "__rnqcc_tmp.3",
    ] {
        assert!(func.body.contains(&TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v(dst),
        }));
    }
    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::Constant(-1),
        dst: v("__rnqcc_tmp.4"),
    }));
}

#[test]
fn constant_folding_preserves_float_multiply_by_zero() {
    let multiply = TackyInstr::Binary {
        op: TackyBinaryOp::Mul,
        left: v("d"),
        right: TackyVal::DoubleConstant(0.0),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        multiply.clone(),
        TackyInstr::Return(v("__rnqcc_tmp.0")),
    ]);
    let types = typed_vars(&[("d", CType::Double), ("__rnqcc_tmp.0", CType::Double)]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&multiply));
}

#[test]
fn constant_folding_preserves_float_zero_left_division() {
    let divide = TackyInstr::Binary {
        op: TackyBinaryOp::Div,
        left: TackyVal::DoubleConstant(0.0),
        right: v("d"),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![divide.clone(), TackyInstr::Return(v("__rnqcc_tmp.0"))]);
    let types = typed_vars(&[("d", CType::Double), ("__rnqcc_tmp.0", CType::Double)]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&divide));
}

#[test]
fn constant_folding_preserves_float_same_operand_subtraction() {
    let subtract = TackyInstr::Binary {
        op: TackyBinaryOp::Sub,
        left: v("d"),
        right: v("d"),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        subtract.clone(),
        TackyInstr::Return(v("__rnqcc_tmp.0")),
    ]);
    let types = typed_vars(&[("d", CType::Double), ("__rnqcc_tmp.0", CType::Double)]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&subtract));
}

#[test]
fn constant_folding_folds_float_width_conversions() {
    let narrowed = 1.0_f64 / 3.0;
    let mut func = empty_function(vec![
        TackyInstr::FloatToDouble {
            src: TackyVal::DoubleConstant(1.25_f32 as f64),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::DoubleToFloat {
            src: TackyVal::DoubleConstant(narrowed),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("__rnqcc_tmp.0", CType::Double),
        ("__rnqcc_tmp.1", CType::Float),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::DoubleConstant(1.25_f32 as f64),
        dst: v("__rnqcc_tmp.0"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: TackyVal::DoubleConstant(narrowed as f32 as f64),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn constant_folding_does_not_reuse_local_after_atomic_exchange() {
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: v("a"),
        },
        TackyInstr::AtomicExchange {
            ptr: v("p"),
            value: TackyVal::Constant(2),
            dst: v("old"),
        },
        TackyInstr::Return(v("a")),
    ]);
    let types = typed_vars(&[
        ("a", CType::Int),
        ("p", CType::Pointer),
        ("old", CType::Int),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Return(v("a"))));
}

#[test]
fn constant_folding_does_not_reuse_local_after_setjmp() {
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: v("a"),
        },
        TackyInstr::BuiltinSetjmp {
            buf: v("env"),
            dst: v("setjmp_result"),
            label: "resume".to_string(),
            end_label: "done".to_string(),
        },
        TackyInstr::Return(v("a")),
    ]);
    let types = typed_vars(&[
        ("a", CType::Int),
        ("env", CType::Pointer),
        ("setjmp_result", CType::Int),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Return(v("a"))));
}

#[test]
fn constant_folding_does_not_reuse_local_after_longjmp() {
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: v("a"),
        },
        TackyInstr::BuiltinLongjmp {
            buf: v("env"),
            value: TackyVal::Constant(1),
        },
        TackyInstr::Return(v("a")),
    ]);
    let types = typed_vars(&[("a", CType::Int), ("env", CType::Pointer)]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Return(v("a"))));
}

#[test]
fn constant_folding_does_not_reuse_local_after_va_start() {
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: v("a"),
        },
        TackyInstr::VaStart { dst: v("ap") },
        TackyInstr::Return(v("a")),
    ]);
    let types = typed_vars(&[("a", CType::Int), ("ap", CType::Pointer)]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Return(v("a"))));
}

#[test]
fn constant_folding_clears_constants_for_address_redefinitions() {
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: v("p0"),
        },
        TackyInstr::GetAddress {
            src: v("a"),
            dst: v("p0"),
        },
        TackyInstr::Return(v("p0")),
        TackyInstr::Copy {
            src: TackyVal::Constant(2),
            dst: v("p1"),
        },
        TackyInstr::LoadLabelAddress("target".to_string(), v("p1")),
        TackyInstr::Return(v("p1")),
        TackyInstr::Copy {
            src: TackyVal::Constant(3),
            dst: v("p2"),
        },
        TackyInstr::FrameAddress { dst: v("p2") },
        TackyInstr::Return(v("p2")),
        TackyInstr::Copy {
            src: TackyVal::Constant(4),
            dst: v("p3"),
        },
        TackyInstr::AddPtr {
            ptr: v("base"),
            index: v("idx"),
            scale: 4,
            dst: v("p3"),
        },
        TackyInstr::Return(v("p3")),
    ]);
    let types = typed_vars(&[
        ("a", CType::Int),
        ("base", CType::Pointer),
        ("idx", CType::Long),
        ("p0", CType::Pointer),
        ("p1", CType::Pointer),
        ("p2", CType::Pointer),
        ("p3", CType::Pointer),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    for name in ["p0", "p1", "p2", "p3"] {
        assert!(func.body.contains(&TackyInstr::Return(v(name))));
    }
}

#[test]
fn constant_folding_canonicalizes_constant_addptr_offsets() {
    let mut func = empty_function(vec![
        TackyInstr::AddPtr {
            ptr: v("base"),
            index: TackyVal::Constant(3),
            scale: 4,
            dst: v("p0"),
        },
        TackyInstr::AddPtr {
            ptr: v("base"),
            index: TackyVal::Constant(-2),
            scale: 8,
            dst: v("p1"),
        },
        TackyInstr::AddPtr {
            ptr: v("base"),
            index: TackyVal::Constant(0),
            scale: 8,
            dst: v("p2"),
        },
        TackyInstr::Return(v("p2")),
    ]);
    let types = typed_vars(&[
        ("base", CType::Pointer),
        ("p0", CType::Pointer),
        ("p1", CType::Pointer),
        ("p2", CType::Pointer),
    ]);

    optimize_function(
        &mut func,
        &flags_with_constant_folding(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::AddPtr {
        ptr: v("base"),
        index: TackyVal::Constant(12),
        scale: 1,
        dst: v("p0"),
    }));
    assert!(func.body.contains(&TackyInstr::AddPtr {
        ptr: v("base"),
        index: TackyVal::Constant(-16),
        scale: 1,
        dst: v("p1"),
    }));
    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("base"),
        dst: v("p2"),
    }));
}

#[test]
fn cleanup_after_copy_propagation_folds_branch_and_removes_dead_block() {
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("cond"),
        },
        TackyInstr::JumpIfZero(v("cond"), "live".to_string()),
        TackyInstr::Return(TackyVal::Constant(1)),
        TackyInstr::Label("live".to_string()),
        TackyInstr::Return(TackyVal::Constant(42)),
    ]);
    let types = int_types(&["cond"]);

    optimize_function(
        &mut func,
        &flags_with_copy_folding_and_unreachable_cleanup(),
        &types,
        &HashSet::new(),
    );

    assert!(!func
        .body
        .contains(&TackyInstr::Return(TackyVal::Constant(1))));
    assert!(func
        .body
        .contains(&TackyInstr::Return(TackyVal::Constant(42))));
}

#[test]
fn copy_propagation_after_dead_store_elimination_resolves_copy_chain() {
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(1),
            dst: v("a"),
        },
        TackyInstr::Copy {
            src: v("a"),
            dst: v("b"),
        },
        TackyInstr::Copy {
            src: TackyVal::Constant(2),
            dst: v("a"),
        },
        TackyInstr::Return(v("b")),
    ]);
    let types = int_types(&["a", "b"]);

    optimize_function(
        &mut func,
        &flags_with_copy_propagation_and_dead_store_elimination(),
        &types,
        &HashSet::new(),
    );

    assert_eq!(func.body, vec![TackyInstr::Return(TackyVal::Constant(1))]);
}

#[test]
fn unreachable_cleanup_removes_fallthrough_after_nonlocal_jump() {
    let mut func = empty_function(vec![
        TackyInstr::NonlocalJump("outer_label".to_string()),
        TackyInstr::Return(TackyVal::Constant(1)),
    ]);

    optimize_function(
        &mut func,
        &flags_with_unreachable_cleanup(),
        &IndexMap::new(),
        &HashSet::new(),
    );

    assert_eq!(
        func.body,
        vec![TackyInstr::NonlocalJump("outer_label".to_string())]
    );
}

#[test]
fn unreachable_cleanup_removes_fallthrough_after_builtin_longjmp() {
    let longjmp = TackyInstr::BuiltinLongjmp {
        buf: v("env"),
        value: TackyVal::Constant(1),
    };
    let mut func = empty_function(vec![
        longjmp.clone(),
        TackyInstr::Return(TackyVal::Constant(1)),
    ]);
    let types = typed_vars(&[("env", CType::Pointer)]);

    optimize_function(
        &mut func,
        &flags_with_unreachable_cleanup(),
        &types,
        &HashSet::new(),
    );

    assert_eq!(func.body, vec![longjmp]);
}

#[test]
fn unreachable_cleanup_threads_jump_chain_and_drops_middle_block() {
    let mut func = empty_function(vec![
        TackyInstr::Jump("mid".to_string()),
        TackyInstr::Return(TackyVal::Constant(1)),
        TackyInstr::Label("mid".to_string()),
        TackyInstr::Jump("end".to_string()),
        TackyInstr::Return(TackyVal::Constant(2)),
        TackyInstr::Label("end".to_string()),
        TackyInstr::Return(TackyVal::Constant(3)),
    ]);

    optimize_function(
        &mut func,
        &flags_with_unreachable_cleanup(),
        &IndexMap::new(),
        &HashSet::new(),
    );

    assert_eq!(
        func.body,
        vec![
            TackyInstr::Label("end".to_string()),
            TackyInstr::Return(TackyVal::Constant(3))
        ]
    );
}

#[test]
fn unreachable_cleanup_keeps_all_blocks_after_indirect_jump() {
    let mut func = empty_function(vec![
        TackyInstr::JumpIndirect(v("target")),
        TackyInstr::Label("case_a".to_string()),
        TackyInstr::Return(TackyVal::Constant(1)),
        TackyInstr::Label("case_b".to_string()),
        TackyInstr::Return(TackyVal::Constant(2)),
    ]);
    let types = typed_vars(&[("target", CType::Pointer)]);

    optimize_function(
        &mut func,
        &flags_with_unreachable_cleanup(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::JumpIndirect(v("target"))));
    assert!(func
        .body
        .contains(&TackyInstr::Return(TackyVal::Constant(1))));
    assert!(func
        .body
        .contains(&TackyInstr::Return(TackyVal::Constant(2))));
}

#[test]
fn copy_from_offset_cse_replaces_duplicate_when_holder_live() {
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.1"),
        },
    ]);

    assert_eq!(
        optimized[1],
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        }
    );
}

#[test]
fn copy_from_offset_cse_does_not_reuse_redefined_holder() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Copy {
            src: TackyVal::Constant(7),
            dst: v("__rnqcc_tmp.0"),
        },
        repeated.clone(),
    ]);

    assert!(optimized.contains(&repeated));
}

#[test]
fn copy_from_offset_cse_does_not_reuse_across_control_flow() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Jump("next".to_string()),
        repeated.clone(),
        TackyInstr::Label("next".to_string()),
    ]);

    assert!(optimized.contains(&repeated));
}

#[test]
fn copy_from_offset_cse_does_not_reuse_across_atomic_write() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::AtomicExchange {
            ptr: v("p"),
            value: TackyVal::Constant(1),
            dst: v("old"),
        },
        repeated.clone(),
    ]);

    assert!(optimized.contains(&repeated));
}

#[test]
fn copy_from_offset_cse_does_not_reuse_after_source_redefinition() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::VaStart { dst: v("s") },
        repeated.clone(),
    ]);

    assert!(optimized.contains(&repeated));
}

#[test]
fn copy_from_offset_cse_does_not_reuse_across_setjmp() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::BuiltinSetjmp {
            buf: v("env"),
            dst: v("setjmp_result"),
            label: "resume".to_string(),
            end_label: "done".to_string(),
        },
        repeated.clone(),
    ]);

    assert!(optimized.contains(&repeated));
}

#[test]
fn copy_from_offset_cse_does_not_reuse_across_va_start() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::VaStart { dst: v("ap") },
        repeated.clone(),
    ]);

    assert!(optimized.contains(&repeated));
}

#[test]
fn copy_from_offset_cse_reuses_across_frame_address() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let optimized = cse_copy_from_offset(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::FrameAddress { dst: v("frame") },
        repeated.clone(),
    ]);

    assert!(optimized.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn licm_hoists_loop_invariant_temp_to_preheader() {
    let invariant = TackyInstr::Binary {
        op: TackyBinaryOp::Add,
        left: v("a"),
        right: TackyVal::Constant(1),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        invariant.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("i")),
    ]);
    let types = int_types(&[
        "a",
        "i",
        "n",
        "__rnqcc_tmp.0",
        "__rnqcc_tmp.1",
        "__rnqcc_tmp.2",
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let invariant_index = instr_index(&func.body, &invariant);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(invariant_index < loop_label_index);
}

#[test]
fn licm_hoists_inlined_loop_invariant_temp_to_preheader() {
    let invariant = TackyInstr::Binary {
        op: TackyBinaryOp::Add,
        left: v("a"),
        right: TackyVal::Constant(1),
        dst: v("__rnqcc_inline.0.__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        invariant.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: v("__rnqcc_inline.0.__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("i")),
    ]);
    let types = int_types(&[
        "a",
        "i",
        "n",
        "__rnqcc_inline.0.__rnqcc_tmp.0",
        "__rnqcc_tmp.1",
        "__rnqcc_tmp.2",
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let invariant_index = instr_index(&func.body, &invariant);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(invariant_index < loop_label_index);
}

#[test]
fn licm_hoists_loop_invariant_get_address_after_source_value_write() {
    let invariant = TackyInstr::GetAddress {
        src: v("a"),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        TackyInstr::Copy {
            src: TackyVal::Constant(7),
            dst: v("a"),
        },
        invariant.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("__rnqcc_tmp.0")),
    ]);
    let types = typed_vars(&[
        ("a", CType::Int),
        ("i", CType::Int),
        ("n", CType::Int),
        ("__rnqcc_tmp.0", CType::Pointer),
        ("__rnqcc_tmp.1", CType::Int),
        ("__rnqcc_tmp.2", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let invariant_index = instr_index(&func.body, &invariant);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(invariant_index < loop_label_index);
}

#[test]
fn licm_hoists_loop_invariant_label_address() {
    let invariant = TackyInstr::LoadLabelAddress("target".to_string(), v("__rnqcc_tmp.0"));
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        invariant.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("__rnqcc_tmp.0")),
    ]);
    let types = typed_vars(&[
        ("i", CType::Int),
        ("n", CType::Int),
        ("__rnqcc_tmp.0", CType::Pointer),
        ("__rnqcc_tmp.1", CType::Int),
        ("__rnqcc_tmp.2", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let invariant_index = instr_index(&func.body, &invariant);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(invariant_index < loop_label_index);
}

#[test]
fn licm_hoists_loop_invariant_frame_address() {
    let invariant = TackyInstr::FrameAddress {
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        invariant.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("__rnqcc_tmp.0")),
    ]);
    let types = typed_vars(&[
        ("i", CType::Int),
        ("n", CType::Int),
        ("__rnqcc_tmp.0", CType::Pointer),
        ("__rnqcc_tmp.1", CType::Int),
        ("__rnqcc_tmp.2", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let invariant_index = instr_index(&func.body, &invariant);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(invariant_index < loop_label_index);
}

#[test]
fn licm_hoists_loop_invariant_copy_from_offset() {
    let invariant = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        invariant.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("i")),
    ]);
    let types = typed_vars(&[
        ("s", CType::Struct),
        ("i", CType::Int),
        ("n", CType::Int),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
        ("__rnqcc_tmp.2", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let invariant_index = instr_index(&func.body, &invariant);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(invariant_index < loop_label_index);
}

#[test]
fn licm_does_not_hoist_copy_from_offset_after_aggregate_write() {
    let field_read = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        TackyInstr::CopyToOffset {
            src: TackyVal::Constant(7),
            dst_name: "s".to_string(),
            offset: 8,
        },
        field_read.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::JumpIfNotZero(v("i"), "loop".to_string()),
        TackyInstr::Return(v("i")),
    ]);
    let types = typed_vars(&[
        ("s", CType::Struct),
        ("i", CType::Int),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let field_read_index = instr_index(&func.body, &field_read);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(field_read_index > loop_label_index);
}

#[test]
fn licm_does_not_hoist_copy_from_offset_from_aliased_aggregate() {
    let field_read = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::GetAddress {
            src: v("s"),
            dst: v("p"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        field_read.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("__rnqcc_tmp.0"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.1"), "loop".to_string()),
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("s", CType::Struct),
        ("p", CType::Pointer),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let field_read_index = instr_index(&func.body, &field_read);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(field_read_index > loop_label_index);
}

#[test]
fn licm_does_not_hoist_without_preheader() {
    let invariant = TackyInstr::Binary {
        op: TackyBinaryOp::Add,
        left: v("a"),
        right: TackyVal::Constant(1),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Label("loop".to_string()),
        invariant.clone(),
        TackyInstr::Jump("loop".to_string()),
    ]);
    let types = int_types(&["a", "__rnqcc_tmp.0"]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let invariant_index = instr_index(&func.body, &invariant);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(invariant_index > loop_label_index);
}

#[test]
fn licm_does_not_hoist_operand_modified_in_loop() {
    let dependent = TackyInstr::Binary {
        op: TackyBinaryOp::Add,
        left: v("a"),
        right: TackyVal::Constant(1),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        TackyInstr::Copy {
            src: TackyVal::Constant(2),
            dst: v("a"),
        },
        dependent.clone(),
        TackyInstr::Jump("loop".to_string()),
    ]);
    let types = int_types(&["a", "__rnqcc_tmp.0"]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let dependent_index = instr_index(&func.body, &dependent);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(dependent_index > loop_label_index);
}

#[test]
fn licm_does_not_hoist_float_to_integer_conversion() {
    let conversion = TackyInstr::DoubleToInt {
        src: v("d"),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        conversion.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("i")),
    ]);
    let types = typed_vars(&[
        ("d", CType::Double),
        ("i", CType::Int),
        ("n", CType::Int),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
        ("__rnqcc_tmp.2", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_licm(), &types, &HashSet::new());

    let conversion_index = instr_index(&func.body, &conversion);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(conversion_index > loop_label_index);
}

#[test]
fn licm_hoisted_copy_feeds_followup_copy_propagation() {
    let invariant_copy = TackyInstr::Copy {
        src: v("a"),
        dst: v("__rnqcc_tmp.0"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Copy {
            src: TackyVal::Constant(0),
            dst: v("i"),
        },
        TackyInstr::Jump("loop".to_string()),
        TackyInstr::Label("loop".to_string()),
        invariant_copy.clone(),
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("i"),
            right: v("__rnqcc_tmp.0"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Copy {
            src: v("__rnqcc_tmp.1"),
            dst: v("i"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::LessThan,
            left: v("i"),
            right: v("n"),
            dst: v("__rnqcc_tmp.2"),
        },
        TackyInstr::JumpIfNotZero(v("__rnqcc_tmp.2"), "loop".to_string()),
        TackyInstr::Return(v("i")),
    ]);
    let types = int_types(&[
        "a",
        "i",
        "n",
        "__rnqcc_tmp.0",
        "__rnqcc_tmp.1",
        "__rnqcc_tmp.2",
    ]);

    optimize_function(
        &mut func,
        &flags_with_copy_propagation_and_licm(),
        &types,
        &HashSet::new(),
    );

    let copy_index = instr_index(&func.body, &invariant_copy);
    let loop_label_index = instr_index(&func.body, &TackyInstr::Label("loop".to_string()));
    assert!(copy_index < loop_label_index);
    assert!(func.body.contains(&TackyInstr::Binary {
        op: TackyBinaryOp::Add,
        left: v("i"),
        right: v("a"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_replaces_repeated_binary_in_basic_block() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("a"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("a"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = int_types(&["a", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_inserted_copy_feeds_followup_copy_propagation() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("a"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("a"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = int_types(&["a", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]);

    optimize_function(
        &mut func,
        &flags_with_copy_propagation_and_cse(),
        &types,
        &HashSet::new(),
    );

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
    assert!(func.body.contains(&TackyInstr::Return(v("__rnqcc_tmp.0"))));
}

#[test]
fn cse_replaces_commuted_integer_binary_in_basic_block() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("a"),
            right: v("b"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("b"),
            right: v("a"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = int_types(&["a", "b", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_replaces_repeated_addptr_in_basic_block() {
    let mut func = empty_function(vec![
        TackyInstr::AddPtr {
            ptr: v("base"),
            index: v("idx"),
            scale: 4,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::AddPtr {
            ptr: v("base"),
            index: v("idx"),
            scale: 4,
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("base", CType::Pointer),
        ("idx", CType::Long),
        ("__rnqcc_tmp.0", CType::Pointer),
        ("__rnqcc_tmp.1", CType::Pointer),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_replaces_repeated_get_address_after_source_value_write() {
    let mut func = empty_function(vec![
        TackyInstr::GetAddress {
            src: v("a"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Copy {
            src: TackyVal::Constant(7),
            dst: v("a"),
        },
        TackyInstr::GetAddress {
            src: v("a"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("a", CType::Int),
        ("__rnqcc_tmp.0", CType::Pointer),
        ("__rnqcc_tmp.1", CType::Pointer),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_replaces_repeated_frame_address_in_basic_block() {
    let mut func = empty_function(vec![
        TackyInstr::FrameAddress {
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::FrameAddress {
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("__rnqcc_tmp.0", CType::Pointer),
        ("__rnqcc_tmp.1", CType::Pointer),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_replaces_repeated_label_address_in_basic_block() {
    let mut func = empty_function(vec![
        TackyInstr::LoadLabelAddress("target".to_string(), v("__rnqcc_tmp.0")),
        TackyInstr::LoadLabelAddress("target".to_string(), v("__rnqcc_tmp.1")),
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("__rnqcc_tmp.0", CType::Pointer),
        ("__rnqcc_tmp.1", CType::Pointer),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_uses_expression_available_from_predecessor_block() {
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Mul,
            left: v("a"),
            right: v("b"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Jump("next".to_string()),
        TackyInstr::Label("next".to_string()),
        TackyInstr::Binary {
            op: TackyBinaryOp::Mul,
            left: v("a"),
            right: v("b"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = int_types(&["a", "b", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_does_not_reuse_expression_after_operand_redefinition() {
    let repeated = TackyInstr::Binary {
        op: TackyBinaryOp::Add,
        left: v("a"),
        right: TackyVal::Constant(1),
        dst: v("__rnqcc_tmp.1"),
    };
    let mut func = empty_function(vec![
        TackyInstr::Binary {
            op: TackyBinaryOp::Add,
            left: v("a"),
            right: TackyVal::Constant(1),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Copy {
            src: TackyVal::Constant(3),
            dst: v("a"),
        },
        repeated.clone(),
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = int_types(&["a", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&repeated));
}

#[test]
fn cse_replaces_repeated_scalar_load_in_basic_block() {
    let mut func = empty_function(vec![
        TackyInstr::Load {
            src_ptr: v("p"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Load {
            src_ptr: v("p"),
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("p", CType::Pointer),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_replaces_repeated_copy_from_offset_in_basic_block() {
    let mut func = empty_function(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("s", CType::Struct),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_replaces_copy_from_offset_available_from_predecessor_block() {
    let mut func = empty_function(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Jump("next".to_string()),
        TackyInstr::Label("next".to_string()),
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.1"),
        },
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("s", CType::Struct),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&TackyInstr::Copy {
        src: v("__rnqcc_tmp.0"),
        dst: v("__rnqcc_tmp.1"),
    }));
}

#[test]
fn cse_does_not_reuse_copy_from_offset_after_aggregate_write() {
    let repeated = TackyInstr::CopyFromOffset {
        src_name: "s".to_string(),
        offset: 8,
        dst: v("__rnqcc_tmp.1"),
    };
    let mut func = empty_function(vec![
        TackyInstr::CopyFromOffset {
            src_name: "s".to_string(),
            offset: 8,
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::CopyToOffset {
            src: TackyVal::Constant(7),
            dst_name: "s".to_string(),
            offset: 8,
        },
        repeated.clone(),
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("s", CType::Struct),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&repeated));
}

#[test]
fn cse_does_not_reuse_scalar_load_after_aliased_write() {
    let repeated = TackyInstr::Load {
        src_ptr: v("p"),
        dst: v("__rnqcc_tmp.1"),
    };
    let mut func = empty_function(vec![
        TackyInstr::GetAddress {
            src: v("a"),
            dst: v("p"),
        },
        TackyInstr::Load {
            src_ptr: v("p"),
            dst: v("__rnqcc_tmp.0"),
        },
        TackyInstr::Copy {
            src: TackyVal::Constant(7),
            dst: v("a"),
        },
        repeated.clone(),
        TackyInstr::Return(v("__rnqcc_tmp.1")),
    ]);
    let types = typed_vars(&[
        ("a", CType::Int),
        ("p", CType::Pointer),
        ("__rnqcc_tmp.0", CType::Int),
        ("__rnqcc_tmp.1", CType::Int),
    ]);

    optimize_function(&mut func, &flags_with_cse(), &types, &HashSet::new());

    assert!(func.body.contains(&repeated));
}

#[test]
fn inline_functions_expands_small_straight_line_callee() {
    let callee = TackyFunction {
        name: "add_one".to_string(),
        return_type: CType::Int,
        params: vec!["x".to_string()],
        global: false,
        body: vec![
            TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: v("x"),
                right: TackyVal::Constant(1),
                dst: v("__rnqcc_tmp.0"),
            },
            TackyInstr::Return(v("__rnqcc_tmp.0")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let caller = TackyFunction {
        name: "main".to_string(),
        return_type: CType::Int,
        params: Vec::new(),
        global: true,
        body: vec![
            TackyInstr::FunCall {
                name: "add_one".to_string(),
                args: vec![TackyVal::Constant(41)],
                dst: v("__rnqcc_tmp.1"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 1,
                hidden_return: false,
                indirect: false,
            },
            TackyInstr::Return(v("__rnqcc_tmp.1")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let mut program = program_with_functions(
        vec![callee, caller],
        int_types(&["x", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]),
    );

    optimize_program(&mut program, &flags_with_inlining());

    let main = function_at(&program, 1);
    assert!(!main
        .body
        .iter()
        .any(|instr| matches!(instr, TackyInstr::FunCall { .. })));
    assert!(main.body.contains(&TackyInstr::Binary {
        op: TackyBinaryOp::Add,
        left: v("__rnqcc_inline.0.x"),
        right: TackyVal::Constant(1),
        dst: v("__rnqcc_inline.0.__rnqcc_tmp.0"),
    }));
    assert_eq!(
        program.symbol_types.get("__rnqcc_inline.0.x"),
        Some(&CType::Int)
    );
}

#[test]
fn inline_functions_leaves_global_callee_unchanged() {
    let callee = TackyFunction {
        name: "add_one".to_string(),
        return_type: CType::Int,
        params: vec!["x".to_string()],
        global: true,
        body: vec![
            TackyInstr::Binary {
                op: TackyBinaryOp::Add,
                left: v("x"),
                right: TackyVal::Constant(1),
                dst: v("__rnqcc_tmp.0"),
            },
            TackyInstr::Return(v("__rnqcc_tmp.0")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let caller = TackyFunction {
        name: "main".to_string(),
        return_type: CType::Int,
        params: Vec::new(),
        global: true,
        body: vec![
            TackyInstr::FunCall {
                name: "add_one".to_string(),
                args: vec![TackyVal::Constant(41)],
                dst: v("__rnqcc_tmp.1"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 1,
                hidden_return: false,
                indirect: false,
            },
            TackyInstr::Return(v("__rnqcc_tmp.1")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let mut program = program_with_functions(
        vec![callee, caller],
        int_types(&["x", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]),
    );

    optimize_program(&mut program, &flags_with_inlining());

    let main = function_at(&program, 1);
    assert!(main
        .body
        .iter()
        .any(|instr| matches!(instr, TackyInstr::FunCall { name, .. } if name == "add_one")));
}

#[test]
fn inline_functions_leaves_self_call_unchanged() {
    let recursive = TackyFunction {
        name: "self_call".to_string(),
        return_type: CType::Int,
        params: Vec::new(),
        global: false,
        body: vec![
            TackyInstr::FunCall {
                name: "self_call".to_string(),
                args: Vec::new(),
                dst: v("__rnqcc_tmp.0"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 0,
                hidden_return: false,
                indirect: false,
            },
            TackyInstr::Return(v("__rnqcc_tmp.0")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let mut program = program_with_functions(vec![recursive], int_types(&["__rnqcc_tmp.0"]));

    optimize_program(&mut program, &flags_with_inlining());

    let func = function_at(&program, 0);
    assert!(func
        .body
        .iter()
        .any(|instr| matches!(instr, TackyInstr::FunCall { name, .. } if name == "self_call")));
}

#[test]
fn ipcp_rewrites_internal_callee_constant_parameter_uses() {
    let callee = TackyFunction {
        name: "scale".to_string(),
        return_type: CType::Int,
        params: vec!["x".to_string(), "factor".to_string()],
        global: false,
        body: vec![
            TackyInstr::Binary {
                op: TackyBinaryOp::Mul,
                left: v("x"),
                right: v("factor"),
                dst: v("__rnqcc_tmp.0"),
            },
            TackyInstr::Return(v("__rnqcc_tmp.0")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let caller = TackyFunction {
        name: "main".to_string(),
        return_type: CType::Int,
        params: Vec::new(),
        global: true,
        body: vec![
            TackyInstr::FunCall {
                name: "scale".to_string(),
                args: vec![v("a"), TackyVal::Constant(3)],
                dst: v("__rnqcc_tmp.1"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 2,
                hidden_return: false,
                indirect: false,
            },
            TackyInstr::Return(v("__rnqcc_tmp.1")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let mut program = program_with_functions(
        vec![callee, caller],
        int_types(&["x", "factor", "a", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]),
    );

    optimize_program(&mut program, &flags_with_ipcp());

    let scale = function_at(&program, 0);
    assert!(scale.body.contains(&TackyInstr::Binary {
        op: TackyBinaryOp::Mul,
        left: v("x"),
        right: TackyVal::Constant(3),
        dst: v("__rnqcc_tmp.0"),
    }));
}

#[test]
fn ipcp_keeps_parameter_when_call_sites_disagree() {
    let callee = TackyFunction {
        name: "scale".to_string(),
        return_type: CType::Int,
        params: vec!["factor".to_string()],
        global: false,
        body: vec![TackyInstr::Return(v("factor"))],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let caller = TackyFunction {
        name: "main".to_string(),
        return_type: CType::Int,
        params: Vec::new(),
        global: true,
        body: vec![
            TackyInstr::FunCall {
                name: "scale".to_string(),
                args: vec![TackyVal::Constant(3)],
                dst: v("__rnqcc_tmp.0"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 1,
                hidden_return: false,
                indirect: false,
            },
            TackyInstr::FunCall {
                name: "scale".to_string(),
                args: vec![TackyVal::Constant(4)],
                dst: v("__rnqcc_tmp.1"),
                stack_arg_indices: HashSet::new(),
                memory_arg_blocks: Vec::new(),
                struct_arg_groups: Vec::new(),
                variadic: false,
                fixed_flat_arg_count: 1,
                hidden_return: false,
                indirect: false,
            },
            TackyInstr::Return(v("__rnqcc_tmp.1")),
        ],
        stack_params: HashSet::new(),
        memory_param_blocks: Vec::new(),
        struct_param_groups: Vec::new(),
    };
    let mut program = program_with_functions(
        vec![callee, caller],
        int_types(&["factor", "__rnqcc_tmp.0", "__rnqcc_tmp.1"]),
    );

    optimize_program(&mut program, &flags_with_ipcp());

    let scale = function_at(&program, 0);
    assert!(scale.body.contains(&TackyInstr::Return(v("factor"))));
}
