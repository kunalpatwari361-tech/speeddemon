use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::thread;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

/// The whole file, read-only. /data is already in the page cache, so this costs no memory.
/// n zeroed values (all-zero bytes are valid for these atomics), whose pages the kernel supplies
/// only when first touched.
fn zeroed<T>(n: usize) -> Vec<T> {
    let layout = std::alloc::Layout::array::<T>(n.max(1)).unwrap();
    unsafe { Vec::from_raw_parts(std::alloc::alloc_zeroed(layout) as *mut T, n, n.max(1)) }
}

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

/// Runs f(j) for every j in 0..k, each on its own thread, and returns the results in order.
fn each<T: Send>(k: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    thread::scope(|s| {
        let f = &f;
        let hs: Vec<_> = (0..k).map(|j| s.spawn(move || f(j))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
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
            let doc = num(&b[i..]) as u32;
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
            (_, Set::Bits(b), true) => r.iter_mut().zip(b).for_each(|(x, &y)| *x = match op {
                b'A' => *x & y,
                b'O' => *x | y,
                _ => *x & !y,
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

/// The documents are indexed and queried in this many rounds, so only one round's postings are held.
const ROUNDS: usize = 8;

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
    drop((terms, pairs));
    let ids = (index.terms.len + index.pairs.len) as usize;
    let n = docs.rsplit(|&c| c == b'\n').nth(1).map_or(0, |last| num(last) + 1); // the last id + 1
    let span: Vec<AtomicU64> = zeroed(n);
    let k = thread::available_parallelism().map_or(1, |x| x.get());

    // The documents go in ROUNDS rounds of k blocks, one block per thread. A round's postings are
    // built, queried and freed before the next round starts.
    let blocks = cuts(docs, k * ROUNDS);
    let first = |b: usize| if blocks[b] < docs.len() { num(&docs[blocks[b]..]) } else { n };

    // A sample, the first thirty-second of every block, estimates which ids are common. Those get a
    // bitset over the round's documents, which the full pass sets directly.
    let sample = each(k, |j| {
        let mut state = vec![(0, 0); ids];
        for b in (j..k * ROUNDS).step_by(k) {
            index.scan(blocks[b], blocks[b] + (blocks[b + 1] - blocks[b]) / 32, &mut state, &span, |_, _, c| *c += 1);
        }
        state.iter().map(|s| s.1).collect::<Vec<u32>>()
    });
    let mut nbits = 0;
    let bit_of: Vec<Option<usize>> = (0..ids)
        .map(|id| (sample.iter().map(|s| s[id] as usize).sum::<usize>() * 32 * 32 >= n).then(|| (nbits, nbits += 1).0))
        .collect();
    let rare: Vec<usize> = sample.iter().map(|s| (0..ids).filter(|&id| bit_of[id].is_none()).map(|id| s[id] as usize).sum()).collect();
    drop(sample);

    // Every query's count, id sum and first ten ids, built up round by round.
    let mut acc = vec![(0u64, 0u64, [0u32; 10]); queries.len()];
    let chunk = queries.len().div_ceil(k).max(1);
    for round in 0..ROUNDS {
        let (d0, d1) = (first(round * k), first(round * k + k));
        let words = (d1 - d0).div_ceil(64);
        let bits: Vec<AtomicU64> = zeroed(nbits * words);

        // The full pass: a common id sets its bit; every other hit is counted and goes to the thread's
        // stream as its id, after a marker (top bit set) naming the document, counted from the round's first.
        let (streams, mut counts): (Vec<Vec<u32>>, Vec<Vec<u32>>) = each(k, |j| {
            // An id's state is (last document + 1, its bitset index), or for a rare id (.., 1 << 31 | hits).
            let mut state: Vec<(u32, u32)> = bit_of.iter().map(|b| (0, b.map_or(1 << 31, |b| b as u32))).collect();
            let b = round * k + j;
            let (mut stream, mut last) = (Vec::with_capacity(rare[j] * 33 / ROUNDS + first(b + 1) - first(b)), u32::MAX);
            index.scan(blocks[b], blocks[b + 1], &mut state, &span, |doc, id, x| {
                let doc = doc - d0 as u32;
                if *x >> 31 == 0 {
                    let w = &bits[*x as usize * words + doc as usize / 64];
                    w.store(w.load(Relaxed) | 1 << (doc % 64), Relaxed);
                } else {
                    *x += 1;
                    if doc != last {
                        (last, _) = (doc, stream.push(1 << 31 | doc));
                    }
                    stream.push(id as u32);
                }
            });
            (stream, state.iter().map(|s| if s.1 >> 31 == 1 { s.1 & !(1 << 31) } else { 0 }).collect())
        })
        .into_iter()
        .unzip();

        // The other ids get a run of one flat array, and each thread places its stream into its own
        // part of every run: its count of an id becomes its cursor in the id's run.
        let mut start = vec![0u32; ids + 1];
        for id in 0..ids {
            let mut at = start[id];
            for c in counts.iter_mut() {
                (c[id], at) = (at, at + c[id]);
            }
            start[id + 1] = at;
        }
        let flat: Vec<AtomicU32> = zeroed(start[ids] as usize);
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
        let nt = index.terms.len;
        let set = |id: u32| {
            let id = if id >> 31 == 1 { (nt + (id & !(1 << 31))) as usize } else { id as usize };
            match bit_of[id] {
                Some(b) => Set::Bits(&bits[b * words..][..words]),
                None => Set::Ids(&flat[start[id] as usize..start[id + 1] as usize]),
            }
        };

        // Answer the queries on the round's documents in k contiguous blocks of queries.
        thread::scope(|s| {
            for (block, acc) in queries.chunks(chunk).zip(acc.chunks_mut(chunk)) {
                let (set, span) = (&set, &span);
                s.spawn(move || {
                    let (mut r, mut v) = (vec![0u64; words], Vec::new());
                    for ((steps, phrase), (count, sum, ten)) in block.iter().zip(acc) {
                        eval(steps.iter().map(|&(op, id)| (op, set(id))), &mut r, &mut v);
                        if phrase.len() > 2 {
                            // Every adjacent pair occurs; check the words occur together, in order.
                            v.retain(|&d| {
                                let sp = span[d0 + d as usize].load(Relaxed);
                                let toks: Vec<&[u8]> = tokens(&docs[(sp >> 32) as usize..sp as u32 as usize]).collect();
                                toks.windows(phrase.len()).any(|w| w.iter().zip(phrase).all(|(a, b)| a.eq_ignore_ascii_case(b)))
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
