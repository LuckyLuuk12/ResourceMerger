# resource_merger

A small, practical Rust library and CLI for merging Minecraft resource packs (folders, zip files, or remote zip URLs) into a single merged resource pack. The merge is ordered: later packs overwrite earlier ones by default.

## Key features

- Accept inputs as directories, zip files on disk, in-memory zip bytes, or HTTP/HTTPS URLs
- CLI with flags: overwrite policy, presence-style boolean flags (e.g. `--dir`, `--dry-run`), JSON config that mirrors CLI args, pack_format override, supported_formats policy
- Library API: in-memory merge, write to file, and directory extraction helper
- Generates a valid resource pack: ensures `pack.mcmeta`, `pack.png` (default if missing), and `README.md` are present in the merged output

## Install / add to Cargo.toml

```toml
[dependencies]
resource_merger = "0.1"
```

## Examples

Usage examples and runnable demos are available in the `examples/` folder. See:

- [examples/cli_commands.md](https://github.com/LuckyLuuk12/resource_merger/blob/main/examples/cli_commands.md) — CLI command examples (PowerShell + Unix)
- [examples/sample_config.json](https://github.com/LuckyLuuk12/resource_merger/blob/main/examples/sample_config.json) — JSON config example covering all supported options
- [examples/merge_example.rs](https://github.com/LuckyLuuk12/resource_merger/blob/main/examples/merge_example.rs) — a small Rust example showing library usage

Refer to those files for copy-pasteable examples and more detailed usage. You can also browse the whole `examples/` directory on GitHub:

- https://github.com/LuckyLuuk12/resource_merger/tree/main/examples

## What the merger generates

- `pack.mcmeta`: always present in merged output. Generated `description` is `Made with Rust API: resource_merger:<version>`.
- `supported_formats`: synthesized according to the chosen policy (see below). By default it's `[1, highest_found]`.
- `pack.png`: a tiny default icon is added if none of the inputs provide one.
- `README.md`: a short file listing the inputs used and the merger version.

## Overwrite policies

- `LastWins` (default): later packs overwrite earlier ones.
- `FirstWins`: first occurrence wins; later duplicates ignored.
- `ErrorIfConflict`: error on duplicate paths.
- `SkipIfExists`: skip writing if file already exists.

## CLI usage

Build the binary with `cargo build --release` or run via `cargo run --bin merge -- ...`.

See `examples/cli_commands.md` for copyable CLI commands (PowerShell and Unix shell variants) and more usage scenarios. The JSON config is intentionally a file-version of the CLI `Args` — any option you can set on the CLI can also be set in the JSON config. CLI arguments always override values present in the JSON config.

### Important CLI flags (summary)

- `--out <PATH>`: output path (zip or directory)
- `--dir`: write the merged output as a directory instead of a zip (presence flag; omit to use config/default)
- `--config <PATH>`: read inputs and optional settings from a JSON config file
- `--overwrite <last|first|error|skip>`: overwrite policy (default `last`)
- `--dry-run`: scan and validate inputs, but don't write output (presence flag)
- `--buffer-size <BYTES>`: buffer size for streaming copies (default 32768)
- `--atomic`/`--no-atomic`: explicitly enable or disable atomic writes (default `--atomic` behavior if neither provided)
- `--preserve-timestamps`: preserve timestamps when extracting (presence flag)
- `--pack-format <N>`: force `pack_format` in generated `pack.mcmeta` (overrides detected values)
- `--supported-formats <policy>`: how to synthesize `supported_formats` in `pack.mcmeta` (default `one-to-highest`). Accepted values:
    - `one-to-highest`: produce `[1, highest_found]`
    - `lowest-to-highest`: produce `[lowest_found, highest_found]`
    - `one-to-latest`: planned (currently falls back to `one-to-highest`)
- `--description <TEXT>`: optional description to include in generated `pack.mcmeta` (overrides config.description)

## JSON config format

The config file is JSON only and intentionally mirrors the CLI `Args` — treat it as a file-version of the CLI flags. Any option you can set on the CLI can also be set in the JSON config. When both are present, CLI flags always override the config file value.

Example (`examples/sample_config.json`):

```json
{
    "inputs": [
        "C:/Users/Example/packs/base_pack",
        "C:/Users/Example/packs/override_pack.zip",
        "https://example.com/packs/extra_resources.zip"
    ],
    "overwrite": "last",
    "dry_run": false,
    "buffer_size": 32768,
    "atomic": true,
    "preserve_timestamps": false,
    "pack_format": 34,
    "supported_formats": "one-to-highest",
    "dir": false,
    "description": "Optional pack description"
}
```

## Defaults

- overwrite: `last` (LastWins)
- buffer_size: 32768
- atomic: true
- preserve_timestamps: false
- pack_format: highest found among inputs (or `1` if none found), unless overridden with `--pack-format`
- supported_formats: `one-to-highest` (can be changed with `--supported-formats`)
- dir: false (set to true in config or pass `--dir` to override)
- description: optional pack description (can be provided in config or via `--description`)

## Security notes

- Sanitize zip entries to avoid zip-slip when extracting. Avoid extracting untrusted zips without validation.
- When downloading remote zips, consider size limits and network reliability.

## Publishing and testing

- The crate exposes `MergeError`, `MergeOptions`, `PackInput`, and convenience functions: `merge_packs_to_bytes`, `merge_packs_to_file_with_options`, `merge_packs_to_dir`, `merge_all_packs_in_folder`.
- Add CI to run `cargo test` and `cargo fmt` / `clippy` before publishing.

## License

MIT — see the `LICENSE` file.
