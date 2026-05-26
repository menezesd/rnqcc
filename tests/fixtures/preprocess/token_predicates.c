#define SPACED_FLAG 1
#define HEADER_NAME "token_include_header.h"
#define HEADER_ID(x) x
#define HEADER_PICK(first, second) first

#if defined/**/(SPACED_FLAG) && defined /* comment */ SPACED_FLAG
int defined_spacing = 1;
#else
int defined_spacing = 0;
#endif

#if __has_include(HEADER_NAME)
int has_include_object_macro = 1;
#else
int has_include_object_macro = 0;
#endif

#if __has_include(HEADER_ID("token_include_header.h"))
int has_include_function_macro = 1;
#else
int has_include_function_macro = 0;
#endif

#if __has_include(HEADER_PICK("token_include_header.h", (unused, tokens)))
int has_include_nested_macro_arg = 1;
#else
int has_include_nested_macro_arg = 0;
#endif

#if 0
# if __has_include(INVALID_IN_INACTIVE_PARENT)
int inactive_has_include_evaluated = 1;
# endif
# if defined INVALID_IN_INACTIVE_PARENT
int inactive_defined_evaluated = 1;
# endif
#else
int inactive_parent_skipped_predicates = 1;
#endif
