# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

## Approach

Rust, with two crates: `memmap2` maps both files, and `/data` is already in the page cache, so reading costs no copy and no memory; `memchr` finds single bytes, such as the tab after a document's id and the end of a line.

Only what the queries ask about is indexed. The queries are parsed first. Every distinct term gets an id, and so does every adjacent pair of terms in a phrase. A document's tokens are found 64 bytes at a time: AVX2 compares give a bitmask of its letters and digits and one of its LF, and a token starts at a set bit after a clear one and ends at the next clear bit. Each token's lowercased bytes are mixed into a 64-bit hash 8 bytes at a time, so a token of up to 7 bytes has a hash no other token has. A token's filter word and table slot are prefetched as soon as it is found, and it is looked up 16 tokens later, in order, so several lookups' cache misses overlap. The hash is looked up in an open-addressing table, sized from the queries, behind a 128 KB filter, two bits of one 64-bit word per key, that answers most misses without touching the table. Most adjacent pairs of query terms are not phrase pairs, so the pair table's filter does most of the work there.

The corpus goes in 8 rounds of one block per hardware thread, each block starting at a document whose id is a multiple of 64:

1. A sample, the first thirty-second of every block, estimates how many documents contain each id. An id expected in at least one document in 32 gets a bitset, which is smaller than a list at that density, and its table entry carries the bitset's index.
2. In each round, every thread scans its block once. A common id sets its bit at every occurrence, with no bookkeeping, since setting a bit again changes nothing; thanks to the 64-document alignment, no two threads ever write the same bitset word. Any other id is reported once per document, which a 4096-entry table of the last document seen at each id's low bits decides, and goes to the thread's stream after a marker naming its document.
3. Each thread counts its stream's hits per id. The counts give each thread its own part of every id's run in one flat array of ascending document ids, each 16 bits, counted from the round's first document (a round always holds fewer than 2^16), and each thread places its stream there.
4. Every query is answered on the round's documents, and its count, id sum and first ten ids are added to its totals. Then the round's postings are freed, so only an eighth of them is ever held.

A `BOOL` query is evaluated left to right. A common term's bitset is copied into the result on load, since at scoring scale sorting the reused query buffer beats maintaining a separate merge buffer; a result left as a bitset is never listed: its count, id sum and first ten come straight from its words (popcounts, and each word's sum of bit positions from six masked popcounts). The result stays a sorted list of ids while it can: AND and NOT filter it, and an OR with another list is satisfied by extending, sorting and deduping the same buffer. It moves to a bitset only to load or OR in a common term. Once the list is empty with no OR still to come, the answer is empty and the remaining steps are not read. A `PHRASE` of two terms is exactly the documents holding that adjacent pair. A longer phrase starts from the documents holding all of its pairs, which are very few, and checks each one's tokens for the whole sequence.

The binary is built for the scoring CPU (`target-cpu=native`) with profile-guided optimisation: during the offline build, a byte-identical copy of the published generator (`generator.py`, seed 2) makes a full-size training dataset, the instrumented binary runs on it, and `llvm-profdata` guides the final build. The runtime image holds only the Rust executable.

## Dependencies

`memchr` 2.7.4 and `memmap2` 0.9.5, vendored in `vendor/` with `libc` 0.2.169, which `memmap2` uses, so the build stays offline.

## With another 24 hours

Where a run's time goes now (laptop, phase timers): the scan of the documents about 60%, answering the queries about 18%, placing the postings about 8%, parsing the queries about 6%, the layout and the sample the rest. The scan already finds tokens with AVX2 bitmasks and prefetches each lookup 16 tokens ahead; prefetching the pair lookups and pre-faulting the round's arrays were measured and gave nothing. What is left in the other phases is worth one or two percent each, about what the lines would cost.

The next real step is a different index: delta-coded posting lists, so a round's postings take less memory and fewer rounds fit the same budget. Every round answers every query again, so fewer rounds would cut the answering and the per-round layout, which six rounds already showed (4% less time) but for 14% more memory with the present layout.
