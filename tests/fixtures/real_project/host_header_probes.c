#include <net/if.h>
#include <netinet/tcp.h>
#include <regex.h>

int main(void) {
    regex_t regex;
    regmatch_t match;
    struct if_nameindex if_name;
    int tcp_flag = TCP_NODELAY;

    regex.re_nsub = 0;
    match.rm_so = 0;
    match.rm_eo = 0;
    if_name.if_index = 0;
    if_name.if_name = 0;

    return regex.re_nsub == 0 && match.rm_so == match.rm_eo &&
           if_name.if_index == 0 && tcp_flag >= 0
               ? 0
               : 1;
}
