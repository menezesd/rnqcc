typedef int sig_atomic_t;
void (*signal(int, void (*)(int)))(int);
int raise(int);
