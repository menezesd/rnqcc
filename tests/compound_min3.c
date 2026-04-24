int main(void) {
    static int (*ptrs[1])[4] = {0};
    int array1[4] = {100, 101, 102, 103};
    ptrs[0] = &array1;
    int (*p)[4] = ptrs[0]; // load the pointer
    if ((*p)[0] != 100) return 1;
    return 0;
}
