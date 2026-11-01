# Track A: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 32.05 | 228 | 0 |
| 1 | Two passes: count, then place each latency; output streamed by hour | run-44123151dbca | 0.0628 | 33.3 | 70.99 |
| 2 | The line scanner in C++ (word-at-a-time space search, endpoint hash 8 bytes at a time), layout and output in Rust | run-382acfea3b49 | 0.0495 | 35.5 | 72.45 |
| 3 | One scan into per-hour chunks; each hour counted, placed and formatted in cache, with no layout pass | run-895db16cd9de | 0.0332 | 36.6 | 76.29 |
| 4 | Nested selection for the percentiles; chunks as 4 KB pages from one arena per part, given back per hour (measured on commit 2e67c3e, which has them) | run-c8c29b53bf85 | 0.0316 | 36.2 | 76.68 |
| 5 | The chunk arena refuses transparent huge pages, so a finished hour's pages really go back | run-557e5bbdbd84 | 0.0305 | 27.9 | 78.17 |
| 6 | An hour's chunk pages go back as soon as its latencies are placed, before formatting | run-b12eb0f51019 | 0.0304 | 25.9 | 78.53 |
| 7 | One 4-byte record per line: endpoint slot, 5xx bit, overflow bit, 18-bit latency or overflow index | run-f39affb61c4b | 0.0309 | 23.7 | 78.68 |
| 7, rerun | The same Track A code, run again on a later commit (9013f4d) | run-f904aa268b33 | 0.0309 | 20.7 | 79.28 |

M is the board's peak memory in MB (10^6 bytes); steps 5 to 7 were read from the board in MiB and converted. The board's timing varies by several percent between runs of the same code, as the two runs of step 7 show.

## Experiments on the laptop (not board runs)

The laptop cannot resolve effects under about 5%. The same binary's paired medians moved between 38 and 69 ms in one session, with two performance cores, eight efficiency cores and host memory pressure. Each experiment below was run interleaved against the step-2 build and judged on paired rounds. None was kept.

| Experiment | Where | Paired result | Decision |
|---|---|---|---|
| Reuse output buffers across hours (mutex-guarded pool) | hour phase | 10 rounds: 62 to 54 ms median, faster 8/10; 20 more: 69 to 68, faster 11/20 | Dropped: no reliable gain |
| Select the three nearest-rank values instead of sorting each run | hour phase | 20 rounds: 62 to 62 ms, faster 10/20 | Dropped: no gain |
| Write decimal digits in place, without a temporary buffer | hour phase | 20 rounds: 56 to 54 ms, faster 13/20; 40 more: 50 to 50, faster 18/40 | Dropped: no gain |
| Full SIMD scanner: 64-byte space search, masked hash, 16-byte newline search, branch-free digits | parse (harness) | 10 rounds: 22.0 to 33.3 ms median | Dropped: slower |
| Branch-free latency digits (two 8-digit words) | parse (harness) | 10 rounds: 25.2 to 24.0 ms median; other windows inconsistent | Dropped: not reliable |
| Software prefetch 1 KB and 4 KB ahead | parse (harness) | 5 rounds: 25.7 to 27.9 and 26.8 ms median | Dropped: no gain |
| Dynamic split: four parts per thread, taken from a shared counter | parse (harness) | 10 rounds: 25.2 to 23.1 ms median; best 15.0 to 15.7 ms | Not kept: best times mixed; the board has no efficiency cores |
| Ablations: no digit arithmetic, no hash mixing, no endpoint table, no endpoint loop | parse (harness) | each within the run-to-run spread | Diagnostic only |

Later experiments, each run interleaved against the build before it:

| Experiment | Where | Paired result | Decision |
|---|---|---|---|
| Nested selection: p50 in the whole run, p95 among the values above it, p99 among those above p95 | hour phase | 12 rounds against step 2: 76 to 67 ms median, faster 6/12; the percentile step's own CPU time, 5 runs each: 72 to 78 ms sorting, 53 to 65 ms selecting | Kept: about a quarter less work; the whole-run effect is below the laptop's resolution |
| Chunks are 4 KB pages of 680 records from one arena per part; a finished hour's pages go back with madvise, adjacent pages in one call | memory | 12 rounds: 84 to 70 ms median, faster 10/12; peak 33 to 35 MB before, 24 to 25 MB after, in every round | Kept: glibc kept freed chunks, so the 10 MB output was added on top of them |
| Endpoint lookup cost: full lookup, hashing without the table, a byte search for the endpoint's end | parse | scan 29 to 46, 24 to 50 and 31 to 63 ms over 6 runs each | Diagnostic: the lookup costs nothing measurable, so it is unchanged |
| Why step 3's memory did not fall on the board: the arena asked for huge pages (madvise MADV_HUGEPAGE), as a kernel with transparent huge pages always on gives them | memory | peak 35.0 to 38.4 MB over 4 runs, as on the board (36.2); without huge pages (MADV_NOHUGEPAGE) 26.1 to 26.6 MB | The arena now refuses huge pages: a 2 MB page holds many hours' chunks, and giving back 4 KB of it only splits it later |

The parse is about 60% of a laptop run. A bare newline count over the same bytes takes 7 to 8 ms, and the scan runs at two to three times that. Two earlier Track A versions, before row 1, were also measured: dc7aa80 as run-a452eefc5c14 (69.93 points) and 5fbde5e as run-12533a696726 (69.89 points).

Memory, later: an hour's chunk pages go back as soon as its latencies are placed, before its lines are formatted, so they no longer overlap the 12 threads' hour buffers. Laptop, 8 runs each: peak 25.7 to 27.1 MB before (median about 26.1), 23.0 to 24.4 MB after in 7 of 8 runs (median about 23.7); time unchanged.

Memory, later: a record is one 32-bit word, the endpoint's slot (12 bits), a 5xx bit, an overflow bit and 18 bits holding the latency, or for a latency of 2^18 or more its index in the part's overflow list (5.4% of lines at full scale), so a chunk page holds 1,020 records instead of 680. Laptop, 12 paired runs: peak 22 to 26 MB before, 19 to 22 MB after, lower in every run; median 42 to 45 ms, faster in 6/12 (the first rounds noisy, the last six all faster), so no clear change in time.

Tried again, not kept: the latency's digits converted all at once when its LF is among the next 8 bytes (up to 7 digits), one word, three multiplies, with the digit loop for longer latencies and near a part's end. Scan phase, 20 paired runs: the first 10 (a noisy session) 36.0 to 33.4 ms median, faster in 7/10; the next 10 (a calm one) 21.6 to 22.2 ms, faster in 3/10. No reliable gain, as with the two-word version, so it was dropped: nearly every latency has the same few digits, so the loop's branch is predicted well.

Percentiles, later. Group sizes at full scale: 126,908 groups, 63% with 8 latencies or fewer and 81% with 16 or fewer; the 1.2% over 256 hold 46% of the records. Selection time summed over the threads, by size, median of 8 runs: about 30 ms for groups of 16 or fewer, 22 ms for 17 to 256, 11 ms above 256, about a quarter of the hour loop, whose other three quarters format the lines. Sorting the small groups instead of the nested selection (then reading the three ranks): up to 16 latencies, 64.6 to 77.8 ms in all (slower); up to 64, 64.6 to 60.5 ms, about 0.3 ms of wall time. Neither kept. The selection is already quickselect with a guaranteed linear bound (Rust's select_nth_unstable), nested for the three ranks, run on all threads one hour each, into a buffer each thread reuses, with the 5xx count taken in the same pass as the request count; latencies are full 32-bit values, so a histogram would not be exact without a second pass.

Locked in for the freeze: the source is back to the step-7 version measured by run-f904aa268b33. Commit 1015004 (no hash step for endpoints of a multiple of 8 bytes, and a shortcut for lines with no errors) matched every output; its two board runs measured 0.0369 s (76.45) and 0.0312 s (78.93), no better than step 7, so it was taken out and the frozen code is the one with the best run.
