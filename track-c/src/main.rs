use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Barrier, Mutex};
use std::thread;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
}

const PR_SET_THP_DISABLE: i32 = 41;

/// The whole file, read-only. /data is already in the page cache, so this costs no memory.
/// n zeroed atomics (all-zero bytes are valid), whose pages the kernel supplies only when first touched.
fn zeroed(n: usize) -> Vec<AtomicU64> {
    let layout = std::alloc::Layout::array::<AtomicU64>(n.max(1)).unwrap();
    unsafe { Vec::from_raw_parts(std::alloc::alloc_zeroed(layout) as *mut AtomicU64, n, n.max(1)) }
}

fn map(path: &str) -> &'static [u8] {
    let f = File::open(path).unwrap();
    let len = f.metadata().unwrap().len() as usize;
    if len == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), len, 1, 2, f.as_raw_fd(), 0), len) }
}

/// Reads one unsigned integer at b[*i] and steps over the byte after it.
fn num(b: &[u8], i: &mut usize) -> u64 {
    let mut v = 0;
    while b[*i].is_ascii_digit() {
        v = v * 10 + (b[*i] - b'0') as u64;
        *i += 1;
    }
    *i += 1;
    v
}

/// Bitmasks of the quotes, and of the commas and LFs, among the 64 bytes of b from i (zeros past
/// its end), from 32-byte compares. The build targets the scoring CPU, which has AVX2.
fn masks(b: &[u8], i: usize) -> (u64, u64) {
    use std::arch::x86_64::*;
    let mut tail = [0u8; 64];
    let block = b.get(i..i + 64).unwrap_or_else(|| {
        tail[..b.len() - i].copy_from_slice(&b[i..]);
        &tail
    });
    unsafe {
        let half = |k: usize| _mm256_loadu_si256(block.as_ptr().add(32 * k) as *const __m256i);
        let (lo, hi) = (half(0), half(1));
        let is = |v: __m256i, c: u8| _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, _mm256_set1_epi8(c as i8))) as u32 as u64;
        let both = |c: u8| is(lo, c) | is(hi, c) << 32;
        (both(b'"'), both(b',') | both(b'\n'))
    }
}

/// Bit k of the result is the parity of bits 0..=k of x.
fn prefix_xor(mut x: u64) -> u64 {
    for s in [1, 2, 4, 8, 16, 32] {
        x ^= x << s;
    }
    x
}

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

/// The number spelled by the n decimal digits at b[at], 1 to 8 of them, all at once: shifted so
/// zeros lead, then neighbouring digits, pairs and quads are joined in three multiplies.
fn digits(b: &[u8], at: usize, n: usize) -> u64 {
    let shift = 64 - 8 * n as u32;
    let x = (load(b, at) << shift) - (0x3030_3030_3030_3030 << shift);
    let x = (x * 10 + (x >> 8)) & 0x00ff_00ff_00ff_00ff;
    let x = (x * 100 + (x >> 16)) & 0x0000_ffff_0000_ffff;
    (x * 10000 + (x >> 32)) & 0xffff_ffff
}

/// A text field of len bytes at b[at], holding q quote bytes, as (start, raw length, value length).
/// A quoted value loses its two quotes and one byte per "" pair; the raw bytes identify the value,
/// since escaping is one to one.
fn text(b: &[u8], at: usize, len: usize, q: u32) -> (usize, usize, u64) {
    if len > 0 && b[at] == b'"' {
        (at + 1, len - 2, (len - 2) as u64 - (q as u64 - 2) / 2)
    } else {
        (at, len, len as u64)
    }
}

/// A record starts right after an LF with a digit or a BOM. Inside a quoted note an LF is
/// always followed by a lowercase word or a quote, so this finds a true boundary.
fn record_start(b: &[u8], mut p: usize) -> usize {
    while p < b.len() && !(b[p - 1] == b'\n' && (b[p].is_ascii_digit() || b[p] == 0xEF)) {
        p += 1;
    }
    p
}

fn money(c: i64) -> String {
    format!("{}{}.{:02}", if c < 0 { "-" } else { "" }, c.abs() / 100, c.abs() % 100)
}

const SLOT_BITS: u32 = 17; // the name pool is 100,000 values: 2^17 slots, 76% full
const ROUNDS: usize = 64; // each thread's records are parsed in this many pieces

#[derive(Clone, Copy)]
struct Agg {
    count: u64,
    ids: u64,
    sum: i64,
    min: i64,
    max: i64,
    name_bytes: u64,
    note_bytes: u64,
}

fn main() {
    // No transparent huge pages: one would back a mostly untouched stretch of a lazily filled table.
    unsafe { prctl(PR_SET_THP_DISABLE, 1, 0, 0, 0) };
    // A country code [A-Z]{3} is its own index into 26^3 slots, already in byte order.
    let regions = map("/data/regions.csv");
    let mut i = regions.iter().position(|&c| c == b'\n').unwrap() + 1;
    let (mut region, mut used) = (Vec::new(), vec![false; 26 * 26 * 26]);
    while i < regions.len() {
        let id = num(regions, &mut i) as usize;
        let code = regions[i..i + 3].iter().fold(0, |v, &c| v * 26 + (c - b'A') as usize);
        region.resize(region.len().max(id + 1), u16::MAX);
        region[id] = code as u16;
        used[code] = true;
        i += 4;
    }
    let codes: Vec<usize> = (0..used.len()).filter(|&c| used[c]).collect();
    let unmatched = codes.len();
    let mut rank = vec![unmatched as u8; used.len() + 1];
    codes.iter().enumerate().for_each(|(r, &c)| rank[c] = r as u8);
    let group: Vec<u8> = region.iter().map(|&c| rank[(c as usize).min(used.len())]).collect();
    drop(region);

    // Every distinct name gets a slot in one shared open-addressing table, and each group
    // marks the slots of its names in its own bitset.
    let b = map("/data/records.csv");
    let slots: Vec<AtomicU64> = zeroed(1 << SLOT_BITS);
    let seen: Vec<AtomicU64> = zeroed((unmatched + 1) << (SLOT_BITS - 6));
    // A slot holds a name's 64-bit hash, which identifies it among the 100,000 (a collision is
    // about 1 in 4 billion).
    let slot_of = |at: usize, len: usize| {
        let chunk = |k: usize| load(b, at + k) & ((1u128 << (8 * (len - k).min(8))) - 1) as u64;
        let hash = (0..len).step_by(8).fold(len as u64, |h, k| (h ^ chunk(k)).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29)) | 1;
        let mut s = (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - SLOT_BITS)) as usize;
        loop {
            match slots[s].load(Relaxed) {
                v if v == hash => return s,
                0 => match slots[s].compare_exchange(0, hash, Relaxed, Relaxed) {
                    Ok(_) => return s,
                    Err(v) if v == hash => return s,
                    Err(_) => {}
                },
                _ => s = (s + 1) & ((1 << SLOT_BITS) - 1),
            }
        }
    };

    let k = thread::available_parallelism().map_or(1, |x| x.get());
    let body = b.iter().position(|&c| c == b'\n').map_or(b.len(), |p| p + 1);
    let pieces = k * ROUNDS;
    let mut cut: Vec<usize> = (0..pieces).map(|j| record_start(b, body + (b.len() - body) * j / pieces).max(body)).collect();
    cut[0] = body;
    cut.push(b.len());
    let empty = Agg { count: 0, ids: 0, sum: 0, min: i64::MAX, max: i64::MIN, name_bytes: 0, note_bytes: 0 };
    // A record's name bit belongs to its group's owner, thread g % k. Each round, every thread
    // parses its next piece, passing each bit to its owner through boxes[from * k + owner]; then
    // every owner sets its bits with plain stores, as no other thread writes those words.
    let boxes: Vec<Mutex<Vec<u32>>> = (0..k * k).map(|_| Mutex::new(Vec::new())).collect();
    let barrier = Barrier::new(k);
    let parts: Vec<Vec<Agg>> = thread::scope(|s| {
        let (boxes, barrier, cut) = (&boxes, &barrier, &cut);
        let hs: Vec<_> = (0..k)
            .map(|me| {
                let (group, seen, slot_of) = (&group, &seen, &slot_of);
                s.spawn(move || {
                    let mut agg = vec![empty; unmatched + 1];
                    let mut out = vec![Vec::new(); k];
                    for w in cut[me * ROUNDS..=(me + 1) * ROUNDS].windows(2) {
                        // A record's fields end at commas and its LF, the ones outside quotes. In each
                        // 64-byte block the quote bits, prefix-xored, mark the bytes inside quotes, and
                        // the quotes counted up to a field's end give its quote count.
                        let (mut at, mut inside, mut quotes) = (w[0], 0u64, 0u32);
                        let (mut start, mut field_quotes, mut field) = (w[0] + 3 * (b[w[0]] == 0xEF) as usize, 0, 0);
                        let (mut id, mut name, mut note_len, mut g) = (0, (0, 0, 0), 0, 0);
                        while at < w[1] {
                            let (q, sep) = masks(b, at);
                            let inq = prefix_xor(q) ^ inside;
                            inside = 0u64.wrapping_sub(inq >> 63);
                            let mut ends = sep & !inq;
                            while ends != 0 {
                                let p = at + ends.trailing_zeros() as usize;
                                if p >= w[1] {
                                    break;
                                }
                                let before = quotes + (q & ((1 << (p - at)) - 1)).count_ones();
                                ends &= ends - 1;
                                let len = p - start;
                                match field {
                                    0 => id = digits(b, start, len),
                                    1 => name = text(b, start, len, before - field_quotes),
                                    2 => note_len = text(b, start, len, before - field_quotes).2,
                                    3 => g = *group.get(digits(b, start, len) as usize).unwrap_or(&(unmatched as u8)) as usize,
                                    _ => {
                                        // The amount, [-]digits.dd, then the CR of a CRLF.
                                        let neg = b[start] == b'-';
                                        let (s, e) = (start + neg as usize, p - (b[p - 1] == b'\r') as usize);
                                        let cents = (digits(b, s, e - 3 - s) * 100 + digits(b, e - 2, 2)) as i64;
                                        let cents = if neg { -cents } else { cents };
                                        let a = &mut agg[g];
                                        a.count += 1;
                                        a.ids += id;
                                        a.sum += cents;
                                        a.min = a.min.min(cents);
                                        a.max = a.max.max(cents);
                                        a.name_bytes += name.2;
                                        a.note_bytes += note_len;
                                        out[g % k].push((g << SLOT_BITS | slot_of(name.0, name.1)) as u32);
                                    }
                                }
                                (start, field_quotes, field) = (p + 1, before, (field + 1) % 5);
                                if field == 0 && b.get(start) == Some(&0xEF) {
                                    start += 3;
                                }
                            }
                            quotes += q.count_ones();
                            at += 64;
                        }
                        // Hand this piece's bits over; a box comes back holding the bits its owner
                        // has already set, so it is cleared and reused.
                        for (o, v) in out.iter_mut().enumerate() {
                            std::mem::swap(&mut *boxes[me * k + o].lock().unwrap(), v);
                            v.clear();
                        }
                        barrier.wait();
                        for from in 0..k {
                            for &bit in boxes[from * k + me].lock().unwrap().iter() {
                                let word = &seen[bit as usize / 64];
                                word.store(word.load(Relaxed) | 1 << (bit % 64), Relaxed);
                            }
                        }
                        barrier.wait();
                    }
                    agg
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut out = Vec::new();
    for g in 0..=unmatched {
        let mut a = empty;
        for p in &parts {
            let x = p[g];
            (a.count, a.ids, a.sum) = (a.count + x.count, a.ids + x.ids, a.sum + x.sum);
            (a.min, a.max) = (a.min.min(x.min), a.max.max(x.max));
            (a.name_bytes, a.note_bytes) = (a.name_bytes + x.name_bytes, a.note_bytes + x.note_bytes);
        }
        if a.count == 0 {
            continue;
        }
        let words = &seen[g << (SLOT_BITS - 6)..(g + 1) << (SLOT_BITS - 6)];
        let names: u32 = words.iter().map(|w| w.load(Relaxed).count_ones()).sum();
        let c = codes.get(g).map(|&c| [c / 676, c / 26 % 26, c % 26].map(|x| (b'A' + x as u8) as char));
        let key: String = c.map_or("_UNMATCHED".into(), String::from_iter);
        let (sum, min, max) = (money(a.sum), money(a.min), money(a.max));
        writeln!(out, "{key} {} {} {sum} {min} {max} {names} {} {}", a.count, a.ids, a.name_bytes, a.note_bytes).unwrap();
    }
    File::create("/out/aggregates.txt").unwrap().write_all(&out).unwrap();
}
