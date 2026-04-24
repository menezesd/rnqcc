double zero = 0.0;
int main(void) {
    double neg = -zero;
    // 1/neg should give -inf if neg is -0.0, +inf if neg is +0.0
    double result = 1.0 / neg;
    if (result > 0.0)
        return 1;  // neg was +0.0 (WRONG - negation didn't work)
    return 0;       // neg was -0.0 (correct)
}
