use resource_merger::{PackInput, merge_packs_to_bytes};
use std::fs::write;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    // Example: merge a base folder with an overlay zip, overlay wins on conflicts
    let base = PackInput::Dir(PathBuf::from("examples/example_base"));
    let overlay = PackInput::ZipFile(PathBuf::from("examples/example_overlay.zip"));

    let merged = merge_packs_to_bytes(&[base, overlay])?;
    write("examples/merged_example.zip", merged)?;
    println!("Wrote examples/merged_example.zip");
    Ok(())
}
