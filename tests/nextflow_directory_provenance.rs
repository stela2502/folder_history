//! Regression: two Nextflow invocations must use the directory values in force
//! at each invocation, not the final values or an unexpanded `$OUTDIR`.
use folder_history::core::engine::{scan, schema};
use folder_history::core::history_db::DbIssues;
use rusqlite::Connection;
use std::{fs, path::Path, sync::atomic::AtomicBool};

const SCRIPT: &str = r#"#!/bin/bash
#SBATCH --account=csens2024-3-2
#SBATCH --time=20:00:00
module purge
ml Nextflow/24.10.0
NEXTFLOW_PACKAGE="/scale/gr01/shared/nf-core/wf-single-cell"
NF_CONFIG="/home/stefanl/shared/nf-core/cosmos_sens_grp1_NFv24_wf-singlecell_targeted.config"
INDEX_10X="/scale/gr01/shared/common/genome/10Xindexes/cellranger/6.0/refdata-gex-GRCh38-2020-A/"
RESUME="-resume"
PATH_FASTQ="./TP53-capt_2/"
OUTDIR="./TP53-capt_2_outdir_targeted/"
SAMPLES="./samples_2.tsv"
nextflow run $NEXTFLOW_PACKAGE --fastq $PATH_FASTQ --samples "$SAMPLES" $RESUME \
        -profile singularity -c $NF_CONFIG \
        --kit '3prime:v3' --expected_cells 10000 \
        --call_variants --out_dir $OUTDIR --threads 20 --ref_genome_dir $INDEX_10X
PATH_FASTQ="./TP53-capt_4/"
OUTDIR="./TP53-capt_4_outdir_targeted/"
SAMPLES="./samples_4.tsv"
nextflow run $NEXTFLOW_PACKAGE --fastq $PATH_FASTQ --samples "$SAMPLES" $RESUME \
        -profile singularity -c $NF_CONFIG \
        --kit '3prime:v3' --expected_cells 10000 \
        --call_variants --out_dir $OUTDIR --threads 20 --ref_genome_dir $INDEX_10X
"#;

fn make_file(root: &Path, relative: &str) -> anyhow::Result<()> {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, b"fixture")?;
    Ok(())
}

#[test]
fn nextflow_two_invocations_connect_correct_input_and_output_directories() -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join(format!(
        "folder-history-nextflow-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&tmp)?;
    let result = (|| -> anyhow::Result<()> {
        fs::write(tmp.join("nextflow_10X_ONT.sh"), SCRIPT)?;
        for capture in [2, 4] {
            fs::write(tmp.join(format!("samples_{capture}.tsv")), format!("sample\ncap{capture}\n"))?;
            make_file(&tmp, &format!("TP53-capt_{capture}/reads.fastq.gz"))?;
            let output = format!("TP53-capt_{capture}_outdir_targeted");
            fs::create_dir_all(tmp.join(&output))?;
            if capture == 2 {
                make_file(&tmp, &format!("{output}/results/counts.tsv"))?;
            }
        }
        let mut db = Connection::open_in_memory()?;
        schema(&db)?;
        scan(&mut db, &tmp, false, &AtomicBool::new(false), |_| {})?;
        let empty_output_kind: String = db.query_row(
            "SELECT kind FROM artifacts WHERE path='TP53-capt_4_outdir_targeted'",
            [], |row| row.get(0),
        )?;
        assert_eq!(empty_output_kind, "directory");

        let source_id: i64 = db.query_row(
            "SELECT id FROM artifacts WHERE path='nextflow_10X_ONT.sh'", [], |r| r.get(0)
        )?;
        let wizard = db.issues(source_id)?;
        let matches: Vec<_> = wizard.groups.iter().flat_map(|g| g.matches.iter()
            .map(move |m| (g.direction.as_str(), m.artifact.as_str()))).collect();
        for capture in [2, 4] {
            let input = format!("TP53-capt_{capture}");
            let output = format!("TP53-capt_{capture}_outdir_targeted");
            let samples = format!("samples_{capture}.tsv");
            assert!(matches.contains(&("reads", input.as_str())),
                "missing input directory {input}; detected: {matches:?}");
            assert!(matches.contains(&("writes_directory", output.as_str())),
                "missing output directory {output}; detected: {matches:?}");
            assert!(matches.contains(&("reads", samples.as_str())),
                "missing sample sheet {samples}; detected: {matches:?}");
        }
        assert!(matches.iter().all(|(direction, _)| *direction != "writes"),
            "directory descendants should be hidden in the wizard: {matches:?}");
        Ok(())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}
