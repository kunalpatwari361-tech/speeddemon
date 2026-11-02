#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

extern unsigned long long XXH3_64bits(const void *data, size_t len);

uint32_t speed_demon_num(const unsigned char *bytes, size_t len) {
    uint32_t value = 0;
    size_t i = 0;
    while (i < len) {
        const unsigned char c = bytes[i];
        if (c < (unsigned char)'0' || c > (unsigned char)'9') {
            break;
        }
        value = value * 10u + (uint32_t)(c - (unsigned char)'0');
        ++i;
    }
    return value;
}

uint64_t speed_demon_xxh3_lower(const unsigned char *bytes, size_t len) {
    unsigned char stack[256];
    unsigned char *lower = stack;
    if (len > sizeof(stack)) {
        lower = (unsigned char *)malloc(len);
        if (lower == NULL) {
            return (uint64_t)XXH3_64bits(bytes, len);
        }
    }
    for (size_t i = 0; i < len; ++i) {
        const unsigned char c = bytes[i];
        lower[i] = (c >= (unsigned char)'A' && c <= (unsigned char)'Z') ? (unsigned char)(c | 0x20u) : c;
    }
    const uint64_t hash = (uint64_t)XXH3_64bits(lower, len);
    if (lower != stack) {
        free(lower);
    }
    return hash;
}
