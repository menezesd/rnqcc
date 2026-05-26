typedef struct {
    size_t gl_pathc;
    char **gl_pathv;
    size_t gl_offs;
} glob_t;

int glob(const char *, int, int (*)(const char *, int), glob_t *);
void globfree(glob_t *);
