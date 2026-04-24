int main(void) {
    int arr[3] = {10, 20, 30};
    int (*p)[3] = &arr;
    // *p should give the array int[3], which decays to int*
    // **p should give arr[0] = 10
    int *q = *p;  // array decay
    return *q;    // should be 10
}
