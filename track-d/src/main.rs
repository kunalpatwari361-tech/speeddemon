use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::thread;

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn mallopt(param: i32, value: i32) -> i32;
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

/// n zeroed atomic integers (all-zero bytes are a valid value), whose pages the kernel supplies
/// only when first touched.
fn zeroed<T>(n: usize) -> Vec<T> {
    let layout = std::alloc::Layout::array::<T>(n.max(1)).unwrap();
    unsafe { Vec::from_raw_parts(std::alloc::alloc_zeroed(layout) as *mut T, n, n.max(1)) }
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

/// A radix heap: a priority queue for distances that never fall below the last one popped, which Dijkstra's
/// never do. An entry waits in the bucket of the highest bit where its distance differs from the last one
/// popped, and moves to a lower bucket each time its bucket is redistributed. An entry packs its node id into
/// the low NODE_BITS bits, so distances must stay below 2^43, as they do for this generator.
struct RadixHeap {
    last: u64,
    len: usize,
    buckets: [Vec<u64>; 65],
}

impl RadixHeap {
    fn new() -> Self {
        RadixHeap { last: 0, len: 0, buckets: std::array::from_fn(|_| Vec::new()) }
    }

    /// Empties the heap but keeps its buckets' storage for the next search.
    fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.last = 0;
        self.len = 0;
    }

    fn push(&mut self, d: u64, v: u32) {
        self.buckets[bucket(d, self.last)].push(d << NODE_BITS | v as u64);
        self.len += 1;
    }

    /// Removes an entry with the smallest distance, as (distance, node).
    fn pop(&mut self) -> Option<(u64, u32)> {
        if self.len == 0 {
            return None;
        }
        if self.buckets[0].is_empty() {
            let i = (1..65).find(|&i| !self.buckets[i].is_empty()).unwrap();
            let mut moved = std::mem::take(&mut self.buckets[i]);
            self.last = moved.iter().map(|&e| e >> NODE_BITS).min().unwrap();
            for &e in &moved {
                self.buckets[bucket(e >> NODE_BITS, self.last)].push(e);
            }
            moved.clear();
            self.buckets[i] = moved;
        }
        self.len -= 1;
        self.buckets[0].pop().map(|e| (e >> NODE_BITS, (e & ((1 << NODE_BITS) - 1)) as u32))
    }
}

/// The bucket of a distance: 0 when it equals the last one popped, else one more than the highest bit where they differ.
fn bucket(d: u64, last: u64) -> usize {
    (64 - (d ^ last).leading_zeros()) as usize
}

/// Dijkstra from s, stopping when t is settled, over a zeroed table of D holding distance + 1
/// (0 is unreached). If the target cannot be reached within D, returns None for a wider retry.
/// Leaves the table zeroed again.
fn search<D: Copy + Default + Into<u64> + TryFrom<u64>>(off: &[u32], adj: &[u64], dist: &mut [D], seen: &mut Vec<usize>, heap: &mut RadixHeap, s: usize, t: usize) -> Option<i64> {
    heap.clear();
    heap.push(0, s as u32);
    dist[s] = D::try_from(1).ok()?;
    seen.push(s);
    let mut found = Some(-1);
    let mut too_wide = false;
    while let Some((d, u)) = heap.pop() {
        let u = u as usize;
        if d + 1 > dist[u].into() {
            continue;
        }
        if u == t {
            found = Some(d as i64);
            break;
        }
        for &e in &adj[off[u] as usize..off[u + 1] as usize] {
            let (v, nd) = ((e >> 32) as usize, d + (e & 0xffff_ffff));
            let Ok(x) = D::try_from(nd + 1) else {
                too_wide = true;
                continue;
            };
            let old: u64 = dist[v].into();
            if old == 0 || nd + 1 < old {
                if old == 0 {
                    seen.push(v);
                }
                dist[v] = x;
                heap.push(nd, v as u32);
            }
        }
    }
    seen.drain(..).for_each(|v| dist[v] = D::default());
    if too_wide && found == Some(-1) { None } else { found }
}

const M_MMAP_THRESHOLD: i32 = -3;

fn main() {
    // A block of 64 KB or more is its own mapping, given back when freed. Otherwise the allocator raises
    // its threshold after the first large free and keeps later large blocks, such as the edge lists.
    unsafe { mallopt(M_MMAP_THRESHOLD, 64 << 10) };
    let g = map("/data/graph.txt");
    let mut i = 0;
    let (n, m) = (num(g, &mut i) as usize, num(g, &mut i) as usize);
    let k = thread::available_parallelism().map_or(1, |x| x.get());
    let parts = cuts(g, i, k);
    // Compressed adjacency: parse each part once into edge triples, counting out-degrees and
    // marking nodes with in-edges, then place every edge from the triples.
    let (deg, has_in): (Vec<AtomicU32>, Vec<AtomicU64>) = (zeroed(n + 1), zeroed(n / 64 + 1));
    let lists: Vec<Vec<[u32; 3]>> = thread::scope(|s| {
        let hs: Vec<_> = parts
            .windows(2)
            .map(|w| {
                let (deg, has_in) = (&deg, &has_in);
                s.spawn(move || {
                    let (mut i, mut list) = (w[0], Vec::with_capacity((w[1] - w[0]) / 16));
                    while i < w[1] {
                        let e = [num(g, &mut i) as u32, num(g, &mut i) as u32, num(g, &mut i) as u32];
                        deg[e[0] as usize + 1].fetch_add(1, Relaxed);
                        has_in[e[1] as usize / 64].fetch_or(1 << (e[1] % 64), Relaxed);
                        list.push(e);
                    }
                    list
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut off: Vec<u32> = deg.into_iter().map(AtomicU32::into_inner).collect();
    for j in 1..=n {
        off[j] += off[j - 1];
    }
    let next: Vec<AtomicU32> = off[..n].iter().map(|&o| AtomicU32::new(o)).collect();
    let adj: Vec<AtomicU64> = zeroed(m);
    thread::scope(|s| {
        for list in lists {
            let (next, adj) = (&next, &adj);
            s.spawn(move || {
                for [u, v, c] in list {
                    adj[next[u as usize].fetch_add(1, Relaxed) as usize].store((v as u64) << 32 | c as u64, Relaxed);
                }
            });
        }
    });
    drop(next);
    let adj: Vec<u64> = adj.into_iter().map(AtomicU64::into_inner).collect();

    // Ids are a random relabelling. Renumber so a search touches nearby memory: k breadth-first
    // searches grow regions in parallel, each claiming unclaimed nodes, and each region gets a
    // contiguous id range. Nodes no region reached go last.
    let owner: Vec<AtomicU32> = zeroed(n); // 0: unclaimed, else region + 1
    let mut regions: Vec<Vec<u32>> = thread::scope(|s| {
        let hs: Vec<_> = (0..k)
            .map(|r| {
                let (off, adj, owner) = (&off, &adj, &owner);
                s.spawn(move || {
                    let claim = |v: usize| owner[v].load(Relaxed) == 0 && owner[v].compare_exchange(0, r as u32 + 1, Relaxed, Relaxed).is_ok();
                    let root = r * n / k;
                    let mut order = if claim(root) { vec![root as u32] } else { Vec::new() };
                    let mut head = 0;
                    while let Some(&u) = order.get(head) {
                        head += 1;
                        for &e in &adj[off[u as usize] as usize..off[u as usize + 1] as usize] {
                            if claim((e >> 32) as usize) {
                                order.push((e >> 32) as u32);
                            }
                        }
                    }
                    order
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    regions.push((0..n as u32).filter(|&v| owner[v as usize].load(Relaxed) == 0).collect());
    drop(owner);

    // Each region, in parallel, numbers its nodes from its base and records their out-degrees in
    // the new order; then every region copies its nodes' edges, renamed, into its own range.
    let (new, deg): (Vec<AtomicU32>, Vec<AtomicU32>) = (zeroed(n), zeroed(n + 1));
    let base: Vec<usize> = regions.iter().scan(0, |at, r| Some((*at, *at += r.len()).0)).collect();
    thread::scope(|s| {
        for (r, &b) in regions.iter().zip(&base) {
            let (off, new, deg) = (&off, &new, &deg);
            s.spawn(move || {
                for (j, &u) in r.iter().enumerate() {
                    new[u as usize].store((b + j) as u32, Relaxed);
                    deg[b + j + 1].store(off[u as usize + 1] - off[u as usize], Relaxed);
                }
            });
        }
    });
    let new: Vec<u32> = new.into_iter().map(AtomicU32::into_inner).collect();
    let mut off2: Vec<u32> = deg.into_iter().map(AtomicU32::into_inner).collect();
    for j in 1..=n {
        off2[j] += off2[j - 1];
    }
    let mut adj2 = vec![0u64; m];
    thread::scope(|s| {
        let mut rest = &mut adj2[..];
        for (r, &b) in regions.iter().zip(&base) {
            let (mine, tail) = std::mem::take(&mut rest).split_at_mut((off2[b + r.len()] - off2[b]) as usize);
            rest = tail;
            let (off, adj, new) = (&off, &adj, &new);
            s.spawn(move || {
                let mut out = mine.iter_mut();
                for &u in r {
                    for &e in &adj[off[u as usize] as usize..off[u as usize + 1] as usize] {
                        *out.next().unwrap() = (new[(e >> 32) as usize] as u64) << 32 | e & 0xffff_ffff;
                    }
                }
            });
        }
    });
    drop((off, adj, regions));
    let (off, adj) = (off2, adj2);

    // Queries sorted by renumbered source, one block per thread, so a thread's searches stay in one
    // part of the graph and touch only that part of its lazily zeroed distance table. A thread that
    // finishes its block helps with the others.
    let q = map("/data/queries.txt");
    let mut i = 0;
    let mut queries = Vec::new();
    while i < q.len() {
        queries.push((num(q, &mut i) as usize, num(q, &mut i) as usize));
    }
    let mut order: Vec<usize> = (0..queries.len()).collect();
    order.sort_unstable_by_key(|&j| new[queries[j].0]);
    let blocks: Vec<&[usize]> = (0..k).map(|b| &order[order.len() * b / k..order.len() * (b + 1) / k]).collect();
    let taken: Vec<AtomicUsize> = (0..k).map(|_| AtomicUsize::new(0)).collect();
    let answers: Vec<AtomicU64> = queries.iter().map(|_| AtomicU64::new(0)).collect();
    let wide = std::sync::Mutex::new(Vec::new());
    thread::scope(|s| {
        for me in 0..k {
            let (blocks, taken, queries, answers, wide, off, adj, new, has_in) = (&blocks, &taken, &queries, &answers, &wide, &off, &adj, &new, &has_in);
            s.spawn(move || {
                // Most answers fit in 16 bits. Search within that exact bound first; queries
                // whose target is not settled there are retried against the shared 64-bit table.
                let (mut dist, mut seen, mut heap) = (vec![0u16; n], Vec::new(), RadixHeap::new());
                for b in (0..k).map(|x| (me + x) % k) {
                    while let Some(&j) = blocks[b].get(taken[b].fetch_add(1, Relaxed)) {
                        let (src, t) = queries[j];
                        let (s, t2) = (new[src] as usize, new[t] as usize);
                        let d = if src == t {
                            0
                        } else if has_in[t / 64].load(Relaxed) >> (t % 64) & 1 == 0 {
                            -1
                        } else if let Some(d) = search(off, adj, &mut dist, &mut seen, &mut heap, s, t2) {
                            d
                        } else {
                            let mut wide = wide.lock().unwrap();
                            if wide.is_empty() {
                                *wide = vec![0u64; n];
                            }
                            search(off, adj, &mut wide, &mut seen, &mut heap, s, t2).unwrap()
                        };
                        answers[j].store(d as u64, Relaxed);
                    }
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
