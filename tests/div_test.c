double zero = 0.0;
int main(void) {
    double neg_zero = -zero;
    double result = 1.0 / neg_zero;
    // result should be -inf
    double neg_inf = -(1.0 / 0.0);
    // neg_inf should also be -inf
    if (result == neg_inf)
        return 0;
    return 1;
}
