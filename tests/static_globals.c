double double_arr[3] = {1.0, 2.0, 3.0};
unsigned uint_arr[5] = {1u, 0u, 2147497230u};
long long_arr[1000];
unsigned long ulong_arr[4] = {100, 11, 12345, 4294967295U};

int main(void) {
    if (double_arr[0] != 1.0) return 1;
    if (double_arr[2] != 3.0) return 2;
    if (uint_arr[0] != 1u) return 3;
    if (uint_arr[2] != 2147497230u) return 4;
    if (uint_arr[3] != 0) return 5;  // not initialized
    if (long_arr[0] != 0) return 6;
    if (ulong_arr[3] != 4294967295UL) return 7;
    return 0;
}
