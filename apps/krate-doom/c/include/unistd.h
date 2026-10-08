#pragma once
#include <stddef.h>
#include <sys/types.h>
int access(const char *p, int mode);
int close(int fd);
int unlink(const char *p);
int usleep(unsigned us);
int isatty(int fd);
#define F_OK 0
#define R_OK 4
#define W_OK 2
