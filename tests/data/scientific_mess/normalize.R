library(zellkonverter)
sce <- readH5AD('original.h5ad')
writeH5AD(sce, 'r_processed.h5ad')
