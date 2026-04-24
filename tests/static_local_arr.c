int check(long *arr) {
    arr[0] = 42;
    return arr[0] == 42;
}
int main(void) {
    static long local_long_arr[1000];
    if (!check(local_long_arr)) return 1;
    return 0;
}
