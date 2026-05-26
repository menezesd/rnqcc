#ifndef SMOKE_CONFIG_H
#define SMOKE_CONFIG_H

#define SMOKE_OFFSET 7
#define SMOKE_ADD(a, b) ((a) + (b))

#if __has_builtin(__builtin_expect)
#define SMOKE_LIKELY(value) (value)
#else
#define SMOKE_LIKELY(value) (value)
#endif

#define SMOKE_NO_INLINE __attribute__((noinline))

struct smoke_pair {
    int left;
    int right;
};

#endif
