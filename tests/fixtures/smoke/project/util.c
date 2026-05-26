#include "smoke_config.h"

int smoke_sum(struct smoke_pair pair) {
    return SMOKE_ADD(pair.left, pair.right);
}

int smoke_attribute_probe(int value) {
    if (SMOKE_LIKELY(value)) {
        return 1;
    }
    return 0;
}
