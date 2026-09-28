use crate::{
    config::{Checkpoint, Environment, Settings},
    corpus::{ConnectionId, Execution},
    result,
};
use anyhow::{Context, Result, bail, ensure};
use arrow::compute::concat_batches;
use duckdb::{Config, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum Operation {
    Statement,
    Query(Execution),
}

impl Operation {
    fn settings(self) -> (bool, bool) {
        match self {
            Self::Statement => (false, false),
            Self::Query(execution) => (true, execution == Execution::AllowFallback),
        }
    }

    fn preamble(self, sirius: bool, checkpoint: Checkpoint) -> String {
        let (query, fallback) = self.settings();
        let mut sql = String::new();
        if sirius {
            sql.push_str("SET gpu_execution = false;\n");
        }
        if query && matches!(checkpoint, Checkpoint::Automatic) {
            sql.push_str("CHECKPOINT;\n");
        }
        if sirius {
            sql.push_str(&format!(
                "SET gpu_execution = {query}; SET enable_duckdb_fallback = {fallback};\n"
            ));
        }
        sql
    }

    pub fn reproduction(self, sql: &str, sirius: bool, checkpoint: Checkpoint) -> String {
        format!("{}{sql};\n", self.preamble(sirius, checkpoint))
    }
}

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub connection: ConnectionId,
    pub checkpoint: Checkpoint,
    pub settings: Settings,
    pub sql: String,
    pub operation: Operation,
    pub result: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ready { version: String },
    Rows { logical_types: Vec<String> },
    Statement { count: u64 },
    Error { message: String, harness: bool },
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn connect(extension: Option<&Path>, install: bool) -> Result<Connection> {
    connect_at(extension, install, None)
}

fn connect_at(
    extension: Option<&Path>,
    install: bool,
    database: Option<&Path>,
) -> Result<Connection> {
    let extension_cache = std::env::var("SIRIUS_SQLTEST_EXTENSION_DIR")
        .unwrap_or_else(|_| "/tmp/sirius-sqltest-extensions".into());
    fs::create_dir_all(&extension_cache)?;
    let config = Config::default()
        .allow_unsigned_extensions()?
        .with("extension_directory", &extension_cache)?
        .with(
            "autoinstall_known_extensions",
            if install { "true" } else { "false" },
        )?
        .with("autoload_known_extensions", "false")?;
    let con = match database {
        Some(path) => Connection::open_with_flags(path, config)?,
        None => Connection::open_in_memory_with_flags(config)?,
    };
    con.execute_batch(
        "SET TimeZone = 'UTC'; SET threads = 2; SET preserve_insertion_order = true;",
    )?;
    if let Some(extension) = extension {
        con.execute_batch(&format!(
            "LOAD {}; SET gpu_execution = false; SET enable_duckdb_fallback = false;",
            quote(&extension.to_string_lossy())
        ))?;
    }
    Ok(con)
}

pub fn run(socket: &Path, extension: Option<&Path>) -> Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    let database = match connect_at(extension, false, Some(Path::new("worker.duckdb"))) {
        Ok(con) => con,
        Err(e) => {
            send(
                &mut stream,
                &Response::Error {
                    message: format!("initialize worker: {e:#}"),
                    harness: true,
                },
            )?;
            return Ok(());
        }
    };
    let version: String = database.query_row("SELECT version()", [], |r| r.get(0))?;
    let optimizers: String =
        database.query_row("SELECT current_setting('disabled_optimizers')", [], |r| {
            r.get(0)
        })?;
    let mut sessions = BTreeMap::new();
    send(&mut stream, &Response::Ready { version })?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let request: Request = serde_json::from_str(&line)?;
        let con = match sessions.entry(request.connection.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let opened = (|| -> Result<Connection> {
                    let con = database.try_clone()?;
                    con.execute_batch("SET TimeZone = 'UTC'; SET threads = 2; SET preserve_insertion_order = true;")?;
                    con.execute_batch(&format!(
                        "SET disabled_optimizers = {};",
                        quote(&optimizers)
                    ))?;
                    con.execute_batch(
                        &Operation::Statement.preamble(extension.is_some(), request.checkpoint),
                    )?;
                    for sql in request.settings.sql() {
                        con.execute_batch(&sql)?;
                    }
                    Ok(con)
                })();
                match opened {
                    Ok(con) => entry.insert(con),
                    Err(error) => {
                        send(
                            &mut stream,
                            &Response::Error {
                                message: format!(
                                    "initialize connection {}: {error:#}",
                                    request.connection.label()
                                ),
                                harness: true,
                            },
                        )?;
                        continue;
                    }
                }
            }
        };
        let (is_query, allow_fallback) = request.operation.settings();
        con.execute_batch(
            &request
                .operation
                .preamble(extension.is_some(), request.checkpoint),
        )?;
        if extension.is_some() {
            let settings: (bool, bool) = con.query_row("SELECT current_setting('gpu_execution'), current_setting('enable_duckdb_fallback')", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
            ensure!(
                settings == (is_query, allow_fallback),
                "execution settings were not applied"
            );
        }
        let response = if is_query {
            query(con, &request)
        } else {
            match con.execute(&request.sql, []) {
                Ok(count) => Response::Statement {
                    count: count as u64,
                },
                Err(e) => Response::Error {
                    message: e.to_string(),
                    harness: false,
                },
            }
        };
        send(&mut stream, &response)?;
    }
    Ok(())
}

fn query(con: &Connection, request: &Request) -> Response {
    let mut stmt = match con.prepare(&request.sql) {
        Ok(s) => s,
        Err(e) => {
            return Response::Error {
                message: e.to_string(),
                harness: false,
            };
        }
    };
    let result = match stmt.query_arrow([]) {
        Ok(r) => r,
        Err(e) => {
            return Response::Error {
                message: e.to_string(),
                harness: false,
            };
        }
    };
    let schema = result.get_schema();
    let batches: Vec<_> = result.collect();
    let logical_types: Vec<_> = (0..stmt.column_count())
        .map(|i| {
            format!(
                "{:?}/{:?}",
                stmt.column_logical_type(i),
                stmt.column_type(i)
            )
        })
        .collect();
    match concat_batches(&schema, &batches)
        .map_err(anyhow::Error::from)
        .and_then(|b| result::write(&request.result, &b))
    {
        Ok(()) => Response::Rows { logical_types },
        Err(e) => Response::Error {
            message: format!("Arrow result: {e:#}"),
            harness: true,
        },
    }
}

fn send(stream: &mut UnixStream, response: &Response) -> Result<()> {
    serde_json::to_writer(&mut *stream, response)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

pub struct Worker {
    pub checkpoint: Checkpoint,
    settings: Settings,
    child: Child,
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    _socket_dir: tempfile::TempDir,
    pub version: String,
}

impl Worker {
    pub fn start(
        directory: &Path,
        extension: Option<&Path>,
        config: Option<&Path>,
        environment: &Environment,
        checkpoint: Checkpoint,
        settings: &Settings,
    ) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let socket_dir = tempfile::tempdir()?;
        let socket = socket_dir.path().join("worker.sock");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("worker")
            .arg("--socket")
            .arg(&socket)
            .current_dir(directory)
            .stdin(Stdio::null())
            .stdout(File::create(directory.join("stdout.log"))?)
            .stderr(File::create(directory.join("stderr.log"))?);
        command.envs(environment.iter());
        if let Some(extension) = extension {
            command.arg("--extension").arg(extension);
            command.env_remove("SIRIUS_DISABLE");
        } else {
            command.env("SIRIUS_DISABLE", "1");
        }
        if let Some(config) = config {
            command.env("SIRIUS_CONFIG_FILE", config);
        }
        let mut child = command.spawn()?;
        let start = Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e.into());
                }
            }
            if let Some(status) = child.try_wait()? {
                bail!(
                    "worker exited during startup ({status}); see {}",
                    directory.join("stderr.log").display()
                );
            }
            if start.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("worker startup timed out");
            }
            thread::sleep(Duration::from_millis(10));
        };
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut worker = Self {
            checkpoint,
            settings: settings.clone(),
            child,
            stream,
            reader,
            _socket_dir: socket_dir,
            version: String::new(),
        };
        match worker.receive()? {
            Response::Ready { version } => worker.version = version,
            response => bail!("worker initialization failed: {response:?}"),
        }
        Ok(worker)
    }

    fn receive(&mut self) -> Result<Response> {
        let mut line = String::new();
        let size = self
            .reader
            .read_line(&mut line)
            .context("worker timeout or transport failure")?;
        ensure!(size > 0, "worker crashed or closed its connection");
        Ok(serde_json::from_str(&line)?)
    }

    pub fn execute(
        &mut self,
        sql: &str,
        connection: &ConnectionId,
        operation: Operation,
        result: &Path,
        timeout: u64,
    ) -> Result<Response> {
        self.stream
            .set_read_timeout(Some(Duration::from_secs(timeout)))?;
        serde_json::to_writer(
            &mut self.stream,
            &Request {
                connection: connection.clone(),
                checkpoint: self.checkpoint,
                settings: self.settings.clone(),
                sql: sql.into(),
                operation,
                result: result.into(),
            },
        )?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()?;
        self.receive()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Let DuckDB and CUDA release their resources before the next worker starts.
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
