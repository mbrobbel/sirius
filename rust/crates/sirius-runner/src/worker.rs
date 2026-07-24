use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    assets::{DataFormat, Engine},
    expected_cache::{ExpectedResult, ResultRow, ResultSchema},
    hashing,
    model::{PinPolicy, ReferenceIdentity},
    process,
    progress::Reporter,
};

const WORKER_PROTOCOL_VERSION: u32 = 2;
const WORKER_SOURCE: &str = include_str!("worker.py");
const TRIAL_EXIT_GRACE_S: u64 = 10;

pub(crate) fn python_executable() -> String {
    std::env::var("SIRIUS_RUNNER_PYTHON").unwrap_or_else(|_| "python".to_owned())
}

pub struct Worker {
    python: String,
    script: PathBuf,
    logs: PathBuf,
    repo_root: PathBuf,
}

impl Worker {
    pub fn materialize(
        bundle_dir: &Path,
        repo_root: &Path,
        reporter: &mut impl Reporter,
    ) -> anyhow::Result<Self> {
        let logs = bundle_dir.join("logs");
        fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
        let script = logs.join("worker.py");
        fs::write(&script, WORKER_SOURCE)
            .with_context(|| format!("writing embedded worker {}", script.display()))?;
        reporter.detail(&format!(
            "Materialized DuckDB worker at {}",
            script.display()
        ))?;
        Ok(Self {
            python: python_executable(),
            script,
            logs,
            repo_root: repo_root.to_path_buf(),
        })
    }

    pub fn reference_identity(
        &self,
        reporter: &mut impl Reporter,
    ) -> anyhow::Result<ReferenceIdentity> {
        reporter.status("Identifying the exact DuckDB reference runtime")?;
        let response: IdentityResponse = self.invoke(
            "identity",
            &IdentityRequest {
                schema_version: WORKER_PROTOCOL_VERSION,
                operation: "identity",
            },
            Duration::from_secs(120),
            None,
            reporter,
        )?;
        ensure!(
            response.schema_version == WORKER_PROTOCOL_VERSION,
            "worker returned identity protocol {}, expected {}",
            response.schema_version,
            WORKER_PROTOCOL_VERSION
        );
        ensure!(
            response.duckdb_threads > 0,
            "worker returned an invalid DuckDB thread count of zero"
        );
        ensure!(
            response.preserve_insertion_order,
            "worker must enable DuckDB preserve_insertion_order"
        );
        reporter.detail(&format!(
            "DuckDB {} ({}, {} threads, preserve insertion order)",
            response.duckdb_version,
            short_id(&response.module_sha256),
            response.duckdb_threads,
        ))?;
        let python_executable_sha256 = hashing::file_with_progress(
            &response.python_executable,
            "Python executable identity",
            reporter,
        )?;
        Ok(ReferenceIdentity {
            duckdb_version: response.duckdb_version,
            duckdb_threads: response.duckdb_threads,
            preserve_insertion_order: response.preserve_insertion_order,
            module_path: response.module_path,
            module_sha256: response.module_sha256,
            python_version: response.python_version,
            python_executable: response.python_executable,
            python_executable_sha256,
            worker_sha256: hashing::bytes(WORKER_SOURCE.as_bytes()),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn trial(
        &self,
        engine: Engine,
        query: &str,
        sql: &str,
        format: DataFormat,
        data_path: &Path,
        extension: Option<&Path>,
        config: Option<&Path>,
        reference_identity: &ReferenceIdentity,
        pin: PinPolicy,
        warmups: u32,
        iterations: u32,
        timeout_s: u64,
        reporter: &mut impl Reporter,
    ) -> anyhow::Result<TrialOutput> {
        let engine_name = match engine {
            Engine::Duckdb => "duckdb",
            Engine::Sirius => "sirius",
            Engine::Both => bail!("worker trials require one concrete engine"),
        };
        let extension_path = extension.map(Path::to_path_buf);
        if engine == Engine::Sirius && extension_path.is_none() {
            bail!("a Sirius trial requires an extension path");
        }
        ensure!(
            reference_identity.duckdb_threads > 0,
            "DuckDB thread count must be greater than zero"
        );
        ensure!(
            reference_identity.preserve_insertion_order,
            "DuckDB preserve_insertion_order must be enabled"
        );
        let source_format = match format {
            DataFormat::Parquet => "parquet",
            DataFormat::Duckdb => "duckdb",
        };
        let pin = match (engine, pin) {
            (Engine::Duckdb, _) | (_, PinPolicy::None) => "none",
            (Engine::Sirius, PinPolicy::Gpu) => "gpu",
            (Engine::Sirius, PinPolicy::Host) => "host",
            (Engine::Both, _) => unreachable!("concrete engine was checked above"),
        };
        let request = TrialRequest {
            schema_version: WORKER_PROTOCOL_VERSION,
            operation: "trial",
            engine: engine_name,
            query,
            sql,
            source: SourceRequest {
                format: source_format,
                path: data_path,
            },
            extension_path,
            repo_root: &self.repo_root,
            duckdb_threads: reference_identity.duckdb_threads,
            preserve_insertion_order: reference_identity.preserve_insertion_order,
            pin,
            warmups,
            iterations,
            timeout_s,
        };
        let maximum = trial_process_timeout(timeout_s);
        let log_dir = self.logs.join(engine_name).join(query);
        fs::create_dir_all(&log_dir).with_context(|| format!("creating {}", log_dir.display()))?;
        let response: TrialResponse = self.invoke(
            &format!("{engine_name}-{query}"),
            &request,
            maximum,
            Some(TrialEnvironment {
                config,
                sirius_log_dir: &log_dir,
                worker_log: &log_dir.join("worker.log"),
            }),
            reporter,
        )?;
        ensure!(
            response.schema_version == WORKER_PROTOCOL_VERSION,
            "worker returned trial protocol {}, expected {}",
            response.schema_version,
            WORKER_PROTOCOL_VERSION
        );
        ensure!(
            response.engine == engine_name && response.query == query,
            "worker response does not match requested {engine_name}/{query}"
        );
        ensure!(
            response.duckdb_threads == reference_identity.duckdb_threads,
            "worker reported {} DuckDB threads, expected {}",
            response.duckdb_threads,
            reference_identity.duckdb_threads
        );
        ensure!(
            response.preserve_insertion_order == reference_identity.preserve_insertion_order,
            "worker returned inconsistent DuckDB preserve_insertion_order"
        );
        ensure!(
            response.measurements.len() == iterations as usize,
            "worker returned {} measurements, expected {iterations}",
            response.measurements.len()
        );
        ensure!(
            response.warmups == warmups,
            "worker reported {} warm-ups, expected {warmups}",
            response.warmups
        );
        ensure!(
            response.pin == pin,
            "worker reported pin policy `{}`, expected `{pin}`",
            response.pin
        );
        ensure!(
            response.pin_setup_succeeded == (engine == Engine::Sirius && pin != "none"),
            "worker returned inconsistent pin-setup evidence"
        );
        if engine == Engine::Sirius {
            ensure!(
                response.fallback_disabled,
                "Sirius worker did not disable DuckDB fallback"
            );
        }

        reporter.status(&format!("Validating {engine_name}/{query} worker results"))?;
        let measurements = response
            .measurements
            .into_iter()
            .map(|measurement| {
                ensure!(
                    measurement.result.row_count == measurement.result.rows.len() as u64,
                    "worker result row count does not match its rows"
                );
                let result =
                    ExpectedResult::new(measurement.result.schema, measurement.result.rows)?;
                Ok(TrialMeasurement {
                    iteration: measurement.iteration,
                    duration_ns: measurement.duration_ns,
                    result,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(TrialOutput {
            engine: response.engine,
            query: response.query,
            fallback_disabled: response.fallback_disabled,
            pin: response.pin,
            pin_setup_succeeded: response.pin_setup_succeeded,
            duckdb_threads: response.duckdb_threads,
            preserve_insertion_order: response.preserve_insertion_order,
            measurements,
        })
    }

    fn invoke<Request: Serialize, Response: DeserializeOwned>(
        &self,
        label: &str,
        request: &Request,
        timeout: Duration,
        environment: Option<TrialEnvironment<'_>>,
        reporter: &mut impl Reporter,
    ) -> anyhow::Result<Response> {
        let request_path = self.logs.join(format!("{label}-request.json"));
        let response_path = self.logs.join(format!("{label}-response.json"));
        write_json(&request_path, request)?;

        let mut command = Command::new(&self.python);
        command
            .current_dir(&self.repo_root)
            .arg(&self.script)
            .arg(&request_path)
            .arg(&response_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        if let Some(environment) = environment {
            if let Some(config) = environment.config {
                command.env("SIRIUS_CONFIG_FILE", config);
            }
            command
                .env("SIRIUS_LOG_DIR", environment.sirius_log_dir)
                .env("SIRIUS_WORKER_LOG", environment.worker_log);
        }
        let result = process::run_with_timeout(
            &mut command,
            format!("Executing {label} worker"),
            Some(timeout),
            reporter,
        );
        if let Err(error) = result {
            if let Ok(failure) = read_json::<FailureResponse>(&response_path)
                && let Some(message) = failure.error
            {
                return Err(error).context(message);
            }
            return Err(error);
        }
        reporter.status(&format!("Reading and validating {label} worker output"))?;
        read_json(&response_path)
    }
}

#[derive(Serialize)]
struct IdentityRequest {
    schema_version: u32,
    operation: &'static str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityResponse {
    schema_version: u32,
    duckdb_version: String,
    duckdb_threads: u32,
    preserve_insertion_order: bool,
    module_path: PathBuf,
    module_sha256: String,
    python_version: String,
    python_executable: PathBuf,
}

#[derive(Serialize)]
struct TrialRequest<'a> {
    schema_version: u32,
    operation: &'static str,
    engine: &'a str,
    query: &'a str,
    sql: &'a str,
    source: SourceRequest<'a>,
    extension_path: Option<PathBuf>,
    repo_root: &'a Path,
    duckdb_threads: u32,
    preserve_insertion_order: bool,
    pin: &'a str,
    warmups: u32,
    iterations: u32,
    timeout_s: u64,
}

#[derive(Serialize)]
struct SourceRequest<'a> {
    format: &'a str,
    path: &'a Path,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrialResponse {
    schema_version: u32,
    engine: String,
    query: String,
    duckdb_threads: u32,
    preserve_insertion_order: bool,
    fallback_disabled: bool,
    pin: String,
    pin_setup_succeeded: bool,
    warmups: u32,
    measurements: Vec<RawMeasurement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeasurement {
    iteration: u32,
    duration_ns: u64,
    result: RawResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResult {
    schema: ResultSchema,
    rows: Vec<ResultRow>,
    row_count: u64,
}

#[derive(Debug)]
pub struct TrialOutput {
    pub engine: String,
    pub query: String,
    pub fallback_disabled: bool,
    pub pin: String,
    pub pin_setup_succeeded: bool,
    pub duckdb_threads: u32,
    pub preserve_insertion_order: bool,
    pub measurements: Vec<TrialMeasurement>,
}

#[derive(Debug)]
pub struct TrialMeasurement {
    pub iteration: u32,
    pub duration_ns: u64,
    pub result: ExpectedResult,
}

struct TrialEnvironment<'a> {
    config: Option<&'a Path>,
    sirius_log_dir: &'a Path,
    worker_log: &'a Path,
}

#[derive(Deserialize)]
struct FailureResponse {
    error: Option<String>,
}

fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let output = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    serde_json::to_writer_pretty(output, value)
        .with_context(|| format!("writing {}", path.display()))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> anyhow::Result<T> {
    let input = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    serde_json::from_reader(input).with_context(|| format!("reading {}", path.display()))
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(12)]
}

fn trial_process_timeout(timeout_s: u64) -> Duration {
    Duration::from_secs(timeout_s.saturating_add(TRIAL_EXIT_GRACE_S))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, time::Instant};

    use super::*;
    use crate::expected_cache::{ResultMapEntry, ResultValue};
    use crate::progress::Progress;

    fn duckdb_python_available() -> bool {
        Command::new(python_executable())
            .args(["-c", "import duckdb"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn create_empty_database(path: &Path) {
        let status = Command::new(python_executable())
            .args([
                "-c",
                "import duckdb, sys; duckdb.connect(sys.argv[1]).close()",
            ])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn embedded_worker_reports_the_installed_duckdb_identity() {
        if !duckdb_python_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let mut progress = Progress::with_writer(Vec::new(), 0);
        let worker = Worker::materialize(temp.path(), temp.path(), &mut progress).unwrap();
        let identity = worker.reference_identity(&mut progress).unwrap();
        assert!(!identity.duckdb_version.is_empty());
        assert!(identity.duckdb_threads > 0);
        assert!(identity.preserve_insertion_order);
        assert_eq!(identity.module_sha256.len(), 64);
        let recorded = serde_json::to_value(&identity).unwrap();
        assert_eq!(
            recorded["duckdb_threads"],
            serde_json::json!(identity.duckdb_threads)
        );
        assert_eq!(
            recorded["preserve_insertion_order"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn trial_process_timeout_is_one_deadline_plus_exit_grace() {
        assert_eq!(trial_process_timeout(1), Duration::from_secs(11));
        assert_eq!(trial_process_timeout(300), Duration::from_secs(310));
    }

    #[test]
    fn worker_encodes_representative_duckdb_values_without_loss() {
        if !duckdb_python_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("types.duckdb");
        create_empty_database(&database);
        let mut progress = Progress::with_writer(Vec::new(), 0);
        let worker = Worker::materialize(temp.path(), temp.path(), &mut progress).unwrap();
        let reference = worker.reference_identity(&mut progress).unwrap();
        let sql = r#"
            SELECT
                NULL::INTEGER AS null_value,
                true AS boolean_value,
                '-170141183460469231731687303715884105728'::HUGEINT AS integer_value,
                '340282366920938463463374607431768211455'::UHUGEINT AS unsigned_value,
                '9999999999999999999999999999999999999999999999999999999999999999'
                    ::BIGNUM AS bignum_value,
                '-0.0'::DOUBLE AS float_value,
                12.3400::DECIMAL(10,4) AS decimal_value,
                'hello'::VARCHAR AS text_value,
                from_hex('00ff80') AS blob_value,
                DATE '2026-07-24' AS date_value,
                TIME '12:34:56.123456' AS time_value,
                TIME WITH TIME ZONE '12:34:56.123456+02' AS time_tz_value,
                TIMESTAMP '2026-07-24 12:34:56.123456' AS timestamp_value,
                UUID '01234567-89ab-cdef-0123-456789abcdef' AS uuid_value,
                json('{"b":2,"a":[1,null]}') AS json_value,
                [1, NULL, 2] AS list_value,
                {'label': 'hello', 'answer': 42} AS struct_value,
                map([1, 2], ['a', 'b']) AS map_value,
                [1, 2]::INTEGER[2] AS array_value,
                bitstring('101', 3) AS bit_value,
                'x'::ENUM('x', 'y') AS enum_value
        "#;
        let output = worker
            .trial(
                Engine::Duckdb,
                "q-types",
                sql,
                DataFormat::Duckdb,
                &database,
                None,
                None,
                &reference,
                PinPolicy::None,
                0,
                1,
                30,
                &mut progress,
            )
            .unwrap();

        assert_eq!(output.duckdb_threads, reference.duckdb_threads);
        assert!(output.preserve_insertion_order);
        let values = &output.measurements[0].result.rows[0].0;
        assert_eq!(
            values,
            &vec![
                ResultValue::Null,
                ResultValue::Boolean(true),
                ResultValue::Integer("-170141183460469231731687303715884105728".into()),
                ResultValue::UnsignedInteger("340282366920938463463374607431768211455".into()),
                ResultValue::Integer(
                    "9999999999999999999999999999999999999999999999999999999999999999".into(),
                ),
                ResultValue::Float("-0.0".into()),
                ResultValue::Decimal("12.3400".into()),
                ResultValue::Text("hello".into()),
                ResultValue::Blob("00ff80".into()),
                ResultValue::Date("2026-07-24".into()),
                ResultValue::Time("12:34:56.123456".into()),
                ResultValue::Time("12:34:56.123456+02:00".into()),
                ResultValue::Timestamp("2026-07-24T12:34:56.123456".into()),
                ResultValue::Uuid("01234567-89ab-cdef-0123-456789abcdef".into()),
                ResultValue::Json(r#"{"b":2,"a":[1,null]}"#.into()),
                ResultValue::List(vec![
                    ResultValue::Integer("1".into()),
                    ResultValue::Null,
                    ResultValue::Integer("2".into()),
                ]),
                ResultValue::Struct(BTreeMap::from([
                    ("answer".into(), ResultValue::Integer("42".into())),
                    ("label".into(), ResultValue::Text("hello".into())),
                ])),
                ResultValue::Map(vec![
                    ResultMapEntry {
                        key: ResultValue::Integer("1".into()),
                        value: ResultValue::Text("a".into()),
                    },
                    ResultMapEntry {
                        key: ResultValue::Integer("2".into()),
                        value: ResultValue::Text("b".into()),
                    },
                ]),
                ResultValue::List(vec![
                    ResultValue::Integer("1".into()),
                    ResultValue::Integer("2".into()),
                ]),
                ResultValue::Text("101".into()),
                ResultValue::Text("x".into()),
            ]
        );
    }

    #[test]
    fn worker_rejects_values_the_python_api_cannot_represent_losslessly() {
        if !duckdb_python_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("unsupported.duckdb");
        create_empty_database(&database);
        let mut progress = Progress::with_writer(Vec::new(), 0);
        let worker = Worker::materialize(temp.path(), temp.path(), &mut progress).unwrap();
        let reference = worker.reference_identity(&mut progress).unwrap();

        let error = worker
            .trial(
                Engine::Duckdb,
                "q-interval",
                "SELECT INTERVAL '1 month'",
                DataFormat::Duckdb,
                &database,
                None,
                None,
                &reference,
                PinPolicy::None,
                0,
                1,
                30,
                &mut progress,
            )
            .unwrap_err();

        assert!(
            format!("{error:#}").contains(
                "DuckDB INTERVAL is unsupported because the Python API collapses months and days"
            ),
            "{error:#}"
        );
    }

    #[test]
    fn watchdog_interrupts_a_long_native_duckdb_query() {
        if !duckdb_python_available() {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("timeout.duckdb");
        create_empty_database(&database);
        let mut progress = Progress::with_writer(Vec::new(), 0);
        let worker = Worker::materialize(temp.path(), temp.path(), &mut progress).unwrap();
        let reference = worker.reference_identity(&mut progress).unwrap();
        let started = Instant::now();

        let error = worker
            .trial(
                Engine::Duckdb,
                "q-timeout",
                "SELECT sum(sqrt(i::DOUBLE)) FROM range(1000000000000) AS values(i)",
                DataFormat::Duckdb,
                &database,
                None,
                None,
                &reference,
                PinPolicy::None,
                0,
                1,
                1,
                &mut progress,
            )
            .unwrap_err();

        assert!(
            format!("{error:#}").contains("duckdb/q-timeout trial exceeded its 1s timeout"),
            "{error:#}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "watchdog took {:?} to stop DuckDB",
            started.elapsed()
        );
    }
}
