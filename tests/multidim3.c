int nested[2][3][5] = {
    {{1,2,3,4,5}, {6,7,8,9,10}, {11,12,13,14,15}},
    {{16,17,18,19,20}, {21,22,23,24,25}, {26,27,28,29,30}}
};

int read(int arr[2][3][5], int i, int j, int k) {
    return arr[i][j][k];
}

int main(void) {
    if (read(nested, 1, 1, 0) != 21) return 1;
    // Pointer arithmetic: nested + 1
    int (*ptr)[3][5] = nested + 1;
    if (ptr[0][0][0] != 16) return 2;
    return 0;
}
