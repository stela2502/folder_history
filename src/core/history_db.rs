use super::ArtifactRole;
use std::str::FromStr;
/// Database operations for persisted file match evidence and global artifact roles.
/// The scanner determines candidates; this layer only persists and loads them.
use anyhow::Result;
use rusqlite::{params, Connection};
use super::engine::{classified_artifact_path, resolve_reference, cell_text,
    notebook_output_text, extract_shell, plausible_reference};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use regex::Regex;
use rusqlite::OpenFlags;
use anyhow::Context;


const DIRECTORY_CHILD_EVIDENCE: &str = ": inferred under output directory ";

/// A file link inherited from a directory output is reviewed through the folder.
pub fn is_directory_child_link(direction: &str, evidence: &str) -> bool {
    direction == "writes" && evidence.contains(DIRECTORY_CHILD_EVIDENCE)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatchRow {
    pub source_id: i64,
    pub filepath_id: i64,
    pub artifact_id: i64,
    pub location: i64,
    pub direction: String,
    pub pattern: String,
    pub is_multi: bool,
    pub source: String,
    pub artifact: String,
    pub default_role: Option<String>,
}

/// Scanner-facing persistence. Matching belongs to FilePath; this API only stores
/// a discovered physical connection, preserving the original source expression.
pub trait DbProvenance {
    fn record_match(&self, source: &str, location: usize, expression: &str,
        artifact: &str, direction: &str, evidence: &str) -> Result<()>;
}

impl DbProvenance for Connection {
    fn record_match(&self, source: &str, location: usize, expression: &str,
        artifact: &str, direction: &str, evidence: &str) -> Result<()> {
        anyhow::ensure!(matches!(direction, "reads" | "writes" | "writes_directory"),
            "Invalid provenance direction: {direction}");
        // Scanner expressions can already be normalized patterns (e.g. "analyzed_*.h5ad").
        // The legacy issue parser expects source-code syntax and misses these.
        let is_multi = expression.contains('*') || expression.contains('?')
            || crate::core::provenance::FilePath::from_issue_expression(expression).is_some();
        // Standalone provenance parsing is also used by tests and callers without
        // a preceding inventory scan. Only matched physical candidates reach here.
        self.execute("INSERT OR IGNORE INTO artifacts(path,kind) VALUES (?1,'script')",[source])?;
        self.execute("INSERT OR IGNORE INTO artifacts(path,kind) VALUES (?1,'file')",[artifact])?;
        self.execute("INSERT INTO file_paths(pattern,is_multi) VALUES (?1,?2)
            ON CONFLICT(pattern) DO UPDATE SET is_multi=MAX(file_paths.is_multi,excluded.is_multi)",
            params![expression, i64::from(is_multi)])?;
        let changed = self.execute("INSERT INTO file_matches(source_id,filepath_id,artifact_id,location,direction,evidence)
            SELECT s.id,p.id,a.id,?4,?5,?6 FROM artifacts s, file_paths p, artifacts a
            WHERE s.path=?1 AND p.pattern=?2 AND a.path=?3
            ON CONFLICT(source_id,filepath_id,artifact_id,location,direction)
            DO UPDATE SET evidence=excluded.evidence",
            params![source,expression,artifact,location as i64,direction,evidence])?;
        anyhow::ensure!(changed == 1, "Unable to record match {source} -> {artifact}");
        Ok(())
    }
}

/// Archive-facing projections of the current file_matches model.
/// `Relation` and `Issue` are transitional display DTOs, NOT legacy tables.
pub trait DbArchiveView {
    fn archive_view(&self) -> Result<(Vec<crate::core::Artifact>, Vec<crate::core::Relation>, Vec<crate::core::Issue>)>;
    fn analysis_names(&self) -> Result<Vec<(String, String)>>;
    fn source_artifact_id(&self, path: &str) -> Result<i64>;
    fn add_manual_match(&self, source: &str, target: &str, direction: &str, evidence: &str) -> Result<()>;
    fn refresh_cached_candidates(&mut self) -> Result<(i64, i64)>;
}

impl DbArchiveView for Connection {
    fn archive_view(&self) -> Result<(Vec<crate::core::Artifact>, Vec<crate::core::Relation>, Vec<crate::core::Issue>)> {
        use crate::core::{Artifact, Relation, Issue};
        let mut q = self.prepare("SELECT id,path,kind FROM artifacts ORDER BY path")?;
        let artifacts = q.query_map([], |r| Ok(Artifact {id:r.get(0)?,path:r.get(1)?,kind:r.get(2)?}))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut q = self.prepare("SELECT m.id,s.path,a.path,m.direction,m.evidence,m.decision_id \
            FROM file_matches m JOIN artifacts s ON s.id=m.source_id \
            JOIN artifacts a ON a.id=m.artifact_id \
            WHERE m.decision_id != 2 ORDER BY m.id")?;
        let relations = q.query_map([], |r| Ok(Relation {
            id:r.get(0)?,source:r.get(1)?,target:r.get(2)?,kind:r.get(3)?,
            evidence:r.get(4)?,manual:r.get::<_,String>(4)?.starts_with("manual: "),
        }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        // Review is needed for missing classifications or competing targets
        // for a single-file expression. A known role needs no confirmation.
        let mut q = self.prepare("SELECT m.id,s.path,m.location,p.pattern,m.direction
            FROM file_matches m JOIN artifacts s ON s.id=m.source_id
            JOIN artifacts a ON a.id=m.artifact_id
            JOIN file_paths p ON p.id=m.filepath_id
            LEFT JOIN artifact_roles ar ON ar.path=a.path
            WHERE m.decision_id=0
            AND (m.direction!='writes' OR instr(m.evidence,?1)=0)
            AND (ar.role IS NULL OR ar.role IN ('Unknown','Not connected')
                OR (p.is_multi=0 AND EXISTS (
                    SELECT 1 FROM file_matches other
                    WHERE other.source_id=m.source_id AND other.filepath_id=m.filepath_id
                    AND other.location=m.location AND other.direction=m.direction
                    AND other.artifact_id!=m.artifact_id AND other.decision_id!=2
                    AND (other.direction!='writes' OR instr(other.evidence,?1)=0)
                )))
            ORDER BY s.path,m.location,m.id")?;
        let issues = q.query_map([DIRECTORY_CHILD_EVIDENCE], |r| Ok(Issue {
            id:r.get(0)?,notebook:r.get(1)?,cell:r.get(2)?,filename:r.get(3)?,
            category:"unresolved".to_owned(),decision:"open".to_owned(),note:r.get(4)?,
        }))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((artifacts,relations,issues))
    }

    fn analysis_names(&self) -> Result<Vec<(String, String)>> {
        let mut q = self.prepare("SELECT source_folder,display_name FROM analysis_names")?;
        Ok(q.query_map([], |r| Ok((r.get(0)?,r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn source_artifact_id(&self, path: &str) -> Result<i64> {
        Ok(self.query_row("SELECT id FROM artifacts WHERE path=?1", [path], |r| r.get(0))?)
    }

    fn add_manual_match(&self, source: &str, target: &str, direction: &str, evidence: &str) -> Result<()> {
        anyhow::ensure!(matches!(direction, "reads" | "writes" | "writes_directory"),
            "Unsupported manual match direction: {direction}");
        let evidence = format!("manual: {evidence}");
        let source_id = self.source_artifact_id(source)?;
        let artifact_id = self.source_artifact_id(target)?;
        self.execute("INSERT INTO file_paths(pattern,is_multi) VALUES (?1,0) ON CONFLICT(pattern) DO NOTHING", [target])?;
        let filepath_id: i64 = self.query_row("SELECT id FROM file_paths WHERE pattern=?1", [target], |r| r.get(0))?;
        self.execute("INSERT INTO file_matches(source_id,filepath_id,artifact_id,location,direction,evidence,decision_id) \
            VALUES (?1,?2,?3,0,?4,?5,1) \
            ON CONFLICT(source_id,filepath_id,artifact_id,location,direction) \
            DO UPDATE SET evidence=excluded.evidence,decision_id=1",
            params![source_id,filepath_id,artifact_id,direction,evidence])?;
        Ok(())
    }

    fn refresh_cached_candidates(&mut self) -> Result<(i64, i64)> {
        let tx = self.transaction()?;
        // Re-evaluation is derived metadata only. Rejected file_matches remain
        // rejected, and accepted connections are never deleted here.
        infer_notebook_candidates(&tx)?;
        tx.execute("DELETE FROM inferred_links WHERE reason='script-order candidate'", [])?;
        infer_script_chains(&tx)?;
        let notebooks = tx.query_row("SELECT COUNT(*) FROM notebook_candidates", [], |r| r.get(0))?;
        let chains = tx.query_row("SELECT COUNT(*) FROM inferred_links WHERE reason='script-order candidate'", [], |r| r.get(0))?;
        tx.commit()?;
        Ok((notebooks, chains))
    }
}

/// Current wizard decision. Numeric values are stable SQLite representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum MatchDecision {
    NeedsWork = 0,
    Accepted = 1,
    Rejected = 2,
}

#[derive(Debug, Clone)]
pub struct WizardMatch {
    pub id: i64,
    pub artifact_id: i64,
    pub artifact: String,
    pub direction: String,
    pub evidence: String,
    pub decision_id: i64,
    pub role: Option<String>,
    pub role_confirmed: bool,
}

#[derive(Debug, Clone)]
pub struct WizardGroup {
    pub filepath_id: i64,
    pub location: i64,
    pub pattern: String,
    pub is_multi: bool,
    pub matches: Vec<WizardMatch>,
    pub direction: String,
}

#[derive(Debug, Clone)]
pub struct WizardInternals {
    pub source_id: i64,
    pub groups: Vec<WizardGroup>,
}

impl WizardInternals {
    pub fn reads(&self) -> impl Iterator<Item = &WizardGroup> {
        self.groups.iter().filter(|g| g.direction == "reads")
    }
    pub fn writes(&self) -> impl Iterator<Item = &WizardGroup> {
        self.groups.iter().filter(|g| matches!(g.direction.as_str(), "writes" | "writes_directory"))
    }
    pub fn reads_mut(&mut self) -> impl Iterator<Item = &mut WizardGroup> {
        self.groups.iter_mut().filter(|g| g.direction == "reads")
    }
    pub fn writes_mut(&mut self) -> impl Iterator<Item = &mut WizardGroup> {
        self.groups.iter_mut().filter(|g| matches!(g.direction.as_str(), "writes" | "writes_directory"))
    }
    pub fn validate(&self) -> Vec<(i64, String)> {
        let mut errors = Vec::new();
        for group in &self.groups {
            let accepted = group.matches.iter().filter(|m| m.decision_id == MatchDecision::Accepted as i64).count();
            if !group.is_multi && accepted > 1 {
                for m in group.matches.iter().filter(|m| m.decision_id == MatchDecision::Accepted as i64) {
                    errors.push((m.id, "Only one file can be accepted for this expression".into()));
                }
            }
            for m in &group.matches {
                if ![0, 1, 2].contains(&m.decision_id) {
                    errors.push((m.id, "Invalid connection decision".into()));
                }
                if let Some(role) = &m.role {
                    if let Err(message) = ArtifactRole::from_str(role) { errors.push((m.id, message)); }
                }
                if m.decision_id == 1 && m.role.as_deref().is_none_or(|r| r == "Unknown" || r == "Not connected") {
                    errors.push((m.id, "Accepted files require a classification".into()));
                }
            }
        }
        errors
    }
}

/// Extension methods on the existing rusqlite connection; no DB wrapper.
pub trait DbIssues {
    fn issues(&self, artifact_id: i64) -> Result<WizardInternals>;
    fn save_wizard(&mut self, data: &WizardInternals) -> Result<()>;
}

impl DbIssues for Connection {
    fn issues(&self, artifact_id: i64) -> Result<WizardInternals> {
        let mut stmt = self.prepare("SELECT m.id,m.filepath_id,m.location,p.pattern,p.is_multi,
                m.artifact_id,a.path,m.direction,m.decision_id,ar.role,m.evidence,COALESCE(ar.confirmed,0)
            FROM file_matches m
            JOIN file_paths p ON p.id=m.filepath_id
            JOIN artifacts a ON a.id=m.artifact_id
            LEFT JOIN artifact_roles ar ON ar.path=a.path
            WHERE m.source_id=?1
            ORDER BY CASE m.direction WHEN 'reads' THEN 0 WHEN 'writes' THEN 1 WHEN 'writes_directory' THEN 1 ELSE 2 END,
                     m.location, m.filepath_id, a.path, m.id")?;
        let rows = stmt.query_map(params![artifact_id], |r| Ok((
            r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,
            r.get::<_,String>(3)?,r.get::<_,i64>(4)? != 0,
            WizardMatch { id:r.get(0)?,artifact_id:r.get(5)?,artifact:r.get(6)?,
                direction:r.get(7)?,evidence:r.get(10)?,decision_id:r.get(8)?,role:r.get(9)?,role_confirmed:r.get::<_,i64>(11)? != 0 }
        )))?;
        let mut groups: Vec<WizardGroup> = Vec::new();
        for row in rows {
            let (id,filepath_id,location,pattern,is_multi,item) = row?;
            let _ = id;
            if is_directory_child_link(&item.direction, &item.evidence) { continue; }
            if let Some(group) = groups.iter_mut().find(|g| g.filepath_id==filepath_id && g.location==location && g.direction==item.direction) {
                group.matches.push(item);
            } else {
                groups.push(WizardGroup {filepath_id,location,pattern,is_multi,direction:item.direction.clone(),matches:vec![item]});
            }
        }
        Ok(WizardInternals {source_id:artifact_id,groups})
    }

    fn save_wizard(&mut self, data: &WizardInternals) -> Result<()> {
        let errors = data.validate();
        anyhow::ensure!(errors.is_empty(), "Wizard has {} invalid selections: {:?}", errors.len(), errors);
        let tx = self.transaction()?;
        let source_path: String = tx.query_row("SELECT path FROM artifacts WHERE id=?1", params![data.source_id], |r| r.get(0))?;
        for group in &data.groups {
            for item in &group.matches {
                let changed = tx.execute("UPDATE file_matches SET decision_id=?1 WHERE id=?2 AND source_id=?3 AND filepath_id=?4 AND location=?5 AND direction=?6 AND artifact_id=?7",
                    params![item.decision_id,item.id,data.source_id,group.filepath_id,group.location,&group.direction,item.artifact_id])?;
                anyhow::ensure!(changed == 1, "Wizard match {} is stale", item.id);
                if item.decision_id == MatchDecision::Accepted as i64 {
                    let role = ArtifactRole::from_str(item.role.as_deref().unwrap_or("Unknown"))
                        .map_err(anyhow::Error::msg)?;
                    tx.execute("INSERT INTO artifact_roles(path,role,source,confirmed) VALUES (?1,?2,?3,1) ON CONFLICT(path) DO UPDATE SET role=excluded.role,source=excluded.source,confirmed=1",
                        params![&item.artifact,role.label(),source_path])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}

pub fn set_match_decision(db: &Connection, match_id: i64, decision_id: i64) -> Result<()> {
    let changed=db.execute("UPDATE file_matches SET decision_id=?1 WHERE id=?2",params![decision_id,match_id])?;
    anyhow::ensure!(changed==1,"Unknown file match {match_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ignored_scripts_survive_rescans_and_remain_reviewable() {
        let db = Connection::open_in_memory().unwrap();
        crate::core::engine::schema(&db).unwrap();
        db.record_match("analysis/a.py", 3, "*.h5ad", "analysis/data.h5ad", "reads", "test").unwrap();
        db.set_script_ignored("analysis/a.py", true).unwrap();
        assert!(db.ignored_review_scripts().unwrap().contains("analysis/a.py"));
        let source: i64 = db.query_row("SELECT id FROM artifacts WHERE path='analysis/a.py'", [], |r| r.get(0)).unwrap();
        assert!(!db.issues(source).unwrap().groups.is_empty());
        // Re-running schema (as the scanner does) must not reset review choices.
        crate::core::engine::schema(&db).unwrap();
        assert!(db.ignored_review_scripts().unwrap().contains("analysis/a.py"));
        db.set_script_ignored("analysis/a.py", false).unwrap();
        assert!(!db.ignored_review_scripts().unwrap().contains("analysis/a.py"));
    }
    #[test]
    fn folder_children_remain_in_provenance_but_are_reviewed_through_the_folder() -> Result<()> {
        let db = Connection::open_in_memory()?;
        crate::core::engine::schema(&db)?;
        db.record_match("a.py", 3, "stats", "stats", "writes_directory", "directory writer")?;
        db.record_match("a.py", 3, "stats/child.csv", "stats/child.csv", "writes",
            "a.py: cell/line 3: inferred under output directory stats from writer(stats)")?;
        db.record_match("a.py", 4, "stats/direct.csv", "stats/direct.csv", "writes", "explicit writer")?;

        let review = db.issues(db.source_artifact_id("a.py")?)?;
        let targets: HashSet<_> = review.writes().flat_map(|g| &g.matches)
            .map(|m| m.artifact.as_str()).collect();
        assert_eq!(targets, HashSet::from(["stats", "stats/direct.csv"]));

        let (_, relations, issues) = db.archive_view()?;
        assert!(relations.iter().any(|r| r.target == "stats/child.csv"));
        assert_eq!(issues.iter().map(|i| i.filename.as_str()).collect::<HashSet<_>>(),
            HashSet::from(["stats", "stats/direct.csv"]));

        db.execute("INSERT INTO artifact_roles(path,role,source,confirmed)
            VALUES ('stats','Statistical results','test',0),
                   ('stats/direct.csv','OtherDataFile','test',0)", [])?;
        let (_, relations, issues) = db.archive_view()?;
        assert_eq!(relations.len(), 3);
        assert!(issues.is_empty());

        db.record_match("a.py", 4, "stats/direct.csv", "alternate/direct.csv", "writes", "competing target")?;
        db.execute("INSERT INTO artifact_roles(path,role,source,confirmed)
            VALUES ('alternate/direct.csv','OtherDataFile','test',0)", [])?;
        db.execute("UPDATE file_paths SET is_multi=0 WHERE pattern='stats/direct.csv'", [])?;
        let (_, _, issues) = db.archive_view()?;
        assert_eq!(issues.len(), 2);
        Ok(())
    }

    #[test]
    fn issues_include_rejected_and_accepted_matches() {
        let db=Connection::open_in_memory().unwrap();
        crate::core::engine::schema(&db).unwrap();
        for path in ["a.py","one.h5ad","two.h5ad"] {
            db.execute("INSERT INTO artifacts(path,kind) VALUES (?1,'file')",params![path]).unwrap();
        }
        db.execute("INSERT INTO file_paths(pattern,is_multi) VALUES ('*.h5ad',1)",[]).unwrap();
        for (path,decision) in [("one.h5ad",1),("two.h5ad",2)] {
            db.execute("INSERT INTO file_matches(source_id,filepath_id,artifact_id,location,direction,decision_id)
                SELECT s.id,p.id,a.id,7,'reads',?1 FROM artifacts s,artifacts a,file_paths p
                WHERE s.path='a.py' AND a.path=?2",params![decision,path]).unwrap();
        }
        let source:i64=db.query_row("SELECT id FROM artifacts WHERE path='a.py'",[],|r|r.get(0)).unwrap();
        let wizard=db.issues(source).unwrap();
        assert_eq!(wizard.groups.len(),1);
        assert_eq!(wizard.groups[0].matches.len(),2);
        assert_eq!(wizard.groups[0].matches.iter().map(|m|m.decision_id).collect::<Vec<_>>(),vec![1,2]);
    }
    #[test]
    fn wizard_edits_reopens_and_reverses_connections() {
        let mut db = Connection::open_in_memory().unwrap();
        crate::core::engine::schema(&db).unwrap();
        db.record_match("a.py", 7, "*.h5ad", "one.h5ad", "reads", "glob").unwrap();
        db.record_match("a.py", 7, "*.h5ad", "two.h5ad", "reads", "glob").unwrap();
        db.record_match("a.py", 8, "out.h5ad", "out.h5ad", "writes", "save").unwrap();
        let id: i64 = db.query_row("SELECT id FROM artifacts WHERE path='a.py'", [], |r| r.get(0)).unwrap();
        let mut wizard = db.issues(id).unwrap();
        assert_eq!(wizard.reads().count(), 1);
        assert_eq!(wizard.writes().count(), 1);
        assert_eq!(wizard.groups[0].location, 7);
        wizard.groups[0].matches[0].decision_id = 1;
        wizard.groups[0].matches[0].role = Some("Processed data".into());
        wizard.groups[0].matches[1].decision_id = 2;
        db.save_wizard(&wizard).unwrap();
        let mut reopened = db.issues(id).unwrap();
        assert_eq!(reopened.groups[0].matches.iter().map(|m|m.decision_id).collect::<Vec<_>>(), vec![1,2]);
        reopened.groups[0].matches[0].decision_id = 2;
        reopened.groups[0].matches[1].decision_id = 1;
        reopened.groups[0].matches[1].role = Some("Processed data".into());
        db.save_wizard(&reopened).unwrap();
        let changed = db.issues(id).unwrap();
        assert_eq!(changed.groups[0].matches.iter().map(|m|m.decision_id).collect::<Vec<_>>(), vec![2,1]);
    }

}

/// Local SQLite workspace, archive publication, and the single database owner.
/// Single owner for the local SQLite session and its archive persistence.
use super::*;
use std::ops::{Deref, DerefMut};

// Keep SQLite transactions off SMB/CIFS shares. The archive only receives a
// closed, verified database snapshot; each worker owns a separate local folder.
pub struct BatchWorkspace { path: PathBuf, preserve: bool }
impl BatchWorkspace {
    pub fn new() -> Result<Self> {
        let base = std::env::var_os("TMPDIR").map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        for attempt in 0..1000u32 {
            let path = base.join(format!("folder-history-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos(),
                attempt));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, preserve: false }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("Unable to create a unique local scanning directory")
    }
}
impl BatchWorkspace {
    pub fn preserve(&mut self) { self.preserve = true; }
    pub fn path(&self) -> &Path { &self.path }
}
impl Drop for BatchWorkspace {
    fn drop(&mut self) {
        if !self.preserve { let _ = std::fs::remove_dir_all(&self.path); }
    }
}

// A real write probe is more reliable than permission bits on SMB mounts.
pub fn probe_archive_write_access(root: &Path) -> Result<()> {
    let target = root.join(".folder_history");
    let dir = if target.is_dir() { target.as_path() } else { root };
    let name = format!(".folderhistory-write-test-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos());
    let path = dir.join(name);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)
        .with_context(|| format!("Archive is not writable: {}", dir.display()))?;
    use std::io::Write;
    let write_result = file.write_all(b"FolderHistory write probe\n");
    drop(file);
    let remove_result = std::fs::remove_file(&path);
    write_result?;
    remove_result?;
    Ok(())
}

// Copy only the canonical archive index into a local workspace. Never use
// SQLite on the archive path, and never import a legacy database.
// A missing archive is a new index, not a migration request.
pub fn archive_working_copy(root: &Path) -> Result<(BatchWorkspace, PathBuf, PathBuf, bool)> {
    let archive = local_index_path(root);
    let workspace = BatchWorkspace::new()?;
    let local = workspace.path.join("history.sqlite");
    let exists = archive.is_file();
    if exists {
        // A raw copy is safe only for a closed, single-file SQLite database.
        // A sidecar is not necessarily a lock: report exactly what we found.
        ensure_no_sqlite_sidecars(&archive)?;
        std::fs::copy(&archive, &local)
            .with_context(|| format!("Copying archive index {} to local workspace", archive.display()))?;
        let compatible = Connection::open_with_flags(&local, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|db| db.query_row("PRAGMA user_version", [], |r| r.get::<_,i64>(0)))
            .is_ok_and(|version| version == INDEX_SCHEMA_VERSION);
        if !compatible {
            // Discard ONLY the disposable local copy. A successful fresh scan
            // atomically replaces the archive index; failed scans leave it intact.
            std::fs::remove_file(&local)?;
        }
    }
    Ok((workspace, local, archive, exists))
}

fn ensure_no_sqlite_sidecars(database: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = std::ffi::OsString::from(format!("{}{}", database.display(), suffix));
        let path = PathBuf::from(sidecar);
        if path.exists() {
            anyhow::bail!("SQLite sidecar exists at {}. Cannot safely copy the database as a single file; this does not necessarily mean it is locked.", path.display());
        }
    }
    Ok(())
}

// Publish a closed, consistent local SQLite snapshot. Both CLI and GUI use this.
pub fn publish_archive_snapshot(local: &Path, archive: &Path) -> Result<()> {
    let parent = archive.parent().context("Archive database has no parent directory")?;
    // Publication must use a closed, standalone SQLite file.
    ensure_no_sqlite_sidecars(local)?;
    if archive.exists() { ensure_no_sqlite_sidecars(archive)?; }
    std::fs::create_dir_all(parent)?;
    let staging = archive.with_extension(format!("sqlite.publish-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()));
    let result = (|| -> Result<()> {
        std::fs::copy(local, &staging)
            .with_context(|| format!("Copying SQLite snapshot into {}", staging.display()))?;
        // Do not replace an existing database with a partial transfer.
        std::fs::rename(&staging, archive)
            .with_context(|| format!("Replacing archive SQLite {}", archive.display()))?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&staging); }
    result
}


/// Owns the SQLite connection and both sides of the archive lifecycle.
/// The workspace is intentionally declared before the connection, and explicitly
/// retained on unsuccessful publication.
pub struct HistoryDatabase {
    connection: Connection,
    workspace: Option<BatchWorkspace>,
    root: PathBuf,
    local: PathBuf,
    archive: PathBuf,
    published: bool,
}

impl HistoryDatabase {
    pub fn open(root: &Path, require_existing: bool, check_writable: bool) -> Result<Self> {
        if !root.is_dir() { anyhow::bail!("Folder does not exist: {}", root.display()); }
        if check_writable { probe_archive_write_access(root)?; }
        let (workspace, local, archive, exists) = archive_working_copy(root)?;
        if require_existing && !exists {
            anyhow::bail!("No FolderHistory database found in .folder_history/history.sqlite");
        }
        let connection = Connection::open(&local)
            .with_context(|| format!("Opening local SQLite {}", local.display()))?;
        connection.busy_timeout(std::time::Duration::from_secs(30))?;
        connection.execute_batch("PRAGMA journal_mode=DELETE;")?;
        schema(&connection)?;
        Ok(Self { connection, workspace: Some(workspace), root: root.to_path_buf(), local, archive, published: false })
    }

    /// Attach to the GUI session's local database without owning its workspace.
    pub fn attach_worker(root: &Path, local: &Path) -> Result<Self> {
        let connection = Self::worker_connection(local)?;
        Ok(Self { connection, workspace: None, root: root.to_path_buf(),
            local: local.to_path_buf(), archive: super::engine::local_index_path(root), published: false })
    }

    pub fn local_path(&self) -> &Path { &self.local }
    pub fn archive_path(&self) -> &Path { &self.archive }
    pub fn root(&self) -> &Path { &self.root }

    /// A separate local connection for worker threads. Never opens SQLite on NAS.
    pub fn worker_connection(local: &Path) -> Result<Connection> {
        let connection = Connection::open(local)?;
        connection.busy_timeout(std::time::Duration::from_secs(30))?;
        connection.execute_batch("PRAGMA journal_mode=DELETE;")?;
        schema(&connection)?;
        Ok(connection)
    }

    /// Publish a consistent snapshot while the local connection remains open.
    pub fn publish(&mut self) -> Result<()> {
        let snapshot = self.local.with_extension("publish-snapshot.sqlite");
        if snapshot.exists() { std::fs::remove_file(&snapshot)?; }
        let escaped = snapshot.to_string_lossy().replace('\'', "''");
        self.connection.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
        match publish_archive_snapshot(&snapshot, &self.archive) {
            Ok(()) => {
                self.published = true;
                let _ = std::fs::remove_file(&snapshot);
                Ok(())
            }
            Err(e) => {
                if let Some(workspace) = &mut self.workspace { workspace.preserve(); }
                Err(e.context(format!("Local recovery snapshot: {}", snapshot.display())))
            }
        }
    }

    pub fn scan(&mut self, hidden: bool, cancel: &AtomicBool, progress: impl FnMut(String)) -> Result<super::model::ScanResult> {
        self.published = false;
        super::engine::scan(&mut self.connection, &self.root, hidden, cancel, progress)
    }

    pub fn relink(&mut self) -> Result<(i64, i64)> {
        self.published = false;
        relink_cached_evidence(&mut self.connection)
    }

    /// Call before dropping a session after edits; preserves the local database
    /// if publication fails.
    pub fn preserve_working_copy(&mut self) { if let Some(workspace) = &mut self.workspace { workspace.preserve(); } }
}

/// Human review status is independent of scan-time provenance and file roles.
/// Ignored scripts continue to receive automatic evidence on every rescan.
pub trait DbScriptReview {
    fn ignored_review_scripts(&self) -> Result<HashSet<String>>;
    fn set_script_ignored(&self, source: &str, ignored: bool) -> Result<()>;
}

impl DbScriptReview for Connection {
    fn ignored_review_scripts(&self) -> Result<HashSet<String>> {
        let mut stmt = self.prepare("SELECT source_path FROM ignored_review_scripts")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>().map_err(Into::into)
    }

    fn set_script_ignored(&self, source: &str, ignored: bool) -> Result<()> {
        if ignored {
            self.execute("INSERT OR IGNORE INTO ignored_review_scripts(source_path) VALUES (?1)", [source])?;
        } else {
            self.execute("DELETE FROM ignored_review_scripts WHERE source_path=?1", [source])?;
        }
        Ok(())
    }
}

impl Deref for HistoryDatabase {
    type Target = Connection;
    fn deref(&self) -> &Connection { &self.connection }
}
impl DerefMut for HistoryDatabase {
    fn deref_mut(&mut self) -> &mut Connection { &mut self.connection }
}


// SQLite schema and migration logic.
pub const INDEX_SCHEMA_VERSION: i64 = 6;

pub fn schema(db: &Connection) -> Result<()> {
    db.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS analysis_names (source_folder TEXT PRIMARY KEY, display_name TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS ignored_review_scripts (source_path TEXT PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS artifacts (
            id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
            size INTEGER, modified INTEGER, created INTEGER);
        CREATE TABLE IF NOT EXISTS relocation_candidates (
            source TEXT NOT NULL, line INTEGER NOT NULL, expression TEXT NOT NULL,
            path TEXT NOT NULL, PRIMARY KEY(source,line,expression,path));
        CREATE TABLE IF NOT EXISTS steps (
            id INTEGER PRIMARY KEY, source TEXT NOT NULL, ordinal INTEGER NOT NULL,
            command TEXT NOT NULL, UNIQUE(source,ordinal));
        CREATE TABLE IF NOT EXISTS step_io (
            step_id INTEGER NOT NULL, artifact TEXT NOT NULL, direction TEXT NOT NULL,
            PRIMARY KEY(step_id,artifact,direction));
        CREATE TABLE IF NOT EXISTS scans (
            id INTEGER PRIMARY KEY, completed INTEGER NOT NULL DEFAULT 0,
            files INTEGER NOT NULL DEFAULT 0, steps INTEGER NOT NULL DEFAULT 0,
            links INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS file_paths (
            id INTEGER PRIMARY KEY,
            pattern TEXT NOT NULL UNIQUE,
            is_multi INTEGER NOT NULL DEFAULT 0 CHECK(is_multi IN (0,1))
        );
        CREATE TABLE IF NOT EXISTS file_matches (
            id INTEGER PRIMARY KEY,
            source_id INTEGER NOT NULL REFERENCES artifacts(id),
            filepath_id INTEGER NOT NULL REFERENCES file_paths(id),
            artifact_id INTEGER NOT NULL REFERENCES artifacts(id),
            location INTEGER NOT NULL,
            direction TEXT NOT NULL CHECK(direction IN ('reads','writes','writes_directory')),
            decision_id INTEGER NOT NULL DEFAULT 0,
            evidence TEXT NOT NULL DEFAULT '',
            UNIQUE(source_id,filepath_id,artifact_id,location,direction)
        );
        CREATE INDEX IF NOT EXISTS file_matches_source ON file_matches(source_id);
        CREATE INDEX IF NOT EXISTS file_matches_artifact ON file_matches(artifact_id);
        PRAGMA user_version=6;")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS file_classifications (
        source TEXT NOT NULL, path TEXT NOT NULL, role TEXT NOT NULL,
        confirmed INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY(source,path)
    );")?;
    // Directory review decisions are annotations, never deletion instructions.
    db.execute_batch("CREATE TABLE IF NOT EXISTS folder_reviews (
        path TEXT PRIMARY KEY, decision TEXT NOT NULL
    );")?;
    // Canonical scientist-confirmed roles, shared by the wizard, scanner and UI.
    // Source-scoped legacy evidence is retained in file_classifications.
    db.execute_batch("CREATE TABLE IF NOT EXISTS artifact_roles (
        path TEXT PRIMARY KEY, role TEXT NOT NULL, source TEXT NOT NULL,
        confirmed INTEGER NOT NULL DEFAULT 1
    );")?;
    // Upgrade confirmations saved by earlier wizard versions. Resolve only
    // unambiguous paths, never invent a match for duplicate basenames.
    {
        let paths: HashSet<String> = db.prepare("SELECT path FROM artifacts")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut q = db.prepare("SELECT source,path,role FROM file_classifications WHERE confirmed=1 ORDER BY rowid")?;
        let rows = q.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (source, reference, role) in rows {
            if role == "Not connected" { continue; }
            let Ok(role) = ArtifactRole::from_str(&role) else { continue; };
            if let Some(path) = classified_artifact_path(&reference, &source, &paths) {
                db.execute("INSERT OR IGNORE INTO artifact_roles(path,role,source,confirmed) VALUES (?1,?2,?3,1)",
                    params![path,role.label(),source])?;
            }
        }
    }
    db.execute_batch("CREATE TABLE IF NOT EXISTS timestamp_assessments (
        folder TEXT PRIMARY KEY, files INTEGER NOT NULL, correlation REAL,
        transfer_mb_s REAL, suspected_copy INTEGER NOT NULL, reason TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS notebook_observations (
            notebook TEXT NOT NULL, cell INTEGER NOT NULL, expression TEXT NOT NULL,
            observed_path TEXT NOT NULL, evidence TEXT NOT NULL,
            PRIMARY KEY(notebook,cell,expression,observed_path));
        CREATE TABLE IF NOT EXISTS notebook_expressions (
            notebook TEXT NOT NULL, cell INTEGER NOT NULL, expression TEXT NOT NULL,
            has_output INTEGER NOT NULL, PRIMARY KEY(notebook,cell,expression));
        CREATE TABLE IF NOT EXISTS notebook_candidates (
            notebook TEXT NOT NULL, cell INTEGER NOT NULL, expression TEXT NOT NULL,
            candidate_path TEXT NOT NULL, source_notebook TEXT NOT NULL, source_cell INTEGER NOT NULL,
            PRIMARY KEY(notebook,cell,expression,candidate_path,source_notebook,source_cell));
        CREATE TABLE IF NOT EXISTS symlink_findings (
            path TEXT PRIMARY KEY, target TEXT NOT NULL, category TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS inferred_links (
        source TEXT NOT NULL, target TEXT NOT NULL, reason TEXT NOT NULL,
        PRIMARY KEY(source,target,reason));")?;
    Ok(())
}

/// Authoritative roles are intentionally not provenance edges: a stage label
/// cannot prove which input produced an output.
pub fn confirmed_roles(db: &Connection) -> Result<HashMap<String, String>> {
    let mut stmt = db.prepare("SELECT path,role FROM artifact_roles WHERE confirmed=1")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let values = rows.collect::<rusqlite::Result<HashMap<String, String>>>()?;
    for (path, role) in &values {
        ArtifactRole::from_str(role).map_err(|e| anyhow::anyhow!("Invalid saved classification for {path}: {e}"))?;
    }
    Ok(values)
}


pub fn infer_notebook_candidates(db: &Connection) -> Result<()> {
    // Cross-notebook observations are candidates, NOT automatically confirmed graph edges.
    db.execute_batch("DELETE FROM notebook_candidates;
        INSERT OR IGNORE INTO notebook_candidates(notebook,cell,expression,candidate_path,source_notebook,source_cell)
        SELECT dst.notebook,dst.cell,dst.expression,src.observed_path,src.notebook,src.cell
        FROM notebook_expressions dst JOIN notebook_observations src
          ON dst.expression=src.expression AND dst.notebook<>src.notebook
        WHERE dst.has_output=0;")?;
    Ok(())
}

pub fn global_registry_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".folder_history/external_links.db"))
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".folder_history/external_links.db")))
}
pub fn save_global_external_link(name: &str, url: &str) -> Result<()> {
    // Only explicit URL resolutions are globally reusable. Never save private local paths.
    if !(url.starts_with("https://") || url.starts_with("http://")) { return Ok(()); }
    let Some(path) = global_registry_path() else { return Ok(()); };
    std::fs::create_dir_all(path.parent().context("Registry parent missing")?)?;
    let db = Connection::open(path)?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS resources (
        filename TEXT PRIMARY KEY, url TEXT NOT NULL, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
    db.execute("INSERT INTO resources(filename,url) VALUES (?1,?2)
        ON CONFLICT(filename) DO UPDATE SET url=excluded.url,updated_at=CURRENT_TIMESTAMP", params![name,url])?;
    Ok(())
}
pub fn lookup_global_external_link(name: &str) -> Option<String> {
    let path = global_registry_path()?;
    let db = Connection::open_with_flags(path,OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    db.query_row("SELECT url FROM resources WHERE filename=?1",params![name],|r|r.get(0)).ok()
}

pub fn infer_script_chains(db: &Connection) -> Result<()> {
    let mut stmt=db.prepare("SELECT s.source,s.ordinal,io.artifact,io.direction FROM steps s JOIN step_io io ON s.id=io.step_id ORDER BY s.source,s.ordinal")?;
    let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?;
    let mut previous_script=String::new();
    let mut inputs=HashSet::<String>::new();
    let mut pending=Vec::<(String,String)>::new();
    for row in rows {
        let (script,_,artifact,direction)=row?;
        if script != previous_script { inputs.clear(); previous_script=script; }
        if direction=="reads" { inputs.insert(artifact); }
        else if direction=="writes" {
            for input in &inputs { if input!=&artifact { pending.push((input.clone(),artifact.clone())); } }
            // A later write should not inherit every earlier input forever.
            inputs.clear();
        }
    }
    for (input,output) in pending {
        db.execute("INSERT OR IGNORE INTO inferred_links(source,target,reason) VALUES (?1,?2,'script-order candidate')",params![input,output])?;
    }
    Ok(())
}
// A file-copy sequence may show time gaps proportional to the preceding file size.
// Require substantial sample size, high positive correlation, and older preserved
// modification times. This flags suspicious metadata; it does not prove copying.

pub fn assess_timestamps(db: &Connection, files: &[(String,String,i64,Option<i64>,Option<i64>)]) -> Result<()> {
    let mut groups: HashMap<String,Vec<(i64,i64,i64)>>=HashMap::new();
    for (path,_,size,modified,created) in files {
        if let (Some(m),Some(c))=(modified,created) {
            if *size>0 { let folder=path.rsplit_once('/').map(|p|p.0).unwrap_or("").to_owned();
                groups.entry(folder).or_default().push((*c,*m,*size)); }
        }
    }
    for (folder,mut items) in groups {
        if items.len()<8 { continue; }
        items.sort_by_key(|v|v.0);
        let mut xy=Vec::new();
        let mut older=0usize;
        for &(c,m,_) in &items { if c-m>86400 { older+=1; } }
        for w in items.windows(2) {
            let dt=w[1].0-w[0].0;
            if (1..=7200).contains(&dt) { xy.push((w[0].2 as f64 / 1e6,dt as f64)); }
        }
        if xy.len()<7 { continue; }
        let n=xy.len() as f64;
        let (sx,sy)=xy.iter().fold((0.,0.),|(a,b),(x,y)|(a+x,b+y));
        let (mx,my)=(sx/n,sy/n);
        let (cov,vx,vy)=xy.iter().fold((0.,0.,0.),|(c,xv,yv),(x,y)| (c+(x-mx)*(y-my),xv+(x-mx).powi(2),yv+(y-my).powi(2)));
        if vx<=0. || vy<=0. { continue; }
        let r=cov/(vx*vy).sqrt();
        let speed=if cov>0. { vx/cov } else { 0. };
        let suspected=xy.len()>=10 && r>0.85 && speed>=1. && speed<=2000. && older*2>=items.len();
        db.execute("INSERT OR REPLACE INTO timestamp_assessments(folder,files,correlation,transfer_mb_s,suspected_copy,reason) VALUES (?1,?2,?3,?4,?5,'creation gaps correlate with preceding file sizes; modification dates predate creation')",
            params![folder,items.len() as i64,r,speed,suspected as i32])?;
    }
    Ok(())
}


pub fn add_relation(db: &Connection, source: &str, target: &str, kind: &str, evidence: &str, manual: bool) -> Result<()> {
    use self::{DbArchiveView, DbProvenance};
    if manual { db.add_manual_match(source, target, kind, evidence) }
    else { db.record_match(source, 0, target, target, kind, evidence) }
}

pub fn discover_notebook_observations(db: &Connection, root: &Path, rel: &str) -> Result<()> {
    let content = std::fs::read_to_string(root.join(rel))?;
    let notebook: serde_json::Value = serde_json::from_str(&content)?;
    let Some(cells) = notebook.get("cells").and_then(|v| v.as_array()) else { return Ok(()); };
    // Generic path discovery: no knowledge of particular scientific packages.
    let glob = Regex::new(r#"(?:Sys\.glob|glob\.glob)\s*\(\s*['\"]([^'\"]+)['\"]"#)?;
    let output_path = Regex::new(r#"(?:[A-Za-z]:[\\/]|/|\.\.?/)[^\s\"'<>\[\],;]+"#)?;
    for (idx, cell) in cells.iter().enumerate() {
        if cell.get("cell_type").and_then(|v| v.as_str()) != Some("code") { continue; }
        let source = cell_text(cell.get("source"));
        let output = notebook_output_text(cell);
        for cap in glob.captures_iter(&source) {
            let expression = cap[1].to_string();
            db.execute("INSERT OR IGNORE INTO notebook_expressions(notebook,cell,expression,has_output) VALUES (?1,?2,?3,?4)",
                params![rel,(idx+1) as i64,expression,(!output.trim().is_empty()) as i32])?;
            for candidate in output_path.find_iter(&output) {
                let observed = candidate.as_str().trim_end_matches([')', ':', '.', '\r']);
                // Require the output to resemble the glob's final component.
                let stem = expression.rsplit('/').next().unwrap_or("")
                    .split(['*','?','[']).next().unwrap_or("");
                if stem.len() < 3 || !observed.rsplit('/').next().unwrap_or("").starts_with(stem) { continue; }
                db.execute("INSERT OR IGNORE INTO notebook_observations(notebook,cell,expression,observed_path,evidence) VALUES (?1,?2,?3,?4,'saved_cell_output')",
                    params![rel,(idx+1) as i64,expression,observed])?;
            }
        }
    }
    Ok(())
}

pub fn relink_cached_evidence(db: &mut Connection) -> Result<(i64, i64)> {
    use self::DbArchiveView;
    db.refresh_cached_candidates()
}


pub fn parse_script(db: &Connection, root: &Path, rel: &str, paths: &HashSet<String>) -> Result<(usize,usize)> {
    let metadata=std::fs::metadata(root.join(rel))?;
    if metadata.len()>8*1024*1024 { return Ok((0,0)); }
    let content=std::fs::read_to_string(root.join(rel))?;
    let mut steps=0; let mut links=0;
    for (idx,line) in content.lines().enumerate() {
        let Some(op)=extract_shell(line) else { continue; };
        steps+=1;
        // Sanitized operation label: no arbitrary command text or external paths.
        db.execute("INSERT OR REPLACE INTO steps(source,ordinal,command) VALUES (?1,?2,'shell operation')",params![rel,(idx+1) as i64])?;
        let step_id:i64=db.query_row("SELECT id FROM steps WHERE source=?1 AND ordinal=?2",params![rel,(idx+1) as i64],|r|r.get(0))?;
        for (values,direction) in [(op.reads,"reads"),(op.writes,"writes")] {
            for value in values {
                if value.contains('$') || value.contains('*') || value.contains('{') { continue; }
                if reference_name(&value).is_empty() || !plausible_reference(&value) { continue; }
                let (resolved,_,_)=resolve_reference(&value,rel,paths);
                if let Some(path)=resolved {
                    db.execute("INSERT OR IGNORE INTO step_io(step_id,artifact,direction) VALUES (?1,?2,?3)",params![step_id,path,direction])?;
                    DbProvenance::record_match(db,rel,idx+1,&value,&path,direction,&format!("{}: line {}: {}",rel,idx+1,direction))?;
                    links+=1;
                }
            }
        }
    }
    Ok((steps,links))
}


pub fn local_index_path(root: &Path) -> PathBuf {
    root.join(".folder_history").join("history.sqlite")
}


// A database may exist after an interrupted scan. Only this marker certifies
// that the batch scan returned successfully. It is not a database lock file.
pub fn batch_finished_path(db_path: &Path) -> PathBuf {
    db_path.with_extension("sqlite.finished")
}

pub fn batch_scan_folder(root: &Path, rescan: bool) -> Result<String> {
    if !root.is_dir() { anyhow::bail!("Not a directory: {}", root.display()); }
    let root = root.canonicalize()?;
    let db_path = local_index_path(&root);
    let finished = batch_finished_path(&db_path);
    if db_path.is_file() && finished.is_file() && !rescan {
        let current_marker = std::fs::read_to_string(&finished).ok()
            .is_some_and(|s| s.lines().any(|line| line == format!("schema_version={INDEX_SCHEMA_VERSION}")));
        let current_db = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|db| db.query_row("PRAGMA user_version", [], |r| r.get::<_,i64>(0)))
            .is_ok_and(|v| v == INDEX_SCHEMA_VERSION);
        if current_marker && current_db {
            return Ok(format!("SKIP completed project index: {}", db_path.display()));
        }
    }
    if let Some(parent) = db_path.parent() { std::fs::create_dir_all(parent)?; }

    let mut database = HistoryDatabase::open(&root, false, true)?;
    let cancel = AtomicBool::new(false);
    let started = std::time::Instant::now();
    let result = database.scan(false, &cancel, |message| {
        eprintln!("    {message}");
    })?;
    let integrity: String = database.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" { anyhow::bail!("Local SQLite integrity check failed: {integrity}"); }
    // Copy into the destination directory first. The existing archive database
    // remains intact if the scan or transfer fails.
    // Invalidate the old completion marker before replacing the archive.
    if finished.exists() { std::fs::remove_file(&finished)?; }
    database.publish()?;
    let marker = format!("schema_version={INDEX_SCHEMA_VERSION}\ncompleted_unix={}\nfiles={}\nsteps={}\nlinks={}\n",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        result.files, result.steps, result.links);
    let temporary = finished.with_extension(format!("sqlite.finished.tmp-{}", std::process::id()));
    std::fs::write(&temporary, marker)?;
    let marker_result = std::fs::rename(&temporary, &finished);
    if marker_result.is_err() { let _ = std::fs::remove_file(&temporary); }
    marker_result?;
    Ok(format!("OK files={} scripts={} parsed={} links={} warnings={} elapsed={:.1}s index={}",
        result.files, result.scripts, result.parsed, result.links,
        result.warnings, started.elapsed().as_secs_f64(), db_path.display()))
}



/// File inventory and source evidence are discovered by the engine; the database
/// owns all table updates and the rescan transaction.
#[derive(Debug, Clone)]
pub struct SymlinkFinding {
    pub path: String,
    pub target: String,
    pub category: String,
}

pub struct ScanInventory {
    pub files: Vec<(String, String, i64, Option<i64>, Option<i64>)>,
    pub paths: HashSet<String>,
    pub symlinks: Vec<SymlinkFinding>,
    pub warnings: usize,
}

pub fn persist_scan(db: &mut Connection, root: &Path, inventory: ScanInventory,
    cancel: &AtomicBool, mut progress: impl FnMut(String)) -> Result<ScanResult> {
    let files = &inventory.files;
    let paths = &inventory.paths;
    let file_paths: HashSet<&str> = files.iter().map(|f| f.0.as_str()).collect();
    let tx = db.transaction()?;
    // Guard against a successful-looking rescan erasing all established
    // provenance. The transaction rolls back if rebuilding yields no matches.
    let previous_match_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM file_matches", [], |row| row.get(0)
    )?;
    // Directory roles must be first-class artifacts too. No synthetic
    // producer/consumer edges are created for directories.
    for path in paths {
        if !file_paths.contains(path.as_str()) {
            tx.execute("INSERT INTO artifacts(path,kind) VALUES (?1,'directory')
                ON CONFLICT(path) DO UPDATE SET kind='directory'", params![path])?;
        }
    }
    for (path, kind, size, modified, created) in files {
        if cancel.load(Ordering::Relaxed) { anyhow::bail!("Scan cancelled"); }
        tx.execute("INSERT INTO artifacts(path,kind,size,modified,created) VALUES (?1,?2,?3,?4,?5)
            ON CONFLICT(path) DO UPDATE SET kind=excluded.kind,size=excluded.size,modified=excluded.modified,created=excluded.created",
            params![path,kind,size,modified,created])?;
    }
    // Matches are derived scan evidence; discard before pruning referenced artifacts.
    // Human classifications and manual relations remain untouched.
    tx.execute_batch("DROP TABLE IF EXISTS temp.saved_match_decisions;
        CREATE TEMP TABLE saved_match_decisions AS
        SELECT source_id,filepath_id,artifact_id,location,direction,decision_id
        FROM file_matches WHERE decision_id <> 0;")?;
    tx.execute("DELETE FROM file_matches", [])?;
    // Drop stale inventory rows from previous scans (including previously indexed
    // checkpoint/work files). Keep manually curated relations untouched.
    {
        let mut stmt = tx.prepare("SELECT path FROM artifacts")?;
        let stored = stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for old in stored {
            if !paths.contains(&old) {
                tx.execute("DELETE FROM artifacts WHERE path=?1", params![old])?;
            }
        }
    }
    tx.execute("DELETE FROM notebook_observations", [])?;
    tx.execute("DELETE FROM notebook_expressions", [])?;
    tx.execute("DELETE FROM notebook_candidates", [])?;
    tx.execute("DELETE FROM symlink_findings", [])?;
    for link in &inventory.symlinks {
        tx.execute("INSERT OR REPLACE INTO symlink_findings(path,target,category) VALUES (?1,?2,?3)",
            params![link.path,link.target,link.category])?;
    }
    tx.execute("DELETE FROM inferred_links", [])?;
    tx.execute("DELETE FROM timestamp_assessments", [])?;
    tx.execute("DELETE FROM step_io", [])?;
    tx.execute("DELETE FROM steps", [])?;
    let (steps, links, warnings, scripts, parsed) = super::engine::extract_provenance(
        &tx, root, files, paths, cancel, inventory.warnings, &mut progress)?;
    let rebuilt_match_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM file_matches", [], |row| row.get(0)
    )?;
    if previous_match_count > 0 && rebuilt_match_count == 0 {
        anyhow::bail!(
            "Refusing to erase {previous_match_count} existing file matches: rescan rebuilt zero. Previous provenance preserved by transaction rollback."
        );
    }
    progress(format!(
        "Provenance: {rebuilt_match_count} file matches rebuilt (previously {previous_match_count})"
    ));
    tx.execute_batch("UPDATE file_matches SET decision_id = (
        SELECT old.decision_id FROM saved_match_decisions old
        WHERE old.source_id=file_matches.source_id AND old.filepath_id=file_matches.filepath_id
          AND old.artifact_id=file_matches.artifact_id AND old.location=file_matches.location
          AND old.direction=file_matches.direction
    ) WHERE EXISTS (
        SELECT 1 FROM saved_match_decisions old
        WHERE old.source_id=file_matches.source_id AND old.filepath_id=file_matches.filepath_id
          AND old.artifact_id=file_matches.artifact_id AND old.location=file_matches.location
          AND old.direction=file_matches.direction
    ); DROP TABLE saved_match_decisions;")?;
    super::classification::refresh(&tx, root, paths)?;
    infer_notebook_candidates(&tx)?;
    assess_timestamps(&tx, files)?;
    infer_script_chains(&tx)?;
    tx.execute("INSERT INTO scans(completed,files,steps,links) VALUES (1,?1,?2,?3)",params![files.len() as i64,steps as i64,links as i64])?;
    tx.commit()?;
    Ok(ScanResult { files:files.len(), steps, links, warnings, scripts, parsed })
}
