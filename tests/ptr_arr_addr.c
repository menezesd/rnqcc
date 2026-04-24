int main(void) {
    int x = 10;
    int y = 20;
    int z = 30;
    int *arr[3] = {&x, &y, &z};
    if (*arr[0] != 10) return 1;
    if (*arr[1] != 20) return 2;
    if (*arr[2] != 30) return 3;
    return 0;
}
