//! Actionable, conservative archive triage. No deletion and no inferred execution.
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::path::Path;
use walkdir::WalkDir;
use super::{Artifact, Relation, Issue, excluded_dir, report_safe};

fn inside(path: &str, dir: &str) -> bool {
    path == dir || path.strip_prefix(dir).is_some_and(|s| s.starts_with('/'))
}
fn script(kind: &str) -> bool { kind == "script" || kind == "notebook" }
fn producer(path: &str, relations: &[Relation]) -> bool {
    relations.iter().any(|r| (r.kind == "writes" || r.kind == "writes_directory")
        && (inside(path, &r.target)))
}

pub fn markdown(root: &Path, artifacts: &[Artifact], relations: &[Relation], issues: &[Issue]) -> String {
    let mut out = String::from("\n## Archive handover: actionable findings\n\n");
    out.push_str("These are review candidates, NOT deletion instructions. Missing producers may be raw inputs, downloads or lost scripts. Only scanned paths are considered.\n");
    let paths: HashSet<&str> = artifacts.iter().map(|a| a.path.as_str()).collect();
    let mut reads: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for r in relations {
        if r.kind == "reads" && paths.contains(r.target.as_str()) {
            reads.entry(&r.target).or_default().push(&r.source);
        }
    }
    out.push_str("\n### Existing inputs with no identified producer\n");
    let mut missing_producer = 0;
    for (path, consumers) in &reads {
        if !producer(path, relations) {
            missing_producer += 1;
            let _ = writeln!(out, "- `{}` — read by {}; identify original source or document lost transformation in a script.", report_safe(path), consumers.iter().map(|p| format!("`{}`", report_safe(p))).collect::<Vec<_>>().join(", "));
        }
    }
    if missing_producer == 0 { out.push_str("- None identified among resolved read relationships.\n"); }
    out.push_str("\n### Referenced inputs not found in inventory\n");
    let mut missing = 0;
    for r in relations.iter().filter(|r| r.kind == "reads" && !paths.contains(r.target.as_str())) {
        // Directory reads are valid even though directories are not artifacts.
        if root.join(&r.target).exists() { continue; }
        missing += 1;
        let _ = writeln!(out, "- `{}` — referenced by `{}` (verify path or recover dependency)", report_safe(&r.target), report_safe(&r.source));
    }
    for i in issues.iter().filter(|i| i.category == "unresolved" || i.category == "external") {
        missing += 1;
        let _ = writeln!(out, "- `{}` — `{}` cell/line {} ({}, verify missing/external dependency)", report_safe(&i.filename), report_safe(&i.notebook), i.cell, i.category);
    }
    if missing == 0 { out.push_str("- None detected. This does not prove all dependencies are present.\n"); }

    // Report maximal subtrees with zero detected relations. The scan inventory is
    // file-only; aggregate descendants to avoid thousands of individual entries.
    let mut dirs: BTreeMap<String, (usize, u64, usize)> = BTreeMap::new();
    for a in artifacts {
        let mut parent = Path::new(&a.path).parent();
        while let Some(dir) = parent {
            if dir.as_os_str().is_empty() { break; }
            let key = dir.to_string_lossy().replace('\\', "/");
            let entry = dirs.entry(key).or_default();
            entry.0 += 1;
            if let Ok(meta) = std::fs::metadata(root.join(&a.path)) { entry.1 = entry.1.saturating_add(meta.len()); }
            if script(&a.kind) { entry.2 += 1; }
            parent = dir.parent();
        }
    }
    let connected: HashSet<&str> = relations.iter().flat_map(|r| [r.source.as_str(), r.target.as_str()]).collect();
    let mut disconnected: Vec<_> = dirs.iter().filter(|(dir, _)| {
        !connected.iter().any(|p| inside(p, dir) || inside(dir, p))
    }).collect();
    disconnected.sort_by(|a,b| a.0.len().cmp(&b.0.len()).then(a.0.cmp(b.0)));
    let mut selected: Vec<&str> = Vec::new();
    out.push_str("\n### Disconnected directory trees (review only)\n");
    for (dir, (count, bytes, scripts)) in disconnected {
        if selected.iter().any(|p| inside(dir, p)) { continue; }
        selected.push(dir);
        let _ = writeln!(out, "- `{}` — {} files, {:.1} MiB, {} scripts/notebooks; no detected provenance edges. {}", report_safe(dir), count, *bytes as f64 / 1048576.0, scripts, if *scripts > 0 { "Potential isolated analysis; preserve until reviewed." } else { "Unclassified; inspect before cleanup." });
    }
    if selected.is_empty() { out.push_str("- None detected in indexed folders.\n"); }

    // Existing scanner excludes common workflow/cache directories entirely.
    // Surface them explicitly rather than accidentally declaring them absent.
    out.push_str("\n### Excluded directory trees (not scanned; inspect separately)\n");
    let mut excluded = 0;
    // Walk all directory entries, pruning immediately when an excluded root is found.
    let mut it = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(result) = it.next() {
        let Ok(entry) = result else { continue };
        if entry.depth() > 0 && entry.file_type().is_dir() && excluded_dir(&entry.file_name().to_string_lossy(), false) {
            excluded += 1;
            if excluded <= 100 {
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    let _ = writeln!(out, "- `{}` — excluded from provenance scan; contents and dependencies unknown.", report_safe(&rel.to_string_lossy()));
                }
            }
            it.skip_current_dir();
        }
    }
    if excluded == 0 { out.push_str("- None detected.\n"); }
    if excluded > 100 { let _ = writeln!(out, "- ... {} more excluded directories", excluded - 100); }
    out.push_str("\n### How to repair a missing connection\n\nAdd a small, human-readable script documenting the known input and output paths, then rescan. The script may be a historical stub; explain uncertainty in comments. FolderHistory parses its references like any other script.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directory_writes_cover_children() {
        let r = Relation { id: 1, source: "analysis.ipynb".into(), target: "stats".into(), kind: "writes".into(), evidence: String::new(), manual: false };
        assert!(producer("stats/a.tsv", &[r.clone()]));
        assert!(!producer("stats2/a.tsv", &[r]));
    }
}
