#!/usr/bin/env bash
set -euo pipefail
# This fixture models a real mapping command; it is not executed by the generator.
STAR --readFilesIn sample_A_R1.fastq.gz sample_A_R2.fastq.gz \
  --readFilesCommand zcat --genomeDir /references/GRCh38_STAR \
  --outFileNamePrefix mapped_
# A downstream count-export step writes these files into the current directory:
cp mapped_matrix.mtx.gz matrix.mtx.gz
cp mapped_features.tsv.gz features.tsv.gz
cp mapped_barcodes.tsv.gz barcodes.tsv.gz
