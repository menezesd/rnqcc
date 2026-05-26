typedef long regoff_t;

typedef struct {
    size_t re_nsub;
} regex_t;

typedef struct {
    regoff_t rm_so;
    regoff_t rm_eo;
} regmatch_t;

int regcomp(regex_t *, const char *, int);
int regexec(const regex_t *, const char *, size_t, regmatch_t [], int);
size_t regerror(int, const regex_t *, char *, size_t);
void regfree(regex_t *);
