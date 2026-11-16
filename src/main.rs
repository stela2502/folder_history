mod provenance;
mod analysis_graph;
mod report;
use anyhow::{Context, Result};
use eframe::egui;
use regex::Regex;
use rusqlite::{params, Connection, OpenFlags};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::process::Command;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver}};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Clone, Debug)]
struct Artifact { id: i64, path: String, kind: String }
#[derive(Clone, Debug)]
struct Relation { id: i64, source: String, target: String, kind: String, evidence: String, manual: bool }
#[derive(Clone, Debug)]
struct Issue { id: i64, notebook: String, cell: i64, filename: String, category: String, decision: String, note: String }
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Tab { Search, Graph, Build, Spectator }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NavState { tab: Tab, selected: Option<i64> }

// The database stores archive-relative paths. Never pass these directly to the OS.
fn reveal_in_file_manager(root: &Path, relative: &str) -> io::Result<()> {
    let candidate = Path::new(relative);
    if candidate.is_absolute() || candidate.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unsafe archive-relative path"));
    }
    let absolute = root.join(candidate);
    let directory = if absolute.is_dir() { absolute } else {
        absolute.parent().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No parent directory"))?.to_path_buf()
    };
    if !directory.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("Directory does not exist: {}", directory.display())));
    }
    #[cfg(target_os = "windows")]
    let mut cmd = { let mut c = Command::new("explorer.exe"); c.arg(&directory); c };
    #[cfg(target_os = "macos")]
    let mut cmd = { let mut c = Command::new("open"); c.arg(&directory); c };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = { let mut c = Command::new("xdg-open"); c.arg(&directory); c };
    #[cfg(not(any(windows, unix)))]
    return Err(io::Error::new(io::ErrorKind::Unsupported, "No file manager launcher for this OS"));
    #[cfg(any(windows, unix))]
    { cmd.spawn().map(|_| ()) }
}

// Group physical FASTQ locations without confusing storage folders with experiments.
// Never collapse unrelated directory branches to the archive root or a storage wrapper.
fn fastq_source_folders(paths: &[&str]) -> Vec<(String, usize)> {
    let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
    for path in paths {
        if let Some((dir, _)) = path.rsplit_once('/') {
            *dirs.entry(dir.to_owned()).or_default() += 1;
        }
    }
    let mut branches: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for (dir, count) in dirs {
        let parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
        let first = parts.first().copied().unwrap_or("");
        // Distinguish physical Server/Local/Desktop branches even when their
        // logical sample names are identical.
        let wrapper = matches!(first.to_ascii_lowercase().as_str(),
            "server" | "local" | "shared" | "desktop" | "stefansdesktop" | "shamit" | "nas");
        let depth = if wrapper { 2 } else { 1 };
        let branch = parts.iter().take(depth).copied().collect::<Vec<_>>().join("/");
        branches.entry(branch).or_default().push((dir, count));
    }
    let mut result = Vec::new();
    for (branch, entries) in branches {
        let total = entries.iter().map(|(_, n)| n).sum();
        let mut common: Vec<&str> = entries[0].0.split('/').collect();
        for (dir, _) in entries.iter().skip(1) {
            let other: Vec<&str> = dir.split('/').collect();
            common.truncate(common.iter().zip(other.iter()).take_while(|(a,b)| a == b).count());
        }
        let common_dir = common.join("/");
        // A branch is already a meaningful, non-root boundary.
        result.push((if common_dir.is_empty() { branch } else { common_dir }, total));
    }
    result
}

enum ScanEvent { Progress(String), Finished(Result<ScanResult, String>), Resolved(Result<(i64, i64), String>) }
#[derive(Debug)]
struct ScanResult { files: usize, steps: usize, links: usize, warnings: usize, scripts: usize, parsed: usize }

struct App {
    root: Option<PathBuf>, db: Option<Connection>, writable: bool, index_path: Option<PathBuf>,
    tab: Tab, query: String, artifacts: Vec<Artifact>, relations: Vec<Relation>,
    selected: Option<i64>, status: String, from: String, to: String,
    relation_kind: String, note: String, show_all: bool, issues: Vec<Issue>, issue_filter: String, active_dependency: Option<(String,String)>, resolution_note: String, scan_summary: String,
    group_filter: String, group_names: HashMap<String, String>, group_name_edit: String,
    analysis_groups: Vec<analysis_graph::AnalysisGroup>, analysis_assignment: HashMap<String, String>,
    nav_history: Vec<NavState>, nav_cursor: usize,
    recent_folders: Vec<PathBuf>, include_hidden: bool, scan_events: Option<Receiver<ScanEvent>>, scan_cancel: Option<Arc<AtomicBool>>,
}
// A display hierarchy, not a replacement for the recorded provenance graph.
// Storage locations never participate in these labels.
fn meaningful_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty() && !matches!(part.to_ascii_lowercase().as_str(),
        "server" | "local" | "shared" | "desktop" | "stefansdesktop" | "shamit" | "nas")).collect()
}
fn experiment_label(path: &str, group_name: &str) -> String {
    let parts = meaningful_segments(path);
    if group_name.eq_ignore_ascii_case("spatial") {
        if let Some(part) = parts.iter().find(|part| part.to_ascii_lowercase().starts_with("singlecell_")) {
            return (*part).to_owned();
        }
    }
    // Other large collection roots: retain an informative nested directory.
    if let Some(pos) = parts.iter().position(|part| part.eq_ignore_ascii_case(group_name)) {
        if matches!(group_name.to_ascii_lowercase().as_str(), "spatial" | "external" | "data") {
            if let Some(part) = parts.get(pos+1) { return (*part).to_owned(); }
        }
    }
    group_name.to_owned()
}
fn sample_label(path: &str, experiment: &str) -> String {
    let parts = meaningful_segments(path);
    if let Some(pos) = parts.iter().position(|part| part.eq_ignore_ascii_case(experiment)) {
        if let Some(next) = parts.get(pos+1) {
            if !matches!(next.to_ascii_lowercase().as_str(), "outs" | "analysis" | "results" | "raw" | "data" | "fastq" | "fastqs") {
                return (*next).to_owned();
            }
        }
    }
    "Shared / unspecified sample".to_owned()
}
fn workflow_label(path: &str, kind: &str) -> &'static str {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".fastq.gz") || p.ends_with(".fq.gz") || p.ends_with(".fastq") || p.ends_with(".fq") { "Raw sequencing libraries" }
    else if p.ends_with(".cloupe") || p.contains("/loupe") { "Loupe / Cell Ranger visualization" }
    else if p.contains("/clustering/") || p.contains("/diffexp/") || p.contains("/analysis/") { "Clustering and differential expression" }
    else if p.contains("/filtered_feature_bc_matrix/") || p.contains("/raw_feature_bc_matrix/") || p.ends_with(".h5ad") || p.ends_with(".mtx.gz") { "Expression matrices" }
    else if kind == "script" || kind == "notebook" { "Scripts and notebooks" }
    else if p.ends_with(".png") || p.ends_with(".pdf") || p.ends_with(".svg") { "Figures" }
    else if p.ends_with(".csv") || p.ends_with(".tsv") { "Tables" }
    else { "Other files / pipeline metadata" }
}
fn fastq_library(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let mut stem = filename.to_owned();
    for ext in [".fastq.gz", ".fq.gz", ".fastq", ".fq"] {
        if stem.to_ascii_lowercase().ends_with(ext) { stem.truncate(stem.len()-ext.len()); break; }
    }
    // Standard Illumina read/lane suffixes. Avoid merging unrelated samples.
    let re = regex::Regex::new(r"(?i)(?:_S[0-9]+)?(?:_L[0-9]{3})?(?:_R[12]|_[12])(?:_[0-9]{3})?$").unwrap();
    re.replace(&stem, "").to_string()
}
fn provenance_step(path: &str, relations: &[Relation]) -> String {
    // A script is the unit of execution when the database actually records it.
    let mut scripts: Vec<&str> = relations.iter().filter(|r| r.source == path || r.target == path)
        .filter_map(|r| {
            let other = if r.source == path { r.target.as_str() } else { r.source.as_str() };
            let o = other.to_ascii_lowercase();
            if [".py", ".r", ".sh", ".ipynb", ".nf"].iter().any(|e| o.ends_with(e)) { Some(other) } else { None }
        }).collect();
    scripts.sort_unstable();
    scripts.dedup();
    if let Some(script) = scripts.first() { return format!("Input → {} → output", script.rsplit('/').next().unwrap_or(script)); }
    let lower = path.to_ascii_lowercase();
    if [".fastq", ".fq", ".fastq.gz", ".fq.gz"].iter().any(|e| lower.ends_with(e)) {
        return format!("FASTQ library: {}", fastq_library(path));
    }
    if lower.ends_with(".cloupe") { return "Cell Ranger → .cloupe".into(); }
    if let Some(pos) = lower.find("/clustering/") {
        let remainder = &path[pos+"/clustering/".len()..];
        return format!("Clustering → {}", remainder.split('/').next().unwrap_or("outputs"));
    }
    if let Some(pos) = lower.find("/diffexp/") {
        let remainder = &path[pos+"/diffexp/".len()..];
        return format!("Differential expression → {}", remainder.split('/').next().unwrap_or("outputs"));
    }
    // Unknown producer: keep files together as an *unresolved* operation, not a fabricated link.
    "Unresolved / no recorded producer".into()
}

impl Default for App {
    fn default() -> Self {
        Self { root: None, db: None, writable: false, index_path: None, tab: Tab::Search,
            query: String::new(), artifacts: vec![], relations: vec![], selected: None,
            status: "Open a research folder to begin.".into(), from: String::new(),
            to: String::new(), relation_kind: "derived_from".into(), note: String::new(),
            show_all: false, issues: vec![], issue_filter: "all".into(), active_dependency: None, resolution_note: String::new(), scan_summary: String::new(),
            group_filter: String::new(), group_names: HashMap::new(), group_name_edit: String::new(),
            analysis_groups: Vec::new(), analysis_assignment: HashMap::new(),
            nav_history: vec![], nav_cursor: 0,
            recent_folders: load_recent_folders(), include_hidden: false, scan_events: None, scan_cancel: None }
    }
}

// Plain UTF-8 file: one absolute folder path per line, newest first.
// This list contains only folder names, never data or manual resolutions.
fn recent_folders_file() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".folder_history/recent_folders.txt"))
}
fn load_recent_folders() -> Vec<PathBuf> {
    let Some(path) = recent_folders_file() else { return Vec::new(); };
    std::fs::read_to_string(path).unwrap_or_default().lines()
        .filter(|line| !line.is_empty()).take(5).map(PathBuf::from).collect()
}
fn save_recent_folders(folders: &[PathBuf]) -> Result<()> {
    let Some(path) = recent_folders_file() else { return Ok(()); };
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let contents = folders.iter().map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>().join("\n") + "\n";
    // Atomic replacement avoids a truncated list if the application exits during writing.
    let temporary = path.with_extension("txt.tmp");
    std::fs::write(&temporary, contents)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS analysis_names (source_folder TEXT PRIMARY KEY, display_name TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS artifacts (
            id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
            size INTEGER, modified INTEGER, created INTEGER);
        CREATE TABLE IF NOT EXISTS relations (
            id INTEGER PRIMARY KEY, source TEXT NOT NULL, target TEXT NOT NULL,
            kind TEXT NOT NULL, evidence TEXT NOT NULL, manual INTEGER NOT NULL DEFAULT 0,
            UNIQUE(source,target,kind,evidence));
        CREATE INDEX IF NOT EXISTS rel_source ON relations(source);
        CREATE INDEX IF NOT EXISTS rel_target ON relations(target);
        CREATE TABLE IF NOT EXISTS issues (
            id INTEGER PRIMARY KEY, notebook TEXT NOT NULL, cell INTEGER NOT NULL,
            filename TEXT NOT NULL, category TEXT NOT NULL,
            decision TEXT NOT NULL DEFAULT 'open', note TEXT NOT NULL DEFAULT '',
            UNIQUE(notebook,cell,filename,category));
        CREATE TABLE IF NOT EXISTS steps (
            id INTEGER PRIMARY KEY, source TEXT NOT NULL, ordinal INTEGER NOT NULL,
            command TEXT NOT NULL, UNIQUE(source,ordinal));
        CREATE TABLE IF NOT EXISTS step_io (
            step_id INTEGER NOT NULL, artifact TEXT NOT NULL, direction TEXT NOT NULL,
            PRIMARY KEY(step_id,artifact,direction));
        CREATE TABLE IF NOT EXISTS scans (
            id INTEGER PRIMARY KEY, completed INTEGER NOT NULL DEFAULT 0,
            files INTEGER NOT NULL DEFAULT 0, steps INTEGER NOT NULL DEFAULT 0,
            links INTEGER NOT NULL DEFAULT 0);
        PRAGMA user_version=2;")?;
    // Existing v2 indexes are upgraded in place without discarding manual links.
    let has_created: bool = db.prepare("PRAGMA table_info(artifacts)")?
        .query_map([], |r| r.get::<_,String>(1))?
        .flatten().any(|name| name == "created");
    if !has_created { db.execute("ALTER TABLE artifacts ADD COLUMN created INTEGER", [])?; }
    db.execute_batch("CREATE TABLE IF NOT EXISTS timestamp_assessments (
        folder TEXT PRIMARY KEY, files INTEGER NOT NULL, correlation REAL,
        transfer_mb_s REAL, suspected_copy INTEGER NOT NULL, reason TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS notebook_observations (
            notebook TEXT NOT NULL, cell INTEGER NOT NULL, expression TEXT NOT NULL,
            observed_path TEXT NOT NULL, evidence TEXT NOT NULL,
            PRIMARY KEY(notebook,cell,expression,observed_path));
        CREATE TABLE IF NOT EXISTS notebook_expressions (
            notebook TEXT NOT NULL, cell INTEGER NOT NULL, expression TEXT NOT NULL,
            has_output INTEGER NOT NULL, PRIMARY KEY(notebook,cell,expression));
        CREATE TABLE IF NOT EXISTS notebook_candidates (
            notebook TEXT NOT NULL, cell INTEGER NOT NULL, expression TEXT NOT NULL,
            candidate_path TEXT NOT NULL, source_notebook TEXT NOT NULL, source_cell INTEGER NOT NULL,
            PRIMARY KEY(notebook,cell,expression,candidate_path,source_notebook,source_cell));
        CREATE TABLE IF NOT EXISTS symlink_findings (
            path TEXT PRIMARY KEY, target TEXT NOT NULL, category TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS inferred_links (
        source TEXT NOT NULL, target TEXT NOT NULL, reason TEXT NOT NULL,
        PRIMARY KEY(source,target,reason));")?;
    Ok(())
}
fn kind(path: &Path) -> &'static str {
    let name = path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    if name.ends_with(".ipynb") { "notebook" }
    else if name.ends_with(".fastq.gz") || name.ends_with(".fq.gz") || name.ends_with(".fastq") { "fastq" }
    else if name.ends_with(".h5ad") { "h5ad" }
    else if name.ends_with(".h5") || name.ends_with(".mtx.gz") { "matrix" }
    else if name.ends_with(".py") || name.ends_with(".r") || name.ends_with(".sh")
        || name.ends_with(".bash") || name.ends_with(".ps1") || name.ends_with(".nf")
        || name.ends_with(".smk") || name == "makefile" || name == "snakefile" { "script" }
    else if name.ends_with(".pdf") || name.ends_with(".png") || name.ends_with(".svg") { "figure" }
    else if name.ends_with(".tsv") || name.ends_with(".csv") { "table" }
    else if name.ends_with(".log") || name.ends_with(".out") || name.ends_with(".err") { "log" }
    else if name.ends_with(".yaml") || name.ends_with(".yml") || name.ends_with(".toml") || name.ends_with(".json") { "config" }
    else { "file" }
}
fn normalized(path: &Path) -> String { path.to_string_lossy().replace('\\', "/") }
fn add_relation(db: &Connection, source: &str, target: &str, kind: &str, evidence: &str, manual: bool) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO relations(source,target,kind,evidence,manual) VALUES (?1,?2,?3,?4,?5)",
        params![source,target,kind,evidence,manual as i32])?;
    Ok(())
}
fn safe_relative(input: &str) -> Option<String> {
    let value = input.replace('\\', "/");
    if value.starts_with('/') || value.starts_with("//") || value.contains(':') { return None; }
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part { "" | "." => {}, ".." => { parts.pop()?; }, _ => parts.push(part) }
    }
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}
fn reference_name(reference: &str) -> String {
    // Never persist historical absolute paths or parent directories in the index.
    reference.replace('\\', "/").rsplit('/').next().unwrap_or("").to_owned()
}
fn resolve_reference(reference: &str, notebook: &str, paths: &HashSet<String>) -> (Option<String>, bool, bool) {
    let value = reference.replace('\\', "/");
    let external = value.starts_with('/') || value.starts_with("//") || value.contains(':') || value.split('/').any(|p| p == "..");
    if !external {
        let parent = Path::new(notebook).parent().unwrap_or(Path::new(""));
        if let Some(relative) = safe_relative(&normalized(&parent.join(&value))) {
            if paths.contains(&relative) { return (Some(relative), false, false); }
        }
        if let Some(relative) = safe_relative(&value) {
            if paths.contains(&relative) { return (Some(relative), false, false); }
        }
    }
    let basename = reference_name(reference);
    let matches: Vec<_> = paths.iter().filter(|p| p.rsplit('/').next() == Some(basename.as_str())).collect();
    if matches.len() == 1 && !external { (Some(matches[0].clone()), false, false) }
    else { (None, external, matches.len() > 1) }
}
fn record_issue(db: &Connection, notebook: &str, cell: usize, filename: &str, category: &str) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO issues(notebook,cell,filename,category) VALUES (?1,?2,?3,?4)",
        params![notebook, cell as i64, filename, category])?;
    Ok(())
}
// Output is evidence of a *saved execution*, never proof of the current code's result.
fn cell_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => String::new(),
    }
}
fn notebook_output_text(cell: &serde_json::Value) -> String {
    let mut result = String::new();
    if let Some(outputs) = cell.get("outputs").and_then(|v| v.as_array()) {
        for output in outputs {
            result.push_str(&cell_text(output.get("text")));
            if let Some(data) = output.get("data") {
                result.push_str(&cell_text(data.get("text/plain")));
            }
            result.push('\n');
        }
    }
    result
}
fn discover_notebook_observations(db: &Connection, root: &Path, rel: &str) -> Result<()> {
    let content = std::fs::read_to_string(root.join(rel))?;
    let notebook: serde_json::Value = serde_json::from_str(&content)?;
    let Some(cells) = notebook.get("cells").and_then(|v| v.as_array()) else { return Ok(()); };
    // Generic path discovery: no knowledge of particular scientific packages.
    let glob = Regex::new(r#"(?:Sys\.glob|glob\.glob)\s*\(\s*['\"]([^'\"]+)['\"]"#)?;
    let output_path = Regex::new(r#"(?:[A-Za-z]:[\\/]|/|\.\.?/)[^\s\"'<>\[\],;]+"#)?;
    for (idx, cell) in cells.iter().enumerate() {
        if cell.get("cell_type").and_then(|v| v.as_str()) != Some("code") { continue; }
        let source = cell_text(cell.get("source"));
        let output = notebook_output_text(cell);
        for cap in glob.captures_iter(&source) {
            let expression = cap[1].to_string();
            db.execute("INSERT OR IGNORE INTO notebook_expressions(notebook,cell,expression,has_output) VALUES (?1,?2,?3,?4)",
                params![rel,(idx+1) as i64,expression,(!output.trim().is_empty()) as i32])?;
            for candidate in output_path.find_iter(&output) {
                let observed = candidate.as_str().trim_end_matches([')', ':', '.', '\r']);
                // Require the output to resemble the glob's final component.
                let stem = expression.rsplit('/').next().unwrap_or("")
                    .split(['*','?','[']).next().unwrap_or("");
                if stem.len() < 3 || !observed.rsplit('/').next().unwrap_or("").starts_with(stem) { continue; }
                db.execute("INSERT OR IGNORE INTO notebook_observations(notebook,cell,expression,observed_path,evidence) VALUES (?1,?2,?3,?4,'saved_cell_output')",
                    params![rel,(idx+1) as i64,expression,observed])?;
            }
        }
    }
    Ok(())
}
fn infer_notebook_candidates(db: &Connection) -> Result<()> {
    // Cross-notebook observations are candidates, NOT automatically confirmed graph edges.
    db.execute_batch("DELETE FROM notebook_candidates;
        INSERT OR IGNORE INTO notebook_candidates(notebook,cell,expression,candidate_path,source_notebook,source_cell)
        SELECT dst.notebook,dst.cell,dst.expression,src.observed_path,src.notebook,src.cell
        FROM notebook_expressions dst JOIN notebook_observations src
          ON dst.expression=src.expression AND dst.notebook<>src.notebook
        WHERE dst.has_output=0;")?;
    Ok(())
}
// Recompute derived candidates using only evidence already persisted in SQLite.
// No filesystem traversal, parsing, or changes to manual decisions/relationships.
fn relink_cached_evidence(db: &mut Connection) -> Result<(i64, i64)> {
    let tx = db.transaction()?;
    infer_notebook_candidates(&tx)?;
    tx.execute("DELETE FROM inferred_links WHERE reason='script-order candidate'", [])?;
    infer_script_chains(&tx)?;
    let notebooks = tx.query_row("SELECT COUNT(*) FROM notebook_candidates", [], |r| r.get(0))?;
    let chains = tx.query_row("SELECT COUNT(*) FROM inferred_links WHERE reason='script-order candidate'", [], |r| r.get(0))?;
    tx.commit()?;
    Ok((notebooks, chains))
}

fn global_registry_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".folder_history/external_links.db"))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".folder_history/external_links.db")))
}
fn save_global_external_link(name: &str, url: &str) -> Result<()> {
    // Only explicit URL resolutions are globally reusable. Never save private local paths.
    if !(url.starts_with("https://") || url.starts_with("http://")) { return Ok(()); }
    let Some(path) = global_registry_path() else { return Ok(()); };
    std::fs::create_dir_all(path.parent().context("Registry parent missing")?)?;
    let db = Connection::open(path)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS resources (
        filename TEXT PRIMARY KEY, url TEXT NOT NULL, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
    db.execute("INSERT INTO resources(filename,url) VALUES (?1,?2)
        ON CONFLICT(filename) DO UPDATE SET url=excluded.url,updated_at=CURRENT_TIMESTAMP", params![name,url])?;
    Ok(())
}
fn lookup_global_external_link(name: &str) -> Option<String> {
    let path = global_registry_path()?;
    let db = Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.query_row("SELECT url FROM resources WHERE filename=?1",params![name],|r|r.get(0)).ok()
}
fn parse_notebook(db: &Connection, root: &Path, rel: &str, paths: &HashSet<String>) -> Result<usize> {
    let content = std::fs::read_to_string(root.join(rel))?;
    let notebook: serde_json::Value = serde_json::from_str(&content)?;
    let Some(cells) = notebook.get("cells").and_then(|v| v.as_array()) else { return Ok(0); };
    let assignment = Regex::new(r#"(?m)^\s*(raw|ofile)\s*=\s*['\"]([^'\"\r\n]+)['\"]"#)?;
    let read_call = Regex::new(r#"(?:read_h5ad|read_10x_mtx|read_10x_h5)\s*\(\s*(raw|ofile|['\"][^'\"]+['\"])"#)?;
    let write_call = Regex::new(r#"(?:write_h5ad|write)\s*\(\s*(raw|ofile|['\"][^'\"]+['\"])"#)?;
    let mut vars: HashMap<String,String> = HashMap::new();
    let mut count = 0;
    for (idx, cell) in cells.iter().enumerate() {
        if cell.get("cell_type").and_then(|v| v.as_str()) != Some("code") { continue; }
        let source = match cell.get("source") {
            Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect::<String>(),
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => String::new(),
        };
        for cap in assignment.captures_iter(&source) { vars.insert(cap[1].to_string(), cap[2].to_string()); }
        for (re, edge_kind) in [(&read_call, "reads"), (&write_call, "writes")] {
            for cap in re.captures_iter(&source) {
                let arg = cap[1].trim_matches(['\'', '"']);
                let file = vars.get(arg).map(String::as_str).unwrap_or(arg);
                let evidence = format!("{}: cell {}: {} reference", rel, idx + 1, edge_kind);
                let filename = reference_name(file);
                let (resolved, external, ambiguous) = resolve_reference(file, rel, paths);
                if external { record_issue(db, rel, idx + 1, &filename, "external")?; }
                if ambiguous { record_issue(db, rel, idx + 1, &filename, "ambiguous")?; }
                if let Some(target) = resolved {
                    add_relation(db, rel, &target, edge_kind, &evidence, false)?;
                    count += 1;
                } else {
                    // Keep unresolved references visible instead of inventing a match.
                    if !external && !ambiguous { record_issue(db, rel, idx + 1, &filename, "unresolved")?; }
                    // No dangling external file paths are inserted into the provenance graph.
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

// Conservative extraction: these are *candidate* operations, not proof of execution.
// Never persist original command strings: they may contain external paths or secrets.
#[derive(Debug)]
struct Operation { reads: Vec<String>, writes: Vec<String> }
fn shell_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in line.chars() {
        if escape { word.push(c); escape=false; continue; }
        if c == '\\' && quote != Some('\'') { escape=true; continue; }
        if let Some(q) = quote {
            if c == q { quote=None; } else { word.push(c); }
        } else if c == '\'' || c == '"' { quote=Some(c); }
        else if c.is_whitespace() {
            if !word.is_empty() { tokens.push(std::mem::take(&mut word)); }
        } else if c == '#' && word.is_empty() { break; }
        else { word.push(c); }
    }
    if !word.is_empty() { tokens.push(word); }
    tokens
}
fn plausible_reference(value: &str) -> bool {
    let v = value.trim();
    !v.is_empty() && !v.starts_with('&') && !v.starts_with('<') && !v.starts_with('>')
        && !v.starts_with('=') && !v.starts_with(']') && !v.ends_with(')')
        && !v.contains(';') && !v.contains('(') && !v.contains(')')
        && !["null", "none", "true", "false"].contains(&v.to_ascii_lowercase().as_str())
}
fn looks_like_path(value: &str) -> bool {
    let v=value.to_ascii_lowercase();
    [".bam",".sam",".cram",".fastq",".fq",".fastq.gz",".fq.gz",".h5ad",".h5",".tsv",".csv",".mtx",".mtx.gz",".pdf",".png",".svg",".vcf",".vcf.gz",".json",".txt",".fasta",".fa",".gtf",".gz"].iter().any(|e| v.ends_with(e))
}
fn extract_shell(line: &str) -> Option<Operation> {
    let tokens=shell_tokens(line);
    if tokens.is_empty() || tokens[0].starts_with('#') { return None; }
    let mut reads=Vec::new(); let mut writes=Vec::new();
    let mut i=0;
    while i < tokens.len() {
        let t=&tokens[i];
        let lower=t.to_ascii_lowercase();
        let output_flag=["-o","--output","--out","--outdir","--output-dir","--output-file","--output-file-name","--out-file","--out-bam","--out-h5ad"];
        let input_flag=["-i","--input","--in","--bam","--fastq","--reads","--reference","--genome","--annotation","--input-file"];
        if (output_flag.contains(&lower.as_str()) || input_flag.contains(&lower.as_str())) && i+1<tokens.len() {
            let val=tokens[i+1].clone();
            if output_flag.contains(&lower.as_str()) { writes.push(val); } else { reads.push(val); }
            i+=2; continue;
        }
        if let Some((flag,val))=t.split_once('=') {
            if output_flag.contains(&flag.to_ascii_lowercase().as_str()) { writes.push(val.to_owned()); }
            else if input_flag.contains(&flag.to_ascii_lowercase().as_str()) { reads.push(val.to_owned()); }
        }
        if [">",">>","1>","2>"].contains(&t.as_str()) && i+1<tokens.len() {
            writes.push(tokens[i+1].clone()); i+=2; continue;
        }
        if t=="<" && i+1<tokens.len() { reads.push(tokens[i+1].clone()); i+=2; continue; }
        if let Some(rest)=t.strip_prefix(">>").or_else(||t.strip_prefix('>')) {
            if !rest.is_empty() { writes.push(rest.to_owned()); }
        }
        i+=1;
    }
    // samtools sort -o out.bam in.bam; common positional inputs
    if !tokens.iter().any(|t| t=="--input" || t=="-i") {
        for t in tokens.iter().skip(1) {
            if !t.starts_with('-') && looks_like_path(t) && !writes.contains(t) && !reads.contains(t) {
                reads.push(t.clone());
            }
        }
    }
    if reads.is_empty() && writes.is_empty() { None } else { Some(Operation{reads,writes}) }
}
fn parse_script(db: &Connection, root: &Path, rel: &str, paths: &HashSet<String>) -> Result<(usize,usize)> {
    let metadata=std::fs::metadata(root.join(rel))?;
    if metadata.len()>8*1024*1024 { return Ok((0,0)); }
    let content=std::fs::read_to_string(root.join(rel))?;
    let mut steps=0; let mut links=0;
    for (idx,line) in content.lines().enumerate() {
        let Some(op)=extract_shell(line) else { continue; };
        steps+=1;
        // Sanitized operation label: no arbitrary command text or external paths.
        db.execute("INSERT OR REPLACE INTO steps(source,ordinal,command) VALUES (?1,?2,'shell operation')",params![rel,(idx+1) as i64])?;
        let step_id:i64=db.query_row("SELECT id FROM steps WHERE source=?1 AND ordinal=?2",params![rel,(idx+1) as i64],|r|r.get(0))?;
        for (values,direction) in [(op.reads,"reads"),(op.writes,"writes")] {
            for value in values {
                if value.contains('$') || value.contains('*') || value.contains('{') { continue; }
                let filename=reference_name(&value);
                if filename.is_empty() || !plausible_reference(&value) { continue; }
                let (resolved,external,ambiguous)=resolve_reference(&value,rel,paths);
                if external { record_issue(db,rel,idx+1,&filename,"external")?; }
                if ambiguous { record_issue(db,rel,idx+1,&filename,"ambiguous")?; }
                if let Some(path)=resolved {
                    db.execute("INSERT OR IGNORE INTO step_io(step_id,artifact,direction) VALUES (?1,?2,?3)",params![step_id,path,direction])?;
                    add_relation(db,rel,&path,direction,&format!("{}: line {}: candidate {}",rel,idx+1,direction),false)?;
                    links+=1;
                } else if !external && !ambiguous {
                    record_issue(db,rel,idx+1,&filename,"unresolved")?;
                }
            }
        }
    }
    Ok((steps,links))
}

fn excluded_dir(name: &str, _hidden: bool) -> bool {
    let lower = name.to_ascii_lowercase();
    // Never descend into dot-directories or generated workflow/build caches.
    // This applies at every depth, not only at the archive root.
    name.starts_with('.') || ["work", "__pycache__", "target", "venv", "node_modules", "lost+found"].contains(&lower.as_str())
}
fn excluded_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // Generated/hidden files are not user-authored analysis scripts.
    name.starts_with('.') || lower.ends_with('~') || lower.ends_with(".swp") || lower.ends_with(".swo")
        || lower.ends_with("-checkpoint.py") || lower.ends_with("-checkpoint.sh")
}
// A candidate is only made when a script explicitly mentions both artifacts.
// We never infer a dependency from timestamps alone or from mere folder proximity.
fn infer_script_chains(db: &Connection) -> Result<()> {
    let mut stmt=db.prepare("SELECT s.source,s.ordinal,io.artifact,io.direction FROM steps s JOIN step_io io ON s.id=io.step_id ORDER BY s.source,s.ordinal")?;
    let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?;
    let mut previous_script=String::new();
    let mut inputs=HashSet::<String>::new();
    let mut pending=Vec::<(String,String)>::new();
    for row in rows {
        let (script,_,artifact,direction)=row?;
        if script != previous_script { inputs.clear(); previous_script=script; }
        if direction=="reads" { inputs.insert(artifact); }
        else if direction=="writes" {
            for input in &inputs { if input!=&artifact { pending.push((input.clone(),artifact.clone())); } }
            // A later write should not inherit every earlier input forever.
            inputs.clear();
        }
    }
    for (input,output) in pending {
        db.execute("INSERT OR IGNORE INTO inferred_links(source,target,reason) VALUES (?1,?2,'script-order candidate')",params![input,output])?;
    }
    Ok(())
}
// A file-copy sequence may show time gaps proportional to the preceding file size.
// Require substantial sample size, high positive correlation, and older preserved
// modification times. This flags suspicious metadata; it does not prove copying.
fn assess_timestamps(db: &Connection, files: &[(String,String,i64,Option<i64>,Option<i64>)]) -> Result<()> {
    let mut groups: HashMap<String,Vec<(i64,i64,i64)>>=HashMap::new();
    for (path,_,size,modified,created) in files {
        if let (Some(m),Some(c))=(modified,created) {
            if *size>0 { let folder=path.rsplit_once('/').map(|p|p.0).unwrap_or("").to_owned();
                groups.entry(folder).or_default().push((*c,*m,*size)); }
        }
    }
    for (folder,mut items) in groups {
        if items.len()<8 { continue; }
        items.sort_by_key(|v|v.0);
        let mut xy=Vec::new();
        let mut older=0usize;
        for &(c,m,_) in &items { if c-m>86400 { older+=1; } }
        for w in items.windows(2) {
            let dt=w[1].0-w[0].0;
            if (1..=7200).contains(&dt) { xy.push((w[0].2 as f64 / 1e6,dt as f64)); }
        }
        if xy.len()<7 { continue; }
        let n=xy.len() as f64;
        let (sx,sy)=xy.iter().fold((0.,0.),|(a,b),(x,y)|(a+x,b+y));
        let (mx,my)=(sx/n,sy/n);
        let (cov,vx,vy)=xy.iter().fold((0.,0.,0.),|(c,xv,yv),(x,y)| (c+(x-mx)*(y-my),xv+(x-mx).powi(2),yv+(y-my).powi(2)));
        if vx<=0. || vy<=0. { continue; }
        let r=cov/(vx*vy).sqrt();
        let speed=if cov>0. { vx/cov } else { 0. };
        let suspected=xy.len()>=10 && r>0.85 && speed>=1. && speed<=2000. && older*2>=items.len();
        db.execute("INSERT OR REPLACE INTO timestamp_assessments(folder,files,correlation,transfer_mb_s,suspected_copy,reason) VALUES (?1,?2,?3,?4,?5,'creation gaps correlate with preceding file sizes; modification dates predate creation')",
            params![folder,items.len() as i64,r,speed,suspected as i32])?;
    }
    Ok(())
}

fn scan(db: &mut Connection, root: &Path, hidden: bool, cancel: &AtomicBool,
        mut progress: impl FnMut(String)) -> Result<ScanResult> {
    let mut files = Vec::new();
    let mut warnings = 0usize;
    for entry in WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| {
        e.depth() == 0 || !e.file_type().is_dir() || !excluded_dir(&e.file_name().to_string_lossy(), hidden)
    }) {
        if cancel.load(Ordering::Relaxed) { anyhow::bail!("Scan cancelled"); }
        let entry = match entry { Ok(e) => e, Err(e) => { warnings+=1; if warnings <= 10 { progress(format!("Cannot access directory entry: {}", e)); } continue; } };
        if !entry.file_type().is_file() || entry.path_is_symlink() || excluded_file(&entry.file_name().to_string_lossy()) { continue; }
        let path = entry.path();
        let rel = normalized(path.strip_prefix(root)?);
        let metadata = match entry.metadata() { Ok(m) => m, Err(_) => { warnings+=1; continue; } };
        let ctime = metadata.created().ok().and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok()).map(|v| v.as_secs() as i64);
        let mtime = metadata.modified().ok().and_then(|v| v.duration_since(std::time::UNIX_EPOCH).ok()).map(|v| v.as_secs() as i64);
        files.push((rel, kind(path).to_owned(), metadata.len() as i64, mtime, ctime));
        if files.len() % 500 == 0 { progress(format!("Inventory: {} files ({} inaccessible)",files.len(),warnings)); }
    }
    progress(format!("Inventory complete: {} files; extracting provenance...",files.len()));
    // Candidate references can identify either files or directories. Keep the
    // artifact inventory file-only, but match script arguments against both.
    let mut paths: HashSet<String> = files.iter().map(|f| f.0.clone()).collect();
    for entry in WalkDir::new(root).into_iter().filter_entry(|e| {
        e.depth() == 0 || !e.file_type().is_dir()
            || !excluded_dir(&e.file_name().to_string_lossy(), hidden)
    }) {
        let entry = match entry { Ok(e) => e, Err(_) => continue };
        if entry.depth() > 0 && entry.file_type().is_dir() && !entry.path_is_symlink() {
            if let Ok(rel) = entry.path().strip_prefix(root) {
                paths.insert(normalized(rel));
            }
        }
    }
    let tx = db.transaction()?;
    for (path, kind, size, modified, created) in &files {
        if cancel.load(Ordering::Relaxed) { anyhow::bail!("Scan cancelled"); }
        tx.execute("INSERT INTO artifacts(path,kind,size,modified,created) VALUES (?1,?2,?3,?4,?5)
            ON CONFLICT(path) DO UPDATE SET kind=excluded.kind,size=excluded.size,modified=excluded.modified,created=excluded.created",
            params![path,kind,size,modified,created])?;
    }
    // Drop stale inventory rows from previous scans (including previously indexed
    // checkpoint/work files). Keep manually curated relations untouched.
    {
        let mut stmt = tx.prepare("SELECT path FROM artifacts")?;
        let stored = stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for old in stored {
            if !paths.contains(&old) {
                tx.execute("DELETE FROM artifacts WHERE path=?1", params![old])?;
            }
        }
    }
    tx.execute("DELETE FROM notebook_observations", [])?;
    tx.execute("DELETE FROM notebook_expressions", [])?;
    tx.execute("DELETE FROM notebook_candidates", [])?;
    tx.execute("DELETE FROM symlink_findings", [])?;
    for entry in WalkDir::new(root).follow_links(false).into_iter().filter_map(Result::ok) {
        if !entry.path_is_symlink() { continue; }
        let Ok(relative) = entry.path().strip_prefix(root) else { continue; };
        let Ok(target) = std::fs::read_link(entry.path()) else { continue; };
        let resolved = if target.is_absolute() { target.clone() } else {
            entry.path().parent().unwrap_or(root).join(&target)
        };
        let category = if !resolved.exists() { "broken" }
            else if resolved.canonicalize().ok().is_some_and(|p| p.starts_with(root.canonicalize().unwrap_or_else(|_|root.to_path_buf()))) { "internal" }
            else { "external" };
        tx.execute("INSERT OR REPLACE INTO symlink_findings(path,target,category) VALUES (?1,?2,?3)",
            params![normalized(relative),normalized(&target),category])?;
    }
    tx.execute("DELETE FROM inferred_links", [])?;
    tx.execute("DELETE FROM timestamp_assessments", [])?;
    tx.execute("DELETE FROM relations WHERE manual=0", [])?;
    tx.execute("DELETE FROM step_io", [])?;
    tx.execute("DELETE FROM steps", [])?;
    tx.execute("DELETE FROM issues WHERE decision='open'", [])?;
    let mut links = 0;
    let mut steps = 0;
    let mut scripts = 0;
    let mut parsed = 0;
    for (path, file_kind, _, _, _) in &files {
        if cancel.load(Ordering::Relaxed) { anyhow::bail!("Scan cancelled"); }
        if file_kind == "notebook" {
            if let Err(e) = provenance::parse(&tx, root, path, &paths).map(|n| links+=n) {
                warnings+=1; if warnings<=10 { progress(format!("Notebook parse warning ({}): {e}", path)); }
            }
        } else if file_kind == "script" {
            scripts+=1;
            if path.to_ascii_lowercase().ends_with(".sh") || path.to_ascii_lowercase().ends_with(".bash") || path.to_ascii_lowercase().ends_with(".ps1") {
                match parse_script(&tx,root,path,&paths) {
                    Ok((s,n)) => { steps+=s; links+=n; parsed+=1; if let Ok(extra)=provenance::parse(&tx,root,path,&paths){links+=extra;} },
                    Err(e) => { warnings+=1; if warnings<=10 { progress(format!("Script parse warning ({}): {e}",path)); } }
                }
            }
            if path.to_ascii_lowercase().ends_with(".py") || path.to_ascii_lowercase().ends_with(".r") {
                match provenance::parse(&tx,root,path,&paths) {
                    Ok(n) => {links+=n;parsed+=1;},
                    Err(e) => {warnings+=1;if warnings<=10 {progress(format!("Script parse warning ({}): {e}",path));}}
                }
            }
            if scripts % 20 == 0 { progress(format!("Scripts: {scripts} found, {parsed} parsed, {steps} steps, {links} links")); }
        }
    }
    for (path, file_kind, _, _, _) in &files {
        if file_kind == "notebook" {
            if let Err(e) = discover_notebook_observations(&tx,root,path) {
                warnings+=1;
                if warnings<=10 { progress(format!("Notebook output warning ({path}): {e}")); }
            }
        }
    }
    infer_notebook_candidates(&tx)?;
    assess_timestamps(&tx, &files)?;
    infer_script_chains(&tx)?;
    tx.execute("INSERT INTO scans(completed,files,steps,links) VALUES (1,?1,?2,?3)",params![files.len() as i64,steps as i64,links as i64])?;
    tx.commit()?;
    Ok(ScanResult { files:files.len(), steps, links, warnings, scripts, parsed })
}
// A researcher's index travels with their data. The global home directory is
// reserved for reusable links and recent-folder preferences.
fn local_index_path(root: &Path) -> PathBuf {
    root.join(".folder_history").join("history.sqlite")
}

// Previous releases used a machine-local hashed index. Import it only when the
// portable index is absent; never overwrite a researcher's existing history.
fn legacy_index_path(root: &Path) -> PathBuf {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in normalized(&canonical).as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("FolderHistory").join(format!("{hash:016x}")).join("folder-history.sqlite")
}

fn import_existing_index(root: &Path, dest: &Path) -> Result<()> {
    if dest.exists() { return Ok(()); }
    let previous_archive = root.join(".folder-history/history.sqlite");
    let legacy = legacy_index_path(root);
    let source = [previous_archive, legacy].into_iter().find(|p| p.is_file());
    if let Some(source) = source {
        if let Some(parent) = dest.parent() { std::fs::create_dir_all(parent)?; }
        // SQLite's VACUUM INTO creates a consistent snapshot even if the source
        // database uses WAL. Never modify or remove the original index.
        let old = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let destination = dest.to_string_lossy().replace('\'', "''");
        old.execute_batch(&format!("VACUUM INTO '{destination}'"))
            .with_context(|| format!("Importing {} into {}", source.display(), dest.display()))?;
    }
    Ok(())
}

fn report_safe(value: &str) -> String {
    // Only archive-relative paths or basenames are allowed in shared diagnostics.
    // Reject path-like tokens with absolute roots, Windows drives, or parent traversal.
    value.split_whitespace().map(|token| {
        let clean = token.trim_matches(|c: char| matches!(c, '`' | '\"' | '\'' | '(' | ')' | ',' | ';'));
        let normalized = clean.replace('\\', "/");
        if normalized.starts_with('/') || normalized.starts_with("//")
            || normalized.contains(':') || normalized.split('/').any(|p| p=="..") {
            "[redacted-path]".to_owned()
        } else { token.to_owned() }
    }).collect::<Vec<_>>().join(" ").replace('\n'," ").replace('\r'," ")
}

impl App {
    fn open(&mut self, root: PathBuf, analyze: bool) {
        let path = local_index_path(&root);
        let result: Result<Connection> = (|| {
            if !root.is_dir() { anyhow::bail!("Folder does not exist: {}", root.display()); }
            if analyze {
                import_existing_index(&root, &path)?;
                if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
            } else if !path.is_file() {
                // Read the old portable index directly in Spectator mode. Importing
                // is a separate, explicit Build operation and must not be required
                // merely to browse an archive.
                let old = root.join(".folder-history/history.sqlite");
                if !old.is_file() {
                    anyhow::bail!("No FolderHistory database found in .folder_history or .folder-history");
                }
            }
            let open_path = if !analyze && !path.is_file() {
                root.join(".folder-history/history.sqlite")
            } else { path.clone() };
            let flags = if analyze {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
            } else { OpenFlags::SQLITE_OPEN_READ_ONLY };
            let db = Connection::open_with_flags(&open_path, flags)
                .with_context(|| format!("Opening {}", open_path.display()))?;
            if analyze { schema(&db)?; }
            Ok(db)
        })();
        match result {
            Ok(db) => {
                self.recent_folders.retain(|p| p != &root);
                self.recent_folders.insert(0, root.clone());
                self.recent_folders.truncate(5);
                let recent_save_error = save_recent_folders(&self.recent_folders).err();
                self.nav_history.clear(); self.nav_cursor = 0;
                self.root = Some(root); self.db = Some(db); self.writable = analyze;
                self.index_path = Some(if !analyze && !path.is_file() {
                    self.root.as_ref().unwrap().join(".folder-history/history.sqlite")
                } else { path.clone() });
                self.selected = None; self.active_dependency = None; self.group_filter.clear();
                self.tab = if analyze { Tab::Build } else { Tab::Spectator };
                self.refresh(); self.status = format!("Opened index {}", self.index_path.as_ref().unwrap().display());
                if let Some(e) = recent_save_error {
                    self.status.push_str(&format!(" (could not save recent folders: {e})"));
                }
            }
            Err(e) => self.status = format!("Cannot open folder history: {e:#}"),
        }
    }
    fn refresh(&mut self) {
        self.artifacts.clear(); self.relations.clear(); self.issues.clear(); self.group_names.clear();
        self.analysis_groups.clear(); self.analysis_assignment.clear();
        let Some(db) = &self.db else { return; };
        let result: Result<(Vec<Artifact>, Vec<Relation>, Vec<Issue>)> = (|| {
            let mut q = db.prepare("SELECT id,path,kind FROM artifacts ORDER BY path")?;
            let artifacts = q.query_map([], |r| Ok(Artifact { id:r.get(0)?,path:r.get(1)?,kind:r.get(2)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut q = db.prepare("SELECT id,source,target,kind,evidence,manual FROM relations ORDER BY id")?;
            let relations = q.query_map([], |r| Ok(Relation { id:r.get(0)?,source:r.get(1)?,target:r.get(2)?,kind:r.get(3)?,evidence:r.get(4)?,manual:r.get::<_,i32>(5)? != 0 }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut q = db.prepare("SELECT id,notebook,cell,filename,category,decision,note FROM issues ORDER BY category,notebook,cell")?;
            let issues = q.query_map([], |r| Ok(Issue { id:r.get(0)?, notebook:r.get(1)?, cell:r.get(2)?, filename:r.get(3)?, category:r.get(4)?, decision:r.get(5)?, note:r.get(6)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((artifacts, relations, issues))
        })();
        match result { Ok((a,r,i)) => { self.artifacts=a;self.relations=r;self.issues=i; }, Err(e) => self.status=format!("Database error: {e:#}") }
        self.analysis_groups = analysis_graph::detect(&self.artifacts, &self.relations);
        for group in &self.analysis_groups {
            for path in &group.paths { self.analysis_assignment.insert(path.clone(), group.key.clone()); }
        }
        if let Some(db) = &self.db {
            if let Ok(mut q) = db.prepare("SELECT source_folder,display_name FROM analysis_names") {
                if let Ok(rows) = q.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))) {
                    self.group_names = rows.flatten().collect();
                }
            }
        }
    }
    fn selected_path(&self) -> Option<String> {
        self.artifacts.iter().find(|a| Some(a.id)==self.selected).map(|a| a.path.clone())
    }
    fn add_manual(&mut self) {
        if !self.writable { return; }
        if let Some(db) = &self.db {
            if !self.artifacts.iter().any(|a| a.path == self.from.trim()) || !self.artifacts.iter().any(|a| a.path == self.to.trim()) { self.status="Both paths must be indexed files inside this archive.".into(); return; }
            match add_relation(db, self.from.trim(), self.to.trim(), self.relation_kind.trim(), self.note.trim(), true) {
                Ok(()) => { self.refresh(); self.publish(); },
                Err(e) => self.status=format!("Save failed: {e:#}"),
            }
        }
    }
    // A resolution belongs to the dependency, not to an individual notebook cell.
    // The existing issue table remains the source of occurrence-level evidence.
    fn resolve_dependency(&mut self, category: &str, filename: &str, decision: &str, note: &str) {
        if !self.writable { return; }
        let Some(db) = &mut self.db else { return; };
        let result: Result<usize> = (|| {
            let tx = db.transaction()?;
            let changed = tx.execute(
                "UPDATE issues SET decision=?1,note=?2 WHERE category=?3 AND filename=?4",
                params![decision,note,category,filename])?;
            tx.commit()?;
            Ok(changed)
        })();
        match result {
            Ok(changed) => {
                if category == "external" && decision == "intentional_external" {
                    if let Err(e) = save_global_external_link(filename,note.trim()) {
                        self.status = format!("Local resolution saved, global registry failed: {e}");
                    }
                }
                self.refresh();
                self.status = format!("Updated {changed} occurrences of {filename}; changes saved to local index.");
            }
            Err(e) => self.status = format!("Dependency resolution failed: {e:#}"),
        }
    }
    fn publish(&mut self) {
        // The live database is already stored with the researcher's files.
        // Checkpoint WAL so copying the folder needs only history.sqlite.
        if let Some(db) = &self.db {
            match db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                Ok(()) => self.status = "Project history already stored in .folder_history/history.sqlite".into(),
                Err(e) => self.status = format!("Checkpoint failed: {e}"),
            }
        }
    }
    fn begin_scan(&mut self) {
        if self.scan_events.is_some() || !self.writable { return; }
        let (Some(root), Some(index)) = (self.root.clone(), self.index_path.clone()) else { return; };
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let hidden = self.include_hidden;
        self.scan_events = Some(rx);
        self.scan_cancel = Some(cancel);
        self.scan_summary = "Re-scan started: inventory and provenance extraction running...".into();
        self.status = "Re-scan running; existing results remain available until completion.".into();
        std::thread::spawn(move || {
            let result = (|| -> Result<ScanResult> {
                let mut db = Connection::open(index)?;
                schema(&db)?;
                scan(&mut db, &root, hidden, &worker_cancel, |message| {
                    let _ = tx.send(ScanEvent::Progress(message));
                })
            })().map_err(|e| format!("{e:#}"));
            let _ = tx.send(ScanEvent::Finished(result));
        });
    }
    fn begin_relink(&mut self) {
        if self.scan_events.is_some() || !self.writable { return; }
        let Some(index) = self.index_path.clone() else { return; };
        let (tx, rx) = mpsc::channel();
        self.scan_events = Some(rx);
        self.status = "Re-evaluating stored provenance evidence (no filesystem scan)...".into();
        std::thread::spawn(move || {
            let result = (|| -> Result<(i64, i64)> {
                let mut db = Connection::open(index)?;
                schema(&db)?;
                relink_cached_evidence(&mut db)
            })().map_err(|e| format!("{e:#}"));
            let _ = tx.send(ScanEvent::Resolved(result));
        });
    }
    fn poll_scan(&mut self) {
        let mut finished = None;
        let mut resolved = None;
        if let Some(rx) = &self.scan_events {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ScanEvent::Progress(message) => self.scan_summary = message,
                    ScanEvent::Finished(result) => { finished = Some(result); break; }
                    ScanEvent::Resolved(result) => { resolved = Some(result); break; }
                }
            }
        }
        if let Some(result) = resolved {
            self.scan_events = None;
            match result {
                Ok((notebooks, chains)) => {
                    self.refresh();
                    self.status = format!("Re-resolved cached evidence: {notebooks} notebook candidates, {chains} script-chain candidates. Manual resolutions preserved.");
                    self.scan_summary = self.status.clone();
                    self.publish();
                }
                Err(e) => self.status = format!("Re-resolve failed: {e}"),
            }
        }
        if let Some(result) = finished {
            self.scan_events = None;
            self.scan_cancel = None;
            match result {
                Ok(r) => {
                    self.scan_summary = format!("Inventory: {} files | Scripts: {} found, {} parsed | Shell steps: {} | Extracted references: {} | Warnings: {}",r.files,r.scripts,r.parsed,r.steps,r.links,r.warnings);
                    self.status = self.scan_summary.clone();
                    self.refresh();
                    self.publish();
                }
                Err(e) => {
                    self.scan_summary = format!("Re-scan did not complete: {e}");
                    self.status = self.scan_summary.clone();
                },
            }
        }
    }
    // Diagnostics are deliberately archive-relative and bounded for sharing.
    // Do not include raw command strings or evidence: they may embed private absolute paths.
    fn diagnostic_report(&self, detailed: bool) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("# FolderHistory diagnostic report\n\n");
        out.push_str("Scanner schema: v2 | Application: FolderHistory v0.2\n");
        out.push_str("Archive location and local index path: omitted for portability.\n\n");
        let _ = writeln!(out, "- Indexed files: {}", self.artifacts.len());
        let _ = writeln!(out, "- Relationships: {} (manual: {})", self.relations.len(), self.relations.iter().filter(|r| r.manual).count());
        let _ = writeln!(out, "- Issues: {}", self.issues.len());
        let _ = writeln!(out, "- Current scan status: {}", if self.scan_events.is_some() { "running" } else { "idle" });
        let _ = writeln!(out, "- Last scan message: {}", report_safe(&self.scan_summary));
        let mut kinds = std::collections::BTreeMap::<&str,usize>::new();
        for a in &self.artifacts { *kinds.entry(&a.kind).or_default() += 1; }
        out.push_str("\n## Inventory by type\n");
        for (kind,count) in kinds { let _ = writeln!(out, "- {kind}: {count}"); }
        let mut categories = std::collections::BTreeMap::<(&str,&str),usize>::new();
        for issue in &self.issues { *categories.entry((&issue.category,&issue.decision)).or_default() += 1; }
        out.push_str("\n## Issue counts (category / decision)\n");
        for ((cat,decision),count) in categories { let _ = writeln!(out, "- {cat} / {decision}: {count}"); }
        if let Some(db) = &self.db {
            let stats = db.query_row("SELECT files,steps,links FROM scans WHERE completed=1 ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?)));
            match stats {
                Ok((files,steps,links)) => { let _ = writeln!(out, "\n## Last completed scan\n- Files: {files}\n- Shell steps: {steps}\n- Extracted references: {links}"); },
                Err(_) => out.push_str("\n## Last completed scan\nNo completed scan recorded.\n"),
            }
            let count = |sql: &str| db.query_row(sql, [], |r| r.get::<_,i64>(0)).unwrap_or(0);
            let _ = writeln!(out, "- Stored processing steps: {}",count("SELECT count(*) FROM steps"));
            let _ = writeln!(out, "- Stored step inputs: {}",count("SELECT count(*) FROM step_io WHERE direction='reads'"));
            let _ = writeln!(out, "- Stored step outputs: {}",count("SELECT count(*) FROM step_io WHERE direction='writes'"));
            let _ = writeln!(out, "- Suspected copied folders: {}",count("SELECT count(*) FROM timestamp_assessments WHERE suspected_copy=1"));
            let _ = writeln!(out, "- Inferred script-order links: {}",count("SELECT count(*) FROM inferred_links"));
            let _ = writeln!(out, "- Script files without recorded processing steps: {}",count("SELECT count(*) FROM artifacts a WHERE a.kind='script' AND NOT EXISTS (SELECT 1 FROM steps s WHERE s.source=a.path)"));
            if detailed {
                out.push_str("\n## Timestamp reliability (suspected copy batches, up to 30)\n");
                if let Ok(mut q) = db.prepare("SELECT folder,files,correlation,transfer_mb_s FROM timestamp_assessments WHERE suspected_copy=1 ORDER BY files DESC LIMIT 30") {
                    if let Ok(rows) = q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,f64>(2)?,r.get::<_,f64>(3)?))) {
                        for row in rows.flatten() { let _=writeln!(out,"- {}: {} files; size/time r={:.3}; ~{:.1} MB/s (suspected copy, not proof)",report_safe(&row.0),row.1,row.2,row.3); }
                    }
                }
                out.push_str("\n## Inferred script-order chains (up to 30)\n");
                if let Ok(mut q) = db.prepare("SELECT source,target FROM inferred_links ORDER BY source,target LIMIT 30") {
                    if let Ok(rows) = q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))) {
                        for row in rows.flatten() { let _=writeln!(out,"- {} -> {} (candidate)",report_safe(&row.0),report_safe(&row.1)); }
                    }
                }
                out.push_str("\n## Scripts without extracted steps (up to 100)\n");
                if let Ok(mut q) = db.prepare("SELECT a.path FROM artifacts a WHERE a.kind='script' AND NOT EXISTS (SELECT 1 FROM steps s WHERE s.source=a.path) ORDER BY a.path LIMIT 100") {
                    if let Ok(rows) = q.query_map([],|r|r.get::<_,String>(0)) {
                        for row in rows.flatten() { let _=writeln!(out,"- {}",report_safe(&row)); }
                    }
                }
                out.push_str("\n## Sample extracted step I/O (up to 100)\n");
                if let Ok(mut q) = db.prepare("SELECT s.source,s.ordinal,io.direction,io.artifact FROM steps s JOIN step_io io ON io.step_id=s.id ORDER BY s.source,s.ordinal LIMIT 100") {
                    if let Ok(rows) = q.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))) {
                        for row in rows.flatten() { let _=writeln!(out,"- {} step {} {} {}",report_safe(&row.0),row.1,row.2,report_safe(&row.3)); }
                    }
                }
            }
        }
        if detailed {
            out.push_str("\n## Issue examples (up to 100 per category)\n");
            for category in ["external","unresolved","ambiguous"] {
                let _=writeln!(out,"\n### {category}");
                for issue in self.issues.iter().filter(|i|i.category==category).take(100) {
                    let _=writeln!(out,"- {} cell/line {}: `{}` [{}] note: {}",report_safe(&issue.notebook),issue.cell,report_safe(&issue.filename),issue.decision,report_safe(&issue.note));
                }
            }
            out.push_str("\n## Representative relationships (up to 100)\n");
            for relation in self.relations.iter().take(100) {
                let _=writeln!(out,"- {} --{}--> {}{}",report_safe(&relation.source),report_safe(&relation.kind),report_safe(&relation.target),if relation.manual {" [manual]"} else {""});
            }
        }
        if let Some(root) = &self.root { out.push_str(&report::markdown(root, &self.artifacts, &self.relations, &self.issues)); }
        out.push_str("\nRaw script commands, external paths and full evidence intentionally excluded.\n");
        out
    }
    fn export_diagnostic_report(&mut self) {
        let Some(root) = self.root.as_ref() else { return; };
        let suggested = root.file_name().and_then(|n|n.to_str()).unwrap_or("archive");
        let filename = format!("{}-folder-history-diagnostic.md",suggested);
        let report = self.diagnostic_report(true);
        if let Some(path) = rfd::FileDialog::new().set_file_name(&filename).add_filter("Markdown", &["md"]).save_file() {
            match std::fs::write(&path,report) {
                Ok(()) => self.status=format!("Diagnostic report saved to {}",path.display()),
                Err(e) => self.status=format!("Diagnostic export failed: {e}"),
            }
        }
    }
    fn export_readme(&mut self) {
        let Some(root) = self.root.clone() else { return; };
        let esc = |v: &str| v.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;");
        let mut html = String::from("<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>FolderHistory archive report</title><style>body{font:16px system-ui;max-width:1000px;margin:3em auto;padding:0 1em;background:#16191e;color:#eee}a{color:#9ccaff}article{border:1px solid #555;padding:1em;margin:1em 0;border-radius:8px}small{color:#bbb}</style><h1>Research archive — FolderHistory</h1>");
        html.push_str(&format!("<p>{} indexed files, {} relationships, {} issues.</p>",self.artifacts.len(),self.relations.len(),self.issues.len()));
        for category in ["external","unresolved","ambiguous"] {
            html.push_str(&format!("<h2>{}</h2>",esc(category)));
            for i in self.issues.iter().filter(|i| i.category == category) {
                let href = i.notebook.split('/').map(|x| x.replace(' ',"%20")).collect::<Vec<_>>().join("/");
                html.push_str(&format!("<article><a href=\"{}\">{}</a> — cell {}<p>Reference: <code>{}</code></p><small>Status: {} — {}</small></article>",esc(&href),esc(&i.notebook),i.cell,esc(&i.filename),esc(&i.decision),esc(&i.note)));
            }
        }
        let findings = report::markdown(&root, &self.artifacts, &self.relations, &self.issues);
        html.push_str("<h2>Archive handover findings</h2><pre style=\"white-space:pre-wrap;overflow-wrap:anywhere\">");
        html.push_str(&esc(&findings));
        html.push_str("</pre><p>Paths in this report are relative to the archive. External source directories are intentionally omitted.</p></html>");
        match std::fs::File::create(root.join("README.html")).and_then(|mut f| f.write_all(html.as_bytes())) {
            Ok(()) => self.status="Generated README.html in archive root.".into(),
            Err(e) => self.status=format!("README export failed: {e}"),
        }
    }
}
// The first path component is the stable provisional analysis/sample group.
// Cross-folder edges remain edges: they never merge groups. Root-level files
// remain visible in their own group.
fn source_group(path: &str) -> String {
    let normalized = path.trim_start_matches("./").replace('\\', "/");
    normalized.split_once('/').map(|(first, _)| first).unwrap_or("(archive root)").to_owned()
}

fn file_role(path: &str, kind: &str, relations: &[Relation]) -> (egui::Color32, &'static str) {
    let lower = path.to_ascii_lowercase();
    let suffix = lower.rsplit('.').next().unwrap_or("");
    let is_script = matches!(suffix, "py" | "r" | "sh" | "ipynb" | "rs" | "nf");
    if is_script {
        let writes_dataset = relations.iter().any(|r| {
            (r.source == path || r.target == path) && {
                let other = if r.source == path { &r.target } else { &r.source };
                let o = other.to_ascii_lowercase();
                [".h5ad", ".h5", ".mtx", ".mtx.gz", ".bam", ".cram", ".parquet"]
                    .iter().any(|ext| o.ends_with(ext))
            }
        });
        return if writes_dataset { (egui::Color32::from_rgb(180, 125, 240), "processing script") }
            else { (egui::Color32::from_rgb(245, 105, 105), "analysis script") };
    }
    if [".fastq", ".fastq.gz", ".fq", ".fq.gz", ".bcl", ".bcl.gz"]
        .iter().any(|ext| lower.ends_with(ext)) {
        (egui::Color32::from_rgb(100, 175, 255), "raw data")
    } else if [".png", ".svg", ".jpg", ".jpeg", ".pdf", ".tiff"]
        .iter().any(|ext| lower.ends_with(ext)) {
        (egui::Color32::from_rgb(105, 215, 145), "figure / document")
    } else if [".h5ad", ".h5", ".mtx", ".mtx.gz", ".bam", ".cram"]
        .iter().any(|ext| lower.ends_with(ext)) {
        (egui::Color32::from_rgb(80, 145, 215), "processed data (verify)")
    } else if [".tsv", ".csv", ".xlsx", ".json"]
        .iter().any(|ext| lower.ends_with(ext)) {
        (egui::Color32::from_rgb(235, 205, 90), "table / metadata")
    } else if [".fa", ".fasta", ".gtf", ".gff", ".fai", ".mmi"]
        .iter().any(|ext| lower.ends_with(ext)) {
        (egui::Color32::GRAY, "reference")
    } else { (egui::Color32::LIGHT_GRAY, if kind.is_empty() { "file" } else { "other" }) }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan();
        if self.scan_events.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(150)); }
        // Record user navigation once per frame; Back/Forward do not create new entries.
        let before = NavState { tab: self.tab, selected: self.selected };
        let mut history_navigation = false;
        let back_key = ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft));
        let forward_key = ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight));
        let home_key = ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::Home));
        ctx.set_visuals(egui::Visuals::dark());
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("FolderHistory");
                if ui.add_enabled(self.scan_events.is_none(), egui::Button::new("Analyze a folder")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.open(p, true); }
                }
                if ui.add_enabled(self.scan_events.is_none(), egui::Button::new("Open existing history")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.open(p, false); }
                }
                if self.db.is_some() {
                    ui.label(if self.writable { "Index: local, writable" } else { "Index: read-only" });
                }
            });
            ui.horizontal(|ui| {
                if self.nav_history.is_empty() { self.nav_history.push(before); self.nav_cursor = 0; }
                let back = ui.add_enabled(self.nav_cursor > 0, egui::Button::new("◀ Back"))
                    .on_hover_text("Alt+Left").clicked() || back_key;
                let forward = ui.add_enabled(self.nav_cursor + 1 < self.nav_history.len(), egui::Button::new("Forward ▶"))
                    .on_hover_text("Alt+Right").clicked() || forward_key;
                if back && self.nav_cursor > 0 {
                    self.nav_cursor -= 1;
                    let state = self.nav_history[self.nav_cursor];
                    self.tab = state.tab; self.selected = state.selected;
                    history_navigation = true;
                } else if forward && self.nav_cursor + 1 < self.nav_history.len() {
                    self.nav_cursor += 1;
                    let state = self.nav_history[self.nav_cursor];
                    self.tab = state.tab; self.selected = state.selected;
                    history_navigation = true;
                }
                if ui.small_button("⌂ Analyses").on_hover_text("Alt+Home").clicked() || home_key {
                    self.tab = Tab::Spectator;
                }
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Search, "Search");
                ui.selectable_value(&mut self.tab, Tab::Graph, "Relationships");
                ui.selectable_value(&mut self.tab, Tab::Build, "Build / Repair");
                ui.selectable_value(&mut self.tab, Tab::Spectator, "Spectator");
                // Re-evaluate persisted evidence; the full filesystem scan remains in Build / Repair.
                let button = egui::Button::new(
                    egui::RichText::new(if self.scan_events.is_some() { "WORKING…" } else { "RE-RESOLVE" })
                        .strong().color(egui::Color32::WHITE),
                ).fill(egui::Color32::from_rgb(170, 40, 40));
                if ui.add_enabled(self.writable && self.db.is_some() && self.scan_events.is_none(), button)
                    .on_hover_text("Recompute stored notebook and script-chain candidates; no file scan; keep manual resolutions")
                    .clicked() { self.begin_relink(); }

            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| { ui.label(&self.status); });
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.db.is_none() {
                ui.heading("Welcome to FolderHistory");
                if !self.recent_folders.is_empty() {
                    ui.add_space(10.0);
                    ui.heading("Recent folders");
                    let entries = self.recent_folders.clone();
                    for folder in entries {
                        let available = folder.is_dir();
                        let label = folder.display().to_string();
                        ui.horizontal(|ui| {
                            if ui.add_enabled(available, egui::Button::new(&label))
                                .on_hover_text("Open existing history without rescanning")
                                .clicked() {
                                // Recent archives are always opened in Spectator mode.
                                // Build / Repair requires an explicit user action.
                                self.open(folder.clone(), false);
                            }
                            if !available { ui.label("Unavailable"); }
                            if ui.small_button("×").on_hover_text("Remove from recent list only").clicked() {
                                self.recent_folders.retain(|p| p != &folder);
                                if let Err(e) = save_recent_folders(&self.recent_folders) {
                                    self.status = format!("Cannot save recent folders: {e}");
                                }
                            }
                        });
                    }
                    ui.separator();
                }
                ui.add_space(16.0);
                ui.heading("Analyze a folder");
                ui.label("Select an archive to scan. The history index is published inside the archive.");
                if ui.button("Choose folder to analyze...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.open(p, true); }
                }
                ui.add_space(20.0);
                ui.heading("Open existing history");
                ui.label("Browse an existing history database without modifying it.");
                if ui.button("Choose indexed folder...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.open(p, false); }
                }
                ui.add_space(16.0);
                ui.label("Search and browsing work offline. No AI service is required.");
                return;
            }
            match self.tab {
                Tab::Spectator => {
                    ui.heading("Scientific analyses · Spectator");
                    ui.label("Experiment → sample/run → workflow → input/operation/output step → files. Storage locations are ignored.");
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(100,175,255), "● raw");
                        ui.colored_label(egui::Color32::from_rgb(180,125,240), "● processing");
                        ui.colored_label(egui::Color32::from_rgb(80,145,215), "● dataset");
                        ui.colored_label(egui::Color32::from_rgb(245,105,105), "● analysis");
                        ui.colored_label(egui::Color32::from_rgb(235,205,90), "● tables");
                        ui.colored_label(egui::Color32::from_rgb(105,215,145), "● figures");
                        ui.colored_label(egui::Color32::GRAY, "● references");
                    });
                    ui.separator();
                    ui.label(format!("{} detected groups · {} indexed files", self.analysis_groups.len(), self.artifacts.len()));
                    let by_path: HashMap<&str, &Artifact> = self.artifacts.iter().map(|a| (a.path.as_str(), a)).collect();
                    let links = &self.relations;
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for group in &self.analysis_groups {
                            let title = self.group_names.get(&group.key).map(String::as_str).unwrap_or(&group.suggested_name);
                            let issues = self.issues.iter().filter(|i| self.analysis_assignment.get(&i.notebook)==Some(&group.key) && i.decision=="open").count();
                            let header = format!("{title} · {} files · {issues} open issues · {} links", group.paths.len(), group.edge_count);
                            ui.collapsing(header, |ui| {
                                ui.small(format!("Analysis group: {} · source hint: {}", group.key, group.source_hint));
                                let mut experiments: BTreeMap<String, BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<&str>>>>> = BTreeMap::new();
                                for path in &group.paths {
                                    let Some(a) = by_path.get(path.as_str()) else { continue; };
                                    let experiment = experiment_label(path, &group.suggested_name);
                                    let sample = sample_label(path, &experiment);
                                    let workflow = workflow_label(path, &a.kind).to_owned();
                                    let step = provenance_step(path, links);
                                    experiments.entry(experiment).or_default().entry(sample).or_default()
                                        .entry(workflow).or_default().entry(step).or_default().push(path);
                                }
                                for (experiment, samples) in experiments {
                                    let experiment_count: usize = samples.values().flat_map(|w| w.values()).flat_map(|s| s.values()).map(Vec::len).sum();
                                    ui.collapsing(format!("🧬 {experiment} · {experiment_count} files"), |ui| {
                                        for (sample, workflows) in samples {
                                            let sample_count: usize = workflows.values().flat_map(|s| s.values()).map(Vec::len).sum();
                                            ui.collapsing(format!("Sample / run: {sample} · {sample_count} files"), |ui| {
                                                for (workflow, steps) in workflows {
                                                    let workflow_count: usize = steps.values().map(Vec::len).sum();
                                                    ui.collapsing(format!("{workflow} · {workflow_count} files"), |ui| {
                                                        for (step, paths) in steps {
                                                            ui.collapsing(format!("{step} · {} files", paths.len()), |ui| {
                                                                if step.starts_with("FASTQ library:") {
                                                                    let sources = fastq_source_folders(&paths);
                                                                    ui.label(format!("{} FASTQ files · {} physical folder cluster(s)", paths.len(), sources.len()));
                                                                    for (dir, count) in &sources {
                                                                        ui.horizontal_wrapped(|ui| {
                                                                            ui.label(format!("📁 {dir} · {count} files"));
                                                                            if ui.small_button("Open in file explorer").clicked() {
                                                                                if let Some(root) = &self.root {
                                                                                    if let Err(e) = reveal_in_file_manager(root, dir) {
                                                                                        self.status = format!("Cannot open file manager: {e}");
                                                                                    }
                                                                                }
                                                                            }
                                                                        });
                                                                    }
                                                                    ui.collapsing(format!("📋 Show FASTQ file list ({} files)", paths.len()), |ui| {
                                                                        for path in &paths {
                                                                            let filename = path.rsplit('/').next().unwrap_or(path);
                                                                            ui.label(format!("{filename} — {path}")).on_hover_text(*path);
                                                                        }
                                                                    });
                                                                    ui.separator();
                                                                }
                                                                for path in paths {
                                                                    let Some(a) = by_path.get(path) else { continue; };
                                                                    let (color, role) = file_role(&a.path, &a.kind, links);
                                                                    let basename = a.path.rsplit('/').next().unwrap_or(&a.path);
                                                                    if ui.selectable_label(self.selected==Some(a.id), egui::RichText::new(format!("● {basename}")).color(color))
                                                                        .on_hover_text(format!("{}\n{role}", a.path)).clicked() { self.selected=Some(a.id); }
                                                                    if self.selected==Some(a.id) {
                                                                        ui.small(&a.path);
                                                                        if ui.small_button("📂 Open containing folder").clicked() {
                                                                            if let Some(root) = &self.root {
                                                                                if let Err(e) = reveal_in_file_manager(root, &a.path) {
                                                                                    self.status = format!("Cannot open file manager: {e}");
                                                                                }
                                                                            }
                                                                        }
                                                                        for r in links.iter().filter(|r| r.source==a.path || r.target==a.path).take(30) {
                                                                            let direction = if r.source==a.path { "→" } else { "←" };
                                                                            let other = if r.source==a.path { &r.target } else { &r.source };
                                                                            ui.small(format!("{direction} {} · {other}", r.kind));
                                                                        }
                                                                        if ui.small_button("Explore relationships").clicked() { self.tab=Tab::Graph; }
                                                                    }
                                                                }
                                                            });
                                                        }
                                                    });
                                                }
                                            });
                                        }
                                    });
                                }
                            });
                        }
                    });
                }
                Tab::Search => {
                    ui.heading("Find research data");
                    ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search filename, sample, notebook, FASTQ, figure..."));
                    ui.checkbox(&mut self.show_all, "Show all files (without search)");
                    ui.separator();
                    let needle = self.query.to_ascii_lowercase();
                    let matches: Vec<(i64,String,String)> = self.artifacts.iter()
                        .filter(|a| (self.show_all || !needle.is_empty()) && a.path.to_ascii_lowercase().contains(&needle))
                        .take(500).map(|a| (a.id,a.path.clone(),a.kind.clone())).collect();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (id,path,kind) in matches {
                            if ui.selectable_label(self.selected==Some(id), format!("[{kind}] {path}")).clicked() {
                                self.selected=Some(id); self.tab=Tab::Graph;
                            }
                        }
                    });
                }
                Tab::Graph => {
                    ui.heading("File history");
                    if let Some(path) = self.selected_path() {
                        ui.label(format!("Selected: {path}"));
                        if ui.button("📂 Open containing folder").clicked() {
                            if let Some(root) = &self.root {
                                if let Err(e) = reveal_in_file_manager(root, &path) {
                                    self.status = format!("Cannot open file manager: {e}");
                                }
                            }
                        }
                        ui.separator();
                        ui.heading("Direct relationships");
                        let links: Vec<Relation> = self.relations.iter().filter(|r| r.source==path || r.target==path).cloned().collect();
                        if links.is_empty() { ui.label("No relationships discovered yet."); }
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for r in links {
                                ui.group(|ui| {
                                    ui.label(format!("{}  —{}→  {}",r.source,r.kind,r.target));
                                    ui.small(format!("{}{}",r.evidence,if r.manual {" [manual]"} else {""}));
                                    let other = if r.source==path { &r.target } else { &r.source };
                                    if let Some(a) = self.artifacts.iter().find(|a| &a.path==other) {
                                        if ui.button(format!("Explore {}",other)).clicked() { self.selected=Some(a.id); }
                                    }
                                });
                            }
                        });
                    } else { ui.label("Select a file in Search to inspect its history."); }
                }
                Tab::Build => {
                    ui.heading("Build / Repair");
                    if let Some(root) = &self.root { ui.label(format!("Archive: {}", root.display())); }
                    if let Some(path) = &self.index_path { ui.label(format!("Index: {}", path.display())); }
                    if !self.writable {
                        ui.label("Read-only inspection: problems, evidence, and existing resolutions are visible below. To make corrections, use Analyze a folder to create a writable local index; the archive database is not modified.");
                    }
                    ui.label(format!("{} indexed files; {} relationships; {} open issue occurrences",self.artifacts.len(),self.relations.len(),self.issues.iter().filter(|issue| issue.decision == "open").count()));
                    ui.separator();
                    ui.heading("Analysis graph context");
                    let previous_group = self.group_filter.clone();
                    egui::ComboBox::from_id_salt("builder_analysis_group")
                        .selected_text(if self.group_filter.is_empty() { "All analyses".to_owned() }
                            else { self.group_names.get(&self.group_filter).cloned().or_else(|| self.analysis_groups.iter().find(|g| g.key==self.group_filter).map(|g|g.suggested_name.clone())).unwrap_or_else(|| self.group_filter.clone()) })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.group_filter, String::new(), "All analyses");
                            for group in &self.analysis_groups {
                                let label = self.group_names.get(&group.key).cloned().unwrap_or_else(|| group.suggested_name.clone());
                                ui.selectable_value(&mut self.group_filter, group.key.clone(), format!("{label} ({} files)",group.paths.len()));
                            }
                        });
                    if self.group_filter != previous_group { self.group_name_edit = self.group_names.get(&self.group_filter).cloned().unwrap_or_default(); }
                    if !self.group_filter.is_empty() {
                        if self.group_name_edit.is_empty() { self.group_name_edit = self.group_names.get(&self.group_filter).cloned().unwrap_or_default(); }
                        if let Some(group) = self.analysis_groups.iter().find(|g|g.key==self.group_filter) {
                            ui.small(format!("Original source hint: {} · {} connected files",group.source_hint,group.paths.len()));
                        }
                        ui.horizontal(|ui| {
                            ui.label("Analysis name:");
                            ui.text_edit_singleline(&mut self.group_name_edit);
                            if ui.add_enabled(self.writable && !self.group_name_edit.trim().is_empty(), egui::Button::new("Save name")).clicked() {
                                if let Some(db) = &self.db {
                                    let result = db.execute("INSERT INTO analysis_names(source_folder,display_name) VALUES(?1,?2) ON CONFLICT(source_folder) DO UPDATE SET display_name=excluded.display_name", params![self.group_filter, self.group_name_edit.trim()]);
                                    match result { Ok(_) => { self.group_names.insert(self.group_filter.clone(), self.group_name_edit.trim().to_owned()); self.status="Analysis name saved".into(); }, Err(e) => self.status=format!("Name save failed: {e}") }
                                }
                            }
                        });
                    }
                    ui.small("Grouping uses weighted provenance edges, not top-level folders. Weak or disconnected files remain visible.");
                    if !self.scan_summary.is_empty() { ui.label(&self.scan_summary); }
                    ui.horizontal(|ui| {
                        if ui.button("Copy Summary").clicked() {
                            ui.ctx().copy_text(self.diagnostic_report(false));
                            self.status="Diagnostic summary copied to clipboard.".into();
                        }
                        if ui.button("Export Diagnostic Report").clicked() {
                            self.export_diagnostic_report();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.writable && self.scan_events.is_none(), egui::Button::new("Publish index into archive")).clicked() { self.publish(); }
                        if ui.button("Generate README.html").clicked() { self.export_readme(); }
                    });
                    ui.separator();
                    ui.heading("Evidence");
                    if let Some(db) = &self.db {
                        let counts = db.query_row("SELECT (SELECT COUNT(*) FROM symlink_findings), (SELECT COUNT(*) FROM notebook_observations), (SELECT COUNT(*) FROM notebook_candidates)",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?)));
                        if let Ok((symlinks,observations,candidates))=counts {
                            ui.label(format!("Symlinks: {symlinks} | Saved notebook output paths: {observations} | Cross-notebook candidates: {candidates}"));
                        }
                        ui.collapsing("Symlink integrity",|ui| {
                            if let Ok(mut stmt)=db.prepare("SELECT path,target,category FROM symlink_findings ORDER BY category,path LIMIT 500") {
                                if let Ok(rows)=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))) {
                                    for row in rows.flatten() { ui.monospace(format!("{}: {} -> {}",row.2,row.0,row.1)); }
                                }
                            }
                        });
                        ui.collapsing("Notebook execution evidence (observed / inferred)",|ui| {
                            if let Ok(mut stmt)=db.prepare("SELECT notebook,cell,expression,observed_path FROM notebook_observations ORDER BY notebook,cell LIMIT 300") {
                                if let Ok(rows)=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))) {
                                    for row in rows.flatten() { ui.monospace(format!("Observed {} cell {} [{}] => {}",row.0,row.1,row.2,row.3)); }
                                }
                            }
                            if let Ok(mut stmt)=db.prepare("SELECT notebook,cell,expression,candidate_path,source_notebook,source_cell FROM notebook_candidates LIMIT 300") {
                                if let Ok(rows)=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?))) {
                                    for row in rows.flatten() { ui.monospace(format!("Candidate {} cell {} [{}] => {} (from {} cell {})",row.0,row.1,row.2,row.3,row.4,row.5)); }
                                }
                            }
                        });
                    }
                    ui.label("One row per dependency. Select a file to inspect its evidence and resolve every matching occurrence.");
                    ui.horizontal_wrapped(|ui| {
                        for category in ["all", "external", "unresolved", "ambiguous", "resolved"] {
                            let n = self.issues.iter().filter(|i| {
                                if category == "resolved" { i.decision != "open" }
                                else if category == "all" { i.decision == "open" }
                                else { i.category == category && i.decision == "open" }
                            }).map(|i| (i.category.as_str(), i.filename.as_str()))
                              .collect::<HashSet<_>>().len();
                            if ui.selectable_label(self.issue_filter == category, format!("{category} ({n})")).clicked() {
                                self.issue_filter = category.into();
                                self.active_dependency = None;
                            }
                        }
                    });
                    ui.separator();
                    if let Some((category, filename)) = self.active_dependency.clone() {
                        if ui.button("← Back to evidence").clicked() { self.active_dependency = None; }
                        ui.heading(&filename);
                        ui.label(format!("Category: {category}"));
                        let occurrences: Vec<Issue> = self.issues.iter()
                            .filter(|i| i.category == category && i.filename == filename).cloned().collect();
                        ui.label(format!("{} occurrences", occurrences.len()));
                        if let Some(first) = occurrences.first() {
                            ui.label(format!("Current decision: {}", first.decision));
                        }
                        ui.separator();
                        ui.heading("Resolution");
                        ui.label("A decision applies to every occurrence of this dependency in this category.");
                        ui.label("Location, external URL, or handover explanation:");
                        if let Some(url) = lookup_global_external_link(&filename) {
                            ui.label(format!("Reusable external resource candidate: {url}"));
                            if ui.button("Use registry URL as explanation").clicked() { self.resolution_note = url; }
                        }
                        ui.text_edit_multiline(&mut self.resolution_note);
                        ui.horizontal_wrapped(|ui| {
                            for (label, decision) in [
                                ("Missing", "missing"),
                                ("Intentional external", "intentional_external"),
                                ("False positive", "ignored"),
                                ("Reopen", "open"),
                            ] {
                                if ui.add_enabled(self.writable, egui::Button::new(label)).clicked() {
                                    let note = self.resolution_note.clone();
                                    self.resolve_dependency(&category, &filename, decision, &note);
                                }
                            }
                        });
                        ui.separator();
                        ui.heading("Occurrences / source evidence");
                        let root = self.root.clone();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for issue in &occurrences {
                                ui.collapsing(format!("{} · cell/line {} · {}", issue.notebook, issue.cell, issue.decision), |ui| {
                                    if let Some(root) = &root {
                                        let path = root.join(&issue.notebook);
                                        if path.extension().is_some_and(|ext| ext == "ipynb") {
                                            match std::fs::read_to_string(&path)
                                                .ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                                                .and_then(|v| v.get("cells")?.as_array()?.get((issue.cell - 1).max(0) as usize).cloned()) {
                                                Some(cell) => {
                                                    let source = match cell.get("source") {
                                                        Some(serde_json::Value::String(s)) => s.clone(),
                                                        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect(),
                                                        _ => String::new(),
                                                    };
                                                    ui.add(egui::Label::new(egui::RichText::new(source).monospace()).wrap());
                                                }
                                                None => { ui.label("Notebook cell unavailable."); }
                                            }
                                        } else {
                                            match std::fs::read_to_string(&path) {
                                                Ok(source) => {
                                                    let lines: Vec<&str> = source.lines().collect();
                                                    let line = (issue.cell - 1).max(0) as usize;
                                                    let from = line.min(lines.len()).saturating_sub(4);
                                                    let to = (line + 5).min(lines.len());
                                                    for (offset, text) in lines[from..to].iter().enumerate() {
                                                        ui.monospace(format!("{:>5}  {}", from + offset + 1, text));
                                                    }
                                                }
                                                Err(_) => { ui.label("Source file unavailable."); }
                                            }
                                        }
                                    }
                                });
                            }
                        });
                    } else {
                        let mut groups = std::collections::BTreeMap::<(String,String),usize>::new();
                        for issue in &self.issues {
                            let show = match self.issue_filter.as_str() {
                                "resolved" => issue.decision != "open",
                                "all" => issue.decision == "open",
                                category => issue.category == category && issue.decision == "open",
                            };
                            if show && (self.group_filter.is_empty() || self.analysis_assignment.get(&issue.notebook)==Some(&self.group_filter)) { *groups.entry((issue.category.clone(),issue.filename.clone())).or_default() += 1; }
                        }
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for ((category, filename), count) in groups {
                                if ui.selectable_label(false, format!("{filename} ({count})  ·  {category}")).clicked() {
                                    self.resolution_note = self.issues.iter()
                                        .find(|i| i.category == category && i.filename == filename)
                                        .map(|i| i.note.clone()).unwrap_or_default();
                                    self.active_dependency = Some((category,filename));
                                }
                            }
                        });
                    }
                    ui.label("Default scan excludes all dot-folders, dot-files and generated work/ directories.");
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.writable && self.scan_events.is_none(), egui::Button::new("Re-scan archive")).clicked() {
                            self.begin_scan();
                        }
                        if let Some(db) = &self.db {
                            let previous = db.query_row(
                                "SELECT files, steps, links FROM scans WHERE completed=1 ORDER BY id DESC LIMIT 1",
                                [],
                                |r| Ok((r.get::<_,i64>(0)?, r.get::<_,i64>(1)?, r.get::<_,i64>(2)?)),
                            );
                            match previous {
                                Ok((files, steps, links)) => {
                                    ui.label(format!("Last successful scan: {files} files, {steps} steps, {links} references"));
                                }
                                Err(_) => { ui.label("No successful scan recorded yet"); }
                            }
                        }
                    });
                    if self.scan_events.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner(); ui.label("Scanning in background...");
                            if ui.button("Cancel scan").clicked() {
                                if let Some(cancel) = &self.scan_cancel { cancel.store(true, Ordering::Relaxed); }
                                self.scan_summary = "Cancellation requested; waiting for current filesystem operation...".into();
                            }
                        });
                    }
                    ui.separator(); ui.heading("Add a confirmed relationship");
                    ui.label("Source path (relative to archive)"); ui.text_edit_singleline(&mut self.from);
                    ui.label("Target path (relative to archive)"); ui.text_edit_singleline(&mut self.to);
                    ui.label("Relationship type (e.g. reads, writes, derived_from)"); ui.text_edit_singleline(&mut self.relation_kind);
                    ui.label("Evidence / note"); ui.text_edit_singleline(&mut self.note);
                    if ui.add_enabled(self.writable && !self.from.trim().is_empty() && !self.to.trim().is_empty(),egui::Button::new("Save relationship")).clicked() { self.add_manual(); }
                    ui.separator(); ui.label("Manual relationships survive rescanning. Automatic notebook references are refreshed.");
                }
            }
        });
        let after = NavState { tab: self.tab, selected: self.selected };
        if !history_navigation && after != before && self.db.is_some() {
            self.nav_history.truncate(self.nav_cursor + 1);
            self.nav_history.push(after);
            self.nav_cursor = self.nav_history.len() - 1;
        }
    }
}
// Shared scanner entry point for the headless batch executable. The GUI and CLI
// intentionally use the exact same schema, parser and scanner implementation.
pub(crate) fn batch_scan_folder(root: &Path, rescan: bool) -> Result<String> {
    if !root.is_dir() { anyhow::bail!("Not a directory: {}", root.display()); }
    let root = root.canonicalize()?;
    let db_path = local_index_path(&root);
    if db_path.is_file() && !rescan {
        return Ok(format!("SKIP existing project index: {}", db_path.display()));
    }
    import_existing_index(&root, &db_path)?;
    if db_path.is_file() && !rescan {
        return Ok(format!("SKIP imported project index: {}", db_path.display()));
    }
    if let Some(parent) = db_path.parent() { std::fs::create_dir_all(parent)?; }
    let mut db = Connection::open(&db_path)
        .with_context(|| format!("Opening {}", db_path.display()))?;
    schema(&db)?;
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let result = scan(&mut db, &root, false, &cancel, |message| {
        eprintln!("    {message}");
    })?;
    Ok(format!("OK files={} scripts={} parsed={} links={} warnings={} elapsed={:.1}s index={}",
        result.files, result.scripts, result.parsed, result.links,
        result.warnings, started.elapsed().as_secs_f64(), db_path.display()))
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0]),
        ..Default::default()
    };

    eframe::run_native(
        "FolderHistory",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(App::default()))
        }),
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    fn scientific_mess() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/scientific_mess")
    }

    fn scan_scientific_mess() -> Connection {
        let root = scientific_mess();
        assert!(root.is_dir(), "missing fixture: {}", root.display());
        let mut db = Connection::open_in_memory().expect("open test SQLite database");
        schema(&db).expect("initialize schema");
        let result = scan(&mut db, &root, false, &AtomicBool::new(false), |_| {})
            .expect("scan scientific mess");
        assert!(result.files >= 30, "unexpectedly small fixture: {result:?}");
        db
    }

    #[test]
    fn relink_preserves_manual_resolutions_and_is_idempotent() {
        let mut db = scan_scientific_mess();
        db.execute("INSERT OR IGNORE INTO issues(notebook,cell,filename,category,decision,note) VALUES ('manual.ipynb',1,'genes.gtf','external','intentional_external','verified by researcher')", []).unwrap();
        add_relation(&db, "normalize.ipynb", "original.h5ad", "reads", "researcher confirmed", true).unwrap();
        let first = relink_cached_evidence(&mut db).unwrap();
        let second = relink_cached_evidence(&mut db).unwrap();
        assert_eq!(first, second, "cached re-link must be idempotent");
        let decision: String = db.query_row("SELECT decision FROM issues WHERE notebook='manual.ipynb'", [], |r| r.get(0)).unwrap();
        assert_eq!(decision, "intentional_external");
        let manual: i64 = db.query_row("SELECT COUNT(*) FROM relations WHERE manual=1 AND evidence='researcher confirmed'", [], |r| r.get(0)).unwrap();
        assert_eq!(manual, 1);
    }

    #[test]
    fn scientific_mess_inventory_and_rescan() {
        let mut db = scan_scientific_mess();
        for path in ["sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
                     "original.h5ad", "normalize.ipynb", "r_analysis.ipynb",
                     "old/analysis.ipynb", "de_results/differential_genes.tsv"] {
            let n: i64 = db.query_row("SELECT COUNT(*) FROM artifacts WHERE path=?1",
                [path], |row| row.get(0)).unwrap();
            assert_eq!(n, 1, "missing inventory artifact: {path}");
        }
        scan(&mut db, &scientific_mess(), false, &AtomicBool::new(false), |_| {})
            .expect("repeat scan");
        let duplicates: i64 = db.query_row(
            "SELECT COUNT(*) FROM (SELECT path FROM artifacts GROUP BY path HAVING COUNT(*)>1)",
            [], |row| row.get(0)).unwrap();
        assert_eq!(duplicates, 0, "duplicate artifacts after rescan");
    }

    #[test]
    fn scientific_mess_expected_provenance() {
        let db = scan_scientific_mess();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/scientific_mess_expected_provenance.json");
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&manifest).expect("read ground truth manifest"))
            .expect("parse ground truth manifest");
        let edges = expected["edges"].as_array().expect("manifest edges array");
        let mut missing = Vec::new();
        for edge in edges {
            let parts = edge.as_array().expect("edge must be [source, kind, target]");
            let source = parts[0].as_str().unwrap();
            let kind = parts[1].as_str().unwrap();
            let target = parts[2].as_str().unwrap();
            // Ground truth uses input READ_BY script, script WRITES output;
            // the current scanner stores script -> file with reads/writes kinds.
            let (stored_source, stored_target, stored_kind) = match kind {
                "READ_BY" => (target, source, "reads"),
                "WRITES" => (source, target, "writes"),
                "WRITES_DIRECTORY" => (source, target, "writes"),
                other => panic!("unsupported ground truth edge kind: {other}"),
            };
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM relations WHERE source=?1 AND target=?2 AND (kind=?3 OR (?3='writes' AND kind='writes_directory'))",
                params![stored_source, stored_target, stored_kind],
                |row| row.get(0)).unwrap();
            if count == 0 { missing.push(format!("{source} --{kind}--> {target}")); }
        }
        assert!(missing.is_empty(), "{} of {} expected provenance edges missing:\n{}",
            missing.len(), edges.len(), missing.join("\n"));
    }

    // Traverse provenance backwards: file -> producing script -> script inputs -> ...
    // A file inside a produced directory inherits that directory's producers.
    // Keep *all* candidate producers rather than selecting one.
    fn upstream_paths(db: &Connection, start: &str) -> std::collections::HashSet<String> {
        use std::collections::{HashSet, VecDeque};
        let mut edges = Vec::<(String, String, String)>::new();
        let mut stmt = db.prepare("SELECT source, target, kind FROM relations").unwrap();
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap();
        for row in rows { edges.push(row.unwrap()); }
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([start.to_string()]);
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) { continue; }
            // A file inherits the producer of any containing directory.
            // File/directory is an inventory property, not a provenance edge kind.
            let mut parent = std::path::Path::new(&current).parent();
            while let Some(dir) = parent {
                let name = dir.to_string_lossy();
                if name.is_empty() { break; }
                queue.push_back(name.into_owned());
                parent = dir.parent();
            }
            for (script, target, kind) in &edges {
                if (kind == "writes" || kind == "writes_directory") && target == &current {
                    queue.push_back(script.clone());
                } else if kind == "reads" && script == &current {
                    queue.push_back(target.clone());
                }
            }
        }
        seen
    }

    #[test]
    fn scientific_mess_full_fastq_ancestry() {
        let db = scan_scientific_mess();
        let cases: &[(&str, &[&str])] = &[
            ("de_results/differential_genes.tsv", &[
                "de_results", "differential_expression.ipynb", "clustered.h5ad",
                "cluster.ipynb", "normalized.h5ad", "normalize.ipynb",
                "original.h5ad", "create_h5ad.py", "matrix.mtx.gz",
                "run_mapping.sh", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
            ]),
            ("stats_output/marker_expression.tsv", &[
                "stats_output", "cluster.ipynb", "alternative_stats.ipynb",
                "clustered.h5ad", "normalized.h5ad", "original.h5ad",
                "create_h5ad.py", "run_mapping.sh",
                "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
            ]),
            ("cell_cycle_stats/phase_statistics.tsv", &[
                "cell_cycle_stats", "cell_cycle.ipynb", "original.h5ad",
                "create_h5ad.py", "run_mapping.sh",
                "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
            ]),
            ("processed.h5ad", &["r_analysis.ipynb", "original.h5ad",
                "create_h5ad.py", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz"]),
            ("r_processed.h5ad", &["normalize.R", "original.h5ad",
                "create_h5ad.py", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz"]),
        ];
        let mut failures = Vec::new();
        for (output, expected) in cases {
            let upstream = upstream_paths(&db, output);
            let missing: Vec<_> = expected.iter()
                .filter(|node| !upstream.contains(**node)).copied().collect();
            let forbidden: Vec<_> = ["sample_B_R1.fastq.gz", "sample_B_R2.fastq.gz",
                "old/original.h5ad", "old/processed.h5ad", "old/mapping.sh"]
                .into_iter().filter(|node| upstream.contains(*node)).collect();
            if !missing.is_empty() || !forbidden.is_empty() {
                failures.push(format!("{output}: missing={missing:?}; unexpected={forbidden:?}"));
            }
        }
        assert!(failures.is_empty(), "Full provenance chains broken:\n{}",
            failures.join("\n"));
    }

    #[test]
    fn shell_extracts_samtools() {
        let op=extract_shell("samtools sort -o results/a.sorted.bam input/a.bam").unwrap();
        assert_eq!(op.writes,["results/a.sorted.bam"]);
        assert_eq!(op.reads,["input/a.bam"]);
    }
    #[test]
    fn no_external_resolution() {
        let paths=HashSet::from(["results/out.h5ad".to_string()]);
        let (found,external,_)=resolve_reference("/home/old/out.h5ad","script.sh",&paths);
        assert!(external);
        assert!(found.is_none());
    }
    #[test]
    fn relative_paths_do_not_escape() {
        assert_eq!(safe_relative("../x"),None);
        assert_eq!(safe_relative("a/../b"),Some("b".to_string()));
    }
}
