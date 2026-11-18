//! File-first classification. Source code is optional evidence, not the question.
use crate::*;
use super::app::{App, open_indexed_file};

#[derive(Clone)]
struct Candidate { path: String, variable: String, folder: bool }

fn looks_like_path(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control)
        && !matches!(value, "\\t" | "\\n" | "\\r") && !value.contains("://")
        && (value.contains('/') || value.contains('\\') || Path::new(value).extension().is_some())
}

// Extract only simple literal assignments. Other syntax belongs to language
// adapters; this function never claims to reconstruct execution order.
fn assignment(line: &str) -> Option<(String, String)> {
    let (lhs, rhs) = line.trim().split_once('=')?;
    let lhs = lhs.trim();
    if lhs.is_empty() || !lhs.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || lhs.chars().next()?.is_ascii_digit() { return None; }
    let rhs = rhs.trim();
    let quote = rhs.chars().next()?;
    if quote != '\'' && quote != '"' { return None; }
    let value = rhs[1..].split(quote).next()?;
    if !looks_like_path(value) { return None; }
    Some((lhs.to_owned(), value.to_owned()))
}

fn candidates(root: &Path, source: &str, issues: &[Issue]) -> Vec<Candidate> {
    let mut found: BTreeMap<String, Candidate> = BTreeMap::new();
    for issue in issues {
        if issue.notebook != source { continue; }
        let path = issue.filename.clone();
        found.entry(path.clone()).or_insert(Candidate {
            folder: Path::new(&path).extension().is_none(), path, variable: String::new(),
        });
    }
    let source_path = Path::new(source);
    if source_path.is_absolute() || source_path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return found.into_values().collect();
    }
    let full = root.join(source_path);
    let content = match std::fs::read_to_string(full) { Ok(s) => s, Err(_) => return found.into_values().collect() };
    let lines: Vec<String> = if source.ends_with(".ipynb") {
        serde_json::from_str::<serde_json::Value>(&content).ok()
            .and_then(|v| v.get("cells")?.as_array().cloned())
            .unwrap_or_default().iter()
            .flat_map(|cell| match cell.get("source") {
                Some(serde_json::Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>(),
                Some(serde_json::Value::String(s)) => vec![s.clone()],
                _ => vec![],
            }).flat_map(|s| s.lines().map(str::to_owned).collect::<Vec<_>>()).collect()
    } else { content.lines().map(str::to_owned).collect() };
    for line in lines {
        if let Some((variable, path)) = assignment(&line) {
            let folder = Path::new(&path).extension().is_none();
            found.entry(path.clone()).and_modify(|c| c.variable = variable.clone())
                .or_insert(Candidate { path, variable, folder });
        }
    }
    found.into_values().collect()
}

// Classifications resolve investigation questions, not unverified read/write edges.
fn matches_issue_path(issue_path: &str, candidate_path: &str) -> bool {
    let issue = issue_path.replace('\\', "/");
    let candidate = candidate_path.replace('\\', "/");
    let issue = issue.trim_end_matches('/');
    let candidate = candidate.trim_end_matches('/');
    issue == candidate || issue.ends_with(&format!("/{candidate}"))
        || candidate.ends_with(&format!("/{issue}"))
}

// Only the selected script is eligible. A short filename must identify exactly
// one candidate; otherwise we must not resolve an unrelated file by accident.
fn resolve_classified_issues(tx: &rusqlite::Transaction<'_>, source: &str, items: &[Candidate]) -> Result<usize> {
    let mut changed = 0;
    let mut stmt = tx.prepare("SELECT rowid, filename FROM issues WHERE notebook=?1 AND decision='open'")?;
    let rows: Vec<(i64, String)> = stmt.query_map(params![source], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    for (rowid, filename) in rows {
        let matches = items.iter().filter(|item| matches_issue_path(&filename, &item.path)).count();
        if matches == 1 {
            changed += tx.execute(
                "UPDATE issues SET decision='classified', note='Scientist confirmed file/folder classification' WHERE rowid=?1 AND decision='open'",
                params![rowid],
            )?;
        }
    }
    Ok(changed)
}

fn suggestion(variable: &str) -> &'static str {
    match variable.to_ascii_lowercase().as_str() {
        "raw" => "Intermediate raw data",
        "analyzed" | "analysed" => "Analyzed data",
        _ => "Unknown",
    }
}

// Existing labels remain selectable so saved classifications are not lost.
const ROLES: &[&str] = &[
    "Unknown",
    "Raw experimental data", "Processed data", "Sample metadata",
    "Reference data", "Configuration / parameters", "Result files",
    "Results folder", "Figures & images", "Other scientific data", "Not relevant",
    // Legacy labels retained for existing databases.
    "Input data", "Other Input Files", "Intermediate raw data",
    "Preprocessed data", "Analyzed data", "Quantification results",
];

// Apply a chosen classification to every candidate in this script.
// This changes only pending UI choices; CONFIRM ALL persists them atomically.
fn set_all_roles(choices: &mut HashMap<String, String>, source: &str, items: &[Candidate], role: &str) {
    for item in items {
        choices.insert(format!("{source}\0{}", item.path), role.to_owned());
    }
}

impl App {
    pub(super) fn draw_investigation(&mut self, ui: &mut egui::Ui) {
        let Some((_, selected)) = self.active_dependency.clone() else { self.wizard_mode = false; return; };
        let Some(root) = self.root.clone() else { self.wizard_mode = false; return; };
        ui.horizontal(|ui| {
            if ui.button("← Back to overview").clicked() { self.wizard_mode = false; }
            ui.separator(); ui.heading("File & folder classification");
        });
        let source = self.issues.get(self.wizard_index)
            .filter(|i| i.filename == selected)
            .map(|i| i.notebook.clone());
        let Some(source) = source else { ui.label("No source script recorded for this reference."); return; };
        ui.label(&source);
        ui.separator();
        let items = candidates(&root, &source, &self.issues);
        if items.is_empty() { ui.label("No classifiable files or folders found in this script."); return; }
        let mut saved: HashMap<String, String> = HashMap::new();
        if let Some(db) = &self.db {
            if let Ok(mut stmt) = db.prepare("SELECT path,role FROM file_classifications WHERE source=?1") {
                if let Ok(rows) = stmt.query_map(params![&source], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                    for row in rows.flatten() { saved.insert(row.0, row.1); }
                }
            }
        }
        // Resolve each reference to a unique indexed artifact and reuse its
        // confirmed role, regardless of which notebook is being investigated.
        // A matching basename alone is not sufficient when paths are ambiguous.
        let shared_roles = self.db.as_ref().and_then(|db| {
            let mut stmt = db.prepare("SELECT path FROM artifacts").ok()?;
            let paths = stmt.query_map([], |r| r.get::<_, String>(0)).ok()?
                .collect::<rusqlite::Result<HashSet<_>>>().ok()?;
            let roles = engine::confirmed_roles(db).ok()?;
            Some((paths, roles))
        });
        for item in &items {
            let key = format!("{source}\0{}", item.path);
            let shared = shared_roles.as_ref().and_then(|(paths, roles)| {
                let path = engine::classified_artifact_path(&item.path, &source, paths)?;
                roles.get(&path).cloned()
            });
            self.classification_choices.entry(key).or_insert_with(|| {
                shared.or_else(|| saved.get(&item.path).cloned())
                    .unwrap_or_else(|| suggestion(&item.variable).to_owned())
            });
        }
        let unknown = items.iter().filter(|item| {
            self.classification_choices.get(&format!("{source}\0{}", item.path))
                .is_none_or(|role| role == "Unknown")
        }).count();
        ui.label(format!("Found {} files/folders · {} classified · {} remaining", items.len(), items.len() - unknown, unknown));
        ui.add_space(8.0);
        egui::ScrollArea::vertical().max_height((ui.available_height() - 230.0).max(120.0)).show(ui, |ui| {
            egui::Grid::new("file_classification_grid").striped(true).min_col_width(55.0).show(ui, |ui| {
                ui.strong("File / folder"); ui.strong("Variable"); ui.strong("Select"); ui.strong("Classification"); ui.end_row();
                for item in &items {
                    let key = format!("{source}\0{}", item.path);
                    ui.label(format!("{} {}", if item.folder { "📁" } else { "📄" }, item.path));
                    ui.label(if item.variable.is_empty() { "—" } else { &item.variable });
                    ui.checkbox(self.classification_selected.entry(key.clone()).or_insert(true), "");
                    let choice = self.classification_choices.get_mut(&key).expect("initialized above");
                    egui::ComboBox::from_id_salt(("file_role", &key)).selected_text(choice.as_str())
                        .show_ui(ui, |ui| { for role in ROLES { ui.selectable_value(choice, (*role).to_owned(), *role); } });
                    ui.end_row();
                }
            });
        });
        ui.add_space(12.0);
        // A horizontal layout gives the frame its natural content width,
        // instead of stretching it to the table or available window width.
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_gray(38))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(58)))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    // Keep selection and classification as two distinct rows.
                    // The outer horizontal layout lets the frame hug its contents.
                    ui.vertical(|ui| {
                    ui.strong("Bulk classification");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Select all").clicked() {
                            for item in &items { self.classification_selected.insert(format!("{source}\0{}", item.path), true); }
                        }
                        if ui.button("Select none").clicked() {
                            for item in &items { self.classification_selected.insert(format!("{source}\0{}", item.path), false); }
                        }
                        if ui.button("Invert selection").clicked() {
                            for item in &items {
                                let key = format!("{source}\0{}", item.path);
                                let current = self.classification_selected.get(&key).copied().unwrap_or(true);
                                self.classification_selected.insert(key, !current);
                            }
                        }
                        let selected_count = items.iter().filter(|item| self.classification_selected.get(&format!("{source}\0{}", item.path)).copied().unwrap_or(true)).count();
                        ui.label(format!("{selected_count}/{} selected", items.len()));
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                    ui.label("Set selected files/folders to:");
                    egui::ComboBox::from_id_salt("bulk_file_role")
                        .selected_text("Choose classification…")
                        .show_ui(ui, |ui| {
                            for role in ROLES.iter().copied().filter(|role| *role != "Unknown") {
                                if ui.selectable_label(false, role).clicked() {
                                    for item in &items {
                                        let key = format!("{source}\0{}", item.path);
                                        if self.classification_selected.get(&key).copied().unwrap_or(true) {
                                            self.classification_choices.insert(key, role.to_owned());
                                        }
                                    }
                                }
                            }
                        });
                    });
                    ui.small("Applies to checked rows only; edit individual rows afterwards.");
                    });
                });
        });
        ui.add_space(10.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("✓ CONFIRM ALL").min_size(egui::vec2(170.0, 38.0))).clicked() {
                if unknown > 0 {
                    self.status = format!("Cannot confirm: {unknown} files or folders are still Unknown. Classify the highlighted entries first.");
                } else {
                let result: Result<usize> = (|| {
                    let db = self.db.as_mut().ok_or_else(|| anyhow::anyhow!("No writable index"))?;
                    let tx = db.transaction()?;
                    let paths: HashSet<String> = {
                        let mut q = tx.prepare("SELECT path FROM artifacts")?;
                        q.query_map([], |r| r.get::<_, String>(0))?
                            .collect::<rusqlite::Result<HashSet<_>>>()?
                    };
                    for item in &items {
                        let role = &self.classification_choices[&format!("{source}\0{}", item.path)];
                        tx.execute("INSERT INTO file_classifications(source,path,role,confirmed) VALUES (?1,?2,?3,1)
                            ON CONFLICT(source,path) DO UPDATE SET role=excluded.role,confirmed=1",
                            params![&source, &item.path, role])?;
                        // Only an unambiguous inventory match is promoted to
                        // the authoritative path-level annotation.
                        if let Some(path) = engine::classified_artifact_path(&item.path, &source, &paths) {
                            tx.execute("INSERT INTO artifact_roles(path,role,source,confirmed)
                                VALUES (?1,?2,?3,1) ON CONFLICT(path) DO UPDATE SET
                                role=excluded.role,source=excluded.source,confirmed=1",
                                params![path, role, &source])?;
                        }
                    }
                    // A confirmed role belongs to the physical artifact, not to
                    // the notebook that happened to reveal it first. Propagate
                    // only when both references resolve uniquely to the same
                    // indexed path; never propagate by basename alone.
                    let mut confirmed_paths = HashSet::new();
                    for item in &items {
                        if let Some(path) = engine::classified_artifact_path(&item.path, &source, &paths) {
                            confirmed_paths.insert(path);
                        }
                    }
                    let mut resolved = resolve_classified_issues(&tx, &source, &items)?;
                    if !confirmed_paths.is_empty() {
                        let mut stmt = tx.prepare("SELECT rowid, notebook, filename FROM issues WHERE decision='open'")?;
                        let open_issues: Vec<(i64, String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                            .collect::<rusqlite::Result<_>>()?;
                        drop(stmt);
                        for (rowid, script, reference) in open_issues {
                            if let Some(path) = engine::classified_artifact_path(&reference, &script, &paths) {
                                if confirmed_paths.contains(&path) {
                                    resolved += tx.execute(
                                        "UPDATE issues SET decision='classified', note='Scientist-confirmed physical artifact classification' WHERE rowid=?1 AND decision='open'",
                                        params![rowid],
                                    )?;
                                }
                            }
                        }
                    }
                    tx.commit()?; Ok(resolved)
                })();
                match result {
                    Ok(resolved) => {
                        self.wizard_mode = false;
                        self.active_dependency = None;
                        ui.ctx().request_repaint();
                        // Keep the user's current Build / Repair category.
                        // Discard cached wizard choices so shared confirmed roles
                        // are loaded afresh when another notebook is opened.
                        self.classification_choices.clear();
                        self.recently_confirmed = Some((source.clone(), selected.clone()));
                        self.refresh();
                        self.publish();
                        self.status = format!("Confirmed {} classifications; resolved {resolved} investigation occurrence(s).", items.len());
                    }
                    Err(e) => self.status = format!("Cannot save classifications: {e:#}"),
                }
                }
            }
            if ui.add(egui::Button::new("📂 OPEN SCRIPT").min_size(egui::vec2(170.0, 38.0))).clicked() {
                if let Err(e) = open_indexed_file(&root, &source) { self.status = format!("Cannot open script: {e}"); }
            }
            if unknown > 0 { ui.label("Classify all unknown entries to confirm."); }
        });
        ui.label(&self.status);
        ui.small("Figures can be classified. Suggestions are editable; confirming does not create global YAML rules.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bulk_classification_updates_every_row() {
        let items = vec![
            Candidate { path: "a.txt".into(), variable: "a".into(), folder: false },
            Candidate { path: "b.txt".into(), variable: "b".into(), folder: false },
            Candidate { path: "data/".into(), variable: "d".into(), folder: true },
        ];
        let mut choices = HashMap::new();
        choices.insert("script.ipynb\0a.txt".into(), "Analyzed data".into());
        set_all_roles(&mut choices, "script.ipynb", &items, "Other Input Files");
        assert_eq!(choices.len(), 3);
        assert!(choices.values().all(|role| role == "Other Input Files"));
    }
    #[test]
    fn classification_labels_include_new_and_legacy_values() {
        for role in ["Sample metadata", "Reference data", "Result files", "Figures & images",
                     "Input data", "Analyzed data", "Quantification results"] {
            assert!(ROLES.contains(&role), "missing role: {role}");
        }
    }
    #[test]
    fn folder_path_matches_prefixed_issue() {
        assert!(matches_issue_path("Local/OMIX236-20/Isabella_OMIX236", "Isabella_OMIX236"));
        assert!(matches_issue_path("data/quant_res/", "data/quant_res"));
        assert!(!matches_issue_path("Isabella_OMIX236_other", "Isabella_OMIX236"));
    }
    #[test]
    fn simple_assignments_and_figures() {
        assert_eq!(assignment("analyzed = \"sample_analyzed.h5ad\""), Some(("analyzed".into(), "sample_analyzed.h5ad".into())));
        assert_eq!(assignment("raw = 'sample_raw.h5ad'"), Some(("raw".into(), "sample_raw.h5ad".into())));
        assert_eq!(assignment("plot = 'figures/plot.png'"),
            Some(("plot".into(), "figures/plot.png".into())));
        assert!(assignment("x = 42").is_none());
        assert!(assignment("sep = '\\t'").is_none());
        assert!(assignment("sep = '\t'").is_none());
    }
}

#[cfg(test)]
mod resolution_tests {
    use super::*;
    #[test]
    fn confirming_classifications_resolves_only_matching_open_investigations() {
        let mut db = Connection::open_in_memory().unwrap();
        let _ = db.execute_batch("CREATE TABLE issues (notebook TEXT, filename TEXT, category TEXT, decision TEXT, note TEXT);
            INSERT INTO issues VALUES ('a.ipynb','sample.h5ad','unresolved','open','');
            INSERT INTO issues VALUES ('a.ipynb','sample.h5ad','ambiguous','open','');
            INSERT INTO issues VALUES ('a.ipynb','sample.h5ad','external','open','');
            INSERT INTO issues VALUES ('b.ipynb','sample.h5ad','unresolved','open','');
            INSERT INTO issues VALUES ('a.ipynb','other.h5ad','unresolved','open','');");
        let tx = db.transaction().unwrap();
        let items = vec![Candidate { path: "sample.h5ad".into(), variable: "analyzed".into(), folder: false }];
        assert_eq!(resolve_classified_issues(&tx, "a.ipynb", &items).unwrap(), 3);
        tx.commit().unwrap();
        let resolved: i64 = db.query_row("SELECT COUNT(*) FROM issues WHERE decision='classified'", [], |r| r.get(0)).unwrap();
        let open: i64 = db.query_row("SELECT COUNT(*) FROM issues WHERE decision='open'", [], |r| r.get(0)).unwrap();
        assert_eq!((resolved,open), (3,2));
    }
}
