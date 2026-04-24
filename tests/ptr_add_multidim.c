int main(void) {
    int nested_arr[3][3] = {{1, 2, 3}, {4, 5, 6}, {7, 8, 9}};
    int (*row_ptr)[3] = nested_arr + 2;
    // **row_ptr should be nested_arr[2][0] = 7
    return **row_ptr;
}
