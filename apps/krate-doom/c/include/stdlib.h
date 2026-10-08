#pragma once
#include <stddef.h>
void *malloc(size_t n);
void *calloc(size_t a, size_t b);
void *realloc(void *p, size_t n);
void free(void *p);
_Noreturn void exit(int code);
_Noreturn void abort(void);
int atoi(const char *s);
long atol(const char *s);
double atof(const char *s);
long strtol(const char *s, char **end, int base);
unsigned long strtoul(const char *s, char **end, int base);
double strtod(const char *s, char **end);
int abs(int x);
long labs(long x);
char *getenv(const char *name);
int system(const char *cmd);
void qsort(void *base, size_t n, size_t size, int (*cmp)(const void *, const void *));
int rand(void);
void srand(unsigned s);
#define RAND_MAX 2147483647
#define EXIT_SUCCESS 0
#define EXIT_FAILURE 1
