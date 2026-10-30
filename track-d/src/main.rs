use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
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

/// k + 1 line-aligned cut points splitting b[from..] into k parts.
fn cuts(b: &[u8], from: usize, k: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..k)
        .map(|j| from + (b.len() - from) * j / k)
        .map(|p| if p == from { p } else { p + b[p..].iter().position(|&c| c == b'\n').map_or(b.len() - p, |x| x + 1) })
        .collect();
    v.push(b.len());
    v
}

/// Node ids fit in the low bits of a heap entry; distances, below 2^43 for this generator, above them.
const NODE_BITS: u32 = 21;

/// Dijkstra from s, stopping when t is settled. Leaves dist all u64::MAX again.
fn search(off: &[u32], adj: &[u64], dist: &mut [u64], seen: &mut Vec<usize>, s: usize, t: usize) -> i64 {
    let mut heap = BinaryHeap::from([Reverse(s as u64)]);
    dist[s] = 0;
    seen.push(s);
    let mut found = -1;
    while let Some(Reverse(top)) = heap.pop() {
        let (d, u) = (top >> NODE_BITS, (top & ((1 << NODE_BITS) - 1)) as usize);
        if d > dist[u] {
            continue;
        }
        if u == t {
            found = d as i64;
            break;
        }
        for &e in &adj[off[u] as usize..off[u + 1] as usize] {
            let (v, nd) = ((e >> 32) as usize, d + (e & 0xffff_ffff));
            if nd < dist[v] {
                if dist[v] == u64::MAX {
                    seen.push(v);
                }
                dist[v] = nd;
                heap.push(Reverse(nd << NODE_BITS | v as u64));
            }
        }
    }
    seen.drain(..).for_each(|v| dist[v] = u64::MAX);
    found
}

fn main() {
    let g = map("/data/graph.txt");
    let mut i = 0;
    let (n, m) = (num(g, &mut i) as usize, num(g, &mut i) as usize);
    let k = thread::available_parallelism().map_or(1, |x| x.get());
    let parts = cuts(g, i, k);
    let edges = |f: &(dyn Fn(usize, u64, u64) + Sync)| {
        thread::scope(|s| {
            for w in parts.windows(2) {
                s.spawn(move || {
                    let mut i = w[0];
                    while i < w[1] {
                        let (u, v, c) = (num(g, &mut i), num(g, &mut i), num(g, &mut i));
                        f(u as usize, v, c);
                    }
                });
            }
        })
    };

    // Compressed adjacency: count out-degrees and mark nodes with in-edges, then place every edge.
    let deg: Vec<AtomicU32> = (0..=n).map(|_| AtomicU32::new(0)).collect();
    let has_in: Vec<AtomicU64> = (0..n / 64 + 1).map(|_| AtomicU64::new(0)).collect();
    edges(&|u, v, _| {
        deg[u + 1].fetch_add(1, Relaxed);
        has_in[v as usize / 64].fetch_or(1 << (v % 64), Relaxed);
    });
    let mut off: Vec<u32> = deg.into_iter().map(AtomicU32::into_inner).collect();
    for j in 1..=n {
        off[j] += off[j - 1];
    }
    let next: Vec<AtomicU32> = off[..n].iter().map(|&o| AtomicU32::new(o)).collect();
    let adj: Vec<AtomicU64> = (0..m).map(|_| AtomicU64::new(0)).collect();
    edges(&|u, v, c| adj[next[u].fetch_add(1, Relaxed) as usize].store(v << 32 | c, Relaxed));
    drop(next);
    let adj: Vec<u64> = adj.into_iter().map(AtomicU64::into_inner).collect();

    // Ids are a random relabelling. Renumber so a search touches nearby memory: k breadth-first
    // searches grow regions in parallel, each claiming unclaimed nodes, and each region gets a
    // contiguous id range. Nodes no region reached go last.
    let owner: Vec<AtomicU32> = (0..n).map(|_| AtomicU32::new(u32::MAX)).collect();
    let mut regions: Vec<Vec<usize>> = thread::scope(|s| {
        let hs: Vec<_> = (0..k)
            .map(|r| {
                let (off, adj, owner) = (&off, &adj, &owner);
                s.spawn(move || {
                    let root = r * n / k;
                    let mut order = Vec::new();
                    if owner[root].compare_exchange(u32::MAX, r as u32, Relaxed, Relaxed).is_ok() {
                        order.push(root);
                    }
                    let mut head = 0;
                    while let Some(&u) = order.get(head) {
                        head += 1;
                        for &e in &adj[off[u] as usize..off[u + 1] as usize] {
                            let v = (e >> 32) as usize;
                            if owner[v].load(Relaxed) == u32::MAX && owner[v].compare_exchange(u32::MAX, r as u32, Relaxed, Relaxed).is_ok() {
                                order.push(v);
                            }
                        }
                    }
                    order
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    regions.push((0..n).filter(|&v| owner[v].load(Relaxed) == u32::MAX).collect());
    let order: Vec<usize> = regions.concat();
    let mut new = vec![0u32; n];
    for (j, &u) in order.iter().enumerate() {
        new[u] = j as u32;
    }
    let mut off2 = vec![0u32; n + 1];
    for (j, &u) in order.iter().enumerate() {
        off2[j + 1] = off2[j] + off[u + 1] - off[u];
    }
    let mut adj2 = vec![0u64; m];
    thread::scope(|s| {
        let mut rest = &mut adj2[..];
        for c in order.chunks(n.div_ceil(k).max(1)) {
            let len = c.iter().map(|&u| (off[u + 1] - off[u]) as usize).sum();
            let (mine, tail) = std::mem::take(&mut rest).split_at_mut(len);
            rest = tail;
            let (off, adj, new) = (&off, &adj, &new);
            s.spawn(move || {
                let mut out = mine.iter_mut();
                for &u in c {
                    for &e in &adj[off[u] as usize..off[u + 1] as usize] {
                        *out.next().unwrap() = (new[(e >> 32) as usize] as u64) << 32 | e & 0xffff_ffff;
                    }
                }
            });
        }
    });
    drop((off, adj, owner, regions, order));
    let (off, adj) = (off2, adj2);

    let q = map("/data/queries.txt");
    let mut i = 0;
    let mut queries = Vec::new();
    while i < q.len() {
        queries.push((num(q, &mut i) as usize, num(q, &mut i) as usize));
    }
    let answers: Vec<AtomicU64> = queries.iter().map(|_| AtomicU64::new(0)).collect();
    let todo = AtomicUsize::new(0);
    thread::scope(|s| {
        for _ in 0..k.min(queries.len()) {
            s.spawn(|| {
                let (mut dist, mut seen) = (vec![u64::MAX; n], Vec::new());
                loop {
                    let j = todo.fetch_add(1, Relaxed);
                    let Some(&(src, t)) = queries.get(j) else { break };
                    let d = if src == t {
                        0
                    } else if has_in[t / 64].load(Relaxed) >> (t % 64) & 1 == 0 {
                        -1
                    } else {
                        search(&off, &adj, &mut dist, &mut seen, new[src] as usize, new[t] as usize)
                    };
                    answers[j].store(d as u64, Relaxed);
                }
            });
        }
    });

    let mut out = Vec::with_capacity(queries.len() * 12);
    for a in answers {
        writeln!(out, "{}", a.into_inner() as i64).unwrap();
    }
    File::create("/out/distances.txt").unwrap().write_all(&out).unwrap();
}
