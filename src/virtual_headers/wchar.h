typedef int wchar_t;
typedef unsigned int wint_t;
typedef struct __rnqcc_mbstate_t {
    int __opaque;
} mbstate_t;
wint_t btowc(int);
int wctob(wint_t);
int mbsinit(const mbstate_t *);
size_t mbrlen(const char *, size_t, mbstate_t *);
size_t mbrtowc(wchar_t *, const char *, size_t, mbstate_t *);
size_t wcrtomb(char *, wchar_t, mbstate_t *);
size_t mbstowcs(wchar_t *, const char *, size_t);
size_t wcstombs(char *, const wchar_t *, size_t);
int wcscmp(const wchar_t *, const wchar_t *);
size_t wcslen(const wchar_t *);
wchar_t *wcschr(const wchar_t *, wchar_t);
wchar_t *wcsrchr(const wchar_t *, wchar_t);
