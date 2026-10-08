#pragma once
#include <sys/types.h>
int open(const char *p, int flags, ...);
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 64
#define O_TRUNC 512
#define O_BINARY 0
