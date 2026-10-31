# Track B: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 0 |
| 1 | One scan indexes only the query terms and adjacent query-term pairs: common ids as bitsets, the rest as posting lists placed from per-thread hit streams | run-20244f1c82c7 | 0.4071 | 199.4 | 58.98 |
| 2 | Rounds: the documents are indexed and queried an eighth at a time, so only one round's postings are held | run-00de16a83ce2 | 0.4271 | 72.4 (69.0 MiB) | 62.57 |
| 3 | A rare id reported once per document, no per-id state; query tables sized from the queries; tokens found and hashed 8 bytes at a time (three commits, measured together) | run-73886d267709 | 0.4309 | 62.2 | 63.07 |

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

On the board the memory fell as measured here, but the time did not: the laptop put the three changes of step 3 at about 22% faster, and the board measured 0.4271 s before them and 0.4309 s after. The same unchanged Track D code has measured from 0.381 s to 0.448 s on the board, so a few percent either way is within its noise.

Memory, later: no transparent huge pages (prctl); every query's steps in one list, a phrase's words re-read from queries.txt; 16-bit places in a round's runs (more rounds if one would hold 2^16 documents); 32-bit bitset indexes; only each document's end kept; term and pair tables compacted after parsing. Laptop, 10 paired runs: peak 44 to 45 MB before, 32 to 33 MB after, in every run; median 568 to 557 ms, faster in 6/10.

Tried after that, not kept: the queries parsed on all threads, each putting keys in shared tables by compare-and-swap, with a pair's key made from its terms' keys, then the keys numbered, the tables compacted and the steps given ids on all threads. The parse phase went from about 33 ms (25 to 56) to about 24 ms (22 to 38) over 5 runs each, since the threads contend for the same table lines and fresh pages, and compacting on all threads was slower than on one. End to end, 10 paired runs: 653 to 723 ms median, faster in 6/10. It added about 50 lines, so it was dropped.

Rounds, later: 12 instead of 8. Laptop, 6 runs each: peak 33 MiB (one run 39) with 8 rounds, 28 MiB in every run with 12, 25 to 26 MiB with 16; times 551 to 843, 524 to 811 and 611 to 1222 ms, too spread to show the cost of more rounds. 12 was kept: 5 MiB less, with the smaller share of extra query passes.

Query planning, later. An early stop, kept: once a query's list is empty with no OR still to come, its remaining steps are not looked up; at full scale it skips 355,592 of 1,606,848 step evaluations across the 12 rounds (22%). Cost-based order, dropped: each run of AND and NOT steps sorted by the sample's estimated counts, the rarest first and the NOTs last, skipped 418,435 (26%), only 4 points more, for 4 ms of one-thread sorting and about 25 lines. On the laptop the answer phase measured 140 to 178 ms before and 125 to 180 ms with both, over 6 runs each, too spread to show either change. 349 of the 50,000 query lines are repeats and 1,291 are a single term, too few to pay for sharing results or a separate path.

Term and pair tables, later. A single-threaded benchmark replayed the scan's lookups (each token in the term table, each adjacent pair of query terms in the pair table) on the full-scale queries (75,747 terms, 40,350 pairs) and the 6,081,631 tokens of the first 40 MB of docs.txt, 9 rounds per design, every design returning the same lookups. Fastest round, ns per token: linear probing behind the 64 KB one-bit filter (the design then) 10.7; Robin Hood behind the same filter 14.2; linear probing with no filter 22.9; Robin Hood with no filter 22.4; four keys per 64-byte bucket 22.3, or 13.9 to 15.7 behind the filter; Swiss-style groups of 16 one-byte tags compared with SSE2 29.0. Then the filter, two runs: 64 KB one bit 12.5 and 11.5; 128 KB one bit 11.1 and 10.4; 256 KB one bit 10.9 and 9.9; 64 KB two bits 11.8 and 10.6; 128 KB two bits 10.9 and 9.96 (medians 14.8 and 11.0, against 15.7 and 12.1). Kept: the 128 KB filter with two bits of one word per key, about 8 to 13% faster lookups for 128 KB more memory in the two tables; lookups are about a third of the scan. Its laptop full runs could not be timed, as other sessions were benchmarking on the same machine.
