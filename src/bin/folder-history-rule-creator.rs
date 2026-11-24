//! Interactive authoring of rules shared by the FolderHistory GUI and CLI.
use anyhow::{Context, Result, bail};
use folder_history::core::provenance::preview_operation_rule;
use serde_yaml::{Mapping, Value};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

fn prompt(label: &str) -> Result<String> {
    print!("{label} ");
    io::stdout().flush()?;
    let mut value = String::new();
    if io::stdin().read_line(&mut value)? == 0 {
        bail!("Input ended before saving");
    }
    Ok(value.trim().to_owned())
}

fn choice(label: &str, choices: &[&str]) -> Result<String> {
    loop {
        let value = prompt(label)?;
        if choices.contains(&value.as_str()) { return Ok(value); }
        println!("Choose {}.", choices.join(", "));
    }
}

fn key(value: &str) -> Value { Value::String(value.to_owned()) }

fn save_rule(path: &Path, rule: Value) -> Result<()> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("Reading {}", path.display())),
    };
    let mut document: Mapping = existing.as_deref()
        .filter(|text| !text.trim().is_empty())
        .map(serde_yaml::from_str).transpose()
        .context("Reading existing YAML")?.unwrap_or_default();
    let operations = document.entry(key("operations"))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut().context("Existing operations must be a YAML list")?;
    if let Some(index) = operations.iter().position(|old| {
        ["name", "language", "subcommand"].iter().all(|field| old.get(*field) == rule.get(*field))
    }) {
        if operations[index] == rule {
            println!("This rule is already saved.");
            return Ok(());
        }
        let old = serde_yaml::to_string(&operations[index])?;
        let new = serde_yaml::to_string(&rule)?;
        // Keep matching lines in order so repeated YAML keys in different
        // argument blocks are not accidentally treated as interchangeable.
        let old: Vec<_> = old.lines().collect();
        let new: Vec<_> = new.lines().collect();
        let mut common = vec![vec![0; new.len() + 1]; old.len() + 1];
        for i in (0..old.len()).rev() {
            for j in (0..new.len()).rev() {
                common[i][j] = if old[i] == new[j] { 1 + common[i + 1][j + 1] }
                    else { common[i + 1][j].max(common[i][j + 1]) };
            }
        }
        println!("Changes to the existing rule (- current, + proposed):");
        let (mut i, mut j) = (0, 0);
        while i < old.len() || j < new.len() {
            if i < old.len() && j < new.len() && old[i] == new[j] {
                i += 1;
                j += 1;
            } else if i < old.len() && (j == new.len() || common[i + 1][j] >= common[i][j + 1]) {
                println!("- {}", old[i]);
                i += 1;
            } else {
                println!("+ {}", new[j]);
                j += 1;
            }
        }
        if !prompt("Replace the existing rule? [y/N]")?.eq_ignore_ascii_case("y") {
            return Ok(());
        }
        operations[index] = rule;
    } else {
        let answer = prompt("Save to your shared rules? [Y/n]")?;
        if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") { return Ok(()); }
        operations.push(rule);
    }

    let yaml = serde_yaml::to_string(&document)?;
    let parent = path.parent().context("Missing configuration directory")?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".operations.yaml.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
    let result = file.write_all(yaml.as_bytes()).and_then(|_| file.sync_all());
    drop(file);
    let result = result.and_then(|_| std::fs::rename(&temp, path));
    if result.is_err() { let _ = std::fs::remove_file(&temp); }
    result.with_context(|| format!("Saving {}", path.display()))?;
    println!("Saved to {}. Rescan the archive in FolderHistory to use it.", path.display());
    Ok(())
}

fn run() -> Result<()> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        .context("Cannot locate your home directory")?;
    let path = PathBuf::from(home).join(".folder_history/operations.yaml");
    println!("FolderHistory rule creator");
    println!("Rules are saved in {} and loaded by both the GUI and CLI.", path.display());
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        println!("Run without arguments to create and preview a rule interactively.");
        return Ok(());
    }

    let language = choice("Language [python/r/bash]:", &["python", "r", "bash"])?;
    let name = prompt("Function or command name (e.g. write_pseudobulk_stats_tables):")?;
    if name.is_empty() { bail!("A function or command name is required"); }
    let mut operation = Mapping::new();
    operation.insert(key("name"), key(&name));
    operation.insert(key("language"), key(&language));
    if language == "bash" {
        let subcommand = prompt("Subcommand (e.g. run, or Enter for none):")?;
        if !subcommand.is_empty() { operation.insert(key("subcommand"), key(&subcommand)); }
    }
    let mut arguments = Vec::new();
    println!("Describe only arguments that point to files or folders. Skip objects such as adata.");
    loop {
        let selector = prompt("Argument number (starting at 1), keyword, or flag (Enter when done):")?;
        if selector.is_empty() { break; }
        let mut argument = Mapping::new();
        if let Ok(position) = selector.parse::<u64>() {
            if position == 0 { bail!("Argument positions start at 1"); }
            argument.insert(key("position"), serde_yaml::to_value(position)?);
        } else {
            argument.insert(key("name"), key(&selector));
        }
        let role = choice("Read or write [input/output]:", &["input", "output"])?;
        let kind = choice("Path type [file/directory]:", &["file", "directory"])?;
        argument.insert(key("role"), key(&role));
        argument.insert(key("kind"), key(&kind));
        let default = prompt("Literal fallback path (Enter for none):")?;
        if !default.is_empty() { argument.insert(key("default"), key(&default)); }
        if role == "output" && kind == "directory" {
            println!("A directory output includes every indexed file beneath it by default.");
            let patterns = prompt("Limit to filenames or patterns, comma separated (Enter for all):")?;
            let patterns: Vec<_> = patterns.split(',').map(str::trim)
                .filter(|pattern| !pattern.is_empty()).map(key).collect();
            if !patterns.is_empty() { argument.insert(key("expected_files"), Value::Sequence(patterns)); }
        }
        arguments.push(Value::Mapping(argument));
    }
    if arguments.is_empty() { bail!("Add at least one input or output argument"); }
    operation.insert(key("arguments"), Value::Sequence(arguments));
    let rule = Value::Mapping(operation);
    let mut document = Mapping::new();
    document.insert(key("operations"), Value::Sequence(vec![rule.clone()]));
    let yaml = serde_yaml::to_string(&document)?;
    println!("\nRule preview\n{yaml}");
    let archive = prompt("Archive folder to try it on (Enter to skip preview):")?;
    if !archive.is_empty() {
        let root = PathBuf::from(archive).canonicalize().context("Opening archive folder")?;
        if !root.is_dir() { bail!("The archive path must be a directory"); }
        let script = prompt("Script path relative to the archive (Enter for script at archive root):")?;
        let script = if script.is_empty() { "preview" } else { script.as_str() };
        if Path::new(script).components().any(|part| !matches!(part, Component::Normal(_) | Component::CurDir)) {
            bail!("Enter a script path relative to the archive");
        }
        let source = prompt("Paste the call to test on one line:")?;
        let results = preview_operation_rule(&yaml, &source, &root, &script.replace('\\', "/"))?;
        println!("\n{} indexed paths matched by this rule", results.len());
        for (direction, path) in results { println!("  {direction:16} {path}"); }
    }
    save_rule(&path, rule)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
