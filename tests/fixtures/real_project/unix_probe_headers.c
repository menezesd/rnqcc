#include <sys/types.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <sys/utsname.h>
#include <termios.h>
#include <netdb.h>
#include <pwd.h>
#include <grp.h>
#include <strings.h>

static int probe_process_headers(void) {
    int status = 42 << 8;
    struct utsname uts;
    struct passwd pw;
    struct group gr;

    uts.machine[0] = 'a';
    pw.pw_uid = 1000;
    gr.gr_gid = 1000;

    return WIFEXITED(status) && WEXITSTATUS(status) == 42 &&
           sizeof(uts.machine) >= 64 && pw.pw_uid == gr.gr_gid &&
           sizeof(ffs(8)) == sizeof(int);
}

static int probe_unix_headers(void) {
    struct rlimit limit;
    struct rusage usage;
    struct winsize window;
    struct termios term;
    struct addrinfo hints;

    limit.rlim_cur = 64;
    limit.rlim_max = RLIM_INFINITY;
    usage.ru_utime.tv_sec = 1;
    usage.ru_stime.tv_usec = 2;
    window.ws_row = 24;
    window.ws_col = 80;
    term.c_iflag = ICRNL | IXON;
    term.c_lflag = ICANON | ECHO | ISIG;
    term.c_cc[VMIN] = 1;
    term.c_ospeed = B115200;
    hints.ai_flags = AI_PASSIVE | AI_NUMERICHOST;
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_addrlen = sizeof(struct sockaddr_in);

    return PROT_READ == 1 && MAP_ANONYMOUS == MAP_ANON &&
           limit.rlim_cur == 64 && limit.rlim_max == RLIM_INFINITY &&
           usage.ru_utime.tv_sec + usage.ru_stime.tv_usec == 3 &&
           sizeof(struct winsize) == 4 * sizeof(unsigned short) &&
           window.ws_row == 24 && window.ws_col == 80 &&
           sizeof(term.c_cc) == NCCS * sizeof(cc_t) &&
           term.c_cc[VMIN] == 1 && term.c_ospeed == B115200 &&
           hints.ai_family == AF_INET && hints.ai_socktype == SOCK_STREAM &&
           hints.ai_addrlen > 0 && NI_MAXHOST > NI_MAXSERV;
}

int main(void) {
    return probe_process_headers() && probe_unix_headers() ? 0 : 1;
}
