int subscript_pp(int **x) {
    return x[0][0];
}
int main(void) {
    int a = 42;
    int *ptr = &a;
    int **pp = &ptr;
    return subscript_pp(pp);
}
