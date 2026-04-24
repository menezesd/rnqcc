int main(void) {
    int a = 3;
    int *ptr = &a;
    if (ptr[0] != 3) return 1;
    return 0;
}
