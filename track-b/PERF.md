# Track B: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 0 |
| 1 | One scan indexes only the query terms and adjacent query-term pairs: common ids as bitsets, the rest as posting lists placed from per-thread hit streams | run-20244f1c82c7 | 0.4071 | 199.4 | 58.98 |

## Local experiments after step 1

Measured on a laptop (Intel Core i5-1335U: 2 performance and 8 efficiency cores, 12 threads) in Docker with the board's limits, on the full public data, running the old and new versions alternately. The laptop's speed drifts by up to 30% within a session, so a change counts through its paired runs; peak memory (the cgroup's `memory.peak`, in MiB) does not drift. Every version writes the `results.txt` whose SHA-256 the spec publishes (`f86973c2...`), and matches on the tiny, sample and seed-7 data too.

| Change | Median ms, old → new | New faster | Peak MiB, old → new | Kept |
|---|---|---|---|---|
| Rounds: the documents are indexed and queried an eighth at a time, so only one round's postings are held | 506 → 532 | 5 of 12 | 176 → 57 | yes, for memory |
| Answering: a dense result counted and summed from its bitset, OR of two lists merged, no allocation in the phrase check | 518 → 508 | | 63 → 57 | no, no clear gain |
| Bitsets from df ≥ n/64, or n/128, instead of n/32 | 530 → 544, 566 → 592 | | 57 → 56 | no, slower |
| A rare id reported once per document through a 4096-entry table of the last document seen; the term table carries an id's bitset index; no per-id state array | 563 → 509 | 12 of 12 | 56 → 45 | yes |
| Query tables sized from the queries' spaces, so no compaction pass | 526 → 495 | 11 of 12 | 45 → 45 | yes |
| Tokens found 8 bytes at a time and hashed from 8-byte chunks; the LF found in the same pass | 590 → 563, then 651 → 724 in a noisy session | 9 of 12, then 6 of 10 | 45 → 46 | yes |
| All kept changes against step 1 | 927 → 669 | 12 of 12 | 178 → 45 | |

Code lines (rustfmt, then scc, as the board counts them): 420 at step 1, 468 with the kept changes.
