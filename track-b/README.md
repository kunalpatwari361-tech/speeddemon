# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

## Approach

Rust, standard library only. Both files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

Only what the queries ask about is indexed. The queries are parsed first. Every distinct term gets an id, and so does every adjacent pair of terms in a phrase. A token is represented by a 64-bit hash of its lowercased bytes, computed in the same loop that finds the token's end (one 256-entry table both classifies and lowercases each byte), and looked up in a small open-addressing table behind a 64 KB filter, one bit per hash, that answers most misses without touching the table.

The corpus is split into one part per hardware thread, each starting at a document whose id is a multiple of 64, and tokenized once:

1. A sample, the first thirty-second of every part, estimates how many documents contain each id. An id expected in at least one document in 32 gets a bitset over all documents, which is smaller than a list at that density.
2. The full pass sets common ids' bits directly. Thanks to the 64-document alignment, no two threads ever write the same bitset word, so the writes need no atomic read-modify-write. Every other hit is appended to the thread's stream as a 4-byte id, after a marker naming its document.
3. Each thread counts its stream's hits per id. The counts give each thread its own part of every id's run in one flat array of ascending document ids, and each thread places its stream there.

A `BOOL` query is evaluated left to right. The result stays a sorted list of ids while it can: AND and NOT filter it, and an OR with another list merges into it. It moves to a bitset only to load or OR in a common term. A `PHRASE` of two terms is exactly the documents holding that adjacent pair. A longer phrase starts from the documents holding all of its pairs, which are very few, and checks each one's tokens for the whole sequence.

The build is profile-guided: an instrumented binary runs on sample data made by the published generator (`generate.py`, an unmodified copy), and the release binary is rebuilt from that profile for the scoring CPU.

## Dependencies

None.

## With another 24 hours

Smaller postings, with delta-coded lists, and faster tokenizing by scanning a word at a time.
