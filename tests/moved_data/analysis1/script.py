import glob
import scanpy

ifiles = glob.glob('*/*.h5')
adatas = [scanpy.read_10x_h5(f) for f in ifiles]
