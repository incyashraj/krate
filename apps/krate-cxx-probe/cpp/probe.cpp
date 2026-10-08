// Ordinary C++: the STL, iostream, files, the clock. Nothing Krate-specific.
#include <algorithm>
#include <chrono>
#include <cstdio>
#include <iostream>
#include <map>
#include <memory>
#include <string>
#include <vector>
#include <dirent.h>
#include <setjmp.h>
static jmp_buf g_jump;
static void jump_back(int v) { longjmp(g_jump, v); }
#include <sys/stat.h>
#include <errno.h>
#include <string.h>

struct Global { std::string name; Global() : name("constructed") {} } g_global;
// Static constructors must run exactly once. Twice turns every
// self-registering list (a game's menus, a plugin table) into a cycle.
static int g_ctor_runs = 0;
struct Counted { Counted() { g_ctor_runs++; } } g_counted;

extern "C" int probe_main(int argc, char **argv) {
    std::vector<std::string> words = {"runs", "krate", "c++"};
    std::sort(words.begin(), words.end());
    std::map<std::string, size_t> lengths;
    for (const auto &w : words) lengths[w] = w.size();
    auto shared = std::make_shared<std::vector<int>>(1000, 7);
    printf("argv: %d %s\n", argc, argc > 1 ? argv[1] : "-");
    printf("global: %s\n", g_global.name.c_str());
    printf("ctors: %d\n", g_ctor_runs);
    std::cout << "iostream: " << words[0] << " " << lengths["krate"] << " " << shared->size() << std::endl;

    mkdir("probe-dir", 0755);
    FILE *f = fopen("probe-dir/hello.txt", "w");
    if (!f) { printf("file: cannot open for write\n"); return 2; }
    fprintf(f, "hello from C++ %d\n", 42);
    fclose(f);
    f = fopen("probe-dir/hello.txt", "r");
    char line[64] = {0};
    fgets(line, sizeof line, f);
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fclose(f);
    printf("file: %s", line);
    printf("size: %ld\n", size);
    struct stat st;
    printf("stat: %d %lld\n", stat("probe-dir/hello.txt", &st), (long long)st.st_size);
    DIR *d = opendir("probe-dir");
    int entries = 0;
    if (d) { errno = 0; struct dirent *e; while ((e = readdir(d))) { entries++; printf("  entry: %s\n", e->d_name); } if (errno) printf("readdir errno: %d %s\n", errno, strerror(errno)); closedir(d); }
    else printf("opendir errno: %d %s\n", errno, strerror(errno));
    printf("readdir: %d\n", entries);

    auto t0 = std::chrono::steady_clock::now();
    volatile double x = 0; for (int i = 0; i < 2000000; i++) x += i * 0.5;
    auto ms = std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now() - t0).count();
    printf("clock: %lld ms\n", (long long)ms);
    int got = setjmp(g_jump);
    if (got == 0) jump_back(7);
    printf("setjmp: came back with %d\n", got);
    printf("probe: ok\n");
    return 0;
}
