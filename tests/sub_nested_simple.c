int read_nested(int nested_arr[2][3], int i, int j, int expected) {
    return (nested_arr[i][j] == expected);
}
int write_nested(int nested_arr[2][3], int i, int j, int new_val) {
    nested_arr[i][j] = new_val;
    return 0;
}
int main(void) {
    int nested_arr[2][3] = {{1, 2, 3}, {4, 5, 6}};
    if (!read_nested(nested_arr, 0, 0, 1)) return 1;
    if (!read_nested(nested_arr, 1, 2, 6)) return 2;
    write_nested(nested_arr, 0, 2, 99);
    if (!read_nested(nested_arr, 0, 2, 99)) return 3;
    return 0;
}
