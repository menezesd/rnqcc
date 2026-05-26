#include "smoke_config.h"

int smoke_sum(struct smoke_pair pair);
int smoke_attribute_probe(int value) SMOKE_NO_INLINE;

int main(void) {
    struct smoke_pair pair = {10, 20};
    return smoke_sum(pair) + SMOKE_OFFSET + smoke_attribute_probe(0);
}
