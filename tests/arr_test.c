int main(void) {
    int a[3] = {10, 20, 30};
    if (a[0] != 10) return 1;
    if (a[1] != 20) return 2;
    if (a[2] != 30) return 3;
    a[1] = 99;
    if (a[1] != 99) return 4;
    return 0;
}
