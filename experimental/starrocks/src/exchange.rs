//! StarRocks exchange routing and the replaceable async transport boundary.
//!
//! Sirius sees only opaque stream identifiers. This module owns destination ids, sender counts,
//! and partitioning validation, while [`LocalExchangeTransport`] provides today's in-process data
//! path. A Nixl-backed implementation can replace the transport without changing engine sessions.

#![cfg_attr(not(feature = "sirius-engine"), allow(dead_code))]

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use arrow_array::RecordBatch;
use starrocks_plan_translator::ExchangeInput;
use starrocks_thrift::data_sinks::TDataSinkType;
use starrocks_thrift::internal_service::TExecPlanFragmentParams;
use starrocks_thrift::partitions::TPartitionType;
use tokio::sync::mpsc;

use crate::result_store::FragmentInstanceId;

/// Local routing key for one StarRocks exchange receiver.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ExchangeRoute {
    /// Destination fragment instance.
    pub(crate) fragment_instance_id: FragmentInstanceId,
    /// Destination `EXCHANGE_NODE`.
    pub(crate) node_id: i32,
}

/// Receiver-side semantics for one translated exchange read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InputExchange {
    /// Local destination used by the transport.
    pub(crate) route: ExchangeRoute,
    /// Number of upstream senders StarRocks expects for this receiver.
    pub(crate) expected_senders: i32,
}

/// Sender-side semantics for one fragment output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OutputExchange {
    /// Broadcast destinations for the supported unpartitioned path.
    pub(crate) routes: Vec<ExchangeRoute>,
    /// StarRocks sender id carried with the exchange payload.
    pub(crate) sender_id: i32,
}

/// Exchange metadata accompanying one translated Substrait plan.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FragmentExchange {
    /// Inputs ordered exactly like `TranslatedPlan::exchange_inputs`.
    pub(crate) inputs: Vec<InputExchange>,
    /// Stream output when this fragment has a `DATA_STREAM_SINK`.
    pub(crate) output: Option<OutputExchange>,
}

impl FragmentExchange {
    /// Extracts and validates the StarRocks semantics supported by the local compatibility path.
    pub(crate) fn from_fragment(
        params: &TExecPlanFragmentParams,
        translated_inputs: &[ExchangeInput],
    ) -> Result<Self, String> {
        let exec = params.params.as_ref();
        let inputs = translated_inputs
            .iter()
            .map(|input| {
                if !matches!(
                    input.partition_type,
                    None | Some(TPartitionType::UNPARTITIONED)
                ) {
                    return Err(format!(
                        "exchange node {} uses unsupported receiver partition {:?}",
                        input.node_id, input.partition_type
                    ));
                }
                let exec = exec.ok_or_else(|| {
                    format!(
                        "exchange node {} is missing TPlanFragmentExecParams",
                        input.node_id
                    )
                })?;
                if exec.enable_exchange_pass_through != Some(true) {
                    return Err(format!(
                        "exchange node {} is not marked for in-process pass-through",
                        input.node_id
                    ));
                }
                let expected_senders = *exec
                    .per_exch_num_senders
                    .get(&input.node_id)
                    .ok_or_else(|| {
                        format!(
                            "exchange node {} is missing per_exch_num_senders",
                            input.node_id
                        )
                    })?;
                if expected_senders != 1 {
                    return Err(format!(
                        "local streaming compatibility requires one sender for exchange node {}, got {expected_senders}",
                        input.node_id
                    ));
                }
                Ok(InputExchange {
                    route: ExchangeRoute {
                        fragment_instance_id: FragmentInstanceId::from(
                            &exec.fragment_instance_id,
                        ),
                        node_id: input.node_id,
                    },
                    expected_senders,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;

        let output =
            match params
                .fragment
                .as_ref()
                .and_then(|fragment| fragment.output_sink.as_ref())
            {
                Some(sink) if sink.type_ == TDataSinkType::DATA_STREAM_SINK => {
                    let stream = sink.stream_sink.as_ref().ok_or_else(|| {
                        "DATA_STREAM_SINK is missing its stream_sink payload".to_string()
                    })?;
                    if stream.output_partition.type_ != TPartitionType::UNPARTITIONED {
                        return Err(format!(
                            "local exchange transport does not support {:?} output partitioning",
                            stream.output_partition.type_
                        ));
                    }
                    if stream.is_merge == Some(true) {
                        return Err("merging data-stream sinks are not supported".to_string());
                    }
                    if stream
                        .output_columns
                        .as_ref()
                        .is_some_and(|columns| !columns.is_empty())
                    {
                        return Err("data-stream sink output column projection is not supported"
                            .to_string());
                    }
                    if stream.limit.is_some_and(|limit| limit >= 0) {
                        return Err("data-stream sink limits are not supported".to_string());
                    }
                    let exec = exec.ok_or_else(|| {
                        "DATA_STREAM_SINK is missing TPlanFragmentExecParams".to_string()
                    })?;
                    if exec.enable_exchange_pass_through != Some(true) {
                        return Err("DATA_STREAM_SINK is not marked for in-process pass-through"
                            .to_string());
                    }
                    let destinations = exec.destinations.as_ref().ok_or_else(|| {
                        "DATA_STREAM_SINK is missing fragment destinations".to_string()
                    })?;
                    if destinations.is_empty() {
                        return Err("DATA_STREAM_SINK has no fragment destinations".to_string());
                    }
                    let sender_id = exec.sender_id.unwrap_or(0);
                    if sender_id < 0 {
                        return Err(format!(
                            "DATA_STREAM_SINK has negative sender id {sender_id}"
                        ));
                    }
                    Some(OutputExchange {
                        routes: destinations
                            .iter()
                            .map(|destination| ExchangeRoute {
                                fragment_instance_id: FragmentInstanceId::from(
                                    &destination.fragment_instance_id,
                                ),
                                node_id: stream.dest_node_id,
                            })
                            .collect(),
                        sender_id,
                    })
                }
                _ => None,
            };

        Ok(Self { inputs, output })
    }
}

/// One complete sender payload for the temporary single-batch compatibility session.
#[derive(Clone, Debug)]
pub(crate) struct ExchangeData {
    /// StarRocks sender id.
    pub(crate) sender_id: i32,
    /// Owned Arrow output batches.
    pub(crate) batches: Vec<RecordBatch>,
}

/// Boxed future returned by an object-safe exchange transport.
type TransportFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Async exchange transport boundary; a Nixl agent can implement this interface later.
pub(crate) trait ExchangeTransport: std::fmt::Debug + Send + Sync {
    /// Sends one complete payload to a StarRocks receiver.
    fn send(&self, route: ExchangeRoute, data: ExchangeData) -> TransportFuture<'_, ()>;

    /// Receives one complete payload for a StarRocks receiver.
    fn receive(&self, route: ExchangeRoute) -> TransportFuture<'_, ExchangeData>;
}

/// Process-local bounded mailboxes used until the Nixl transport is available.
#[derive(Debug, Default)]
pub(crate) struct LocalExchangeTransport {
    mailboxes: Mutex<HashMap<ExchangeRoute, Mailbox>>,
}

#[derive(Debug)]
struct Mailbox {
    sender: mpsc::Sender<ExchangeData>,
    receiver: Option<mpsc::Receiver<ExchangeData>>,
}

impl LocalExchangeTransport {
    fn sender(&self, route: ExchangeRoute) -> mpsc::Sender<ExchangeData> {
        let mut mailboxes = self
            .mailboxes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mailboxes
            .entry(route)
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(1);
                Mailbox {
                    sender,
                    receiver: Some(receiver),
                }
            })
            .sender
            .clone()
    }

    fn receiver(&self, route: ExchangeRoute) -> Result<mpsc::Receiver<ExchangeData>, String> {
        let mut mailboxes = self
            .mailboxes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mailboxes
            .entry(route)
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(1);
                Mailbox {
                    sender,
                    receiver: Some(receiver),
                }
            })
            .receiver
            .take()
            .ok_or_else(|| format!("exchange receiver {route:?} is already registered"))
    }
}

impl ExchangeTransport for LocalExchangeTransport {
    fn send(&self, route: ExchangeRoute, data: ExchangeData) -> TransportFuture<'_, ()> {
        let sender = self.sender(route);
        Box::pin(async move {
            sender
                .send(data)
                .await
                .map_err(|_| format!("exchange receiver {route:?} stopped"))
        })
    }

    fn receive(&self, route: ExchangeRoute) -> TransportFuture<'_, ExchangeData> {
        let receiver = self.receiver(route);
        Box::pin(async move {
            receiver?
                .recv()
                .await
                .ok_or_else(|| format!("exchange sender {route:?} stopped"))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{ArrayRef, Int64Array};
    use arrow_schema::{DataType, Field, Schema};

    use super::*;

    fn route() -> ExchangeRoute {
        ExchangeRoute {
            fragment_instance_id: FragmentInstanceId::from_halves(1, 2),
            node_id: 7,
        }
    }

    fn data(value: i64) -> ExchangeData {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let values: ArrayRef = Arc::new(Int64Array::from(vec![value]));
        ExchangeData {
            sender_id: 3,
            batches: vec![RecordBatch::try_new(schema, vec![values]).unwrap()],
        }
    }

    #[tokio::test]
    async fn local_transport_buffers_send_before_receive() {
        let transport = LocalExchangeTransport::default();
        transport.send(route(), data(11)).await.unwrap();
        let received = transport.receive(route()).await.unwrap();
        assert_eq!(received.sender_id, 3);
        assert_eq!(received.batches[0].num_rows(), 1);
    }

    #[tokio::test]
    async fn local_transport_wakes_receive_before_send() {
        let transport = Arc::new(LocalExchangeTransport::default());
        let receiver = {
            let transport = transport.clone();
            tokio::spawn(async move { transport.receive(route()).await })
        };
        tokio::task::yield_now().await;
        transport.send(route(), data(17)).await.unwrap();
        assert_eq!(receiver.await.unwrap().unwrap().sender_id, 3);
    }
}
