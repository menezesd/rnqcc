int main(void) {
    int x = 5;
    int result = 1;
    while (x > 0) {
        result = result * x;
        x = x - 1;
    }
    // 5! = 120
    return result;
}
