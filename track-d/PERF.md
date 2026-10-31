# Track D: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 72.69 | 893 | 0 |
| 1 | Packed radix heap for the search, its buckets kept between queries | run-4a3cd5f77eb4 | 0.3426 | 147.3 | 59.36 |

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
| Give each search's distance-table pages back after it (madvise), so a thread's table holds memory only while a search runs | search, memory | full runs, 20 rounds: 671 to 1043 ms median, slower in 20/20; peak 131 to 139 MiB before, 111 to 112 MiB after, in every round | Dropped: re-touching the freed pages costs far more time than the memory saves |

Memory, later: no transparent huge pages (prctl), and the old-to-new numbering (7 MB) is freed before the searches, once the queries are renamed. Laptop, 6 runs each: peak 130 to 139 MiB before (median about 137), 133 to 138 MiB after (median about 134); the searches' per-thread distance tables, about 68 MiB, set the peak.

Where the memory goes (laptop, full scale, the cgroup's peak read after each phase, 2 runs): the parse holds every edge as a triple, 66 MiB; building the adjacency array next to them peaks at 111 MiB; the renumbering stays below that; the searches then lift the peak to 130 to 132 MiB with each thread's distance table. Below 111 MiB the build itself would have to change, since it holds the triples and the adjacency array, or the two adjacency arrays while renumbering, at the same time.
