//! Cost of reading every file under a directory: `std::fs::read`, a cap-std
//! open, and (on macOS) an `openat` with `O_RESOLVE_BENEATH`, which is what
//! the filesystem builtins use for reads.
//!
//! Usage: cargo run --release -p hiko-vm --example fs_open_cost -- DIR
//! DIR is relative to the current directory, which is the sandbox root.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            walk(&entry.path(), out);
        } else if file_type.is_file() {
            out.push(entry.path());
        }
    }
}

fn time(label: &str, files: usize, f: impl FnOnce() -> usize) -> usize {
    let start = Instant::now();
    let bytes = f();
    println!("{label:32} {:>8.1?}  ({files} files)", start.elapsed());
    bytes
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: fs_open_cost DIR");
    let root = std::fs::canonicalize(".").unwrap();
    let mut files = Vec::new();
    walk(&root.join(&dir), &mut files);
    let relative: Vec<PathBuf> = files
        .iter()
        .map(|f| f.strip_prefix(&root).unwrap().to_path_buf())
        .collect();
    let cap = cap_std::fs::Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
    let n = files.len();

    for round in 0..3 {
        println!("round {round}");
        let expected = time("std::fs::read", n, || {
            files.iter().map(|f| std::fs::read(f).unwrap().len()).sum()
        });
        let cap_bytes = time("cap-std open + read", n, || {
            relative
                .iter()
                .map(|f| {
                    let mut buf = Vec::new();
                    cap.open(f).unwrap().read_to_end(&mut buf).unwrap();
                    buf.len()
                })
                .sum()
        });
        assert_eq!(cap_bytes, expected);

        #[cfg(target_os = "macos")]
        {
            use rustix::fs::{Mode, OFlags, openat};
            use std::os::fd::AsFd;
            let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::from_bits_retain(0x1000);
            let beneath_bytes = time("openat O_RESOLVE_BENEATH + read", n, || {
                relative
                    .iter()
                    .map(|f| {
                        let fd = openat(cap.as_fd(), f, flags, Mode::empty()).unwrap();
                        let mut buf = Vec::new();
                        std::fs::File::from(fd).read_to_end(&mut buf).unwrap();
                        buf.len()
                    })
                    .sum()
            });
            assert_eq!(beneath_bytes, expected);
        }
    }
}
