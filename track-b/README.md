# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

## Approach

Rust, standard library only. Both files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

Only what the queries ask about is indexed. The queries are parsed first. Every distinct term gets an id, and so does every adjacent pair of terms in a phrase. A document's tokens are found 64 bytes at a time: AVX2 compares give a bitmask of its letters and digits and one of its LF, and a token starts at a set bit after a clear one and ends at the next clear bit. Each token's lowercased bytes are mixed into a 64-bit hash 8 bytes at a time, so a token of up to 7 bytes has a hash no other token has. A token's filter word and table slot are prefetched as soon as it is found, and it is looked up 16 tokens later, in order, so several lookups' cache misses overlap. The hash is looked up in an open-addressing table, sized from the queries, behind a 128 KB filter, two bits of one 64-bit word per key, that answers most misses without touching the table. Most adjacent pairs of query terms are not phrase pairs, so the pair table's filter does most of the work there.

The corpus goes in 8 rounds of one block per hardware thread, each block starting at a document whose id is a multiple of 64:

1. A sample, the first thirty-second of every block, estimates how many documents contain each id. An id expected in at least one document in 32 gets a bitset, which is smaller than a list at that density, and its table entry carries the bitset's index.
2. In each round, every thread scans its block once. A common id sets its bit at every occurrence, with no bookkeeping, since setting a bit again changes nothing; thanks to the 64-document alignment, no two threads ever write the same bitset word. Any other id is reported once per document, which a 4096-entry table of the last document seen at each id's low bits decides, and goes to the thread's stream after a marker naming its document.
3. Each thread counts its stream's hits per id. The counts give each thread its own part of every id's run in one flat array of ascending document ids, each 16 bits, counted from the round's first document (a round always holds fewer than 2^16), and each thread places its stream there.
4. Every query is answered on the round's documents, and its count, id sum and first ten ids are added to its totals. Then the round's postings are freed, so only an eighth of them is ever held.

A `BOOL` query is evaluated left to right. The result stays a sorted list of ids while it can: AND and NOT filter it, and an OR with another list merges into it. It moves to a bitset only to load or OR in a common term. Once the list is empty with no OR still to come, the answer is empty and the remaining steps are not read. A `PHRASE` of two terms is exactly the documents holding that adjacent pair. A longer phrase starts from the documents holding all of its pairs, which are very few, and checks each one's tokens for the whole sequence.

The binary is built for the scoring CPU (`target-cpu=native`) with profile-guided optimisation: during the offline build, a byte-identical copy of the published generator (`generator.py`, seed 2) makes a full-size training dataset, the instrumented binary runs on it, and `llvm-profdata` guides the final build. The runtime image holds only the Rust executable.

## Dependencies

None.

## With another 24 hours

Delta-coded posting lists, for less memory per round, so fewer rounds would fit the same budget.
