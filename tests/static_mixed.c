// Mixed type static init like in static.c
static unsigned long ulong_arr[4] = {
    100, 11, 12345, 4294967295U
};

int check(unsigned long *arr) {
    if (arr[3] != 4294967295UL) return 1;
    return 0;
}

int *increment_static_element(void) {
    static int arr[4];
    arr[3] = arr[3] + 1;
    return arr;
}

int main(void) {
    if (check(ulong_arr)) return 1;
    int *p = increment_static_element();
    if (p[3] != 1) return 2;
    p = increment_static_element();
    if (p[3] != 2) return 3;
    return 0;
}
