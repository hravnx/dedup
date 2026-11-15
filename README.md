# dedup

Find near-duplicate images with perceptual hashing. A Rust library with a CLI on top.

## CLI

```
dedup scan <DIR> [-a ahash|dhash|phash] [-t <max-distance>] [-r] [--json]
dedup compare <A> <B> [-a ahash|dhash|phash]
```

`scan` hashes every image in a directory (`-r` to recurse) and prints groups of files whose
hashes differ in at most `-t` bits out of 64 (default 8). Lower is stricter; 0 matches identical
hashes only. Each line shows the hash, the distance to the first file of the group, and the path.
Unreadable files are reported on stderr and skipped. Nothing is ever deleted or moved.

`compare` prints the hashes, bit distance and similarity percentage of two files.

Supported formats: JPEG, PNG, GIF, BMP, TIFF, WebP.

## Algorithms

| Flag    | Method                                                        | Notes                         |
| ------- | ------------------------------------------------------------- | ----------------------------- |
| `ahash` | 8x8 thumbnail, pixel brighter than the mean                   | Fastest, most false positives |
| `dhash` | 9x8 thumbnail, pixel brighter than its right neighbour        | Robust to brightness changes  |
| `phash` | 32x32 thumbnail, low-frequency DCT coefficients vs the median | Default, most robust          |

All produce 64-bit hashes compared by Hamming distance. Groups are transitive: if A matches B and
B matches C, all three are reported together.

## Library

```rust
use std::path::Path;
use dedup::{Algorithm, ImageHash, ScanOptions};

let score = dedup::similarity(Path::new("a.jpg"), Path::new("b.jpg"), Algorithm::Perceptual)?;

let a = ImageHash::from_path(Path::new("a.jpg"), Algorithm::Difference)?;
let b = ImageHash::from_path(Path::new("b.jpg"), Algorithm::Difference)?;
let bits = a.distance(&b);

let report = dedup::scan(Path::new("photos"), &ScanOptions::default())?;
```

Depend on it without the CLI dependencies using `default-features = false`.
