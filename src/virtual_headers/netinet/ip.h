struct ip {
    unsigned char ip_hl;
    unsigned char ip_v;
    unsigned char ip_tos;
    unsigned short ip_len;
    unsigned short ip_id;
    unsigned short ip_off;
    unsigned char ip_ttl;
    unsigned char ip_p;
    unsigned short ip_sum;
    struct in_addr ip_src;
    struct in_addr ip_dst;
};
