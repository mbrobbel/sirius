//! GPU-backed fragment coordinator: owns Sirius sessions on one dedicated thread.
//!
//! [`sirius::SiriusContext`] and [`sirius::StreamSession`] stay on this thread under one mutable
//! coordinator. Fragment RPC workers and asynchronous exchange tasks communicate with it using
//! channels carrying owned plans and Arrow batches. Sirius fragments therefore execute
//! back-to-back; neither the context nor a session is shared with a Tokio task.
//!
//! The coordinator inspects each [`sirius::SubstraitPlan`] before session creation and correlates
//! its opaque [`sirius::StreamId`] values with StarRocks metadata owned by the compute node.
//! Exchange tasks use [`crate::exchange::ExchangeTransport`]; today's implementation is a local
//! in-process mailbox, and a Nixl agent can replace that boundary later.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use arrow_array::RecordBatch;
use sirius::{SiriusContext, StreamId, SubstraitPlan};
use starrocks_plan_translator::{ExchangeInput, TranslatedPlan};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::info;

use crate::exchange::{
    ExchangeData, ExchangeTransport, FragmentExchange, InputExchange, LocalExchangeTransport,
    OutputExchange,
};
use crate::fragment_executor::{FragmentExecutor, FragmentResult};

/// One owned execution request handed to the coordinator.
struct ExecuteRequest {
    /// Serialized Substrait plan bytes.
    plan: Vec<u8>,
    /// Translation-side exchange inputs, ordered like their Substrait reads.
    translated_inputs: Vec<ExchangeInput>,
    /// StarRocks routing and partition semantics.
    exchange: FragmentExchange,
    /// Channel the coordinator sends the result (or a flattened error) back on.
    respond: Sender<Result<Vec<RecordBatch>, String>>,
}

/// A StarRocks exchange input correlated with one opaque Sirius stream id.
#[derive(Clone, Debug)]
struct BoundInput {
    stream_id: StreamId,
    metadata: InputExchange,
}

/// A StarRocks exchange output correlated with one opaque Sirius stream id.
#[derive(Clone, Debug)]
struct BoundOutput {
    stream_id: StreamId,
    metadata: OutputExchange,
}

/// Inspected plan waiting for an exchange input or immediate execution.
struct PreparedRequest {
    request: ExecuteRequest,
    plan: SubstraitPlan,
    input: Option<BoundInput>,
    output_stream_id: StreamId,
    output: Option<BoundOutput>,
}

/// Completion messages sent from Tokio exchange tasks to the context-owning coordinator.
enum CoordinatorEvent {
    InputReady {
        request_id: u64,
        result: Result<ExchangeData, String>,
    },
    OutputSent {
        request_id: u64,
        result: Result<(), String>,
    },
}

/// Output retained while an async transport task sends it.
struct PendingOutput {
    batches: Vec<RecordBatch>,
    respond: Sender<Result<Vec<RecordBatch>, String>>,
}

/// GPU-backed [`FragmentExecutor`] running plans on an embedded Sirius engine coordinator.
///
/// Dropping the handle closes the request channel, ends pending exchange work, and joins the
/// coordinator thread for ordered context teardown.
#[derive(Debug)]
pub struct SiriusEngine {
    /// Sender to the coordinator, held in an option so `Drop` can close it before joining.
    requests: Mutex<Option<UnboundedSender<ExecuteRequest>>>,
    /// Engine thread handle, taken and joined on drop.
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl SiriusEngine {
    /// Brings up the engine on a dedicated thread (fail-fast) and returns a handle.
    ///
    /// Blocks until the context is initialized — or bring-up fails — so a bad config or GPU
    /// failure surfaces here, before any RPC is served. `config` is the optional Sirius YAML path
    /// (built-in defaults when `None`).
    pub fn start(config: Option<PathBuf>) -> Result<Self, String> {
        let (request_tx, request_rx) = unbounded_channel::<ExecuteRequest>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let transport: Arc<dyn ExchangeTransport> = Arc::new(LocalExchangeTransport::default());
        let thread = std::thread::Builder::new()
            .name("sirius-engine".to_string())
            .spawn(move || engine_thread(config, request_rx, ready_tx, transport))
            .map_err(|err| format!("failed to spawn sirius-engine thread: {err}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                requests: Mutex::new(Some(request_tx)),
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(err)) => Err(err),
            Err(_) => Err("sirius-engine thread exited during bring-up".to_string()),
        }
    }
}

/// Engine-thread body: bring up the context, signal readiness, then serve requests until the
/// request channel closes. The context is dropped here, on this thread, when the loop ends.
fn engine_thread(
    config: Option<PathBuf>,
    requests: UnboundedReceiver<ExecuteRequest>,
    ready: Sender<Result<(), String>>,
    transport: Arc<dyn ExchangeTransport>,
) {
    let context = match build_context(config) {
        Ok(context) => context,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            let _ = ready.send(Err(format!(
                "failed to build Sirius coordinator runtime: {err}"
            )));
            return;
        }
    };
    // A send error means the caller is already gone; nothing to serve.
    if ready.send(Ok(())).is_err() {
        return;
    }
    info!("sirius-engine thread ready");
    runtime.block_on(run_coordinator(context, requests, transport));
    info!("sirius-engine thread shutting down");
}

/// Owns the only mutable context and advances exchange-dependent fragments when inputs arrive.
async fn run_coordinator(
    mut context: SiriusContext,
    mut requests: UnboundedReceiver<ExecuteRequest>,
    transport: Arc<dyn ExchangeTransport>,
) {
    let (event_tx, mut event_rx) = unbounded_channel();
    let mut next_request_id = 0_u64;
    let mut waiting_for_input = HashMap::<u64, PreparedRequest>::new();
    let mut waiting_for_output = HashMap::<u64, PendingOutput>::new();

    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else {
                    fail_pending(
                        waiting_for_input,
                        waiting_for_output,
                        "sirius-engine is shutting down",
                    );
                    break;
                };
                let request_id = next_request_id;
                next_request_id = next_request_id.wrapping_add(1);
                match inspect_request(request) {
                    Ok(prepared) => {
                        if let Some(input) = prepared.input.clone() {
                            waiting_for_input.insert(request_id, prepared);
                            let transport = transport.clone();
                            let event_tx = event_tx.clone();
                            tokio::spawn(async move {
                                tracing::debug!(
                                    stream_id = input.stream_id.get(),
                                    route = ?input.metadata.route,
                                    "waiting for local exchange input"
                                );
                                let result = transport.receive(input.metadata.route).await;
                                let _ = event_tx.send(CoordinatorEvent::InputReady {
                                    request_id,
                                    result,
                                });
                            });
                        } else {
                            execute_prepared(
                                request_id,
                                prepared,
                                None,
                                &mut context,
                                transport.clone(),
                                event_tx.clone(),
                                &mut waiting_for_output,
                            );
                        }
                    }
                    Err((respond, err)) => {
                        let _ = respond.send(Err(err));
                    }
                }
            }
            event = event_rx.recv() => {
                let Some(event) = event else {
                    continue;
                };
                match event {
                    CoordinatorEvent::InputReady { request_id, result } => {
                        let Some(prepared) = waiting_for_input.remove(&request_id) else {
                            continue;
                        };
                        match result {
                            Ok(data) => execute_prepared(
                                request_id,
                                prepared,
                                Some(data),
                                &mut context,
                                transport.clone(),
                                event_tx.clone(),
                                &mut waiting_for_output,
                            ),
                            Err(err) => {
                                let _ = prepared.request.respond.send(Err(err));
                            }
                        }
                    }
                    CoordinatorEvent::OutputSent { request_id, result } => {
                        let Some(pending) = waiting_for_output.remove(&request_id) else {
                            continue;
                        };
                        let response = result.map(|()| pending.batches);
                        let _ = pending.respond.send(response);
                    }
                }
            }
        }
    }
}

/// Decodes and inspects a Sirius plan, then binds opaque stream ids to CN metadata.
fn inspect_request(
    request: ExecuteRequest,
) -> Result<PreparedRequest, (Sender<Result<Vec<RecordBatch>, String>>, String)> {
    let respond = request.respond.clone();
    let plan = SubstraitPlan::decode(&request.plan).map_err(|err| {
        (
            respond.clone(),
            format!("failed to decode Substrait plan: {err}"),
        )
    })?;
    if request.translated_inputs.len() != request.exchange.inputs.len() {
        return Err((
            respond,
            format!(
                "translated plan has {} exchange inputs but execution metadata has {}",
                request.translated_inputs.len(),
                request.exchange.inputs.len()
            ),
        ));
    }
    for (translated, execution) in request
        .translated_inputs
        .iter()
        .zip(&request.exchange.inputs)
    {
        if translated.node_id != execution.route.node_id {
            return Err((
                respond,
                format!(
                    "translated exchange node {} was paired with StarRocks route node {}",
                    translated.node_id, execution.route.node_id
                ),
            ));
        }
    }

    let input = if request.exchange.inputs.is_empty() {
        None
    } else {
        if request.exchange.inputs.len() != 1 {
            return Err((
                respond,
                format!(
                    "temporary streaming compatibility supports one exchange input, got {}",
                    request.exchange.inputs.len()
                ),
            ));
        }
        if plan.input_streams().len() != request.exchange.inputs.len() {
            return Err((
                respond,
                format!(
                    "exchange fragment has {} StarRocks inputs but Sirius discovered {} input streams",
                    request.exchange.inputs.len(),
                    plan.input_streams().len()
                ),
            ));
        }
        Some(BoundInput {
            stream_id: plan.input_streams()[0],
            metadata: request.exchange.inputs[0].clone(),
        })
    };
    if plan.output_streams().len() != 1 {
        return Err((
            respond,
            format!(
                "temporary streaming compatibility supports one output stream, got {}",
                plan.output_streams().len()
            ),
        ));
    }
    let output_stream_id = plan.output_streams()[0];
    let output = request.exchange.output.clone().map(|metadata| BoundOutput {
        stream_id: output_stream_id,
        metadata,
    });
    Ok(PreparedRequest {
        request,
        plan,
        input,
        output_stream_id,
        output,
    })
}

/// Executes one fragment synchronously on the coordinator, then delegates exchange output.
fn execute_prepared(
    request_id: u64,
    prepared: PreparedRequest,
    input_data: Option<ExchangeData>,
    context: &mut SiriusContext,
    transport: Arc<dyn ExchangeTransport>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    waiting_for_output: &mut HashMap<u64, PendingOutput>,
) {
    let result = if let Some(input) = prepared.input.as_ref() {
        let input_data = input_data.expect("input-ready event supplies exchange data");
        if input_data.sender_id < 0 {
            Err("exchange sender id must be non-negative".to_string())
        } else {
            context
                .create_stream_session(prepared.plan)
                .map_err(|err| err.to_string())
                .and_then(|mut session| {
                    session
                        .push_batches_sync(input.stream_id, input_data.batches)
                        .and_then(|()| session.end_stream(input.stream_id))
                        .and_then(|()| session.pull_batches_sync(prepared.output_stream_id))
                        .map_err(|err| err.to_string())
                })
        }
    } else {
        context
            .execute_substrait(&prepared.plan.encode_to_vec())
            .map_err(|err| err.to_string())
    };

    let batches = match result {
        Ok(batches) => batches,
        Err(err) => {
            let _ = prepared.request.respond.send(Err(err));
            return;
        }
    };
    let Some(output) = prepared.output else {
        let _ = prepared.request.respond.send(Ok(batches));
        return;
    };

    // Only owned batches and opaque/CN metadata cross into the async task. The mutable session
    // ended above and the Sirius context remains exclusively with this coordinator.
    waiting_for_output.insert(
        request_id,
        PendingOutput {
            batches: batches.clone(),
            respond: prepared.request.respond,
        },
    );
    tokio::spawn(async move {
        let result = send_output(transport, output, batches).await;
        let _ = event_tx.send(CoordinatorEvent::OutputSent { request_id, result });
    });
}

/// Sends an unpartitioned output to every StarRocks destination.
async fn send_output(
    transport: Arc<dyn ExchangeTransport>,
    output: BoundOutput,
    batches: Vec<RecordBatch>,
) -> Result<(), String> {
    for route in output.metadata.routes {
        tracing::debug!(
            stream_id = output.stream_id.get(),
            ?route,
            "sending local exchange output"
        );
        transport
            .send(
                route,
                ExchangeData {
                    sender_id: output.metadata.sender_id,
                    batches: batches.clone(),
                },
            )
            .await?;
    }
    Ok(())
}

/// Replies to all blocked callers when the coordinator request channel closes.
fn fail_pending(
    waiting_for_input: HashMap<u64, PreparedRequest>,
    waiting_for_output: HashMap<u64, PendingOutput>,
    reason: &str,
) {
    for (_, pending) in waiting_for_input {
        let _ = pending.request.respond.send(Err(reason.to_string()));
    }
    for (_, pending) in waiting_for_output {
        let _ = pending.respond.send(Err(reason.to_string()));
    }
}

/// Brings up a [`SiriusContext`] from an optional config path (built-in defaults when `None`).
fn build_context(config: Option<PathBuf>) -> Result<SiriusContext, String> {
    let context = match config {
        Some(path) => SiriusContext::from_config_file(&path),
        None => SiriusContext::new(),
    }
    .map_err(|err| format!("failed to bring up Sirius engine: {err}"))?;
    info!("Sirius engine context created");
    Ok(context)
}

impl FragmentExecutor for SiriusEngine {
    fn execute(
        &self,
        translated: &TranslatedPlan,
        exchange: &FragmentExchange,
    ) -> Result<FragmentResult, String> {
        let (respond_tx, respond_rx) = channel();
        let request = ExecuteRequest {
            plan: translated.to_substrait_bytes(),
            translated_inputs: translated.exchange_inputs.clone(),
            exchange: exchange.clone(),
            respond: respond_tx,
        };
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .ok_or_else(|| "sirius-engine is shutting down".to_string())?
            .send(request)
            .map_err(|_| "sirius-engine thread is not running".to_string())?;
        let batches = respond_rx
            .recv()
            .map_err(|_| "sirius-engine thread dropped the response".to_string())??;
        Ok(FragmentResult::new(batches))
    }
}

impl Drop for SiriusEngine {
    fn drop(&mut self) {
        // Close the request channel so the engine thread's `recv()` returns and it drops the
        // context, then join for an ordered, complete teardown. The sender must drop before the
        // join or `recv()` would block forever.
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(thread) = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Array, ArrayRef, Int64Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;

    use super::*;

    /// Builds a single-file `local_files` parquet read plan with `names` as the root output
    /// names — the shape DuckDB's Substrait reader resolves to `parquet_scan(<path>)`.
    fn local_files_plan(path: &str, names: Vec<String>) -> TranslatedPlan {
        use substrait::proto::read_rel::local_files::FileOrFiles;
        use substrait::proto::read_rel::local_files::file_or_files::{
            FileFormat, ParquetReadOptions, PathType,
        };
        use substrait::proto::read_rel::{LocalFiles, ReadType};
        use substrait::proto::{Plan, PlanRel, ReadRel, Rel, RelRoot, plan_rel, rel};

        let read = Rel {
            rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
                read_type: Some(ReadType::LocalFiles(LocalFiles {
                    items: vec![FileOrFiles {
                        path_type: Some(PathType::UriFile(path.to_string())),
                        file_format: Some(FileFormat::Parquet(ParquetReadOptions {})),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }))),
        };
        let plan = Plan {
            relations: vec![PlanRel {
                rel_type: Some(plan_rel::RelType::Root(RelRoot {
                    input: Some(read),
                    names: names.clone(),
                })),
            }],
            ..Default::default()
        };
        TranslatedPlan {
            plan,
            output_names: names,
            exchange_inputs: Vec::new(),
        }
    }

    /// Like [`local_files_plan`] but declares a `base_schema` (names + types) on the read — the
    /// shape the translator emits for a `FILES()` scan. DuckDB's Substrait reader projects the
    /// parquet columns onto these names, so a pruned/reordered `base_schema` selects columns by
    /// name rather than by file position. `columns` is `(name, is_string)` in output order.
    fn local_files_plan_with_base_schema(path: &str, columns: &[(&str, bool)]) -> TranslatedPlan {
        use substrait::proto::read_rel::local_files::FileOrFiles;
        use substrait::proto::read_rel::local_files::file_or_files::{
            FileFormat, ParquetReadOptions, PathType,
        };
        use substrait::proto::read_rel::{LocalFiles, ReadType};
        use substrait::proto::{
            NamedStruct, Plan, PlanRel, ReadRel, Rel, RelRoot, Type, plan_rel, rel, r#type,
        };

        let names: Vec<String> = columns.iter().map(|(name, _)| name.to_string()).collect();
        let types: Vec<Type> = columns
            .iter()
            .map(|(_, is_string)| {
                let kind = if *is_string {
                    r#type::Kind::String(r#type::String {
                        type_variation_reference: 0,
                        nullability: r#type::Nullability::Nullable as i32,
                    })
                } else {
                    r#type::Kind::I64(r#type::I64 {
                        type_variation_reference: 0,
                        nullability: r#type::Nullability::Nullable as i32,
                    })
                };
                Type { kind: Some(kind) }
            })
            .collect();
        let read = Rel {
            rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
                base_schema: Some(NamedStruct {
                    names: names.clone(),
                    r#struct: Some(r#type::Struct {
                        types,
                        type_variation_reference: 0,
                        nullability: r#type::Nullability::Required as i32,
                    }),
                }),
                read_type: Some(ReadType::LocalFiles(LocalFiles {
                    items: vec![FileOrFiles {
                        path_type: Some(PathType::UriFile(path.to_string())),
                        file_format: Some(FileFormat::Parquet(ParquetReadOptions {})),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }))),
        };
        let plan = Plan {
            relations: vec![PlanRel {
                rel_type: Some(plan_rel::RelType::Root(RelRoot {
                    input: Some(read),
                    names: names.clone(),
                })),
            }],
            ..Default::default()
        };
        TranslatedPlan {
            plan,
            output_names: names,
            exchange_inputs: Vec::new(),
        }
    }

    #[test]
    fn inspection_binds_opaque_stream_id_to_cn_exchange_metadata() {
        use starrocks_thrift::partitions::TPartitionType;

        let mut translated = local_files_plan("unused.parquet", vec!["id".to_string()]);
        translated.exchange_inputs.push(ExchangeInput {
            node_id: 7,
            partition_type: Some(TPartitionType::UNPARTITIONED),
        });
        let route = crate::exchange::ExchangeRoute {
            fragment_instance_id: crate::result_store::FragmentInstanceId::from_halves(3, 4),
            node_id: 7,
        };
        let exchange = FragmentExchange {
            inputs: vec![InputExchange {
                route,
                expected_senders: 1,
            }],
            output: None,
        };
        let (respond, _response) = channel();

        let prepared = inspect_request(ExecuteRequest {
            plan: translated.to_substrait_bytes(),
            translated_inputs: translated.exchange_inputs,
            exchange,
            respond,
        })
        .unwrap();

        let input = prepared.input.unwrap();
        assert_eq!(input.stream_id, StreamId::new(0));
        assert_eq!(input.metadata.route, route);
        assert_eq!(prepared.output_stream_id, StreamId::new(0));
    }

    /// End-to-end: drive a `local_files` parquet plan through the engine actor and read the rows
    /// back. Exercises the dedicated-thread bring-up, the channel round-trip, and GPU execution.
    /// Requires a GPU and `LD_LIBRARY_PATH` to the built engine (like the `sirius` crate's context
    /// test); the parquet extension path is set from `SIRIUS_BUILD_DIR` (default mirrors sirius-sys).
    #[test]
    fn engine_executes_local_files_plan() {
        // Point the embedded DuckDB at the locally-built parquet extension so it can bind
        // `parquet_scan`. This is the only context-constructing test in the crate, so no other
        // thread reads the environment concurrently.
        if std::env::var_os("SIRIUS_DUCKDB_PARQUET_EXTENSION").is_none() {
            let manifest = env!("CARGO_MANIFEST_DIR");
            let build_dir = std::env::var("SIRIUS_BUILD_DIR")
                .unwrap_or_else(|_| format!("{manifest}/../../build/release"));
            let parquet = format!("{build_dir}/extension/parquet/parquet.duckdb_extension");
            // SAFETY: set before the engine thread brings up the context; no other thread reads it.
            unsafe { std::env::set_var("SIRIUS_DUCKDB_PARQUET_EXTENSION", parquet) };
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rows.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let ids: ArrayRef = Arc::new(Int64Array::from(vec![1, 2, 3]));
        let names: ArrayRef = Arc::new(StringArray::from(vec!["a", "b", "c"]));
        let batch = RecordBatch::try_new(schema.clone(), vec![ids, names]).unwrap();
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }

        let plan = local_files_plan(
            path.to_str().unwrap(),
            vec!["id".to_string(), "name".to_string()],
        );

        let engine = SiriusEngine::start(None).expect("bring up sirius engine");
        let result = engine
            .execute(&plan, &FragmentExchange::default())
            .expect("execute fragment on GPU");
        let total_rows: usize = result.batches.iter().map(RecordBatch::num_rows).sum();
        assert_eq!(total_rows, 3, "expected 3 rows from the parquet fixture");

        // A `base_schema` that prunes and reorders the file's columns must bind by name, not by
        // file position (exercises the Substrait reader's `local_files` projection). The fixture
        // file is [id, name, extra]; the plan asks for [name, id], so a positional bind would
        // return the wrong columns.
        let cols_path = dir.path().join("cols.parquet");
        let cols_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("extra", DataType::Int64, false),
        ]));
        let cols_batch = RecordBatch::try_new(
            cols_schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef,
                Arc::new(StringArray::from(vec!["a", "b", "c"])) as ArrayRef,
                Arc::new(Int64Array::from(vec![10, 20, 30])) as ArrayRef,
            ],
        )
        .unwrap();
        {
            let file = std::fs::File::create(&cols_path).unwrap();
            let mut writer = ArrowWriter::try_new(file, cols_schema, None).unwrap();
            writer.write(&cols_batch).unwrap();
            writer.close().unwrap();
        }

        let pruned = local_files_plan_with_base_schema(
            cols_path.to_str().unwrap(),
            &[("name", true), ("id", false)],
        );
        let result = engine
            .execute(&pruned, &FragmentExchange::default())
            .expect("execute pruned fragment on GPU");
        let batch = result
            .batches
            .iter()
            .find(|batch| batch.num_rows() > 0)
            .expect("a non-empty result batch");
        assert_eq!(batch.num_columns(), 2, "base_schema pruned to two columns");
        assert_eq!(batch.schema().field(0).name(), "name");
        assert_eq!(batch.schema().field(1).name(), "id");
        // Bound by name, not position: column 0 carries the strings, column 1 the ids.
        let name_col = batch
            .column(0)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("first output column is the utf8 name column");
        assert_eq!(name_col.value(0), "a");
        let id_col = batch
            .column(1)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("second output column is the int64 id column");
        assert_eq!(id_col.value(0), 1);
    }
}
