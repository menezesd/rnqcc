int main(void) {
    static unsigned long arr[4] = {
        100.0, 11, 12345, 4294967295U
    };
    if (arr[0] != 100) return 1;
    if (arr[1] != 11) return 2;
    return 0;
}
