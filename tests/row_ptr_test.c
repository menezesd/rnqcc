int main(void) {
    int multi_dim[2][3] = {{0, 1, 2}, {3, 4, 5}};
    int (*row_pointer)[3] = multi_dim;
    row_pointer = row_pointer + 1;
    // row_pointer now points to multi_dim[1] = {3, 4, 5}
    if (row_pointer[0][1] != 4) return 1;
    return 0;
}
