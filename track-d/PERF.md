# Track D: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 72.69 | 893 | 0 |
| 1 | | | | | |

## Experiments on the laptop (not board runs)

The laptop's timings move widely within a session (one build's full runs ranged from 0.7 to 1.5 s), so each experiment was judged on interleaved paired runs. Phase timers on the untouched build, full scale: search 380 to 410 ms of about 700 ms, parse 77 to 120 ms, adjacency build about 100 ms, renumbering about 120 ms. "Search harness" below is the search phase timed inside one process over five repeats, taking the best.

| Experiment | Where | Paired result | Decision |
|---|---|---|---|
| Radix heap, 16-byte entries (distance and node), buckets allocated per search | search | full runs, 10 rounds: 1009 to 1144 ms median; memory 129 to 146 MiB | Dropped: slower |
| Radix heap, 16-byte entries, buckets kept between searches | search | search harness, 8 rounds: 521 to 534 ms, faster in 3/8 | Dropped: no gain |
| Binary heap kept between searches (workspace alone) | search | search harness, 8 rounds: 421 to 444 ms, faster in 3/8 | Dropped: no gain |
| Packed radix heap, 8-byte entries, buckets kept between searches | search | search harness: 468 to 426 ms (8 rounds) and 463 to 390 ms (10 rounds), faster in 18/18; full runs, 20 rounds: 917 to 868 ms median, faster in 12/20; memory 129 to 140 MiB | Kept: about 5% of a full run. The memory increase is probably the radix redistribution, which briefly holds two copies of the largest bucket (not measured separately) |
| Packed radix, buckets capped at 1,024 entries after each search | search, memory | search harness: 390 to 407 ms, slower than uncapped in 6/10 rounds; full runs: 128 to 138 MiB | Dropped: slower, and saves only 2 MiB |
| Source grouping: one search per run of queries with the same source, stopping at its last target | search | search harness, 8 rounds: 421 to 425 ms, faster in 3/8. At full scale 1,979 distinct sources among 2,000 queries | Dropped: no gain; at most 21 searches could be shared
