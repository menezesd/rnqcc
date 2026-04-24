int main(void) {
    long arr[2][3][4] = {
        {{1, 2, 3, 4}, {5, 6, 7, 8}, {9, 10, 11, 12}},
        {{13, 14, 15, 16}, {17, 18, 19, 20}, {21, 22, 23, 24}}};
    long (*inner_ptr)[4] = arr[0] + 1;
    // inner_ptr points to arr[0][1] = {5,6,7,8}
    if (inner_ptr[0][2] != 7) return 1;
    if (inner_ptr++[0][2] != 7) return 2;
    // Now inner_ptr points to arr[0][2] = {9,10,11,12}
    if (inner_ptr[0][2] != 11) return 3;
    return 0;
}
