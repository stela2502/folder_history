// Keep one implementation of scanning: this binary imports the same core as
// the GUI. A future library extraction can replace this module inclusion.
#[path = "../main.rs"]
mod folder_history;

use std::path::PathBuf;

fn usage() {
    eprintln!("Usage: folder-history-cli sub-folders-of <main-folder> [--resume] [--rescan] [--jobs 1]");
    eprintln!("Each immediate subfolder is scanned into its existing local FolderHistory index.");
    eprintln!("Default: skip existing databases. --rescan: refresh while retaining manual decisions.");
}

fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("sub-folders-of") { usage(); anyhow::bail!("Expected sub-folders-of"); }
    let Some(root_arg) = args.next() else { usage(); anyhow::bail!("Missing main folder"); };
    let root = PathBuf::from(root_arg).canonicalize()?;
    if !root.is_dir() { anyhow::bail!("Not a directory: {}", root.display()); }
    let mut rescan = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--resume" => {}, // Skipping indexed folders is the default.
            "--rescan" => rescan = true,
            "--jobs" => {
                let value = args.next().ok_or_else(|| anyhow::anyhow!("--jobs requires a number"))?;
                if value != "1" { anyhow::bail!("Only --jobs 1 is supported (NAS-safe sequential scanning)"); }
            },
            "--help" | "-h" => { usage(); return Ok(()); },
            other => anyhow::bail!("Unknown argument: {other}"),
        }
    }
    let mut folders = std::fs::read_dir(&root)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let ty = entry.file_type().ok()?;
            // Do not follow symlinked top-level research folders.
            if ty.is_dir() && !ty.is_symlink() && !entry.file_name().to_string_lossy().starts_with('.') {
                Some(entry.path())
            } else { None }
        })
        .collect::<Vec<_>>();
    folders.sort();
    println!("FolderHistory batch: {} subfolders under {}", folders.len(), root.display());
    let mut failed = 0usize;
    for (i, folder) in folders.iter().enumerate() {
        println!("\n[{}/{}] {}", i+1, folders.len(), folder.display());
        match folder_history::batch_scan_folder(folder, rescan) {
            Ok(message) => println!("{message}"),
            Err(error) => { failed+=1; eprintln!("ERROR {}: {error:#}", folder.display()); },
        }
    }
    println!("\nFinished: {} folders, {} failed", folders.len(), failed);
    if failed > 0 { anyhow::bail!("{failed} folders failed; rerun to resume"); }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
