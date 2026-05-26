typedef struct {
    unsigned long fds_bits[16];
} fd_set;

void FD_ZERO(fd_set *);
void FD_SET(int, fd_set *);
void FD_CLR(int, fd_set *);
int FD_ISSET(int, fd_set *);
int select(int, fd_set *, fd_set *, fd_set *, struct timeval *);
