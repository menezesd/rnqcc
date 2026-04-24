int main(void) {
    int arr[2][3] = {{1, 2, 3}, {4, 5, 6}};
    if (arr[0][0] != 1) return 1;
    if (arr[0][2] != 3) return 2;
    if (arr[1][0] != 4) return 3;
    if (arr[1][2] != 6) return 4;
    return 0;
}
