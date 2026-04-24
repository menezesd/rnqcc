int check_double_arr(double *arr) {
    if (arr[0] != 1.0) return 1;
    if (arr[1] != 2.0) return 2;
    if (arr[2] != 3.0) return 3;
    return 0;
}
int main(void) {
    double local_double_arr[3] = {1.0, 2.0, 3.0};
    return check_double_arr(local_double_arr);
}
