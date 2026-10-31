# Track A: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 32.05 | 228 | 0 |
| 1 | Two passes: count, then place each latency; output streamed by hour | run-44123151dbca | 0.0628 | 33.3 | 70.99 |
| 2 | One scan into per-hour chunks; each hour counted, placed and formatted in cache, with no layout pass | run-895db16cd9de | 0.0332 | 36.6 | 76.29 |

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

The parse is about 60% of a laptop run. A bare newline count over the same bytes takes 7 to 8 ms, and the scan runs at two to three times that. Track A commits between rows 1 and 2 (0bcc05e, 5fbde5e, 695f577) have no rows: their board runs are no longer in the board data.
