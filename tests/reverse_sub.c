int main(void) {
    int arr[5] = {10, 20, 30, 40, 50};
    // 3[arr] is equivalent to arr[3]
    if (&3[arr] != &arr[3]) return 1;
    return 0;
}
