mod support;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use arrow_schema::DataType;
use sirius::SiriusContext;
use support::plans::{KeyType, Mode, ReceiverOperator, receiver_plan, sender_plan};
use support::{Row, collect_rows, multiset, read_parquet, sender_rows, write_parquet};

const STAGING_BYTES: usize = 1024 * 1024;
const PROCESS_DEADLINE: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const WORKER_ROOT: &str = "SIRIUS_EXCHANGE_TEST_ROOT";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Rows,
    AllEmpty,
    MissingSender,
    SchemaMismatch,
    LimitOne,
    LimitZero,
    FilterFalse,
    DropBeforeRun,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::AllEmpty => "empty",
            Self::MissingSender => "missing",
            Self::SchemaMismatch => "schema",
            Self::LimitOne => "limit-one",
            Self::LimitZero => "limit-zero",
            Self::FilterFalse => "filter-false",
            Self::DropBeforeRun => "drop-before-run",
        }
    }

    fn rounds(self) -> usize {
        match self {
            Self::Rows | Self::LimitOne | Self::LimitZero | Self::FilterFalse => 2,
            _ => 1,
        }
    }

    fn expects_error(self) -> bool {
        matches!(
            self,
            Self::MissingSender | Self::SchemaMismatch | Self::DropBeforeRun
        )
    }

    fn receiver_operator(self, round: usize) -> ReceiverOperator {
        match (self, round) {
            (Self::Rows, 1) => ReceiverOperator::NotNull,
            (Self::LimitOne, 0) => ReceiverOperator::Limit(1),
            (Self::LimitZero, 0) => ReceiverOperator::Limit(0),
            (Self::FilterFalse, 0) => ReceiverOperator::FilterFalse,
            _ => ReceiverOperator::Identity,
        }
    }
}

struct Worker {
    child: Child,
    log: PathBuf,
}

impl Worker {
    fn check(&mut self) {
        if let Some(status) = self.child.try_wait().expect("poll exchange worker") {
            assert!(
                status.success(),
                "worker exited with {status}:\n{}",
                self.log_tail()
            );
        }
    }

    fn log_tail(&self) -> String {
        let data = fs::read(&self.log).unwrap_or_default();
        String::from_utf8_lossy(&data[data.len().saturating_sub(16_384)..]).into_owned()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn publish(path: &Path, bytes: &[u8]) {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).expect("write coordinator message");
    fs::rename(temporary, path).expect("publish coordinator message");
}

fn wait_for(paths: &[PathBuf], workers: &mut [Worker]) {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while paths.iter().any(|path| !path.exists()) {
        for worker in workers.iter_mut() {
            worker.check();
        }
        assert!(
            Instant::now() < deadline,
            "exchange workers timed out waiting for {paths:?}:\n{}",
            workers
                .iter()
                .map(Worker::log_tail)
                .collect::<Vec<_>>()
                .join("\n")
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn worker_wait(path: &Path) {
    let deadline = Instant::now() + PROCESS_DEADLINE;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "coordinator timed out: {}",
            path.display()
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn peer_name(root: &Path, worker: usize) -> String {
    format!("{}-{worker}", root.file_name().unwrap().to_str().unwrap())
}

/// Invoked in separate processes by the coordinator so every engine owns its GPU state.
#[test]
#[ignore = "exchange worker; launched by exchange_end_to_end"]
fn exchange_worker() {
    let Some(root) = std::env::var_os(WORKER_ROOT).map(PathBuf::from) else {
        return;
    };
    let index: usize = std::env::var("SIRIUS_EXCHANGE_WORKER")
        .unwrap()
        .parse()
        .unwrap();
    let peers: usize = std::env::var("SIRIUS_EXCHANGE_PEERS")
        .unwrap()
        .parse()
        .unwrap();
    let receiver: bool = std::env::var("SIRIUS_EXCHANGE_RECEIVER")
        .unwrap()
        .parse()
        .unwrap();
    let scenario = match std::env::var("SIRIUS_EXCHANGE_SCENARIO").unwrap().as_str() {
        "rows" => Scenario::Rows,
        "empty" => Scenario::AllEmpty,
        "missing" => Scenario::MissingSender,
        "schema" => Scenario::SchemaMismatch,
        "limit-one" => Scenario::LimitOne,
        "limit-zero" => Scenario::LimitZero,
        "filter-false" => Scenario::FilterFalse,
        "drop-before-run" => Scenario::DropBeforeRun,
        value => panic!("unknown scenario {value}"),
    };
    let mut context = SiriusContext::from_config_file(&root.join("memory.yaml")).unwrap();
    context
        .enable_exchange(
            &peer_name(&root, index),
            STAGING_BYTES,
            Duration::from_secs(if scenario.expects_error() { 3 } else { 30 }),
        )
        .unwrap();
    publish(
        &root.join(format!("metadata-{index}")),
        &context.exchange_metadata().unwrap(),
    );
    worker_wait(&root.join("bootstrap"));
    for peer in 0..peers {
        if peer != index {
            let metadata = fs::read(root.join(format!("metadata-{peer}"))).unwrap();
            assert_eq!(
                context.add_exchange_peer(&metadata).unwrap(),
                peer_name(&root, peer)
            );
        }
    }
    for round in 0..scenario.rounds() {
        {
            let plan = fs::read(root.join(format!("plan-{round}-{index}"))).unwrap();
            let mut fragment = context.fragment(&plan).expect("build exchange fragment");
            publish(&root.join(format!("ready-{round}-{index}")), b"");
            worker_wait(&root.join(format!("start-{round}")));
            if scenario == Scenario::DropBeforeRun {
                drop(fragment);
                let retry = receiver_plan(
                    1001,
                    index as u64,
                    1,
                    ReceiverOperator::Identity,
                    KeyType::I64,
                );
                let error = match context.fragment(&retry) {
                    Ok(_) => panic!("abandoned exchange must require a new context"),
                    Err(error) => error.to_string(),
                };
                assert!(
                    error.contains("unusable") && error.contains("new context"),
                    "{error}"
                );
                publish(&root.join("expected-error"), error.as_bytes());
                publish(&root.join(format!("done-{round}-{index}")), b"");
                continue;
            }
            if scenario.expects_error() && receiver {
                let run_started = Instant::now();
                let error = fragment
                    .run()
                    .expect_err("invalid exchange must fail")
                    .to_string();
                let lower = error.to_ascii_lowercase();
                match scenario {
                    Scenario::MissingSender => {
                        assert!(
                            lower.contains("timeout") || lower.contains("timed out"),
                            "{error}"
                        );
                        assert!(
                            run_started.elapsed() >= Duration::from_secs(2),
                            "timeout started before run(): {error}"
                        );
                    }
                    Scenario::SchemaMismatch => assert!(
                        lower.contains("schema")
                            || lower.contains("type")
                            || lower.contains("mismatch"),
                        "{error}"
                    ),
                    _ => unreachable!(),
                }
                publish(&root.join("expected-error"), error.as_bytes());
            } else if scenario == Scenario::SchemaMismatch {
                // The sender may observe the receiver's rejection after its transfer finishes.
                let _ = fragment.run();
            } else {
                fragment.run().expect("execute exchange fragment");
                if receiver {
                    let output = fragment.result().expect("collect receiver output");
                    assert_eq!(output.schema.fields().len(), 2);
                    assert_eq!(output.schema.field(0).name(), "key");
                    assert_eq!(output.schema.field(1).name(), "payload");
                    assert_eq!(output.schema.field(0).data_type(), &DataType::Int64);
                    assert_eq!(output.schema.field(1).data_type(), &DataType::Utf8);
                    write_parquet(
                        &root.join(format!("result-{round}-{index}.parquet")),
                        &collect_rows(&output.batches),
                    );
                }
            }
            let repeated = fragment
                .run()
                .expect_err("a fragment cannot run again after success or failure")
                .to_string();
            assert!(repeated.contains("already run or failed"), "{repeated}");
        }
        publish(&root.join(format!("done-{round}-{index}")), b"");
    }
}

fn run_case(mode: Mode, scenario: Scenario) {
    let keep_artifacts = std::env::var_os("SIRIUS_EXCHANGE_KEEP_ARTIFACTS").is_some();
    let directory = tempfile::Builder::new()
        .prefix("sirius-exchange-")
        .disable_cleanup(keep_artifacts)
        .tempdir()
        .unwrap();
    let root = directory.path();
    if keep_artifacts {
        eprintln!(
            "keeping {mode:?}/{} artifacts in {}",
            scenario.name(),
            root.display()
        );
    }
    fs::write(
        root.join("memory.yaml"),
        "sirius:\n  topology:\n    num_gpus: 1\n  space:\n    gpu:\n      - device_id: 0\n        memory_capacity: 134217728\n    host:\n      - numa_id: 0\n        memory_capacity: 536870912\n",
    ).unwrap();
    let mut inputs = sender_rows();
    match scenario {
        Scenario::Rows => {
            // The packed string column exceeds a staging buffer and must cross in chunks.
            inputs[0].push((Some(42), Some("large payload ".repeat(STAGING_BYTES / 8))));
        }
        Scenario::AllEmpty => inputs.iter_mut().for_each(Vec::clear),
        Scenario::MissingSender | Scenario::DropBeforeRun => inputs.clear(),
        Scenario::SchemaMismatch => inputs.truncate(1),
        Scenario::LimitOne | Scenario::LimitZero | Scenario::FilterFalse => inputs.truncate(2),
    }
    let receivers = if mode == Mode::Gather { 1 } else { 2 };
    let senders = inputs.len();
    let count = receivers + senders;
    let receiver_peers = (0..receivers)
        .map(|index| peer_name(root, index))
        .collect::<Vec<_>>();
    for (sender, rows) in inputs.iter().take(senders).enumerate() {
        if !rows.is_empty() {
            write_parquet(&root.join(format!("input-{sender}.parquet")), rows);
        }
    }
    for round in 0..scenario.rounds() {
        let query = round as u64 + 1000;
        for receiver in 0..receivers {
            fs::write(
                root.join(format!("plan-{round}-{receiver}")),
                receiver_plan(
                    query,
                    receiver as u64,
                    senders.max(1) as u32,
                    scenario.receiver_operator(round),
                    if scenario == Scenario::SchemaMismatch {
                        KeyType::I32
                    } else {
                        KeyType::I64
                    },
                ),
            )
            .unwrap();
        }
        for (sender, rows) in inputs.iter().enumerate() {
            fs::write(
                root.join(format!("plan-{round}-{}", receivers + sender)),
                sender_plan(
                    &root.join(format!("input-{sender}.parquet")),
                    rows.is_empty(),
                    mode,
                    query,
                    sender as u32,
                    &receiver_peers,
                ),
            )
            .unwrap();
        }
    }
    let mut workers = Vec::new();
    for index in 0..count {
        let log = root.join(format!("worker-{index}.log"));
        let stdout = File::create(&log).unwrap();
        let stderr = stdout.try_clone().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "exchange_worker", "--ignored", "--nocapture"])
            .env(WORKER_ROOT, root)
            .env("SIRIUS_EXCHANGE_WORKER", index.to_string())
            .env("SIRIUS_EXCHANGE_PEERS", count.to_string())
            .env("SIRIUS_EXCHANGE_RECEIVER", (index < receivers).to_string())
            .env("SIRIUS_EXCHANGE_SCENARIO", scenario.name())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        workers.push(Worker {
            child: command.spawn().expect("spawn exchange worker"),
            log,
        });
    }
    wait_for(
        &(0..count)
            .map(|index| root.join(format!("metadata-{index}")))
            .collect::<Vec<_>>(),
        &mut workers,
    );
    publish(&root.join("bootstrap"), b"");
    for round in 0..scenario.rounds() {
        wait_for(
            &(0..count)
                .map(|index| root.join(format!("ready-{round}-{index}")))
                .collect::<Vec<_>>(),
            &mut workers,
        );
        if scenario == Scenario::MissingSender {
            // Build-to-run coordinator delay must not consume the three-second idle timeout.
            thread::sleep(Duration::from_secs(4));
        }
        publish(&root.join(format!("start-{round}")), b"");
        wait_for(
            &(0..count)
                .map(|index| root.join(format!("done-{round}-{index}")))
                .collect::<Vec<_>>(),
            &mut workers,
        );
        if scenario.expects_error() {
            assert!(root.join("expected-error").exists());
            continue;
        }
        let operation = scenario.receiver_operator(round);
        let mut expected: Vec<Row> = inputs
            .iter()
            .flatten()
            .filter(|row| operation != ReceiverOperator::NotNull || row.0.is_some())
            .cloned()
            .collect();
        if matches!(
            operation,
            ReceiverOperator::Limit(0) | ReceiverOperator::FilterFalse
        ) {
            expected.clear();
        }
        let outputs = (0..receivers)
            .map(|index| read_parquet(&root.join(format!("result-{round}-{index}.parquet"))))
            .collect::<Vec<_>>();
        if operation == ReceiverOperator::Limit(1) {
            let rows = outputs.iter().flatten().collect::<Vec<_>>();
            assert_eq!(rows.len(), 1, "LIMIT 1 must return exactly one row");
            assert!(
                expected.contains(rows[0]),
                "LIMIT 1 returned a row absent from its inputs"
            );
        } else if mode == Mode::Broadcast {
            for output in &outputs {
                assert!(
                    multiset(output.clone()) == multiset(expected.clone()),
                    "broadcast receiver changed the row multiset in round {round}"
                );
            }
        } else {
            assert!(
                multiset(outputs.iter().flatten().cloned()) == multiset(expected),
                "{mode:?} changed the row multiset in round {round}"
            );
        }
        if mode == Mode::Hash {
            let mut key_destinations = BTreeMap::new();
            for (destination, rows) in outputs.iter().enumerate() {
                for (key, _) in rows {
                    if let Some(previous) = key_destinations.insert(*key, destination) {
                        assert_eq!(previous, destination, "key {key:?} split between receivers");
                    }
                }
            }
        }
    }
    let deadline = Instant::now() + PROCESS_DEADLINE;
    for worker in &mut workers {
        loop {
            worker.check();
            if worker.child.try_wait().unwrap().is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "worker teardown timed out:\n{}",
                worker.log_tail()
            );
            thread::sleep(POLL_INTERVAL);
        }
    }
}

#[test]
#[ignore = "requires a GPU"]
fn exchange_end_to_end() {
    for mode in [Mode::Gather, Mode::Hash, Mode::Broadcast] {
        eprintln!("testing {mode:?} exchange, then a second query with a receiver filter");
        run_case(mode, Scenario::Rows);
    }
    eprintln!("testing all senders empty");
    run_case(Mode::Gather, Scenario::AllEmpty);
    eprintln!("testing LIMIT 1, followed by an unrestricted query");
    run_case(Mode::Gather, Scenario::LimitOne);
    eprintln!("testing LIMIT 0, followed by an unrestricted query");
    run_case(Mode::Gather, Scenario::LimitZero);
    eprintln!("testing constant false filter, followed by an unrestricted query");
    run_case(Mode::Gather, Scenario::FilterFalse);
    eprintln!("testing received schema mismatch");
    run_case(Mode::Gather, Scenario::SchemaMismatch);
    eprintln!("testing missing sender timeout");
    run_case(Mode::Gather, Scenario::MissingSender);
    eprintln!("testing fragment abandonment before run");
    run_case(Mode::Gather, Scenario::DropBeforeRun);
}
