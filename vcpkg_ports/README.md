# Overlay ports

The DLPack and FlatBuffers overlays preserve Sirius's dependency pins when
DuckDB merges extension manifests without carrying over their `overrides`.
Keep their versions aligned with `vcpkg.json`.

These ports come from [Microsoft vcpkg](https://github.com/microsoft/vcpkg),
with local formatting changes:

| Port | Version | Upstream port tree |
| --- | --- | --- |
| dlpack | 0.8 | `935f86ccd4d13dfc3534e81cc898026736249c06` |
| flatbuffers | 24.3.25 | `f8e85b45608b4005d8e8b4a96cfe11a5c2686e92` |

Their port recipes are covered by [VCPKG-LICENSE.txt](VCPKG-LICENSE.txt).
