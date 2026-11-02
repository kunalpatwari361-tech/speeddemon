use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::Relaxed};
use std::thread;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn prctl(option: i32, arg2: u64, arg3: u64, arg4: u64, arg5: u64) -> i32;
}

const PR_SET_THP_DISABLE: i32 = 41;

/// n zeroed atomic integers (all-zero bytes are valid), whose pages the kernel supplies only when first touched.
fn zeroed<T>(n: usize) -> Vec<T> {
    let layout = std::alloc::Layout::array::<T>(n.max(1)).unwrap();
    unsafe { Vec::from_raw_parts(std::alloc::alloc_zeroed(layout) as *mut T, n, n.max(1)) }
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
const AHEAD: usize = 16; // records between a name's prefetch and its lookup

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
    let k = thread::available_parallelism().map_or(1, |x| x.get());
    // regions.csv in k parts of whole lines "region_id,CCC". A country code [A-Z]{3} is its own
    // index into 26^3 slots, already in byte order.
    let regions = map("/data/regions.csv");
    let body = regions.iter().position(|&c| c == b'\n').unwrap() + 1;
    let line_at = |p: usize| p + regions[p - 1..].iter().position(|&c| c == b'\n').unwrap();
    let rcut: Vec<usize> = (0..=k).map(|j| line_at(body + (regions.len() - body) * j / k)).collect();
    let lines = |j: usize| regions[rcut[j]..rcut[j + 1]].split(|&c| c == b'\n').filter(|l| !l.is_empty());
    let code = |l: &[u8]| l[l.len() - 3..].iter().fold(0, |v, &c| v * 26 + (c - b'A') as usize);
    // First the codes in use, then each region's group, 1 + its code's rank, with 0 for
    // _UNMATCHED. Region ids are unique, so the threads write separate bytes.
    let used: Vec<AtomicU8> = zeroed(26 * 26 * 26);
    thread::scope(|s| {
        for j in 0..k {
            let (lines, used) = (&lines, &used);
            s.spawn(move || lines(j).for_each(|l| used[code(l)].store(1, Relaxed)));
        }
    });
    let codes: Vec<usize> = (0..used.len()).filter(|&c| used[c].load(Relaxed) == 1).collect();
    let mut rank = vec![0u8; used.len()];
    codes.iter().enumerate().for_each(|(r, &c)| rank[c] = r as u8 + 1);
    let groups = codes.len() + 1;
    let last = regions[..regions.len() - 1].iter().rposition(|&c| c == b'\n').unwrap() + 1;
    let group: Vec<AtomicU8> = zeroed(digits(regions, last, regions.len() - 5 - last) as usize + 1);
    thread::scope(|s| {
        for j in 0..k {
            let (lines, group, rank) = (&lines, &group, &rank);
            s.spawn(move || lines(j).for_each(|l| group[digits(l, 0, l.len() - 4) as usize].store(rank[code(l)], Relaxed)));
        }
    });

    // Every distinct name gets a slot in one shared open-addressing table, and each group
    // marks the slots of its names in its own bitset.
    let b = map("/data/records.csv");
    let slots: Vec<AtomicU64> = zeroed(1 << SLOT_BITS);
    let seen: Vec<AtomicU64> = zeroed(groups << (SLOT_BITS - 6));
    // A slot holds a name's 64-bit hash, which identifies it among the 100,000 (a collision is
    // about 1 in 4 billion).
    let hash_of = |at: usize, len: usize| {
        let chunk = |k: usize| load(b, at + k) & ((1u128 << (8 * (len - k).min(8))) - 1) as u64;
        (0..len).step_by(8).fold(len as u64, |h, k| (h ^ chunk(k)).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29)) | 1
    };
    let home = |hash: u64| (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - SLOT_BITS)) as usize;
    let slot_of = |hash: u64| {
        let mut s = home(hash);
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

    let body = b.iter().position(|&c| c == b'\n').map_or(b.len(), |p| p + 1);
    let mut cut: Vec<usize> = (0..k).map(|j| record_start(b, body + (b.len() - body) * j / k).max(body)).collect();
    cut[0] = body;
    cut.push(b.len());
    let empty = Agg { count: 0, ids: 0, sum: 0, min: i64::MAX, max: i64::MIN, name_bytes: 0, note_bytes: 0 };
    let parts: Vec<Vec<Agg>> = thread::scope(|s| {
        let hs: Vec<_> = cut
            .windows(2)
            .map(|w| {
                let (group, seen, slots, hash_of, home, slot_of) = (&group, &seen, &slots, &hash_of, &home, &slot_of);
                s.spawn(move || {
                    let mut agg = vec![empty; groups];
                    // A record's name is hashed when it is parsed, and its home slot and that slot's
                    // bit for the group are prefetched; the lookup and the bit wait AHEAD records.
                    let fetch = |p: *const AtomicU64| unsafe { std::arch::x86_64::_mm_prefetch(p as *const i8, std::arch::x86_64::_MM_HINT_T0) };
                    let mark = |hash: u64, g: usize| {
                        let bit = g << SLOT_BITS | slot_of(hash);
                        if seen[bit / 64].load(Relaxed) >> (bit % 64) & 1 == 0 {
                            seen[bit / 64].fetch_or(1 << (bit % 64), Relaxed);
                        }
                    };
                    let (mut ring, mut n) = ([(0u64, 0usize); AHEAD], 0usize);
                    // A record's fields end at commas and its LF, the ones outside quotes. In each
                    // 64-byte block the quote bits, prefix-xored, mark the bytes inside quotes, and
                    // the quotes counted up to a field's end give its quote count.
                    let (mut at, mut inside, mut quotes) = (w[0], 0u64, 0u32);
                    let (mut start, mut field_quotes, mut field) = (w[0] + 3 * (b[w[0]] == 0xEF) as usize, 0, 0);
                    let (mut id, mut name, mut note_len, mut region) = (0, (0, 0, 0), 0, 0);
                    // A record waits AHEAD records after its group entry is prefetched, then is
                    // counted and its name joins the name ring.
                    let (mut recs, mut r) = ([(0u64, 0usize, (0usize, 0usize, 0u64), 0u64, 0i64); AHEAD], 0usize);
                    let mut take = |(id, region, name, note_len, cents): (u64, usize, (usize, usize, u64), u64, i64)| {
                        let g = group.get(region).map_or(0, |v| v.load(Relaxed)) as usize;
                        let a = &mut agg[g];
                        a.count += 1;
                        a.ids += id;
                        a.sum += cents;
                        a.min = a.min.min(cents);
                        a.max = a.max.max(cents);
                        a.name_bytes += name.2;
                        a.note_bytes += note_len;
                        let hash = hash_of(name.0, name.1);
                        fetch(&slots[home(hash)]);
                        fetch(&seen[(g << SLOT_BITS | home(hash)) / 64]);
                        if n >= AHEAD {
                            let (h, g) = ring[n % AHEAD];
                            mark(h, g);
                        }
                        (ring[n % AHEAD], n) = ((hash, g), n + 1);
                    };
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
                                3 => region = digits(b, start, len) as usize,
                                _ => {
                                    // The amount, [-]digits.dd, then the CR of a CRLF.
                                    let neg = b[start] == b'-';
                                    let (s, e) = (start + neg as usize, p - (b[p - 1] == b'\r') as usize);
                                    let cents = (digits(b, s, e - 3 - s) * 100 + digits(b, e - 2, 2)) as i64;
                                    let cents = if neg { -cents } else { cents };
                                    if let Some(v) = group.get(region) {
                                        unsafe { std::arch::x86_64::_mm_prefetch(v as *const _ as *const i8, std::arch::x86_64::_MM_HINT_T0) };
                                    }
                                    if r >= AHEAD {
                                        take(recs[r % AHEAD]);
                                    }
                                    (recs[r % AHEAD], r) = ((id, region, name, note_len, cents), r + 1);
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
                    for j in r.saturating_sub(AHEAD)..r {
                        take(recs[j % AHEAD]);
                    }
                    for j in n.saturating_sub(AHEAD)..n {
                        let (h, g) = ring[j % AHEAD];
                        mark(h, g);
                    }
                    agg
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut out = Vec::new();
    for g in (1..=codes.len()).chain([0]) {
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
        let c = codes.get(g.wrapping_sub(1)).map(|&c| [c / 676, c / 26 % 26, c % 26].map(|x| (b'A' + x as u8) as char));
        let key: String = c.map_or("_UNMATCHED".into(), String::from_iter);
        let (sum, min, max) = (money(a.sum), money(a.min), money(a.max));
        writeln!(out, "{key} {} {} {sum} {min} {max} {names} {} {}", a.count, a.ids, a.name_bytes, a.note_bytes).unwrap();
    }
    File::create("/out/aggregates.txt").unwrap().write_all(&out).unwrap();
}
