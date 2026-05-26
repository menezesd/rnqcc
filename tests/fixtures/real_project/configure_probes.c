#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <unistd.h>

#define PROBE_MAX(a, b) ({ __auto_type _a = (a); __auto_type _b = (b); _a > _b ? _a : _b; })

struct probe_record {
    int tag;
    long count;
    char name[8];
};

static int probe_layout(void) {
    return sizeof(size_t) == sizeof(unsigned long) &&
           offsetof(struct probe_record, count) >= sizeof(int) &&
           sizeof(((struct probe_record *)0)->name) == 8;
}

static int probe_headers(void) {
    struct stat st;
    struct timeval tv;
    memset(&st, 0, sizeof(st));
    tv.tv_sec = 1;
    tv.tv_usec = 2;
    return STDIN_FILENO == 0 && S_ISREG(S_IFREG) && tv.tv_sec + tv.tv_usec == 3;
}

static int probe_builtins(void) {
    return __builtin_constant_p(sizeof(int)) &&
           __builtin_types_compatible_p(uint32_t, unsigned int) &&
           __builtin_object_size("abc", 0) != 0;
}

int main(void) {
    return probe_layout() && probe_headers() && probe_builtins() &&
                   PROBE_MAX(7, 42) == 42
               ? 42
               : 1;
}
