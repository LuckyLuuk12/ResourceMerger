# resource_merger — CLI examples

Below are example commands for building and running the merger and several common use cases. Use the PowerShell variants on Windows and the shell variants on Unix-like systems. Replace paths with your own.

## Build the project

PowerShell (Windows):

```powershell
# Build the project (creates target/debug/merge.exe)
cargo build --manifest-path C:\Projects\cli\ResourceMerger\Cargo.toml
```

Unix / macOS:

```bash
cargo build --manifest-path /path/to/ResourceMerger/Cargo.toml
```

## Run with positional inputs

PowerShell:

```powershell
# Merge two packs (directory + zip) into a single zip
.\target\debug\merge.exe --out merged.zip C:\packs\base C:\packs\override.zip
```

Or with cargo run (no separate build step required):

```powershell
cargo run --manifest-path C:\Projects\cli\ResourceMerger\Cargo.toml --bin merge -- --out merged.zip C:\packs\base C:\packs\override.zip
```

## Use a config file (entries from the config will be used first)

PowerShell:

```powershell
.\target\debug\merge.exe --config examples\sample_config.json --out merged_from_config.zip additional_pack.zip
```

## Force pack_format and supported_formats policy

- Force the generated `pack.mcmeta` to use `pack_format = 34`:

```powershell
.\target\debug\merge.exe --out merged.zip --pack-format 34 base_pack override.zip
```

- Choose how `supported_formats` is synthesized in `pack.mcmeta` (default is `one-to-highest`):
  - `one-to-highest` — produce `[1, highest_found]`
  - `lowest-to-highest` — produce `[lowest_found, highest_found]`
  - `one-to-latest` — planned (falls back to `one-to-highest` currently)

Example:

```powershell
.\target\debug\merge.exe --out merged.zip --supported-formats lowest-to-highest base_pack override.zip
```

## Write output as a directory instead of a zip

```powershell
.\target\debug\merge.exe --dir true --out merged_folder base_pack override.zip
```

This will extract the merged pack contents into `merged_folder/`.

## Dry run (validate and print what would be done)

```powershell
.\target\debug\merge.exe --out merged.zip --dry-run true base_pack override.zip
```

## Example: combine options

```powershell
.\target\debug\merge.exe --config examples\sample_config.json --out final.zip --pack-format 34 --supported-formats one-to-highest
```

## Notes

- If you build a release binary use `cargo build --release` and run `target\release\merge.exe`.
- The CLI also accepts HTTP/HTTPS URLs (it will download the zip bytes) — be mindful of large downloads and network reliability.
- If you need `supported_formats` to be based on an authoritative mapping (Mojang API), I can implement a `one-to-latest` mode that queries an upstream source.
 - CLI arguments always override values present in the JSON config file. Use the config as a convenient defaults file and pass CLI flags to override specific settings.
