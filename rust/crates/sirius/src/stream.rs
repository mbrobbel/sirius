//! Temporary synchronous compatibility surface for streaming execution.
//!
//! The public shape mirrors the intended streaming API, but the implementation
//! deliberately supports one input stream, one pushed batch, and one output
//! batch. The input batch is encoded as an in-memory Substrait `VirtualTable`
//! and substituted for the plan's single `ReadRel`, then the existing eager
//! Substrait entry point executes the rewritten plan once when the input stream
//! ends.

use std::fmt;

use arrow_array::temporal_conversions::as_datetime;
use arrow_array::types::{
    TimestampMicrosecondType, TimestampMillisecondType, TimestampNanosecondType,
    TimestampSecondType,
};
use arrow_array::{
    Array, BooleanArray, Date32Array, Decimal128Array, Float32Array, Float64Array, Int8Array,
    Int16Array, Int32Array, Int64Array, LargeStringArray, RecordBatch, StringArray,
    StringViewArray, TimestampMicrosecondArray, TimestampMillisecondArray,
    TimestampNanosecondArray, TimestampSecondArray,
};
use arrow_schema::{DataType, TimeUnit};
use prost::Message;
use substrait::proto::expression::{self, literal, nested};
use substrait::proto::read_rel::{ReadType, VirtualTable};
use substrait::proto::{
    Expression, NamedStruct, Plan as ProtoPlan, Rel, Type, plan_rel, rel, r#type,
};

use crate::{SiriusContext, SiriusError};

/// Identifies one input or output stream declared by a [`StreamSession`]'s plan.
///
/// Stream identifiers are discovered when the session is created rather than
/// configured independently by the caller. The compatibility implementation
/// assigns ordinal `0` to its sole input and sole output; native streaming plans
/// can preserve the identifiers carried by their source and sink relations.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StreamId(u64);

impl StreamId {
    /// Creates a stream identifier from its wire-level integer value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the wire-level integer value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for StreamId {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

/// An owned, decoded Substrait plan and its externally connected streams.
///
/// The stream identifiers are opaque correlation keys. An embedding runtime can
/// inspect them before creating a [`StreamSession`] and match them with
/// engine-external transport metadata retained during plan translation. Sirius
/// does not interpret or retain that transport metadata.
///
/// Until native streaming relations carry explicit stream metadata, inputs are
/// inferred from `ReadRel`s and outputs from top-level plan relations. Their
/// identifiers are stable, direction-local ordinals in traversal order.
#[derive(Clone, Debug)]
pub struct SubstraitPlan {
    plan: ProtoPlan,
    input_streams: Vec<StreamId>,
    output_streams: Vec<StreamId>,
}

impl SubstraitPlan {
    /// Decodes a protobuf-encoded plan and discovers its stream topology.
    pub fn decode(encoded: &[u8]) -> Result<Self, prost::DecodeError> {
        ProtoPlan::decode(encoded).map(Self::from_proto)
    }

    /// Wraps an already-decoded protobuf plan and discovers its stream topology.
    pub fn from_proto(mut plan: ProtoPlan) -> Self {
        let (input_streams, output_streams) = discover_plan_streams(&mut plan);
        Self {
            plan,
            input_streams,
            output_streams,
        }
    }

    /// Returns the streams feeding the plan's source relations.
    pub fn input_streams(&self) -> &[StreamId] {
        &self.input_streams
    }

    /// Returns the streams emitted by the plan's sink relations.
    ///
    /// A partitioned native sink can expose one entry per output partition.
    pub fn output_streams(&self) -> &[StreamId] {
        &self.output_streams
    }

    /// Returns the decoded protobuf plan.
    pub fn as_proto(&self) -> &ProtoPlan {
        &self.plan
    }

    /// Encodes the wrapped plan as protobuf.
    pub fn encode_to_vec(&self) -> Vec<u8> {
        self.plan.encode_to_vec()
    }
}

impl From<ProtoPlan> for SubstraitPlan {
    fn from(plan: ProtoPlan) -> Self {
        Self::from_proto(plan)
    }
}

/// A synchronous, single-batch streaming compatibility session.
///
/// The session borrows its [`SiriusContext`] mutably, preventing another query
/// from using that context until the session is dropped. The current lifecycle
/// is:
///
/// 1. push exactly one batch with [`Self::push_batch`] or
///    [`Self::push_batches_sync`];
/// 2. call [`Self::end_stream`] for the input stream, which executes the plan
///    once through [`SiriusContext::execute_substrait`];
/// 3. pull the sole result with [`Self::pull_batch_sync`] or
///    [`Self::pull_batches_sync`].
///
/// The Substrait plan must contain exactly one `ReadRel`. At execution time that
/// read is replaced with an in-memory virtual table over the pushed batch.
/// Multiple input reads, multiple pushed batches, zero or multiple result
/// batches, and unsupported Arrow input types are rejected until the native
/// streaming source/sink operators are ready.
pub struct StreamSession<'context> {
    context: &'context mut SiriusContext,
    plan: ProtoPlan,
    input_streams: Vec<StreamId>,
    output_streams: Vec<StreamId>,
    state: CompatibilityState,
}

impl SiriusContext {
    /// Creates a streaming session from an inspected Substrait plan.
    ///
    /// The plan's opaque stream identifiers can be correlated with
    /// engine-external transport metadata before the plan is moved into this
    /// method. This validates that its topology fits the temporary synchronous
    /// adapter. No GPU work starts until [`StreamSession::end_stream`] is
    /// called.
    pub fn create_stream_session(
        &mut self,
        plan: SubstraitPlan,
    ) -> Result<StreamSession<'_>, StreamSessionError> {
        validate_compatibility_streams(&plan)?;
        let SubstraitPlan {
            plan,
            input_streams,
            output_streams,
        } = plan;
        Ok(StreamSession {
            context: self,
            plan,
            input_streams,
            output_streams,
            state: CompatibilityState::default(),
        })
    }
}

impl StreamSession<'_> {
    /// Returns the input streams discovered from the plan's source relations.
    ///
    /// The compatibility implementation returns a one-element slice. Native
    /// streaming plans may expose multiple source streams.
    pub fn input_streams(&self) -> &[StreamId] {
        &self.input_streams
    }

    /// Returns the output streams discovered from the plan's sink relations.
    ///
    /// The compatibility implementation returns a one-element slice. A native
    /// partitioned sink may expose one stream per output partition.
    pub fn output_streams(&self) -> &[StreamId] {
        &self.output_streams
    }

    /// Pushes the session's sole input batch.
    ///
    /// A second call is rejected. The batch is retained until
    /// [`Self::end_stream`] adapts it into the plan's single input read.
    pub fn push_batch(
        &mut self,
        stream: StreamId,
        batch: RecordBatch,
    ) -> Result<(), StreamSessionError> {
        self.require_input_stream(stream)?;
        self.state.push_batch(batch)
    }

    /// Synchronously pushes a collection of input batches.
    ///
    /// The API accepts a collection for compatibility with future streaming
    /// callers, but the temporary adapter requires that it contain exactly one
    /// batch.
    pub fn push_batches_sync<I>(
        &mut self,
        stream: StreamId,
        batches: I,
    ) -> Result<(), StreamSessionError>
    where
        I: IntoIterator<Item = RecordBatch>,
    {
        self.require_input_stream(stream)?;
        self.state.push_batches_sync(batches)
    }

    /// Ends the input stream and executes the adapted Substrait plan once.
    ///
    /// This is synchronous: it returns only after the existing eager Substrait
    /// API has completed and its single result batch is ready to pull.
    pub fn end_stream(&mut self, stream: StreamId) -> Result<(), StreamSessionError> {
        self.require_input_stream(stream)?;
        let input = self.state.begin_execution()?;
        let result = execute_compatibility_plan(self.context, &self.plan, input);
        match result {
            Ok(batches) => self.state.finish_execution(batches),
            Err(err) => {
                self.state.fail_execution();
                Err(err)
            }
        }
    }

    /// Pulls the next output batch synchronously.
    ///
    /// The first successful pull returns `Some(batch)` and the next returns
    /// `None`. Pulling before [`Self::end_stream`] is rejected.
    pub fn pull_batch_sync(
        &mut self,
        stream: StreamId,
    ) -> Result<Option<RecordBatch>, StreamSessionError> {
        self.require_output_stream(stream)?;
        self.state.pull_batch_sync()
    }

    /// Drains all currently available output batches synchronously.
    ///
    /// For the compatibility implementation this returns a vector containing
    /// exactly one batch when called after [`Self::end_stream`] and before any
    /// other pull.
    pub fn pull_batches_sync(
        &mut self,
        stream: StreamId,
    ) -> Result<Vec<RecordBatch>, StreamSessionError> {
        self.require_output_stream(stream)?;
        self.state.pull_batches_sync()
    }

    fn require_input_stream(&self, actual: StreamId) -> Result<(), StreamSessionError> {
        if self.input_streams.contains(&actual) {
            Ok(())
        } else {
            Err(StreamSessionError::UnknownInputStream { actual })
        }
    }

    fn require_output_stream(&self, actual: StreamId) -> Result<(), StreamSessionError> {
        if self.output_streams.contains(&actual) {
            Ok(())
        } else {
            Err(StreamSessionError::UnknownOutputStream { actual })
        }
    }
}

/// Error returned by the synchronous streaming compatibility API.
#[derive(Debug)]
pub enum StreamSessionError {
    /// The temporary adapter requires exactly one input `ReadRel`.
    UnsupportedInputReadCount { actual: usize },
    /// The temporary adapter requires exactly one top-level output relation.
    UnsupportedOutputRelationCount { actual: usize },
    /// An operation addressed a stream not declared as an input by the plan.
    UnknownInputStream { actual: StreamId },
    /// An operation addressed a stream not declared as an output by the plan.
    UnknownOutputStream { actual: StreamId },
    /// The temporary adapter was given a batch collection other than length one.
    UnsupportedInputBatchCount { actual: usize },
    /// The sole input batch has already been pushed.
    InputAlreadyPushed,
    /// The input stream has already ended.
    InputAlreadyEnded,
    /// End-of-stream was requested before the input batch was pushed.
    MissingInputBatch,
    /// Output was pulled before the input stream ended.
    InputNotEnded,
    /// The existing eager execution returned a number of Arrow batches the
    /// temporary adapter cannot expose as one streaming result.
    UnsupportedOutputBatchCount { actual: usize },
    /// The pushed batch width does not match the plan input schema.
    InputColumnCountMismatch { plan: usize, batch: usize },
    /// An Arrow input column has no supported in-memory Substrait representation.
    UnsupportedInputType { column: usize, data_type: DataType },
    /// An Arrow value could not be represented by the in-memory adapter.
    InvalidInputValue {
        column: usize,
        row: usize,
        reason: &'static str,
    },
    /// The session failed during end-of-stream execution and is terminal.
    SessionFailed,
    /// The adapted plan failed in the existing Sirius Substrait API.
    Execution(SiriusError),
}

impl fmt::Display for StreamSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedInputReadCount { actual } => write!(
                f,
                "temporary streaming compatibility supports exactly one input read, got {actual}"
            ),
            Self::UnsupportedOutputRelationCount { actual } => write!(
                f,
                "temporary streaming compatibility supports exactly one output relation, got {actual}"
            ),
            Self::UnknownInputStream { actual } => write!(
                f,
                "input stream {} is not declared by the Substrait plan",
                actual.get()
            ),
            Self::UnknownOutputStream { actual } => write!(
                f,
                "output stream {} is not declared by the Substrait plan",
                actual.get()
            ),
            Self::UnsupportedInputBatchCount { actual } => write!(
                f,
                "temporary streaming compatibility supports exactly one input batch, got {actual}"
            ),
            Self::InputAlreadyPushed => f.write_str("the input batch has already been pushed"),
            Self::InputAlreadyEnded => f.write_str("the input stream has already ended"),
            Self::MissingInputBatch => {
                f.write_str("the input stream cannot end before one batch is pushed")
            }
            Self::InputNotEnded => {
                f.write_str("output is unavailable until the input stream has ended")
            }
            Self::UnsupportedOutputBatchCount { actual } => write!(
                f,
                "temporary streaming compatibility requires exactly one output batch, got {actual}"
            ),
            Self::InputColumnCountMismatch { plan, batch } => write!(
                f,
                "stream input has {batch} columns but the Substrait read declares {plan}"
            ),
            Self::UnsupportedInputType { column, data_type } => write!(
                f,
                "stream input column {column} has unsupported Arrow type {data_type}"
            ),
            Self::InvalidInputValue {
                column,
                row,
                reason,
            } => write!(
                f,
                "stream input value at column {column}, row {row} is invalid: {reason}"
            ),
            Self::SessionFailed => f.write_str("the streaming session has failed"),
            Self::Execution(err) => write!(f, "stream execution error: {err}"),
        }
    }
}

impl std::error::Error for StreamSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Execution(err) => Some(err),
            _ => None,
        }
    }
}

#[derive(Default)]
struct CompatibilityState {
    input: Option<RecordBatch>,
    output: Option<RecordBatch>,
    ended: bool,
    failed: bool,
}

impl CompatibilityState {
    fn push_batch(&mut self, batch: RecordBatch) -> Result<(), StreamSessionError> {
        if self.ended {
            return Err(StreamSessionError::InputAlreadyEnded);
        }
        if self.input.is_some() {
            return Err(StreamSessionError::InputAlreadyPushed);
        }
        self.input = Some(batch);
        Ok(())
    }

    fn push_batches_sync<I>(&mut self, batches: I) -> Result<(), StreamSessionError>
    where
        I: IntoIterator<Item = RecordBatch>,
    {
        let batches: Vec<_> = batches.into_iter().collect();
        if batches.len() != 1 {
            return Err(StreamSessionError::UnsupportedInputBatchCount {
                actual: batches.len(),
            });
        }
        self.push_batch(batches.into_iter().next().expect("length checked"))
    }

    fn begin_execution(&mut self) -> Result<&RecordBatch, StreamSessionError> {
        if self.ended {
            return Err(StreamSessionError::InputAlreadyEnded);
        }
        if self.input.is_none() {
            return Err(StreamSessionError::MissingInputBatch);
        }
        self.ended = true;
        Ok(self.input.as_ref().expect("checked above"))
    }

    fn finish_execution(
        &mut self,
        mut batches: Vec<RecordBatch>,
    ) -> Result<(), StreamSessionError> {
        self.input = None;
        if batches.len() != 1 {
            self.failed = true;
            return Err(StreamSessionError::UnsupportedOutputBatchCount {
                actual: batches.len(),
            });
        }
        self.output = batches.pop();
        Ok(())
    }

    fn fail_execution(&mut self) {
        self.input = None;
        self.failed = true;
    }

    fn pull_batch_sync(&mut self) -> Result<Option<RecordBatch>, StreamSessionError> {
        if !self.ended {
            return Err(StreamSessionError::InputNotEnded);
        }
        if self.failed {
            return Err(StreamSessionError::SessionFailed);
        }
        Ok(self.output.take())
    }

    fn pull_batches_sync(&mut self) -> Result<Vec<RecordBatch>, StreamSessionError> {
        Ok(self.pull_batch_sync()?.into_iter().collect())
    }
}

fn execute_compatibility_plan(
    context: &mut SiriusContext,
    plan: &ProtoPlan,
    input: &RecordBatch,
) -> Result<Vec<RecordBatch>, StreamSessionError> {
    let mut adapted = plan.clone();
    replace_read_with_batch(&mut adapted, input)?;
    context
        .execute_substrait(&adapted.encode_to_vec())
        .map_err(StreamSessionError::Execution)
}

fn discover_plan_streams(plan: &mut ProtoPlan) -> (Vec<StreamId>, Vec<StreamId>) {
    let inputs = (0..count_reads(plan))
        .map(|ordinal| StreamId::new(ordinal as u64))
        .collect();
    let outputs = plan
        .relations
        .iter()
        .filter(|relation| relation.rel_type.is_some())
        .enumerate()
        .map(|(ordinal, _)| StreamId::new(ordinal as u64))
        .collect();
    (inputs, outputs)
}

fn validate_compatibility_streams(plan: &SubstraitPlan) -> Result<(), StreamSessionError> {
    if plan.input_streams.len() != 1 {
        return Err(StreamSessionError::UnsupportedInputReadCount {
            actual: plan.input_streams.len(),
        });
    }
    if plan.output_streams.len() != 1 {
        return Err(StreamSessionError::UnsupportedOutputRelationCount {
            actual: plan.output_streams.len(),
        });
    }
    Ok(())
}

fn count_reads(plan: &mut ProtoPlan) -> usize {
    let mut count = 0;
    visit_plan_reads_mut(plan, &mut |_| count += 1);
    count
}

fn replace_read_with_batch(
    plan: &mut ProtoPlan,
    batch: &RecordBatch,
) -> Result<(), StreamSessionError> {
    let mut result = None;
    visit_plan_reads_mut(plan, &mut |read| {
        result = Some(build_virtual_table(read, batch));
    });
    result.unwrap_or(Ok(()))
}

fn build_virtual_table(
    read: &mut substrait::proto::ReadRel,
    batch: &RecordBatch,
) -> Result<(), StreamSessionError> {
    let target_types = match read
        .base_schema
        .as_ref()
        .and_then(|schema| schema.r#struct.as_ref())
    {
        Some(schema) => {
            if schema.types.len() != batch.num_columns() {
                return Err(StreamSessionError::InputColumnCountMismatch {
                    plan: schema.types.len(),
                    batch: batch.num_columns(),
                });
            }
            schema.types.clone()
        }
        None => {
            let schema = batch.schema();
            let types = schema
                .fields()
                .iter()
                .enumerate()
                .map(|(column, field)| {
                    arrow_type_to_substrait(field.data_type(), field.is_nullable(), column)
                })
                .collect::<Result<Vec<_>, _>>()?;
            read.base_schema = Some(NamedStruct {
                names: schema
                    .fields()
                    .iter()
                    .map(|field| field.name().clone())
                    .collect(),
                r#struct: Some(r#type::Struct {
                    types: types.clone(),
                    type_variation_reference: 0,
                    nullability: r#type::Nullability::Required as i32,
                }),
            });
            types
        }
    };

    let mut rows = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let fields = batch
            .columns()
            .iter()
            .zip(&target_types)
            .enumerate()
            .map(|(column, (array, target))| value_expression(array.as_ref(), row, column, target))
            .collect::<Result<Vec<_>, _>>()?;
        rows.push(nested::Struct { fields });
    }
    read.read_type = Some(ReadType::VirtualTable(VirtualTable {
        expressions: rows,
        ..Default::default()
    }));
    Ok(())
}

fn value_expression(
    array: &dyn Array,
    row: usize,
    column: usize,
    target: &Type,
) -> Result<Expression, StreamSessionError> {
    let literal_type = if array.is_null(row) {
        literal::LiteralType::Null(target.clone())
    } else {
        literal_value(array, row, column)?
    };
    let literal = Expression {
        rex_type: Some(expression::RexType::Literal(expression::Literal {
            literal_type: Some(literal_type),
            nullable: array.null_count() != 0,
            type_variation_reference: 0,
        })),
    };
    Ok(Expression {
        rex_type: Some(expression::RexType::Cast(Box::new(expression::Cast {
            r#type: Some(target.clone()),
            input: Some(Box::new(literal)),
            failure_behavior: expression::cast::FailureBehavior::ThrowException as i32,
        }))),
    })
}

fn literal_value(
    array: &dyn Array,
    row: usize,
    column: usize,
) -> Result<literal::LiteralType, StreamSessionError> {
    let value = match array.data_type() {
        DataType::Boolean => {
            literal::LiteralType::Boolean(downcast_array::<BooleanArray>(array).value(row))
        }
        DataType::Int8 => {
            literal::LiteralType::I8(i32::from(downcast_array::<Int8Array>(array).value(row)))
        }
        // DuckDB's current Substrait literal reader does not handle the I16
        // literal variant, so carry the value losslessly as I32 and cast it.
        DataType::Int16 => {
            literal::LiteralType::I32(i32::from(downcast_array::<Int16Array>(array).value(row)))
        }
        DataType::Int32 => {
            literal::LiteralType::I32(downcast_array::<Int32Array>(array).value(row))
        }
        DataType::Int64 => {
            literal::LiteralType::I64(downcast_array::<Int64Array>(array).value(row))
        }
        DataType::Float32 => {
            literal::LiteralType::Fp32(downcast_array::<Float32Array>(array).value(row))
        }
        DataType::Float64 => {
            literal::LiteralType::Fp64(downcast_array::<Float64Array>(array).value(row))
        }
        DataType::Utf8 => {
            literal::LiteralType::String(downcast_array::<StringArray>(array).value(row).to_owned())
        }
        DataType::LargeUtf8 => literal::LiteralType::String(
            downcast_array::<LargeStringArray>(array)
                .value(row)
                .to_owned(),
        ),
        DataType::Utf8View => literal::LiteralType::String(
            downcast_array::<StringViewArray>(array)
                .value(row)
                .to_owned(),
        ),
        DataType::Decimal128(precision, scale) => {
            let value = downcast_array::<Decimal128Array>(array).value(row);
            literal::LiteralType::Decimal(literal::Decimal {
                value: value.to_le_bytes().to_vec(),
                precision: i32::from(*precision),
                scale: i32::from(*scale),
            })
        }
        DataType::Date32 => {
            literal::LiteralType::Date(downcast_array::<Date32Array>(array).value(row))
        }
        DataType::Timestamp(unit, _) => {
            literal::LiteralType::String(format_timestamp(array, row, column, unit)?)
        }
        data_type => {
            return Err(StreamSessionError::UnsupportedInputType {
                column,
                data_type: data_type.clone(),
            });
        }
    };
    Ok(value)
}

fn downcast_array<A: Array + 'static>(array: &dyn Array) -> &A {
    array
        .as_any()
        .downcast_ref::<A>()
        .expect("Arrow data type and concrete array type must agree")
}

fn format_timestamp(
    array: &dyn Array,
    row: usize,
    column: usize,
    unit: &TimeUnit,
) -> Result<String, StreamSessionError> {
    let datetime = match unit {
        TimeUnit::Second => as_datetime::<TimestampSecondType>(
            downcast_array::<TimestampSecondArray>(array).value(row),
        ),
        TimeUnit::Millisecond => {
            let value = downcast_array::<TimestampMillisecondArray>(array).value(row);
            as_datetime::<TimestampMillisecondType>(value)
        }
        TimeUnit::Microsecond => {
            let value = downcast_array::<TimestampMicrosecondArray>(array).value(row);
            as_datetime::<TimestampMicrosecondType>(value)
        }
        TimeUnit::Nanosecond => {
            let value = downcast_array::<TimestampNanosecondArray>(array).value(row);
            as_datetime::<TimestampNanosecondType>(value)
        }
    };
    datetime
        .map(|value| value.format("%Y-%m-%d %H:%M:%S%.9f").to_string())
        .ok_or(StreamSessionError::InvalidInputValue {
            column,
            row,
            reason: "timestamp is outside the supported calendar range",
        })
}

fn arrow_type_to_substrait(
    data_type: &DataType,
    nullable: bool,
    column: usize,
) -> Result<Type, StreamSessionError> {
    let nullability = if nullable {
        r#type::Nullability::Nullable as i32
    } else {
        r#type::Nullability::Required as i32
    };
    let kind = match data_type {
        DataType::Boolean => r#type::Kind::Bool(r#type::Boolean {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Int8 => r#type::Kind::I8(r#type::I8 {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Int16 => r#type::Kind::I16(r#type::I16 {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Int32 => r#type::Kind::I32(r#type::I32 {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Int64 => r#type::Kind::I64(r#type::I64 {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Float32 => r#type::Kind::Fp32(r#type::Fp32 {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Float64 => r#type::Kind::Fp64(r#type::Fp64 {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {
            r#type::Kind::String(r#type::String {
                type_variation_reference: 0,
                nullability,
            })
        }
        DataType::Decimal128(precision, scale) => r#type::Kind::Decimal(r#type::Decimal {
            precision: i32::from(*precision),
            scale: i32::from(*scale),
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Date32 => r#type::Kind::Date(r#type::Date {
            type_variation_reference: 0,
            nullability,
        }),
        DataType::Timestamp(unit, timezone) => {
            let precision = match unit {
                TimeUnit::Second => 0,
                TimeUnit::Millisecond => 3,
                TimeUnit::Microsecond => 6,
                TimeUnit::Nanosecond => 9,
            };
            if timezone.is_some() {
                r#type::Kind::PrecisionTimestampTz(r#type::PrecisionTimestampTz {
                    precision,
                    type_variation_reference: 0,
                    nullability,
                })
            } else {
                r#type::Kind::PrecisionTimestamp(r#type::PrecisionTimestamp {
                    precision,
                    type_variation_reference: 0,
                    nullability,
                })
            }
        }
        data_type => {
            return Err(StreamSessionError::UnsupportedInputType {
                column,
                data_type: data_type.clone(),
            });
        }
    };
    Ok(Type { kind: Some(kind) })
}

fn visit_plan_reads_mut(
    plan: &mut ProtoPlan,
    visitor: &mut impl FnMut(&mut substrait::proto::ReadRel),
) {
    for relation in &mut plan.relations {
        match relation.rel_type.as_mut() {
            Some(plan_rel::RelType::Rel(rel)) => visit_reads_mut(rel, visitor),
            Some(plan_rel::RelType::Root(root)) => {
                if let Some(input) = root.input.as_mut() {
                    visit_reads_mut(input, visitor);
                }
            }
            None => {}
        }
    }
}

fn visit_reads_mut(rel: &mut Rel, visitor: &mut impl FnMut(&mut substrait::proto::ReadRel)) {
    match rel.rel_type.as_mut() {
        Some(rel::RelType::Read(read)) => visitor(read),
        Some(rel::RelType::Filter(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Fetch(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Aggregate(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Sort(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Join(node)) => {
            visit_optional_read_mut(&mut node.left, visitor);
            visit_optional_read_mut(&mut node.right, visitor);
        }
        Some(rel::RelType::Project(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Set(node)) => {
            for input in &mut node.inputs {
                visit_reads_mut(input, visitor);
            }
        }
        Some(rel::RelType::ExtensionSingle(node)) => {
            visit_optional_read_mut(&mut node.input, visitor);
        }
        Some(rel::RelType::ExtensionMulti(node)) => {
            for input in &mut node.inputs {
                visit_reads_mut(input, visitor);
            }
        }
        Some(rel::RelType::ExtensionLeaf(_)) | Some(rel::RelType::Reference(_)) => {}
        Some(rel::RelType::Cross(node)) => {
            visit_optional_read_mut(&mut node.left, visitor);
            visit_optional_read_mut(&mut node.right, visitor);
        }
        Some(rel::RelType::Write(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Ddl(node)) => {
            visit_optional_read_mut(&mut node.view_definition, visitor);
        }
        Some(rel::RelType::Update(_)) => {}
        Some(rel::RelType::HashJoin(node)) => {
            visit_optional_read_mut(&mut node.left, visitor);
            visit_optional_read_mut(&mut node.right, visitor);
        }
        Some(rel::RelType::MergeJoin(node)) => {
            visit_optional_read_mut(&mut node.left, visitor);
            visit_optional_read_mut(&mut node.right, visitor);
        }
        Some(rel::RelType::NestedLoopJoin(node)) => {
            visit_optional_read_mut(&mut node.left, visitor);
            visit_optional_read_mut(&mut node.right, visitor);
        }
        Some(rel::RelType::Window(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Exchange(node)) => visit_optional_read_mut(&mut node.input, visitor),
        Some(rel::RelType::Expand(node)) => visit_optional_read_mut(&mut node.input, visitor),
        None => {}
    }
}

fn visit_optional_read_mut(
    input: &mut Option<Box<Rel>>,
    visitor: &mut impl FnMut(&mut substrait::proto::ReadRel),
) {
    if let Some(input) = input.as_deref_mut() {
        visit_reads_mut(input, visitor);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{ArrayRef, Int64Array, UInt64Array};
    use arrow_schema::{DataType, Field, Schema};
    use substrait::proto::read_rel::{NamedTable, ReadType};
    use substrait::proto::{PlanRel, ReadRel, RelRoot};

    use super::*;

    fn batch(values: &[i64]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            false,
        )]));
        let values: ArrayRef = Arc::new(Int64Array::from(values.to_vec()));
        RecordBatch::try_new(schema, vec![values]).unwrap()
    }

    fn single_read_plan() -> ProtoPlan {
        let read = Rel {
            rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
                read_type: Some(ReadType::NamedTable(NamedTable {
                    names: vec!["stream_input".to_owned()],
                    ..Default::default()
                })),
                ..Default::default()
            }))),
        };
        ProtoPlan {
            relations: vec![PlanRel {
                rel_type: Some(plan_rel::RelType::Root(RelRoot {
                    input: Some(read),
                    names: vec!["value".to_owned()],
                })),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn replaces_the_single_plan_read_with_an_in_memory_virtual_table() {
        let mut plan = single_read_plan();
        assert_eq!(count_reads(&mut plan), 1);
        replace_read_with_batch(&mut plan, &batch(&[7, 11])).unwrap();

        let mut reads = Vec::new();
        visit_plan_reads_mut(&mut plan, &mut |read| reads.push(read.clone()));
        let read = &reads[0];
        let ReadType::VirtualTable(table) = read.read_type.as_ref().unwrap() else {
            panic!("expected virtual_table replacement");
        };
        assert_eq!(table.expressions.len(), 2);
        assert_eq!(read.base_schema.as_ref().unwrap().names, ["value"]);
        let expression::RexType::Cast(cast) =
            table.expressions[0].fields[0].rex_type.as_ref().unwrap()
        else {
            panic!("expected a cast preserving the plan's declared input type");
        };
        let expression::RexType::Literal(literal) =
            cast.input.as_ref().unwrap().rex_type.as_ref().unwrap()
        else {
            panic!("expected a literal virtual-table value");
        };
        assert!(matches!(
            literal.literal_type,
            Some(literal::LiteralType::I64(7))
        ));
    }

    #[test]
    fn discovers_streams_from_the_plan() {
        let encoded = single_read_plan().encode_to_vec();
        let plan = SubstraitPlan::decode(&encoded).unwrap();

        assert_eq!(plan.input_streams(), [StreamId::new(0)]);
        assert_eq!(plan.output_streams(), [StreamId::new(0)]);
        validate_compatibility_streams(&plan).unwrap();

        let mut proto = single_read_plan();
        proto.relations.push(PlanRel {
            rel_type: Some(plan_rel::RelType::Root(RelRoot::default())),
        });
        let plan = SubstraitPlan::from_proto(proto);
        assert_eq!(plan.output_streams(), [StreamId::new(0), StreamId::new(1)]);
        assert!(matches!(
            validate_compatibility_streams(&plan),
            Err(StreamSessionError::UnsupportedOutputRelationCount { actual: 2 })
        ));

        assert!(SubstraitPlan::decode(&[0xff]).is_err());
    }

    #[test]
    fn rejects_input_types_without_a_virtual_table_representation() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::UInt64,
            false,
        )]));
        let values: ArrayRef = Arc::new(UInt64Array::from(vec![1]));
        let unsupported = RecordBatch::try_new(schema, vec![values]).unwrap();
        let mut plan = single_read_plan();

        assert!(matches!(
            replace_read_with_batch(&mut plan, &unsupported),
            Err(StreamSessionError::UnsupportedInputType {
                column: 0,
                data_type: DataType::UInt64,
            })
        ));
    }

    #[test]
    fn counts_reads_across_relation_trees() {
        let read = || Rel {
            rel_type: Some(rel::RelType::Read(Box::default())),
        };
        let join = Rel {
            rel_type: Some(rel::RelType::Join(Box::new(substrait::proto::JoinRel {
                left: Some(Box::new(read())),
                right: Some(Box::new(read())),
                ..Default::default()
            }))),
        };
        let mut plan = ProtoPlan {
            relations: vec![PlanRel {
                rel_type: Some(plan_rel::RelType::Root(RelRoot {
                    input: Some(join),
                    ..Default::default()
                })),
            }],
            ..Default::default()
        };
        assert_eq!(count_reads(&mut ProtoPlan::default()), 0);
        assert_eq!(count_reads(&mut plan), 2);
        let plan = SubstraitPlan::from_proto(plan);
        assert_eq!(plan.input_streams(), [StreamId::new(0), StreamId::new(1)]);
        assert!(matches!(
            validate_compatibility_streams(&plan),
            Err(StreamSessionError::UnsupportedInputReadCount { actual: 2 })
        ));
    }

    #[test]
    fn accepts_exactly_one_input_batch() {
        let mut state = CompatibilityState::default();
        state.push_batch(batch(&[1, 2])).unwrap();
        assert!(matches!(
            state.push_batch(batch(&[3])),
            Err(StreamSessionError::InputAlreadyPushed)
        ));

        let mut empty = CompatibilityState::default();
        assert!(matches!(
            empty.push_batches_sync(Vec::new()),
            Err(StreamSessionError::UnsupportedInputBatchCount { actual: 0 })
        ));
        let mut multiple = CompatibilityState::default();
        assert!(matches!(
            multiple.push_batches_sync(vec![batch(&[1]), batch(&[2])]),
            Err(StreamSessionError::UnsupportedInputBatchCount { actual: 2 })
        ));
    }

    #[test]
    fn end_requires_input_and_accepts_exactly_one_output_batch() {
        let mut missing = CompatibilityState::default();
        assert!(matches!(
            missing.begin_execution(),
            Err(StreamSessionError::MissingInputBatch)
        ));

        let mut state = CompatibilityState::default();
        state.push_batch(batch(&[1])).unwrap();
        state.begin_execution().unwrap();
        state.finish_execution(vec![batch(&[2, 3])]).unwrap();
        let output = state.pull_batch_sync().unwrap().unwrap();
        assert_eq!(output.num_rows(), 2);
        assert!(state.pull_batch_sync().unwrap().is_none());
    }

    #[test]
    fn rejects_zero_or_multiple_output_batches() {
        for outputs in [Vec::new(), vec![batch(&[1]), batch(&[2])]] {
            let mut state = CompatibilityState::default();
            state.push_batch(batch(&[0])).unwrap();
            state.begin_execution().unwrap();
            let actual = outputs.len();
            assert!(matches!(
                state.finish_execution(outputs),
                Err(StreamSessionError::UnsupportedOutputBatchCount { actual: n }) if n == actual
            ));
            assert!(matches!(
                state.pull_batch_sync(),
                Err(StreamSessionError::SessionFailed)
            ));
        }
    }

    #[test]
    fn pull_before_end_is_rejected() {
        let mut state = CompatibilityState::default();
        state.push_batch(batch(&[1])).unwrap();
        assert!(matches!(
            state.pull_batches_sync(),
            Err(StreamSessionError::InputNotEnded)
        ));
    }
}
