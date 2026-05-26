#define STR(x) #x
#define CAT(a, b) a ## b
#define SUM(first, ...) first + __VA_ARGS__
#define CALL(first, ...) call(first, ##__VA_ARGS__)
#define VA_WRAP(fmt, ...) call(fmt __VA_OPT__(,) __VA_ARGS__)
int CAT(add, _two)(int a, int b) { return a + b; }
char *text = STR(alpha    + beta);
int total = SUM(39, 1 + 2);
int only = CALL(1);
int many = CALL(1, 2, 3);
int opt_empty = VA_WRAP(7);
int opt_many = VA_WRAP(7, 8, 9);
