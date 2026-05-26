void closelog(void);
void openlog(const char *, int, int);
int setlogmask(int);
void syslog(int, const char *, ...);
void vsyslog(int, const char *, va_list);
