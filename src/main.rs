use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use dedup::{Algorithm, DEFAULT_MAX_DISTANCE, HASH_BITS, ImageHash, ScanOptions, ScanReport};
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
        } => {
            let options = ScanOptions {
                algorithm: algorithm.into(),
                max_distance,
                recursive,
            };
            let report = dedup::scan(&dir, &options)?;
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
    let out = json!({ "hashed": report.hashed, "groups": groups, "failed": failed });
    println!("{out:#}");
}
