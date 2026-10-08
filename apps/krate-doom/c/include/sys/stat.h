#pragma once
#include <sys/types.h>
struct stat { long st_size; int st_mode; };
int stat(const char *p, struct stat *s);
int mkdir(const char *p, mode_t m);
#define S_ISDIR(m) (0)
