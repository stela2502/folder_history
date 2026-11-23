//! End-to-end core contract: real folder -> scanner -> SQLite -> wizard -> SQLite -> rescan.
//! No GUI, no manually inserted file matches, and no legacy relations/issues tables.
use folder_history::core::engine::{scan, schema};
use folder_history::core::history_db::{DbArchiveView, DbIssues, MatchDecision};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[test]
fn scan_review_save_reopen_rescan_without_legacy_tables() -> anyhow::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/scientific_mess");
    anyhow::ensure!(root.is_dir(), "Missing scientific_mess fixture");

    // A real on-disk SQLite database, isolated from the research fixture.
    let db_path = std::env::temp_dir().join(format!(
        "folder-history-full-workflow-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
    ));
    let result = (|| -> anyhow::Result<()> {
        let mut db = Connection::open(&db_path)?;
        schema(&db)?;
        for legacy in ["relations", "issues"] {
            let exists: i64 = db.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [legacy], |r| r.get(0)
            )?;
            anyhow::ensure!(exists == 0, "Legacy table {legacy} must not exist");
        }

        // This MUST exercise the real scanner. No direct record_match() calls.
        scan(&mut db, &root, false, &AtomicBool::new(false), |_| {})?;
        let paths: i64 = db.query_row("SELECT COUNT(*) FROM file_paths", [], |r| r.get(0))?;
        let matches: i64 = db.query_row("SELECT COUNT(*) FROM file_matches", [], |r| r.get(0))?;
        assert!(paths > 0, "scanner extracted no script expressions");
        assert!(matches > 0, "scanner persisted no script-to-file connections");

        // Archive loaders must use the current schema; the GUI and documentation
        // use this same projection. No legacy tables may be required.
        let (artifacts, relations, unresolved) = db.archive_view()?;
        assert!(!artifacts.is_empty());
        assert!(!relations.is_empty(), "archive view lost scanner provenance");
        assert!(!unresolved.is_empty(), "wizard review candidates not exposed");
        // Re-evaluation must preserve the same file_matches and decisions.
        db.refresh_cached_candidates()?;
        assert_eq!(db.query_row("SELECT COUNT(*) FROM file_matches", [], |r| r.get::<_, i64>(0))?, matches);

        // Verify real scientific provenance, not merely nonzero totals.
        let source_id: i64 = db.query_row(
            "SELECT id FROM artifacts WHERE path='run_mapping.sh'", [], |r| r.get(0)
        )?;
        let mut wizard = db.issues(source_id)?;
        let has_fastq = wizard.reads().flat_map(|g| &g.matches)
            .any(|m| m.artifact == "sample_A_R1.fastq.gz");
        assert!(has_fastq, "run_mapping.sh must read sample_A_R1.fastq.gz");
        assert!(wizard.writes().flat_map(|g| &g.matches).next().is_some(),
            "run_mapping.sh must have at least one produced file");

        // Simulate a scientist reviewing the actual candidates loaded from SQLite.
        let accepted_id = {
            let group = wizard.reads_mut()
                .find(|g| g.matches.iter().any(|m| m.artifact == "sample_A_R1.fastq.gz"))
                .expect("FASTQ expression must be exposed in the wizard");
            let item = group.matches.iter_mut()
                .find(|m| m.artifact == "sample_A_R1.fastq.gz").unwrap();
            item.decision_id = MatchDecision::Accepted as i64;
            item.role = Some("Raw experimental data".into());
            item.id
        };
        // Reject another independent candidate if available; all decisions remain reversible.
        let rejected_id = wizard.groups.iter_mut().flat_map(|g| &mut g.matches)
            .find(|m| m.id != accepted_id)
            .map(|m| { m.decision_id = MatchDecision::Rejected as i64; m.id });
        assert!(wizard.validate().is_empty(), "wizard validation failed: {:?}", wizard.validate());
        db.save_wizard(&wizard)?;
        drop(db);

        // Verify persistence after closing/reopening the actual SQLite file.
        let mut db = Connection::open(&db_path)?;
        let loaded = db.issues(source_id)?;
        let decision = |db: &Connection, id: i64| -> rusqlite::Result<i64> {
            db.query_row("SELECT decision_id FROM file_matches WHERE id=?1", params![id], |r| r.get(0))
        };
        assert_eq!(decision(&db, accepted_id)?, MatchDecision::Accepted as i64);
        if let Some(id) = rejected_id {
            assert_eq!(decision(&db, id)?, MatchDecision::Rejected as i64);
        }
        assert!(loaded.reads().flat_map(|g| &g.matches)
            .any(|m| m.id == accepted_id && m.role.as_deref() == Some("Raw experimental data")));

        // Re-run the production scan and verify no previously reviewed edge disappears.
        scan(&mut db, &root, false, &AtomicBool::new(false), |_| {})?;
        // Rescans rebuild match rows; their SQLite IDs are not stable identifiers.
        let after = db.issues(source_id)?;
        assert!(after.groups.iter().flat_map(|g| &g.matches)
            .any(|m| m.artifact == "sample_A_R1.fastq.gz" && m.decision_id == 1));
        if let Some(id) = rejected_id {
            let old = loaded.groups.iter().flat_map(|g| &g.matches)
                .find(|m| m.id == id).expect("rejected match before rescan");
            assert!(after.groups.iter().flat_map(|g| &g.matches)
                .any(|m| m.artifact == old.artifact && m.direction == old.direction
                    && m.decision_id == MatchDecision::Rejected as i64),
                "rejected decision lost during rescan");
        }
        assert!(after.reads().flat_map(|g| &g.matches)
            .any(|m| m.artifact == "sample_A_R1.fastq.gz" && m.decision_id == 1));
        Ok(())
    })();
    // Remove only the private test database, never any fixture or researcher data.
    let _ = std::fs::remove_file(&db_path);
    result
}
