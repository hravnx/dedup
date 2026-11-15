//! Perceptual image hashing and near-duplicate detection.
//!
//! ```no_run
//! use std::path::Path;
//! use dedup::{Algorithm, ScanOptions};
//!
//! let score = dedup::similarity(Path::new("a.jpg"), Path::new("b.jpg"), Algorithm::Perceptual)?;
//! println!("{:.0}% similar", score * 100.0);
//!
//! let report = dedup::scan(Path::new("photos"), &ScanOptions::default())?;
//! for group in &report.groups {
//!     println!("{} similar files", group.len());
//! }
//! # Ok::<(), dedup::Error>(())
//! ```

mod error;
mod hash;
mod scan;

pub use error::{Error, Result};
pub use hash::{Algorithm, HASH_BITS, ImageHash, similarity};
pub use scan::{DEFAULT_MAX_DISTANCE, HashedFile, IMAGE_EXTENSIONS, ScanOptions, ScanReport, scan};
