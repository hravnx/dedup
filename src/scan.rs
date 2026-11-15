use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::hash::{Algorithm, ImageHash};

/// File extensions (lowercase) treated as images by [`scan`].
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp"];

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

#[derive(Debug)]
pub struct ScanReport {
    /// Number of images successfully hashed.
    pub hashed: usize,
    /// Groups of two or more similar images. Files within a group and the
    /// groups themselves are sorted by path.
    ///
    /// Groups are transitive: if A matches B and B matches C, all three share
    /// a group even when A and C are further apart than the threshold.
    pub groups: Vec<Vec<HashedFile>>,
    /// Files and directory entries that could not be read. These never abort
    /// a scan.
    pub failed: Vec<Error>,
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

    let mut failed = Vec::new();
    let mut paths = Vec::new();
    let max_depth = if options.recursive { usize::MAX } else { 1 };
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

    let total = paths.len();
    let done = AtomicUsize::new(0);
    on_progress(0, total);
    let results: Vec<Result<HashedFile>> = paths
        .into_par_iter()
        .map(|path| {
            let hash = ImageHash::from_path(&path, options.algorithm);
            on_progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
            Ok(HashedFile { path, hash: hash? })
        })
        .collect();

    let mut files = Vec::new();
    for result in results {
        match result {
            Ok(file) => files.push(file),
            Err(e) => failed.push(e),
        }
    }

    Ok(ScanReport {
        hashed: files.len(),
        groups: group_similar(files, options.max_distance),
        failed,
    })
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Groups `files` (sorted by path) into connected components of the
/// "within `max_distance`" relation, dropping singletons.
fn group_similar(files: Vec<HashedFile>, max_distance: u32) -> Vec<Vec<HashedFile>> {
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
    groups.retain(|g| g.len() > 1);
    groups
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
        let groups = group_similar(files, 1);
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
    }
}
