//! Just enough of LevelDB to read a Chromium "Local Storage" folder (`leveldb\*.ldb` tables and `*.log` journals, snappy-compressed blocks):
//! the latest value of each key, deleted keys left out. Read-only; the files are only opened for reading, so a running browser is not disturbed.

use std::collections::HashMap;
use std::path::Path;

fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
    let mut r = 0u64;
    let mut s = 0;
    loop {
        let c = *b.get(*i)?;
        *i += 1;
        r |= ((c & 0x7f) as u64) << s;
        if c < 0x80 {
            return Some(r);
        }
        s += 7;
        if s > 63 {
            return None;
        }
    }
}

/// Raw snappy (the block format, no framing).
pub fn snappy(b: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0;
    let n = varint(b, &mut i)? as usize;
    let mut out: Vec<u8> = Vec::with_capacity(n.min(64 << 20));
    while i < b.len() {
        let t = b[i];
        i += 1;
        let (len, off) = match t & 3 {
            0 => {
                let mut len = (t >> 2) as usize;
                if len >= 60 {
                    let nb = len - 59;
                    len = b.get(i..i + nb)?.iter().rev().fold(0usize, |a, &x| (a << 8) | x as usize);
                    i += nb;
                }
                len += 1;
                out.extend_from_slice(b.get(i..i + len)?);
                i += len;
                continue;
            }
            1 => {
                let off = (((t >> 5) as usize) << 8) | *b.get(i)? as usize;
                i += 1;
                (((t >> 2) & 7) as usize + 4, off)
            }
            2 => {
                let off = u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize;
                i += 2;
                ((t >> 2) as usize + 1, off)
            }
            _ => {
                let off = u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize;
                i += 4;
                ((t >> 2) as usize + 1, off)
            }
        };
        if off == 0 || off > out.len() {
            return None;
        }
        for _ in 0..len {
            out.push(out[out.len() - off]);
        }
    }
    (out.len() == n).then_some(out)
}

fn read_block(data: &[u8], handle: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0;
    let off = varint(handle, &mut i)? as usize;
    let size = varint(handle, &mut i)? as usize;
    let raw = data.get(off..off + size)?;
    match *data.get(off + size)? {
        0 => Some(raw.to_vec()),
        1 => snappy(raw),
        _ => None,
    }
}

/// The (key, value) entries of a table block (keys are prefix-compressed).
fn block_entries(b: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    if b.len() < 4 {
        return out;
    }
    let restarts = u32::from_le_bytes(b[b.len() - 4..].try_into().unwrap()) as usize;
    let Some(end) = b.len().checked_sub(4 + 4 * restarts) else { return out };
    let mut i = 0;
    let mut key: Vec<u8> = Vec::new();
    while i < end {
        let (Some(shared), Some(unshared), Some(vlen)) = (varint(b, &mut i), varint(b, &mut i), varint(b, &mut i)) else { break };
        let (shared, unshared, vlen) = (shared as usize, unshared as usize, vlen as usize);
        let (Some(k), Some(v)) = (b.get(i..i + unshared), b.get(i + unshared..i + unshared + vlen)) else { break };
        key.truncate(shared);
        key.extend_from_slice(k);
        out.push((key.clone(), v.to_vec()));
        i += unshared + vlen;
    }
    out
}

/// (user key, sequence, value or None when deleted)
type Entry = (Vec<u8>, u64, Option<Vec<u8>>);

fn table(data: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    if data.len() < 48 {
        return out;
    }
    let foot = &data[data.len() - 48..];
    let mut i = 0;
    // the metaindex handle, then the index handle
    if varint(foot, &mut i).and(varint(foot, &mut i)).is_none() {
        return out;
    }
    let Some(index) = read_block(data, &foot[i..]) else { return out };
    for (_, handle) in block_entries(&index) {
        let Some(blk) = read_block(data, &handle) else { continue };
        for (k, v) in block_entries(&blk) {
            if k.len() < 8 {
                continue;
            }
            let tag = u64::from_le_bytes(k[k.len() - 8..].try_into().unwrap());
            let user = k[..k.len() - 8].to_vec();
            out.push((user, tag >> 8, if tag & 0xff == 1 { Some(v) } else { None }));
        }
    }
    out
}

fn journal(data: &[u8]) -> Vec<Entry> {
    // records in 32 KiB blocks, each with a 7-byte header; a batch may be split over several (first / middle / last)
    let mut batches: Vec<Vec<u8>> = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut i = 0;
    while i + 7 <= data.len() {
        let left = 32768 - i % 32768;
        if left < 7 {
            i += left;
            continue;
        }
        let len = u16::from_le_bytes([data[i + 4], data[i + 5]]) as usize;
        let kind = data[i + 6];
        let Some(frag) = data.get(i + 7..i + 7 + len) else { break };
        i += 7 + len;
        match kind {
            1 => batches.push(frag.to_vec()),
            2 => pending = frag.to_vec(),
            3 => pending.extend_from_slice(frag),
            4 => {
                pending.extend_from_slice(frag);
                batches.push(std::mem::take(&mut pending));
            }
            _ => {
                // zero padding: skip to the next block
                i += (32768 - i % 32768) % 32768;
            }
        }
    }
    let mut out = Vec::new();
    for r in batches {
        if r.len() < 12 {
            continue;
        }
        let seq = u64::from_le_bytes(r[..8].try_into().unwrap());
        let count = u32::from_le_bytes(r[8..12].try_into().unwrap()) as u64;
        let mut j = 12;
        for n in 0..count {
            let Some(&kind) = r.get(j) else { break };
            j += 1;
            let Some(kl) = varint(&r, &mut j) else { break };
            let Some(k) = r.get(j..j + kl as usize) else { break };
            j += kl as usize;
            let v = if kind == 1 {
                let Some(vl) = varint(&r, &mut j) else { break };
                let Some(v) = r.get(j..j + vl as usize) else { break };
                j += vl as usize;
                Some(v.to_vec())
            } else {
                None
            };
            out.push((k.to_vec(), seq + n, v));
        }
    }
    out
}

/// The live value of every key whose bytes contain `needle` (all keys when empty).
pub fn read(dir: &Path, needle: &[u8]) -> HashMap<Vec<u8>, Vec<u8>> {
    let mut best: HashMap<Vec<u8>, (u64, Option<Vec<u8>>)> = HashMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return HashMap::new() };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        let parse: fn(&[u8]) -> Vec<Entry> = if name.ends_with(".ldb") || name.ends_with(".sst") {
            table
        } else if name.ends_with(".log") {
            journal
        } else {
            continue;
        };
        let Ok(data) = std::fs::read(e.path()) else { continue };
        for (k, seq, v) in parse(&data) {
            if !needle.is_empty() && !k.windows(needle.len()).any(|w| w == needle) {
                continue;
            }
            if best.get(&k).map_or(true, |b| b.0 < seq) {
                best.insert(k, (seq, v));
            }
        }
    }
    best.into_iter().filter_map(|(k, (_, v))| v.map(|v| (k, v))).collect()
}

/// A Chromium Local Storage value as text (a leading 0 means UTF-16LE, 1 Latin-1).
pub fn chromium_string(v: &[u8]) -> Option<String> {
    match v.first()? {
        0 => {
            let u: Vec<u16> = v[1..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16(&u).ok()
        }
        1 => Some(v[1..].iter().map(|&b| b as char).collect()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snappy_literals_and_copies() {
        // "abcabcabcd": a literal "abc", a copy of 6 from 3 back, a literal "d"
        let b = [10u8, 2 << 2, b'a', b'b', b'c', ((6 - 4) << 2) | 1, 3, 0, b'd'];
        assert_eq!(snappy(&b).unwrap(), b"abcabcabcd");
        assert!(snappy(&[5u8, 0, b'x']).is_none(), "the length must match");
    }

    #[test]
    fn journal_keeps_the_latest_value() {
        fn batch(seq: u64, ops: &[(u8, &[u8], &[u8])]) -> Vec<u8> {
            let mut r = seq.to_le_bytes().to_vec();
            r.extend((ops.len() as u32).to_le_bytes());
            for (kind, k, v) in ops {
                r.push(*kind);
                r.push(k.len() as u8);
                r.extend_from_slice(k);
                if *kind == 1 {
                    r.push(v.len() as u8);
                    r.extend_from_slice(v);
                }
            }
            let mut rec = vec![0, 0, 0, 0];
            rec.extend((r.len() as u16).to_le_bytes());
            rec.push(1);
            rec.extend(r);
            rec
        }
        let dir = std::env::temp_dir().join(format!("stl-test-ldb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut log = batch(1, &[(1, b"k1", b"old"), (1, b"k2", b"gone")]);
        log.extend(batch(3, &[(1, b"k1", b"new"), (0, b"k2", b"")]));
        std::fs::write(dir.join("000001.log"), log).unwrap();
        let m = read(&dir, b"");
        assert_eq!(m.get(&b"k1"[..]).map(|v| v.as_slice()), Some(&b"new"[..]));
        assert!(!m.contains_key(&b"k2"[..]));
        assert_eq!(chromium_string(&[1, b'h', b'i']).as_deref(), Some("hi"));
        assert_eq!(chromium_string(&[0, b'h', 0, b'i', 0]).as_deref(), Some("hi"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
