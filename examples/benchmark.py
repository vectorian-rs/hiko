
"""benchmark.py — Walk directory recursively, hash every file."""
import os
import time

try:
    import blake3 as _blake3
    def hash_fn(data): return _blake3.blake3(data).hexdigest()
except ImportError:
    import hashlib
    def hash_fn(data): return hashlib.blake2b(data, digest_size=32).hexdigest()

SKIP = set()
TARGET_DIR = "/Users/l1x/code/home/vectorian-rs/hiko/.git"

t0 = time.monotonic()
results = []
count = 0

for root, dirs, files in os.walk(TARGET_DIR):
    dirs[:] = [d for d in dirs if d not in SKIP]
    for name in files:
        path = os.path.join(root, name)
        try:
            with open(path, "rb") as f:
                data = f.read()
            results.append(f"{hash_fn(data)}  {len(data)}  {path}")
            count += 1
        except (PermissionError, OSError):
            pass

elapsed = time.monotonic() - t0
for line in results:
    print(line)
print(f"  {count} files, {elapsed*1000:.0f} ms")
