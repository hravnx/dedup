use std::f64::consts::PI;
use std::fmt;
use std::path::Path;

use image::DynamicImage;
use image::imageops::{self, FilterType};

use crate::error::{Error, Result};

/// Number of bits in every hash.
pub const HASH_BITS: u32 = 64;

/// Perceptual hashing algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Algorithm {
    /// aHash: each bit says whether a pixel of the 8x8 thumbnail is brighter
    /// than the mean. Fastest, most prone to false positives.
    Average,
    /// dHash: each bit says whether a pixel of the 9x8 thumbnail is brighter
    /// than its right neighbour. Tracks gradients, robust to brightness shifts.
    Difference,
    /// pHash: compares the low-frequency DCT coefficients of a 32x32 thumbnail
    /// against their median. Slowest, most robust to scaling and compression.
    #[default]
    Perceptual,
}

impl Algorithm {
    /// Every algorithm, fastest first.
    pub const ALL: [Algorithm; 3] = [
        Algorithm::Average,
        Algorithm::Difference,
        Algorithm::Perceptual,
    ];
}

/// A 64-bit perceptual hash. Hashes are only comparable when produced by the
/// same [`Algorithm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageHash(u64);

impl ImageHash {
    /// Hashes an already decoded image.
    pub fn from_image(img: &DynamicImage, algorithm: Algorithm) -> Self {
        ImageHash(match algorithm {
            Algorithm::Average => average_hash(img),
            Algorithm::Difference => difference_hash(img),
            Algorithm::Perceptual => perceptual_hash(img),
        })
    }

    /// Decodes the image at `path` and hashes it.
    pub fn from_path(path: &Path, algorithm: Algorithm) -> Result<Self> {
        let img = image::open(path).map_err(|source| Error::Image {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self::from_image(&img, algorithm))
    }

    pub fn from_bits(bits: u64) -> Self {
        ImageHash(bits)
    }

    pub fn bits(&self) -> u64 {
        self.0
    }

    /// Hamming distance: the number of differing bits, 0 (identical) to 64.
    pub fn distance(&self, other: &Self) -> u32 {
        (self.0 ^ other.0).count_ones()
    }

    /// Similarity in `0.0..=1.0`, where 1.0 means identical hashes.
    pub fn similarity(&self, other: &Self) -> f64 {
        1.0 - f64::from(self.distance(other)) / f64::from(HASH_BITS)
    }
}

impl fmt::Display for ImageHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// Similarity in `0.0..=1.0` of two image files under `algorithm`.
pub fn similarity(a: &Path, b: &Path, algorithm: Algorithm) -> Result<f64> {
    let a = ImageHash::from_path(a, algorithm)?;
    let b = ImageHash::from_path(b, algorithm)?;
    Ok(a.similarity(&b))
}

/// Grayscale thumbnail as row-major luma values.
fn thumbnail(img: &DynamicImage, width: u32, height: u32) -> Vec<f64> {
    let gray = img.to_luma8();
    imageops::resize(&gray, width, height, FilterType::Triangle)
        .into_raw()
        .into_iter()
        .map(f64::from)
        .collect()
}

fn pack_bits(bits: impl Iterator<Item = bool>) -> u64 {
    bits.fold(0, |acc, bit| (acc << 1) | u64::from(bit))
}

fn average_hash(img: &DynamicImage) -> u64 {
    let px = thumbnail(img, 8, 8);
    let mean = px.iter().sum::<f64>() / px.len() as f64;
    pack_bits(px.iter().map(|&p| p > mean))
}

fn difference_hash(img: &DynamicImage) -> u64 {
    let px = thumbnail(img, 9, 8);
    pack_bits(
        px.as_chunks::<9>()
            .0
            .iter()
            .flat_map(|row| row.windows(2).map(|w| w[0] > w[1])),
    )
}

const DCT_SIZE: usize = 32;
const DCT_KEEP: usize = 8;

fn perceptual_hash(img: &DynamicImage) -> u64 {
    let px = thumbnail(img, DCT_SIZE as u32, DCT_SIZE as u32);

    // Separable 2D DCT, keeping only the lowest DCT_KEEP frequencies per axis.
    let mut rows = [[0.0; DCT_KEEP]; DCT_SIZE];
    for (y, row) in px.as_chunks::<DCT_SIZE>().0.iter().enumerate() {
        rows[y] = dct_low(row);
    }
    let mut coeffs = [0.0; DCT_KEEP * DCT_KEEP];
    for u in 0..DCT_KEEP {
        let column: [f64; DCT_SIZE] = std::array::from_fn(|y| rows[y][u]);
        for (v, c) in dct_low(&column).into_iter().enumerate() {
            coeffs[v * DCT_KEEP + u] = c;
        }
    }

    // The DC term is the average brightness and dwarfs everything else, so it
    // is left out of the median.
    let mut ac = coeffs[1..].to_vec();
    ac.sort_by(f64::total_cmp);
    let median = ac[ac.len() / 2];
    pack_bits(coeffs.iter().map(|&c| c > median))
}

/// First `DCT_KEEP` coefficients of the (unnormalised) DCT-II of `input`.
fn dct_low(input: &[f64]) -> [f64; DCT_KEEP] {
    let n = input.len() as f64;
    std::array::from_fn(|k| {
        input
            .iter()
            .enumerate()
            .map(|(i, &x)| x * (PI * (2.0 * i as f64 + 1.0) * k as f64 / (2.0 * n)).cos())
            .sum()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn dct_of_constant_has_only_dc() {
        let out = dct_low(&[2.0; 32]);
        assert!((out[0] - 64.0).abs() < 1e-9);
        assert!(out[1..].iter().all(|c| c.abs() < 1e-9));
    }

    #[test]
    fn dct_of_cosine_peaks_at_its_frequency() {
        let input: Vec<f64> = (0..32)
            .map(|i| (PI * (2.0 * i as f64 + 1.0) * 3.0 / 64.0).cos())
            .collect();
        let out = dct_low(&input);
        assert!((out[3] - 16.0).abs() < 1e-9);
        assert!(
            out.iter()
                .enumerate()
                .all(|(k, c)| k == 3 || c.abs() < 1e-9)
        );
    }

    #[test]
    fn distance_and_similarity() {
        let a = ImageHash::from_bits(0);
        let b = ImageHash::from_bits(0xffff);
        assert_eq!(a.distance(&a), 0);
        assert_eq!(a.distance(&b), 16);
        assert_eq!(a.similarity(&a), 1.0);
        assert_eq!(a.similarity(&b), 0.75);
        assert_eq!(a.similarity(&ImageHash::from_bits(u64::MAX)), 0.0);
    }

    #[test]
    fn display_is_16_hex_digits() {
        assert_eq!(ImageHash::from_bits(0xabc).to_string(), "0000000000000abc");
    }

    #[test]
    fn same_image_hashes_identically() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 48, |x, y| {
            Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8])
        }));
        for algo in Algorithm::ALL {
            let a = ImageHash::from_image(&img, algo);
            let b = ImageHash::from_image(&img.clone(), algo);
            assert_eq!(a.distance(&b), 0, "{algo:?}");
        }
    }
}
