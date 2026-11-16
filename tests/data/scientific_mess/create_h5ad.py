import scanpy as sc
adata = sc.read_10x_mtx('.', var_names='gene_symbols')
adata.write_h5ad('original.h5ad')
