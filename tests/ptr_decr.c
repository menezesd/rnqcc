int main(void) {
    long arr[2][3][4] = {
        {{1, 2, 3, 4}, {5, 6, 7, 8}, {9, 10, 11, 12}},
        {{13, 14, 15, 16}, {17, 18, 19, 20}, {21, 22, 23, 24}}};
    long (*outer_ptr)[3][4] = arr + 1;
    // outer_ptr points to arr[1]
    if (outer_ptr[0][0][0] != 13) return 1;
    outer_ptr--;
    // Now points to arr[0]
    if (outer_ptr[0][0][0] != 1) return 2;
    return 0;
}
