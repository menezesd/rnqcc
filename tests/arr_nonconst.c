int three(void) { return 3; }
int main(void) {
    int arr[3] = {three(), three() + 1, three() + 2};
    if (arr[0] != 3) return 1;
    if (arr[1] != 4) return 2;
    if (arr[2] != 5) return 3;
    return 0;
}
