int three(void) { return 3; }
int main(void) {
    long neg7b = -7000000000l;
    int i = 1;
    long var = neg7b * three();
    long arr[5] = {
        neg7b,
        three() * 7l,
        -(long)i,
        var + (neg7b ? 2 : 3)
    };
    if (arr[0] != -7000000000l) return 1;
    if (arr[1] != 21) return 2;
    if (arr[2] != -1) return 3;
    if (arr[3] != -21000000000l + 2) return 4;
    if (arr[4] != 0) return 5;
    return 0;
}
