int read_nested(int nested_arr[2][3], int i, int j) {
    return nested_arr[i][j];
}
int main(void) {
    int arr[2][3] = {{1, 2, 3}, {4, 5, 6}};
    if (read_nested(arr, 1, 2) != 6) return 1;
    return 0;
}
