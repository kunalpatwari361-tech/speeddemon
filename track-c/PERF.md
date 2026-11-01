# Track C: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 14.44 | 367 | 0 |
| 1 | Parts parsed in parallel into per-part totals per country; a quoted field's end found 8 bytes at a time; distinct names marked in one bitset per country over a shared name table | run-af0ccced31f5 | 0.0651 | 11.4 | 71.11 |
| 2 | Field ends from AVX2 bitmasks (quotes prefix-xored mark the bytes inside quotes); numbers parsed 8 digits at once; names hashed 8 bytes at a time | run-8b6e18ce9bbe | 0.0595 | 11.3 | 71.61 |
| 3 | The region table freed before the parse; no transparent huge pages | run-d3822c6b0947 | 0.0593 | 9.9 | 72.17 |
| 3, rerun | The same Track C code, run again on a later commit (03d11db) | run-016234f2ccc9 | 0.0589 | 9.5 | 72.44 |
| 4 | Profile-guided build, with a byte-identical copy of the published generator (e95c13d) | run-3815089f02e8 | 0.0573 | 9.8 | 72.48 |
| 5 | Each country's name bits owned by one thread and set with plain stores after each of 64 pieces, instead of an atomic fetch_or per record; 299 to 320 code lines (9898558) | run-5a82e375abf8 | 0.0562 | 10.1 | 72.31 |

M is the board's peak memory in MB (10^6 bytes); step 3 was read from the board in MiB and converted.

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

Where the parse's time goes now (laptop, full scale, 10 interleaved runs each, switching parts off): 106 ms median as it is, 88 ms without marking distinct names, 48 ms without hashing, probing and marking names. So the names cost about 55% of a run, and marking them about a sixth.

| Change | Median ms, old → new | New faster | Kept |
|---|---|---|---|
| Name table of 32-bit tags (512 KB) instead of 64-bit hashes (1 MB), so it stays in cache beside the bitsets | 135 → 144 | 7 of 20 | no, no gain, and two names would merge about once in 7,000 runs |
| Non-temporal prefetch of the records 512 bytes ahead, so the stream does not push the name table and the bitsets out of the caches | 150 → 173 | 9 of 20 | no, no gain |
| The 32-bit name table again, on the PGO build (three sessions of 12, 16 and 20 pairs) | 80 → 78, 73 → 70, 88 → 83 | 29 of 48 | no: peak 8 MB in 16 of 20 runs against 8 to 9 MB, but no reliable time gain, and the merge risk stands |

## Local full-data experiments (not board runs)

Distinct-name marking now uses one atomic `fetch_or` per record instead of first loading the word and branching when the bit is clear. Setting the same bit more than once has the same result, so this preserves the exact counts while avoiding a separate atomic load and branch. On the official seed-1 full data in Windows Docker, 10 interleaved process runs had median times of 1.798 s before and 1.432 s after; the candidate was faster in 7 of 10 pairs. Every run produced the published full output hash. These host timings are noisy and are not a live-board result.

Profile-guided optimisation is trained during the build on seed-2 full data generated by the unchanged public generator. Compared with the unprofiled `fetch_or` build, 10 interleaved full-data runs had median process times of 1.567 s and 1.474 s; PGO was faster in 6 of 10 pairs. Both variants produced the official full output hash. The figures include Windows Docker host noise and are not a board result.

On the board, the profile-guided build (271df67, measured on e411dbf as run-215ea4b765a3) ran in 0.0561 s against the best 0.0589 s, but the board counted the bundled 362-line Python generator as Track C code: 736 lines instead of 289, and 69.55 points instead of 72.44. The build was reverted to the 03d11db version, with its source, so the generator and the one-atomic-per-record name marking went with it.

Profile-guided build again, now with the generator byte-identical to the published `generators/c_join.py` (LF line endings, SHA-256 9e61672e...). The board exempts such a copy from the code lines; the first attempt's copy had Windows CRLF line endings, so it was counted. Source unchanged from the best run (03d11db).

Where a run goes (laptop, 15 interleaved full runs, medians): the whole run 129 ms; without setting the name bits 88 ms; without the name lookup either 67 ms. Setting the bits, an atomic fetch_or per record on the 3.9 MB of per-country bitsets (240 countries), was the largest part. A slot holding the name's hash and all its country bits in one 64-byte line (8 MB in all) did not help: 16 paired runs, median 172 to 171 ms, faster in 5/16, peak 8 to 9 MB against 12 to 13 MB. Dropped.

Kept: each country's bits are owned by one thread (rank modulo the thread count). Each thread parses its records in pieces; after each piece the bit indexes go to their owners, and after a barrier each owner sets its bits with plain stores. All four datasets matched. The laptop's two core types make a barrier wait for the slowest core, so wall time there was mixed (16 paired runs, 108 to 115 ms median, faster in 9/16); pinned to the 8 efficiency cores, 12 paired runs: 171 to 103 ms, faster in 10/12; on the 2 performance cores (4 threads): 204 to 176 ms, faster in 8/12. Total CPU time, which barrier waits do not add to, on the 8 efficiency cores: 793 ms to 615 ms with 16 pieces per thread (lower in 11/12) and 648 ms with 64 (10/12); a second set of 10: 629 ms against 493 with 32 and 485 with 64. Peak memory: 8 to 9 MB before, 10 to 13 MB with 16 or 32 pieces (the hand-off buffers), 9 MB with 64. Kept with 64 pieces.

Additional Windows-Docker full-data checks, not board runs: the owner-routed implementation and the previous atomic-bitset implementation both produced the published seed-1 hash `d8088b1cd42ba1338b1345b107b55db7b96ef1f5066f3ff64fd26a8912a0f536`. In eight interleaved runs on this host, owner routing had a 1.13 s median against 0.75 s and was slower in all eight. A separate design with one full name bitset per worker also matched the hash but measured 0.79 s against 0.74 s, faster in 2/8, and raised peak memory from 214 to 257 MB; dropped. Retrying unconditional `fetch_or` against load-then-`fetch_or` measured 1.21 s against 1.22 s and was faster in 5/8; no repeatable improvement.

On the board, owner routing (step 5, commit 9898558) measured 0.0562 s and 10.1 MB, but scored 72.31 versus 72.44 for the earlier implementation; its code grew from 299 to 320 lines. The time gain did not cover the memory and source-size costs. Decision: restore the earlier source and retain the PGO build, whose run scored 72.48 (run-3815089f02e8).

Further local trial, not retained: the later one-barrier variant in the main checkout passed the full seed-1 hash. With the official timer in Windows Docker, eight interleaved runs against the PGO pre-route build measured medians of 1,180.7 ms and 1,636.0 ms; the trial was faster in 1/8. Peak cgroup memory was 214.9 MB and 215.7 MB. This host result does not replace a board run, but it gives no local support for the barrier change.

A portable fallback for `masks` on CPUs without x86-64 (f036636, merged in f725fe6) was measured on the board at 0b33a45 (run-06390194c566): 0.0582 s, 9.9 MB, 315 code lines, 72.07 points. The scoring CPU is x86-64, so the fallback never runs there and only adds lines; the source is back to the 299-line version (run-3815089f02e8, 72.48 points). All four datasets matched.
