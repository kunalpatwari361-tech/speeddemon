#include <stddef.h>
#include <stdint.h>

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
