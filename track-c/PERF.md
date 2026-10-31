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

A C++ port of the step 1 parser (commit 01c8a40) ran 3.5% faster on the board but its extra code lowered the score to 70.46, so it was reverted (commit 171aa0e).
