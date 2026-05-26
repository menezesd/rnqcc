typedef long jmp_buf[8];
int setjmp(jmp_buf);
void longjmp(jmp_buf, int);
