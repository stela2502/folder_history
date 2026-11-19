#!/usr/bin/env bash
set -euo pipefail

# Intentional decoys: defined but not used in the count invocation.
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FASTQ_A="$PROJECT_ROOT/data/fastqs/sample_A"
FASTQ_B="$PROJECT_ROOT/data/fastqs/sample_B"
FASTQ_OLD="$PROJECT_ROOT/old/fastqs"
REFERENCE="$PROJECT_ROOT/references/refdata-gex-GRCh38-2024-A"
REFERENCE_OLD="$PROJECT_ROOT/references/obsolete_reference"
FEATURE_REF="$PROJECT_ROOT/config/feature_reference.csv"
SAMPLE_A="SampleA"
SAMPLE_B="SampleB"
RUN_ID="sample_A_count"
UNUSED_RUN_ID="obsolete_count"
CORES=8
MEMORY_GB=32
UNUSED_CHEMISTRY="SC3Pv3"
OUTPUT_PARENT="$PROJECT_ROOT/results"
UNUSED_OUTPUT="$PROJECT_ROOT/results/never_created"

# Only a subset of variables participates in the actual invocation.
mkdir -p "$OUTPUT_PARENT"
cd "$OUTPUT_PARENT"
cellranger count \
  --id="$RUN_ID" \
  --transcriptome="$REFERENCE" \
  --fastqs="$FASTQ_A" \
  --sample="$SAMPLE_A" \
  --localcores="$CORES" \
  --localmem="$MEMORY_GB"
