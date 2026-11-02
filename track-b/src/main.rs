use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::thread;

use rayon::prelude::*;
use rustc_hash::FxHashSet;

extern "C" {
    fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
    fn speed_demon_num(bytes: *const u8, len: usize) -> u32;
    fn speed_demon_xxh3_lower(bytes: *const u8, len: usize) -> u64;
}

const PR_SET_THP_DISABLE: i32 = 41;

/// n zeroed values (all-zero bytes are valid for these atomics), whose pages the kernel supplies
/// only when first touched.
fn zeroed<T>(n: usize) -> Vec<T> {
    let layout = std::alloc::Layout::array::<T>(n.max(1)).unwrap();
    unsafe { Vec::from_raw_parts(std::alloc::alloc_zeroed(layout) as *mut T, n, n.max(1)) }
}

/// The whole file, read-only, mapped with memmap2. /data is already in the page cache, so this
/// costs no memory.
fn map(path: &str) -> &'static [u8] {
    let f = File::open(path).unwrap();
    if f.metadata().unwrap().len() == 0 {
        return &[];
    }
    &Box::leak(Box::new(unsafe { memmap2::Mmap::map(&f).unwrap() }))[..]
}

/// Open addressing from a non-zero 64-bit key to a dense id, in insertion order, and the id's bitset
/// index (u32::MAX for none), behind a 128 KB filter, two bits of one word per key, that answers most
/// misses without touching the table.
struct Table {
    slots: Vec<(u64, u32, u32)>,
    filter: Vec<u64>,
    len: u32,
}

/// A key's filter word and its two bits there, from a second hash: the top 14 bits pick the word.
fn bit(key: u64) -> (usize, u64) {
    let h = key.wrapping_mul(0xff51_afd7_ed55_8ccd);
    ((h >> 50) as usize, 1 << (h & 63) | 1 << (h >> 6 & 63))
}

impl Table {
    /// A table with room for fewer than `keys` keys, so a lookup always ends at an empty slot.
    fn new(keys: usize) -> Self {
        Table { slots: vec![(0, 0, 0); keys.next_power_of_two()], filter: vec![0; 1 << 14], len: 0 }
    }
    fn slot(&self, key: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut s = (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 32) as usize & mask;
        while self.slots[s].0 != 0 && self.slots[s].0 != key {
            s = (s + 1) & mask;
        }
        s
    }
    /// Starts loading the filter word and the home slot a lookup of key will read.
    fn prefetch(&self, key: u64) {
        use std::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
        let s = (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 32) as usize & (self.slots.len() - 1);
        unsafe {
            _mm_prefetch(self.filter.as_ptr().add(bit(key).0) as *const i8, _MM_HINT_T0);
            _mm_prefetch(self.slots.as_ptr().add(s) as *const i8, _MM_HINT_T0);
        }
    }
    fn get(&self, key: u64) -> Option<(u32, u32)> {
        let (w, b) = bit(key);
        if self.filter[w] & b != b {
            return None;
        }
        let (k, id, dense) = self.slots[self.slot(key)];
        (k != 0).then_some((id, dense))
    }
    fn add(&mut self, key: u64) -> u32 {
        let (s, (w, b)) = (self.slot(key), bit(key));
        self.filter[w] |= b;
        if self.slots[s].0 == 0 {
            (self.slots[s], self.len) = ((key, self.len, u32::MAX), self.len + 1);
        }
        self.slots[s].1
    }
    /// The same map in a table about two thirds full at most, the smallest a lookup stays short in.
    fn compact(&self) -> Table {
        let mut t = Table::new(self.len as usize * 3 / 2 + 1);
        t.filter.clone_from(&self.filter);
        for &s in self.slots.iter().filter(|s| s.0 != 0) {
            let i = t.slot(s.0);
            t.slots[i] = s;
        }
        t.len = self.len;
        t
    }
}

const AHEAD: usize = 16; // tokens between a lookup's prefetch and the lookup
const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

/// The 8 bytes of b from i, little-endian, with zeros past its end.
fn load(b: &[u8], i: usize) -> u64 {
    match b.get(i..i + 8) {
        Some(w) => u64::from_le_bytes(w.try_into().unwrap()),
        None => {
            let mut w = [0u8; 8];
            w[..b.len() - i].copy_from_slice(&b[i..]);
            u64::from_le_bytes(w)
        }
    }
}

/// The high bit of every byte of x that is an ASCII letter or digit. Each byte is tested on its
/// low 7 bits, where adding a constant cannot carry into the next byte.
fn alnum(x: u64) -> u64 {
    let (a, l) = (x & !HIGH, x & !HIGH | 0x20 * ONES);
    let letter = (l + (0x80 - 0x61) * ONES) & !(l + (0x7f - 0x7a) * ONES);
    let digit = (a + (0x80 - 0x30) * ONES) & !(a + (0x7f - 0x39) * ONES);
    (letter | digit) & !x & HIGH
}

/// The token at b[i]: where it ends, and a 64-bit hash of it lowercased (a letter or digit | 0x20
/// is its lowercase), which stands in for the token. A token of up to 7 bytes is one chunk, so its
/// hash is unique.
fn token(b: &[u8], mut i: usize) -> (usize, u64) {
    let start = i;
    loop {
        let x = load(b, i);
        let n = (!alnum(x) & HIGH).trailing_zeros() as usize / 8;
        i += n;
        if n < 8 {
            let hash = unsafe { speed_demon_xxh3_lower(b[start..i].as_ptr(), i - start) };
            return (i, hash);
        }
    }
}

/// The XXH3 hash of the lowercased len-byte token at b[i].
fn hash(b: &[u8], i: usize, len: usize) -> u64 {
    unsafe { speed_demon_xxh3_lower(b[i..i + len].as_ptr(), len) }
}

/// Bitmasks of the ASCII letters and digits, and of the LFs, among the 64 bytes of b from i (zeros
/// past its end), from 32-byte compares. The build targets the scoring CPU, which has AVX2. A byte
/// of 0x80 or more is negative in a signed compare, so it is neither.
fn masks(b: &[u8], i: usize) -> (u64, u64) {
    use std::arch::x86_64::*;
    let mut tail = [0u8; 64];
    let block = b.get(i..i + 64).unwrap_or_else(|| {
        tail[..b.len() - i].copy_from_slice(&b[i..]);
        &tail
    });
    unsafe {
        let set = |c: u8| _mm256_set1_epi8(c as i8);
        let half = |k: usize| {
            let v = _mm256_loadu_si256(block.as_ptr().add(32 * k) as *const __m256i);
            let l = _mm256_or_si256(v, set(0x20));
            let letter = _mm256_and_si256(_mm256_cmpgt_epi8(l, set(b'a' - 1)), _mm256_cmpgt_epi8(set(b'z' + 1), l));
            let digit = _mm256_and_si256(_mm256_cmpgt_epi8(v, set(b'0' - 1)), _mm256_cmpgt_epi8(set(b'9' + 1), v));
            let bits = |m: __m256i| _mm256_movemask_epi8(m) as u32 as u64;
            (bits(_mm256_or_si256(letter, digit)), bits(_mm256_cmpeq_epi8(v, set(b'\n'))))
        };
        let ((a0, l0), (a1, l1)) = (half(0), half(1));
        (a0 | a1 << 32, l0 | l1 << 32)
    }
}

/// The key of the adjacent pair (a, b) of term ids.
fn pair(a: u32, b: u32) -> u64 {
    (a as u64 + 1) << 32 | (b as u64 + 1)
}

/// The spec's tokens: maximal runs of ASCII letters and digits, still in their original case.
fn tokens(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    text.split(|c| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty())
}

/// k + 1 cut points splitting b into k parts, each starting at a document whose id is a
/// multiple of 64, so no two parts ever touch the same bitset word.
fn cuts(b: &[u8], k: usize) -> Vec<usize> {
    let next = |p: usize| p + memchr::memchr(b'\n', &b[p..]).map_or(b.len() - p, |x| x + 1);
    let mut v: Vec<usize> = (0..k).map(|j| (b.len() * j / k).saturating_sub(1)).collect();
    for p in v.iter_mut() {
        *p = if *p == 0 { 0 } else { next(*p) };
        while *p < b.len() && num(&b[*p..]) % 64 != 0 {
            *p = next(*p);
        }
    }
    v.push(b.len());
    v
}

/// The decimal number the bytes start with.
fn num(b: &[u8]) -> usize {
    unsafe { speed_demon_num(b.as_ptr(), b.len()) as usize }
}

/// Runs f(j) for every j in 0..k, each on its own thread, and returns the results in order.
fn each<T: Send>(k: usize, f: impl Fn(usize) -> T + Sync + Send) -> Vec<T> {
    (0..k).into_par_iter().map(f).collect()
}

struct Index<'a> {
    docs: &'a [u8],
    terms: Table,
    pairs: Table,
}

impl Index<'_> {
    /// Calls f(doc, id, bitset index) for the query terms and phrase pairs in each document: once
    /// per document for an id without a bitset (index u32::MAX), at every occurrence for one with a
    /// bitset, since setting its bit again does nothing. Pair ids follow the term ids. Also records
    /// where each document's line ends.
    fn scan(&self, from: usize, to: usize, end: &[AtomicU32], mut f: impl FnMut(u32, u32, u32)) {
        let (b, nt) = (self.docs, self.terms.len);
        // FxHashSet keeps the per-document duplicate filter compact and makes duplicate checks
        // constant-time even when a document contains many repeated query terms.
        let mut ids = FxHashSet::default();
        let mut i = from;
        while i < to {
            let tab = i + memchr::memchr(b'\t', &b[i..]).unwrap();
            let doc = num(&b[i..]) as u32;
            ids.clear();
            let mut hit = |(id, dense): (u32, u32)| {
                if dense != u32::MAX {
                    f(doc, id, dense);
                } else if ids.insert(id) {
                    f(doc, id, dense);
                }
            };
            let mut prev = None;
            let mut word = |h: u64| {
                let cur = self.terms.get(h | 1);
                if let Some((c, dense)) = cur {
                    hit((c, dense));
                    if let Some((p, dense)) = prev.and_then(|p| self.pairs.get(pair(p, c))) {
                        hit((nt + p, dense));
                    }
                }
                prev = cur.map(|(c, _)| c);
            };
            // A token's lookup waits AHEAD tokens after its prefetch; the document's last ones are
            // looked up at its LF.
            let (mut ring, mut n, mut done) = ([0u64; AHEAD], 0usize, 0usize);
            let mut queue = |h: u64| {
                self.terms.prefetch(h | 1);
                if n - done == AHEAD {
                    word(ring[done % AHEAD]);
                    done += 1;
                }
                (ring[n % AHEAD], n) = (h, n + 1);
            };
            // The text 64 bytes at a time, from a byte that is not inside a token: a token starts at
            // a letter or digit after a byte that is neither, and ends at the next byte that is
            // neither. A token running past the block is read on its own, and the next block
            // starts where it ends. The LF ends the document.
            let mut j = tab + 1;
            loop {
                let (al, lf) = masks(b, j);
                let stop = lf.trailing_zeros();
                let mut starts = al & !(al << 1) & if stop == 64 { !0 } else { (1 << stop) - 1 };
                let mut next = j + 64;
                while starts != 0 {
                    let s = starts.trailing_zeros();
                    let e = s + (!al >> s).trailing_zeros();
                    if e >= 64 {
                        let h;
                        (next, h) = token(b, j + s as usize);
                        queue(h);
                        break;
                    }
                    queue(hash(b, j + s as usize, (e - s) as usize));
                    starts &= starts - 1;
                }
                if stop < 64 && next == j + 64 {
                    j += stop as usize;
                    break;
                }
                j = next;
            }
            for d in done..n {
                word(ring[d % AHEAD]);
            }
            end[doc as usize].store(j as u32, Relaxed);
            i = j + 1;
        }
    }
}

/// A posting set: a bitset over all documents when common, else ascending ids. Both count from the
/// round's first document, and a round holds fewer than 2^16 documents, so an id fits 16 bits.
#[derive(Clone, Copy)]
enum Set<'a> {
    Bits(&'a [u64]),
    Ids(&'a [u16]),
}

fn has(bits: &[u64], d: u32) -> bool {
    bits[d as usize / 64] >> (d % 64) & 1 == 1
}

/// The operators a step can hold, by the two bits it keeps them in.
const OPS: [u8; 4] = *b"=AON";

/// A step: the operator's index in OPS in bits 29 and 30, under a pair id's top bit; ids stay far
/// below 2^29.
fn step(op: u8, id: u32) -> u32 {
    (OPS.iter().position(|&o| o == op).unwrap() as u32) << 29 | id
}

/// Evaluates (operator, id) steps left to right into v, as ascending ids: '=' load, 'A' and,
/// 'O' or, 'N' and-not, with set(id) an id's documents. The result is held as the bitset r instead
/// only while loading or ORing in a common term has made it large; an AND with ids makes it a list
/// again. An empty list with no OR still to come is the answer, and the other steps are not read.
fn eval<'a>(steps: &[u32], set: impl Fn(u32) -> Set<'a>, r: &mut [u64], v: &mut Vec<u32>) -> bool {
    let mut dense = false;
    for (i, &s) in steps.iter().enumerate() {
        let (op, id) = (OPS[(s >> 29 & 3) as usize], s & !(3 << 29));
        if i > 0 && !dense && v.is_empty() && steps[i..].iter().all(|&s| OPS[(s >> 29 & 3) as usize] != b'O') {
            break;
        }
        match (op, set(id), dense) {
            (b'=', Set::Ids(a), _) => (v.clear(), v.extend(a.iter().map(|&d| d as u32)), dense = false).0,
            (b'=', Set::Bits(b), _) => (r.copy_from_slice(b), dense = true).0,
            (_, Set::Bits(b), false) if op != b'O' => v.retain(|&d| has(b, d) == (op == b'A')),
            (_, Set::Ids(a), false) if op != b'O' => {
                // Gallop: steps of 1, 2, 4, ... past the ids below d, then a binary search in the last step.
                let mut i = 0;
                v.retain(|&d| {
                    let mut step = 1;
                    while i + step < a.len() && (a[i + step] as u32) < d {
                        i += step;
                        step *= 2;
                    }
                    i += a[i..(i + step + 1).min(a.len())].partition_point(|&x| (x as u32) < d);
                    (a.get(i).map(|&x| x as u32) == Some(d)) == (op == b'A')
                });
            }
            // Both postings are sorted; sorting the reused query buffer is faster here than
            // maintaining a second merge buffer on the scoring workload.
            (_, Set::Ids(a), false) => (v.extend(a.iter().map(|&d| d as u32)), v.sort(), v.dedup()).0,
            (_, Set::Bits(b), false) => {
                r.copy_from_slice(b);
                v.iter().for_each(|&d| r[d as usize / 64] |= 1 << (d % 64));
                dense = true;
            }
            (b'A', Set::Ids(a), true) => (v.clear(), v.extend(a.iter().map(|&d| d as u32).filter(|&d| has(r, d))), dense = false).0,
            (b'O', Set::Ids(a), true) => a.iter().for_each(|&d| r[d as usize / 64] |= 1 << (d % 64)),
            (_, Set::Ids(a), true) => a.iter().for_each(|&d| r[d as usize / 64] &= !(1 << (d % 64))),
            (_, Set::Bits(b), true) => r.iter_mut().zip(b).for_each(|(x, &y)| *x = match op {
                b'A' => *x & y,
                b'O' => *x | y,
                _ => *x & !y,
            }),
        }
    }
    dense
}

/// The ids in the bitset r, ascending, into v.
fn list(r: &[u64], v: &mut Vec<u32>) {
    v.clear();
    for (wi, &w) in r.iter().enumerate() {
        let mut x = w;
        while x != 0 {
            v.push((wi * 64) as u32 + x.trailing_zeros());
            x &= x - 1;
        }
    }
}

/// The sum of the positions of x's set bits, using one parallel mask per bit position.
fn positions(x: u64) -> u64 {
    const MASKS: [u64; 6] = [
        0xaaaa_aaaa_aaaa_aaaa,
        0xcccc_cccc_cccc_cccc,
        0xf0f0_f0f0_f0f0_f0f0,
        0xff00_ff00_ff00_ff00,
        0xffff_0000_ffff_0000,
        0xffff_ffff_0000_0000,
    ];
    MASKS
        .iter()
        .enumerate()
        .map(|(b, &m)| ((x & m).count_ones() as u64) << b)
        .sum()
}

/// The documents are indexed and queried in this many rounds, so only one round's postings are held.
const ROUNDS: usize = 8;

fn main() {
    // No transparent huge pages: one would back a mostly untouched stretch of a lazily filled array,
    // and a freed buffer's huge pages stay counted until they are split.
    unsafe { prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0) };
    let (docs, q) = (map("/data/docs.txt"), map("/data/queries.txt"));

    // Every query becomes (operator, id) steps, all in one list: a BOOL's terms as written, a PHRASE's
    // adjacent pairs ANDed (a pair id has the top bit set). A query is its range of steps and, for a
    // PHRASE, where its words lie in queries.txt, to check longer ones.
    let lines = || q.split(|&c| c == b'\n').filter(|l| !l.is_empty());
    let spaces = |l: &[u8]| l.iter().filter(|&&c| c == b' ').count() + 1;
    let phrases = lines().filter(|l| l.starts_with(b"PHRASE")).map(spaces).sum();
    let (mut terms, mut pairs) = (Table::new(spaces(q)), Table::new(phrases));
    let (mut steps, mut queries) = (Vec::with_capacity(spaces(q)), Vec::new());
    for line in lines() {
        let phrase = line.starts_with(b"PHRASE");
        let rest = if phrase { &line[8..line.len() - 1] } else { &line[5..] };
        let (from, mut op, mut prev) = (steps.len() as u32, b'=', None);
        for (j, w) in rest.split(|&c| c == b' ').enumerate() {
            if phrase {
                let c = terms.add(token(w, 0).1 | 1);
                if let Some(p) = prev {
                    (_, op) = (steps.push(step(op, pairs.add(pair(p, c)) | 1 << 31)), b'A');
                }
                prev = Some(c);
            } else if j % 2 == 1 {
                op = w[0];
            } else {
                steps.push(step(op, terms.add(token(w, 0).1 | 1)));
            }
        }
        let at = if phrase { (rest.as_ptr() as usize - q.as_ptr() as usize) as u32 } else { 0 };
        queries.push([from, steps.len() as u32, at, at + phrase as u32 * rest.len() as u32]);
    }

    let mut index = Index { docs, terms: terms.compact(), pairs: pairs.compact() };
    drop((terms, pairs));
    let ids = (index.terms.len + index.pairs.len) as usize;
    let n = docs.rsplit(|&c| c == b'\n').nth(1).map_or(0, |last| num(last) + 1); // the last id + 1
    let end: Vec<AtomicU32> = zeroed(n);
    let k = thread::available_parallelism().map_or(1, |x| x.get());

    // The documents go in rounds of k blocks, one block per thread. A round's postings are built,
    // queried and freed before the next round starts. There are at least ROUNDS rounds, and more if
    // a round would hold 2^16 documents, so a thread's place in a round's run fits 16 bits.
    let first = |blocks: &[usize], b: usize| if blocks[b] < docs.len() { num(&docs[blocks[b]..]) } else { n };
    let mut rounds = ROUNDS;
    let blocks = loop {
        let blocks = cuts(docs, k * rounds);
        if (0..rounds).all(|r| first(&blocks, r * k + k) - first(&blocks, r * k) < 1 << 16) {
            break blocks;
        }
        rounds *= 2;
    };

    // A sample, the first thirty-second of every block, estimates which ids are common. Those get a
    // bitset over the round's documents, which the full pass sets directly.
    let sample = each(k, |j| {
        let mut count = vec![0u32; ids];
        for b in (j..k * rounds).step_by(k) {
            index.scan(blocks[b], blocks[b] + (blocks[b + 1] - blocks[b]) / 32, &end, |_, id, _| count[id as usize] += 1);
        }
        count
    });
    let mut nbits = 0;
    let bit_of: Vec<u32> = (0..ids)
        .map(|id| if sample.iter().map(|s| s[id] as usize).sum::<usize>() * 32 * 32 >= n { (nbits, nbits += 1).0 } else { u32::MAX })
        .collect();
    let rare: Vec<usize> = sample.iter().map(|s| (0..ids).filter(|&id| bit_of[id] == u32::MAX).map(|id| s[id] as usize).sum()).collect();
    drop(sample);
    let nt = index.terms.len;
    for (t, base) in [(&mut index.terms, 0), (&mut index.pairs, nt)] {
        for s in t.slots.iter_mut().filter(|s| s.0 != 0) {
            s.2 = bit_of[(base + s.1) as usize];
        }
    }

    // Every query's count, id sum and first ten ids, built up round by round.
    let mut acc = vec![(0u64, 0u64, [0u32; 10]); queries.len()];
    let chunk = queries.len().div_ceil(k).max(1);
    for round in 0..rounds {
        let (d0, d1) = (first(&blocks, round * k), first(&blocks, round * k + k));
        let words = (d1 - d0).div_ceil(64);
        let bits: Vec<AtomicU64> = zeroed(nbits as usize * words);

        // The full pass: a common id sets its bit; every other id goes to the thread's stream, after
        // a marker (top bit set) naming the document, counted from the round's first. Then the
        // thread counts its stream's hits per id.
        let (streams, mut counts): (Vec<Vec<u32>>, Vec<Vec<u16>>) = each(k, |j| {
            let b = round * k + j;
            let (mut stream, mut last) = (Vec::with_capacity(rare[j] * 33 / rounds + first(&blocks, b + 1) - first(&blocks, b)), u32::MAX);
            index.scan(blocks[b], blocks[b + 1], &end, |doc, id, dense| {
                let doc = doc - d0 as u32;
                if dense != u32::MAX {
                    let w = &bits[dense as usize * words + doc as usize / 64];
                    w.store(w.load(Relaxed) | 1 << (doc % 64), Relaxed);
                } else {
                    if doc != last {
                        (last, _) = (doc, stream.push(1 << 31 | doc));
                    }
                    stream.push(id);
                }
            });
            let mut count = vec![0u16; ids];
            stream.iter().filter(|&&v| v >> 31 == 0).for_each(|&v| count[v as usize] += 1);
            (stream, count)
        })
        .into_iter()
        .unzip();

        // The other ids get a run of one flat array, and each thread places its stream into its own
        // part of every run: its count of an id becomes its place in the id's run.
        let mut start = vec![0u32; ids + 1];
        for id in 0..ids {
            let mut at = 0;
            for c in counts.iter_mut() {
                (c[id], at) = (at as u16, at + c[id] as u32);
            }
            start[id + 1] = start[id] + at;
        }
        let flat: Vec<AtomicU16> = zeroed(start[ids] as usize);
        thread::scope(|s| {
            for (stream, mut cur) in streams.into_iter().zip(counts) {
                let (flat, start) = (&flat, &start);
                s.spawn(move || {
                    let mut doc = 0;
                    for v in stream {
                        if v >> 31 == 1 {
                            doc = v & !(1 << 31);
                        } else {
                            let v = v as usize;
                            flat[(start[v] + cur[v] as u32) as usize].store(doc as u16, Relaxed);
                            cur[v] += 1;
                        }
                    }
                });
            }
        });
        let bits: Vec<u64> = bits.into_iter().map(AtomicU64::into_inner).collect();
        let flat: Vec<u16> = flat.into_iter().map(AtomicU16::into_inner).collect();
        let set = |id: u32| {
            let id = if id >> 31 == 1 { (nt + (id & !(1 << 31))) as usize } else { id as usize };
            match bit_of[id] {
                u32::MAX => Set::Ids(&flat[start[id] as usize..start[id + 1] as usize]),
                b => Set::Bits(&bits[b as usize * words..][..words]),
            }
        };

        // Answer the queries on the round's documents in k contiguous blocks of queries.
        thread::scope(|s| {
            for (block, acc) in queries.chunks(chunk).zip(acc.chunks_mut(chunk)) {
                let (set, end, steps) = (&set, &end, &steps);
                s.spawn(move || {
                    let (mut r, mut v, mut phrase, mut toks) = (vec![0u64; words], Vec::new(), Vec::new(), Vec::new());
                    for (&[from, to, at, stop], (count, sum, ten)) in block.iter().zip(acc) {
                        let dense = eval(&steps[from as usize..to as usize], set, &mut r, &mut v);
                        let long = to - from > 1 && stop > at;
                        if dense && !long {
                            // Count, sum of ids, and the first ids directly from the bitset.
                            for (wi, &w) in r.iter().enumerate().filter(|&(_, &w)| w != 0) {
                                let mut x = w;
                                while *count < 10 && x != 0 {
                                    ten[*count as usize] = (d0 + wi * 64) as u32 + x.trailing_zeros();
                                    (*count, x) = (*count + 1, x & (x - 1));
                                }
                                *count += x.count_ones() as u64;
                                *sum += (d0 + wi * 64) as u64 * w.count_ones() as u64 + positions(w);
                            }
                            continue;
                        }
                        if dense {
                            list(&r, &mut v);
                        }
                        if long {
                            // Every adjacent pair occurs; check the words occur together, in order.
                            phrase.clear();
                            phrase.extend(q[at as usize..stop as usize].split(|&c| c == b' '));
                            v.retain(|&d| {
                                let d = d0 + d as usize;
                                let line = if d == 0 { 0 } else { end[d - 1].load(Relaxed) as usize + 1 };
                                let tab = line + memchr::memchr(b'\t', &docs[line..]).unwrap();
                                toks.clear();
                                toks.extend(tokens(&docs[tab + 1..end[d].load(Relaxed) as usize]));
                                toks.windows(phrase.len()).any(|w| w.iter().zip(&phrase).all(|(a, b)| a.eq_ignore_ascii_case(b)))
                            });
                        }
                        let have = (*count).min(10) as usize;
                        ten[have..].iter_mut().zip(&v).for_each(|(t, &d)| *t = d0 as u32 + d);
                        *count += v.len() as u64;
                        *sum += v.iter().map(|&d| (d0 + d as usize) as u64).sum::<u64>();
                    }
                });
            }
        });
    }

    // Write the answers in k contiguous blocks, each into its own buffer.
    let outs: Vec<Vec<u8>> = thread::scope(|s| {
        let hs: Vec<_> = acc
            .chunks(chunk)
            .map(|block| {
                s.spawn(move || {
                    let mut out = Vec::new();
                    for (count, sum, ten) in block {
                        write!(out, "{count} {sum}").unwrap();
                        ten[..(*count).min(10) as usize].iter().for_each(|d| write!(out, " {d}").unwrap());
                        writeln!(out).unwrap();
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    std::fs::write("/out/results.txt", outs.concat()).unwrap();
}
