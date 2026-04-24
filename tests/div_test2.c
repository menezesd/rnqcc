double zero = 0.0;
int main(void) {
    double neg_zero = -zero;
    double pos_inf = 1.0 / zero;
    double neg_inf = 1.0 / neg_zero;
    // pos_inf should be +inf, neg_inf should be -inf
    // They should NOT be equal
    if (pos_inf == neg_inf)
        return 1;
    // neg_inf < 0
    if (neg_inf > 0.0)
        return 2;
    // pos_inf > 0
    if (pos_inf < 0.0)
        return 3;
    return 0;
}
