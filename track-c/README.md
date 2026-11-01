# Track C: Join

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/c-join.md).

## Approach

Rust, standard library only. Both files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

`regions.csv` becomes one byte per region id. A country code `[A-Z]{3}` is used directly as an index into 26³ slots, so the countries come out in byte order without sorting, and each region maps to its country's rank.

`records.csv` is split into one part per hardware thread. A part starts at a true record boundary: an LF followed by a digit or a BOM. Inside a quoted note an LF is always followed by a lowercase word or a quote, so a boundary can't be mistaken. Each thread finds its fields' ends 64 bytes at a time. AVX2 compares give one bitmask of the block's quotes and one of its commas and LFs; the quote mask, prefix-xored, marks the bytes inside quotes, so the commas and LFs outside them are exactly the ends of fields and records. A field is then read from its known extent: a number's digits all at once, shifted so zeros lead and joined in three multiplies; a quoted field's value length is its raw length minus its quotes' pairs, counted from the quote mask. A BOM is skipped where a record starts. Count, id sum, cent sums, min, max and byte totals are kept per country per thread and summed at the end; no field is ever copied.

Distinct names are counted without storing any strings. Every name gets a slot in one lock-free open-addressing table of 2^17 slots (76% full at 100,000 names), shared by all threads and allocated lazily, keyed by a 64-bit hash of the name's raw bytes, taken 8 at a time. The raw bytes identify the value because escaping is one to one, and among 100,000 names a 64-bit collision is about 1 in 4 billion. Each country has a bitset over those slots, and its distinct count is the bitset's population count. Setting those bits with an atomic operation per record took about 40% of a run, as each locked operation stops the core from overlapping its cache misses. Instead, each country's bitset is owned by one thread (country rank modulo the thread count). Each thread parses its share of the records in 64 pieces; after each piece it hands every record's bit index to the owning thread, and once all threads have finished the piece, each owner sets its bits with plain stores, since no other thread writes those words. The hand-off buffers are small enough that peak memory is unchanged.

The process asks for no transparent huge pages, and the region table is freed before the records are parsed.

The binary is built for the scoring CPU (`target-cpu=native`) with profile-guided optimisation: during the offline build, a byte-identical copy of the published generator (`generator.py`, seed 2) makes a full-size training dataset, the instrumented binary runs on it, and `llvm-profdata` guides the final build. The runtime image holds only the Rust executable.

## Dependencies

None.

## With another 24 hours

The name table lookup, about a sixth of a run: one random access into a 1 MB table per record. Reading `regions.csv` on all threads was tried and saved only about 3%.
