//! Spectator UI: isolated from the provenance scanner and database.
use super::*;
use super::gui::file::{self, FileAction};

fn apply_file_action(app: &mut App, ui: &mut egui::Ui, path: &str, id: Option<i64>, action: FileAction) {
    if action == FileAction::Select {
        if let Some(id) = id { app.selected = Some(id); }
    } else if let Some(root) = &app.root {
        if let Err(e) = file::perform(ui, root, path, action) {
            app.status = format!("Cannot access {path}: {e}");
        }
    }
}


fn is_fastq(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".fastq", ".fastq.gz", ".fq", ".fq.gz"].iter().any(|ext| lower.ends_with(ext))
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

// A shape is always visible; fill indicates recorded relationships.
#[derive(Clone, Copy)]
enum RelationshipShape { Raw, Input, Output }

fn relationship_badge(ui: &mut egui::Ui, shape: RelationshipShape, count: usize, label: &str) {
    let color = match shape {
        RelationshipShape::Raw => egui::Color32::from_rgb(180, 125, 240),
        RelationshipShape::Input => egui::Color32::from_rgb(100, 175, 255),
        RelationshipShape::Output => egui::Color32::from_rgb(235, 205, 90),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(15.0, 15.0), egui::Sense::hover());
    let center = rect.center();
    let fill = if count > 0 { color } else { egui::Color32::TRANSPARENT };
    let stroke = egui::Stroke::new(1.5, if count > 0 { color } else { egui::Color32::GRAY });
    let painter = ui.painter();
    match shape {
        RelationshipShape::Raw => { painter.circle(center, 5.0, fill, stroke); }
        RelationshipShape::Input => {
            painter.rect(rect.shrink(3.0), 0.0, fill, stroke, egui::StrokeKind::Middle);
        }
        RelationshipShape::Output => {
            let points = vec![
                egui::pos2(center.x - 4.5, center.y - 5.0),
                egui::pos2(center.x + 5.0, center.y),
                egui::pos2(center.x - 4.5, center.y + 5.0),
            ];
            painter.add(egui::Shape::convex_polygon(points, fill, stroke));
        }
    }
    ui.label(format!("{count} {label}"));
}

// A script is a provenance actor, not a collection inferred from its filename.
fn is_analysis_script(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".py", ".r", ".rmd", ".ipynb", ".sh", ".nf", ".rscript"]
        .iter().any(|suffix| p.ends_with(suffix))
}

// Use recorded writes to identify generated scripts within this analysis.
fn split_generated_scripts<'a>(scripts: &[&'a str], relations: &[Relation])
    -> (Vec<&'a str>, Vec<&'a str>)
{
    let script_paths: std::collections::HashSet<_> = scripts.iter().copied().collect();
    let generated: std::collections::HashSet<_> = relations.iter()
        .filter(|edge| edge.kind == "writes" && edge.source != edge.target
            && script_paths.contains(edge.source.as_str()))
        .map(|edge| edge.target.as_str())
        .collect();
    scripts.iter().copied().partition(|script| !generated.contains(script))
}

fn show_spectator_file(
    ui: &mut egui::Ui,
    path: &str,
    by_path: &HashMap<&str, &Artifact>,
    relations: &[Relation],
    selected: Option<i64>,
    actions: &mut Vec<(String, Option<i64>, FileAction)>,
) {
    let Some(a) = by_path.get(path) else { return; };
    let (color, _) = file_role(path, &a.kind, relations);
    let filename = path.rsplit('/').next().unwrap_or(path);
    ui.horizontal(|ui| {
        if let Some(action) = file::show(ui, filename, path, selected == Some(a.id), Some(color)) {
            actions.push((path.to_owned(), Some(a.id), action));
        }
        if ui.small_button("Folder").on_hover_text("Open containing directory").clicked() {
            actions.push((path.to_owned(), Some(a.id), FileAction::OpenParent));
        }
    });
}

// Review only physical, unlinked directory trees. Never group across a protected
// FASTQ, script, or provenance endpoint, and never collapse the archive root.
fn reviewable_folders<'a>(unassigned: &[&'a str], protected: &std::collections::HashSet<&str>)
    -> (BTreeMap<String, Vec<&'a str>>, Vec<&'a str>)
{
    let mut groups: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut loose = Vec::new();
    for &path in unassigned {
        let mut parent = path.rsplit_once('/').map(|(p, _)| p);
        let mut best = None;
        while let Some(dir) = parent {
            if dir.is_empty() { break; }
            let prefix = format!("{dir}/");
            if protected.iter().any(|p| *p == dir || p.starts_with(&prefix)) { break; }
            best = Some(dir);
            parent = dir.rsplit_once('/').map(|(p, _)| p);
        }
        if let Some(dir) = best {
            groups.entry(dir.to_owned()).or_default().push(path);
        } else { loose.push(path); }
    }
    (groups, loose)
}

impl App {
    pub(crate) fn show_spectator(&mut self, ui: &mut egui::Ui) {
        ui.heading("Scientific analyses · Spectator");
        ui.label("Folder → scripts / notebooks → raw input, other input, output. Only recorded relationships attach files to scripts.");
        ui.horizontal_wrapped(|ui| {
            ui.label("Relationships:");
            relationship_badge(ui, RelationshipShape::Raw, 1, "raw (purple)");
            relationship_badge(ui, RelationshipShape::Input, 1, "input (blue)");
            relationship_badge(ui, RelationshipShape::Output, 1, "output (yellow)");
            ui.weak("Filled = files present · Outline = none");
        });
        ui.separator();

        let by_path: HashMap<&str, &Artifact> = self.artifacts.iter()
            .map(|a| (a.path.as_str(), a)).collect();
        let relations = &self.relations;
        let mut pending: Vec<(String, Option<i64>, FileAction)> = Vec::new();
        let mut folder_actions: Vec<(String, bool)> = Vec::new();
        let folder_reviews: HashMap<String, String> = self.db.as_ref().and_then(|db| {
            let mut stmt = db.prepare("SELECT path,decision FROM folder_reviews").ok()?;
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .ok()?.collect::<rusqlite::Result<_>>().ok()
        }).unwrap_or_default();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for group in &self.analysis_groups {
                let title = self.group_names.get(&group.key)
                    .map(String::as_str).unwrap_or(&group.suggested_name);
                let paths: std::collections::HashSet<&str> = group.paths.iter().map(String::as_str).collect();
                let issues = self.issues.iter().filter(|i|
                    self.analysis_assignment.get(&i.notebook) == Some(&group.key)
                        && i.decision == "open").count();
                let fastqs: Vec<&str> = group.paths.iter().map(String::as_str)
                    .filter(|p| is_fastq(p)).collect();
                let mut fastq_dirs: BTreeMap<String, usize> = BTreeMap::new();
                for path in &fastqs {
                    let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    *fastq_dirs.entry(dir.to_owned()).or_default() += 1;
                }
                let mut scripts: Vec<&str> = group.paths.iter().map(String::as_str)
                    .filter(|p| is_analysis_script(p)).collect();
                scripts.sort_unstable();
                scripts.dedup();
                let mut assigned = std::collections::HashSet::new();
                for script in &scripts { assigned.insert(*script); }
                for r in relations {
                    if scripts.contains(&r.source.as_str()) && paths.contains(r.target.as_str())
                        && matches!(r.kind.as_str(), "reads" | "writes" | "writes_directory") {
                        assigned.insert(r.target.as_str());
                    }
                }
                let mut unassigned: Vec<&str> = group.paths.iter().map(String::as_str)
                    .filter(|p| !assigned.contains(p) && !is_fastq(p)).collect();
                unassigned.sort_unstable();
                unassigned.dedup();

                let header = format!("{title} · {} files · {issues} open issues · {} links",
                    group.paths.len(), group.edge_count);
                egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(), ui.make_persistent_id(("spectator-folder-v2", &group.key)), false)
                    .show_header(ui, |ui| {
                        ui.label(header);
                        if !fastqs.is_empty() {
                            ui.menu_button(format!("{} FASTQ ▾", fastqs.len()), |ui| {
                                ui.label("Open FASTQ containing directories:");
                                for (dir, count) in &fastq_dirs {
                                    let display = if dir.is_empty() { "." } else { dir.as_str() };
                                    if ui.button(format!("{display} · {count} files")).clicked() {
                                        pending.push((dir.clone(), None, FileAction::OpenParent));
                                        ui.close();
                                    }
                                }
                                if fastq_dirs.len() > 1 && ui.button("Open all directories").clicked() {
                                    for dir in fastq_dirs.keys() {
                                        pending.push((dir.clone(), None, FileAction::OpenParent));
                                    }
                                    ui.close();
                                }
                            });
                        } else { ui.weak("0 FASTQ"); }
                    }).body(|ui| {
                        ui.label(format!("Scripts / notebooks · {}", scripts.len()));
                        let (primary_scripts, generated_scripts) = split_generated_scripts(&scripts, relations);
                        let mut show_script = |ui: &mut egui::Ui, script: &str| {
                            let basename = script.rsplit('/').next().unwrap_or(script);
                            let mut raw = Vec::new();
                            let mut input = Vec::new();
                            let mut output = Vec::new();
                            for r in relations.iter().filter(|r| r.source == script && by_path.contains_key(r.target.as_str())) {
                                match r.kind.as_str() {
                                    "reads" if is_fastq(&r.target) => raw.push(r.target.as_str()),
                                    "reads" => input.push(r.target.as_str()),
                                    "writes" | "writes_directory" => output.push(r.target.as_str()),
                                    _ => {}
                                }
                            }
                            for items in [&mut raw, &mut input, &mut output] {
                                items.sort_unstable();
                                items.dedup();
                            }
                            let id = ui.make_persistent_id(("spectator-script-v2", script));
                            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                                .show_header(ui, |ui| {
                                    let script_id = by_path.get(script).map(|a| a.id);
                                    if let Some(action) = file::show(
                                        ui, basename, script, script_id.is_some() && self.selected == script_id, None,
                                    ) {
                                        pending.push((script.to_owned(), script_id, action));
                                    }
                                    relationship_badge(ui, RelationshipShape::Raw, raw.len(), "raw");
                                    relationship_badge(ui, RelationshipShape::Input, input.len(), "input");
                                    relationship_badge(ui, RelationshipShape::Output, output.len(), "output");
                                    if ui.small_button("Open").clicked() {
                                        pending.push((script.to_owned(), script_id, FileAction::Open));
                                    }
                                }).body(|ui| {
                                    for (label, items) in [("Raw input", &raw), ("Other input", &input), ("Output", &output)] {
                                        ui.collapsing(format!("{label} · {}", items.len()), |ui| {
                                            for path in items {
                                                show_spectator_file(ui, path, &by_path, relations,
                                                    self.selected, &mut pending);
                                            }
                                        });
                                    }
                                    if raw.is_empty() && input.is_empty() && output.is_empty() {
                                        ui.weak("No recorded file relationships — inspect or resolve provenance.");
                                    }
                                });
                        };
                        for script in primary_scripts {
                            show_script(ui, script);
                        }
                        if !generated_scripts.is_empty() {
                            ui.collapsing(format!("Generated scripts · {}", generated_scripts.len()), |ui| {
                                for script in generated_scripts {
                                    show_script(ui, script);
                                }
                            });
                        }
                        if !unassigned.is_empty() {
                            ui.collapsing(format!("Unassigned files · {}", unassigned.len()), |ui| {
                                let mut protected: std::collections::HashSet<&str> = group.paths.iter()
                                    .map(String::as_str).filter(|p| is_fastq(p) || is_analysis_script(p)).collect();
                                for relation in relations {
                                    protected.insert(&relation.source);
                                    protected.insert(&relation.target);
                                }
                                let (folders, loose) = reviewable_folders(&unassigned, &protected);
                                for (dir, members) in folders {
                                    let previous = folder_reviews.get(&dir).map(String::as_str);
                                    ui.horizontal(|ui| {
                                        if ui.label(format!("📁 {dir}/ · {} unlinked files", members.len()))
                                            .on_hover_text("Double-click to open folder").double_clicked() {
                                            pending.push((dir.clone(), None, FileAction::Open));
                                        }
                                        if ui.small_button("Open folder").clicked() {
                                            pending.push((dir.clone(), None, FileAction::OpenParent));
                                        }
                                        if previous == Some("useless") {
                                            ui.weak("Useless (reviewed)");
                                            if ui.small_button("Undo").clicked() { folder_actions.push((dir.clone(), false)); }
                                        } else if ui.small_button("Mark useless").clicked() {
                                            folder_actions.push((dir.clone(), true));
                                        }
                                    });
                                }
                                for path in loose {
                                    show_spectator_file(ui, path, &by_path, relations,
                                        self.selected, &mut pending);
                                }
                            });
                        }
                    });
            }
        });
        for (path, id, action) in pending {
            apply_file_action(self, ui, &path, id, action);
        }
        for (path, useless) in folder_actions {
            if let Some(db) = &self.db {
                let result = if useless {
                    db.execute("INSERT INTO folder_reviews(path,decision) VALUES (?1,'useless')
                        ON CONFLICT(path) DO UPDATE SET decision='useless'", params![path])
                } else {
                    db.execute("DELETE FROM folder_reviews WHERE path=?1", params![path])
                };
                match result {
                    Ok(_) => { self.status = format!("Folder review updated: {path}"); self.publish(); }
                    Err(e) => self.status = format!("Cannot save folder review: {e}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod folder_review_tests {
    use super::*;
    use rusqlite::Connection;
    #[test]
    fn generated_scripts_require_a_recorded_write() {
        let scripts = ["workflow.R", "runRFclust_1.R", "runRFclust_1.sh", "helper.R"];
        let edge = |target: &str, kind: &str| Relation {
            id: 1, source: "workflow.R".into(), target: target.into(),
            kind: kind.into(), evidence: String::new(), manual: false,
        };
        let relations = [edge("runRFclust_1.R", "writes"), edge("helper.R", "reads")];
        let (primary, generated) = split_generated_scripts(&scripts, &relations);
        assert_eq!(primary, ["workflow.R", "runRFclust_1.sh", "helper.R"]);
        assert_eq!(generated, ["runRFclust_1.R"]);
    }
    #[test]
    fn groups_unlinked_images_but_not_protected_trees() {
        let files = ["images/run1/a.tif", "images/run2/b.tif", "raw/set1/a.txt",
            "raw/set2/b.txt", "loose.txt"];
        let protected = std::collections::HashSet::from(["raw/set1/sample.fastq.gz"]);
        let (groups, loose) = reviewable_folders(&files, &protected);
        assert_eq!(groups["images"].len(), 2);
        assert!(!groups.contains_key("raw"));
        assert_eq!(groups["raw/set2"], vec!["raw/set2/b.txt"]);
        assert!(loose.contains(&"loose.txt"));
        assert!(loose.contains(&"raw/set1/a.txt"));
    }
    #[test]
    fn review_decision_survives_schema_reopen() {
        let db = Connection::open_in_memory().unwrap();
        engine::schema(&db).unwrap();
        db.execute("INSERT INTO folder_reviews(path,decision) VALUES ('images','useless')", []).unwrap();
        engine::schema(&db).unwrap();
        let role: String = db.query_row("SELECT decision FROM folder_reviews WHERE path='images'", [], |r| r.get(0)).unwrap();
        assert_eq!(role, "useless");
    }
}
