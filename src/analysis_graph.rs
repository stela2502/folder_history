//! Analysis groups anchored in original dataset/project identity.
//! Storage wrappers never become analysis names or connectivity evidence.
//! Existing provenance edges are used to attach otherwise unclassified files;
//! references and reusable scripts do not merge unrelated projects.
use super::{Artifact, Relation};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug)]
pub(crate) struct AnalysisGroup {
    pub key: String,
    pub suggested_name: String,
    pub source_hint: String,
    pub paths: Vec<String>,
    pub edge_count: usize,
}

fn reference(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".fa", ".fasta", ".fna", ".fa.gz", ".fasta.gz", ".gtf", ".gtf.gz",
     ".gff", ".gff3", ".mmi", ".fai", ".dict", ".2bit"]
        .iter().any(|suffix| p.ends_with(suffix))
        || p.split('/').any(|part| matches!(part, "references" | "genomes" | "star_index"))
}

fn storage(part: &str) -> bool {
    matches!(part.to_ascii_lowercase().as_str(),
        "server" | "local" | "shared" | "desktop" | "stefansdesktop" | "shamit" |
        "nas" | "home" | "sens05_home" | "sens05_shared")
}
fn generic(part: &str) -> bool {
    matches!(part.to_ascii_lowercase().as_str(),
        "analysis" | "sc_analysis" | "results" | "result" | "data" | "raw" |
        "figures" | "plots" | "output" | "outputs" | "scripts" | "notebooks" |
        "pipeline" | "processed" | "counts" | "qc" | "fastq" | "fastqs")
}
fn is_raw(a: &Artifact) -> bool {
    let p = a.path.to_ascii_lowercase();
    a.kind == "fastq" || [".fastq", ".fastq.gz", ".fq", ".fq.gz", ".bcl", ".bcl.gz"]
        .iter().any(|s| p.ends_with(s))
}
fn is_script(a: &Artifact) -> bool { matches!(a.kind.as_str(), "script" | "notebook") }

// The first informative folder *below* storage wrappers is a provisional
// project/sample identity. This is a label, not a claimed biological sample ID.
fn project(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let parts: Vec<_> = normalized.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.len() < 2 { return None; }
    for part in &parts[..parts.len()-1] {
        if !storage(part) && !generic(part) && !part.starts_with('.') {
            return Some((*part).to_owned());
        }
    }
    None
}

pub(crate) fn detect(artifacts: &[Artifact], relations: &[Relation]) -> Vec<AnalysisGroup> {
    let ids: HashMap<&str, usize> = artifacts.iter().enumerate()
        .map(|(i, a)| (a.path.as_str(), i)).collect();
    let mut project_names: Vec<Option<String>> = artifacts.iter().map(|a| {
        if reference(&a.path) { None } else { project(&a.path) }
    }).collect();

    // Dataset roots take priority. A file without a usable project name can
    // inherit a project from its *recorded* relationship to a dataset.
    // Never use references to propagate identity, and never overwrite an
    // explicit project identity: shared scripts may serve many projects.
    let mut candidates: Vec<BTreeSet<String>> = vec![BTreeSet::new(); artifacts.len()];
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); artifacts.len()];
    let mut edge_pairs = HashSet::<(usize, usize)>::new();
    for r in relations {
        if matches!(r.kind.as_str(), "candidate" | "inferred") { continue; }
        let (Some(&a), Some(&b)) = (ids.get(r.source.as_str()), ids.get(r.target.as_str())) else { continue; };
        if a == b || reference(&artifacts[a].path) || reference(&artifacts[b].path) { continue; }
        // Links to shared scripts must not merge projects.
        neighbors[a].push(b);
        neighbors[b].push(a);
        edge_pairs.insert((a.min(b), a.max(b)));
    }
    for (i, a) in artifacts.iter().enumerate() {
        if is_raw(a) {
            if let Some(name) = &project_names[i] { candidates[i].insert(name.clone()); }
        }
    }
    // Propagate raw-root project labels through known provenance links, but
    // stop at explicitly different project boundaries and reusable scripts.
    for _ in 0..artifacts.len().min(64) {
        let mut changed = false;
        for i in 0..artifacts.len() {
            if reference(&artifacts[i].path) { continue; }
            let own = project_names[i].as_deref();
            for &j in &neighbors[i] {
                if let (Some(a), Some(b)) = (own, project_names[j].as_deref()) {
                    if !a.eq_ignore_ascii_case(b) { continue; }
                }
                let inherited: Vec<_> = candidates[j].iter()
                    .filter(|name| own.is_none_or(|p| p.eq_ignore_ascii_case(name)))
                    .cloned().collect();
                for name in inherited { changed |= candidates[i].insert(name); }
            }
        }
        if !changed { break; }
    }
    for (i, a) in artifacts.iter().enumerate() {
        if project_names[i].is_none() && !reference(&a.path) && !is_script(a) && candidates[i].len() == 1 {
            project_names[i] = candidates[i].iter().next().cloned();
        }
    }

    let mut grouped = BTreeMap::<String, Vec<usize>>::new();
    for (i, a) in artifacts.iter().enumerate() {
        let key = if reference(&a.path) { "references".to_owned() }
            else if let Some(p) = &project_names[i] { format!("project:{}", p.to_ascii_lowercase()) }
            else { "unassigned".to_owned() };
        grouped.entry(key).or_default().push(i);
    }
    let mut output = Vec::new();
    for (key, members) in grouped {
        let paths: Vec<String> = members.iter().map(|&i| artifacts[i].path.clone()).collect();
        let source_hint = if key == "references" { "Shared references".to_owned() }
            else if key == "unassigned" { "Unassigned files".to_owned() }
            else { project_names[members[0]].clone().unwrap_or_else(|| "Unassigned".into()) };
        let set: HashSet<usize> = members.iter().copied().collect();
        let edge_count = edge_pairs.iter().filter(|(a,b)| set.contains(a) && set.contains(b)).count();
        output.push(AnalysisGroup {
            key: format!("analysis:{key}"), suggested_name: source_hint.clone(),
            source_hint, paths, edge_count,
        });
    }
    output.sort_by(|a,b| {
        let a_last = a.key.ends_with("references") || a.key.ends_with("unassigned");
        let b_last = b.key.ends_with("references") || b.key.ends_with("unassigned");
        a_last.cmp(&b_last).then_with(|| b.paths.len().cmp(&a.paths.len()))
            .then_with(|| a.key.cmp(&b.key))
    });
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    fn a(path: &str, kind: &str) -> Artifact {
        Artifact { id: 0, path: path.into(), kind: kind.into() }
    }
    fn r(source: &str, target: &str) -> Relation {
        Relation { id: 0, source: source.into(), target: target.into(), kind: "writes".into(), evidence: String::new(), manual: false }
    }
    #[test]
    fn storage_wrappers_are_not_analyses() {
        let files = vec![a("Server/cglu00455/raw/A.fastq.gz", "fastq"),
            a("Local/cglu00455/analysis.h5ad", "h5ad"),
            a("StefansDesktop/cglu00455/figure.png", "figure")];
        let groups = detect(&files, &[]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].suggested_name, "cglu00455");
        assert_eq!(groups[0].paths.len(), 3);
    }
    #[test]
    fn separate_raw_projects_remain_separate_despite_shared_script() {
        let files = vec![a("Server/A/raw/a.fastq.gz", "fastq"),
            a("Server/B/raw/b.fastq.gz", "fastq"), a("Local/tools/map.py", "script")];
        let links = vec![r("Local/tools/map.py", "Server/A/raw/a.fastq.gz"),
                         r("Local/tools/map.py", "Server/B/raw/b.fastq.gz")];
        let groups = detect(&files, &links);
        assert!(groups.iter().any(|g| g.suggested_name == "A"));
        assert!(groups.iter().any(|g| g.suggested_name == "B"));
    }
    #[test]
    fn reference_does_not_join_projects() {
        let files = vec![a("Server/A/a.fastq.gz", "fastq"),
            a("Server/B/b.fastq.gz", "fastq"), a("references/GRCh38.fa", "file")];
        let groups = detect(&files, &[]);
        assert_eq!(groups.len(), 3);
    }
}
