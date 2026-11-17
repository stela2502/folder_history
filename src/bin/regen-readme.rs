//! Developer-only generator for an example of the local research help.
use anyhow::{bail, Context, Result};
use std::path::Path;

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme = root.join("example_local_readme.md");
    let generated = folder_history::core::documentation::example_local_help_markdown();
    if readme.exists() {
        let existing = std::fs::read_to_string(&readme)
            .with_context(|| format!("Reading {}", readme.display()))?;
        if !existing.contains("FOLDERHISTORY-GENERATED-DOCUMENTATION") {
            bail!("Refusing to overwrite unmanaged README: {}", readme.display());
        }
    }
    let temp = root.join(".example_local_readme.md.folderhistory.tmp");
    std::fs::write(&temp, generated)?;
    if let Err(error) = std::fs::rename(&temp, &readme) {
        let _ = std::fs::remove_file(&temp);
        return Err(error).with_context(|| format!("Replacing {}", readme.display()));
    }
    println!("Regenerated {}", readme.display());
    Ok(())
}
