int main(void) {
    static int (*ptrs[1])[4] = {0};
    int array1[4] = {100, 101, 102, 103};
    ptrs[0] = &array1;
    // Direct deref through subscript result
    return (*ptrs[0])[0];
}
