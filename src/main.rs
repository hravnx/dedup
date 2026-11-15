use std::fmt;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use dedup::{Algorithm, DEFAULT_MAX_DISTANCE, HASH_BITS, ImageHash, ScanOptions, ScanReport};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};
use serde_json::json;

/// Find near-duplicate images with perceptual hashing.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report groups of similar images in a directory.
    Scan {
        dir: PathBuf,
        #[arg(short, long, value_enum, default_value_t = Algo::Phash)]
        algorithm: Algo,
        /// Maximum number of differing hash bits (0-64) for images to count as duplicates.
        #[arg(short = 't', long, default_value_t = DEFAULT_MAX_DISTANCE,
              value_parser = clap::value_parser!(u32).range(0..=HASH_BITS as i64))]
        max_distance: u32,
        /// Descend into subdirectories.
        #[arg(short, long)]
        recursive: bool,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
        /// Do not show a progress bar while hashing.
        #[arg(long)]
        no_progress: bool,
    },
    /// Compare two image files.
    Compare {
        a: PathBuf,
        b: PathBuf,
        #[arg(short, long, value_enum, default_value_t = Algo::Phash)]
        algorithm: Algo,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Algo {
    /// Mean-brightness hash: fastest, least precise.
    Ahash,
    /// Gradient hash: fast, robust to brightness changes.
    Dhash,
    /// DCT hash: slowest, most robust.
    Phash,
}

impl From<Algo> for Algorithm {
    fn from(algo: Algo) -> Self {
        match algo {
            Algo::Ahash => Algorithm::Average,
            Algo::Dhash => Algorithm::Difference,
            Algo::Phash => Algorithm::Perceptual,
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> dedup::Result<()> {
    match cli.command {
        Command::Scan {
            dir,
            algorithm,
            max_distance,
            recursive,
            json,
            no_progress,
        } => {
            let options = ScanOptions {
                algorithm: algorithm.into(),
                max_distance,
                recursive,
            };
            // Drawn on stderr, and hidden automatically when stderr is not a terminal.
            let bar = if no_progress {
                ProgressBar::hidden()
            } else {
                ProgressBar::no_length().with_style(
                    ProgressStyle::with_template(
                        "Hashing {bar:30} {pos}/{len} images ({rate}, eta {eta})",
                    )
                    .expect("valid progress template")
                    // A custom key, because in a template `{per_sec:.0}` is a
                    // colour (0 = black), not a precision.
                    .with_key(
                        "rate",
                        |state: &ProgressState, w: &mut dyn fmt::Write| {
                            let _ = write!(w, "{:.0}/s", state.per_sec());
                        },
                    ),
                )
            };
            let report = dedup::scan_with_progress(&dir, &options, |done, total| {
                bar.set_length(total as u64);
                bar.set_position(bar.position().max(done as u64));
            });
            bar.finish_and_clear();
            let report = report?;
            if json {
                print_json(&report);
            } else {
                print_text(&report);
            }
        }
        Command::Compare { a, b, algorithm } => {
            let ha = ImageHash::from_path(&a, algorithm.into())?;
            let hb = ImageHash::from_path(&b, algorithm.into())?;
            println!("{ha}  {}", a.display());
            println!("{hb}  {}", b.display());
            println!(
                "distance {} of {HASH_BITS} bits, {:.1}% similar",
                ha.distance(&hb),
                ha.similarity(&hb) * 100.0
            );
        }
    }
    Ok(())
}

fn print_text(report: &ScanReport) {
    for e in &report.failed {
        eprintln!("skipped: {e}");
    }
    for (i, group) in report.groups.iter().enumerate() {
        println!("Group {} ({} files)", i + 1, group.len());
        let first = &group[0];
        for file in group {
            println!(
                "  {}  {:>2}  {}",
                file.hash,
                first.hash.distance(&file.hash),
                file.path.display()
            );
        }
        println!();
    }
    let duplicates: usize = report.groups.iter().map(|g| g.len() - 1).sum();
    println!(
        "{} images hashed, {} skipped, {} groups, {} redundant files",
        report.hashed,
        report.failed.len(),
        report.groups.len(),
        duplicates
    );

    let stats = Stats::new(report);
    println!(
        "Took {:.2}s: discovery {:.2}s, hashing {:.2}s ({:.0} images/s on {} threads), comparing {:.2}s ({} pairs)",
        report.timings.total().as_secs_f64(),
        report.timings.discover.as_secs_f64(),
        report.timings.hash.as_secs_f64(),
        stats.images_per_sec,
        stats.threads,
        report.timings.compare.as_secs_f64(),
        stats.pairs
    );
}

/// Figures derived from a report's timings.
struct Stats {
    /// Image files processed per second of hashing, including ones that failed to decode.
    images_per_sec: f64,
    /// Worker threads used for hashing.
    threads: usize,
    /// Hash pairs compared.
    pairs: u64,
}

impl Stats {
    fn new(report: &ScanReport) -> Self {
        let processed = report.hashed + report.failed.iter().filter(|e| is_image_error(e)).count();
        let secs = report.timings.hash.as_secs_f64();
        let n = report.hashed as u64;
        Stats {
            images_per_sec: if secs > 0.0 {
                processed as f64 / secs
            } else {
                0.0
            },
            threads: rayon::current_num_threads(),
            pairs: n * n.saturating_sub(1) / 2,
        }
    }
}

fn is_image_error(e: &dedup::Error) -> bool {
    matches!(e, dedup::Error::Image { .. })
}

fn print_json(report: &ScanReport) {
    let groups: Vec<_> = report
        .groups
        .iter()
        .map(|group| {
            let first = &group[0];
            group
                .iter()
                .map(|file| {
                    json!({
                        "path": file.path,
                        "hash": file.hash.to_string(),
                        "distance": first.hash.distance(&file.hash),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let failed: Vec<_> = report
        .failed
        .iter()
        .map(|e| json!({ "path": e.path(), "error": e.to_string() }))
        .collect();
    let stats = Stats::new(report);
    let out = json!({
        "hashed": report.hashed,
        "groups": groups,
        "failed": failed,
        "stats": {
            "total_secs": report.timings.total().as_secs_f64(),
            "discover_secs": report.timings.discover.as_secs_f64(),
            "hash_secs": report.timings.hash.as_secs_f64(),
            "compare_secs": report.timings.compare.as_secs_f64(),
            "images_per_sec": stats.images_per_sec,
            "threads": stats.threads,
            "pairs_compared": stats.pairs,
        },
    });
    println!("{out:#}");
}
