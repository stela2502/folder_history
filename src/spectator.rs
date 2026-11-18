//! Spectator UI: isolated from the provenance scanner and database.
use super::*;
use super::gui::file::{self, FileAction};

fn apply_file_action(app: &mut App, ui: &mut egui::Ui, path: &str, id: Option<i64>, action: FileAction) {
    match action {
        FileAction::Select => { if let Some(id) = id { app.selected = Some(id); } },
        FileAction::CopyPath => { if let Some(root) = &app.root { ui.ctx().copy_text(root.join(path).display().to_string()); } },
        FileAction::Open | FileAction::OpenParent => {
            if let Some(root) = &app.root {
                let result = if action == FileAction::Open { open_indexed_file(root, path) } else { reveal_in_file_manager(root, path) };
                if let Err(e) = result { app.status = format!("Cannot open {}: {e}", path); }
            }
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

// A script is a provenance actor, not a collection inferred from its filename.
fn is_analysis_script(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".py", ".r", ".rmd", ".ipynb", ".sh", ".nf", ".rscript"]
        .iter().any(|suffix| p.ends_with(suffix))
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

impl App {
    pub(crate) fn show_spectator(&mut self, ui: &mut egui::Ui) {
        ui.heading("Scientific analyses · Spectator");
        ui.label("Folder → scripts / notebooks → raw input, other input, output. Only recorded relationships attach files to scripts.");
        ui.separator();

        let by_path: HashMap<&str, &Artifact> = self.artifacts.iter()
            .map(|a| (a.path.as_str(), a)).collect();
        let relations = &self.relations;
        let mut pending: Vec<(String, Option<i64>, FileAction)> = Vec::new();
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
                        for script in scripts {
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
                                    ui.label(format!("{basename} · {} raw · {} input · {} output",
                                        raw.len(), input.len(), output.len())).on_hover_text(script);
                                    if ui.small_button("Open").clicked() {
                                        pending.push((script.to_owned(), by_path.get(script).map(|a| a.id), FileAction::Open));
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
                        }
                        if !unassigned.is_empty() {
                            ui.collapsing(format!("Unassigned files · {}", unassigned.len()), |ui| {
                                for path in unassigned {
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
    }
}
