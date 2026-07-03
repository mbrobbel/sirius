-- Results-store schema (design stage). Target engine is a local DuckDB file;
-- the DDL is kept engine-portable.

-- One row per distinct system-spec snapshot.
CREATE TABLE environments (
  id BIGINT PRIMARY KEY,
  hostname TEXT NOT NULL,
  os TEXT,
  cpu_model TEXT,
  cpu_cores BIGINT,
  ram_bytes BIGINT,
  gpu_name TEXT,
  gpu_memory_bytes BIGINT,
  gpu_driver TEXT,
  cuda_version TEXT,
  disks_json TEXT, -- [{mount, total_bytes, free_bytes}]
  collected_at TEXT NOT NULL
);

-- One row per suite or bench invocation.
CREATE TABLE runs (
  id BIGINT PRIMARY KEY,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  environment_id BIGINT REFERENCES environments (id),
  commit_sha TEXT,
  branch TEXT,
  build_preset TEXT,
  engine TEXT NOT NULL, -- sirius | duckdb (baseline)
  engine_config_json TEXT, -- resolved engine config snapshot; secrets (object-store credentials etc.) must be redacted before storing
  suite TEXT, -- NULL for ad-hoc bench runs
  dataset_benchmark TEXT,
  scale_factor DOUBLE,
  data_format TEXT,
  data_compression TEXT,
  data_encoding TEXT,
  mode TEXT, -- cold | warm
  iterations BIGINT,
  notes TEXT
);

-- One row per (query, iteration).
CREATE TABLE results (
  id BIGINT PRIMARY KEY,
  run_id BIGINT NOT NULL REFERENCES runs (id),
  query TEXT NOT NULL,
  iteration BIGINT NOT NULL,
  runtime_ms DOUBLE,
  status TEXT NOT NULL, -- ok | error | timeout
  error TEXT,
  telemetry_path TEXT,
  UNIQUE (run_id, query, iteration)
);

-- Correctness of a run's results against expected results.
CREATE TABLE validations (
  id BIGINT PRIMARY KEY,
  run_id BIGINT NOT NULL REFERENCES runs (id),
  query TEXT NOT NULL,
  status TEXT NOT NULL, -- match | mismatch | error
  detail TEXT,
  UNIQUE (run_id, query)
);
