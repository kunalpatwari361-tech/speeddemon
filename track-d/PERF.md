# Track D: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 72.69 | 893 | 0 |
| 1 | Edges parsed in parallel into a compressed adjacency; nodes renumbered by parallel breadth-first regions for locality; queries sorted by source; Dijkstra with a binary heap, stopping at the target, over 32-bit distances | run-3bd4a30bbaf0 | 0.3808 | 131.9 | 59.21 |
| 2 | Packed radix heap for the search, its buckets kept between queries | run-4a3cd5f77eb4 | 0.3426 | 147.3 | 59.36 |
| 2, rerun | The same Track D code, run again on a later commit (d837159) | run-4c7a04395e32 | 0.3416 | 147.4 | 59.38 |
| 3 | The old-to-new numbering freed before the searches; no transparent huge pages | run-4acff2494398 | 0.3901 | 142.7 | 58.01 |

M is the board's peak memory in MB (10^6 bytes); steps 1 and 3 were read from the board in MiB and converted. Step 3's time is within the spread the same code shows from run to run on the board (0.342 to 0.448 s).

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

## Local full-data experiments (not board runs)

The bidirectional-search candidate produced the official seed-1 full output hash but was slower in all 8 interleaved pairs against the unidirectional radix search. In this Windows Docker run, median process time rose from 2.009 s to 2.834 s. The reverse graph and second distance table were dropped.

Grouping exact duplicate queries (full hash correct) was also dropped: the candidate was faster in 3 of 8 pairs, with median time 1.940 s versus 1.816 s for the baseline. The grouped candidate also skipped sources with no outgoing edges, but the combination did not beat the baseline.

A chunked decimal parser with 64-bit delimiter detection matched the full hash. In two separate 8-pair batches, baseline-to-parser medians were 2.552 s to 2.317 s, then 1.543 s to 1.553 s; it was faster in 5 of 8 pairs in each batch but did not show a repeatable median gain. It adds about 32 Rust code lines, so it was dropped.

PGO trained on full data from seed 2 and seed 1 both matched the full hash. Seed 2 was slower in the 8-pair comparison (median 3.065 s versus 2.888 s); seed 1 was effectively tied (1.637 s versus 1.641 s, 4 of 8 pairs faster). Neither profile was retained. All these Windows Docker timings varied widely and are local experiments, not board results.

Locked in for the freeze: the source is back to the step-2 version (the radix heap, as measured by run-4c7a04395e32). Every run after step 3 measured 0.386 to 0.393 s against 0.342 s for step 2, so step 3's 4 MiB saving is not worth the doubt.

Compact exact distance pass, not a board run: the first Dijkstra pass stores distance+1 in `u16`; the six generated answers that exceed that range are retried in the existing `u64` table, and a touched-node list resets only values actually reached. The official seed-1 output hash remained `a23dbdf0a3e49cc8901310a9ee1dc6a9a569a85297b6da75c2a7002cb2ed277e`. Six paired Windows-Docker runs measured 1.738 s / 233,086,976 bytes for `u32` versus 1.783 s / 212,680,704 bytes for `u16` plus fallback: 2.6% slower, 8.8% lower peak memory. Using the published 50% time and 20% memory weights, that local pair implies about +0.12 machine points before other axes; it is not a live-board score.
