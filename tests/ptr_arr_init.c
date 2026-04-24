int *arr[3] = {0, 0, 0};
int main(void) {
    if (arr[0] != 0) return 1;
    int x = 42;
    arr[0] = &x;
    if (*arr[0] != 42) return 2;
    return 0;
}
