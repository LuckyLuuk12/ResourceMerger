# resource_merger

A small Rust library to merge multiple Minecraft resource packs (directories, zip files, or in-memory zip bytes) into a single zip archive. The merge is ordered: later packs overwrite files from earlier packs, making it easy to compose resource packs.

Features
- Accepts directories, zip files on disk, or zip bytes in memory
- Later packs overwrite earlier ones (order-preserving)
- Returns a merged zip as bytes you can write to disk or send over network

Usage

Add to your Cargo.toml:

```toml
[dependencies]
resource_merger = { path = "../ResourceMerger" }
```

Basic example (merging two directories and a zip on disk):

```rust
use resource_merger::{merge_packs_to_bytes, PackInput};
use std::fs::write;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let packs = vec![
        PackInput::Dir(PathBuf::from("./pack_base")),
        PackInput::ZipFile(PathBuf::from("./overlay.zip")),
    ];

    let merged = merge_packs_to_bytes(&packs)?;
    write("merged_resourcepack.zip", merged)?;
    Ok(())
}
```

In-memory zip bytes example:

```rust
use resource_merger::{merge_packs_to_bytes, PackInput};

let zip_bytes: Vec<u8> = /* load from somewhere */ vec![];
let packs = vec![PackInput::ZipBytes(zip_bytes)];
let merged = merge_packs_to_bytes(&packs)?;
```

Notes
- Paths inside directories are normalized to use forward slashes so they work well inside zip archives.
- The crate returns errors via `anyhow::Result` to simplify error handling in examples.

License

This project is licensed under MIT. See LICENSE file for details.
