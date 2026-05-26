extern char *optarg;
extern int opterr;
extern int optind;
extern int optopt;

struct option {
    const char *name;
    int has_arg;
    int *flag;
    int val;
};

int getopt(int, char *const [], const char *);
int getopt_long(int, char *const [], const char *, const struct option *, int *);
int getopt_long_only(int, char *const [], const char *, const struct option *, int *);
