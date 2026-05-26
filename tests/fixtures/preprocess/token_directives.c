%:define DIGRAPH_VALUE 11
#define PUNCT_VALUE 31
#define LINE_NUMBER 123
#define LINE_FILE "macro_line.c"
#define SPACING_FLAG 1

int digraph_define_value = DIGRAPH_VALUE;

#line LINE_NUMBER LINE_FILE
int line_from_macro = __LINE__;
char *file_from_macro = __FILE__;

# 777 "marker_flags.c" 1 3
int line_from_marker_flags = __LINE__;
char *file_from_marker_flags = __FILE__;
int include_level_from_marker_flags = __INCLUDE_LEVEL__;

# 50 "builtin_after_marker.c"
int line_after_marker = __LINE__;
char *file_after_marker = __FILE__;

int adjacent_values[] = {
    PUNCT_VALUE,
    PUNCT_VALUE+1,
    (PUNCT_VALUE),
    PUNCT_VALUE/* comment */+DIGRAPH_VALUE
};

#if /* leading */ defined ( SPACING_FLAG ) /* middle */ && SPACING_FLAG == 1
# define SPACED_VALUE 42
#else
# define SPACED_VALUE 0
#endif

#if 0
int conditional_value = 0;
# else /* branch selected with spacing and a trailing comment */
int conditional_value = SPACED_VALUE;
# endif
