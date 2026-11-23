//! Database-backed scientist review: no matching or SQL in the GUI.
use crate::*;
use folder_history::core::history_db::{DbArchiveView, DbIssues, MatchDecision, WizardInternals};
use std::collections::HashSet;
use folder_history::core::ArtifactRole;
use super::app::{App, open_indexed_file, open_indexed_entry};


impl App {
    pub(super) fn draw_investigation(&mut self, ui: &mut egui::Ui) {
        let Some((_, selected)) = self.active_dependency.clone() else { self.wizard_mode = false; return; };
        let Some(root) = self.root.clone() else { self.wizard_mode = false; return; };
        let source = if selected == "<script review>" {
            Some(self.wizard_source.clone())
        } else {
            self.issues.get(self.wizard_index)
                .filter(|i| i.filename == selected)
                .map(|i| i.notebook.clone())
        };
        let Some(source) = source else { ui.label("No source script recorded for this reference."); return; };
        ui.horizontal(|ui| {
            if ui.button("← Back to overview").clicked() { self.wizard_mode = false; }
            ui.separator(); ui.heading("File & folder classification");
        });
        ui.label(&source).on_hover_text(root.join(&source).display().to_string());
        if self.wizard_source != source || self.wizard_data.is_none() {
            self.wizard_source = source.clone();
            self.wizard_data = None;
            self.classification_errors.clear();
            let result = self.db.as_ref().ok_or_else(|| anyhow::anyhow!("No database"))
                .and_then(|db| {
                    let id = db.source_artifact_id(&source)?;
                    db.issues(id)
                });
            match result {
                Ok(data) => self.wizard_data = Some(data),
                Err(e) => { ui.colored_label(egui::Color32::RED, format!("Cannot load script matches: {e:#}")); return; }
            }
        }
        if let Some(data) = self.wizard_data.as_mut() {
            if let Some(path) = show_wizard_data(ui, data, &self.classification_errors) {
                if let Err(e) = open_indexed_entry(&root, &path) {
                    self.status = format!("Cannot open entry: {e}");
                }
            }
        }

        let mut confirm = false;
        ui.horizontal(|ui| {
            confirm = ui.add_sized([170.0, 34.0], egui::Button::new("✓ CONFIRM ALL")).clicked();
            if ui.add_sized([170.0, 34.0], egui::Button::new("📂 OPEN SCRIPT")).clicked() {
                if let Err(e) = open_indexed_file(&root, &source) {
                    self.status = format!("Cannot open script: {e}");
                }
            }
        });

        if confirm {
            if let Some(data) = self.wizard_data.as_ref() {
                let errors = data.validate();
                if !errors.is_empty() {
                    self.classification_errors = errors.into_iter()
                        .map(|(id, msg)| (id.to_string(), msg)).collect();
                    self.status = format!("{} invalid selections", self.classification_errors.len());
                } else {
                    self.classification_errors.clear();
                    let result = self.db.as_mut()
                        .ok_or_else(|| anyhow::anyhow!("No writable index"))
                        .and_then(|db| db.save_wizard(data));
                    match result {
                        Ok(()) => {
                            self.status = "Saved connection decisions and global file roles".into();
                            self.wizard_mode = false;
                            self.wizard_data = None;
                            self.active_dependency = None;
                            self.refresh();
                        }
                        Err(e) => self.status = format!("Cannot save wizard: {e:#}"),
                    }
                }
            }
        }
        ui.label(&self.status);
    }
}

/// Render only in-memory review data; bulk selection lives in egui's temporary UI state.
fn show_wizard_data(
    ui: &mut egui::Ui,
    data: &mut WizardInternals,
    errors: &std::collections::HashMap<String, String>,
) -> Option<String> {
    // One visible row per physical artifact. Keep every occurrence in WizardInternals
    // so saving still updates every file_matches record, including hidden repeats.
    let mut seen = HashSet::new();
    let visible: Vec<(usize, usize, i64, usize)> = data.groups.iter().enumerate()
        .flat_map(|(gi, group)| group.matches.iter().enumerate().map(move |(mi, m)| (gi, mi, m.artifact_id)))
        .filter(|(_, _, artifact_id)| seen.insert(*artifact_id))
        .map(|(gi, mi, artifact_id)| {
            let occurrences = data.groups.iter().flat_map(|g| &g.matches)
                .filter(|m| m.artifact_id == artifact_id).count();
            (gi, mi, artifact_id, occurrences)
        }).collect();
    let state_id = egui::Id::new(("wizard_bulk", data.source_id));
    let mut selected: HashSet<i64> = ui.ctx().data_mut(|d| d.get_temp(state_id).unwrap_or_default());
    selected.retain(|id| visible.iter().any(|(_, _, artifact_id, _)| artifact_id == id));
    let role_id = egui::Id::new(("wizard_bulk_role", data.source_id));
    let mut bulk_role: String = ui.ctx().data_mut(|d| d.get_temp(role_id).unwrap_or_else(|| "Unknown".to_owned()));
    let total: usize = data.groups.iter().map(|g| g.matches.len()).sum();
    ui.label(format!("{} unique files · {total} recorded occurrences", visible.len()));
    ui.separator();

    ui.group(|ui| {
        ui.strong("Bulk classification");
        ui.horizontal(|ui| {
            if ui.button("Select all").clicked() {
                selected = visible.iter().map(|(_, _, id, _)| *id).collect();
            }
            if ui.button("Invert selection").clicked() {
                for (_, _, id, _) in &visible {
                    if !selected.insert(*id) { selected.remove(id); }
                }
            }
            if ui.button("Clear selection").clicked() { selected.clear(); }
            ui.label(format!("{} selected", selected.len()));
        });
        ui.horizontal(|ui| {
            ui.label("Set selected files/folders to:");
            egui::ComboBox::from_id_salt(("wizard_bulk_role", data.source_id))
                .selected_text(&bulk_role)
                .show_ui(ui, |ui| {
                    for role in ArtifactRole::dropdown() { ui.selectable_value(&mut bulk_role, role.label().to_owned(), role.label()); }
                });
            if ui.button("Apply to selected").clicked() {
                for item in data.groups.iter_mut().flat_map(|g| &mut g.matches) {
                    if selected.contains(&item.artifact_id) { set_classification(item, &bulk_role); }
                }
            }
        });
    });
    ui.add_space(6.0);

    // Collect edits first, then propagate them to *all* occurrences of each file.
    // Never delete or merge persisted provenance evidence.
    let mut changes: Vec<(i64, String)> = Vec::new();
    let mut open_entry: Option<String> = None;
    egui::ScrollArea::vertical().max_height((ui.available_height()-180.0).max(120.0)).show(ui, |ui| {
        for (title, reads) in [("Files read by the script:", true), ("Files created by the script:", false)] {
            ui.heading(title);
            for &(gi, mi, artifact_id, occurrences) in &visible {
                let group = &data.groups[gi];
                if (group.direction == "reads") != reads { continue; }
                let item = &group.matches[mi];
                ui.horizontal(|ui| {
                    let mut checked = selected.contains(&artifact_id);
                    if ui.checkbox(&mut checked, "").changed() {
                        if checked { selected.insert(artifact_id); } else { selected.remove(&artifact_id); }
                    }
                    let short = item.artifact.rsplitn(3, '/').take(2).collect::<Vec<_>>();
                    let short = short.into_iter().rev().collect::<Vec<_>>().join("/");
                    if ui.selectable_label(false, short)
                        .on_hover_text(format!("{}\nDouble-click to open", item.artifact))
                        .double_clicked() {
                        open_entry = Some(item.artifact.clone());
                    }
                    if occurrences > 1 {
                        ui.small(format!("{occurrences} occurrences"))
                            .on_hover_text("All notebook/script occurrences are preserved in the database.");
                    }
                    let mut classification = current_classification(item).to_owned();
                    egui::ComboBox::from_id_salt(("classification", data.source_id, artifact_id))
                        .selected_text(&classification)
                        .show_ui(ui, |ui| {
                            for role in ArtifactRole::dropdown() {
                                ui.selectable_value(&mut classification, role.label().to_owned(), role.label());
                            }
                        });
                    if classification != current_classification(item) {
                        changes.push((artifact_id, classification));
                    }
                });
                // Hidden occurrences may have different match IDs; surface any validation error.
                for match_item in data.groups.iter().flat_map(|g| &g.matches)
                    .filter(|m| m.artifact_id == artifact_id) {
                    if let Some(error) = errors.get(&match_item.id.to_string()) {
                        ui.colored_label(egui::Color32::RED, error);
                    }
                }
            }
            ui.separator();
        }
    });
    for (artifact_id, role) in changes {
        for item in data.groups.iter_mut().flat_map(|g| &mut g.matches) {
            if item.artifact_id == artifact_id { set_classification(item, &role); }
        }
    }
    ui.ctx().data_mut(|d| {
        d.insert_temp(state_id, selected);
        d.insert_temp(role_id, bulk_role);
    });
    open_entry
}

fn current_classification(item: &folder_history::core::history_db::WizardMatch) -> &str {
    match item.decision_id {
        2 => "Not relevant",
        0 => "Unknown",
        _ => item.role.as_deref().unwrap_or("Unknown"),
    }
}

fn set_classification(item: &mut folder_history::core::history_db::WizardMatch, role: &str) {
    match role {
        "Unknown" => { item.decision_id = MatchDecision::NeedsWork as i64; item.role = None; }
        "Not relevant" => { item.decision_id = MatchDecision::Rejected as i64; item.role = None; }
        _ => { item.decision_id = MatchDecision::Accepted as i64; item.role = Some(role.to_owned()); }
    }
}
