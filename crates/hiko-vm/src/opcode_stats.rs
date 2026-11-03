//! Counts of executed opcodes and opcode pairs, built only with the
//! `opcode-stats` feature (#152). Set `HIKO_OPCODE_STATS=<path>`: each VM
//! writes the running totals of the whole process there when it is dropped.
//! Writes are serialized, each takes its snapshot inside the lock and
//! replaces the file atomically, so the file holds the totals once the last
//! VM is gone. Every drop rescans the table, which is fine for a
//! measurement build.
//!
//! A pair is two instructions executed one after the other, in any frame: a
//! call's last instruction and the callee's first count as a pair.

use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use hiko_compile::op::Op;

/// `PAIRS[prev << 8 | op]`. `prev` is `START` before a VM's first
/// instruction.
static PAIRS: [AtomicU64; 1 << 16] = [const { AtomicU64::new(0) }; 1 << 16];

pub(crate) const START: u8 = u8::MAX;

/// Held while a VM takes its snapshot and writes the file.
static DUMP: Mutex<()> = Mutex::new(());

#[inline]
pub(crate) fn record(prev: u8, op: u8) {
    PAIRS[(prev as usize) << 8 | op as usize].fetch_add(1, Ordering::Relaxed);
}

fn name(byte: u8) -> String {
    if byte == START {
        return "<start>".into();
    }
    Op::try_from(byte).map_or_else(|b| format!("<{b}>"), |op| format!("{op:?}"))
}

/// Write `opcode` and `pair` lines, most frequent first:
/// `opcode <count> <name>` and `pair <count> <first> <second>`.
pub(crate) fn dump() {
    let Ok(path) = std::env::var("HIKO_OPCODE_STATS") else {
        return;
    };
    let _guard = DUMP.lock().unwrap_or_else(|e| e.into_inner());
    let mut ops = [0u64; 256];
    let mut pairs = Vec::new();
    for (i, count) in PAIRS.iter().enumerate() {
        let count = count.load(Ordering::Relaxed);
        if count == 0 {
            continue;
        }
        let (prev, op) = ((i >> 8) as u8, (i & 0xff) as u8);
        ops[op as usize] += count;
        if prev != START {
            pairs.push((count, prev, op));
        }
    }
    let mut out = String::new();
    let mut singles: Vec<_> = (0..=255u8).filter(|&b| ops[b as usize] > 0).collect();
    singles.sort_by_key(|&b| std::cmp::Reverse(ops[b as usize]));
    for b in singles {
        writeln!(out, "opcode {} {}", ops[b as usize], name(b)).unwrap();
    }
    pairs.sort_by_key(|&(count, _, _)| std::cmp::Reverse(count));
    for (count, prev, op) in pairs {
        writeln!(out, "pair {count} {} {}", name(prev), name(op)).unwrap();
    }
    let tmp = format!("{path}.tmp");
    if let Err(e) = std::fs::write(&tmp, out).and_then(|()| std::fs::rename(&tmp, &path)) {
        eprintln!("opcode-stats: cannot write {path}: {e}");
    }
}
