## Getting started

These releases contain a CUDA-free `sirius` loader and two GPU backends,
`sirius_cuda12` and `sirius_cuda13`. The loader selects a backend from the NVIDIA
driver's supported CUDA version. Download all three from the same release.

**Requirements**: Linux glibc on amd64 or arm64 SBSA, a supported NVIDIA GPU, and
an NVIDIA driver supporting CUDA 12 or newer. Jetson/Tegra is not supported.
See [driver and GPU coverage](https://github.com/{{REPO}}/blob/main/sirius-duckdb/README.md#cuda-selecting-core-extensions)
for the architecture lists and driver qualification limits.

### 1. Install DuckDB {{DUCKDB_VERSION}}

The host must match the exact DuckDB version used to build the extensions:

```bash
curl https://install.duckdb.org | DUCKDB_VERSION={{DUCKDB_VERSION_BARE}} sh
```

### 2. Download all three extensions for your platform

{{DOWNLOAD_LINKS}}

Extract them with their canonical extension names. For amd64 (use `linux_arm64`
instead on arm64):

```bash
for extension in sirius sirius_cuda12 sirius_cuda13; do
  gunzip -c "${extension}-linux_amd64.duckdb_extension.gz" > "${extension}.duckdb_extension"
done
```

### 3. Install and load

GitHub release binaries are unsigned; they are separate from DuckDB's signed
core repository. Start DuckDB with unsigned extensions enabled:

```bash
duckdb -unsigned
```

For client APIs, allow unsigned extensions at connection time, for example
`duckdb.connect(config={"allow_unsigned_extensions": "true"})` in Python.
Unsigned extensions execute native code with the host process's privileges.

Install the two backends and the loader locally. `FORCE INSTALL` also replaces an
older direct Sirius extension or backend:

```sql
FORCE INSTALL './sirius_cuda12.duckdb_extension';
FORCE INSTALL './sirius_cuda13.duckdb_extension';
FORCE INSTALL './sirius.duckdb_extension';
LOAD sirius;

CREATE TABLE numbers AS SELECT range AS id FROM range(100000);
SELECT SUM(id) FROM numbers;
```

Supported queries run on the GPU; unsupported operators fall back to CPU.
