import glob
import anndata

infiles = glob.glob('*/*.h5')
adatase = [ anndata.read_h5( f) for f in infiles ]
