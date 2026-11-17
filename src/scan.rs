use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use image::ImageFormat;
use rayon::iter::Either;
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::hash::{Algorithm, ImageHash};

/// Default maximum Hamming distance for two images to count as duplicates.
pub const DEFAULT_MAX_DISTANCE: u32 = 8;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub algorithm: Algorithm,
    /// Two images are duplicates when their hashes differ in at most this
    /// many bits (0 = identical hashes only, 64 = everything matches).
    pub max_distance: u32,
    /// Descend into subdirectories.
    pub recursive: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            algorithm: Algorithm::default(),
            max_distance: DEFAULT_MAX_DISTANCE,
            recursive: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedFile {
    pub path: PathBuf,
    pub hash: ImageHash,
}

/// Two or more similar images, sorted by path. Dereferences to a slice of
/// its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group(Vec<HashedFile>);

impl Group {
    /// Each file with the Hamming distance of its hash from the first file's.
    pub fn with_distances(&self) -> impl Iterator<Item = (&HashedFile, u32)> {
        let first = self.0[0].hash;
        self.0.iter().map(move |f| (f, first.distance(&f.hash)))
    }
}

impl Deref for Group {
    type Target = [HashedFile];

    fn deref(&self) -> &[HashedFile] {
        &self.0
    }
}

#[derive(Debug)]
pub struct ScanReport {
    /// Number of image files found, whether or not they could be hashed.
    pub discovered: usize,
    /// Number of images successfully hashed.
    pub hashed: usize,
    /// Groups of similar images, sorted by path.
    ///
    /// Groups are transitive: if A matches B and B matches C, all three share
    /// a group even when A and C are further apart than the threshold.
    pub groups: Vec<Group>,
    /// Files and directory entries that could not be read. These never abort
    /// a scan.
    pub failed: Vec<Error>,
    /// Number of hash pairs compared while grouping.
    pub pairs_compared: u64,
    /// Worker threads used for hashing.
    pub threads: usize,
    /// Wall-clock time spent in each phase of the scan.
    pub timings: ScanTimings,
}

impl ScanReport {
    /// Number of files that duplicate another: every group member but one.
    pub fn redundant(&self) -> usize {
        self.groups.iter().map(|g| g.len() - 1).sum()
    }

    /// Image files processed per second of hashing, including ones that
    /// failed to decode.
    pub fn images_per_sec(&self) -> f64 {
        let secs = self.timings.hash.as_secs_f64();
        if secs > 0.0 {
            self.discovered as f64 / secs
        } else {
            0.0
        }
    }
}

/// Wall-clock time spent in each phase of a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanTimings {
    /// Walking the directory to find image files.
    pub discover: Duration,
    /// Decoding and hashing the images, in parallel.
    pub hash: Duration,
    /// Comparing all pairs of hashes and grouping the matches.
    pub compare: Duration,
}

impl ScanTimings {
    pub fn total(&self) -> Duration {
        self.discover + self.hash + self.compare
    }
}

/// Hashes every image in `dir` and groups those within
/// `options.max_distance` of each other.
pub fn scan(dir: &Path, options: &ScanOptions) -> Result<ScanReport> {
    scan_with_progress(dir, options, |_, _| {})
}

/// Like [`scan`], reporting hashing progress through `on_progress(done, total)`.
///
/// The callback is first called with `(0, total)` once the files to hash are
/// known, then once per processed file (hashed or failed). It is called from
/// worker threads, so calls may arrive out of order.
pub fn scan_with_progress(
    dir: &Path,
    options: &ScanOptions,
    on_progress: impl Fn(usize, usize) + Sync,
) -> Result<ScanReport> {
    if !dir.is_dir() {
        return Err(Error::NotADirectory(dir.to_path_buf()));
    }

    let ((paths, mut failed), discover) = timed(|| discover(dir, options.recursive));
    let discovered = paths.len();
    let ((files, undecodable), hash) = timed(|| hash_all(paths, options.algorithm, &on_progress));
    failed.extend(undecodable);
    let hashed = files.len();
    let ((groups, pairs_compared), compare) = timed(|| group_similar(files, options.max_distance));

    Ok(ScanReport {
        discovered,
        hashed,
        groups,
        failed,
        pairs_compared,
        threads: rayon::current_num_threads(),
        timings: ScanTimings {
            discover,
            hash,
            compare,
        },
    })
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let out = f();
    (out, started.elapsed())
}

/// Image files under `dir`, sorted by path, and the entries that could not
/// be visited.
fn discover(dir: &Path, recursive: bool) -> (Vec<PathBuf>, Vec<Error>) {
    let mut paths = Vec::new();
    let mut failed = Vec::new();
    let max_depth = if recursive { usize::MAX } else { 1 };
    for entry in WalkDir::new(dir).max_depth(max_depth) {
        match entry {
            Ok(entry) if entry.file_type().is_file() && is_image(entry.path()) => {
                paths.push(entry.into_path());
            }
            Ok(_) => {}
            Err(e) => failed.push(Error::Walk(e)),
        }
    }
    paths.sort();
    (paths, failed)
}

/// Hashes `paths` in parallel, keeping both outputs in input order.
fn hash_all(
    paths: Vec<PathBuf>,
    algorithm: Algorithm,
    on_progress: &(impl Fn(usize, usize) + Sync),
) -> (Vec<HashedFile>, Vec<Error>) {
    let total = paths.len();
    let done = AtomicUsize::new(0);
    on_progress(0, total);
    paths.into_par_iter().partition_map(|path| {
        let hash = ImageHash::from_path(&path, algorithm);
        on_progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
        match hash {
            Ok(hash) => Either::Left(HashedFile { path, hash }),
            Err(e) => Either::Right(e),
        }
    })
}

/// Whether the extension of `path` names a format this build can decode.
fn is_image(path: &Path) -> bool {
    ImageFormat::from_path(path).is_ok_and(|format| format.reading_enabled())
}

/// Groups `files` (sorted by path) into connected components of the
/// "within `max_distance`" relation, dropping singletons. Also returns the
/// number of pairs compared.
fn group_similar(files: Vec<HashedFile>, max_distance: u32) -> (Vec<Group>, u64) {
    let mut parent: Vec<usize> = (0..files.len()).collect();

    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }

    for i in 0..files.len() {
        for j in i + 1..files.len() {
            if files[i].hash.distance(&files[j].hash) <= max_distance {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                // Keep the lowest index as root so groups stay in path order.
                parent[a.max(b)] = a.min(b);
            }
        }
    }

    let mut groups: Vec<Vec<HashedFile>> = (0..files.len()).map(|_| Vec::new()).collect();
    for (i, file) in files.into_iter().enumerate() {
        let root = find(&mut parent, i);
        groups[root].push(file);
    }
    let groups = groups
        .into_iter()
        .filter(|g| g.len() > 1)
        .map(Group)
        .collect();
    let n = parent.len() as u64;
    (groups, n * n.saturating_sub(1) / 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, bits: u64) -> HashedFile {
        HashedFile {
            path: PathBuf::from(name),
            hash: ImageHash::from_bits(bits),
        }
    }

    #[test]
    fn groups_are_transitive_and_drop_singletons() {
        let files = vec![
            file("a", 0b0000),
            file("b", 0b1111_0000_0000),
            file("c", 0b0001),
            file("d", 0b0011),
            file("e", u64::MAX),
        ];
        let (groups, pairs) = group_similar(files, 1);
        assert_eq!(pairs, 10);
        let names: Vec<Vec<&str>> = groups
            .iter()
            .map(|g| g.iter().map(|f| f.path.to_str().unwrap()).collect())
            .collect();
        assert_eq!(names, vec![vec!["a", "c", "d"]]);
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert!(is_image(Path::new("x/photo.JPG")));
        assert!(is_image(Path::new("photo.webp")));
        assert!(!is_image(Path::new("notes.txt")));
        assert!(!is_image(Path::new("png")));
        // Known to the image crate, but its decoder is not compiled in.
        assert!(!is_image(Path::new("icon.ico")));
    }
}
