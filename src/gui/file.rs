//! Shared file-name interaction widget. No application state is borrowed here.
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileAction { Select, Open, OpenParent, CopyPath }

/// The text is the click target; role glyphs belong to the caller and are inert.
/// `Open` is also offered explicitly so opening does not depend on header double-click routing.
pub(crate) fn show(ui: &mut egui::Ui, label: &str, path: &str, selected: bool, color: Option<egui::Color32>) -> Option<FileAction> {
    let text = match color {
        Some(c) => egui::RichText::new(label).color(c),
        None => egui::RichText::new(label),
    };
    let response = ui.selectable_label(selected, text)
        .on_hover_text(format!("{path}\nDouble-click to open"));
    let mut action = if response.double_clicked() { Some(FileAction::Open) }
        else if response.clicked() { Some(FileAction::Select) } else { None };
    response.context_menu(|ui| {
        for (title, value) in [
            ("Open file", FileAction::Open),
            ("Open containing folder", FileAction::OpenParent),
            ("Copy full path", FileAction::CopyPath),
        ] {
            if ui.button(title).clicked() { action = Some(value); ui.close(); }
        }
    });
    action
}
