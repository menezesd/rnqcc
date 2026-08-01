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

unsigned set_bit32(unsigned value) {
    return value | 32u;
}

unsigned long toggle_bit64(unsigned long value) {
    return value ^ (1ul << 40);
}

unsigned mask_pattern32(unsigned value) {
    return value & 0x00ff00ffu;
}

unsigned long set_pattern64(unsigned long value) {
    return value | 0x00ff00ff00ff00fful;
}

unsigned toggle_pattern32(unsigned value) {
    return value ^ 0xff00ff00u;
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

__int128 signed_shift128(__int128 value) {
    return value >> 96;
}

unsigned __int128 typed_shift_count128(unsigned __int128 value) {
    return value >> (unsigned __int128)96;
}

unsigned _BitInt(128) bitint_shift128(unsigned _BitInt(128) value) {
    return value >> (unsigned _BitInt(128))96;
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
    if (set_bit32(1u) != 33u || toggle_bit64(0ul) != (1ul << 40)) {
        return 7;
    }
    if (mask_pattern32(0xdeadbeefu) != 0x00ad00efu
        || set_pattern64(0ul) != 0x00ff00ff00ff00fful
        || toggle_pattern32(0u) != 0xff00ff00u) {
        return 8;
    }
    if (mul128(value128) != ((unsigned __int128)0x1234 << 96)
        || div128(value128) != 16
        || rem128(value128) != 0x1234) {
        return 3;
    }
    if (signed_shift128(-((__int128)1 << 100)) != -16) {
        return 4;
    }
    if (typed_shift_count128(value128) != 16) {
        return 5;
    }
    if (bitint_shift128((unsigned _BitInt(128))value128) != 16) {
        return 6;
    }
    return 0;
}
