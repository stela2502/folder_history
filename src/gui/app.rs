use crate::*;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Tab { Search, Graph, Build, Spectator }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NavState { tab: Tab, selected: Option<i64> }

// The database stores archive-relative paths. Never pass these directly to the OS.
pub(crate) fn reveal_in_file_manager(root: &Path, relative: &str) -> io::Result<()> {
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

// Open only indexed archive-relative files; never execute a directory or missing path.
pub(crate) fn open_indexed_file(root: &Path, relative: &str) -> io::Result<()> {
    let candidate = Path::new(relative);
    if candidate.is_absolute() || candidate.components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_))) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unsafe archive-relative path"));
    }
    let absolute = root.join(candidate);
    if !absolute.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("File does not exist: {}", absolute.display())));
    }
    #[cfg(target_os = "windows")]
    let mut cmd = { let mut c = Command::new("powershell.exe"); let escaped = absolute.to_string_lossy().replace("'", "''"); c.args(["-NoProfile", "-NonInteractive", "-Command", &format!("Invoke-Item -LiteralPath '{escaped}'")]); c };
    #[cfg(target_os = "macos")]
    let mut cmd = { let mut c = Command::new("open"); c.arg(&absolute); c };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = { let mut c = Command::new("xdg-open"); c.arg(&absolute); c };
    #[cfg(not(any(windows, unix)))]
    return Err(io::Error::new(io::ErrorKind::Unsupported, "No file opener for this OS"));
    #[cfg(any(windows, unix))]
    { cmd.spawn().map(|_| ()) }
}

// Candidate analysis membership is advisory. Never write these scores to the
// provenance graph: directory similarity is not evidence of a processing edge.
pub(crate) fn issue_analysis_candidates(
    issue: &Issue,
    assignment: &HashMap<String, String>,
    artifacts: &[Artifact],
    relations: &[Relation],
) -> Vec<(String, u32, String)> {
    let mut candidates: HashMap<String, (u32, Vec<&'static str>)> = HashMap::new();
    let mut add = |path: &str, score: u32, reason: &'static str| {
        if let Some(group) = assignment.get(path) {
            let entry = candidates.entry(group.clone()).or_insert((0, Vec::new()));
            if score > entry.0 { entry.0 = score; }
            if !entry.1.contains(&reason) { entry.1.push(reason); }
        }
    };
    // Source notebook/script is the strongest existing assignment evidence.
    add(&issue.notebook, 100, "source script belongs to analysis");
    // Explicitly recorded provenance links, not inferred directory links.
    for edge in relations {
        if edge.source == issue.notebook { add(&edge.target, 85, "recorded downstream link"); }
        if edge.target == issue.notebook { add(&edge.source, 85, "recorded upstream link"); }
    }
    let target_name = Path::new(&issue.filename).file_name().and_then(|n| n.to_str()).unwrap_or("");
    for artifact in artifacts {
        if artifact.path == issue.filename {
            add(&artifact.path, 95, "exact indexed path");
        } else if !target_name.is_empty() && Path::new(&artifact.path).file_name().and_then(|n| n.to_str()) == Some(target_name) {
            add(&artifact.path, 50, "matching basename (ambiguous)");
        }
    }
    let mut ranked: Vec<_> = candidates.into_iter()
        .map(|(group, (score, reasons))| (group, score, reasons.join("; "))).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}


pub(crate) enum ScanEvent { Progress(String), Finished(Result<ScanResult, String>), Resolved(Result<(i64, i64), String>) }

pub(crate) struct App {
    pub(crate) root: Option<PathBuf>, local_workspace: Option<BatchWorkspace>, pub(crate) db: Option<Connection>, pub(crate) index_path: Option<PathBuf>,
    pub(crate) tab: Tab, pub(crate) query: String, pub(crate) artifacts: Vec<Artifact>, pub(crate) relations: Vec<Relation>,
    pub(crate) selected: Option<i64>, pub(crate) status: String, pub(crate) from: String, pub(crate) to: String,
    pub(crate) relation_kind: String, pub(crate) note: String, pub(crate) show_all: bool, pub(crate) issues: Vec<Issue>, pub(crate) issue_filter: String, pub(crate) active_dependency: Option<(String,String)>, pub(crate) resolution_note: String, pub(super) wizard_index: usize, pub(super) wizard_mode: bool, pub(super) classification_choices: HashMap<String, String>, pub(super) classification_selected: HashMap<String, bool>, pub(super) recently_confirmed: Option<(String, String)>, pub(crate) scan_summary: String,
    pub(crate) group_filter: String, pub(crate) group_names: HashMap<String, String>, pub(crate) group_name_edit: String,
    pub(crate) analysis_groups: Vec<analysis_graph::AnalysisGroup>, pub(crate) analysis_assignment: HashMap<String, String>,
    pub(crate) nav_history: Vec<NavState>, pub(crate) nav_cursor: usize,
    pub(crate) recent_folders: Vec<PathBuf>, pub(crate) include_hidden: bool, missing_index: Option<PathBuf>, error_popup: Option<String>, pub(crate) open_events: Option<Receiver<App>>, pub(crate) scan_after_open: bool, pub(crate) scan_events: Option<Receiver<ScanEvent>>, pub(crate) scan_cancel: Option<Arc<AtomicBool>>,
}
impl Default for App {
    fn default() -> Self {
        Self { root: None, local_workspace: None, db: None, index_path: None, tab: Tab::Search,
            query: String::new(), artifacts: vec![], relations: vec![], selected: None,
            status: "Open a research folder to begin.".into(), from: String::new(),
            to: String::new(), relation_kind: "derived_from".into(), note: String::new(),
            show_all: false, issues: vec![], issue_filter: "all".into(), active_dependency: None, resolution_note: String::new(), wizard_index: 0, wizard_mode: false, classification_choices: HashMap::new(), classification_selected: HashMap::new(), recently_confirmed: None, scan_summary: String::new(),
            group_filter: String::new(), group_names: HashMap::new(), group_name_edit: String::new(),
            analysis_groups: Vec::new(), analysis_assignment: HashMap::new(),
            nav_history: vec![], nav_cursor: 0,
            recent_folders: load_recent_folders(), include_hidden: false, missing_index: None, error_popup: None, open_events: None, scan_after_open: false, scan_events: None, scan_cancel: None }
    }
}

// Plain UTF-8 file: one absolute folder path per line, newest first.
// This list contains only folder names, never data or manual resolutions.
pub(crate) fn recent_folders_file() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".folder_history/recent_folders.txt"))
}
pub(crate) fn load_recent_folders() -> Vec<PathBuf> {
    let Some(path) = recent_folders_file() else { return Vec::new(); };
    std::fs::read_to_string(path).unwrap_or_default().lines()
        .filter(|line| !line.is_empty()).take(5).map(PathBuf::from).collect()
}
pub(crate) fn save_recent_folders(folders: &[PathBuf]) -> Result<()> {
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

impl App {
    pub(crate) fn open(&mut self, root: PathBuf, analyze: bool) {
        if self.open_events.is_some() || self.scan_events.is_some() { return; }
        let (tx, rx) = mpsc::channel();
        self.open_events = Some(rx);
        self.missing_index = None;
        self.scan_after_open = analyze;
        self.status = format!("Loading archive {}…", root.display());
        std::thread::spawn(move || {
            // SQLite reads and analysis-group construction must never block egui.
            let mut loaded = App::default();
            loaded.open_sync(root, analyze);
            let _ = tx.send(loaded);
        });
    }

    fn poll_open(&mut self) {
        let result = self.open_events.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(mut loaded) = result {
            self.open_events = None;
            if loaded.db.is_none() {
                self.missing_index = loaded.missing_index.take();
                self.scan_after_open = false;
                self.status = loaded.status;
                if self.missing_index.is_none() { self.error_popup = Some(self.status.clone()); }
                return;
            }
            self.db = None; // Release any previous archive connection before its workspace.
            self.local_workspace = loaded.local_workspace.take();
            self.root = loaded.root.take();
            self.db = loaded.db.take();
            self.index_path = loaded.index_path.take();
            self.tab = loaded.tab;
            self.artifacts = std::mem::take(&mut loaded.artifacts);
            self.relations = std::mem::take(&mut loaded.relations);
            self.issues = std::mem::take(&mut loaded.issues);
            self.group_names = std::mem::take(&mut loaded.group_names);
            self.analysis_groups = std::mem::take(&mut loaded.analysis_groups);
            self.analysis_assignment = std::mem::take(&mut loaded.analysis_assignment);
            self.recent_folders = std::mem::take(&mut loaded.recent_folders);
            self.nav_history.clear();
            self.nav_cursor = 0;
            self.selected = None;
            self.active_dependency = None;
            self.group_filter.clear();
            self.status = loaded.status;
            if self.scan_after_open {
                self.scan_after_open = false;
                self.tab = Tab::Build;
                self.begin_scan();
            } else {
                self.ensure_missing_html();
            }
        } else if self.open_events.as_ref().is_some_and(|rx| matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected))) {
            self.open_events = None;
            self.scan_after_open = false;
            self.status = "Archive opening failed: background worker disconnected".into();
        }
    }

    fn open_sync(&mut self, root: PathBuf, analyze: bool) {
        let result: Result<(Connection, BatchWorkspace, PathBuf, PathBuf)> = (|| {
            if !root.is_dir() { anyhow::bail!("Folder does not exist: {}", root.display()); }
            if analyze { probe_archive_write_access(&root)?; }
            let (workspace, local, archive, exists) = archive_working_copy(&root)?;
            if !exists && !analyze {
                self.missing_index = Some(root.clone());
                anyhow::bail!("No FolderHistory database found in .folder_history/history.sqlite");
            }
            // Both entry points use an editable local SQLite working copy.
            // Tabs select a workflow, not a permission level.
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
            let db = Connection::open_with_flags(&local, flags)
                .with_context(|| format!("Opening local SQLite {}", local.display()))?;
            db.busy_timeout(std::time::Duration::from_secs(30))?;
            db.execute_batch("PRAGMA journal_mode=DELETE;")?;
            schema(&db)?;
            Ok((db, workspace, local, archive))
        })();
        match result {
            Ok((db, workspace, local, archive)) => {
                self.recent_folders.retain(|p| p != &root);
                self.recent_folders.insert(0, root.clone());
                self.recent_folders.truncate(5);
                let recent_save_error = save_recent_folders(&self.recent_folders).err();
                self.nav_history.clear(); self.nav_cursor = 0;
                self.root = Some(root); self.db = Some(db);
                self.local_workspace = Some(workspace);
                self.index_path = Some(local);
                self.selected = None; self.active_dependency = None; self.group_filter.clear();
                self.tab = if analyze { Tab::Build } else { Tab::Spectator };
                self.refresh(); self.status = format!("Opened local copy of {}", archive.display());
                if !analyze { self.ensure_missing_html(); }
                if let Some(e) = recent_save_error {
                    self.status.push_str(&format!(" (could not save recent folders: {e})"));
                }
            }
            Err(e) => self.status = format!("Cannot open folder history: {e:#}"),
        }
    }
    // Documentation is a researcher-facing convenience, not a scan prerequisite.
    // Only generate absent HTML; never touch README.md or replace existing HTML.
    fn ensure_missing_html(&mut self) {
        let Some(root) = self.root.as_ref() else { return; };
        match folder_history::core::documentation::generate_missing_html(
            root, &self.artifacts, &self.relations, &self.issues,
        ) {
            Ok(true) => self.status.push_str(" · Created README.html"),
            Ok(false) => {},
            Err(e) => self.status.push_str(&format!(" · README.html not created: {e}")),
        }
    }

    pub(crate) fn refresh(&mut self) {
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
    pub(crate) fn selected_path(&self) -> Option<String> {
        self.artifacts.iter().find(|a| Some(a.id)==self.selected).map(|a| a.path.clone())
    }
    pub(crate) fn add_manual(&mut self) {
        
        if let Some(db) = &self.db {
            if !self.artifacts.iter().any(|a| a.path == self.from.trim()) || !self.artifacts.iter().any(|a| a.path == self.to.trim()) { self.status="Both paths must be indexed files inside this archive.".into(); return; }
            match add_relation(db, self.from.trim(), self.to.trim(), self.relation_kind.trim(), self.note.trim(), true) {
                Ok(()) => { self.refresh(); self.publish(); },
                Err(e) => self.status=format!("Save failed: {e:#}"),
            }
        }
    }
    // Save a classification only for the equivalent source references shown in
    // the wizard. Do not silently classify other calls using the same filename.
    pub(super) fn resolve_issue_group(&mut self, ids: &[i64], decision: &str, note: &str) {
        
        if decision == "ignore" && note.trim().is_empty() {
            self.status = "Ignoring a reference requires an explanation.".into();
            return;
        }
        let Some(db) = &mut self.db else { return; };
        let result: Result<()> = (|| {
            let tx = db.transaction()?;
            for id in ids {
                tx.execute("UPDATE issues SET decision=?1,note=?2 WHERE id=?3",
                    params![decision, note, id])?;
            }
            tx.commit()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.refresh();
                self.publish();
                self.status = format!("Saved {decision} for {} source occurrence(s).", ids.len());
            }
            Err(e) => self.status = format!("Cannot save classification: {e:#}"),
        }
    }

    // A resolution belongs to the dependency, not to an individual notebook cell.
    // The existing issue table remains the source of occurrence-level evidence.
    pub(crate) fn resolve_dependency(&mut self, category: &str, filename: &str, decision: &str, note: &str) {
        
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
                self.status = format!("Updated {changed} occurrences of {filename}.");
                self.publish();
            }
            Err(e) => self.status = format!("Dependency resolution failed: {e:#}"),
        }
    }
    pub(crate) fn publish(&mut self) {
        if self.scan_events.is_some() { return; }
        let (Some(db), Some(root), Some(local)) = (&self.db, &self.root, &self.index_path) else { return; };
        let snapshot = local.with_extension("publish-snapshot.sqlite");
        let result: Result<()> = (|| {
            if snapshot.exists() { std::fs::remove_file(&snapshot)?; }
            // Snapshot locally, even while the GUI holds its local connection open.
            let escaped = snapshot.to_string_lossy().replace('\'', "''");
            db.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
            let archive = local_index_path(root);
            publish_archive_snapshot(&snapshot, &archive)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                let _ = std::fs::remove_file(&snapshot);
                self.status = "Published local SQLite snapshot to archive.".into();
            }
            Err(e) => {
                // Keep the standalone VACUUM snapshot for manual transfer.
                let archive = local_index_path(root);
                self.status = if snapshot.is_file() {
                    format!("Publication failed: {e:#} | Recovery SQLite snapshot: {} | Intended destination: {} | Local working DB: {}", snapshot.display(), archive.display(), local.display())
                } else {
                    format!("Publication failed: {e:#} | No recovery snapshot created. Local working DB: {} | Intended destination: {}", local.display(), archive.display())
                };
            }
        }
    }
    pub(crate) fn begin_scan(&mut self) {
        if self.scan_events.is_some() { return; }
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
                db.busy_timeout(std::time::Duration::from_secs(30))?;
                schema(&db)?;
                scan(&mut db, &root, hidden, &worker_cancel, |message| {
                    let _ = tx.send(ScanEvent::Progress(message));
                })
            })().map_err(|e| format!("{e:#}"));
            let _ = tx.send(ScanEvent::Finished(result));
        });
    }
    pub(crate) fn begin_relink(&mut self) {
        if self.scan_events.is_some() { return; }
        let Some(index) = self.index_path.clone() else { return; };
        let (tx, rx) = mpsc::channel();
        self.scan_events = Some(rx);
        self.status = "Re-evaluating stored provenance evidence (no filesystem scan)...".into();
        std::thread::spawn(move || {
            let result = (|| -> Result<(i64, i64)> {
                let mut db = Connection::open(index)?;
                db.busy_timeout(std::time::Duration::from_secs(30))?;
                db.busy_timeout(std::time::Duration::from_secs(30))?;
                schema(&db)?;
                relink_cached_evidence(&mut db)
            })().map_err(|e| format!("{e:#}"));
            let _ = tx.send(ScanEvent::Resolved(result));
        });
    }
    pub(crate) fn poll_scan(&mut self) {
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
                Err(e) => { self.status = format!("Re-resolve failed: {e}"); self.error_popup = Some(self.status.clone()); },
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
                    self.ensure_missing_html();
                }
                Err(e) => {
                    self.scan_summary = format!("Re-scan did not complete: {e}");
                    self.status = self.scan_summary.clone();
                    self.error_popup = Some(self.status.clone());
                },
            }
        }
    }
    // Diagnostics are deliberately archive-relative and bounded for sharing.
    // Do not include raw command strings or evidence: they may embed private absolute paths.
    pub(crate) fn diagnostic_report(&self, detailed: bool) -> String {
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
    pub(crate) fn export_diagnostic_report(&mut self) {
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
    pub(crate) fn export_readme(&mut self) {
        let Some(root) = self.root.as_ref() else { return; };
        match folder_history::core::documentation::generate(
            root, &self.artifacts, &self.relations, &self.issues
        ) {
            Ok(outcome) => self.status = outcome.summary(),
            Err(e) => self.status = format!("Documentation generation failed: {e:#}"),
        }
    }
}

/*
// The first path component is the stable provisional analysis/sample group.
// Cross-folder edges remain edges: they never merge groups. Root-level files
// remain visible in their own group.
pub(crate) fn source_group(path: &str) -> String {
    let normalized = path.trim_start_matches("./").replace('\\', "/");
    normalized.split_once('/').map(|(first, _)| first).unwrap_or("(archive root)").to_owned()
}
*/
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_open();
        self.poll_scan();
        if self.scan_events.is_some() || self.open_events.is_some() { ctx.request_repaint_after(std::time::Duration::from_millis(150)); }
        // Record user navigation once per frame; Back/Forward do not create new entries.
        let before = NavState { tab: self.tab, selected: self.selected };
        let mut history_navigation = false;
        let back_key = ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft));
        let forward_key = ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight));
        let home_key = ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::Home));
        ctx.set_visuals(egui::Visuals::dark());
        // Investigation is a separate application view, not an expansion of
        // Build / Repair. Draw it before the normal navigation and return.
        if self.wizard_mode {
            egui::CentralPanel::default().show(ctx, |ui| {
                self.draw_investigation(ui);
            });
            return;
        }
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("FolderHistory");
                if ui.add_enabled(self.scan_events.is_none() && self.open_events.is_none(), egui::Button::new("Analyze a folder")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.open(p, true); }
                }
                if ui.add_enabled(self.scan_events.is_none() && self.open_events.is_none(), egui::Button::new("Open existing history")).clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.open(p, false); }
                }
                if self.db.is_some() {
                    ui.label("Index: local, editable");
                    let scan_label = if self.scan_events.is_some() { "Scanning…" } else { "Scan / Rescan archive" };
                    if ui.add_enabled(self.scan_events.is_none() && self.open_events.is_none(), egui::Button::new(scan_label))
                        .on_hover_text("Inventory files and extract provenance; works even if no previous scan exists")
                        .clicked() { self.tab = Tab::Build; self.begin_scan(); }
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
                if ui.add_enabled(self.db.is_some() && self.scan_events.is_none(), button)
                    .on_hover_text("Recompute stored notebook and script-chain candidates; no file scan; keep manual resolutions")
                    .clicked() { self.begin_relink(); }

            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| { ui.label(&self.status); });
        if let Some(message) = self.error_popup.clone() {
            egui::Window::new("⚠ FolderHistory operation failed")
                .collapsible(false).resizable(true).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.colored_label(egui::Color32::YELLOW, "The operation could not complete.");
                    ui.label(&message);
                    if ui.button("Close").clicked() { self.error_popup = None; }
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.db.is_none() {
                if let Some(folder) = self.missing_index.clone() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.colored_label(egui::Color32::YELLOW, "⚠ This folder has not been indexed yet");
                        ui.label(format!("Folder: {}", folder.display()));
                        ui.label("No FolderHistory database was found. Create an index and scan this folder?");
                        ui.horizontal(|ui| {
                            if ui.add_enabled(self.open_events.is_none(), egui::Button::new("Create index & Analyze")).clicked() {
                                self.open(folder, true);
                            }
                            if ui.button("Cancel").clicked() {
                                self.missing_index = None;
                                self.status = "Index creation cancelled; no files changed.".into();
                            }
                        });
                    });
                    ui.separator();
                }
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
                Tab::Spectator => self.show_spectator(ui),
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
                            let response = ui.selectable_label(self.selected==Some(id), format!("[{kind}] {path}"))
                                .on_hover_text("Double-click to open; right-click for file actions");
                            response.context_menu(|ui| {
                                if ui.button("Open file").clicked() {
                                    if let Some(root) = &self.root { if let Err(e) = open_indexed_file(root, &path) { self.status = format!("Cannot open file: {e}"); } }
                                    ui.close();
                                }
                                if ui.button("Open containing folder").clicked() {
                                    if let Some(root) = &self.root { if let Err(e) = reveal_in_file_manager(root, &path) { self.status = format!("Cannot open file manager: {e}"); } }
                                    ui.close();
                                }
                                if ui.button("Copy path").clicked() {
                                    if let Some(root) = &self.root { ui.ctx().copy_text(root.join(&path).display().to_string()); }
                                    ui.close();
                                }
                            });
                            if response.double_clicked() {
                                if let Some(root) = &self.root { if let Err(e) = open_indexed_file(root, &path) { self.status = format!("Cannot open file: {e}"); } }
                            } else if response.clicked() { self.selected=Some(id); }
                            if self.selected == Some(id) {
                                ui.horizontal(|ui| {
                                    if ui.small_button("Open file with default application").clicked() {
                                        if let Some(root) = &self.root {
                                            if let Err(e) = open_indexed_file(root, &path) { self.status = format!("Cannot open file: {e}"); }
                                        }
                                    }
                                    if ui.small_button("Relationships").clicked() { self.tab = Tab::Graph; }
                                });
                            }
                        }
                    });
                }
                Tab::Graph => {
                    ui.heading("File history");
                    if let Some(path) = self.selected_path() {
                        ui.label(format!("Selected: {path}"));
                        if ui.button("Open file").clicked() {
                            if let Some(root) = &self.root {
                                if let Err(e) = open_indexed_file(root, &path) { self.status = format!("Cannot open file: {e}"); }
                            }
                        }
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
                            if ui.add_enabled(!self.group_name_edit.trim().is_empty(), egui::Button::new("Save name")).clicked() {
                                if let Some(db) = &self.db {
                                    let result = db.execute("INSERT INTO analysis_names(source_folder,display_name) VALUES(?1,?2) ON CONFLICT(source_folder) DO UPDATE SET display_name=excluded.display_name", params![self.group_filter, self.group_name_edit.trim()]);
                                    match result { Ok(_) => { self.group_names.insert(self.group_filter.clone(), self.group_name_edit.trim().to_owned()); self.status="Analysis name saved".into(); self.publish(); }, Err(e) => self.status=format!("Name save failed: {e}") }
                                }
                            }
                        });
                    }
                    ui.small("Issues are organized by their source script's analysis. Candidate assignments below are suggestions, not new provenance edges.");
                    let unassigned: Vec<&Issue> = self.issues.iter().filter(|i| i.decision == "open" && !self.analysis_assignment.contains_key(&i.notebook)).collect();
                    ui.collapsing(format!("Unassigned to an analysis · {} issue occurrences", unassigned.len()), |ui| {
                        let mut by_dependency = BTreeMap::<(String, String, String), &Issue>::new();
                        for issue in &unassigned { by_dependency.entry((issue.notebook.clone(), issue.category.clone(), issue.filename.clone())).or_insert(issue); }
                        ui.label("Suggestions are ranked from recorded provenance and indexed filename matches. Nothing is assigned automatically.");
                        egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                            for ((source, category, filename), issue) in by_dependency {
                                let ranked = issue_analysis_candidates(issue, &self.analysis_assignment, &self.artifacts, &self.relations);
                                ui.horizontal_wrapped(|ui| {
                                    if ui.link(format!("{filename} · {category} · {source}")).clicked() {
                                        self.active_dependency = Some((category.clone(), filename.clone()));
                                        self.wizard_index = self.issues.iter().position(|i| i.notebook == source && i.filename == filename && i.category == category).unwrap_or(0);
                                        self.wizard_mode = true;
                                    }
                                    if let Some((group, score, reason)) = ranked.first() {
                                        let name = self.group_names.get(group).cloned().or_else(|| self.analysis_groups.iter().find(|g| &g.key == group).map(|g| g.suggested_name.clone())).unwrap_or_else(|| group.clone());
                                        ui.label(format!("→ {name} ({score}/100; {reason})"));
                                        if ui.small_button("View analysis issues").clicked() { self.group_filter = group.clone(); }
                                    } else { ui.weak("No supported analysis candidate"); }
                                });
                            }
                        });
                    });
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
                        if ui.add_enabled(self.scan_events.is_none(), egui::Button::new("Publish index into archive")).clicked() { self.publish(); }
                        if ui.button("Generate README.md + HTML").clicked() { self.export_readme(); }
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
                            }).map(|i| (i.notebook.as_str(), i.filename.as_str()))
                              .collect::<HashSet<_>>().len();
                            if ui.selectable_label(self.issue_filter == category, format!("{category} ({n})")).clicked() {
                                self.issue_filter = category.into();
                                self.active_dependency = None;
                            }
                        }
                    });
                    ui.separator();
                    {

                        let mut groups = std::collections::BTreeMap::<(String,String,String),usize>::new();
                        for issue in &self.issues {
                            let show = match self.issue_filter.as_str() {
                                "resolved" => issue.decision != "open",
                                "all" => issue.decision == "open",
                                category => issue.category == category && issue.decision == "open",
                            };
                            if show && (self.group_filter.is_empty() || self.analysis_assignment.get(&issue.notebook)==Some(&self.group_filter)) { *groups.entry((issue.notebook.clone(),issue.category.clone(),issue.filename.clone())).or_default() += 1; }
                        }
                        let focus = self.recently_confirmed.take();
                        egui::ScrollArea::vertical().id_salt("issue_list").max_height(320.0).show(ui, |ui| {
                            for ((source, category, filename), count) in groups {
                                let is_recent = focus.as_ref().is_some_and(|(s, p)| s == &source && (p == &filename || filename.ends_with(p)));
                                let response = ui.selectable_label(is_recent, format!("{filename} ({count})  ·  {category} · {source}"));
                                if is_recent { response.scroll_to_me(Some(egui::Align::Center)); }
                                if response.clicked() {
                                    self.resolution_note = self.issues.iter()
                                        .find(|i| i.notebook == source && i.category == category && i.filename == filename)
                                        .map(|i| i.note.clone()).unwrap_or_default();
                                    self.wizard_index = self.issues.iter().position(|i| i.notebook == source && i.filename == filename && i.category == category).unwrap_or(0);
                                    self.active_dependency = Some((category, filename));
                                    self.wizard_mode = true;
                                    ui.ctx().request_repaint();
                                }
                            }
                        });
                    }
                    ui.label("Default scan excludes all dot-folders, dot-files and generated work/ directories.");
                    if self.scan_events.is_none() && self.db.is_some() {
                        ui.label("Use Scan / Rescan archive to start the first scan or refresh an existing one.");
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.root.is_some() && self.scan_events.is_none() && self.open_events.is_none(), egui::Button::new("Scan / Rescan archive")).clicked() {
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
                                Err(_) => { ui.colored_label(egui::Color32::YELLOW, "No successful scan recorded yet — start a scan above."); }
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
                    if ui.add_enabled(!self.from.trim().is_empty() && !self.to.trim().is_empty(),egui::Button::new("Save relationship")).clicked() { self.add_manual(); }
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
