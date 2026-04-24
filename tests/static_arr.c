int *get_arr(void) {
    static int arr[4];
    arr[3] = arr[3] + 1;
    return arr;
}
int main(void) {
    int *p = get_arr();
    if (p[3] != 1) return 1;
    return 0;
}
