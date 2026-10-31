# Track B: Index

The task, the input files and the exact output format are in the [spec](https://github.com/hackathon-raptors/speed-demon-2026/blob/main/specs/b-index.md).

## Approach

Rust, standard library only. Both files are memory-mapped; `/data` is already in the page cache, so reading costs no copy and no memory.

Only what the queries ask about is indexed. The queries are parsed first. Every distinct term gets an id, and so does every adjacent pair of terms in a phrase. A token is represented by a 64-bit hash of its lowercased bytes, computed in the same loop that finds the token's end (one 256-entry table both classifies and lowercases each byte), and looked up in a small open-addressing table behind a 64 KB filter, one bit per hash, that answers most misses without touching the table.

The corpus is split into one part per hardware thread, each starting at a document whose id is a multiple of 64, and tokenized once:

1. A sample, the first thirty-second of every part, estimates how many documents contain each id. An id expected in at least one document in 32 gets a bitset over all documents, which is smaller than a list at that density.
2. The full pass sets common ids' bits directly. Thanks to the 64-document alignment, no two threads ever write the same bitset word, so the writes need no atomic read-modify-write. Every other hit is appended to the thread's stream: a 4-byte document marker, then the ids of that document's hits. Streams are kept in chunks, so each chunk is freed as soon as it is placed. Each hit also adds the size of its posting to its id's byte count.
3. Rare ids' postings are stored compactly. A posting is a 2-byte gap from the id's previous document in the part, or, for the first posting of an id in a part and for gaps of 65,535 or more, the marker 0xFFFF followed by the document (6 bytes). Each id's postings form one region of a shared buffer: its parts' segments, in part order. A part's first posting carries its document in full, so a region decodes in one pass with no part boundaries, and a list is read from one contiguous place.

A `BOOL` query is evaluated left to right. A rare term's list is decoded into a scratch buffer for its step. The result stays a sorted list of ids while it can: AND and NOT filter it, and an OR with another list merges into it. It moves to a bitset only to load or OR in a common term. A `PHRASE` of two terms is exactly the documents holding that adjacent pair. A longer phrase starts from the documents holding all of its pairs, which are very few, and checks each one's tokens for the whole sequence.

The binary is built for the scoring CPU (`target-cpu=native`).

## Dependencies

None.

Measured on the full corpus on this machine (against the previous version): peak memory 176 MB → 146 MB; time about the same. Tried and rejected: variable-width (varint) gaps (smaller, but decoding was 12% slower); an 8-byte token classifier (12% slower than the byte table); a single shared lookup over all parts (no gain).

## With another 24 hours

Lists decoded once per id for queries that share terms, and the densest bitsets moved into the compact form if the query phase allows it.
