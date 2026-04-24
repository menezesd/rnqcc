int main(void) {
    int multi_dim[2][3] = {{0, 1, 2}, {3, 4, 5}};
    int (*array_pointer)[2][3] = &multi_dim;
    int (*row_pointer)[3] = (int (*)[3]) array_pointer;
    if (row_pointer != (int (*)[3]) multi_dim) return 1;
    row_pointer = row_pointer + 1;
    if (row_pointer[0][1] != 4) return 2;
    return 0;
}
