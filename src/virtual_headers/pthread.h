typedef unsigned long pthread_t;
typedef struct __rnqcc_pthread_attr_t {
    unsigned long __opaque;
} pthread_attr_t;
typedef struct __rnqcc_pthread_mutex_t {
    unsigned long __opaque;
} pthread_mutex_t;
typedef struct __rnqcc_pthread_mutexattr_t {
    unsigned long __opaque;
} pthread_mutexattr_t;
typedef struct __rnqcc_pthread_cond_t {
    unsigned long __opaque;
} pthread_cond_t;
typedef struct __rnqcc_pthread_condattr_t {
    unsigned long __opaque;
} pthread_condattr_t;
int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *);
int pthread_join(pthread_t, void **);
pthread_t pthread_self(void);
int pthread_equal(pthread_t, pthread_t);
int pthread_mutex_init(pthread_mutex_t *, const pthread_mutexattr_t *);
int pthread_mutex_destroy(pthread_mutex_t *);
int pthread_mutex_lock(pthread_mutex_t *);
int pthread_mutex_unlock(pthread_mutex_t *);
int pthread_cond_init(pthread_cond_t *, const pthread_condattr_t *);
int pthread_cond_destroy(pthread_cond_t *);
int pthread_cond_signal(pthread_cond_t *);
int pthread_cond_broadcast(pthread_cond_t *);
int pthread_cond_wait(pthread_cond_t *, pthread_mutex_t *);
