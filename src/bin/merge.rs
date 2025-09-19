use clap::Parser;
use std::path::PathBuf;

/// Merge Minecraft resource packs into a single zip. Later inputs overwrite earlier ones.
#[derive(Parser, Debug)]
#[command(name = "resource-merger")]
struct Args {
    /// Output zip path
    #[arg(short, long)]
    out: PathBuf,

    /// Input packs (directories or zip files). Order matters; the last will have highest priority.
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Read inputs from a config file (one URL/path per line); entries from the config will be used first
    #[arg(long)]
    config: Option<PathBuf>,
    /// Write output as a directory instead of a zip file
    #[arg(long)]
    dir: bool,

    /// Overwrite policy: last, first, error, skip
    #[arg(long, default_value = "last")]
    overwrite: String,

    /// Don't write output, only print what would be done
    #[arg(long)]
    dry_run: bool,

    /// Buffer size in bytes for streaming copies (default 32768)
    #[arg(long, default_value_t = 32768)]
    buffer_size: usize,

    /// Use atomic write semantics (write to temp and rename)
    #[arg(long, default_value_t = true)]
    atomic: bool,

    /// Preserve timestamps when extracting
    #[arg(long)]
    preserve_timestamps: bool,
}

fn main() {
    let args = Args::parse();

    // Build input list from config (if any) and positional args.
    let mut inputs: Vec<resource_merger::PackInput> = Vec::new();
    if let Some(cfg) = &args.config {
        match resource_merger::read_config_file(cfg) {
            Ok(mut cfg_packs) => inputs.append(&mut cfg_packs),
            Err(e) => {
                eprintln!("failed to read config {}: {}", cfg.display(), e);
                std::process::exit(2);
            }
        }
    }

    // Add positional inputs
    for p in &args.inputs {
        if !p.exists() {
            eprintln!("input path does not exist: {}", p.display());
            std::process::exit(2);
        }
        inputs.push(p.clone().into());
    }

    // Build options
    let overwrite = match args.overwrite.parse::<resource_merger::OverwritePolicy>() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("invalid overwrite value: {}", e);
            std::process::exit(2);
        }
    };

    let opts = resource_merger::MergeOptions {
        overwrite,
        dry_run: args.dry_run,
        buffer_size: args.buffer_size,
        atomic: args.atomic,
        preserve_timestamps: args.preserve_timestamps,
    };

    let res = if args.dir {
        resource_merger::merge_packs_to_dir(&inputs, &args.out, &opts)
    } else {
        resource_merger::merge_packs_to_file_with_options(&inputs, &args.out, &opts)
    };

    if let Err(e) = res {
        eprintln!("error merging packs: {}", e);
        std::process::exit(1);
    }

    println!("Wrote merged zip to {}", args.out.display());
}
