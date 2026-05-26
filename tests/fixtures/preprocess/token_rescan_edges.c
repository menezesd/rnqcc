#define OBJ_COUNTER __COUNTER__
#define FN_COUNTER(x) x + __COUNTER__
#define IDENTITY(x) x
#define ADD(a, b) a + b
#define CAT(a, b) a ## b
#define KW_INT in ## t
#define MAKE_MEMBER(name) object. name
#define HIDDEN_COUNTER OBJ_COUNTER

int object_counter_first = OBJ_COUNTER;
int object_counter_second = HIDDEN_COUNTER;
int function_counter_literal = FN_COUNTER(40);
int function_counter_arg = FN_COUNTER(__COUNTER__);
char *literal_arg = IDENTITY("x /* not a comment */ y");
int comment_arg = ADD(20 /* comment with , and ) */, 22);
KW_INT CAT(key, word_value) = 7;
int pasted_identifier_adjacent = CAT(alpha, 42)+CAT(beta, _value);
int member_paste = MAKE_MEMBER(CAT(field, _name));

#if 0
#define INACTIVE_COUNTER __COUNTER__
int inactive_value = __COUNTER__;
#else
int active_counter = __COUNTER__;
#endif

#ifdef INACTIVE_COUNTER
int inactive_define_leaked = INACTIVE_COUNTER;
#endif
