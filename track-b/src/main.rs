use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
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
    /// The same map in the smallest table that holds it, so lookups stay in cache.
    fn compact(&self) -> Table {
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

/// A 64-bit hash of a lowercased token stands in for the token itself. For a letter or digit,
/// c | 0x20 is its lowercase.
fn step(h: u64, c: u8) -> u64 {
    (h ^ (c | 0x20) as u64).wrapping_mul(0x100_0000_01b3)
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
    let id = |p: usize| b[p..].iter().take_while(|c| c.is_ascii_digit()).fold(0, |v, &c| v * 10 + (c - b'0') as usize);
    let mut v: Vec<usize> = (0..k)
        .map(|j| {
            let mut p = b.len() * j / k;
            if p > 0 {
                p = next(p);
            }
            while p < b.len() && id(p) % 64 != 0 {
                p = next(p);
            }
            p
        })
        .collect();
    v.push(b.len());
    v
}

struct Index<'a> {
    docs: &'a [u8],
    terms: Table,
    pairs: Table,
}

impl Index<'_> {
    /// Calls f(doc, id, value) once per document for every query term and phrase pair it
    /// contains; pair ids follow the term ids. state[id] is (last document + 1, value), kept
    /// together so a hit touches one place. Also records where each document's text lies.
    fn scan(&self, from: usize, to: usize, state: &mut [(u32, u32)], span: &[AtomicU64], mut f: impl FnMut(u32, usize, &mut u32)) {
        let (b, nt) = (self.docs, self.terms.len as usize);
        let mut i = from;
        while i < to {
            let tab = i + b[i..].iter().position(|&c| c == b'\t').unwrap();
            let end = tab + b[tab..].iter().position(|&c| c == b'\n').unwrap();
            let doc = b[i..tab].iter().fold(0, |v, &c| v * 10 + (c - b'0') as u32);
            span[doc as usize].store((tab as u64 + 1) << 32 | end as u64, Relaxed);
            let mut hit = |id: usize| {
                let e = &mut state[id];
                if e.0 != doc + 1 {
                    e.0 = doc + 1;
                    f(doc, id, &mut e.1);
                }
            };
            let (mut j, mut prev) = (tab + 1, None);
            while j < end {
                if !b[j].is_ascii_alphanumeric() {
                    j += 1;
                    continue;
                }
                let mut h = SEED;
                while b[j].is_ascii_alphanumeric() {
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

/// A posting set: a bitset over all documents when common, else ascending ids.
enum Set<'a> {
    Bits(&'a [u64]),
    Ids(&'a [u32]),
}

/// Applies one operator to the running result r, a bitset: '=' load, 'A' and, 'O' or, 'N' and-not.
fn apply(r: &mut [u64], op: u8, s: Set, keep: &mut Vec<u32>) {
    let bit = |d: u32| (d as usize / 64, 1u64 << (d % 64));
    match (op, s) {
        (b'=', Set::Bits(b)) => r.copy_from_slice(b),
        (b'A', Set::Bits(b)) => r.iter_mut().zip(b).for_each(|(x, y)| *x &= y),
        (b'O', Set::Bits(b)) => r.iter_mut().zip(b).for_each(|(x, y)| *x |= y),
        (b'N', Set::Bits(b)) => r.iter_mut().zip(b).for_each(|(x, y)| *x &= !y),
        (b'O', Set::Ids(a)) => a.iter().for_each(|&d| r[bit(d).0] |= bit(d).1),
        (b'N', Set::Ids(a)) => a.iter().for_each(|&d| r[bit(d).0] &= !bit(d).1),
        (op, Set::Ids(a)) => {
            keep.clear();
            keep.extend(a.iter().filter(|&&d| op == b'=' || r[bit(d).0] & bit(d).1 != 0));
            r.fill(0);
            keep.iter().for_each(|&d| r[bit(d).0] |= bit(d).1);
        }
        _ => unreachable!(),
    }
}

fn main() {
    let (docs, q) = (map("/data/docs.txt"), map("/data/queries.txt"));

    // Queries: a BOOL keeps its term ids and operators, a PHRASE its words and the ids of its adjacent pairs.
    let words_in_queries = q.iter().filter(|&&c| c == b' ').count();
    let (mut terms, mut pairs) = (Table::new(words_in_queries), Table::new(words_in_queries));
    let mut queries = Vec::new();
    for line in q.split(|&c| c == b'\n').filter(|l| !l.is_empty()) {
        let phrase = line.starts_with(b"PHRASE");
        let rest = if phrase { &line[8..line.len() - 1] } else { &line[5..] };
        let words: Vec<&[u8]> = rest.split(|&c| c == b' ').collect();
        let mut id = |w: &[u8]| terms.add(w.iter().fold(SEED, |h, &c| step(h, c)) | 1);
        if phrase {
            let ids: Vec<u32> = words.iter().map(|w| id(w)).collect();
            let pair_ids: Vec<u32> = ids.windows(2).map(|p| pairs.add(pair(p[0], p[1]))).collect();
            queries.push((Vec::new(), pair_ids, words));
        } else {
            let ops: Vec<u8> = std::iter::once(b'=').chain(words.iter().skip(1).step_by(2).map(|o| o[0])).collect();
            let ids: Vec<u32> = words.iter().step_by(2).map(|w| id(w)).collect();
            queries.push((ops, ids, Vec::new()));
        }
    }

    let index = Index { docs, terms: terms.compact(), pairs: pairs.compact() };
    let ids = (index.terms.len + index.pairs.len) as usize;
    let n = docs[..docs.len().saturating_sub(1)].iter().rposition(|&c| c == b'\n').map_or(0, |p| p + 1);
    let n = if docs.is_empty() { 0 } else { docs[n..].iter().take_while(|c| c.is_ascii_digit()).fold(0, |v, &c| v * 10 + (c - b'0') as usize) + 1 };
    let words = n.div_ceil(64);
    let span: Vec<AtomicU64> = (0..n).map(|_| AtomicU64::new(0)).collect();
    let k = thread::available_parallelism().map_or(1, |x| x.get());
    let parts = cuts(docs, k);

    // Runs f(part, state) on every part in parallel, each thread with its own state.
    let each = |f: &(dyn Fn(usize, &mut [(u32, u32)]) -> Vec<u32> + Sync)| -> Vec<Vec<u32>> {
        thread::scope(|s| {
            let hs: Vec<_> = (0..parts.len() - 1).map(|j| s.spawn(move || f(j, &mut vec![(0, 0); ids]))).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        })
    };

    // A sample, the first sixteenth of every part, estimates which ids are common. Those get a
    // bitset over all documents, which the full pass sets directly.
    let sample = each(&|j, state| {
        index.scan(parts[j], parts[j] + (parts[j + 1] - parts[j]) / 16, state, &span, |_, _, c| *c += 1);
        state.iter().map(|s| s.1).collect()
    });
    let mut nbits = 0;
    let bit_of: Vec<Option<usize>> = (0..ids)
        .map(|id| (sample.iter().map(|s| s[id] as usize).sum::<usize>() * 16 * 32 >= n).then(|| (nbits, nbits += 1).0))
        .collect();
    let bits: Vec<AtomicU64> = (0..nbits * words).map(|_| AtomicU64::new(0)).collect();

    // The full pass: a common id sets its bit; every other hit goes to the thread's stream as
    // its id, after a marker (top bit set) naming the document.
    let streams = each(&|j, state| {
        let rare: usize = (0..ids).filter(|&id| bit_of[id].is_none()).map(|id| sample[j][id] as usize).sum();
        let (mut stream, mut last) = (Vec::with_capacity(rare * 17 + n / k), u32::MAX);
        index.scan(parts[j], parts[j + 1], state, &span, |doc, id, _| match bit_of[id] {
            Some(b) => {
                let w = &bits[b * words + doc as usize / 64];
                w.store(w.load(Relaxed) | 1 << (doc % 64), Relaxed);
            }
            None => {
                if doc != last {
                    (last, _) = (doc, stream.push(1 << 31 | doc));
                }
                stream.push(id as u32);
            }
        });
        stream
    });

    // The other ids get a run of one flat array: count each thread's hits per id, then each
    // thread places its stream into its own part of every run.
    let mut counts = each(&|j, _| {
        let mut count = vec![0u32; ids];
        streams[j].iter().filter(|&&v| v >> 31 == 0).for_each(|&v| count[v as usize] += 1);
        count
    });
    let df: Vec<usize> = (0..ids).map(|id| counts.iter().map(|c| c[id] as usize).sum()).collect();
    let mut nflat = 0;
    let start: Vec<usize> = df.iter().map(|&d| (nflat, nflat += d).0).collect();
    for id in 0..ids {
        let mut at = start[id] as u32;
        for c in counts.iter_mut() {
            (c[id], at) = (at, at + c[id]);
        }
    }
    let flat: Vec<AtomicU32> = (0..nflat).map(|_| AtomicU32::new(0)).collect();
    thread::scope(|s| {
        for (stream, mut cur) in streams.into_iter().zip(counts) {
            let flat = &flat;
            s.spawn(move || {
                let mut doc = 0;
                for v in stream {
                    if v >> 31 == 1 {
                        doc = v & !(1 << 31);
                    } else {
                        flat[cur[v as usize] as usize].store(doc, Relaxed);
                        cur[v as usize] += 1;
                    }
                }
            });
        }
    });
    let bits: Vec<u64> = bits.into_iter().map(AtomicU64::into_inner).collect();
    let flat: Vec<u32> = flat.into_iter().map(AtomicU32::into_inner).collect();
    let set = |id: usize| match bit_of[id] {
        Some(b) => Set::Bits(&bits[b * words..][..words]),
        None => Set::Ids(&flat[start[id]..start[id] + df[id]]),
    };

    // Answer the queries in k contiguous blocks, each into its own buffer.
    let nt = index.terms.len as usize;
    let outs: Vec<Vec<u8>> = thread::scope(|s| {
        let hs: Vec<_> = queries
            .chunks(queries.len().div_ceil(k).max(1))
            .map(|block| {
                let (set, span) = (&set, &span);
                s.spawn(move || {
                    let (mut r, mut keep, mut out) = (vec![0u64; words], Vec::new(), Vec::new());
                    for (ops, ids, phrase) in block {
                        if phrase.is_empty() {
                            for (&op, &id) in ops.iter().zip(ids) {
                                apply(&mut r, op, set(id as usize), &mut keep);
                            }
                        } else {
                            for (j, &p) in ids.iter().enumerate() {
                                apply(&mut r, if j == 0 { b'=' } else { b'A' }, set(nt + p as usize), &mut keep);
                            }
                            if phrase.len() > 2 {
                                // Every adjacent pair occurs; check the words occur together, in order.
                                for wi in 0..words {
                                    let mut x = r[wi];
                                    while x != 0 {
                                        let d = wi * 64 + x.trailing_zeros() as usize;
                                        x &= x - 1;
                                        let sp = span[d].load(Relaxed);
                                        let toks: Vec<&[u8]> = tokens(&docs[(sp >> 32) as usize..sp as u32 as usize]).collect();
                                        let found = toks.windows(phrase.len()).any(|w| w.iter().zip(phrase).all(|(a, b)| a.eq_ignore_ascii_case(b)));
                                        if !found {
                                            r[wi] &= !(1 << (d % 64));
                                        }
                                    }
                                }
                            }
                        }
                        let (mut count, mut sum, mut first) = (0u64, 0u64, Vec::new());
                        for (wi, &w) in r.iter().enumerate() {
                            let mut x = w;
                            count += x.count_ones() as u64;
                            while x != 0 {
                                let d = wi * 64 + x.trailing_zeros() as usize;
                                sum += d as u64;
                                if first.len() < 10 {
                                    first.push(d);
                                }
                                x &= x - 1;
                            }
                        }
                        write!(out, "{count} {sum}").unwrap();
                        first.iter().for_each(|d| write!(out, " {d}").unwrap());
                        out.push(b'\n');
                    }
                    out
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    File::create("/out/results.txt").unwrap().write_all(&outs.concat()).unwrap();
}
