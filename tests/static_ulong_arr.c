int main(void) {
    static unsigned long arr[4] = {100, 11, 12345, 4294967295U};
    if (arr[0] != 100) return 1;
    if (arr[3] != 4294967295UL) return 2;
    return 0;
}
