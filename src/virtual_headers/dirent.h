typedef struct __rnqcc_DIR DIR;
struct dirent {
    unsigned long d_ino;
    char d_name[256];
};
DIR *opendir(const char *);
struct dirent *readdir(DIR *);
int closedir(DIR *);
void rewinddir(DIR *);
