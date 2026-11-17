use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use dedup::{Algorithm, DEFAULT_MAX_DISTANCE, ImageHash, ScanOptions};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, Rgb, RgbImage};

const ALL: [Algorithm; 3] = [
    Algorithm::Average,
    Algorithm::Difference,
    Algorithm::Perceptual,
];

/// A bright blob on a diagonal gradient.
fn base_image() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(320, 240, |x, y| {
        let (dx, dy) = (x as f64 - 110.0, y as f64 - 150.0);
        let blob = 140.0 * (-(dx * dx + dy * dy) / 5000.0).exp();
        let gradient = (x as f64 / 320.0 + y as f64 / 240.0) * 40.0;
        let v = (20.0 + blob + gradient) as u8;
        Rgb([v, (v as f64 * 0.8) as u8, 255 - v])
    }))
}

/// Vertical stripes with a dark corner: nothing like `base_image`.
fn other_image() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(320, 240, |x, y| {
        let stripe = if (x / 40) % 2 == 0 { 200 } else { 60 };
        let v = if x > 200 && y < 80 { 10 } else { stripe };
        Rgb([v, v, v])
    }))
}

fn save_jpeg(img: &DynamicImage, path: &Path, quality: u8) {
    let writer = BufWriter::new(File::create(path).unwrap());
    img.write_with_encoder(JpegEncoder::new_with_quality(writer, quality))
        .unwrap();
}

/// Writes the base image, three altered copies, an unrelated image and a
/// corrupt file into `dir`.
fn write_fixtures(dir: &Path) {
    let base = base_image();
    base.save(dir.join("base.png")).unwrap();
    base.resize_exact(128, 96, FilterType::Lanczos3)
        .save(dir.join("small.png"))
        .unwrap();
    base.brighten(25).save(dir.join("bright.png")).unwrap();
    save_jpeg(&base, &dir.join("compressed.jpg"), 40);
    other_image().save(dir.join("other.png")).unwrap();
    fs::write(dir.join("corrupt.png"), b"not an image").unwrap();
    fs::write(dir.join("notes.txt"), b"ignored").unwrap();
}

#[test]
fn altered_copies_are_similar_and_unrelated_images_are_not() {
    let dir = tempfile::tempdir().unwrap();
    write_fixtures(dir.path());

    for algo in ALL {
        let base = ImageHash::from_path(&dir.path().join("base.png"), algo).unwrap();
        for name in ["small.png", "bright.png", "compressed.jpg"] {
            let variant = ImageHash::from_path(&dir.path().join(name), algo).unwrap();
            let d = base.distance(&variant);
            assert!(d <= DEFAULT_MAX_DISTANCE, "{algo:?} {name}: distance {d}");
        }
        let other = ImageHash::from_path(&dir.path().join("other.png"), algo).unwrap();
        let d = base.distance(&other);
        assert!(d > DEFAULT_MAX_DISTANCE, "{algo:?} other: distance {d}");
    }
}

#[test]
fn similarity_of_files() {
    let dir = tempfile::tempdir().unwrap();
    write_fixtures(dir.path());
    let p = |name: &str| dir.path().join(name);

    let same = dedup::similarity(&p("base.png"), &p("base.png"), Algorithm::Perceptual).unwrap();
    assert_eq!(same, 1.0);
    let close = dedup::similarity(&p("base.png"), &p("small.png"), Algorithm::Perceptual).unwrap();
    let far = dedup::similarity(&p("base.png"), &p("other.png"), Algorithm::Perceptual).unwrap();
    assert!(close > far);

    let err = dedup::similarity(&p("base.png"), &p("missing.png"), Algorithm::Perceptual);
    assert!(err.is_err());
}

#[test]
fn scan_groups_duplicates_and_reports_failures() {
    let dir = tempfile::tempdir().unwrap();
    write_fixtures(dir.path());
    let nested = dir.path().join("nested");
    fs::create_dir(&nested).unwrap();
    base_image().save(nested.join("copy.png")).unwrap();

    let flat = dedup::scan(dir.path(), &ScanOptions::default()).unwrap();
    assert_eq!(flat.discovered, 6);
    assert_eq!(flat.hashed, 5);
    assert_eq!(flat.redundant(), 3);
    assert_eq!(flat.pairs_compared, 10);
    let distances: Vec<_> = flat.groups[0].with_distances().collect();
    assert_eq!(distances[0], (&flat.groups[0][0], 0));
    assert_eq!(flat.groups.len(), 1);
    let names: Vec<_> = flat.groups[0]
        .iter()
        .map(|f| f.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["base.png", "bright.png", "compressed.jpg", "small.png"]
    );
    assert_eq!(flat.failed.len(), 1);
    assert!(flat.failed[0].path().unwrap().ends_with("corrupt.png"));
    assert!(!flat.timings.hash.is_zero());
    assert!(flat.timings.total() >= flat.timings.hash);

    let options = ScanOptions {
        recursive: true,
        ..ScanOptions::default()
    };
    let deep = dedup::scan(dir.path(), &options).unwrap();
    assert_eq!(deep.hashed, 6);
    assert_eq!(deep.groups.len(), 1);
    assert_eq!(deep.groups[0].len(), 5);
}

#[test]
fn scan_reports_progress_for_every_file() {
    let dir = tempfile::tempdir().unwrap();
    write_fixtures(dir.path());

    let calls = std::sync::Mutex::new(Vec::new());
    dedup::scan_with_progress(dir.path(), &ScanOptions::default(), |done, total| {
        calls.lock().unwrap().push((done, total));
    })
    .unwrap();

    // Five images plus the corrupt file; notes.txt is not counted.
    let mut calls = calls.into_inner().unwrap();
    assert_eq!(calls[0], (0, 6));
    calls.sort();
    assert_eq!(calls, (0..=6).map(|done| (done, 6)).collect::<Vec<_>>());
}

#[test]
fn scan_rejects_missing_directory() {
    let err = dedup::scan(Path::new("/no/such/dir"), &ScanOptions::default());
    assert!(matches!(err, Err(dedup::Error::NotADirectory(_))));
}
