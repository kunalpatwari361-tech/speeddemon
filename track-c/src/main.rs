use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
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

/// The first `"` at or after i, 8 bytes at a time: it is the first zero byte of a word xored
/// with quotes. A record's end follows every quote, so every 8-byte read is in bounds.
fn quote(b: &[u8], mut i: usize) -> usize {
    loop {
        let x = u64::from_le_bytes(b[i..i + 8].try_into().unwrap()) ^ 0x2222_2222_2222_2222;
        let found = x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080;
        if found != 0 {
            return i + found.trailing_zeros() as usize / 8;
        }
        i += 8;
    }
}

/// A text field at b[*i] as (start, raw length, value length); leaves *i past its comma.
/// The raw bytes of a quoted field identify its value, since escaping is one to one.
fn field(b: &[u8], i: &mut usize) -> (usize, usize, u64) {
    let start = *i;
    if b[start] != b'"' {
        *i += b[start..].iter().position(|&c| c == b',').unwrap() + 1;
        return (start, *i - 1 - start, (*i - 1 - start) as u64);
    }
    let (mut j, mut escapes) = (start + 1, 0);
    loop {
        j = quote(b, j);
        if b[j + 1] != b'"' {
            break;
        }
        escapes += 1;
        j += 2;
    }
    *i = j + 2;
    (start + 1, j - start - 1, (j - start - 1 - escapes) as u64)
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

const SLOT_BITS: u32 = 18; // the name pool is 100,000 values

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

    // Every distinct name gets a slot in one shared open-addressing table, and each group
    // marks the slots of its names in its own bitset.
    let b = map("/data/records.csv");
    let slots: Vec<AtomicU64> = (0..1usize << SLOT_BITS).map(|_| AtomicU64::new(0)).collect();
    let seen: Vec<AtomicU64> = (0..(unmatched + 1) << (SLOT_BITS - 6)).map(|_| AtomicU64::new(0)).collect();
    // A slot holds a name's 64-bit hash, which identifies it among the 100,000 (a collision is
    // about 1 in 4 billion).
    let slot_of = |at: usize, len: usize| {
        // The name hashed 8 bytes at a time (the last piece zero-padded), so a name costs a few
        // multiplies, not one per byte.
        let hash = b[at..at + len].chunks(8).fold(len as u64, |h, c| {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            (h ^ u64::from_le_bytes(w)).wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left(29)
        }) | 1;
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
    let mut cut: Vec<usize> = (0..k).map(|j| record_start(b, body + (b.len() - body) * j / k).max(body)).collect();
    cut[0] = body;
    cut.push(b.len());
    let empty = Agg { count: 0, ids: 0, sum: 0, min: i64::MAX, max: i64::MIN, name_bytes: 0, note_bytes: 0 };
    let parts: Vec<Vec<Agg>> = thread::scope(|s| {
        let hs: Vec<_> = cut
            .windows(2)
            .map(|w| {
                let (group, seen, slot_of) = (&group, &seen, &slot_of);
                s.spawn(move || {
                    let mut agg = vec![empty; unmatched + 1];
                    let mut i = w[0];
                    while i < w[1] {
                        if b[i] == 0xEF {
                            i += 3;
                        }
                        let id = num(b, &mut i);
                        let (name_at, name_raw, name_len) = field(b, &mut i);
                        let (_, _, note_len) = field(b, &mut i);
                        let g = *group.get(num(b, &mut i) as usize).unwrap_or(&(unmatched as u8)) as usize;
                        let neg = b[i] == b'-';
                        i += neg as usize;
                        let cents = (num(b, &mut i) * 100 + (b[i] - b'0') as u64 * 10 + (b[i + 1] - b'0') as u64) as i64;
                        let cents = if neg { -cents } else { cents };
                        i += 2 + (b[i + 2] == b'\r') as usize + 1;

                        let a = &mut agg[g];
                        a.count += 1;
                        a.ids += id;
                        a.sum += cents;
                        a.min = a.min.min(cents);
                        a.max = a.max.max(cents);
                        a.name_bytes += name_len;
                        a.note_bytes += note_len;
                        let bit = g << SLOT_BITS | slot_of(name_at, name_raw);
                        if seen[bit / 64].load(Relaxed) >> (bit % 64) & 1 == 0 {
                            seen[bit / 64].fetch_or(1 << (bit % 64), Relaxed);
                        }
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
