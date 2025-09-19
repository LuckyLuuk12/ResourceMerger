# resource_merger

A small, practical Rust library and CLI for merging Minecraft resource packs (folders, zip files, or remote zip URLs) into a single merged resource pack. The merge is ordered: later packs overwrite earlier ones by default.

Key features
- Accept inputs as directories, zip files on disk, in-memory zip bytes, or HTTP/HTTPS URLs
- CLI with handy flags (overwrite policy, dry-run, config file, write-as-dir)
- Library API: in-memory merge, file output, and (placeholder) directory output
- Config file support: sorted list of paths/URLs to merge in order

Install / add to Cargo.toml

```toml
[dependencies]
resource_merger = { path = "../ResourceMerger" }
```

Library usage (examples)

- Simple merge to bytes:

```rust
use resource_merger::{merge_packs_to_bytes, PackInput};
use std::path::PathBuf;

let packs = vec![
    PackInput::Dir(PathBuf::from("./pack_base")),
    PackInput::ZipFile(PathBuf::from("./overlay.zip")),
];
let merged = merge_packs_to_bytes(&packs)?; // Vec<u8> with a zip archive
```

- Merge and write to a file with options:

```rust
use resource_merger::{merge_packs_to_file_with_options, PackInput, MergeOptions};
use std::path::PathBuf;

let opts = MergeOptions::default();
let packs = vec![PackInput::Dir(PathBuf::from("./pack_base"))];
merge_packs_to_file_with_options(&packs, PathBuf::from("merged.zip"), &opts)?;
```

- Merge a folder containing multiple packs (useful for a `resourcepacks/` directory):

```rust
use resource_merger::merge_all_packs_in_folder;
use std::path::Path;

let bytes = merge_all_packs_in_folder(Path::new("./resourcepacks"))?;
```

- Read a config file (one URL/path per line) and use it:

```rust
use resource_merger::read_config_file;
let packs = read_config_file(Path::new("packs.txt"))?; // Vec<PackInput>
```

Overwrite policies
- `LastWins` (default): later packs overwrite earlier ones.
- `FirstWins`: first occurrence wins; later duplicates ignored.
- `ErrorIfConflict`: error on duplicate paths.
- `SkipIfExists`: skip writing if file already exists.

CLI usage

Build the binary with `cargo build --release` or run via `cargo run --bin merge -- ...`.

Examples:

- Merge into a zip (last wins):

```bash
resource-merger --out merged.zip packA packB packC
```

- Merge into a directory (streaming / safer for large packs):

```bash
resource-merger --dir --out merged_folder packA packB
```

- Use a config file (entries are processed first, then positional args):

```bash
resource-merger --config packs.txt --out merged.zip
```

Flags (CLI)
- `--out <PATH>`: output path (zip or directory)
- `--dir`: write the merged output as a directory instead of a zip
- `--config <PATH>`: read inputs (URLs or file paths) from a newline-separated config file
- `--overwrite <last|first|error|skip>`: overwrite policy (default `last`)
- `--dry-run`: scan and validate inputs, but don't write output
- `--buffer-size <BYTES>`: buffer size for streaming copies (default 32768)
- `--atomic`/`--no-atomic`: write atomically when possible (default true)
- `--preserve-timestamps`: preserve timestamps when extracting (optional)

Config file format
- Plain text file; one entry per line. Lines starting with `#` are comments.
- Each entry can be an absolute or relative filesystem path, or an `http://` / `https://` URL.
- Entries are processed in file order. Example `packs.txt`:

```
# base packs
https://example.com/base_pack.zip
./mods/some_pack.zip
# overlay packs
./local_overlays/overlay_folder
```

Security notes
- The library must sanitize zip paths to avoid zip-slip (entries attempting to write outside the destination). Avoid passing untrusted zips to extraction without validation.
- Remote URLs are downloaded and treated as zip archives. Consider size limits and validating the response before extraction.

Publishing notes
- The crate exposes a typed `MergeError` and `MergeOptions` for predictable consumption by other crates.
- Add CI to run tests and `cargo fmt` / `clippy` before publishing.

License

This project is licensed under MIT. See the `LICENSE` file for details.
