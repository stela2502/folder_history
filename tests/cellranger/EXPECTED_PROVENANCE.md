# Expected provenance (fixture only; do not run Cell Ranger)

The shell script defines 15 variables but only a subset enters the `cellranger count` command.

## Expected read dependencies
- `data/fastqs/sample_A/SampleA_S1_L001_R1_001.fastq.gz`
- `data/fastqs/sample_A/SampleA_S1_L001_R2_001.fastq.gz`
- `references/refdata-gex-GRCh38-2024-A/` (directory input, potentially its descendants)

## Expected outputs
- `results/sample_A_count/` and its descendants, including the matrix files, `molecule_info.h5`, and `metrics_summary.csv`.

## Forbidden associations
- Any files under `data/fastqs/sample_B/`
- Any files under `old/fastqs/`
- `references/obsolete_reference/`
- `config/feature_reference.csv`

The output directory is inferred from `cd "$OUTPUT_PARENT"` and `--id="$RUN_ID"`, not passed as a direct output filename. Real Cell Ranger would generate these files; this fixture merely simulates their existence. The FASTQ and gz files are deliberately tiny text placeholders, so no actual sequencing pipeline can run against them.
