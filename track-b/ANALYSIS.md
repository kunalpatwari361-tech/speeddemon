# Track B: memory and runtime analysis

Source: `src/main.rs` at f546787 (704 counted lines, std only). Board: 0.311 to 0.384 s, 30 to 31 MB.
Laptop (i5-1335U, Docker, 12 threads) numbers below are medians of the stated runs; the laptop is about
twice the board's time, and its paired runs resolve about 5%.

## 1. Hotspot map

Phase timers, 5 runs (ms): scan 340 to 400, answer 106 to 150, place 42 to 66, parse queries 34 to 55,
sample 12 to 22, layout 9 to 14, compact 5 to 9, write about 4. Total about 600 ms.

Scan ablation, 7 runs on 8 cores (ms, cumulative): tokens only 127; + term lookup 248; + hits 400; + pair
lookup 495 (before the AVX2 tokens and the lookup prefetch, which took the scan 10 to 19% and 7 to 9% lower).

Counters, one full run: 37.4 M tokens; 29.6 M term hits (79%); 16.5 M of them to common-term bitsets;
23.4 M pair lookups, 4.2 M hits.

Top cache-miss sites (by ablation and table size):
1. term table probe: 131,072 slots x 16 B = 2 MB, one probe per token (prefetched 16 tokens ahead).
2. common-term bitset store: 479 bitsets x 489 words per round, one store per occurrence.
3. stream push and its later placement: 1.6 M rare hits per round, random writes into `flat`.
4. pair filter: 128 KB, one probe per pair; in cache (prefetching it measured no gain).
5. query-table inserts while parsing: about 350,000, into a table pre-sized for every word.

Branch profile: not measurable here (no perf counters in Docker Desktop's VM).

## 2. Memory layout

| Type | size | align | note |
|---|---|---|---|
| `Table` slot `(u64, u32, u32)` | 16 | 8 | key, id, bitset index; 4 per line |
| query `[u32; 4]` | 16 | 4 | step range, phrase span |
| step `u32` | 4 | 4 | operator in bits 29 and 30 |
| per-query totals `(u64, u64, [u32; 10])` | 56 | 8 | 2.8 MB for 50,000 |
| `Set<'a>` | 24 | 8 | enum of two slices, built per step |
| stream entry `u32` | 4 | 4 | id, or doc marker with the top bit |
| `flat` entry `u16` | 2 | 2 | round-relative document |

No hot struct is shared for writing between threads: bitset words are written by one thread each (blocks
start at a multiple of 64 documents), `end` entries by the owner of the document.

## 3. Allocation audit (per run)

| Site | Count | Size | Verdict |
|---|---|---|---|
| `bits` per round | 8 | 1.9 MB | keep (freed each round) |
| thread `stream` per round | 96 | 0.55 MB avg | keep, pre-sized from the sample |
| thread `count` per round | 96 | 232 KB | keep |
| `start` per round | 8 | 464 KB | keep |
| `flat` per round | 8 | 3.3 MB | keep |
| answer buffers `r, v, merged` per thread per round | 96 x 3 | 4 KB to tens of KB | pool candidate: reused within a round only |
| scan `seen`, `ids` per thread per round | 96 x 2 | 16 KB | pool candidate |
| query tables before compaction | 2 | 4 MB + 1 MB | measured: growing them saves about 3 ms |
| output buffers | 12 | 1.5 MB total | keep |

Allocations total a few hundred per run, each a large block: the allocator is not on the hot path, so
jemalloc or mimalloc have nothing to win here and would add a dependency.

## 4. Copy audit

1. `masks`/`load` tail copies: only within 64 bytes of the file's end.
2. `r.copy_from_slice(b)`: a common term's bitset (3.9 KB) per query that loads it; now deferred until changed.
3. `bits`/`flat` `into_iter().map(into_inner).collect()`: in-place, no copy.
4. `outs.concat()` then one write: 1.5 MB once.

## 5. Concurrency audit

1. No locks on hot paths. Atomics: relaxed load/store on bitset words and `end`, plain atomics (no RMW).
2. Threads: each round spawns 12 threads three times (scan, place, answer), so a run spawns about 312
   threads; at roughly 20 to 50 µs each that is 6 to 15 ms of the 600, the largest overhead left outside
   the scan.
3. False sharing: none on hot data; per-query totals are written by the thread that owns a block of queries.

## 6. Candidates, ranked by expected gain

1. One set of 12 workers for the whole run, phases separated by a barrier: removes about 300 spawns
   (estimate 1 to 2% of the run). std only.
2. The per-round layout loop (ids x threads, single-threaded): split by id range across the workers
   (estimate about 1%).
3. Sample a 64th instead of a 32nd: about half of 12 to 22 ms; affects only which ids get bitsets, never
   the output.
4. Fewer rounds at the same memory needs smaller streams; a stream entry needs 17 bits of id, so this is
   a new index format (not attempted).
5. Allocator crates: not justified by the allocation audit (a few hundred large blocks).

## 7. Risk register

1. `std::arch::x86_64` AVX2 intrinsics (`masks`), `_mm_prefetch`: x86-64 with AVX2 only; the build targets
   the scoring CPU (`target-cpu=native`).
2. `unsafe`: `mmap` of the input (read-only, file not modified during the run); `zeroed` builds a `Vec` from
   `alloc_zeroed` (all-zero bytes are valid for the integer and atomic types used); prefetch pointers
   computed with in-bounds indexes.
3. A round must hold fewer than 2^16 documents (16-bit ids); the round count doubles until it does.
4. Term ids must stay below 2^29 (operator bits in a step); the query file has about 300,000 words.

## 8. Results

| Patch | Measurement | Decision |
|---|---|---|
| 1. One worker set for the run | 312 spawns of 12 threads: 20 to 30 ms on the laptop; one spawn and 26 barriers: about 9 ms (5 runs each). Saving 2 to 3% of a run for about 80 changed lines and 30 or more counted, worth about as much as the lines cost | Not done |
| 3. Sample a 64th | Same output. 12 paired full runs: 6 performance threads wall 759 to 809 ms (faster in 5/12), CPU less in 3/12; all cores 671 to 639 ms (6/12), CPU less in 5/12; peak unchanged | Dropped |
| 2. Parallel layout | The layout is 9 to 14 ms of about 600; below what the laptop resolves | Not done |
| Allocator crates | A few hundred allocations per run, all large blocks reused within a round | Not done |

Cumulative from today's kept Track B changes (AVX2 tokens, lookup prefetch, 16-bit ids, packed steps, bitset
totals, lazy bitset copy, merged ORs): board 0.327 to 0.384 s before, 0.311 to 0.339 s since; peak 31.2 to 36.4 MB
before, 30.3 to 31.2 MB since. Remaining headroom is in a new index format (section 6, item 4), not in the
memory subsystem of this one.
