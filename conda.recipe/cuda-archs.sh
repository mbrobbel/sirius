# Sourced by the build scripts; mirrors the CUDAARCHS values in pixi.toml.
case "${cuda_version}" in
  12) cuda_archs="75-real;80-real;86-real;89-real;90a-real;100-real;120-real" ;;
  13) cuda_archs="75-real;80-real;86-real;89-real;90a-real;100f-real;120f-real" ;;
  *) echo "unsupported CUDA version: ${cuda_version}" >&2; exit 1 ;;
esac
