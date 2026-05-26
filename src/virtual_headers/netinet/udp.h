struct udphdr {
    in_port_t uh_sport;
    in_port_t uh_dport;
    unsigned short uh_ulen;
    unsigned short uh_sum;
};
