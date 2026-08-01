fn probe_name_in(name: &str, probes: &[&str]) -> bool {
    probes.contains(&name)
}

const CORE_BUILTIN_PROBES: &[&str] = &[
    "__builtin_expect",
    "__builtin_expect_with_probability",
    "__builtin_types_compatible_p",
    "__builtin_choose_expr",
    "__builtin_offsetof",
    "__builtin_convertvector",
    "__builtin_shuffle",
    "__builtin_constant_p",
    "__builtin_classify_type",
    "__builtin_signbit",
    "__builtin_assume_aligned",
    "__builtin_prefetch",
    "__builtin_object_size",
    "__builtin_dynamic_object_size",
];

const NUMERIC_BUILTIN_PROBES: &[&str] = &[
    "__builtin_inf",
    "__builtin_inff",
    "__builtin_infl",
    "__builtin_huge_val",
    "__builtin_huge_valf",
    "__builtin_huge_vall",
    "__builtin_isinf",
    "__builtin_isinff",
    "__builtin_isinfl",
    "__builtin_bswap16",
    "__builtin_bswap32",
    "__builtin_bswap64",
    "__builtin_ffs",
    "__builtin_ffsl",
    "__builtin_ffsll",
    "__builtin_clz",
    "__builtin_clzl",
    "__builtin_clzll",
    "__builtin_ctz",
    "__builtin_ctzl",
    "__builtin_ctzll",
    "__builtin_clrsb",
    "__builtin_clrsbl",
    "__builtin_clrsbll",
    "__builtin_popcount",
    "__builtin_popcountl",
    "__builtin_popcountll",
    "__builtin_parity",
    "__builtin_parityl",
    "__builtin_parityll",
    "__builtin_add_overflow",
    "__builtin_sub_overflow",
    "__builtin_mul_overflow",
    "__builtin_mul_overflow_p",
    "__builtin_abs",
    "__builtin_labs",
    "__builtin_llabs",
    "__builtin_fabs",
    "__builtin_fabsf",
    "__builtin_fabsl",
    "__builtin_copysign",
    "__builtin_copysignf",
    "__builtin_copysignl",
    "__builtin_pow",
    "__builtin_powf",
    "__builtin_conj",
    "__builtin_conjf",
    "__builtin_conjl",
    "__builtin_sqrtl",
    "__builtin_atan2l",
];

const MEMORY_STRING_BUILTIN_PROBES: &[&str] = &[
    "__builtin_memcpy",
    "__builtin_memmove",
    "__builtin_memset",
    "__builtin_memcmp",
    "__builtin_memchr",
    "__builtin_mempcpy",
    "__builtin_strlen",
    "__builtin_strcmp",
    "__builtin_strncmp",
    "__builtin_strchr",
    "__builtin_strrchr",
    "__builtin_strstr",
    "__builtin_strspn",
    "__builtin_strcspn",
    "__builtin_strcpy",
    "__builtin_stpcpy",
    "__builtin_strncpy",
    "__builtin_strcat",
    "__builtin_strncat",
    "__builtin___sprintf_chk",
    "__builtin___memcpy_chk",
    "__builtin___memmove_chk",
    "__builtin___memset_chk",
    "__builtin___strcpy_chk",
    "__builtin___stpcpy_chk",
    "__builtin___strncpy_chk",
    "__builtin___strcat_chk",
    "__builtin___strncat_chk",
];

const CONTROL_AND_CALL_BUILTIN_PROBES: &[&str] = &[
    "__builtin_unreachable",
    "__builtin_trap",
    "__builtin_abort",
    "__builtin_exit",
    "__builtin_printf",
    "__builtin_sprintf",
    "__builtin_snprintf",
    "__builtin_puts",
    "__builtin_apply",
    "__builtin_apply_args",
    "__builtin_return_address",
    "__builtin_frame_address",
    "__builtin_extract_return_addr",
    "__builtin_alloca",
    "__builtin_malloc",
    "__builtin_free",
];

const VARARG_BUILTIN_PROBES: &[&str] = &[
    "__builtin_setjmp",
    "__builtin_longjmp",
    "__builtin_va_start",
    "__builtin_va_end",
    "__builtin_va_copy",
    "__builtin_va_arg",
    "__builtin_va_arg_pack",
    "__va_copy",
];

const ATOMIC_BUILTIN_PROBES: &[&str] = &[
    "__atomic_load_n",
    "__atomic_store_n",
    "__atomic_exchange_n",
    "__atomic_compare_exchange_n",
    "__atomic_thread_fence",
    "__atomic_signal_fence",
    "__atomic_add_fetch",
    "__atomic_sub_fetch",
    "__atomic_and_fetch",
    "__atomic_or_fetch",
    "__atomic_xor_fetch",
    "__atomic_fetch_add",
    "__atomic_fetch_sub",
    "__atomic_fetch_and",
    "__atomic_fetch_nand",
    "__atomic_fetch_or",
    "__atomic_fetch_xor",
    "__atomic_nand_fetch",
    "__sync_add_and_fetch",
    "__sync_sub_and_fetch",
    "__sync_and_and_fetch",
    "__sync_nand_and_fetch",
    "__sync_or_and_fetch",
    "__sync_xor_and_fetch",
    "__sync_fetch_and_add",
    "__sync_fetch_and_sub",
    "__sync_fetch_and_and",
    "__sync_fetch_and_nand",
    "__sync_fetch_and_or",
    "__sync_fetch_and_xor",
    "__sync_bool_compare_and_swap",
    "__sync_val_compare_and_swap",
    "__sync_synchronize",
];

pub fn has_builtin(name: &str) -> bool {
    has_core_builtin(name)
        || has_numeric_builtin(name)
        || has_memory_string_builtin(name)
        || has_control_and_call_builtin(name)
        || has_vararg_builtin(name)
        || has_atomic_builtin(name)
}

fn has_core_builtin(name: &str) -> bool {
    probe_name_in(name, CORE_BUILTIN_PROBES)
}

fn has_numeric_builtin(name: &str) -> bool {
    probe_name_in(name, NUMERIC_BUILTIN_PROBES)
}

fn has_memory_string_builtin(name: &str) -> bool {
    probe_name_in(name, MEMORY_STRING_BUILTIN_PROBES)
}

fn has_control_and_call_builtin(name: &str) -> bool {
    probe_name_in(name, CONTROL_AND_CALL_BUILTIN_PROBES)
}

fn has_vararg_builtin(name: &str) -> bool {
    probe_name_in(name, VARARG_BUILTIN_PROBES)
}

fn has_atomic_builtin(name: &str) -> bool {
    probe_name_in(name, ATOMIC_BUILTIN_PROBES)
}

const LAYOUT_OR_CODEGEN_ATTRIBUTE_PROBES: &[&str] = &[
    "aligned",
    "align",
    "alias",
    "mode",
    "no_instrument_function",
    "noreturn",
    "packed",
    "scalar_storage_order",
    "transparent_union",
    "vector_size",
];

const OPTIMIZER_HINT_ATTRIBUTE_PROBES: &[&str] = &[
    "always_inline",
    "cold",
    "const",
    "hot",
    "malloc",
    "noinline",
    "nonnull",
    "pure",
    "returns_nonnull",
    "warn_unused_result",
];

const COMPAT_NOOP_ATTRIBUTE_PROBES: &[&str] = &[
    "alloc_align",
    "alloc_size",
    "deprecated",
    "fallthrough",
    "format",
    "format_arg",
    "format_strfmon",
    "gnu_inline",
    "section",
    "unavailable",
    "unused",
    "used",
    "visibility",
    "weak",
];

const C_ATTRIBUTE_PROBES: &[&str] = &[
    "deprecated",
    "fallthrough",
    "maybe_unused",
    "nodiscard",
    "noreturn",
    "reproducible",
    "unsequenced",
    "clang::fallthrough",
    "clang::noreturn",
    "clang::unused",
    "gnu::noreturn",
    "gnu::unused",
    "gcc::noreturn",
    "gcc::unused",
];

const DECLSPEC_ATTRIBUTE_PROBES: &[&str] =
    &["align", "deprecated", "dllexport", "dllimport", "noreturn"];

const C_LANGUAGE_FEATURE_PROBES: &[&str] = &[
    "c_alignas",
    "c_alignof",
    "c_atomic",
    "c_bitint",
    "c_embed",
    "c_generic_selection_with_controlling_type",
    "c_generic_selections",
    "c_static_assert",
    "c_thread_local",
    "c_variadic_macros",
];

const ATTRIBUTE_FEATURE_PROBES: &[&str] = &[
    "attribute_deprecated_with_message",
    "attribute_unavailable_with_message",
];

const COMPAT_FEATURE_PROBES: &[&str] = &["nullability"];

const WARNING_PROBES: &[&str] = &[
    "-Wall",
    "-Wextra",
    "-Wpedantic",
    "-Wunreachable",
    "-Wmissing-return",
    "-Werror",
    "-Wunknown-pragmas",
    "-Wcompare-distinct-pointer-types",
    "-Wdeprecated-declarations",
];

pub fn has_attribute(name: &str) -> bool {
    let name = probe_name_without_outer_underscores(name);
    has_layout_or_codegen_attribute(name)
        || has_optimizer_hint_attribute(name)
        || has_compat_noop_attribute(name)
}

fn has_layout_or_codegen_attribute(name: &str) -> bool {
    probe_name_in(name, LAYOUT_OR_CODEGEN_ATTRIBUTE_PROBES)
}

fn has_optimizer_hint_attribute(name: &str) -> bool {
    probe_name_in(name, OPTIMIZER_HINT_ATTRIBUTE_PROBES)
}

fn has_compat_noop_attribute(name: &str) -> bool {
    probe_name_in(name, COMPAT_NOOP_ATTRIBUTE_PROBES)
}

pub fn has_c_attribute(name: &str) -> bool {
    let normalized;
    let name = if let Some((namespace, attribute)) = name.split_once("::") {
        normalized = format!(
            "{}::{}",
            c_attribute_namespace_alias(namespace),
            probe_name_without_outer_underscores(attribute)
        );
        normalized.as_str()
    } else {
        probe_name_without_outer_underscores(name)
    };
    probe_name_in(name, C_ATTRIBUTE_PROBES)
}

fn c_attribute_namespace_alias(namespace: &str) -> &str {
    match probe_name_without_outer_underscores(namespace) {
        "_Clang" => "clang",
        other => other,
    }
}

pub fn has_declspec_attribute(name: &str) -> bool {
    let name = probe_name_without_outer_underscores(name);
    probe_name_in(name, DECLSPEC_ATTRIBUTE_PROBES)
}

pub fn has_feature(name: &str) -> bool {
    let name = probe_name_without_outer_underscores(name);
    has_c_language_feature(name) || has_attribute_feature(name) || has_compat_feature(name)
}

fn probe_name_without_outer_underscores(name: &str) -> &str {
    name.strip_prefix("__")
        .and_then(|inner| inner.strip_suffix("__"))
        .unwrap_or(name)
}

fn has_c_language_feature(name: &str) -> bool {
    probe_name_in(name, C_LANGUAGE_FEATURE_PROBES)
}

fn has_attribute_feature(name: &str) -> bool {
    probe_name_in(name, ATTRIBUTE_FEATURE_PROBES)
}

fn has_compat_feature(name: &str) -> bool {
    probe_name_in(name, COMPAT_FEATURE_PROBES)
}

pub fn has_extension(name: &str) -> bool {
    has_feature(name)
}

pub fn has_warning(name: &str) -> bool {
    probe_name_in(name, WARNING_PROBES)
}

const LEXED_KEYWORD_PROBES: &[&str] = &[
    "auto",
    "break",
    "case",
    "char",
    "const",
    "__const",
    "__const__",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "__inline",
    "__inline__",
    "int",
    "long",
    "register",
    "restrict",
    "__restrict",
    "__restrict__",
    "return",
    "short",
    "signed",
    "__signed",
    "__signed__",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "typeof",
    "__typeof",
    "__typeof__",
    "typeof_unqual",
    "__typeof_unqual",
    "__typeof_unqual__",
    "union",
    "unsigned",
    "void",
    "volatile",
    "__volatile",
    "__volatile__",
    "while",
    "_Alignas",
    "alignas",
    "_Alignof",
    "alignof",
    "__alignof",
    "__alignof__",
    "_Atomic",
    "_Bool",
    "_Generic",
    "_Noreturn",
    "_Static_assert",
    "static_assert",
    "_Thread_local",
    "thread_local",
    "__thread",
    "__auto_type",
];

const PARSER_RESERVED_TYPE_NAME_PROBES: &[&str] = &[
    "_BitInt",
    "_Complex",
    "__complex",
    "__complex__",
    "__int128",
    "__int128__",
];

const PARSER_RESERVED_FLOAT_TYPE_NAME_PROBES: &[&str] = &[
    "_Float16",
    "_Float32",
    "_Float64",
    "_Float128",
    "_Float32x",
    "_Float64x",
    "_Float128x",
    "_Decimal32",
    "_Decimal64",
    "_Decimal128",
    "__bf16",
    "__float128",
    "__float80",
    "__fp16",
];

const SKIPPED_EXTENSION_TOKEN_PROBES: &[&str] = &[
    "__extension__",
    "_Nullable",
    "_Nonnull",
    "_Null_unspecified",
    "__asm",
    "__asm__",
    "asm",
    "__attribute",
    "__attribute__",
    "__declspec",
];

pub fn is_identifier(name: &str) -> bool {
    !(is_lexed_keyword(name)
        || is_parser_reserved_type_name(name)
        || is_parser_reserved_float_type_name(name)
        || is_skipped_extension_token(name))
}

fn is_lexed_keyword(name: &str) -> bool {
    probe_name_in(name, LEXED_KEYWORD_PROBES)
}

fn is_parser_reserved_type_name(name: &str) -> bool {
    probe_name_in(name, PARSER_RESERVED_TYPE_NAME_PROBES)
}

fn is_parser_reserved_float_type_name(name: &str) -> bool {
    probe_name_in(name, PARSER_RESERVED_FLOAT_TYPE_NAME_PROBES)
}

fn is_skipped_extension_token(name: &str) -> bool {
    probe_name_in(name, SKIPPED_EXTENSION_TOKEN_PROBES)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn assert_no_duplicates(name: &str, probes: &[&str]) {
        let mut seen = HashSet::new();
        for probe in probes {
            assert!(seen.insert(*probe), "{name} repeats {probe}");
        }
    }

    #[test]
    fn probe_tables_do_not_repeat_entries() {
        for (name, probes) in [
            ("core builtins", CORE_BUILTIN_PROBES),
            ("numeric builtins", NUMERIC_BUILTIN_PROBES),
            ("memory builtins", MEMORY_STRING_BUILTIN_PROBES),
            ("control builtins", CONTROL_AND_CALL_BUILTIN_PROBES),
            ("vararg builtins", VARARG_BUILTIN_PROBES),
            ("atomic builtins", ATOMIC_BUILTIN_PROBES),
            ("layout attributes", LAYOUT_OR_CODEGEN_ATTRIBUTE_PROBES),
            ("optimizer attributes", OPTIMIZER_HINT_ATTRIBUTE_PROBES),
            ("compat attributes", COMPAT_NOOP_ATTRIBUTE_PROBES),
            ("C attributes", C_ATTRIBUTE_PROBES),
            ("declspec attributes", DECLSPEC_ATTRIBUTE_PROBES),
            ("C language features", C_LANGUAGE_FEATURE_PROBES),
            ("attribute features", ATTRIBUTE_FEATURE_PROBES),
            ("compat features", COMPAT_FEATURE_PROBES),
            ("warnings", WARNING_PROBES),
            ("lexed keywords", LEXED_KEYWORD_PROBES),
            ("reserved type names", PARSER_RESERVED_TYPE_NAME_PROBES),
            (
                "reserved float type names",
                PARSER_RESERVED_FLOAT_TYPE_NAME_PROBES,
            ),
            ("skipped extension tokens", SKIPPED_EXTENSION_TOKEN_PROBES),
        ] {
            assert_no_duplicates(name, probes);
        }
    }

    #[test]
    fn normalized_probe_names_work_without_table_aliases() {
        assert!(has_attribute("__unused__"));
        assert!(!COMPAT_NOOP_ATTRIBUTE_PROBES.contains(&"__unused__"));
        assert!(has_declspec_attribute("__dllexport__"));
        assert!(has_feature("__c_static_assert__"));
        assert!(has_extension(
            "__c_generic_selection_with_controlling_type__"
        ));
        assert!(has_c_attribute("_Clang::fallthrough"));
        assert!(has_c_attribute("clang::__fallthrough__"));
    }

    #[test]
    fn unsupported_call_like_names_stay_false() {
        assert!(!has_builtin("__builtin_stack_save"));
        assert!(!has_builtin("__builtin_stack_restore"));
        assert!(!has_builtin("__builtin_rnqcc_missing"));
        assert!(!has_warning("-Wrnqcc-missing-warning"));
    }
}
