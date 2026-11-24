//! Provenance analysis: symbolic paths, source interpretation, and inventory evidence.
//! Keep the external parse API stable while isolating the analysis responsibilities.
use super::*;

mod rules;
mod resolver;
mod variable;
mod expressions;
mod logic;
mod operations;
mod analyzer;
mod cli_rules;
mod notebook;
#[cfg(test)] mod tests;

pub use rules::configure_cli_rule_files;
use rules::*;
use resolver::*;
use variable::{Environment, FileMatches, PathPart, Variable};
use expressions::*;
use logic::*;
use operations::*;
use analyzer::*;
use cli_rules::*;
use notebook::*;

pub(super) use notebook::parse;

// Shared path matching for both provenance extraction and review.
pub use variable::FilePath;

/// Effective, layered classification rules. Later entries override earlier ones.
pub(crate) fn classification_rules(root: &std::path::Path) -> anyhow::Result<(Vec<(String, super::ArtifactRole)>, Vec<(Vec<String>, super::ArtifactRole, Option<super::ArtifactRole>)>, Vec<(String, String, String, super::ArtifactRole, bool)>)> {
    let mut patterns = Vec::new();
    let mut groups = Vec::new();
    let mut operations = Vec::new();
    for layer in rules::rule_layers(root)? {
        for rule in layer.file_types { patterns.push((rule.pattern, rule.file_type)); }
        for rule in layer.file_groups { groups.push((rule.required, rule.file_type, rule.members_file_type)); }
        for operation in layer.operations {
            for arg in operation.arguments {
                if arg.role != "output" { continue; }
                if let Some(role) = arg.file_type {
                    operations.push((operation.name.clone(), arg.name.clone(), "direct".into(), role, false));
                }
                let descendant_role = arg.descendant_file_type.or_else(|| {
                    (arg.kind.as_deref() == Some("directory") && arg.expected_files.is_empty())
                        .then_some(arg.file_type).flatten()
                });
                if let Some(role) = descendant_role {
                    operations.push((operation.name.clone(), arg.name.clone(), "descendant".into(), role, true));
                }
            }
        }
    }
    Ok((patterns, groups, operations))
}

/// Explain shell operation matching without changing the scanner or database.
/// Uses the same rule loader, shell tokenizer, variable expander and FilePath matcher.
pub fn diagnose_shell_rules(root: &std::path::Path, script: &std::path::Path, rel: &str,
    inventory: &std::collections::HashSet<String>) -> anyhow::Result<Vec<String>> {
    let rules = rules::load_operation_rules(root)?;
    let source = std::fs::read_to_string(script)?;
    let mut env = Environment::default();
    let mut output = Vec::new();
    let mut logical = Vec::<(usize, String)>::new();
    let mut continued: Option<(usize, String)> = None;
    for (idx, raw) in source.lines().enumerate() {
        let trimmed = raw.trim_end();
        if let Some(part) = trimmed.strip_suffix('\\') {
            let (line, mut text) = continued.take().unwrap_or((idx + 1, String::new()));
            text.push_str(part); text.push(' '); continued = Some((line, text));
        } else if let Some((line, mut text)) = continued.take() {
            text.push_str(trimmed); logical.push((line, text));
        } else { logical.push((idx + 1, raw.to_owned())); }
    }
    if let Some(last) = continued { logical.push(last); }
    output.push(format!("script directory: {}", script.parent().unwrap_or(root).display()));
    output.push(format!("inventory root: {}", root.display()));
    output.push("working directory: UNKNOWN at runtime; scanner uses source-relative matching".into());
    let mut dirs = inventory.clone();
    for path in inventory {
        let mut end = path.rfind('/');
        while let Some(i) = end { dirs.insert(path[..i].to_owned()); end = path[..i].rfind('/'); }
    }
    output.push(format!("Loaded {} operation rules; {} indexed files, {} paths including parent directories", rules.len(), inventory.len(), dirs.len()));
    for (line, raw) in logical {
        let raw = raw.trim();
        if raw.starts_with('#') || raw.is_empty() { continue; }
        if let Some((name, value)) = raw.split_once('=') {
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                let resolved = analyzer::shell_expand(value, &env);
                output.push(format!("line {line}: variable {name} = {resolved:?}"));
                env.variables.insert(name.to_owned(), resolved);
                continue;
            }
        }
        let (segments, _) = cli_rules::shell_pipeline(raw);
        for tokens in segments {
            let Some(program) = tokens.first() else { continue };
            let basename = program.rsplit('/').next().unwrap_or(program);
            let candidates: Vec<_> = rules.iter().filter(|r| r.language == "bash" && r.name == basename).collect();
            if candidates.is_empty() {
                if tokens.iter().any(|t| t == "run" || t == "--out_dir" || t == "--fastq") {
                    output.push(format!("line {line}: command {basename:?} -> NO YAML RULE ({} loaded)", rules.len()));
                }
                continue;
            }
            for rule in candidates {
                if let Some(sub) = &rule.subcommand {
                    if tokens.get(1) != Some(sub) {
                        output.push(format!("line {line}: {} -> subcommand mismatch: expected {sub:?}, got {:?}", rule.name, tokens.get(1)));
                        continue;
                    }
                }
                output.push(format!("line {line}: {} -> RULE MATCHED", rule.name));
                for spec in &rule.arguments {
                    let value = tokens.iter().enumerate().find_map(|(i, token)| {
                        if token == &spec.name { tokens.get(i+1).cloned() }
                        else { token.strip_prefix(&format!("{}=", spec.name)).map(str::to_owned) }
                    });
                    let Some(raw_value) = value else {
                        output.push(format!("  {} -> FLAG NOT FOUND", spec.name)); continue;
                    };
                    let expanded = if raw_value.contains('$') { analyzer::shell_expand(&raw_value, &env) }
                        else { Variable::Exact(raw_value.clone()) };
                    let path = expanded.clone().into_filepath();
                    let candidates = if spec.kind.as_deref() == Some("directory") { &dirs } else { inventory };
                    let mut trace = Vec::new();
                    let found = path.find_matches_with_trace(rel, candidates, &mut trace);
                    output.push(format!("  {} ({}/{}) raw={raw_value:?} expanded={expanded:?}",
                        spec.name, spec.role, spec.kind.as_deref().unwrap_or("file")));
                    let literal = match &expanded {
                        Variable::Exact(s) => Some(s.as_str()),
                        Variable::Parts(fp) if !fp.contains_wildcards() => fp.parts.first().and_then(|part| {
                            if let PathPart::Literal(s) = part { Some(s.as_str()) } else { None }
                        }),
                        _ => None,
                    };
                    if let Some(literal) = literal {
                        let base = std::path::Path::new(literal);
                        let source_dir = script.parent().unwrap_or(root);
                        let source_candidate = source_dir.join(base);
                        let root_candidate = root.join(base);
                        for (label, candidate) in [("script-parent hypothesis", source_candidate), ("inventory-root hypothesis", root_candidate)] {
                            let exists = candidate.exists();
                            let indexed = candidate.strip_prefix(root).ok().map(|p| {
                                let normalized = p.to_string_lossy().replace('\\', "/");
                                let normalized = normalized.trim_start_matches("./").trim_end_matches('/');
                                candidates.contains(normalized)
                            }).unwrap_or(false);
                            output.push(format!("    {label}: {} | filesystem={} | inventory={}",
                                candidate.display(), if exists {"FOUND"} else {"MISSING"},
                                if indexed {"FOUND"} else {"MISSING"}));
                        }
                    } else {
                        output.push("    physical path probes: skipped (symbolic/wildcard expression)".into());
                    }
                    for entry in trace { output.push(format!("    matcher: {entry}")); }
                    output.push(format!("    result: {found:?}"));
                    if matches!(found, FileMatches::Funky(_)) {
                        output.push("    NOTE: fuzzy results are suggestions, not confirmed provenance edges".into());
                    }
                }
            }
        }
    }
    Ok(output)
}

/// Try one operation against a pasted source fragment and an archive inventory.
pub fn preview_operation_rule(
    yaml: &str, source: &str, root: &std::path::Path, relative_script: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    use std::collections::HashSet;
    use crate::core::engine::{excluded_dir, excluded_file, schema};

    let rules: RuleSet = serde_yaml::from_str(yaml)?;
    let [operation] = rules.operations.as_slice() else {
        anyhow::bail!("Preview requires exactly one operation");
    };
    let db = rusqlite::Connection::open_in_memory()?;
    schema(&db)?;
    let mut paths = HashSet::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|entry| {
        entry.depth() == 0 || !entry.file_type().is_dir()
            || !excluded_dir(&entry.file_name().to_string_lossy(), false)
    }) {
        let entry = entry?;
        if entry.depth() == 0 { continue; }
        let directory = entry.file_type().is_dir();
        if !directory && (!entry.file_type().is_file()
            || excluded_file(&entry.file_name().to_string_lossy())) { continue; }
        let path = entry.path().strip_prefix(root)?.to_string_lossy().replace('\\', "/");
        if directory {
            db.execute("INSERT INTO artifacts(path,kind) VALUES (?1,'directory')", [&path])?;
        }
        paths.insert(path);
    }
    let language = operation.language.as_str();
    if language == "bash" {
        let lines: Vec<_> = source.lines().enumerate()
            .map(|(i, line)| (i + 1, line.to_owned())).collect();
        analyze_lines(&db, relative_script, &lines, &paths, true, &[], &[], &rules.operations)?;
    } else {
        analyze_source(&db, relative_script, source, 1, &paths, &mut Environment::default(),
            &[], &mut std::collections::HashMap::new(), &[], &rules.operations, language)?;
    }
    let marker = format!("{}{} [", operation.name,
        operation.subcommand.as_ref().map(|s| format!(" {s}")).unwrap_or_default());
    let direct = format!(" via {marker}");
    let inherited = format!(" from {marker}");
    let mut stmt = db.prepare("SELECT DISTINCT m.direction, a.path
        FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id
        WHERE instr(m.evidence, ?1)>0 OR instr(m.evidence, ?2)>0
        ORDER BY m.direction, a.path")?;
    let matches = stmt.query_map([direct, inherited], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(matches)
}
