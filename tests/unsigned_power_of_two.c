unsigned mul32(unsigned value) {
    return value * 8u;
}

unsigned div32(unsigned value) {
    return value / 16u;
}

unsigned rem32(unsigned value) {
    return value % 32u;
}

unsigned long mul64(unsigned long value) {
    return 32ul * value;
}

unsigned long div64(unsigned long value) {
    return value / 64ul;
}

unsigned long rem64(unsigned long value) {
    return value % 128ul;
}

unsigned __int128 mul128(unsigned __int128 value) {
    return value * ((unsigned __int128)1 << 96);
}

unsigned __int128 div128(unsigned __int128 value) {
    return value / ((unsigned __int128)1 << 96);
}

unsigned __int128 rem128(unsigned __int128 value) {
    return value % ((unsigned __int128)1 << 96);
}

int main(void) {
    unsigned value32 = 0xf0000003u;
    unsigned long value64 = 0xffffffffffffff8aul;
    unsigned __int128 value128 = ((unsigned __int128)1 << 100) + 0x1234;

    if (mul32(value32) != 0x80000018u || div32(value32) != 0x0f000000u || rem32(value32) != 3u) {
        return 1;
    }
    if (mul64(value64) != 0xfffffffffffff140ul
        || div64(value64) != 0x03fffffffffffffeul
        || rem64(value64) != 10ul) {
        return 2;
    }
    if (mul128(value128) != ((unsigned __int128)0x1234 << 96)
        || div128(value128) != 16
        || rem128(value128) != 0x1234) {
        return 3;
    }
    return 0;
}
