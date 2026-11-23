//! Read-only diagnostic runner for FolderHistory's existing provenance engine.
//! All SQLite state lives in memory; no GUI or archive index is opened.
use anyhow::{Context, Result, bail};
use folder_history::core::engine::{scan, schema};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut yaml, mut script, mut root) = (None, None, None);
    let mut verbose = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--yaml" => yaml = Some(PathBuf::from(args.next().context("--yaml needs a file")?)),
            "--script" => script = Some(PathBuf::from(args.next().context("--script needs a file")?)),
            "--root" => root = Some(PathBuf::from(args.next().context("--root needs a directory")?)),
            "--verbose" => verbose = true,
            "--help" | "-h" => {
                println!("Usage: test_yaml_rules --yaml <file> --script <path> [--root <folder>] [--verbose]");
                return Ok(());
            }
            other => bail!("Unknown argument: {other}"),
        }
    }
    let yaml = yaml.context("Missing --yaml")?.canonicalize().context("YAML not found")?;
    let script = script.context("Missing --script")?.canonicalize().context("Script not found")?;
    let root = root.unwrap_or_else(|| script.parent().unwrap_or(Path::new(".")).to_path_buf())
        .canonicalize().context("Root not found")?;
    if !root.is_dir() { bail!("--root must be a directory"); }
    let rel = script.strip_prefix(&root).context("Script must be inside --root")?
        .to_string_lossy().replace('\\', "/");
    if !yaml.is_file() || !script.is_file() { bail!("--yaml and --script must be files"); }

    // Validate the supplied YAML independently before invoking the scanner.
    let contents = std::fs::read_to_string(&yaml)?;
    let doc: serde_yaml::Value = serde_yaml::from_str(&contents)
        .with_context(|| format!("Invalid YAML: {}", yaml.display()))?;
    if !matches!(doc.get("version").and_then(serde_yaml::Value::as_u64), Some(1 | 2)) {
        bail!("YAML must declare version: 1 or version: 2");
    }
    if doc.get("operations").is_none() && doc.get("functions").is_none() {
        bail!("YAML has no operations or functions");
    }

    // FOLDER_HISTORY_RULES is the existing external-rule entry point.
    // This single-threaded diagnostic process owns its environment.
    unsafe { std::env::set_var("FOLDER_HISTORY_RULES", &yaml); }
    let mut db = Connection::open_in_memory()?;
    schema(&db)?;
    println!("SCRIPT: {rel}\nROOT: {}\nYAML: {}", root.display(), yaml.display());
    scan(&mut db, &root, false, &AtomicBool::new(false), |message| {
        if verbose { eprintln!("[scan] {message}"); }
    })?;

    if verbose {
        let mut inventory = std::collections::HashSet::new();
        let mut files = db.prepare("SELECT path FROM artifacts WHERE kind='file'")?;
        let rows = files.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows { inventory.insert(row?); }
        println!("\nRULE DIAGNOSTICS:");
        for line in folder_history::core::provenance::diagnose_shell_rules(&root, &script, &rel, &inventory)? {
            println!("  {line}");
        }
    }

    let mut stmt = db.prepare("SELECT m.direction, a.path, p.pattern, m.location, m.evidence \
        FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id \
        JOIN artifacts src ON src.id=m.source_id \
        JOIN file_paths p ON p.id=m.filepath_id \
        WHERE src.path=?1 ORDER BY m.location, m.direction, a.path")?;
    let rows = stmt.query_map([&rel], |row| Ok((
        row.get::<_, String>(0)?, row.get::<_, String>(1)?,
        row.get::<_, String>(2)?, row.get::<_, i64>(3)?,
        row.get::<_, String>(4)?,
    )))?;
    let mut count = 0;
    println!("\nMATCHED:");
    for row in rows {
        let (direction, path, pattern, line, evidence) = row?;
        println!("  {direction:16} {path}  [line {line}; reference: {pattern}]");
        if verbose && !evidence.is_empty() { println!("    evidence: {evidence}"); }
        count += 1;
    }
    println!("\nSUMMARY: {count} matched connections for {rel}");
    if count == 0 { println!("No matches. Check YAML command names, variables, and indexed paths."); }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("ERROR: {error:#}");
        std::process::exit(1);
    }
}
