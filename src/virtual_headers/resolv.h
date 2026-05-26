typedef struct __res_state *res_state;

struct __res_state {
    int retrans;
    int retry;
    unsigned long options;
};

int res_init(void);
