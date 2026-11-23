//! Search the existing indexed artifacts, provenance and scan steps. No rescanning.
use super::{Artifact, Relation};
use super::provenance::FilePath;
use rusqlite::Connection;
use std::collections::HashMap;

/// The recorded command text is the existing searchable source for Calls.
/// No claim is made for calls that the scanner did not record as steps.
pub fn load_steps(db: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = db.prepare("SELECT source, command FROM steps")?;
    statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

fn matches_pattern(pattern: &str, value: &str) -> bool {
    if pattern.contains('*') {
        let pattern = pattern.to_lowercase();
        let value = value.to_lowercase();
        let symbolic = FilePath::from_search_pattern(&pattern);
        symbolic.matches_filename(&value) && !pattern.contains('/')
            || symbolic.matches_search_path(&value)
    } else {
        value.to_lowercase().contains(&pattern.to_lowercase())
    }
}
fn script(a: &Artifact) -> bool {
    matches!(a.kind.as_str(), "script" | "notebook") ||
    [".py", ".ipynb", ".r", ".sh", ".bash", ".rmd"].iter().any(|e| a.path.to_ascii_lowercase().ends_with(e))
}

/// Results are indexed files (scripts for Loads/Saves/Calls), with the matching evidence.
pub fn find(artifacts: &[Artifact], relations: &[Relation], assignment: &HashMap<String,String>,
    group: &str, mode: &str, pattern: &str, show_all: bool, steps: &[(String,String)], limit: usize,
) -> Vec<(i64,String,String,String)> {
    let mut result = Vec::new();
    for a in artifacts {
        if !group.is_empty() && assignment.get(&a.path).map(String::as_str) != Some(group) { continue; }
        if !show_all && pattern.trim().is_empty() { continue; }
        let reason = if show_all { Some(String::new()) } else { match mode {
            "Filename" => matches_pattern(pattern, &a.path).then(String::new),
            "Script type" => (script(a) && matches_pattern(pattern, &a.path)).then(String::new),
            "Loads" | "Saves" if script(a) => relations.iter().find(|r| {
                r.source == a.path &&
                (if mode == "Loads" { r.kind == "reads" } else { matches!(r.kind.as_str(), "writes" | "writes_directory") }) &&
                matches_pattern(pattern, &r.target)
            }).map(|r| format!("— {} → {}", r.kind, r.target)),
            "Calls" if script(a) => steps.iter().find(|(source, command)| {
                source == &a.path && matches_pattern(pattern, command)
            }).map(|(_, command)| format!("— CALLS → {}", command.chars().take(100).collect::<String>())),
            _ => None,
        }};
        if let Some(reason) = reason { result.push((a.id,a.path.clone(),a.kind.clone(),reason)); }
        if result.len() >= limit { break; }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wildcard_and_search_modes() {
        assert!(matches_pattern("sampleA*.csv", "sampleA_stats.csv"));
        assert!(!matches_pattern("sampleA*.csv", "sampleB_stats.csv"));
        // Shared matcher: wildcard never crosses directory boundaries.
        assert!(!matches_pattern("A/*.csv", "A/sub/sampleA_stats.csv"));
        assert!(matches_pattern("A/*.csv", "A/sampleA_stats.csv"));
        let files = vec![Artifact { id: 1, path: "A/run.ipynb".into(), kind: "notebook".into() },
                         Artifact { id: 2, path: "B/run.ipynb".into(), kind: "notebook".into() }];
        let edges = vec![Relation { id: 1, source: "A/run.ipynb".into(), target: "A/sampleA_stats.csv".into(),
            kind: "writes".into(), evidence: String::new(), manual: false }];
        let groups = HashMap::from([("A/run.ipynb".into(), "A".into()), ("B/run.ipynb".into(), "B".into())]);
        let hits = find(&files, &edges, &groups, "A", "Saves", "sampleA*.csv", false, &[], 500);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "A/run.ipynb");
        assert!(find(&files, &edges, &groups, "B", "Saves", "*.csv", false, &[], 500).is_empty());
        let steps = [("A/run.ipynb".into(), "write_stats_tables(adata)".into())];
        assert_eq!(find(&files, &edges, &groups, "A", "Calls", "write_stats_tables", false, &steps, 500).len(), 1);
    }
}
