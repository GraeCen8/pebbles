#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

void pebbles_print_str(const char *s) {
    if (s == NULL) {
        puts("<null>");
        return;
    }
    puts(s);
}

char *pebbles_input(void) {
    char *line = NULL;
    size_t cap = 0;
    ssize_t n = getline(&line, &cap, stdin);
    if (n <= 0) {
        free(line);
        return NULL;
    }
    if (line[n - 1] == '\n') {
        line[n - 1] = '\0';
    }
    return line;
}

int32_t pebbles_len_str(const char *s) {
    if (s == NULL) {
        return 0;
    }
    return (int32_t)strlen(s);
}

char *pebbles_str_i32(int32_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof(buf), "%d", v);
    if (n < 0) {
        return NULL;
    }
    char *out = (char *)malloc((size_t)n + 1);
    if (!out) {
        return NULL;
    }
    memcpy(out, buf, (size_t)n + 1);
    return out;
}

char *pebbles_str_concat(const char *a, const char *b) {
    if (a == NULL && b == NULL) {
        return NULL;
    }
    if (a == NULL) {
        size_t lb = strlen(b);
        char *out = (char *)malloc(lb + 1);
        if (!out) {
            return NULL;
        }
        memcpy(out, b, lb + 1);
        return out;
    }
    if (b == NULL) {
        size_t la = strlen(a);
        char *out = (char *)malloc(la + 1);
        if (!out) {
            return NULL;
        }
        memcpy(out, a, la + 1);
        return out;
    }
    size_t la = strlen(a);
    size_t lb = strlen(b);
    char *out = (char *)malloc(la + lb + 1);
    if (!out) {
        return NULL;
    }
    memcpy(out, a, la);
    memcpy(out + la, b, lb + 1);
    return out;
}

bool pebbles_str_eq(const char *a, const char *b) {
    if (a == NULL || b == NULL) {
        return a == b;
    }
    return strcmp(a, b) == 0;
}
