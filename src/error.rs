use std::path::{Path, PathBuf};

/// Errors produced while hashing or scanning images.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file could not be read or decoded as an image.
    #[error("cannot read image {}: {source}", path.display())]
    Image {
        path: PathBuf,
        #[source]
        source: image::ImageError,
    },

    /// The scan root does not exist or is not a directory.
    #[error("not a directory: {}", .0.display())]
    NotADirectory(PathBuf),

    /// A directory entry could not be visited during a scan.
    #[error("cannot walk directory: {0}")]
    Walk(#[from] walkdir::Error),
}

impl Error {
    /// The path the error relates to, when known.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Error::Image { path, .. } => Some(path),
            Error::NotADirectory(path) => Some(path),
            Error::Walk(e) => e.path(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
