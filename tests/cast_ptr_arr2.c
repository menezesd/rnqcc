int main(void) {
    int multi_dim[2][3] = {{0, 1, 2}, {3, 4, 5}};
    int (*row_pointer)[3] = (int (*)[3]) multi_dim;
    // Without +1: row_pointer points to multi_dim[0]
    if (row_pointer[0][1] != 1) return 1;
    // With +1:
    row_pointer = row_pointer + 1;
    if (row_pointer[0][0] != 3) return 2;
    if (row_pointer[0][1] != 4) return 3;
    return 0;
}
