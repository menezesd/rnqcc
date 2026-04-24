int main(void) {
    double arr[5] = {1.0, 123e4};
    if (arr[0] != 1.0) return 1;
    if (arr[1] != 123e4) return 2;
    if (arr[2] != 0.0) return 3;
    if (arr[3] != 0.0) return 4;
    if (arr[4] != 0.0) return 5;
    return 0;
}
