use anyhow::{Context, Result};
use eframe::egui;
use regex::Regex;
use rusqlite::{params, Connection, OpenFlags};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver}};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Clone, Debug)]
struct Artifact { id: i64, path: String, kind: String }
#[derive(Clone, Debug)]
struct Relation { id: i64, source: String, target: String, kind: String, evidence: String, manual: bool }
#[derive(Clone, Debug)]
struct Issue { id: i64, notebook: String, cell: i64, filename: String, category: String, decision: String, note: String }
#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab { Search, Graph, Build }

enum ScanEvent { Progress(String), Finished(Result<ScanResult, String>) }
#[derive(Debug)]
struct ScanResult { files: usize, steps: usize, links: usize, warnings: usize, scripts: usize, parsed: usize }

struct App {
    root: Option<PathBuf>, db: Option<Connection>, writable: bool, index_path: Option<PathBuf>,
    tab: Tab, query: String, artifacts: Vec<Artifact>, relations: Vec<Relation>,
    selected: Option<i64>, status: String, from: String, to: String,
    relation_kind: String, note: String, show_all: bool, issues: Vec<Issue>, issue_filter: String, scan_summary: String,
    include_hidden: bool, scan_events: Option<Receiver<ScanEvent>>, scan_cancel: Option<Arc<AtomicBool>>,
}
impl Default for App {
    fn default() -> Self {
        Self { root: None, db: None, writable: false, index_path: None, tab: Tab::Search,
            query: String::new(), artifacts: vec![], relations: vec![], selected: None,
            status: "Open a research folder to begin.".into(), from: String::new(),
            to: String::new(), relation_kind: "derived_from".into(), note: String::new(),
            show_all: false, issues: vec![], issue_filter: "all".into(), scan_summary: String::new(),
            include_hidden: false, scan_events: None, scan_cancel: None }
    }
}

fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("PRAGMA foreign_keys=ON;
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
    let paths: HashSet<String> = files.iter().map(|f| f.0.clone()).collect();
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
            if let Err(e) = parse_notebook(&tx, root, path, &paths).map(|n| links+=n) {
                warnings+=1; if warnings<=10 { progress(format!("Notebook parse warning ({}): {e}", path)); }
            }
        } else if file_kind == "script" {
            scripts+=1;
            if path.to_ascii_lowercase().ends_with(".sh") || path.to_ascii_lowercase().ends_with(".bash") || path.to_ascii_lowercase().ends_with(".ps1") {
                match parse_script(&tx,root,path,&paths) {
                    Ok((s,n)) => { steps+=s; links+=n; parsed+=1; },
                    Err(e) => { warnings+=1; if warnings<=10 { progress(format!("Script parse warning ({}): {e}",path)); } }
                }
            }
            if scripts % 20 == 0 { progress(format!("Scripts: {scripts} found, {parsed} parsed, {steps} steps, {links} links")); }
        }
    }
    assess_timestamps(&tx, &files)?;
    infer_script_chains(&tx)?;
    tx.execute("INSERT INTO scans(completed,files,steps,links) VALUES (1,?1,?2,?3)",params![files.len() as i64,steps as i64,links as i64])?;
    tx.commit()?;
    Ok(ScanResult { files:files.len(), steps, links, warnings, scripts, parsed })
}
// Keep the writable index on this computer; never require write access to the archive.
fn local_index_path(root: &Path) -> PathBuf {
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
        let archive_db = root.join(".folder-history").join("history.sqlite");
        let local_db = local_index_path(&root);
        let path = if analyze { local_db.clone() } else if archive_db.is_file() {
            archive_db.clone()
        } else { local_db.clone() };
        let result: Result<Connection> = (|| {
            if !root.is_dir() { anyhow::bail!("Folder does not exist: {}", root.display()); }
            if analyze {
                if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
                if !path.exists() && archive_db.is_file() {
                    std::fs::copy(&archive_db, &path)?;
                }
            }
            let flags = if analyze {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
            } else { OpenFlags::SQLITE_OPEN_READ_ONLY };
            let db = Connection::open_with_flags(&path, flags)
                .with_context(|| format!("Opening {}", path.display()))?;
            if analyze { schema(&db)?; }
            Ok(db)
        })();
        match result {
            Ok(db) => {
                self.root = Some(root); self.db = Some(db); self.writable = analyze;
                self.index_path = Some(path.clone());
                self.selected = None; self.tab = if analyze { Tab::Build } else { Tab::Search };
                self.refresh(); self.status = format!("Opened index {}", path.display());
            }
            Err(e) => self.status = format!("Cannot open folder history: {e:#}"),
        }
    }
    fn refresh(&mut self) {
        self.artifacts.clear(); self.relations.clear(); self.issues.clear();
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
    fn update_issue(&mut self, id: i64, decision: &str, note: &str) {
        if !self.writable { return; }
        if let Some(db) = &self.db {
            match db.execute("UPDATE issues SET decision=?1,note=?2 WHERE id=?3", params![decision,note,id]) {
                Ok(_) => { self.refresh(); self.publish(); },
                Err(e) => self.status=format!("Issue update failed: {e}"),
            }
        }
    }
    fn publish(&mut self) {
        if !self.writable { return; }
        let (Some(root),Some(index)) = (self.root.clone(),self.index_path.clone()) else { return; };
        let archive = root.join(".folder-history");
        let dest = archive.join("history.sqlite");
        // Close SQLite before copying, to ensure a consistent standalone database.
        self.db.take();
        let result: Result<()> = (|| {
            std::fs::create_dir_all(&archive)?;
            let tmp = archive.join("history.sqlite.tmp");
            std::fs::copy(&index, &tmp)?;
            // On Windows rename cannot replace an existing file; retain a backup
            // and restore it if publishing the new snapshot fails.
            let backup=archive.join("history.sqlite.backup");
            if backup.exists() { std::fs::remove_file(&backup)?; }
            if dest.exists() { std::fs::rename(&dest,&backup)?; }
            if let Err(e)=std::fs::rename(&tmp,&dest) {
                if backup.exists() { let _=std::fs::rename(&backup,&dest); }
                return Err(e.into());
            }
            if backup.exists() { let _=std::fs::remove_file(&backup); }
            Ok(())
        })();
        self.db = Connection::open(&index).ok();
        match result { Ok(()) => self.status=format!("Published {}",dest.display()), Err(e) => self.status=format!("Publish failed: {e:#}") }
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
    fn poll_scan(&mut self) {
        let mut finished = None;
        if let Some(rx) = &self.scan_events {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ScanEvent::Progress(message) => self.scan_summary = message,
                    ScanEvent::Finished(result) => { finished = Some(result); break; }
                }
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
        html.push_str("<p>Paths in this report are relative to the archive. External source directories are intentionally omitted.</p></html>");
        match std::fs::File::create(root.join("README.html")).and_then(|mut f| f.write_all(html.as_bytes())) {
            Ok(()) => self.status="Generated README.html in archive root.".into(),
            Err(e) => self.status=format!("README export failed: {e}"),
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan();
        if self.scan_events.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(150)); }
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
                ui.selectable_value(&mut self.tab, Tab::Search, "Search");
                ui.selectable_value(&mut self.tab, Tab::Graph, "Relationships");
                ui.selectable_value(&mut self.tab, Tab::Build, "Build / Repair");
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| { ui.label(&self.status); });
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.db.is_none() {
                ui.heading("Welcome to FolderHistory");
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
                        ui.label("This index is open read-only. You can inspect findings here. To scan or correct relationships, use Analyze a folder to create a local writable index.");
                    }
                    ui.label(format!("{} indexed files; {} relationships",self.artifacts.len(),self.relations.len()));
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
                    ui.heading("Archive integrity");
                    ui.horizontal(|ui| {
                        for category in ["all","external","unresolved","ambiguous"] {
                            let n = self.issues.iter().filter(|i| category=="all" || i.category==category).count();
                            if ui.selectable_label(self.issue_filter==category,format!("{category} ({n})")).clicked() { self.issue_filter=category.into(); }
                        }
                    });
                    let visible: Vec<Issue> = self.issues.iter().filter(|i| self.issue_filter=="all" || i.category==self.issue_filter).cloned().collect();
                    egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                        for issue in visible {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!("[{}] {} cell {}: {} — {}",issue.category,issue.notebook,issue.cell,issue.filename,issue.decision));
                                if ui.add_enabled(self.writable,egui::Button::new("Missing")).clicked() { self.update_issue(issue.id,"missing",&issue.note); }
                                if ui.add_enabled(self.writable,egui::Button::new("Intentional external")).clicked() { self.update_issue(issue.id,"intentional_external",&issue.note); }
                                if ui.add_enabled(self.writable,egui::Button::new("Reopen")).clicked() { self.update_issue(issue.id,"open",&issue.note); }
                            });
                            let mut note=issue.note.clone();
                            ui.horizontal(|ui| {
                                ui.label("Handover note:");
                                ui.text_edit_singleline(&mut note);
                                if ui.add_enabled(self.writable && note!=issue.note,egui::Button::new("Save note")).clicked() { self.update_issue(issue.id,&issue.decision,&note); }
                            });
                        }
                    });
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
    }
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
