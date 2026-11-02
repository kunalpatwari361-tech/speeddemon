# Track B: performance log

Every step from the reference to your final version, each with the live-board run ID that measured it. Judges check that the run IDs exist and that each change moved the way you say it did.

| Step | Change | Run ID | T (s) | M (MB) | Points |
|---|---|---|---|---|---|
| 0 | The reference, unchanged | | 56.35 | 2,200 | 0 |
| 1 | One scan indexes only the query terms and adjacent query-term pairs: common ids as bitsets, the rest as posting lists placed from per-thread hit streams | run-20244f1c82c7 | 0.4071 | 199.4 | 58.98 |
| 2 | Rounds: the documents are indexed and queried an eighth at a time, so only one round's postings are held | run-00de16a83ce2 | 0.4271 | 72.4 (69.0 MiB) | 62.57 |
| 3 | A rare id reported once per document, no per-id state; query tables sized from the queries; tokens found and hashed 8 bytes at a time (three commits, measured together) | run-73886d267709 | 0.4309 | 62.2 | 63.07 |
| 4 | Less memory during the rounds: one step list for all queries, 16-bit cursors, 32-bit bitset indexes, document ends only, compacted tables, no transparent huge pages | run-aaa7db0a16f5 | 0.3748 | 34.9 | 65.73 |
| 5 | Twelve rounds instead of eight | run-58d4e30251c5 | 0.3898 | 31.3 | 65.30 |
| 6 | A query stops once its result is empty with no OR to come (measured at twelve rounds) | run-a4766cffac8b | 0.4526 | 29.3 | 63.70 |
| 7 | Eight rounds again, with step 6 and a 128 KB two-bit filter in front of the tables (28fa01c, whose own run is no longer in the board data) | run-20d1c6cd1af2 | 0.3730 | 36.5 | 65.80 |
| 8 | Profile-guided build, source unchanged (e3048e6, measured on e95c13d) | run-4dfd8b2f22f7 | 0.4390 | 37.8 | 63.97 |
| 8, rerun | The same Track B build, run again on a later commit (931a5f7) | run-a147f3d7e4c3 | 0.3859 | 35.3 | 65.37 |

M is the board's peak memory in MB (10^6 bytes); steps 2, 4, 5 and 6 were read from the board in MiB and converted. The same Track B code has measured from 0.373 to 0.453 s on the board, so a single run cannot show a time change of a few percent; the memory figures are steady.

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

Rounds again: 12, 8 and 6, 6 runs each, laptop (a slow session, so totals near 1 s): median total about 1183, 1085 and 1022 ms; the answer phase about 251, 227 and 207 ms; the layout about 28, 19 and 12.5 ms; peak 27 to 29, 32 to 33 and 38 MiB. 12 was slowest in 5 of 6 rounds. Back to 8: about 8% less time for 4 to 5 MiB more, and the board's best run so far was at 8 rounds (run-aaa7db0a16f5), while 12-round runs measured 0.389 to 0.453 s. 6 was not kept: a little more time saved, but 38 MiB, close to the best Track B memory on the board.

Prefetching, dropped: while a query is answered, the run starts and bitset indexes of the query two ahead were prefetched, and from those the first line of each list or bitset of the next one. 8 rounds, 8 paired runs: answer phase median 243 to 227 ms, faster in 4/8, but as a share of each run's full pass 0.345 against 0.342, so no measurable gain; the lists a query reads per round are small and mostly already cached.

Profile-guided build, trained on seed-2 full data from a byte-identical copy of the published `generators/b_index.py` (exempt from the code lines). Laptop, 10 paired full-data runs against the 9013f4d build: median 601 to 539 ms, faster in 7/10, same peak memory and output. Source unchanged.
On the board (step 8) its two runs measured 0.439 and 0.386 s, inside the 0.373 to 0.453 s the unprofiled build has shown, so the board has not resolved a change in time. S rose from 519 to 529 lines. Kept for now.

Answer phase, three changes together: an OR of two id lists is merged as two ascending runs (the stable sort finds the runs and merges them in one pass, instead of sorting the joined list); an AND or AND-NOT against an id list gallops (steps of 1, 2, 4, ... then a binary search) instead of stepping one id at a time; the phrase check reuses one token list instead of allocating one per candidate document. All four datasets matched. Laptop, phase timers, 12 paired full runs: answer phase median 169.3 to 152.0 ms, faster in 9/12. The answer phase is about a fifth of a run (the full pass over the documents about 470 ms), so whole runs, 12 paired: median 829 to 847 ms, faster in 7/12, peak 32 to 34 MB both: inside the laptop's spread.

Follow-up, not kept: a separate binary-search threshold for highly skewed AND lists, using scratch storage, kept the seed-1 hash. Eight paired Windows-Docker timer runs measured 1.78 s on the committed galloping version and 1.82 s on the trial, with the trial faster in 3/8. It did not improve on the existing exponential gallop and was discarded.

Memory, later: traced from inside the container, the fixed structures take about 11 MB (the query steps, the per-query totals, the tables) and a round about 18 MB, mostly each thread's stream of rare ids (6.8 MB) and the flat array they are placed into (6.5 MB), both alive while placing. A round holds fewer than 2^16 documents, so the flat array now stores each document as 16 bits, counted from the round's first. All four datasets matched. 12 paired full runs: peak 32 to 33 MB before, 29 to 30 MB after, lower in 12/12; median 768 to 679 ms, faster in 9/12. More rounds would cut the streams too, but every round answers every query again: twelve rounds measured 31.3 MB at 0.390 s on the board (65.30 points, step 5).

Tokens from bitmasks: where the scan looked for each token's start and end 8 bytes at a time, AVX2 compares now give a bitmask of the letters and digits and one of the LF for 64 bytes at once; a token starts at a set bit after a clear one and ends at the next clear bit, and one running past the block is read as before. The hash is unchanged, so every key is the same. All four datasets matched. Phase timers, 10 paired full runs: sample and full passes together 439 to 397 ms on the 8 efficiency cores (faster in 9/10) and 428 to 347 ms on all cores (9/10). Whole runs, 12 paired: median 629 to 639 ms, faster in 8/12, peak 29 to 31 MB both: the gain is inside the laptop's whole-run spread.

Query steps packed: a step was an (operator, id) pair, 8 bytes with padding; it is now one u32 with the operator in bits 29 and 30 (ids stay far below 2^29, and a pair id keeps the top bit). About 300,000 steps, so about 1.2 MB less for the whole run. All four datasets matched. 12 paired full runs: peak 29 to 31 MB before, 29 to 30 MB after (29 MB in 11/12 against 4/12), lower in 7/12; median 498 to 514 ms, faster in 7/12, so no change in time.

Rounds again, on the current build: 10 interleaved full runs each, medians: 8 rounds 622 ms wall and 30.1 MB peak; 6 rounds 596 ms and 34.3 MB; 5 rounds 574 ms and 37.4 MB (same output). With time weighted 0.5 and memory 0.2, 4 to 8% less time does not pay for 14 to 24% more memory, so 8 rounds stay.

Tried, not kept: the shared arrays each round first-touches (the bitsets, the flat array, the document ends) faulted in with one madvise(MADV_POPULATE_WRITE) call each, so the threads do not queue on page faults. Same output; 10 paired full runs on 6 performance threads: wall 801 to 761 ms (faster in 5/10), CPU 3,591 to 3,497 ms (less in 5/10); on all cores: wall 584 to 611 ms (4/10), CPU 4,223 to 4,351 ms (5/10); peak unchanged. Page faults are not what Track B waits on.

Kept: a token's filter word and home slot are prefetched as soon as it is found, and it is looked up AHEAD tokens later, in order (the document's last ones at its LF), so pairs and hits are unchanged and several lookups' misses overlap. All four datasets matched; peak unchanged (about 28 to 30 MB). 8 tokens ahead, 12 paired full runs: on 6 performance threads wall 580 to 545 ms (faster in 9/12), CPU 2,743 to 2,602 ms (less in 9/12); on all cores wall 454 to 431 ms (7/12), CPU 3,399 to 3,186 ms (10/12). 10 more runs on the 6 threads: wall 586 / 550 / 543 ms and CPU 2,838 / 2,617 / 2,570 ms for none / 8 / 16 ahead, so 16.

Tried, not kept: a pair's lookup prefetched when its second term is found and done 16 pairs later (the document's waiting pairs at its LF; the order of hits within a document does not matter). Same output; 12 paired full runs on 6 performance threads: wall 776 to 767 ms (faster in 7/12), CPU 3,669 to 3,775 ms (less in 6/12); on all cores: wall 622 to 630 ms (5/12), CPU 4,465 to 4,551 ms (4/12). Most pair lookups end at the 128 KB filter, which stays in cache.

Where a run goes now (laptop, 5 runs, phase timers): parsing the queries 34 to 55 ms, sample 12 to 22, full pass 340 to 400, layout 9 to 14, placing 42 to 66, answering 133 to 222, writing about 4. Tried, not kept: the query tables started at 4,096 slots and doubled before two thirds full, instead of being sized for every word of queries.txt up front. Same output; 8 interleaved runs: parsing 27 to 29 ms median before and 22 to 35 after, compacting 4.5 to 3.0 ms: about 3 ms of a 600 ms run.


## Additive change: allocation-free phrase verification (not yet benchmarked)

Added `src/opt.rs` and wired it into the existing phrase-candidate verification path. The existing index, postings representation, BOOL evaluator, output format, and all other tracks are unchanged. The new verifier streams ASCII-alphanumeric tokens directly from the memory-mapped document slice and compares them case-insensitively; it does not allocate a per-candidate `Vec<&[u8]>` for document tokens or a phrase-token vector. This should reduce transient allocations and allocator work for PHRASE queries. It has not yet been benchmarked on the official datasets, and no speedup or peak-RAM reduction is claimed until the full checker confirms byte-identical output and measurements.


## Additive change: linear-time empty-result skipping (not yet benchmarked)

When a BOOL evaluation's sorted result becomes empty, the evaluator now searches once for the next OR and jumps directly to it, or stops if none remains. Previously it rechecked the remaining suffix at each AND/NOT step, which could repeatedly scan the same operators when an OR occurred later. This preserves left-to-right evaluation and avoids that repeated control-flow work. It has not yet been benchmarked on the official datasets; no speedup is claimed until the full checker confirms identical output and measurements.
