double copysign(double x, double y);
double zero = 0.0;

int main(void) {
    double negative_zero = -zero;
    if (negative_zero != 0)
        return 1;
    if ( 1.0/negative_zero != -(1.0/0.0) )
        return 2;
    return 0;
}
