#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use anyhow::Result;
use eframe::egui;
//use regex::Regex;
use rusqlite::params;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::process::Command;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver}};
use std::path::{Path, PathBuf};
//use walkdir::WalkDir;
use folder_history::core::*;
use folder_history::core::{analysis_graph, report};
mod spectator;
mod gui;
use gui::app::*;
fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 760.0]),
        ..Default::default()
    };

    eframe::run_native(
        "FolderHistory",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(App::default()))
        }),
    )
}

