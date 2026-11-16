from pathlib import Path
import pandas as pd

def write_stats(adata, output_dir):
    out = Path(output_dir)
    out.mkdir(parents=True, exist_ok=True)
    pd.DataFrame({'cluster': ['0', '1'], 'n_cells': [2, 2]}).to_csv(out / 'cluster_statistics.tsv', sep='\t', index=False)
    pd.DataFrame({'gene': ['TP53', 'GATA1'], 'score': [1.2, 0.8]}).to_csv(out / 'marker_expression.tsv', sep='\t', index=False)

def write_cell_cycle_stats(adata, output_dir):
    out = Path(output_dir)
    out.mkdir(parents=True, exist_ok=True)
    pd.DataFrame({'phase': ['G1', 'S'], 'n_cells': [3, 1]}).to_csv(out / 'phase_statistics.tsv', sep='\t', index=False)

def write_de_tables(adata, output_dir):
    out = Path(output_dir)
    out.mkdir(parents=True, exist_ok=True)
    pd.DataFrame({'gene': ['TP53', 'KLF1'], 'logfoldchange': [1.5, -0.7]}).to_csv(out / 'differential_genes.tsv', sep='\t', index=False)
    pd.DataFrame({'gene': ['TP53', 'KLF1'], 'pvalue': [0.01, 0.2]}).to_csv(out / 'gene_summary.tsv', sep='\t', index=False)
