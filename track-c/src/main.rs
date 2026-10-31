use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::thread;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

// The record parser, in C++ (native/parse.cpp), built into native/libparse.a by the Dockerfile.
#[link(name = "parse", kind = "static")]
extern "C" {
    fn parse_part(b: *const u8, from: usize, to: usize, group: *const u8, groups: usize, unmatched: usize, slots: *mut u64, seen: *mut u64, agg: *mut Agg);
}

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

/// One group's totals. The C++ parser fills them in, so the layout is C's.
#[repr(C)]
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
    // marks the slots of its names in its own bitset. A slot holds a name's 64-bit hash, which
    // identifies it among the 100,000 (a collision is about 1 in 4 billion).
    let b = map("/data/records.csv");
    let slots: Vec<AtomicU64> = zeroed(1 << SLOT_BITS);
    let seen: Vec<AtomicU64> = zeroed((unmatched + 1) << (SLOT_BITS - 6));

    // Each part of the file is parsed in C++, into that part's own totals per group.
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
                let (group, slots, seen) = (&group, &slots, &seen);
                s.spawn(move || {
                    let mut agg = vec![empty; unmatched + 1];
                    unsafe {
                        parse_part(
                            b.as_ptr(),
                            w[0],
                            w[1],
                            group.as_ptr(),
                            group.len(),
                            unmatched,
                            slots.as_ptr() as *mut u64,
                            seen.as_ptr() as *mut u64,
                            agg.as_mut_ptr(),
                        )
                    };
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
