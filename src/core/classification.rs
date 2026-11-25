//! Classification from existing inventory and provenance evidence; no new matcher.
use super::{ArtifactRole, provenance};
use anyhow::Result;
use rusqlite::{params, Connection};
use std::{collections::{HashMap, HashSet}, path::Path};

// Case-insensitive filename glob with * and ?; '/' is never matched by a wildcard.
fn matches_pattern(pattern: &str, path: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let path = path.to_ascii_lowercase();
    let p = pattern.as_bytes();
    let t = path.rsplit('/').next().unwrap_or(&path);
    let t = t.as_bytes();
    let (mut i, mut j) = (0, 0);
    let (mut star, mut backtrack) = (None, 0);
    while j < t.len() {
        if i < p.len() && (p[i] == t[j] || p[i] == b'?') { i+=1; j+=1; }
        else if i < p.len() && p[i] == b'*' { star=Some(i); i+=1; backtrack=j; }
        else if let Some(s) = star { backtrack+=1; j=backtrack; i=s+1; }
        else { return false; }
    }
    while i < p.len() && p[i] == b'*' { i+=1; }
    i == p.len()
}

/// Apply concrete inferences while preserving confirmed concrete roles.
pub(crate) fn refresh(db: &Connection, root: &Path, paths: &HashSet<String>) -> Result<()> {
    let (patterns, groups, operations) = provenance::classification_rules(root)?;
    let mut inferred = HashMap::<String, (ArtifactRole, String)>::new();
    for path in paths {
        for (pattern, role) in &patterns {
            if *role != ArtifactRole::Unknown && matches_pattern(pattern, path) {
                inferred.insert(path.clone(), (*role, format!("file_types:{pattern}")));
            }
        }
    }
    // Group rules apply only when every required member is present in the same folder.
    for path in paths {
        for (required, group_role, member_role) in &groups {
            if required.is_empty() || !required.iter().all(|name| paths.contains(&format!("{path}/{name}"))) { continue; }
            if *group_role != ArtifactRole::Unknown {
                inferred.insert(path.clone(), (*group_role, "file_groups".into()));
            }
            if let Some(role) = member_role.filter(|role| *role != ArtifactRole::Unknown) {
                for name in required {
                    inferred.insert(format!("{path}/{name}"), (role, "file_groups:member".into()));
                }
            }
        }
    }
    // Classify only outputs with an actual recorded provenance match. YAML alone
    // must not assert that a script was executed or that an output was produced.
    let mut stmt = db.prepare("SELECT a.path, m.direction, m.evidence FROM file_matches m JOIN artifacts a ON a.id=m.artifact_id WHERE m.direction IN ('writes','writes_directory')")?;
    let matches = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (path, direction, evidence) in matches {
        for (name, arg, which, role, descendants) in &operations {
            if *role == ArtifactRole::Unknown { continue; }
            let marker = format!("via {name} [");
            let exact = format!("via {name} [{arg}]");
            if !evidence.contains(&exact) && !(arg.is_empty() && evidence.contains(&marker)) { continue; }
            if *descendants {
                if direction != "writes_directory" { continue; }
                for child in paths.iter().filter(|p| p.starts_with(&format!("{path}/"))) {
                    inferred.insert(child.clone(), (*role, format!("operation:{name}:{which}")));
                }
            } else {
                inferred.insert(path.clone(), (*role, format!("operation:{name}:{which}")));
            }
        }
    }
    // Missing or unknown evidence must not erase an existing concrete role.
    for (path, (role, source)) in inferred {
        db.execute("INSERT INTO artifact_roles(path,role,source,confirmed) VALUES (?1,?2,?3,0) ON CONFLICT(path) DO UPDATE SET role=excluded.role,source=excluded.source,confirmed=0 WHERE artifact_roles.confirmed=0 OR artifact_roles.role=?4",
            params![path,role.label(),source,ArtifactRole::Unknown.label()])?;
    }
    Ok(())
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn glob_filename_only() {
        assert!(matches_pattern("*.FASTQ.GZ", "a/b/c.fastq.gz"));
        assert!(!matches_pattern("*.fastq.gz", "a/b/c.h5ad"));
    }
    #[test] fn confirmed_wins_and_yaml_rescan_updates() {
        let dir = std::env::temp_dir().join(format!("folder_history_classification_{}_{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(dir.join(".folder_history")).unwrap();
        let config = dir.join(".folder_history/file_types.yaml");
        std::fs::write(&config, "file_types:\n  - {pattern: '*.testrole', file_type: RawData}\n").unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE artifact_roles(path TEXT PRIMARY KEY,role TEXT NOT NULL,source TEXT NOT NULL,confirmed INTEGER NOT NULL);
            CREATE TABLE artifacts(id INTEGER PRIMARY KEY,path TEXT,kind TEXT);
            CREATE TABLE file_matches(artifact_id INTEGER,direction TEXT,evidence TEXT);").unwrap();
        let paths = HashSet::from(["a.testrole".to_owned(), "b.testrole".to_owned()]);
        refresh(&db, &dir, &paths).unwrap();
        assert_eq!(db.query_row("SELECT role FROM artifact_roles WHERE path='a.testrole'", [], |r| r.get::<_,String>(0)).unwrap(), "Raw experimental data");
        db.execute("UPDATE artifact_roles SET role='Reference data',confirmed=1 WHERE path='a.testrole'", []).unwrap();
        db.execute("UPDATE artifact_roles SET role='Unknown',confirmed=1 WHERE path='b.testrole'", []).unwrap();
        std::fs::write(&config, "file_types:\n  - {pattern: '*.testrole', file_type: StatisticalResults}\n").unwrap();
        refresh(&db, &dir, &paths).unwrap();
        assert_eq!(db.query_row("SELECT role FROM artifact_roles WHERE path='a.testrole'", [], |r| r.get::<_,String>(0)).unwrap(), "Reference data");
        assert_eq!(db.query_row("SELECT role FROM artifact_roles WHERE path='b.testrole'", [], |r| r.get::<_,String>(0)).unwrap(), "Statistical results");
        std::fs::write(&config, "file_types:\n  - {pattern: '*.testrole', file_type: Unknown}\n").unwrap();
        refresh(&db, &dir, &paths).unwrap();
        assert_eq!(db.query_row("SELECT role FROM artifact_roles WHERE path='b.testrole'", [], |r| r.get::<_,String>(0)).unwrap(), "Statistical results");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
