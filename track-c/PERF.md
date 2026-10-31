# Track C: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 14.44 | 367 | 0 |
| 1 | Parts parsed in parallel into per-part totals per country; a quoted field's end found 8 bytes at a time; distinct names marked in one bitset per country over a shared name table | run-af0ccced31f5 | 0.0651 | 11.4 | 71.11 |

## Local experiments after step 1

Measured on a laptop (Intel Core i5-1335U: 2 performance and 8 efficiency cores, 12 threads) in Docker with the board's limits, on the full public data, running the old and new versions alternately. Every version writes the same `aggregates.txt` and matches on the tiny, sample and seed-7 data.

Where step 1's parsing time goes, by switching parts off (median of 5 runs): reading the fields about 39 ms, the region lookup and totals about 6 ms, hashing the name and probing the name table about 20 ms, marking distinct names about 13 ms; reading `regions.csv` beforehand takes 5 to 10 ms on one thread.

| Change | Median ms, old → new | New faster | Kept |
|---|---|---|---|
| Field ends from 64-byte bitmasks: quote bits prefix-xored mark the bytes inside quotes, and the commas and LFs outside them end the fields; masks from 64-bit word arithmetic | 126 → 164 | 1 of 10 | no, slower |
| The same, masks from AVX2 compares | 111 → 119 | 5 of 10 | no, no gain: finding the field ends is not what limits it |
| The AVX2 field ends, plus each number's digits parsed in one 8-byte step and each name hashed 8 bytes at a time | 96 → 85 | 10 of 12 (1 tie) | yes, pending a board run: 240 → 285 code lines cost about 0.6 points, so it needs about 5% less time there |

A C++ port of the step 1 parser (commit 01c8a40) ran 3.5% faster on the board but its extra code lowered the score to 70.46, so it was reverted (commit 171aa0e).

Memory, later: no transparent huge pages (prctl), and the 2 MB region-to-country table is freed once each region has its country's rank, before the parse. Laptop, 8 runs each: peak 11.0 to 11.6 MB before, 9.1 to 9.6 MB after in 6 of 8 runs (12.8 and 13.1 MB in the other two, the usual run-to-run spread).

Tried after that, not kept: regions.csv read on all threads, a part of its lines each, in two passes (countries used, then each region's rank written into its thread's own range of ids). The regions phase went from 5.0 to 10.6 ms (median about 6.6) to 3.2 to 7.2 ms (median about 4.8), faster in 5 of 6 paired runs; what is left is starting the threads and first touching the table. About 3% of a run against about 34 more code lines, roughly no change in points, so it was dropped.
