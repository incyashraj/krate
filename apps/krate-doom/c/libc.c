/* The C library Doom needs, and nothing else, for a Krate app.
 *
 * A Krate app imports Krate's interfaces and no WASI at all, so the usual
 * wasi-libc is out: its file and clock calls would be imports Krate refuses.
 * Doom only calls about forty libc functions (llvm-nm over its objects),
 * so they are here: strings and formatting in C, and the four things that
 * need the outside world -- memory, text output, the WAD file and leaving
 * -- handed to the Rust side (src/lib.rs), which owns the Krate calls.
 *
 * Files: the WAD is read from bytes built into the app; anything Doom
 * writes (its config, a save) goes to a sink for now.
 */
#include <stddef.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>

/* ---- from the Rust side ------------------------------------------------ */
extern void *krate_c_malloc(size_t n);
extern void krate_c_free(void *p);
extern void *krate_c_realloc(void *p, size_t n);
extern void krate_c_write(int stream, const char *p, size_t n);
extern const unsigned char *krate_c_wad(size_t *len);
_Noreturn extern void krate_c_exit(int code);

int errno = 0;

/* ---- memory ------------------------------------------------------------ */
void *malloc(size_t n) { return krate_c_malloc(n ? n : 1); }
void free(void *p) { if (p) krate_c_free(p); }
void *realloc(void *p, size_t n) { return p ? krate_c_realloc(p, n ? n : 1) : malloc(n); }
void *calloc(size_t a, size_t b) {
    size_t n = a * b;
    void *p = malloc(n);
    if (p) memset(p, 0, n);
    return p;
}

/* ---- strings ----------------------------------------------------------- */
/* memcpy, memmove, memset and memcmp come from Rust's compiler-builtins,
 * which the app links anyway; a second copy here is a duplicate symbol. */
void *memchr(const void *s, int c, size_t n) {
    const unsigned char *p = s;
    for (; n; n--, p++) if (*p == (unsigned char)c) return (void *)p;
    return NULL;
}
size_t strlen(const char *s) { const char *p = s; while (*p) p++; return p - s; }
int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}
int strncmp(const char *a, const char *b, size_t n) {
    for (; n; n--, a++, b++) { if (*a != *b || !*a) return (unsigned char)*a - (unsigned char)*b; }
    return 0;
}
int strcasecmp(const char *a, const char *b) {
    while (*a && tolower(*a) == tolower(*b)) { a++; b++; }
    return tolower((unsigned char)*a) - tolower((unsigned char)*b);
}
int strncasecmp(const char *a, const char *b, size_t n) {
    for (; n; n--, a++, b++) {
        int x = tolower((unsigned char)*a), y = tolower((unsigned char)*b);
        if (x != y || !x) return x - y;
    }
    return 0;
}
char *strcpy(char *d, const char *s) { char *r = d; while ((*d++ = *s++)); return r; }
char *strncpy(char *d, const char *s, size_t n) {
    size_t i = 0;
    for (; i < n && s[i]; i++) d[i] = s[i];
    for (; i < n; i++) d[i] = 0;
    return d;
}
char *strcat(char *d, const char *s) { strcpy(d + strlen(d), s); return d; }
char *strncat(char *d, const char *s, size_t n) {
    char *e = d + strlen(d);
    while (n-- && *s) *e++ = *s++;
    *e = 0;
    return d;
}
char *strchr(const char *s, int c) {
    for (;; s++) { if (*s == (char)c) return (char *)s; if (!*s) return NULL; }
}
char *strrchr(const char *s, int c) {
    const char *r = NULL;
    for (;; s++) { if (*s == (char)c) r = s; if (!*s) return (char *)r; }
}
char *strstr(const char *h, const char *n) {
    size_t k = strlen(n);
    if (!k) return (char *)h;
    for (; *h; h++) if (!strncmp(h, n, k)) return (char *)h;
    return NULL;
}
char *strdup(const char *s) {
    size_t n = strlen(s) + 1;
    char *p = malloc(n);
    if (p) memcpy(p, s, n);
    return p;
}
char *strerror(int e) { (void)e; return "error"; }

int isspace(int c) { return c == ' ' || (c >= 9 && c <= 13); }
int isdigit(int c) { return c >= '0' && c <= '9'; }
int isalpha(int c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z'); }
int isalnum(int c) { return isalpha(c) || isdigit(c); }
int isupper(int c) { return c >= 'A' && c <= 'Z'; }
int islower(int c) { return c >= 'a' && c <= 'z'; }
int isprint(int c) { return c >= 32 && c < 127; }
int ispunct(int c) { return isprint(c) && !isalnum(c) && c != ' '; }
int isxdigit(int c) { return isdigit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'); }
int toupper(int c) { return islower(c) ? c - 32 : c; }
int tolower(int c) { return isupper(c) ? c + 32 : c; }

/* ---- numbers ----------------------------------------------------------- */
int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }
double fabs(double x) { return x < 0 ? -x : x; }
long strtol(const char *s, char **end, int base) {
    while (isspace(*s)) s++;
    int neg = 0;
    if (*s == '-' || *s == '+') neg = *s++ == '-';
    if ((base == 0 || base == 16) && s[0] == '0' && (s[1] == 'x' || s[1] == 'X')) { s += 2; base = 16; }
    else if (base == 0 && s[0] == '0') base = 8;
    else if (base == 0) base = 10;
    long v = 0;
    for (;; s++) {
        int d = isdigit(*s) ? *s - '0' : isalpha(*s) ? tolower(*s) - 'a' + 10 : 99;
        if (d >= base) break;
        v = v * base + d;
    }
    if (end) *end = (char *)s;
    return neg ? -v : v;
}
unsigned long strtoul(const char *s, char **end, int base) { return (unsigned long)strtol(s, end, base); }
int atoi(const char *s) { return (int)strtol(s, NULL, 10); }
long atol(const char *s) { return strtol(s, NULL, 10); }
double strtod(const char *s, char **end) {
    while (isspace(*s)) s++;
    int neg = 0;
    if (*s == '-' || *s == '+') neg = *s++ == '-';
    double v = 0, scale = 1;
    while (isdigit(*s)) v = v * 10 + (*s++ - '0');
    if (*s == '.') { s++; while (isdigit(*s)) { scale /= 10; v += (*s++ - '0') * scale; } }
    if (end) *end = (char *)s;
    return neg ? -v : v;
}
double atof(const char *s) { return strtod(s, NULL); }

/* Doom's sscanf: one number, after optional literal text (" 0x%x", "%i"). */
int sscanf(const char *s, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int got = 0;
    while (*fmt) {
        if (*fmt == ' ') { while (isspace(*s)) s++; fmt++; continue; }
        if (*fmt != '%') { if (*s != *fmt) break; s++; fmt++; continue; }
        fmt++;
        int base = *fmt == 'x' ? 16 : *fmt == 'o' ? 8 : *fmt == 'i' ? 0 : 10;
        char *end;
        long v = strtol(s, &end, base);
        if (end == s) break;
        *va_arg(ap, int *) = (int)v;
        got++;
        s = end;
        fmt++;
    }
    va_end(ap);
    return got;
}

/* ---- formatting -------------------------------------------------------- */
typedef struct { char *buf; size_t cap, len; } out_t;
static void put(out_t *o, char c) { if (o->len + 1 < o->cap) o->buf[o->len] = c; o->len++; }
static void put_padded(out_t *o, const char *s, size_t n, int width, int left, char pad) {
    int fill = width > (int)n ? width - (int)n : 0;
    if (!left) while (fill--) put(o, pad);
    for (size_t i = 0; i < n; i++) put(o, s[i]);
    if (left) while (fill-- > 0) put(o, ' ');
}
int vsnprintf(char *buf, size_t cap, const char *fmt, va_list ap) {
    out_t o = { buf, cap, 0 };
    for (; *fmt; fmt++) {
        if (*fmt != '%') { put(&o, *fmt); continue; }
        fmt++;
        int left = 0; char pad = ' ';
        for (;; fmt++) {
            if (*fmt == '-') left = 1;
            else if (*fmt == '0') pad = '0';
            else if (*fmt == '+' || *fmt == ' ' || *fmt == '#') {}
            else break;
        }
        int width = 0, prec = -1;
        if (*fmt == '*') { width = va_arg(ap, int); fmt++; }
        while (isdigit(*fmt)) width = width * 10 + (*fmt++ - '0');
        if (*fmt == '.') {
            fmt++; prec = 0;
            if (*fmt == '*') { prec = va_arg(ap, int); fmt++; }
            while (isdigit(*fmt)) prec = prec * 10 + (*fmt++ - '0');
        }
        int longs = 0;
        while (*fmt == 'l' || *fmt == 'h' || *fmt == 'z') { if (*fmt == 'l') longs++; fmt++; }
        char tmp[32];
        switch (*fmt) {
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            size_t n = strlen(s);
            if (prec >= 0 && (size_t)prec < n) n = prec;
            put_padded(&o, s, n, width, left, ' ');
            break;
        }
        case 'c': { char c = (char)va_arg(ap, int); put_padded(&o, &c, 1, width, left, ' '); break; }
        case 'd': case 'i': case 'u': case 'x': case 'X': case 'o': case 'p': {
            unsigned long long v; int neg = 0;
            int base = (*fmt == 'x' || *fmt == 'X' || *fmt == 'p') ? 16 : *fmt == 'o' ? 8 : 10;
            if (*fmt == 'd' || *fmt == 'i') {
                long long sv = longs >= 2 ? va_arg(ap, long long) : longs ? va_arg(ap, long) : va_arg(ap, int);
                neg = sv < 0; v = neg ? -(unsigned long long)sv : (unsigned long long)sv;
            } else if (*fmt == 'p') {
                v = (uintptr_t)va_arg(ap, void *);
            } else {
                v = longs >= 2 ? va_arg(ap, unsigned long long) : longs ? va_arg(ap, unsigned long) : va_arg(ap, unsigned int);
            }
            const char *digits = *fmt == 'X' ? "0123456789ABCDEF" : "0123456789abcdef";
            int n = 0;
            do { tmp[sizeof tmp - 1 - n++] = digits[v % base]; v /= base; } while (v);
            while (prec > n) tmp[sizeof tmp - 1 - n++] = '0';
            if (neg) {
                if (pad == '0' && !left) { put(&o, '-'); if (width) width--; }
                else tmp[sizeof tmp - 1 - n++] = '-';
            }
            put_padded(&o, tmp + sizeof tmp - n, n, width, left, pad);
            break;
        }
        case 'f': case 'g': {
            double d = va_arg(ap, double);
            if (d < 0) { put(&o, '-'); d = -d; }
            long whole = (long)d;
            int n = snprintf(tmp, sizeof tmp, "%ld.%02ld", whole, (long)((d - whole) * 100));
            put_padded(&o, tmp, n, width, left, ' ');
            break;
        }
        case '%': put(&o, '%'); break;
        default: put(&o, '%'); put(&o, *fmt); break;
        }
    }
    if (cap) buf[o.len < cap ? o.len : cap - 1] = 0;
    return (int)o.len;
}
int snprintf(char *s, size_t n, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt); int r = vsnprintf(s, n, fmt, ap); va_end(ap); return r;
}
int sprintf(char *s, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt); int r = vsnprintf(s, (size_t)-1 >> 1, fmt, ap); va_end(ap); return r;
}

/* ---- files ------------------------------------------------------------- */
struct KFILE { const unsigned char *data; size_t len, pos; int stream; int sink; };
static struct KFILE out_file = { 0, 0, 0, 1, 0 }, err_file = { 0, 0, 0, 2, 0 }, in_file = { 0, 0, 0, 0, 1 };
FILE *stdout = &out_file, *stderr = &err_file, *stdin = &in_file;

static int is_wad(const char *path) {
    size_t n = strlen(path);
    return n >= 4 && !strcasecmp(path + n - 4, ".wad");
}
FILE *fopen(const char *path, const char *mode) {
    if (mode[0] == 'r' && is_wad(path)) {
        size_t len = 0;
        const unsigned char *data = krate_c_wad(&len);
        if (!data) return NULL;
        FILE *f = malloc(sizeof *f);
        f->data = data; f->len = len; f->pos = 0; f->stream = 0; f->sink = 0;
        return f;
    }
    if (mode[0] == 'w' || mode[0] == 'a') {
        FILE *f = malloc(sizeof *f);
        f->data = 0; f->len = 0; f->pos = 0; f->stream = 0; f->sink = 1;
        return f;
    }
    errno = 2;
    return NULL;
}
int fclose(FILE *f) { if (f && f != stdout && f != stderr && f != stdin) free(f); return 0; }
size_t fread(void *p, size_t s, size_t n, FILE *f) {
    if (!f->data || !s) return 0;
    size_t want = s * n, left = f->len - f->pos;
    if (want > left) want = left - left % s;
    memcpy(p, f->data + f->pos, want);
    f->pos += want;
    return want / s;
}
size_t fwrite(const void *p, size_t s, size_t n, FILE *f) {
    if (f->stream) krate_c_write(f->stream, p, s * n);
    return n;
}
int fseek(FILE *f, long off, int whence) {
    long base = whence == SEEK_SET ? 0 : whence == SEEK_CUR ? (long)f->pos : (long)f->len;
    long to = base + off;
    if (to < 0 || (size_t)to > f->len) return -1;
    f->pos = (size_t)to;
    return 0;
}
long ftell(FILE *f) { return (long)f->pos; }
int fflush(FILE *f) { (void)f; return 0; }
int feof(FILE *f) { return f->pos >= f->len; }
int fgetc(FILE *f) { return f->pos < f->len ? f->data[f->pos++] : EOF; }
char *fgets(char *s, int n, FILE *f) {
    int i = 0;
    while (i < n - 1) { int c = fgetc(f); if (c == EOF) break; s[i++] = (char)c; if (c == '\n') break; }
    if (!i) return NULL;
    s[i] = 0;
    return s;
}
int vfprintf(FILE *f, const char *fmt, va_list ap) {
    char buf[1024];
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    if (f->stream) krate_c_write(f->stream, buf, n < (int)sizeof buf ? (size_t)n : sizeof buf - 1);
    return n;
}
int fprintf(FILE *f, const char *fmt, ...) { va_list ap; va_start(ap, fmt); int r = vfprintf(f, fmt, ap); va_end(ap); return r; }
int vprintf(const char *fmt, va_list ap) { return vfprintf(stdout, fmt, ap); }
int printf(const char *fmt, ...) { va_list ap; va_start(ap, fmt); int r = vfprintf(stdout, fmt, ap); va_end(ap); return r; }
int fputs(const char *s, FILE *f) { if (f->stream) krate_c_write(f->stream, s, strlen(s)); return 0; }
int puts(const char *s) { fputs(s, stdout); krate_c_write(1, "\n", 1); return 0; }
int fputc(int c, FILE *f) { char ch = (char)c; if (f->stream) krate_c_write(f->stream, &ch, 1); return c; }
int putchar(int c) { return fputc(c, stdout); }

/* ---- the rest: a sandboxed app neither runs commands nor touches disk --- */
int remove(const char *p) { (void)p; return -1; }
int rename(const char *a, const char *b) { (void)a; (void)b; return -1; }
int mkdir(const char *p, int m) { (void)p; (void)m; return 0; }
int system(const char *c) { (void)c; return -1; }
char *getenv(const char *n) { (void)n; return NULL; }
_Noreturn void exit(int code) { krate_c_exit(code); }
_Noreturn void abort(void) { krate_c_exit(134); }
