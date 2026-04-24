int main(void) {
    int arr[4] = {100, 200, 300, 400};
    int (*p)[4] = &arr;
    // *p should decay to int*, (*p)[0] should be 100
    return (*p)[0];
}
