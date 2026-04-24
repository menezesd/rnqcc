int main(void) {
    static int (*ptrs[3])[4] = {0, 0, 0};
    int array1[4] = {100, 101, 102, 103};
    ptrs[0] = &array1;
    if ((*ptrs[0])[0] != 100) return 1;
    ptrs[0] += 1;
    if (ptrs[0][-1][3] != 103) return 2;
    return 0;
}
