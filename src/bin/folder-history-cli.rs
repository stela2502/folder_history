use folder_history::core::batch_scan_folder;

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

fn usage() {
    eprintln!("Usage: folder-history-cli sub-folders-of <main-folder> [--resume] [--rescan] [--jobs N]");
    eprintln!("Each immediate subfolder is scanned into its own local FolderHistory index.");
    eprintln!("Default: --jobs 1; skip existing databases. --rescan: refresh while retaining manual decisions.");
}

fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("sub-folders-of") { usage(); anyhow::bail!("Expected sub-folders-of"); }
    let Some(root_arg) = args.next() else { usage(); anyhow::bail!("Missing main folder"); };
    let root = PathBuf::from(root_arg).canonicalize()?;
    if !root.is_dir() { anyhow::bail!("Not a directory: {}", root.display()); }
    let mut rescan = false;
    let mut jobs = 1usize;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--resume" => {}, // Skipping indexed folders is the default.
            "--rescan" => rescan = true,
            "--jobs" => {
                let value = args.next().ok_or_else(|| anyhow::anyhow!("--jobs requires a number"))?;
                jobs = value.parse::<usize>()
                    .map_err(|_| anyhow::anyhow!("--jobs must be a positive integer"))?;
                if jobs == 0 { anyhow::bail!("--jobs must be at least 1"); }
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
    let worker_count = jobs.min(folders.len());
    println!("FolderHistory batch: {} subfolders under {} ({} workers)",
        folders.len(), root.display(), worker_count);

    // Each folder is assigned exactly once. The core scanner opens a separate
    // SQLite index per folder, so workers do not share database connections.
    let queue = Arc::new(Mutex::new(
        folders.iter().cloned().enumerate().collect::<std::collections::VecDeque<_>>()
    ));
    let (sender, receiver) = mpsc::channel::<(usize, PathBuf, anyhow::Result<String>)>();
    let mut handles = Vec::new();
    for _ in 0..worker_count {
        let queue = Arc::clone(&queue);
        let sender = sender.clone();
        let queue_total = folders.len();
        handles.push(thread::spawn(move || {
            loop {
                let task = {
                    let mut pending = queue.lock().expect("batch work queue poisoned");
                    pending.pop_front()
                };
                let Some((index, folder)) = task else { break };
                eprintln!("[START {}/{}] {}", index + 1, queue_total, folder.display());
                let result = batch_scan_folder(&folder, rescan);
                if sender.send((index, folder, result)).is_err() { break; }
            }
        }));
    }
    drop(sender);

    let mut failed = 0usize;
    let mut completed = 0usize;
    for (index, folder, result) in receiver {
        completed += 1;
        match result {
            Ok(message) => println!("[DONE {}/{}] {}: {message}", index + 1, folders.len(), folder.display()),
            Err(error) => {
                failed += 1;
                eprintln!("[ERROR {}/{}] {}: {error:#}", index + 1, folders.len(), folder.display());
            }
        }
    }
    for handle in handles {
        if handle.join().is_err() {
            failed += 1;
            eprintln!("ERROR: a batch worker panicked");
        }
    }
    println!("\\nFinished: {completed}/{} folders, {failed} failed", folders.len());
    if completed != folders.len() || failed > 0 {
        anyhow::bail!("Batch incomplete or failed; rerun to resume");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
