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

// Compact role histogram: the same palette as file_role, with one painted glyph
// per category (no emoji or duplicate monochrome Unicode symbols).
fn role_summary<'a>(
    ui: &mut egui::Ui,
    paths: impl IntoIterator<Item = &'a str>,
    by_path: &HashMap<&str, &Artifact>,
    relations: &[Relation],
) {
    let mut counts = [0usize; 7];
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if !seen.insert(path) { continue; }
        let Some(artifact) = by_path.get(path) else { continue; };
        let (_, role) = file_role(&artifact.path, &artifact.kind, relations);
        let index = match role {
            "raw data" => 0,
            "processing script" => 1,
            "processed data (verify)" => 2,
            "analysis script" => 3,
            "table / metadata" => 4,
            "figure / document" => 5,
            _ => 6,
        };
        counts[index] += 1;
    }
    let colors = [
        egui::Color32::from_rgb(100, 175, 255),
        egui::Color32::from_rgb(180, 125, 240),
        egui::Color32::from_rgb(80, 145, 215),
        egui::Color32::from_rgb(245, 105, 105),
        egui::Color32::from_rgb(235, 205, 90),
        egui::Color32::from_rgb(105, 215, 145),
        egui::Color32::GRAY,
    ];
    let names = ["Raw data", "Processing", "Processed data", "Analysis", "Tables", "Figures", "Other / reference"];
    for (i, &count) in counts.iter().enumerate() {
        if count == 0 { continue; }
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(13.0, 16.0), egui::Sense::hover());
            let center = rect.center();
            let painter = ui.painter();
            // Shape identifies role even without color. A single painted shape is
            // used rather than an emoji plus a second Unicode glyph.
            match i {
                2 => {
                    let r = 5.0;
                    painter.add(egui::Shape::convex_polygon(
                        vec![center + egui::vec2(0.0, -r), center + egui::vec2(r, 0.0),
                             center + egui::vec2(0.0, r), center + egui::vec2(-r, 0.0)],
                        colors[i], egui::Stroke::NONE));
                }
                4 => { painter.rect_filled(egui::Rect::from_center_size(center, egui::vec2(9.0, 9.0)), 0.0, colors[i]); }
                5 => {
                    painter.add(egui::Shape::convex_polygon(
                        vec![center + egui::vec2(0.0, -5.5), center + egui::vec2(5.5, 4.5),
                             center + egui::vec2(-5.5, 4.5)], colors[i], egui::Stroke::NONE));
                }
                _ => { painter.circle_filled(center, 5.0, colors[i]); }
            }
            ui.label(count.to_string());
        }).response.on_hover_text(names[i]);
    }
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


impl App {
    pub(crate) fn show_spectator(&mut self, ui: &mut egui::Ui) {

                    ui.heading("Scientific analyses · Spectator");
                    ui.label("Experiment → sample/run → workflow → files. Double-click a filename to open it.");
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
                    // Scientist confirmations are authoritative annotations, not guesses
                    // from filename extensions. Keep source and path together: a relative
                    // filename in one notebook must not annotate a namesake elsewhere.
                    let confirmed: Vec<(String, String, String)> = self.db.as_ref()
                        .and_then(|db| {
                            let mut stmt = db.prepare("SELECT source,path,role FROM file_classifications WHERE confirmed=1 ORDER BY source,path").ok()?;
                            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok()?;
                            Some(rows.flatten().collect())
                        }).unwrap_or_default();
                    ui.label(format!("{} detected groups · {} indexed files · {} confirmed classifications",
                        self.analysis_groups.len(), self.artifacts.len(), confirmed.len()));
                    if !confirmed.is_empty() {
                        ui.collapsing(format!("✓ Scientist-confirmed files and folders ({})", confirmed.len()), |ui| {
                            egui::Grid::new("confirmed_classifications_overview").striped(true).show(ui, |ui| {
                                ui.strong("File / folder"); ui.strong("Classification"); ui.strong("Source script"); ui.end_row();
                                for (source, path, role) in &confirmed {
                                    ui.label(path).on_hover_text(path);
                                    ui.strong(role);
                                    ui.small(source);
                                    ui.end_row();
                                }
                            });
                        });
                    }
                    // The authoritative role table is shared with the scanner and wizard.
                    let confirmed_by_path = self.db.as_ref()
                        .and_then(|db| engine::confirmed_roles(db).ok()).unwrap_or_default();
                    let mut pending_file_actions: Vec<(String, Option<i64>, FileAction)> = Vec::new();
                    let by_path: HashMap<&str, &Artifact> = self.artifacts.iter().map(|a| (a.path.as_str(), a)).collect();
                    let links = &self.relations;
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for group in &self.analysis_groups {
                            let title = self.group_names.get(&group.key).map(String::as_str).unwrap_or(&group.suggested_name);
                            let issues = self.issues.iter().filter(|i| self.analysis_assignment.get(&i.notebook)==Some(&group.key) && i.decision=="open").count();
                            let header = format!("{title} · {} files · {issues} open issues · {} links", group.paths.len(), group.edge_count);
                            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(("analysis", &group.key)), false).show_header(ui, |ui| {
                                ui.label(&header);
                                role_summary(ui, group.paths.iter().map(String::as_str), &by_path, links);
                            }).body(|ui| {
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
                                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(("experiment", &experiment)), false).show_header(ui, |ui| {
                                        ui.label(format!("🧬 {experiment} · {experiment_count} files"));
                                        role_summary(ui, samples.values().flat_map(|w| w.values()).flat_map(|s| s.values()).flat_map(|paths| paths.iter().copied()), &by_path, links);
                                    }).body(|ui| {
                                        for (sample, workflows) in samples {
                                            let sample_count: usize = workflows.values().flat_map(|s| s.values()).map(Vec::len).sum();
                                            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(("sample", &sample)), false).show_header(ui, |ui| {
                                                {
                                                    let script = workflows.get("Scripts and notebooks")
                                                        .and_then(|steps| steps.values().flatten().copied().find(|path| {
                                                            path.rsplit('/').next() == Some(sample.as_str())
                                                        }));
                                                    if let Some(path) = script {
                                                        ui.label("Sample / run:");
                                                        // First visible filename is actionable; the adjacent button
                                                        // is a reliable alternative to double-click in a collapsing header.
                                                        if let Some(action) = file::show(ui, &sample, path, false, None) {
                                                            pending_file_actions.push((path.to_owned(), None, action));
                                                        }
                                                        if ui.small_button("Open").on_hover_text("Open with default application").clicked() {
                                                            pending_file_actions.push((path.to_owned(), None, FileAction::Open));
                                                        }
                                                        ui.label(format!("· {sample_count}"));
                                                    } else {
                                                        ui.label(format!("Sample / run: {sample} · {sample_count}"));
                                                    }
                                                }
                                                role_summary(ui, workflows.values().flat_map(|s| s.values()).flat_map(|paths| paths.iter().copied()), &by_path, links);
                                            }).body(|ui| {
                                                for (workflow, steps) in workflows {
                                                    let workflow_count: usize = steps.values().map(Vec::len).sum();
                                                    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(("workflow", &workflow)), false).show_header(ui, |ui| {
                                                        ui.label(format!("{workflow} · {workflow_count}"));
                                                        role_summary(ui, steps.values().flat_map(|paths| paths.iter().copied()), &by_path, links);
                                                    }).body(|ui| {
                                                        for (step, paths) in steps {
                                                            let path_count = paths.len();
                                                            let is_script_step = workflow == "Scripts and notebooks";
                                                            let mut render_files = |ui: &mut egui::Ui| {
                                                                if step.starts_with("FASTQ library:") {
                                                                    let sources = fastq_source_folders(&paths);
                                                                    ui.label(format!("{} FASTQ files · {} physical folder cluster(s)", paths.len(), sources.len()));
                                                                    for (dir, count) in &sources {
                                                                        ui.horizontal_wrapped(|ui| {
                                                                            ui.label(format!("📁 {dir} · {count} files"));
                                                                            if ui.small_button("Open in file explorer").clicked() {
                                                                                pending_file_actions.push((dir.to_owned(), None, FileAction::OpenParent));
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
                                                                for path in &paths {
                                                                    let Some(a) = by_path.get(path) else { continue; };
                                                                    let (mut color, _role) = file_role(&a.path, &a.kind, links);
                                                                    if let Some(role) = confirmed_by_path.get(a.path.as_str()) {
                                                                        color = match role.as_str() {
                                                                            "Input data" | "Intermediate raw data" => egui::Color32::from_rgb(100,175,255),
                                                                            "Preprocessed data" | "Quantification results" => egui::Color32::from_rgb(80,145,215),
                                                                            "Analyzed data" | "Results folder" => egui::Color32::from_rgb(245,105,105),
                                                                            _ => color,
                                                                        };
                                                                    }
                                                                    let basename = a.path.rsplit('/').next().unwrap_or(&a.path);
                                                                    if let Some(role) = confirmed_by_path.get(a.path.as_str()) {
                                                                        ui.strong(format!("✓ {role}"));
                                                                    }
                                                                    if let Some(action) = file::show(ui, basename, &a.path, self.selected == Some(a.id), Some(color)) {
                                                                        pending_file_actions.push((a.path.clone(), Some(a.id), action));
                                                                    }
                                                                    if self.selected==Some(a.id) {
                                                                        ui.small(&a.path);
                                                                        if ui.small_button("📂 Open containing folder").clicked() {
                                                                            pending_file_actions.push((a.path.clone(), Some(a.id), FileAction::OpenParent));
                                                                        }
                                                                        let mut shown = std::collections::HashSet::new();
                                                                        for r in links.iter().filter(|r| r.source==a.path || r.target==a.path) {
                                                                            let other = if r.source==a.path { &r.target } else { &r.source };
                                                                            let direction = if r.source==a.path { "→" } else { "←" };
                                                                            if !shown.insert((direction, r.kind.as_str(), other.as_str())) { continue; }
                                                                            if shown.len() > 30 { break; }
                                                                            let name = other.rsplit('/').next().unwrap_or(other);
                                                                            ui.small(format!("{direction} {} · {name}", r.kind)).on_hover_text(other);
                                                                        }
                                                                        if ui.small_button("Explore relationships").clicked() { self.tab=Tab::Graph; }
                                                                    }
                                                                }
                                                            };
                                                            if is_script_step {
                                                                render_files(ui);
                                                            } else {
                                                                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id(("step", &step)), false).show_header(ui, |ui| {
                                                                    ui.label(format!("{step} · {path_count}"));
                                                                    role_summary(ui, paths.iter().copied(), &by_path, links);
                                                                }).body(render_files);
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
                    // Apply UI events only after all immutable artifact/group borrows end.
                    for (path, id, action) in pending_file_actions {
                        apply_file_action(self, ui, &path, id, action);
                    }
                    }
}
