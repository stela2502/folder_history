use super::*;
pub fn schema(db: &Connection) -> Result<()> {
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
        CREATE TABLE IF NOT EXISTS relocation_candidates (
            source TEXT NOT NULL, line INTEGER NOT NULL, expression TEXT NOT NULL,
            path TEXT NOT NULL, PRIMARY KEY(source,line,expression,path));
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
    db.execute_batch("CREATE TABLE IF NOT EXISTS file_classifications (
        source TEXT NOT NULL, path TEXT NOT NULL, role TEXT NOT NULL,
        confirmed INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY(source,path)
    );")?;
    // Directory review decisions are annotations, never deletion instructions.
    db.execute_batch("CREATE TABLE IF NOT EXISTS folder_reviews (
        path TEXT PRIMARY KEY, decision TEXT NOT NULL
    );")?;
    // Canonical scientist-confirmed roles, shared by the wizard, scanner and UI.
    // Source-scoped legacy evidence is retained in file_classifications.
    db.execute_batch("CREATE TABLE IF NOT EXISTS artifact_roles (
        path TEXT PRIMARY KEY, role TEXT NOT NULL, source TEXT NOT NULL,
        confirmed INTEGER NOT NULL DEFAULT 1
    );")?;
    // Upgrade confirmations saved by earlier wizard versions. Resolve only
    // unambiguous paths, never invent a match for duplicate basenames.
    {
        let paths: HashSet<String> = db.prepare("SELECT path FROM artifacts")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut q = db.prepare("SELECT source,path,role FROM file_classifications WHERE confirmed=1 ORDER BY rowid")?;
        let rows = q.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (source, reference, role) in rows {
            if let Some(path) = classified_artifact_path(&reference, &source, &paths) {
                db.execute("INSERT OR IGNORE INTO artifact_roles(path,role,source,confirmed) VALUES (?1,?2,?3,1)",
                    params![path,role,source])?;
            }
        }
    }
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

/// Authoritative roles are intentionally not provenance edges: a stage label
/// cannot prove which input produced an output.
pub fn confirmed_roles(db: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = db.prepare("SELECT path,role FROM artifact_roles WHERE confirmed=1")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    rows.collect::<rusqlite::Result<HashMap<_, _>>>().map_err(Into::into)
}

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
pub fn add_relation(db: &Connection, source: &str, target: &str, kind: &str, evidence: &str, manual: bool) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO relations(source,target,kind,evidence,manual) VALUES (?1,?2,?3,?4,?5)",
        params![source,target,kind,evidence,manual as i32])?;
    Ok(())
}
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
pub fn record_issue(db: &Connection, notebook: &str, cell: usize, filename: &str, category: &str) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO issues(notebook,cell,filename,category) VALUES (?1,?2,?3,?4)",
        params![notebook, cell as i64, filename, category])?;
    Ok(())
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
pub fn discover_notebook_observations(db: &Connection, root: &Path, rel: &str) -> Result<()> {
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
pub fn infer_notebook_candidates(db: &Connection) -> Result<()> {
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
pub fn relink_cached_evidence(db: &mut Connection) -> Result<(i64, i64)> {
    let tx = db.transaction()?;
    infer_notebook_candidates(&tx)?;
    tx.execute("DELETE FROM inferred_links WHERE reason='script-order candidate'", [])?;
    infer_script_chains(&tx)?;
    let notebooks = tx.query_row("SELECT COUNT(*) FROM notebook_candidates", [], |r| r.get(0))?;
    let chains = tx.query_row("SELECT COUNT(*) FROM inferred_links WHERE reason='script-order candidate'", [], |r| r.get(0))?;
    tx.commit()?;
    Ok((notebooks, chains))
}

pub fn global_registry_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".folder_history/external_links.db"))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".folder_history/external_links.db")))
}
pub fn save_global_external_link(name: &str, url: &str) -> Result<()> {
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
pub fn lookup_global_external_link(name: &str) -> Option<String> {
    let path = global_registry_path()?;
    let db = Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.query_row("SELECT url FROM resources WHERE filename=?1",params![name],|r|r.get(0)).ok()
}
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
pub fn parse_script(db: &Connection, root: &Path, rel: &str, paths: &HashSet<String>) -> Result<(usize,usize)> {
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
pub fn infer_script_chains(db: &Connection) -> Result<()> {
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
pub fn assess_timestamps(db: &Connection, files: &[(String,String,i64,Option<i64>,Option<i64>)]) -> Result<()> {
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
    let file_paths: HashSet<&str> = files.iter().map(|f| f.0.as_str()).collect();
    let tx = db.transaction()?;
    // Directory roles must be first-class artifacts too. No synthetic
    // producer/consumer edges are created for directories.
    for path in &paths {
        if !file_paths.contains(path.as_str()) {
            tx.execute("INSERT INTO artifacts(path,kind) VALUES (?1,'directory')
                ON CONFLICT(path) DO UPDATE SET kind='directory'", params![path])?;
        }
    }
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
    tx.execute("DELETE FROM relocation_candidates", [])?;
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
pub fn local_index_path(root: &Path) -> PathBuf {
    root.join(".folder_history").join("history.sqlite")
}

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


// A database may exist after an interrupted scan. Only this marker certifies
// that the batch scan returned successfully. It is not a database lock file.
pub fn batch_finished_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("sqlite.finished")
}

// Keep SQLite transactions off SMB/CIFS shares. The archive only receives a
// closed, verified database snapshot; each worker owns a separate local folder.
pub struct BatchWorkspace(PathBuf);
impl BatchWorkspace {
    pub fn new() -> Result<Self> {
        let base = std::env::var_os("TMPDIR").map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        for attempt in 0..1000u32 {
            let path = base.join(format!("folder-history-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos(),
                attempt));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("Unable to create a unique local scanning directory")
    }
}
impl Drop for BatchWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// A real write probe is more reliable than permission bits on SMB mounts.
pub fn probe_archive_write_access(root: &Path) -> Result<()> {
    let target = root.join(".folder_history");
    let dir = if target.is_dir() { target.as_path() } else { root };
    let name = format!(".folderhistory-write-test-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos());
    let path = dir.join(name);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
        .with_context(|| format!("Archive is not writable: {}", dir.display()))?;
    use std::io::Write;
    let write_result = file.write_all(b"FolderHistory write probe\n");
    drop(file);
    let remove_result = std::fs::remove_file(&path);
    write_result?;
    remove_result?;
    Ok(())
}

// Copy only the canonical archive index into a local workspace. Never use
// SQLite on the archive path, and never import a legacy database.
// A missing archive is a new index, not a migration request.
pub fn archive_working_copy(root: &Path) -> Result<(BatchWorkspace, PathBuf, PathBuf, bool)> {
    let archive = local_index_path(root);
    let workspace = BatchWorkspace::new()?;
    let local = workspace.0.join("history.sqlite");
    let exists = archive.is_file();
    if exists {
        // A raw copy is safe only for a closed, single-file SQLite database.
        // A sidecar is not necessarily a lock: report exactly what we found.
        ensure_no_sqlite_sidecars(&archive)?;
        std::fs::copy(&archive, &local)
            .with_context(|| format!("Copying archive index {} to local workspace", archive.display()))?;
    }
    Ok((workspace, local, archive, exists))
}

fn ensure_no_sqlite_sidecars(database: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = std::ffi::OsString::from(format!("{}{}", database.display(), suffix));
        let path = PathBuf::from(sidecar);
        if path.exists() {
            anyhow::bail!("SQLite sidecar exists at {}. Cannot safely copy the database as a single file; this does not necessarily mean it is locked.", path.display());
        }
    }
    Ok(())
}

// Publish a closed, consistent local SQLite snapshot. Both CLI and GUI use this.
pub fn publish_archive_snapshot(local: &Path, archive: &Path) -> Result<()> {
    let parent = archive.parent().context("Archive database has no parent directory")?;
    // Publication must use a closed, standalone SQLite file.
    ensure_no_sqlite_sidecars(local)?;
    if archive.exists() { ensure_no_sqlite_sidecars(archive)?; }
    std::fs::create_dir_all(parent)?;
    let staging = archive.with_extension(format!("sqlite.publish-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
    let result = (|| -> Result<()> {
        std::fs::copy(local, &staging)
            .with_context(|| format!("Copying SQLite snapshot into {}", staging.display()))?;
        // Do not replace an existing database with a partial transfer.
        std::fs::rename(&staging, archive)
            .with_context(|| format!("Replacing archive SQLite {}", archive.display()))?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&staging); }
    result
}

pub fn batch_scan_folder(root: &Path, rescan: bool) -> Result<String> {
    if !root.is_dir() { anyhow::bail!("Not a directory: {}", root.display()); }
    let root = root.canonicalize()?;
    let db_path = local_index_path(&root);
    let finished = batch_finished_path(&db_path);
    if db_path.is_file() && finished.is_file() && !rescan {
        return Ok(format!("SKIP completed project index: {}", db_path.display()));
    }
    if let Some(parent) = db_path.parent() { std::fs::create_dir_all(parent)?; }

    let (_workspace, local_db, _, _) = archive_working_copy(&root)?;

    let mut db = Connection::open(&local_db)
        .with_context(|| format!("Opening local SQLite {}", local_db.display()))?;
    db.busy_timeout(std::time::Duration::from_secs(30))?;
    // Do not produce WAL sidecars for the published archive.
    db.execute_batch("PRAGMA journal_mode=DELETE;")?;
    schema(&db)?;
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let result = scan(&mut db, &root, false, &cancel, |message| {
        eprintln!("    {message}");
    })?;
    let integrity: String = db.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" { anyhow::bail!("Local SQLite integrity check failed: {integrity}"); }
    db.close().map_err(|(_, error)| error)?;

    // Copy into the destination directory first. The existing archive database
    // remains intact if the scan or transfer fails.
    // Invalidate the old completion marker before replacing the archive.
    if finished.exists() { std::fs::remove_file(&finished)?; }
    publish_archive_snapshot(&local_db, &db_path)?;
    let marker = format!("schema_version=1\ncompleted_unix={}\nfiles={}\nsteps={}\nlinks={}\n",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        result.files, result.steps, result.links);
    let temporary = finished.with_extension(format!("sqlite.finished.tmp-{}", std::process::id()));
    std::fs::write(&temporary, marker)?;
    let marker_result = std::fs::rename(&temporary, &finished);
    if marker_result.is_err() { let _ = std::fs::remove_file(&temporary); }
    marker_result?;
    Ok(format!("OK files={} scripts={} parsed={} links={} warnings={} elapsed={:.1}s index={}",
        result.files, result.scripts, result.parsed, result.links,
        result.warnings, started.elapsed().as_secs_f64(), db_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_index_round_trip_ignores_legacy() {
        let scratch = BatchWorkspace::new().unwrap();
        let root = scratch.0.join("research");
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
                "old/original.h5ad",
            ]),
            ("stats_output/marker_expression.tsv", &[
                "stats_output", "cluster.ipynb", "alternative_stats.ipynb",
                "clustered.h5ad", "normalized.h5ad", "original.h5ad",
                "create_h5ad.py", "run_mapping.sh",
                "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
                "old/original.h5ad",
            ]),
            ("cell_cycle_stats/phase_statistics.tsv", &[
                "cell_cycle_stats", "cell_cycle.ipynb", "original.h5ad",
                "create_h5ad.py", "run_mapping.sh",
                "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
                "old/original.h5ad",
            ]),
            ("processed.h5ad", &["r_analysis.ipynb", "original.h5ad",
                "create_h5ad.py", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
                "old/original.h5ad"]),
            ("r_processed.h5ad", &["normalize.R", "original.h5ad",
                "create_h5ad.py", "sample_A_R1.fastq.gz", "sample_A_R2.fastq.gz",
                "old/original.h5ad"]),
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
            "SELECT target, kind FROM relations WHERE source='run_cellranger.sh'"
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

