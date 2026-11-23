use super::*;

pub use super::history_db::{schema, INDEX_SCHEMA_VERSION};

/// Resolve a source-relative reference to an inventory path. Never guess between
/// multiple same-named files; a non-existing path is not an indexed artifact.
pub fn classified_artifact_path(reference: &str, source: &str, paths: &HashSet<String>) -> Option<String> {
    let reference = safe_relative(reference)?;
    if paths.contains(&reference) { return Some(reference); }
    let parent = std::path::Path::new(source).parent().unwrap_or_else(|| std::path::Path::new(""));
    let local = safe_relative(&normalized(&parent.join(&reference)))?;
    if paths.contains(&local) { return Some(local); }
    let name = reference.rsplit('/').next()?;
    let mut matches = paths.iter().filter(|p| p.rsplit('/').next() == Some(name));
    let only = matches.next()?.clone();
    if matches.next().is_some() { None } else { Some(only) }
}

pub use super::history_db::{confirmed_roles};

pub fn kind(path: &Path) -> &'static str {
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
pub fn normalized(path: &Path) -> String { path.to_string_lossy().replace('\\', "/") }
pub use super::history_db::{add_relation};

pub fn safe_relative(input: &str) -> Option<String> {
    let value = input.replace('\\', "/");
    if value.starts_with('/') || value.starts_with("//") || value.contains(':') { return None; }
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part { "" | "." => {}, ".." => { parts.pop()?; }, _ => parts.push(part) }
    }
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}
pub fn reference_name(reference: &str) -> String {
    // Never persist historical absolute paths or parent directories in the index.
    reference.replace('\\', "/").rsplit('/').next().unwrap_or("").to_owned()
}
pub fn resolve_reference(reference: &str, notebook: &str, paths: &HashSet<String>) -> (Option<String>, bool, bool) {
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
// Output is evidence of a *saved execution*, never proof of the current code's result.
pub fn cell_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect(),
        _ => String::new(),
    }
}
pub fn notebook_output_text(cell: &serde_json::Value) -> String {
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
pub use super::history_db::{discover_notebook_observations};

pub use super::history_db::{infer_notebook_candidates};

// Recompute derived candidates using only evidence already persisted in SQLite.
// No filesystem traversal, parsing, or changes to manual decisions/relationships.
pub use super::history_db::{relink_cached_evidence};

pub use super::history_db::{global_registry_path, save_global_external_link, lookup_global_external_link};

pub fn parse_notebook(db: &Connection, root: &Path, rel: &str, paths: &HashSet<String>) -> Result<usize> {
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
                let (resolved, _, _) = resolve_reference(file, rel, paths);
                if let Some(target) = resolved {
                    super::history_db::DbProvenance::record_match(db, rel, idx + 1, file, &target, edge_kind, &evidence)?;
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
pub struct Operation { pub(crate) reads: Vec<String>, pub(crate) writes: Vec<String> }
pub fn shell_tokens(line: &str) -> Vec<String> {
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
pub fn plausible_reference(value: &str) -> bool {
    let v = value.trim();
    !v.is_empty() && !v.starts_with('&') && !v.starts_with('<') && !v.starts_with('>')
        && !v.starts_with('=') && !v.starts_with(']') && !v.ends_with(')')
        && !v.contains(';') && !v.contains('(') && !v.contains(')')
        && !["null", "none", "true", "false"].contains(&v.to_ascii_lowercase().as_str())
}
pub fn looks_like_path(value: &str) -> bool {
    let v=value.to_ascii_lowercase();
    [".bam",".sam",".cram",".fastq",".fq",".fastq.gz",".fq.gz",".h5ad",".h5",".tsv",".csv",".mtx",".mtx.gz",".pdf",".png",".svg",".vcf",".vcf.gz",".json",".txt",".fasta",".fa",".gtf",".gz"].iter().any(|e| v.ends_with(e))
}
pub fn extract_shell(line: &str) -> Option<Operation> {
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
pub use super::history_db::{parse_script};

pub fn excluded_dir(name: &str, _hidden: bool) -> bool {
    let lower = name.to_ascii_lowercase();
    // Never descend into dot-directories or generated workflow/build caches.
    // This applies at every depth, not only at the archive root.
    name.starts_with('.') || ["work", "__pycache__", "target", "venv", "node_modules", "lost+found"].contains(&lower.as_str())
}
pub fn excluded_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // Generated/hidden files are not user-authored analysis scripts.
    name.starts_with('.') || lower.ends_with('~') || lower.ends_with(".swp") || lower.ends_with(".swo")
        || lower.ends_with("-checkpoint.py") || lower.ends_with("-checkpoint.sh")
}
// A candidate is only made when a script explicitly mentions both artifacts.
// We never infer a dependency from timestamps alone or from mere folder proximity.
pub use super::history_db::{infer_script_chains};

pub use super::history_db::{assess_timestamps};

pub fn scan(db: &mut Connection, root: &Path, hidden: bool, cancel: &AtomicBool,
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
    let mut symlinks = Vec::new();
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
        symlinks.push(super::history_db::SymlinkFinding { path: normalized(relative), target: normalized(&target), category: category.to_owned() });
    }
    super::history_db::persist_scan(db, root,
        super::history_db::ScanInventory { files, paths, symlinks, warnings },
        cancel, progress)
}
// A researcher's index travels with their data. The global home directory is
// reserved for reusable links and recent-folder preferences.

/// Run provenance discovery inside the database-managed transaction. The engine
/// owns parsing; only the database module owns SQL and table reconciliation.
pub fn extract_provenance(db: &Connection, root: &Path,
    files: &[(String,String,i64,Option<i64>,Option<i64>)],
    paths: &HashSet<String>, cancel: &AtomicBool,
    initial_warnings: usize, progress: &mut impl FnMut(String)) -> Result<(usize,usize,usize,usize,usize)> {
    let mut warnings = initial_warnings;
    let mut links = 0;
    let mut steps = 0;
    let mut scripts = 0;
    let mut parsed = 0;
    for (path, file_kind, _, _, _) in files {
        if cancel.load(Ordering::Relaxed) { anyhow::bail!("Scan cancelled"); }
        if file_kind == "notebook" {
            if let Err(e) = provenance::parse(db, root, path, paths).map(|n| links+=n) {
                warnings+=1; if warnings<=10 { progress(format!("Notebook parse warning ({}): {e}", path)); }
            }
        } else if file_kind == "script" {
            scripts+=1;
            if path.to_ascii_lowercase().ends_with(".sh") || path.to_ascii_lowercase().ends_with(".bash") || path.to_ascii_lowercase().ends_with(".ps1") {
                match super::history_db::parse_script(db,root,path,paths) {
                    Ok((s,n)) => { steps+=s; links+=n; parsed+=1; if let Ok(extra)=provenance::parse(db,root,path,paths){links+=extra;} },
                    Err(e) => { warnings+=1; if warnings<=10 { progress(format!("Script parse warning ({}): {e}",path)); } }
                }
            }
            if path.to_ascii_lowercase().ends_with(".py") || path.to_ascii_lowercase().ends_with(".r") {
                match provenance::parse(db,root,path,paths) {
                    Ok(n) => {links+=n;parsed+=1;},
                    Err(e) => {warnings+=1;if warnings<=10 {progress(format!("Script parse warning ({}): {e}",path));}}
                }
            }
            if scripts % 20 == 0 { progress(format!("Scripts: {scripts} found, {parsed} parsed, {steps} steps, {links} links")); }
        }
    }
    for (path, file_kind, _, _, _) in files {
        if file_kind == "notebook" {
            if let Err(e) = super::history_db::discover_notebook_observations(db,root,path) {
                warnings+=1;
                if warnings<=10 { progress(format!("Notebook output warning ({path}): {e}")); }
            }
        }
    }
    Ok((steps, links, warnings, scripts, parsed))
}

pub use super::history_db::{local_index_path};

pub fn report_safe(value: &str) -> String {
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


pub use super::history_db::{batch_finished_path, batch_scan_folder};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use rusqlite::{params, OpenFlags};
    use super::super::history_db::{BatchWorkspace, archive_working_copy, publish_archive_snapshot};

    #[test]
    fn scanner_records_matches_directly_and_preserves_decisions() {
        use super::super::history_db::{DbIssues, DbProvenance};
        let db = Connection::open_in_memory().unwrap();
        schema(&db).unwrap();
        for path in ["analysis.ipynb", "out/analyzed_A.h5ad", "out/analyzed_B.h5ad"] {
            db.execute("INSERT INTO artifacts(path,kind) VALUES (?1,'file')", params![path]).unwrap();
        }
        let expression = "analyzed_*.h5ad";
        for target in ["out/analyzed_A.h5ad", "out/analyzed_B.h5ad"] {
            db.record_match("analysis.ipynb", 7, expression, target, "reads", "cell 7").unwrap();
        }
        let source: i64 = db.query_row("SELECT id FROM artifacts WHERE path='analysis.ipynb'", [], |r|r.get(0)).unwrap();
        let wizard = db.issues(source).unwrap();
        assert_eq!(wizard.groups.len(), 1);
        assert!(wizard.groups[0].is_multi);
        assert_eq!(wizard.groups[0].matches.len(), 2);
        let match_id = wizard.groups[0].matches[0].id;
        super::super::history_db::set_match_decision(&db, match_id, 2).unwrap();
        db.record_match("analysis.ipynb", 7, expression, "out/analyzed_A.h5ad", "reads", "cell 7").unwrap();
        assert_eq!(db.query_row("SELECT decision_id FROM file_matches WHERE id=?1",params![match_id],|r|r.get::<_,i64>(0)).unwrap(),2);
    }

    #[test]
    fn incompatible_index_is_discarded_only_in_temporary_workspace() {
        let scratch = BatchWorkspace::new().unwrap();
        let root = scratch.path().join("research");
        std::fs::create_dir_all(root.join(".folder_history")).unwrap();
        let archive = local_index_path(&root);
        let old = Connection::open(&archive).unwrap();
        old.execute_batch("PRAGMA user_version=2; CREATE TABLE old_data (value TEXT);
            INSERT INTO old_data VALUES ('untouched');").unwrap();
        old.close().unwrap();
        let (_workspace, local, returned_archive, exists) = archive_working_copy(&root).unwrap();
        assert!(exists);
        assert!(!local.exists());
        assert_eq!(returned_archive, archive);
        let check = Connection::open(&archive).unwrap();
        let value: String = check.query_row("SELECT value FROM old_data", [], |r| r.get(0)).unwrap();
        assert_eq!(value, "untouched");
    }

    #[test]
    fn canonical_index_round_trip_ignores_legacy() {
        let scratch = BatchWorkspace::new().unwrap();
        let root = scratch.path().join("research");
        std::fs::create_dir_all(root.join(".folder-history")).unwrap();
        std::fs::write(root.join(".folder-history/history.sqlite"), b"legacy ignored").unwrap();
        let (workspace, local, archive, exists) = archive_working_copy(&root).unwrap();
        assert!(!exists);
        assert!(!local.exists());
        {
            let db = Connection::open(&local).unwrap();
            schema(&db).unwrap();
            db.execute("INSERT INTO artifacts(path,kind) VALUES ('input.fastq','fastq')", []).unwrap();
            db.close().unwrap();
        }
        publish_archive_snapshot(&local, &archive).unwrap();
        drop(workspace);
        let (workspace, copied, _, exists) = archive_working_copy(&root).unwrap();
        assert!(exists);
        let db = Connection::open_with_flags(&copied, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let count: i64 = db.query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
        db.close().unwrap();
        drop(workspace);
    }

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
    fn scientific_mess_inventory_and_rescan() {
        let mut db = scan_scientific_mess();
        for path in ["sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
                     "old/original.h5ad", "normalize.ipynb", "r_analysis.ipynb",
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
                "SELECT COUNT(*) FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path=?1 AND a.path=?2 AND (m.direction=?3 OR (?3='writes' AND m.direction='writes_directory'))",
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
        let mut stmt = db.prepare("SELECT s.path, a.path, m.direction FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id").unwrap();
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
                "old/original.h5ad", "create_h5ad.py", "matrix.mtx.gz",
                "run_mapping.sh", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
            ]),
            ("stats_output/marker_expression.tsv", &[
                "stats_output", "cluster.ipynb", "alternative_stats.ipynb",
                "clustered.h5ad", "normalized.h5ad", "old/original.h5ad",
                "create_h5ad.py", "run_mapping.sh",
                "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
            ]),
            ("cell_cycle_stats/phase_statistics.tsv", &[
                "cell_cycle_stats", "cell_cycle.ipynb", "old/original.h5ad",
                "create_h5ad.py", "run_mapping.sh",
                "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
            ]),
            ("processed.h5ad", &["r_analysis.ipynb", "old/original.h5ad",
                "create_h5ad.py", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz"]),
            ("r_processed.h5ad", &["normalize.R", "old/original.h5ad",
                "create_h5ad.py", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz"]),
        ];
        let mut failures = Vec::new();
        for (output, expected) in cases {
            let upstream = upstream_paths(&db, output);
            let missing: Vec<_> = expected.iter()
                .filter(|node| !upstream.contains(**node)).copied().collect();
            let forbidden: Vec<_> = ["sample_B_R1.fastq.gz", "sample_B_R2.fastq.gz",
                "old/processed.h5ad", "old/mapping.sh"]
                .into_iter().filter(|node| upstream.contains(*node)).collect();
            if !missing.is_empty() || !forbidden.is_empty() {
                failures.push(format!("{output}: missing={missing:?}; unexpected={forbidden:?}"));
            }
        }
        assert!(failures.is_empty(), "Full provenance chains broken:\n{}",
            failures.join("\n"));
    }

    #[test]
    #[ignore = "Cell Ranger provenance not implemented yet"]
    fn cellranger_count_variable_subset_provenance() {
        // Real directory tree, fake data. The command must never be executed.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cellranger");
        assert!(root.join("run_cellranger.sh").is_file());
        let mut db = Connection::open_in_memory().unwrap();
        schema(&db).unwrap();
        scan(&mut db, &root, false, &AtomicBool::new(false), |_| {})
            .expect("scan Cell Ranger fixture");

        let input_files = [
            "data/fastqs/sample_A/SampleA_S1_L001_R1_001.fastq.gz",
            "data/fastqs/sample_A/SampleA_S1_L001_R2_001.fastq.gz",
            "references/refdata-gex-GRCh38-2024-A/genes/genes.gtf",
            "references/refdata-gex-GRCh38-2024-A/fasta/genome.fa",
        ];
        let outputs = [
            "results/sample_A_count/outs/metrics_summary.csv",
            "results/sample_A_count/outs/molecule_info.h5",
            "results/sample_A_count/outs/filtered_feature_bc_matrix/matrix.mtx.gz",
            "results/sample_A_count/outs/filtered_feature_bc_matrix/features.tsv.gz",
            "results/sample_A_count/outs/filtered_feature_bc_matrix/barcodes.tsv.gz",
        ];
        let forbidden = [
            "data/fastqs/sample_B/SampleB_S2_L001_R1_001.fastq.gz",
            "data/fastqs/sample_B/SampleB_S2_L001_R2_001.fastq.gz",
            "old/fastqs/Legacy_S1_L001_R1_001.fastq.gz",
            "references/obsolete_reference/reference.json",
            "config/feature_reference.csv",
        ];
        // Check inventory separately from inferred relationships.
        for path in input_files.iter().chain(outputs.iter()).chain(forbidden.iter()) {
            let count: i64 = db.query_row(
                "SELECT COUNT(*) FROM artifacts WHERE path=?1", [path], |r| r.get(0)
            ).unwrap();
            assert_eq!(count, 1, "fixture file not inventoried: {path}");
        }
        let edges: std::collections::HashSet<(String, String)> = db.prepare(
            "SELECT a.path, m.direction FROM file_matches m JOIN artifacts s ON s.id=m.source_id JOIN artifacts a ON a.id=m.artifact_id WHERE s.path='run_cellranger.sh'"
        ).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap().map(|r| r.unwrap()).collect();
        // Input directory and reference directory must be expanded to real files.
        for path in input_files {
            assert!(edges.contains(&(path.to_string(), "reads".to_string())),
                "Cell Ranger input not linked: {path}; actual={edges:?}");
        }
        // --id and cd determine the output tree, not the output basename.
        for path in outputs {
            assert!(edges.contains(&(path.to_string(), "writes".to_string())) ||
                    edges.contains(&(path.to_string(), "writes_directory".to_string())),
                "Cell Ranger output not linked: {path}; actual={edges:?}");
        }
        for path in forbidden {
            assert!(!edges.iter().any(|(target, _)| target == path),
                "unused variable incorrectly created a link to {path}");
        }
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

#[cfg(test)]
mod classification_integration_tests {
    use super::*;
    #[test]
    fn unique_and_ambiguous_paths() {
        let paths: HashSet<String> = ["a/data.h5ad", "b/data.h5ad", "a/quant_res"].iter().map(|s| s.to_string()).collect();
        assert_eq!(classified_artifact_path("data.h5ad", "a/script.ipynb", &paths).as_deref(), Some("a/data.h5ad"));
        assert_eq!(classified_artifact_path("data.h5ad", "other/script.ipynb", &paths), None);
        assert_eq!(classified_artifact_path("quant_res", "a/script.ipynb", &paths).as_deref(), Some("a/quant_res"));
    }
    #[test]
    fn confirmed_roles_survive_schema_and_are_authoritative() {
        let db = Connection::open_in_memory().unwrap();
        schema(&db).unwrap();
        db.execute("INSERT INTO artifact_roles(path,role,source) VALUES ('a/quant_res','Quantification results','a/script.ipynb')", []).unwrap();
        schema(&db).unwrap();
        assert_eq!(confirmed_roles(&db).unwrap().get("a/quant_res").map(String::as_str), Some("Quantification results"));
    }
}

