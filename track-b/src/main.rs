use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::Relaxed};
use std::thread;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

/// The whole file, read-only. /data is already in the page cache, so this costs no memory.
fn map(path: &str) -> &'static [u8] {
    let f = File::open(path).unwrap();
    let len = f.metadata().unwrap().len() as usize;
    if len == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), len, 1, 2, f.as_raw_fd(), 0), len) }
}

/// n zeroed values (all-zero bytes are valid for these atomics), whose pages the kernel supplies
/// only when first touched.
fn zeroed<T>(n: usize) -> Vec<T> {
    let layout = std::alloc::Layout::array::<T>(n.max(1)).unwrap();
    unsafe { Vec::from_raw_parts(std::alloc::alloc_zeroed(layout) as *mut T, n, n.max(1)) }
}

/// Runs f(j) for j in 0..n, each on its own thread, and returns the results in order.
fn par<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    thread::scope(|s| {
        let f = &f;
        let hs: Vec<_> = (0..n).map(|j| s.spawn(move || f(j))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// Open addressing from a non-zero 64-bit key to a dense id, in insertion order, behind a 64 KB
/// one-bit-per-hash filter that answers most misses without touching the table.
struct Table {
    slots: Vec<(u64, u32)>,
    filter: Vec<u64>,
    len: u32,
}

/// The filter bit of a key: 19 bits of a second hash.
fn bit(key: u64) -> (usize, u64) {
    let h = (key.wrapping_mul(0xff51_afd7_ed55_8ccd) >> 45) as usize;
    (h / 64, 1 << (h % 64))
}

impl Table {
    fn new(cap: usize) -> Self {
        Table { slots: vec![(0, 0); (cap * 2).next_power_of_two()], filter: vec![0; 1 << 13], len: 0 }
    }
    fn slot(&self, key: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut s = (key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 32) as usize & mask;
        while self.slots[s].0 != 0 && self.slots[s].0 != key {
            s = (s + 1) & mask;
        }
        s
    }
    fn get(&self, key: u64) -> Option<u32> {
        let (w, b) = bit(key);
        if self.filter[w] & b == 0 {
            return None;
        }
        let (k, id) = self.slots[self.slot(key)];
        (k != 0).then_some(id)
    }
    fn add(&mut self, key: u64) -> u32 {
        let (s, (w, b)) = (self.slot(key), bit(key));
        self.filter[w] |= b;
        if self.slots[s].0 == 0 {
            (self.slots[s], self.len) = ((key, self.len), self.len + 1);
        }
        self.slots[s].1
    }
    /// The same map in the smallest table that holds it, so lookups stay in cache. The old table
    /// is freed.
    fn compact(self) -> Table {
        let mut t = Table::new(self.len as usize);
        t.filter.clone_from(&self.filter);
        for &(k, id) in self.slots.iter().filter(|s| s.0 != 0) {
            let s = t.slot(k);
            t.slots[s] = (k, id);
        }
        t.len = self.len;
        t
    }
}

/// A token byte's lowercase, or 0 for a separator (for a letter or digit, c | 0x20 is its lowercase).
const LOWER: [u8; 256] = {
    let (mut t, mut c) = ([0u8; 256], 0);
    while c < 256 {
        if (c as u8).is_ascii_alphanumeric() {
            t[c] = c as u8 | 0x20;
        }
        c += 1;
    }
    t
};

/// A 64-bit hash of a lowercased token stands in for the token itself.
fn step(h: u64, c: u8) -> u64 {
    (h ^ LOWER[c as usize] as u64).wrapping_mul(0x100_0000_01b3)
}
const SEED: u64 = 0xcbf2_9ce4_8422_2325;

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
    let next = |p: usize| p + b[p..].iter().position(|&c| c == b'\n').map_or(b.len() - p, |x| x + 1);
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
    b.iter().take_while(|c| c.is_ascii_digit()).fold(0, |v, &c| v * 10 + (c - b'0') as usize)
}

struct Index<'a> {
    docs: &'a [u8],
    terms: Table,
    pairs: Table,
}

impl Index<'_> {
    /// Calls f(doc, id, previous, value) once per document for every query term and phrase pair
    /// it contains, where previous is the id's last document + 1 in this part (0 if none). Pair
    /// ids follow the term ids. state[id] is (last document + 1, value), kept together so a hit
    /// touches one place. Also records where each document's text lies.
    fn scan(&self, from: usize, to: usize, state: &mut [(u32, u32)], span: &[AtomicU64], mut f: impl FnMut(u32, usize, u32, &mut u32)) {
        let (b, nt) = (self.docs, self.terms.len as usize);
        let mut i = from;
        while i < to {
            let tab = i + b[i..].iter().position(|&c| c == b'\t').unwrap();
            let end = tab + b[tab..].iter().position(|&c| c == b'\n').unwrap();
            let doc = num(&b[i..]) as u32;
            span[doc as usize].store((tab as u64 + 1) << 32 | end as u64, Relaxed);
            let mut hit = |id: usize| {
                let e = &mut state[id];
                if e.0 != doc + 1 {
                    let prev = e.0;
                    e.0 = doc + 1;
                    f(doc, id, prev, &mut e.1);
                }
            };
            let (mut j, mut prev) = (tab + 1, None);
            while j < end {
                if LOWER[b[j] as usize] == 0 {
                    j += 1;
                    continue;
                }
                let mut h = SEED;
                while LOWER[b[j] as usize] != 0 {
                    h = step(h, b[j]);
                    j += 1;
                }
                let cur = self.terms.get(h | 1);
                if let Some(c) = cur {
                    hit(c as usize);
                    if let Some(p) = prev.and_then(|p| self.pairs.get(pair(p, c))) {
                        hit(nt + p as usize);
                    }
                }
                prev = cur;
            }
            i = end + 1;
        }
    }
}

/// One part's hits in the full pass, as u32s in document order: a document marker (top bit set),
/// then the ids of that document's hits. Kept in chunks, so placement frees each one as it goes.
#[derive(Default)]
struct Stream(Vec<Vec<u32>>);

const CHUNK: usize = 1 << 16;

/// Flags a state value as a dense id's bitset index (rare ids hold their byte count instead).
const DENSE: u32 = 1 << 31;

impl Stream {
    fn push(&mut self, v: u32) {
        match self.0.last_mut() {
            Some(c) if c.len() < CHUNK => c.push(v),
            _ => {
                let mut c = Vec::with_capacity(CHUNK);
                c.push(v);
                self.0.push(c);
            }
        }
    }
}

/// The bytes a posting takes, given prev, the previous document + 1 of its id in the same part (0
/// for the first in the part): a 2-byte gap, or else the marker 0xFFFF and the document (6 bytes).
fn gap_len(prev: u32, doc: u32) -> u32 {
    if prev != 0 && doc + 1 - prev < 0xFFFF {
        2
    } else {
        6
    }
}

/// Writes bytes at buf[p..].
fn store(buf: &[AtomicU8], p: usize, bytes: &[u8]) {
    for (i, &b) in bytes.iter().enumerate() {
        buf[p + i].store(b, Relaxed);
    }
}

/// Fills out with the documents of a rare id from its region of the postings buffer, ascending.
fn decode(postings: &[u8], (mut p, end): (usize, usize), out: &mut Vec<u32>) {
    out.clear();
    let mut doc = 0;
    while p < end {
        let gap = u16::from_le_bytes([postings[p], postings[p + 1]]);
        if gap == 0xFFFF {
            doc = u32::from_le_bytes(postings[p + 2..p + 6].try_into().unwrap());
            p += 6;
        } else {
            doc += gap as u32;
            p += 2;
        }
        out.push(doc);
    }
}

/// A posting set: a bitset over all documents when common, else ascending ids.
#[derive(Clone, Copy)]
enum Set<'a> {
    Bits(&'a [u64]),
    Ids(&'a [u32]),
}

fn has(bits: &[u64], d: u32) -> bool {
    bits[d as usize / 64] >> (d % 64) & 1 == 1
}

/// Evaluates (operator, set) steps left to right into v, as ascending ids: '=' load, 'A' and,
/// 'O' or, 'N' and-not. The result is held as the bitset r instead only while loading or ORing
/// in a common term has made it large; an AND with ids makes it a list again.
fn eval<'a>(steps: impl Iterator<Item = (u8, Set<'a>)>, r: &mut [u64], v: &mut Vec<u32>) {
    let mut dense = false;
    for (op, s) in steps {
        match (op, s, dense) {
            (b'=', Set::Ids(a), _) => (v.clear(), v.extend_from_slice(a), dense = false).0,
            (b'=', Set::Bits(b), _) => (r.copy_from_slice(b), dense = true).0,
            (_, Set::Bits(b), false) if op != b'O' => v.retain(|&d| has(b, d) == (op == b'A')),
            (_, Set::Ids(a), false) if op != b'O' => {
                let mut i = 0;
                v.retain(|&d| {
                    i += a[i..].iter().take_while(|&&x| x < d).count();
                    (a.get(i) == Some(&d)) == (op == b'A')
                });
            }
            (_, Set::Ids(a), false) => (v.extend_from_slice(a), v.sort_unstable(), v.dedup()).0,
            (_, Set::Bits(b), false) => {
                r.copy_from_slice(b);
                v.iter().for_each(|&d| r[d as usize / 64] |= 1 << (d % 64));
                dense = true;
            }
            (b'A', Set::Ids(a), true) => (v.clear(), v.extend(a.iter().filter(|&&d| has(r, d))), dense = false).0,
            (b'O', Set::Ids(a), true) => a.iter().for_each(|&d| r[d as usize / 64] |= 1 << (d % 64)),
            (_, Set::Ids(a), true) => a.iter().for_each(|&d| r[d as usize / 64] &= !(1 << (d % 64))),
            (_, Set::Bits(b), true) => r.iter_mut().zip(b).for_each(|(x, &y)| {
                *x = match op {
                    b'A' => *x & y,
                    b'O' => *x | y,
                    _ => *x & !y,
                }
            }),
        }
    }
    if dense {
        v.clear();
        for (wi, &w) in r.iter().enumerate() {
            let mut x = w;
            while x != 0 {
                v.push((wi * 64) as u32 + x.trailing_zeros());
                x &= x - 1;
            }
        }
    }
}

fn main() {
    let (docs, q) = (map("/data/docs.txt"), map("/data/queries.txt"));

    // Every query becomes (operator, id) steps: a BOOL's terms as written, a PHRASE's adjacent pairs
    // ANDed (a pair id has the top bit set). A PHRASE also keeps its words, to check longer ones.
    let words_in_queries = q.iter().filter(|&&c| c == b' ').count();
    let (mut terms, mut pairs) = (Table::new(words_in_queries), Table::new(words_in_queries));
    let mut queries = Vec::new();
    for line in q.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let phrase = line.starts_with(b"PHRASE");
        let rest = if phrase { &line[8..line.len() - 1] } else { &line[5..] };
        let words: Vec<&[u8]> = rest.split(|&c| c == b' ').collect();
        let mut id = |w: &[u8]| terms.add(w.iter().fold(SEED, |h, &c| step(h, c)) | 1);
        let steps: Vec<(u8, u32)> = if phrase {
            let ids: Vec<u32> = words.iter().map(|w| id(w)).collect();
            let pair_ids = ids.windows(2).map(|p| pairs.add(pair(p[0], p[1])) | 1 << 31);
            pair_ids.enumerate().map(|(j, p)| (if j == 0 { b'=' } else { b'A' }, p)).collect()
        } else {
            let ops = std::iter::once(b'=').chain(words.iter().skip(1).step_by(2).map(|o| o[0]));
            ops.zip(words.iter().step_by(2)).map(|(op, w)| (op, id(w))).collect()
        };
        queries.push((steps, if phrase { words } else { Vec::new() }));
    }

    let index = Index { docs, terms: terms.compact(), pairs: pairs.compact() };
    let ids = (index.terms.len + index.pairs.len) as usize;
    let n = docs.rsplit(|&c| c == b'\n').nth(1).map_or(0, |last| num(last) + 1); // the last id + 1
    let words = n.div_ceil(64);
    let span: Vec<AtomicU64> = zeroed(n);
    let k = thread::available_parallelism().map_or(1, |x| x.get());
    let parts = cuts(docs, k);
    let nparts = parts.len() - 1;

    // A sample, the first thirty-second of every part, estimates which ids are common (in at least
    // one document in 32). Those get a bitset over all documents, which the full pass sets directly.
    let sample = par(nparts, |j| {
        let mut state = vec![(0u32, 0u32); ids];
        index.scan(parts[j], parts[j] + (parts[j + 1] - parts[j]) / 32, &mut state, &span, |_, _, _, c| *c += 1);
        state.iter().map(|s| s.1).collect::<Vec<u32>>()
    });
    let mut nbits = 0;
    let bit_of: Vec<Option<usize>> = (0..ids)
        .map(|id| (sample.iter().map(|s| s[id] as usize).sum::<usize>() * 32 * 32 >= n).then(|| (nbits, nbits += 1).0))
        .collect();
    drop(sample);
    let bits: Vec<AtomicU64> = zeroed(nbits * words);

    // The full pass: a common id sets its bit; every other hit goes to the part's stream, and the
    // size of its gap is added to the id's byte count. A state value is the dense bitset
    // index (flagged) for a common id, else the byte count its postings will take.
    let streams: Vec<(Stream, Vec<(u32, u32)>)> = par(nparts, |j| {
        let mut state: Vec<(u32, u32)> = bit_of
            .iter()
            .map(|b| (0, match *b {
                Some(b) => DENSE | b as u32,
                None => 0,
            }))
            .collect();
        let (mut stream, mut last) = (Stream::default(), u32::MAX);
        index.scan(parts[j], parts[j + 1], &mut state, &span, |doc, id, prev, value| {
            if *value & DENSE != 0 {
                let w = &bits[(*value & !DENSE) as usize * words + doc as usize / 64];
                w.store(w.load(Relaxed) | 1 << (doc % 64), Relaxed);
            } else {
                // The gap from the id's previous posting in this part (from 0 for the first).
                *value += gap_len(prev, doc);
                if doc != last {
                    last = doc;
                    stream.push(1 << 31 | doc);
                }
                stream.push(id as u32);
            }
        });
        (stream, state.iter().map(|s| (if s.1 & DENSE != 0 { 0 } else { s.1 }, 0)).collect())
    });

    // Each id's postings are one region of a shared buffer: its parts' segments, in part order.
    // Every part first gets, for each id, where its own segment starts inside that region.
    // Each id's postings are one region of a shared buffer: its parts' segments, in part order.
    // Every part gets, for each id, where its own segment starts inside that region (its cursor),
    // and the document + 1 of its last posting for the id (0 if none).
    let mut totals = vec![0u32; ids];
    for (_, sizes) in &streams {
        for (t, &(size, _)) in totals.iter_mut().zip(sizes) {
            *t += size;
        }
    }
    let mut starts = vec![0u32; ids + 1];
    for x in 0..ids {
        starts[x + 1] = starts[x] + totals[x];
    }
    drop(totals);
    let mut next = starts[..ids].to_vec();
    let mut cursored: Vec<(Stream, Vec<(u32, u32)>)> = streams
        .into_iter()
        .map(|(stream, mut cursor)| {
            for (c, n) in cursor.iter_mut().zip(next.iter_mut()) {
                let size = c.0;
                *c = (*n, 0);
                *n += size;
            }
            (stream, cursor)
        })
        .collect();
    drop(next);
    let postings: Vec<AtomicU8> = zeroed(starts[ids] as usize);
    let buf: &[AtomicU8] = &postings;
    thread::scope(|s| {
        for (stream, mut cursor) in cursored.drain(..) {
            s.spawn(move || {
                let mut doc = 0;
                for v in stream.0.into_iter().flatten() {
                    if v >> 31 == 1 {
                        doc = v & !(1 << 31);
                    } else {
                        let x = v as usize;
                        let (p, prev) = (cursor[x].0 as usize, cursor[x].1);
                        let q = if prev != 0 && doc + 1 - prev < 0xFFFF {
                            store(buf, p, &((doc + 1 - prev) as u16).to_le_bytes());
                            p + 2
                        } else {
                            store(buf, p, &0xFFFFu16.to_le_bytes());
                            store(buf, p + 2, &doc.to_le_bytes());
                            p + 6
                        };
                        cursor[x] = (q as u32, doc + 1);
                    }
                }
            });
        }
    });
    let postings: Vec<u8> = postings.into_iter().map(AtomicU8::into_inner).collect();
    let bits: Vec<u64> = bits.into_iter().map(AtomicU64::into_inner).collect();

    // Answer the queries in k contiguous blocks, each into its own buffer. Rare ids are decoded
    // into a per-step scratch list; common ids are read from their bitsets.
    let nt = index.terms.len;
    let resolve = |id: u32| if id >> 31 == 1 { (nt + (id & !(1 << 31))) as usize } else { id as usize };
    let outs: Vec<Vec<u8>> = thread::scope(|s| {
        let hs: Vec<_> = queries
            .chunks(queries.len().div_ceil(k).max(1))
            .map(|block| {
                let (postings, starts, bit_of, bits, resolve, span, docs) = (&postings, &starts, &bit_of, &bits, &resolve, &span, docs);
                s.spawn(move || {
                    let (mut r, mut v, mut out, mut toks) = (vec![0u64; words], Vec::new(), Vec::new(), Vec::new());
                    let mut scratch: Vec<Vec<u32>> = Vec::new();
                    for (steps, phrase) in block {
                        if scratch.len() < steps.len() {
                            scratch.resize_with(steps.len(), Vec::new);
                        }
                        for (list, &(_, id)) in scratch.iter_mut().zip(steps) {
                            let x = resolve(id);
                            if bit_of[x].is_none() {
                                decode(postings, (starts[x] as usize, starts[x + 1] as usize), list);
                            }
                        }
                        let sets = steps.iter().zip(&scratch).map(|(&(op, id), list)| {
                            let x = resolve(id);
                            let set = match bit_of[x] {
                                Some(b) => Set::Bits(&bits[b * words..][..words]),
                                None => Set::Ids(list),
                            };
                            (op, set)
                        });
                        eval(sets, &mut r, &mut v);
                        if phrase.len() > 2 {
                            // Every adjacent pair occurs; check the words occur together, in order.
                            v.retain(|&d| {
                                let sp = span[d as usize].load(Relaxed);
                                toks.clear();
                                toks.extend(tokens(&docs[(sp >> 32) as usize..sp as u32 as usize]));
                                toks.windows(phrase.len()).any(|w| w.iter().zip(phrase).all(|(a, b)| a.eq_ignore_ascii_case(b)))
                            });
                        }
                        let sum: u64 = v.iter().map(|&d| d as u64).sum();
                        write!(out, "{} {sum}", v.len()).unwrap();
                        v.iter().take(10).for_each(|d| write!(out, " {d}").unwrap());
                        out.push(b'\n');
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    std::fs::write("/out/results.txt", outs.concat()).unwrap();
}
