static unsigned long local_ulong_arr[4] = {
    100, 11, 12345, 4294967295U
};

int check_ulong_arr(unsigned long *arr) {
    if (arr[0] != 100) return 1;
    if (arr[1] != 11) return 2;
    if (arr[2] != 12345) return 3;
    if (arr[3] != 4294967295UL) return 4;
    return 0;
}

int main(void) {
    return check_ulong_arr(local_ulong_arr);
}
