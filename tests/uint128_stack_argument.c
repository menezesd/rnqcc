extern unsigned __int128 sink(
    unsigned long, unsigned long, unsigned long, unsigned long,
    unsigned long, unsigned long, unsigned long, unsigned __int128);

unsigned __int128 forward(void) {
    return sink(0, 1, 2, 3, 4, 5, 6, ((unsigned __int128)1 << 96));
}
