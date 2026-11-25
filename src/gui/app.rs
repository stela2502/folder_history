use crate::*;
use folder_history::core::history_db::is_directory_child_link;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Tab { Search, Graph, Build, Spectator }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchMode { Filename, ScriptType, Loads, Saves, Calls }
impl SearchMode {
    const ALL: [Self; 5] = [Self::Filename, Self::ScriptType, Self::Loads, Self::Saves, Self::Calls];
    fn label(self) -> &'static str { match self {
        Self::Filename => "Filename", Self::ScriptType => "Script type", Self::Loads => "Loads",
        Self::Saves => "Saves", Self::Calls => "Calls",
    }}
}


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

// Shared external opener for archive entries. Files use their associated app;
// directories use the system file manager. Missing entries remain an error.
pub(crate) fn open_indexed_entry(root: &Path, relative: &str) -> io::Result<()> {
    let candidate = Path::new(relative);
    if candidate.is_absolute() || candidate.components().any(|c| matches!(c,
        std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_))) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unsafe archive-relative path"));
    }
    let absolute = root.join(candidate);
    if absolute.is_dir() {
        reveal_in_file_manager(root, relative)
    } else {
        open_indexed_file(root, relative)
    }
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


// Inventory-only FASTQ summary: never create provenance edges from directory membership.
pub(crate) fn is_fastq_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".fastq", ".fastq.gz", ".fq", ".fq.gz"].iter().any(|ext| p.ends_with(ext))
}

// Keep independently located FASTQ trees separate, but collapse sibling sets
// such as data/set1 and data/set2 to their shared data/ parent.
pub(crate) fn fastq_folders(paths: &[String]) -> Vec<(String, usize)> {
    let mut counts = BTreeMap::<String, usize>::new();
    for path in paths {
        let parent = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
        let parts: Vec<_> = parent.components().collect();
        let folder = if parts.len() >= 2 {
            // First meaningful directory is a stable grouping within an analysis.
            parts[0].as_os_str().to_string_lossy().to_string()
        } else { parent.to_string_lossy().to_string() };
        *counts.entry(folder).or_default() += 1;
    }
    counts.into_iter().collect()
}

pub(crate) enum ScanEvent {
    Progress(String),
    Finished(Result<ScanResult, String>),
    Resolved(Result<(i64, i64), String>),
    // Constructed entirely off the egui thread; the GUI only swaps vectors.
    Loaded { view: Box<App>, summary: String },
}

pub(crate) struct App {
    pub(crate) root: Option<PathBuf>, pub(crate) db: Option<folder_history::core::history_db::HistoryDatabase>, pub(crate) index_path: Option<PathBuf>,
    pub(crate) tab: Tab, pub(crate) query: String, search_mode: SearchMode, search_group: String, search_steps: Vec<(String,String)>, pub(crate) artifacts: Vec<Artifact>, pub(crate) relations: Vec<Relation>,
    pub(crate) selected: Option<i64>, pub(crate) status: String, pub(crate) from: String, pub(crate) to: String,
    pub(crate) relation_kind: String, pub(crate) note: String, pub(crate) show_all: bool, pub(crate) issues: Vec<Issue>, pub(crate) issue_filter: String, pub(crate) ignored_review_scripts: HashSet<String>, pub(crate) active_dependency: Option<(String,String)>, pub(crate) resolution_note: String, pub(super) wizard_index: usize, pub(super) wizard_mode: bool, pub(super) wizard_data: Option<folder_history::core::history_db::WizardInternals>, pub(super) wizard_source: String, pub(super) classification_errors: HashMap<String, String>, pub(crate) scan_summary: String,
    pub(crate) group_filter: String, pub(crate) group_names: HashMap<String, String>, pub(crate) group_name_edit: String,
    pub(crate) analysis_groups: Vec<analysis_graph::AnalysisGroup>, pub(crate) analysis_assignment: HashMap<String, String>,
    pub(crate) nav_history: Vec<NavState>, pub(crate) nav_cursor: usize,
    pub(crate) recent_folders: Vec<PathBuf>, pub(crate) include_hidden: bool, missing_index: Option<PathBuf>, error_popup: Option<String>, pub(crate) open_events: Option<Receiver<App>>, open_cancel: Option<Arc<AtomicBool>>, pub(crate) scan_after_open: bool, pub(crate) scan_events: Option<Receiver<ScanEvent>>, pub(crate) scan_cancel: Option<Arc<AtomicBool>>,
}
impl Default for App {
    fn default() -> Self {
        Self { root: None, db: None, index_path: None, tab: Tab::Search,
            query: String::new(), search_mode: SearchMode::Filename, search_group: String::new(), search_steps: Vec::new(), artifacts: vec![], relations: vec![], selected: None,
            status: "Open a research folder to begin.".into(), from: String::new(),
            to: String::new(), relation_kind: "derived_from".into(), note: String::new(),
            show_all: false, issues: vec![], issue_filter: "unresolved".into(), ignored_review_scripts: HashSet::new(), active_dependency: None, resolution_note: String::new(), wizard_index: 0, wizard_mode: false, wizard_data: None, wizard_source: String::new(), classification_errors: HashMap::new(), scan_summary: String::new(),
            group_filter: String::new(), group_names: HashMap::new(), group_name_edit: String::new(),
            analysis_groups: Vec::new(), analysis_assignment: HashMap::new(),
            nav_history: vec![], nav_cursor: 0,
            recent_folders: load_recent_folders(), include_hidden: false, missing_index: None, error_popup: None, open_events: None, open_cancel: None, scan_after_open: false, scan_events: None, scan_cancel: None }
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
        let cancel = Arc::new(AtomicBool::new(false));
        self.open_cancel = Some(Arc::clone(&cancel));
        self.missing_index = None;
        self.scan_after_open = analyze;
        self.status = format!("Loading archive {}…", root.display());
        std::thread::spawn(move || {
            // SQLite reads and analysis-group construction must never block egui.
            let mut loaded = App::default();
            loaded.open_sync(root, analyze, &cancel);
            if !cancel.load(Ordering::Relaxed) { let _ = tx.send(loaded); }
        });
    }

    fn poll_open(&mut self) {
        let result = self.open_events.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(mut loaded) = result {
            self.open_events = None;
            self.open_cancel = None;
            if loaded.db.is_none() {
                self.missing_index = loaded.missing_index.take();
                self.scan_after_open = false;
                self.status = loaded.status;
                if self.missing_index.is_none() { self.error_popup = Some(self.status.clone()); }
                return;
            }
            self.db = None; // Release any previous archive connection before its workspace.
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
            self.open_cancel = None;
            self.scan_after_open = false;
            self.status = "Archive opening failed: background worker disconnected".into();
        }
    }

    fn open_sync(&mut self, root: PathBuf, analyze: bool, cancel: &AtomicBool) {
        let result: Result<folder_history::core::history_db::HistoryDatabase> = (|| {
            if cancel.load(Ordering::Relaxed) { anyhow::bail!("Opening cancelled"); }
            let database = folder_history::core::history_db::HistoryDatabase::open(&root, !analyze, analyze)?;
            if cancel.load(Ordering::Relaxed) { anyhow::bail!("Opening cancelled"); }
            Ok(database)
        })();
        match result {
            Ok(db) => {
                let local = db.local_path().to_path_buf();
                let archive = db.archive_path().to_path_buf();
                if cancel.load(Ordering::Relaxed) { return; }
                self.recent_folders.retain(|p| p != &root);
                self.recent_folders.insert(0, root.clone());
                self.recent_folders.truncate(5);
                let recent_save_error = save_recent_folders(&self.recent_folders).err();
                self.nav_history.clear(); self.nav_cursor = 0;
                self.root = Some(root); self.db = Some(db);
                self.index_path = Some(local);
                self.selected = None; self.active_dependency = None; self.group_filter.clear();
                self.tab = if analyze { Tab::Build } else { Tab::Spectator };
                self.refresh(); self.status = format!("Opened local copy of {}", archive.display());
                if cancel.load(Ordering::Relaxed) { return; }
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
        self.artifacts.clear(); self.relations.clear(); self.issues.clear(); self.group_names.clear(); self.search_steps.clear();
        self.analysis_groups.clear(); self.analysis_assignment.clear(); self.ignored_review_scripts.clear();
        let Some(db) = &self.db else { return; };
        use folder_history::core::history_db::DbArchiveView;
        let result = db.archive_view();
        match result { Ok((a,r,i)) => { self.artifacts=a;self.relations=r;self.issues=i; }, Err(e) => self.status=format!("Database error: {e:#}") }
        self.analysis_groups = analysis_graph::detect(&self.artifacts, &self.relations);
        for group in &self.analysis_groups {
            for path in &group.paths { self.analysis_assignment.insert(path.clone(), group.key.clone()); }
        }
        if let Some(db) = &self.db {
            self.search_steps = folder_history::core::search::load_steps(db).unwrap_or_default();
            use folder_history::core::history_db::DbScriptReview;
            match db.ignored_review_scripts() {
                Ok(ignored) => self.ignored_review_scripts = ignored,
                Err(e) => self.status = format!("Database error loading ignored scripts: {e:#}"),
            }
            match db.analysis_names() {
                Ok(names) => self.group_names = names.into_iter().collect(),
                Err(e) => self.status = format!("Database error loading analysis names: {e:#}"),
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
    pub(crate) fn publish(&mut self) {
        if self.scan_events.is_some() { return; }
        let Some(db) = &mut self.db else { return; };
        match db.publish() {
            Ok(()) => self.status = "Published local SQLite snapshot to archive.".into(),
            Err(e) => self.status = format!("Publication failed: {e:#}"),
        }
    }
    // All expensive post-processing belongs to the worker, never poll_scan().
    // Uses a separate local SQLite connection; no egui state crosses into the worker.
    fn finish_scan_in_worker(
        root: PathBuf,
        index: PathBuf,
        tx: &mpsc::Sender<ScanEvent>,
        summary: String,
        generate_html: bool,
    ) -> Result<()> {
        let _ = tx.send(ScanEvent::Progress("Loading provenance and analysis groups…".into()));
        let mut view = App::default();
        view.index_path = Some(index.clone());
        let db = folder_history::core::history_db::HistoryDatabase::attach_worker(&root, &index)?;
        view.root = Some(root);
        view.db = Some(db);
        view.refresh();
        if view.status.starts_with("Database error:") {
            anyhow::bail!("{}", view.status);
        }
        if generate_html {
            let _ = tx.send(ScanEvent::Progress("Generating missing documentation…".into()));
            view.ensure_missing_html();
        }
        let _ = tx.send(ScanEvent::Progress("Publishing SQLite snapshot to archive…".into()));
        view.publish();
        if view.status.starts_with("Publication failed:") {
            anyhow::bail!("{}", view.status);
        }
        let _ = tx.send(ScanEvent::Loaded { view: Box::new(view), summary });
        Ok(())
    }

    // Only cheap ownership transfers happen on the GUI thread.
    fn install_scan_view(&mut self, mut view: App, summary: String) {
        self.artifacts = std::mem::take(&mut view.artifacts);
        self.relations = std::mem::take(&mut view.relations);
        self.issues = std::mem::take(&mut view.issues);
        self.group_names = std::mem::take(&mut view.group_names);
        self.search_steps = std::mem::take(&mut view.search_steps);
        self.ignored_review_scripts = std::mem::take(&mut view.ignored_review_scripts);
        self.wizard_data = None;
        self.classification_errors.clear();
        self.analysis_groups = std::mem::take(&mut view.analysis_groups);
        self.analysis_assignment = std::mem::take(&mut view.analysis_assignment);
        self.scan_summary = summary.clone();
        self.status = summary;
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
        self.status = "Re-scan running…".into();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let scan_result = {
                    let mut db = folder_history::core::history_db::HistoryDatabase::attach_worker(&root, &index)?;
                    db.scan(hidden, &worker_cancel, |message| {
                        let _ = tx.send(ScanEvent::Progress(message));
                    })?
                }; // Close the writer before loading and publishing the snapshot.
                let summary = format!("Inventory: {} files | Scripts: {} found, {} parsed | Shell steps: {} | Extracted references: {} | Warnings: {}",
                    scan_result.files, scan_result.scripts, scan_result.parsed, scan_result.steps, scan_result.links, scan_result.warnings);
                App::finish_scan_in_worker(root, index, &tx, summary, true)
            })();
            if let Err(e) = result {
                let _ = tx.send(ScanEvent::Finished(Err(format!("{e:#}"))));
            }
        });
    }
    pub(crate) fn begin_relink(&mut self) {
        if self.scan_events.is_some() { return; }
        let Some(index) = self.index_path.clone() else { return; };
        let Some(root_for_relink) = self.root.clone() else { return; };
        let (tx, rx) = mpsc::channel();
        self.scan_events = Some(rx);
        self.status = "Re-evaluating stored provenance evidence (no filesystem scan)...".into();
        std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let (notebooks, chains) = {
                    let mut db = folder_history::core::history_db::HistoryDatabase::attach_worker(&root_for_relink, &index)?;
                    db.relink()?
                };
                let root = root_for_relink;
                let summary = format!("Re-resolved cached evidence: {notebooks} notebook candidates, {chains} script-chain candidates. Manual resolutions preserved.");
                App::finish_scan_in_worker(root, index, &tx, summary, false)
            })();
            if let Err(e) = result {
                let _ = tx.send(ScanEvent::Resolved(Err(format!("{e:#}"))));
            }
        });
    }
    pub(crate) fn poll_scan(&mut self) {
        let mut completed = None;
        if let Some(rx) = &self.scan_events {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ScanEvent::Progress(message) => {
                        self.scan_summary = message.clone();
                        self.status = message;
                    }
                    other => { completed = Some(other); break; }
                }
            }
        }
        match completed {
            Some(ScanEvent::Loaded { view, summary }) => {
                self.scan_events = None;
                self.scan_cancel = None;
                self.install_scan_view(*view, summary);
            }
            Some(ScanEvent::Finished(Err(e))) | Some(ScanEvent::Resolved(Err(e))) => {
                self.scan_events = None;
                self.scan_cancel = None;
                self.scan_summary = format!("Background operation failed: {e}");
                self.status = self.scan_summary.clone();
                self.error_popup = Some(self.status.clone());
            }
            Some(ScanEvent::Finished(Ok(_))) | Some(ScanEvent::Resolved(Ok(_))) => {
                self.scan_events = None;
                self.scan_cancel = None;
                self.status = "Background worker ended without a refreshed view".into();
            }
            _ => {}
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
        if self.scan_events.is_some() || self.open_events.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        // The worker can hold a SQLite write lock. The archive views issue
        // synchronous queries, so only render scan progress until it finishes.
        if self.scan_events.is_some() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("FolderHistory");
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(&self.status);
                });
                if let Some(cancel) = &self.scan_cancel {
                    let cancelling = cancel.load(Ordering::Relaxed);
                    if ui.add_enabled(!cancelling, egui::Button::new(
                        if cancelling { "Cancelling…" } else { "Cancel scan" },
                    )).clicked() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            });
            return;
        }
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
                ui.menu_button(concat!("About v", env!("CARGO_PKG_VERSION")), |ui| {
                    super::diagnostics::show(ui);
                });
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
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.open_events.is_some() || self.scan_events.is_some() { ui.spinner(); }
                ui.add(egui::Label::new(&self.status).truncate());
                if self.open_events.is_some() || self.scan_cancel.is_some() {
                    if ui.button("■ Stop").on_hover_text("Cancel archive opening or request scan cancellation").clicked() {
                        if let Some(cancel) = self.open_cancel.take() {
                            cancel.store(true, Ordering::Relaxed);
                            // Detach a blocked filesystem worker; ignore its eventual result.
                            self.open_events = None;
                            self.scan_after_open = false;
                            self.status = "Archive opening stopped; an in-flight filesystem call may still finish in the background.".into();
                        } else if let Some(cancel) = &self.scan_cancel {
                            cancel.store(true, Ordering::Relaxed);
                            self.status = "Stopping scan; waiting for the current filesystem operation...".into();
                        }
                    }
                }
            });
        });
        if let Some(message) = self.error_popup.clone() {
            egui::Window::new("⚠ FolderHistory operation failed")
                .collapsible(false).resizable(true).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.colored_label(egui::Color32::YELLOW, "The operation could not complete.");
                    ui.label(&message);
                    if ui.button("Close").clicked() { self.error_popup = None; }
                });
        }
        if self.scan_events.is_some() {
            ctx.request_repaint();
            return;
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
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("search_analysis_group")
                            .selected_text(if self.search_group.is_empty() { "All analyses".to_owned() } else {
                                self.group_names.get(&self.search_group).cloned().or_else(||
                                    self.analysis_groups.iter().find(|g| g.key == self.search_group).map(|g| g.suggested_name.clone()))
                                    .unwrap_or_else(|| self.search_group.clone())
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.search_group, String::new(), "All analyses");
                                for group in &self.analysis_groups {
                                    let label = self.group_names.get(&group.key).cloned().unwrap_or_else(|| group.suggested_name.clone());
                                    ui.selectable_value(&mut self.search_group, group.key.clone(), label);
                                }
                            });
                        egui::ComboBox::from_id_salt("search_mode")
                            .selected_text(self.search_mode.label())
                            .show_ui(ui, |ui| {
                                for mode in SearchMode::ALL { ui.selectable_value(&mut self.search_mode, mode, mode.label()); }
                            });
                    });
                    ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(match self.search_mode {
                        SearchMode::Filename => "Filename, e.g. *.h5ad",
                        SearchMode::ScriptType => "Script extension, e.g. *.ipynb, *.R, *.sh",
                        SearchMode::Loads => "Read files, e.g. *.fastq.gz",
                        SearchMode::Saves => "Written files, e.g. *.tsv",
                        SearchMode::Calls => "Function or command, e.g. write_stats_tables",
                    }));
                    ui.checkbox(&mut self.show_all, "Show all files (without search)");
                    ui.separator();
                    let matches: Vec<(i64,String,String,String)> = folder_history::core::search::find(
                        &self.artifacts, &self.relations, &self.analysis_assignment,
                        &self.search_group, self.search_mode.label(), &self.query, self.show_all, &self.search_steps, 500);
                    ui.small(format!("{} matching indexed files", matches.len()));
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (id,path,kind,reason) in matches {
                            let response = ui.selectable_label(self.selected==Some(id), format!("[{kind}] {path}  {reason}"))
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
                        let links: Vec<Relation> = self.relations.iter()
                            .filter(|r| r.target == path || (r.source == path
                                && !is_directory_child_link(&r.kind, &r.evidence)))
                            .cloned().collect();
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
                    // Show raw FASTQs regardless of whether the processing pipeline is archived.
                    // The analysis filter prevents mixing independent research projects.
                    let fastqs: Vec<String> = self.artifacts.iter()
                        .filter(|a| is_fastq_path(&a.path))
                        .filter(|a| self.group_filter.is_empty() || self.analysis_assignment.get(&a.path) == Some(&self.group_filter))
                        .map(|a| a.path.clone()).collect();
                    let fastq_count = fastqs.len();
                    ui.collapsing(format!("FASTQs ({fastq_count})"), |ui| {
                        for (folder, count) in fastq_folders(&fastqs) {
                            ui.horizontal(|ui| {
                                ui.label(format!("{folder}/ · {count} files"));
                                if ui.small_button("📂 Open folder").clicked() {
                                    if let Some(root) = &self.root {
                                        if let Err(e) = reveal_in_file_manager(root, &folder) {
                                            self.status = format!("Cannot open FASTQ folder: {e}");
                                        }
                                    }
                                }
                            });
                        }
                        // A complete path requires every FASTQ to reach an existing
                        // non-FASTQ result through recorded relationships.
                        let complete = !fastqs.is_empty() && fastqs.iter().all(|fq| {
                            let mut visited = std::collections::HashSet::<String>::new();
                            let mut pending = vec![fq.as_str()];
                            let mut reaches_result = false;
                            while let Some(current) = pending.pop() {
                                if !visited.insert(current.to_owned()) { continue; }
                                for edge in &self.relations {
                                    if edge.source == current {
                                        if !is_fastq_path(&edge.target) &&
                                            self.artifacts.iter().any(|a| a.path == edge.target && a.kind != "script" && a.kind != "notebook") {
                                            reaches_result = true;
                                        }
                                        pending.push(edge.target.as_str());
                                    }
                                }
                            }
                            reaches_result
                        });
                        ui.label(format!("Complete FASTQ → results provenance: {}", if complete { "Yes" } else { "No" }));
                    });
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
                    ui.heading("Script provenance review");
                    ui.small("Folder → script / notebook → references. Double-click opens the source externally.");
                    // Status is per script, never per individual occurrence.
                    let mut open_by_script = HashMap::<String, usize>::new();
                    for issue in &self.issues {
                        if issue.decision == "open" {
                            *open_by_script.entry(issue.notebook.clone()).or_default() += 1;
                        }
                    }
                    let mut scripts = BTreeMap::<String, Vec<String>>::new();
                    for artifact in &self.artifacts {
                        let path = &artifact.path;
                        let lower = path.to_ascii_lowercase();
                        if [".py", ".r", ".rmd", ".ipynb", ".sh", ".nf", ".rscript"]
                            .iter().any(|ext| lower.ends_with(ext)) {
                            let folder = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(".");
                            scripts.entry(folder.to_owned()).or_default().push(path.clone());
                        }
                    }
                    let mut unresolved_scripts = 0usize;
                    let mut resolved_scripts = 0usize;
                    let mut ignored_scripts = 0usize;
                    for entries in scripts.values() {
                        for source in entries {
                            if !self.group_filter.is_empty() &&
                                self.analysis_assignment.get(source) != Some(&self.group_filter) { continue; }
                            if self.ignored_review_scripts.contains(source) {
                                ignored_scripts += 1;
                            } else if open_by_script.get(source).copied().unwrap_or(0) > 0 {
                                unresolved_scripts += 1;
                            } else {
                                resolved_scripts += 1;
                            }
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        for (category, count) in [("unresolved", unresolved_scripts), ("ignored", ignored_scripts), ("resolved", resolved_scripts)] {
                            if ui.selectable_label(self.issue_filter == category, format!("{category} ({count})")).clicked() {
                                self.issue_filter = category.into();
                                self.active_dependency = None;
                            }
                        }
                    });
                    let show_unresolved = self.issue_filter != "resolved";
                    let show_ignored = self.issue_filter == "ignored";
                    let mut folders = BTreeMap::<String, BTreeMap<String, Vec<(String, String, usize)>>>::new();
                    for (folder, entries) in scripts {
                        for source in entries {
                            if !self.group_filter.is_empty() &&
                                self.analysis_assignment.get(&source) != Some(&self.group_filter) { continue; }
                            let has_open = open_by_script.get(&source).copied().unwrap_or(0) > 0;
                            if self.ignored_review_scripts.contains(&source) != show_ignored { continue; }
                            if !show_ignored && has_open != show_unresolved { continue; }
                            folders.entry(folder.clone()).or_default().entry(source).or_default();
                        }
                    }
                    if show_unresolved {
                        for issue in self.issues.iter().filter(|i| i.decision == "open") {
                            let folder = issue.notebook.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(".");
                            if let Some(refs) = folders.get_mut(folder).and_then(|scripts| scripts.get_mut(&issue.notebook)) {
                                if let Some(existing) = refs.iter_mut().find(|(category, filename, _)|
                                    category == &issue.category && filename == &issue.filename) {
                                    existing.2 += 1;
                                } else {
                                    refs.push((issue.category.clone(), issue.filename.clone(), 1));
                                }
                            }
                        }
                    } else {
                        for edge in &self.relations {
                            if is_directory_child_link(&edge.kind, &edge.evidence) { continue; }
                            let folder = edge.source.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(".");
                            if let Some(refs) = folders.get_mut(folder).and_then(|scripts| scripts.get_mut(&edge.source)) {
                                if matches!(edge.kind.as_str(), "reads" | "writes" | "writes_directory") &&
                                    !refs.iter().any(|(_, path, _)| path == &edge.target) {
                                    refs.push((format!("✓ {} · {}", if edge.manual { "manual" } else { "auto" }, edge.kind), edge.target.clone(), 1));
                                }
                            }
                        }
                    }
                    let mut review_action: Option<(String, bool)> = None;
                    let mut selected_script: Option<(String, Option<(String, String)>)> = None;
                    use super::file::{self, FileAction};
                    let mut file_action = None;
                    let mut open_folder: Option<String> = None;
                    egui::ScrollArea::vertical().id_salt("repair_script_tree").max_height(420.0).show(ui, |ui| {
                        for (folder, scripts) in folders {
                            let count: usize = scripts.values().map(|refs| refs.iter().map(|r| r.2).sum::<usize>()).sum();
                            let folder_header = egui::collapsing_header::CollapsingState::load_with_default_open(
                                ui.ctx(), ui.make_persistent_id(("repair_folder_open", &folder)), false);
                            folder_header.show_header(ui, |ui| {
                                if ui.label(format!("📁 {folder} · {} scripts · {count} occurrences", scripts.len()))
                                    .on_hover_text("Double-click to open folder").double_clicked() {
                                    open_folder = Some(folder.clone());
                                }
                            }).body(|ui| {
                                for (script, refs) in scripts {
                                    let name = script.rsplit('/').next().unwrap_or(&script);
                                    let count: usize = refs.iter().map(|r| r.2).sum();
                                    ui.horizontal(|ui| {
                                        let label = format!("{name} · {count} {}", if show_unresolved { "issues" } else { "links" });
                                        match file::show(ui, &label, &script, false, None) {
                                            Some(FileAction::Select) => selected_script = Some((script.clone(), None)),
                                            Some(action) => file_action = Some((script.clone(), action)),
                                            None => {}
                                        }
                                        if ui.small_button("Review").clicked() { selected_script = Some((script.clone(), None)); }
                                        if ui.small_button("Open").clicked() { file_action = Some((script.clone(), FileAction::Open)); }
                                        if show_ignored {
                                            if ui.small_button("Restore").clicked() { review_action = Some((script.clone(), false)); }
                                        } else if ui.small_button("Ignore").on_hover_text("Move to Ignored; scanning and automatic provenance continue").clicked() {
                                            review_action = Some((script.clone(), true));
                                        }
                                    });
                                    ui.indent(("repair_refs", &script), |ui| {
                                        for (category, filename, occurrences) in &refs {
                                            let label = format!("{filename} ({occurrences}) · {category}");
                                            match file::show(ui, &label, filename, false, None) {
                                                Some(FileAction::Select) => {
                                                    selected_script = Some((script.clone(), Some((category.clone(), filename.clone()))));
                                                }
                                                Some(action) => file_action = Some((filename.clone(), action)),
                                                None => {}
                                            }
                                        }
                                    });
                                }
                            });
                        }
                    });
                    if let Some((source, ignored)) = review_action {
                        use folder_history::core::history_db::DbScriptReview;
                        if let Some(db) = &self.db {
                            match db.set_script_ignored(&source, ignored) {
                                Ok(()) => {
                                    if ignored { self.ignored_review_scripts.insert(source); }
                                    else { self.ignored_review_scripts.remove(&source); }
                                    self.publish();
                                }
                                Err(e) => self.status = format!("Review status save failed: {e:#}"),
                            }
                        }
                    }
                    if let Some(folder) = open_folder {
                        if let Some(root) = &self.root {
                            if let Err(e) = open_indexed_entry(root, &folder) { self.status = format!("Cannot open folder: {e}"); }
                        }
                    }
                    if let (Some((path, action)), Some(root)) = (file_action, &self.root) {
                        if let Err(e) = file::perform(ui, root, &path, action) {
                            self.status = format!("Cannot access {path}: {e}");
                        }
                    }
                    if let Some((source, reference)) = selected_script {
                        if let Some(index) = self.issues.iter().position(|i| i.notebook == source &&
                            reference.as_ref().is_none_or(|(category, filename)| &i.category == category && &i.filename == filename)) {
                            self.wizard_index = index;
                            let issue = &self.issues[index];
                            self.active_dependency = Some((issue.category.clone(), issue.filename.clone()));
                            self.resolution_note = issue.note.clone();
                            self.wizard_mode = true;
                        } else {
                            self.wizard_source = source;
                            self.wizard_data = None;
                            self.active_dependency = Some(("script".into(), "<script review>".into()));
                            self.wizard_mode = true;
                        }
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

#[cfg(test)]
mod fastq_summary_tests {
    use super::*;

    #[test]
    fn fastq_extensions_and_folder_groups() {
        assert!(is_fastq_path("data/set1/sample_R1.fastq.gz"));
        assert!(is_fastq_path("data/set2/sample_R2.FQ"));
        assert!(!is_fastq_path("data/set2/matrix.mtx.gz"));
        let paths = vec!["data/set1/a.fastq.gz".to_string(), "data/set2/b.fq.gz".to_string(), "other/c.fastq".to_string()];
        assert_eq!(fastq_folders(&paths), vec![("data".into(), 2), ("other".into(), 1)]);
    }

    #[test]
    fn relocated_h5_fixture_is_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/unlinked_fastq_fixture");
        assert!(root.join("analysis1/analyze.py").is_file());
        for i in 1..=3 {
            assert!(root.join(format!("data/set{i}/set{i}.h5")).is_file());
            assert!(!root.join(format!("analysis1/set{i}/set{i}.h5")).exists());
        }
    }
}
