#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

typedef struct {
    int32_t len;
    int32_t cap;
    uint8_t *data;
    int32_t elem_size;
} PebblesArray;

void pebbles_print_str(const char *s) {
    if (s == NULL) {
        puts("<null>");
        return;
    }
    puts(s);
}

char *pebbles_input(void) {
    char buf[4096];
    if (!fgets(buf, sizeof(buf), stdin)) {
        return NULL;
    }
    size_t n = strlen(buf);
    if (n > 0 && buf[n - 1] == '\n') {
        buf[n - 1] = '\0';
        n -= 1;
    }
    char *out = (char *)malloc(n + 1);
    if (!out) {
        return NULL;
    }
    memcpy(out, buf, n + 1);
    return out;
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

PebblesArray pebbles_array_new(int32_t elem_size, int32_t len) {
    PebblesArray arr;
    arr.len = len > 0 ? len : 0;
    arr.cap = arr.len;
    arr.elem_size = elem_size;
    arr.data = NULL;
    if (arr.len > 0 && elem_size > 0) {
        size_t bytes = (size_t)arr.len * (size_t)elem_size;
        arr.data = (uint8_t *)malloc(bytes);
        if (arr.data == NULL) {
            arr.len = 0;
            arr.cap = 0;
        }
    }
    return arr;
}

void pebbles_array_push(PebblesArray *arr, const void *elem) {
    if (arr == NULL || arr->elem_size <= 0 || elem == NULL) {
        return;
    }
    if (arr->len >= arr->cap) {
        int32_t new_cap = arr->cap > 0 ? arr->cap * 2 : 4;
        size_t bytes = (size_t)new_cap * (size_t)arr->elem_size;
        void *new_data = realloc(arr->data, bytes);
        if (new_data == NULL) {
            return;
        }
        arr->data = (uint8_t *)new_data;
        arr->cap = new_cap;
    }
    memcpy(arr->data + (size_t)arr->len * (size_t)arr->elem_size, elem, (size_t)arr->elem_size);
    arr->len += 1;
}

bool pebbles_array_pop(PebblesArray *arr, void *out) {
    if (arr == NULL || arr->len <= 0 || arr->elem_size <= 0) {
        return false;
    }
    arr->len -= 1;
    if (out != NULL) {
        memcpy(out, arr->data + (size_t)arr->len * (size_t)arr->elem_size, (size_t)arr->elem_size);
    }
    return true;
}

int32_t pebbles_int_str(const char *s) {
    if (s == NULL) {
        return 0;
    }
    char *end = NULL;
    long v = strtol(s, &end, 10);
    return (int32_t)v;
}

double pebbles_float_str(const char *s) {
    if (s == NULL) {
        return 0.0;
    }
    char *end = NULL;
    double v = strtod(s, &end);
    return v;
}

double pebbles_sqrt_f64(double v) {
    return sqrt(v);
}

char *pebbles_str_index(const char *s, int32_t idx) {
    if (s == NULL || idx < 0) {
        char *out = (char *)malloc(1);
        if (!out) {
            return NULL;
        }
        out[0] = '\0';
        return out;
    }
    size_t len = strlen(s);
    if ((size_t)idx >= len) {
        char *out = (char *)malloc(1);
        if (!out) {
            return NULL;
        }
        out[0] = '\0';
        return out;
    }
    char *out = (char *)malloc(2);
    if (!out) {
        return NULL;
    }
    out[0] = s[idx];
    out[1] = '\0';
    return out;
}
