use clap::{CommandFactory, Parser};
use std::path::PathBuf;

/// Merge Minecraft resource packs into a single zip. Later inputs overwrite earlier ones.
#[derive(Parser, Debug)]
#[command(
    name = "resource-merger",
    version,
    about,
    long_about = None,
    // If no args are provided, show help instead of silently failing
    arg_required_else_help = true
)]
struct Args {
    /// Output zip path
    #[arg(
        short,
        long,
        value_name = "PATH",
        help = "Output path. If --dir is set, this is a directory; otherwise a zip file."
    )]
    out: Option<PathBuf>,

    /// Input packs (directories, zip files, or URLs). Order matters; later inputs overwrite earlier ones.
    #[arg(
        required = true,
        value_name = "INPUTS",
        help = "Input packs (directories, zip files, or HTTP/HTTPS URLs). Order matters; later inputs override earlier ones."
    )]
    inputs: Vec<PathBuf>,
    /// Read inputs from a config file (JSON); entries from the config will be used first
    #[arg(
        long,
        value_name = "PATH",
        help = "Path to a JSON config file that mirrors CLI options. Values from the CLI override config."
    )]
    config: Option<PathBuf>,
    /// Write output as a directory instead of a zip file
    #[arg(
        long,
        help = "Write output as a directory instead of a zip file. Overrides config.dir if present."
    )]
    dir: bool,

    /// Overwrite policy: last, first, error, skip
    #[arg(
        long,
        value_name = "POLICY",
        help = "Overwrite policy: last|first|error|skip (default: last). Later packs overwrite earlier ones under 'last'."
    )]
    overwrite: Option<String>,

    /// Don't write output, only print what would be done
    #[arg(
        long,
        help = "Validate and print plan, but do not write output (dry run)."
    )]
    dry_run: bool,

    /// Buffer size in bytes for streaming copies
    #[arg(
        long,
        value_name = "BYTES",
        help = "Buffer size in bytes for streaming copies (default 32768)."
    )]
    buffer_size: Option<usize>,

    /// Use atomic write semantics (write to temp and rename)
    #[arg(
        long,
        help = "Write atomically when possible (write to a temp file then rename). Use --no-atomic to disable."
    )]
    atomic: bool,
    /// Explicitly disable atomic semantics
    #[arg(long = "no-atomic", help = "Disable atomic write semantics.")]
    no_atomic: bool,

    /// Preserve timestamps when extracting
    #[arg(
        long,
        help = "Preserve file timestamps when extracting into a directory."
    )]
    preserve_timestamps: bool,
    /// Force pack_format in generated pack.mcmeta (overrides detected formats)
    #[arg(
        long,
        value_name = "N",
        help = "Force pack_format in pack.mcmeta (overrides detected values)."
    )]
    pack_format: Option<u32>,
    /// How to synthesize supported_formats in pack.mcmeta: one-to-highest, lowest-to-highest, one-to-latest
    #[arg(
        long,
        value_name = "POLICY",
        help = "Supported formats synthesis policy: one-to-highest|lowest-to-highest|one-to-latest."
    )]
    supported_formats: Option<String>,

    /// Optional pack description to include in generated pack.mcmeta (overrides config)
    #[arg(
        long,
        value_name = "TEXT",
        help = "Set a custom description for the generated pack.mcmeta (overrides config.description)."
    )]
    description: Option<String>,
}

fn main() {
    let args = match Args::try_parse() {
        Ok(a) => a,
        Err(e) => {
            // Print the error message (parser will include suggestions) and then show full help
            eprintln!("{}", e);
            // Print help to stdout for the user
            let _ = Args::command().print_help();
            println!();
            std::process::exit(2);
        }
    };

    // Build input list from config (if any) and positional args.
    let mut inputs: Vec<resource_merger::PackInput> = Vec::new();
    let mut cfg_obj: Option<resource_merger::Config> = None;
    if let Some(cfg_path) = &args.config {
        match resource_merger::read_config_file(cfg_path) {
            Ok(c) => cfg_obj = Some(c),
            Err(e) => {
                eprintln!("failed to read config {}: {}", cfg_path.display(), e);
                std::process::exit(2);
            }
        }
    }

    // If config has inputs, add them first
    if let Some(cfg) = &cfg_obj {
        if let Some(cfg_inputs) = &cfg.inputs {
            for s in cfg_inputs {
                inputs.push(resource_merger::PackInput::from(s.clone()));
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

    // Build options with clear precedence: CLI (Some) -> config -> default
    let overwrite = if let Some(s) = &args.overwrite {
        match s.parse::<resource_merger::OverwritePolicy>() {
            Ok(o) => o,
            Err(e) => {
                eprintln!("invalid overwrite value: {}", e);
                std::process::exit(2);
            }
        }
    } else if let Some(cfg) = &cfg_obj {
        if let Some(s) = &cfg.overwrite {
            match s.parse::<resource_merger::OverwritePolicy>() {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("invalid overwrite value in config: {}", e);
                    std::process::exit(2);
                }
            }
        } else {
            resource_merger::OverwritePolicy::LastWins
        }
    } else {
        resource_merger::OverwritePolicy::LastWins
    };

    let dry_run = if args.dry_run {
        true
    } else {
        cfg_obj.as_ref().and_then(|c| c.dry_run).unwrap_or(false)
    };

    let buffer_size = args
        .buffer_size
        .or_else(|| cfg_obj.as_ref().and_then(|c| c.buffer_size))
        .unwrap_or(32 * 1024);

    // atomic: CLI --atomic sets true; --no-atomic sets false; otherwise use config or default true
    let atomic = if args.atomic {
        true
    } else if args.no_atomic {
        false
    } else {
        cfg_obj.as_ref().and_then(|c| c.atomic).unwrap_or(true)
    };

    let preserve_timestamps = if args.preserve_timestamps {
        true
    } else {
        cfg_obj
            .as_ref()
            .and_then(|c| c.preserve_timestamps)
            .unwrap_or(false)
    };

    let pack_format_override = args
        .pack_format
        .or_else(|| cfg_obj.as_ref().and_then(|c| c.pack_format));

    let supported_formats_str: Option<String> = args
        .supported_formats
        .clone()
        .or_else(|| cfg_obj.as_ref().and_then(|c| c.supported_formats.clone()));

    let supported_formats_policy = match supported_formats_str {
        Some(s) => match s.parse::<resource_merger::SupportedFormatsPolicy>() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("invalid supported_formats value in config or args: {}", e);
                std::process::exit(2);
            }
        },
        None => resource_merger::SupportedFormatsPolicy::OneToHighest,
    };

    let opts = resource_merger::MergeOptions {
        overwrite,
        dry_run,
        buffer_size,
        atomic,
        preserve_timestamps,
        pack_format_override,
        supported_formats_policy,
        description_override: args
            .description
            .clone()
            .or_else(|| cfg_obj.as_ref().and_then(|c| c.description.clone())),
    };
    // Determine output path: CLI `--out` takes precedence, otherwise try config `out`.
    let out_path: PathBuf = if let Some(o) = &args.out {
        o.clone()
    } else if let Some(cfg) = &cfg_obj {
        if let Some(co) = &cfg.out {
            PathBuf::from(co)
        } else {
            eprintln!("no output path provided; pass --out or add `out` to config");
            std::process::exit(2);
        }
    } else {
        eprintln!("no output path provided; pass --out or add `out` to config");
        std::process::exit(2);
    };

    let dir_flag = if args.dir {
        true
    } else {
        cfg_obj.as_ref().and_then(|c| c.dir).unwrap_or(false)
    };

    let res = if dir_flag {
        resource_merger::merge_packs_to_dir(&inputs, &out_path, &opts)
    } else {
        resource_merger::merge_packs_to_file_with_options(&inputs, &out_path, &opts)
    };

    if let Err(e) = res {
        eprintln!("error merging packs: {}", e);
        std::process::exit(1);
    }

    println!("Wrote merged output to {}", out_path.display());
}
