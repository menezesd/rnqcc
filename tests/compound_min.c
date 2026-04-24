int main(void) {
    static int (*ptrs[3])[4] = {0, 0, 0};
    int array1[4] = {100, 101, 102, 103};
    ptrs[0] = &array1;
    // Just check if we can read through the pointer
    if ((*ptrs[0])[0] != 100) return 1;
    return 0;
}
