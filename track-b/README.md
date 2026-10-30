# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

## Approach

Rust, standard library only. Both files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

Only what the queries ask about is indexed. The queries are parsed first. Every distinct term gets an id, and so does every adjacent pair of terms in a phrase. A token is represented by a 64-bit hash of its lowercased bytes, computed in the same loop that finds the token's end, and looked up in a small open-addressing table.

The corpus is split into one part per hardware thread, each starting at a document whose id is a multiple of 64. Two passes run over it:

1. Count, per thread, how many documents contain each term and each phrase pair.
2. Fill the postings. An id found in at least one document in 32 gets a bitset over all documents, which is smaller than a list at that density. The rest get a run of a single flat array of ascending document ids, and each thread writes its own part of every run. Thanks to the 64-document alignment, no two threads ever write the same bitset word, so the writes need no atomic read-modify-write.

A `BOOL` query is evaluated left to right on a bitset result, combining it with each term's bitset or id list. A `PHRASE` of two terms is exactly the documents holding that adjacent pair. A longer phrase starts from the documents holding all of its pairs, which are very few, and checks each one's tokens for the whole sequence.

## Dependencies

None.

## With another 24 hours

A single pass that keeps each thread's hits, and faster tokenizing by scanning a word at a time.
