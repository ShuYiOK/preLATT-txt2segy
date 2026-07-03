use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use segy_core::{
    synthesize_segy, DatasetBuilder, SegyRevision, SynthesisOptions,
};

#[derive(Parser)]
#[command(name = "segy", version, about = "Microseismic TXT to SEG-Y converter")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Convert(ConvertArgs),
    Info(InfoArgs),
}

#[derive(Parser)]
struct ConvertArgs {
    #[arg(short, long)]
    input: PathBuf,

    #[arg(short, long)]
    output: PathBuf,

    #[arg(long, default_value = "microseismic")]
    prefix: String,

    #[arg(long)]
    gps: Option<PathBuf>,

    #[arg(long, default_value_t = 5000.0)]
    sample_rate: f64,

    #[arg(long)]
    per_minute: bool,

    #[arg(long, default_value = "rev2", value_parser = parse_revision)]
    revision: SegyRevision,
}

#[derive(Parser)]
struct InfoArgs {
    #[arg(short, long)]
    input: PathBuf,

    #[arg(long)]
    gps: Option<PathBuf>,
}

fn parse_revision(s: &str) -> Result<SegyRevision, String> {
    match s.to_lowercase().as_str() {
        "rev0" | "0" => Ok(SegyRevision::Rev0),
        "rev1" | "1" => Ok(SegyRevision::Rev1),
        "rev2" | "2" => Ok(SegyRevision::Rev2),
        _ => Err(format!("Unknown revision: {}. Use rev0, rev1, or rev2", s)),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Convert(args) => cmd_convert(args),
        Commands::Info(args) => cmd_info(args),
    }
}

struct SimpleProgress {
    total: u64,
    start: Instant,
    last_print: AtomicU64,
}

impl SimpleProgress {
    fn new(total: u64) -> Self {
        Self {
            total,
            start: Instant::now(),
            last_print: AtomicU64::new(0),
        }
    }

    fn update(&self, current: u64) {
        let last = self.last_print.load(Ordering::Relaxed);
        if current - last >= 10 || current == self.total {
            self.last_print.store(current, Ordering::Relaxed);
            let pct = if self.total > 0 {
                current as f64 / self.total as f64 * 100.0
            } else {
                0.0
            };
            let elapsed = self.start.elapsed().as_secs_f64();
            let eta = if current > 0 {
                (elapsed / current as f64 * (self.total - current) as f64) as u64
            } else {
                0
            };
            let bar_len = 30;
            let filled = (pct / 100.0 * bar_len as f64) as usize;
            let bar: String = "=".repeat(filled) + &"-".repeat(bar_len - filled);
            eprint!("\r[{bar}] {current}/{total} {pct:5.1}% ETA: {eta}s", bar = bar, total = self.total);
            if current == self.total {
                eprintln!();
            }
        }
    }
}

fn cmd_convert(args: ConvertArgs) -> Result<()> {
    println!("Scanning input directory: {}", args.input.display());

    let gps_map = if let Some(gps_path) = &args.gps {
        println!("Loading GPS stations from: {}", gps_path.display());
        let stations = segy_core::gps::parse_gps_file(gps_path)
            .with_context(|| format!("Failed to parse GPS file: {}", gps_path.display()))?;
        println!("Loaded {} GPS stations", stations.len());
        segy_core::gps::gps_to_map(stations)
    } else {
        std::collections::HashMap::new()
    };

    let mut builder = DatasetBuilder::new(&args.input)
        .with_gps(gps_map.clone());

    if args.sample_rate > 0.0 && args.sample_rate != 5000.0 {
        builder = builder.with_sample_rate(args.sample_rate);
    }

    let dataset = builder
        .build()
        .with_context(|| "Failed to build dataset")?;

    println!("Found {} devices, {} time slices", dataset.num_devices(), dataset.num_time_slices());
    println!("Sample rate: {:.2} Hz", dataset.sample_rate_hz);
    println!("Samples per trace: {}", dataset.samples_per_minute);

    if let Some((start, end)) = dataset.time_range() {
        println!(
            "Time range: {} to {}",
            start.format("%Y-%m-%d %H:%M:%S"),
            end.format("%Y-%m-%d %H:%M:%S")
        );
    }

    let total = dataset.num_time_slices() as u64;
    let progress = Arc::new(SimpleProgress::new(total));

    let mut options = SynthesisOptions::default();
    options.output_dir = args.output.clone();
    options.file_prefix = args.prefix.clone();
    options.revision = args.revision;
    options.per_minute_files = args.per_minute;
    options.use_gps_coords = !gps_map.is_empty();

    let progress_clone = progress.clone();
    options.progress_callback = Some(Box::new(move |current, _total| {
        progress_clone.update(current);
    }));

    println!("\nSynthesizing SEG-Y files...");
    let output_files = synthesize_segy(&dataset, &options)
        .with_context(|| "Synthesis failed")?;

    println!("\nOutput files:");
    for f in &output_files {
        let size = std::fs::metadata(f)?.len();
        println!("  {} ({:.2} MB)", f.display(), size as f64 / 1024.0 / 1024.0);
    }

    println!("\nSuccess! {} files generated.", output_files.len());
    Ok(())
}

fn cmd_info(args: InfoArgs) -> Result<()> {
    println!("Scanning: {}", args.input.display());

    let gps_map = if let Some(gps_path) = &args.gps {
        let stations = segy_core::gps::parse_gps_file(gps_path)
            .with_context(|| format!("Failed to parse GPS file: {}", gps_path.display()))?;
        segy_core::gps::gps_to_map(stations)
    } else {
        std::collections::HashMap::new()
    };

    let dataset = DatasetBuilder::new(&args.input)
        .with_gps(gps_map)
        .build()
        .with_context(|| "Failed to build dataset")?;

    println!("\n=== Dataset Info ===");
    println!("Devices:        {}", dataset.num_devices());
    println!("Time slices:    {}", dataset.num_time_slices());
    println!("Sample rate:    {:.2} Hz", dataset.sample_rate_hz);
    println!("Samples/minute: {}", dataset.samples_per_minute);

    if let Some((start, end)) = dataset.time_range() {
        println!("Start time:     {}", start.format("%Y-%m-%d %H:%M:%S UTC"));
        println!("End time:       {}", end.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    println!("\nDevices:");
    for (i, dev) in dataset.devices.iter().enumerate() {
        let has_gps = dataset.gps.contains_key(dev);
        println!("  {:3}. {} [GPS: {}]", i + 1, dev, if has_gps { "yes" } else { "no" });
    }

    let total_samples = dataset.num_time_slices() * dataset.samples_per_minute * dataset.num_devices();
    let est_size_mb = total_samples as f64 * 4.0 / 1024.0 / 1024.0
        + dataset.num_time_slices() as f64 * dataset.num_devices() as f64 * 240.0 / 1024.0 / 1024.0
        + 3.5;

    println!("\nTotal samples:  {}", total_samples);
    println!("Estimated size: {:.2} MB (single file)", est_size_mb);

    Ok(())
}
