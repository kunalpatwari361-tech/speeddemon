// Track C's record parser, in C++. The Rust driver calls parse_part once per part of records.csv: each
// record is added to its group's totals, and its name is marked in the group's bitset. The name table
// and the bitsets are shared by every part, so their updates are atomic.
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>

// One group's totals, laid out as the Rust side's #[repr(C)] Agg.
struct Agg {
    uint64_t count, ids;
    int64_t sum, min, max;
    uint64_t name_bytes, note_bytes;
};

namespace {

constexpr unsigned SLOT_BITS = 17;

uint64_t load64(const uint8_t* p) {
    uint64_t w;
    std::memcpy(&w, p, 8);  // little-endian, as on the x86 machines this builds for
    return w;
}

// Reads one unsigned integer at b[i] and steps over the byte after it.
uint64_t num(const uint8_t* b, size_t& i) {
    uint64_t v = 0;
    while (b[i] >= '0' && b[i] <= '9') v = v * 10 + (b[i++] - '0');
    ++i;
    return v;
}

// The first '"' at or after i, 8 bytes at a time. A record's end follows every quote, so every read is
// in bounds.
size_t quote(const uint8_t* b, size_t i) {
    for (;;) {
        uint64_t x = load64(b + i) ^ 0x2222222222222222ull;
        uint64_t found = (x - 0x0101010101010101ull) & ~x & 0x8080808080808080ull;
        if (found) return i + __builtin_ctzll(found) / 8;
        i += 8;
    }
}

// A text field at b[i]: where its raw bytes start, how many there are, and the value's length. Leaves i
// past its comma. The raw bytes of a quoted field identify its value, since escaping is one to one.
struct Field {
    size_t at, raw;
    uint64_t len;
};

Field field(const uint8_t* b, size_t& i) {
    size_t start = i;
    if (b[start] != '"') {
        size_t j = start;
        while (b[j] != ',') ++j;
        i = j + 1;
        return {start, j - start, j - start};
    }
    size_t j = start + 1, escapes = 0;
    for (;;) {
        j = quote(b, j);
        if (b[j + 1] != '"') break;
        ++escapes;
        j += 2;
    }
    i = j + 2;
    return {start + 1, j - start - 1, j - start - 1 - escapes};
}

// The slot of a name in the shared table, which holds its 64-bit hash; interned on first sight.
size_t slot_of(const uint8_t* p, size_t len, std::atomic<uint64_t>* slots) {
    uint64_t h = 0xcbf29ce484222325ull;
    for (size_t k = 0; k < len; ++k) h = (h ^ p[k]) * 0x100000001b3ull;
    h |= 1;
    size_t s = static_cast<size_t>((h * 0x9e3779b97f4a7c15ull) >> (64 - SLOT_BITS));
    for (;;) {
        uint64_t v = slots[s].load(std::memory_order_relaxed);
        if (v == h) return s;
        if (v == 0) {
            uint64_t expected = 0;
            if (slots[s].compare_exchange_strong(expected, h, std::memory_order_relaxed) || expected == h) return s;
            continue;
        }
        s = (s + 1) & ((size_t{1} << SLOT_BITS) - 1);
    }
}

}  // namespace

extern "C" void parse_part(const uint8_t* b, size_t from, size_t to, const uint8_t* group, size_t groups,
                           size_t unmatched, std::atomic<uint64_t>* slots, std::atomic<uint64_t>* seen, Agg* agg) {
    size_t i = from;
    while (i < to) {
        if (b[i] == 0xEF) i += 3;  // a byte order mark
        uint64_t id = num(b, i);
        Field name = field(b, i);
        Field note = field(b, i);
        uint64_t region = num(b, i);
        size_t g = region < groups ? group[region] : unmatched;
        bool neg = b[i] == '-';
        i += neg;
        uint64_t whole = num(b, i);
        int64_t cents = static_cast<int64_t>(whole * 100 + (b[i] - '0') * 10 + (b[i + 1] - '0'));
        if (neg) cents = -cents;
        i += 2 + (b[i + 2] == '\r') + 1;

        Agg& a = agg[g];
        a.count += 1;
        a.ids += id;
        a.sum += cents;
        if (cents < a.min) a.min = cents;
        if (cents > a.max) a.max = cents;
        a.name_bytes += name.len;
        a.note_bytes += note.len;
        size_t bit = g << SLOT_BITS | slot_of(b + name.at, name.raw, slots);
        uint64_t mask = uint64_t{1} << (bit % 64);
        if (!(seen[bit / 64].load(std::memory_order_relaxed) & mask)) seen[bit / 64].fetch_or(mask, std::memory_order_relaxed);
    }
}
