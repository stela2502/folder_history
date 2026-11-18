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

fn display_sample_label(sample: &str) -> String {
    if sample == "Shared / unspecified sample" { return "Shared files / unassigned".to_owned(); }
    let lower = sample.to_ascii_lowercase();
    if [".txt", ".ipynb", ".py", ".r", ".sh", ".zip", ".pdf", ".csv", ".tsv"]
        .iter().any(|ext| lower.ends_with(ext)) {
        return format!("File / collection: {sample}");
    }
    format!("Folder / possible sample: {sample}")
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
