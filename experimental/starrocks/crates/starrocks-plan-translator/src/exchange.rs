//! StarRocks fragment boundaries expressed with native Substrait exchange relations.

use prost::Message;
use sirius_exchange_proto::{
    DESTINATION_TYPE_URL, ExchangeDestination, ExchangeSink, ExchangeSource, Id128, SINK_TYPE_URL,
    SOURCE_TYPE_URL,
};
use starrocks_thrift::data_sinks::TDataSinkType;
use starrocks_thrift::exprs::{TExpr, TExprNodeType};
use starrocks_thrift::internal_service::{TExecPlanFragmentParams, TPlanFragmentExecParams};
use starrocks_thrift::partitions::TPartitionType;
use starrocks_thrift::plan_nodes::TPlanNode;
use starrocks_thrift::types::{TPrimitiveType, TTypeNodeType, TUniqueId};
use substrait::proto::{
    ExchangeRel, Expression, NamedStruct, ProjectRel, ReadRel, Rel, RelCommon, Type, exchange_rel,
    expression, read_rel, rel, rel_common, r#type,
};

use crate::descriptor_table::DescriptorTable;
use crate::error::{Result, TranslateError};
use crate::node_translator::TranslatedRel;
use crate::type_mapper;

/// Return the physical exchange type, normalizing character types for DuckDB casts.
fn exchange_type(ty: &Type) -> Result<Type> {
    let text = match &ty.kind {
        Some(r#type::Kind::Binary(_)) => {
            return Err(TranslateError::UnsupportedType {
                primitive: Some(TPrimitiveType::BINARY),
                node_type: Some(TTypeNodeType::SCALAR),
                reason: "binary exchange columns are unsupported by Sirius",
            });
        }
        Some(r#type::Kind::Varchar(text)) => Some((text.length, text.nullability)),
        Some(r#type::Kind::FixedChar(text)) => Some((text.length, text.nullability)),
        _ => None,
    };
    if let Some((length, nullability)) = text {
        if length < 1 {
            return Err(TranslateError::malformed(
                "exchange character length must be positive",
            ));
        }
        return Ok(Type {
            kind: Some(r#type::Kind::String(r#type::String {
                type_variation_reference: 0,
                nullability,
            })),
        });
    }
    Ok(ty.clone())
}

/// Enforce the frontend's output schema even when DuckDB infers wider aggregate results.
fn cast_output(
    input: Rel,
    params: &TExecPlanFragmentParams,
    desc: &DescriptorTable,
    input_layout: &[i32],
) -> Result<Rel> {
    let fragment = params.fragment.as_ref().unwrap();
    let types = if let Some(outputs) = fragment
        .output_exprs
        .as_ref()
        .filter(|outputs| !outputs.is_empty())
    {
        outputs
            .iter()
            .map(|expr| {
                let node = expr.nodes.first().ok_or_else(|| {
                    TranslateError::malformed("exchange output expression is empty")
                })?;
                type_mapper::map_type_desc(&node.type_, node.is_nullable.unwrap_or(true))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        let mut types = Vec::new();
        for tuple in input_layout {
            types.extend(desc.named_struct(*tuple)?.r#struct.unwrap().types);
        }
        types
    };
    let width = i32::try_from(types.len())
        .map_err(|_| TranslateError::malformed("too many exchange columns"))?;
    let expressions = types
        .iter()
        .enumerate()
        .map(|(index, ty)| {
            Ok(Expression {
                rex_type: Some(expression::RexType::Cast(Box::new(expression::Cast {
                    r#type: Some(exchange_type(ty)?),
                    input: Some(Box::new(Expression {
                        rex_type: Some(expression::RexType::Selection(Box::new(field(index)?))),
                    })),
                    failure_behavior: expression::cast::FailureBehavior::ThrowException as i32,
                }))),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Rel {
        rel_type: Some(rel::RelType::Project(Box::new(ProjectRel {
            common: Some(RelCommon {
                emit_kind: Some(rel_common::EmitKind::Emit(rel_common::Emit {
                    output_mapping: (width..width.checked_mul(2).ok_or_else(|| {
                        TranslateError::malformed("exchange column count overflows i32")
                    })?)
                        .collect(),
                })),
                ..Default::default()
            }),
            input: Some(Box::new(input)),
            expressions,
            ..Default::default()
        }))),
    })
}

fn id(value: &TUniqueId) -> Id128 {
    Id128 {
        high: value.hi as u64,
        low: value.lo as u64,
    }
}

fn nonnegative(value: i32, field: &str) -> Result<u32> {
    u32::try_from(value)
        .map_err(|_| TranslateError::malformed(format!("{field} must be nonnegative")))
}

fn exec(params: Option<&TPlanFragmentExecParams>) -> Result<&TPlanFragmentExecParams> {
    params.ok_or(TranslateError::MissingField {
        context: "exchange fragment",
        field: "params",
    })
}

pub(crate) fn source(
    node: &TPlanNode,
    desc: &DescriptorTable,
    params: Option<&TPlanFragmentExecParams>,
) -> Result<TranslatedRel> {
    let exchange = node
        .exchange_node
        .as_ref()
        .ok_or(TranslateError::MissingField {
            context: "EXCHANGE_NODE",
            field: "exchange_node",
        })?;
    if exchange.sort_info.is_some() || exchange.enable_parallel_merge == Some(true) {
        return Err(TranslateError::UnsupportedPlanNode {
            node_id: node.node_id,
            node_type: node.node_type,
            reason: "merging exchanges require ordered stream merging",
        });
    }
    if exchange.offset.is_some_and(|offset| offset < 0) {
        return Err(TranslateError::malformed(
            "exchange offset must be nonnegative",
        ));
    }
    if exchange.input_row_tuples != node.row_tuples || node.row_tuples.is_empty() {
        return Err(TranslateError::UnsupportedPlanNode {
            node_id: node.node_id,
            node_type: node.node_type,
            reason: "exchange input tuples must match its complete output layout",
        });
    }
    let params = exec(params)?;
    let senders = params
        .per_exch_num_senders
        .get(&node.node_id)
        .copied()
        .ok_or_else(|| TranslateError::malformed("exchange has no expected sender count"))?;
    if senders < 1 {
        return Err(TranslateError::malformed(
            "exchange expected sender count must be positive",
        ));
    }
    let metadata = ExchangeSource {
        query_id: Some(id(&params.query_id)),
        receiver_fragment_id: Some(id(&params.fragment_instance_id)),
        exchange_id: Some(nonnegative(node.node_id, "exchange node id")?),
        expected_sender_ids: (0..senders as u32).collect(),
    };
    let mut schema = NamedStruct {
        names: Vec::new(),
        r#struct: Some(r#type::Struct {
            nullability: r#type::Nullability::Required as i32,
            ..Default::default()
        }),
    };
    for tuple in &node.row_tuples {
        let tuple_schema = desc.named_struct(*tuple)?;
        for ty in &tuple_schema.r#struct.as_ref().unwrap().types {
            exchange_type(ty)?;
        }
        schema.names.extend(tuple_schema.names);
        schema
            .r#struct
            .as_mut()
            .unwrap()
            .types
            .extend(tuple_schema.r#struct.unwrap().types);
    }
    if schema.names.is_empty() {
        return Err(TranslateError::malformed(
            "exchange needs at least one materialized column",
        ));
    }
    let output_width = schema.names.len();
    let mut table = read_rel::ExtensionTable::default();
    let detail = table.detail.get_or_insert_default();
    detail.type_url = SOURCE_TYPE_URL.into();
    detail.value = metadata.encode_to_vec().into();
    Ok(TranslatedRel {
        rel: Rel {
            rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
                base_schema: Some(schema),
                read_type: Some(read_rel::ReadType::ExtensionTable(table)),
                ..Default::default()
            }))),
        },
        row_tuples: node.row_tuples.clone(),
        output_width,
    })
}

fn slot(expr: &TExpr) -> Option<(i32, i32)> {
    match expr.nodes.as_slice() {
        [node] if node.node_type == TExprNodeType::SLOT_REF && node.num_children == 0 => node
            .slot_ref
            .as_ref()
            .map(|slot| (slot.tuple_id, slot.slot_id)),
        _ => None,
    }
}

fn field(index: usize) -> Result<expression::FieldReference> {
    use expression::{field_reference, reference_segment};
    let index = i32::try_from(index)
        .map_err(|_| TranslateError::malformed("exchange column index overflows i32"))?;
    Ok(expression::FieldReference {
        reference_type: Some(field_reference::ReferenceType::DirectReference(
            expression::ReferenceSegment {
                reference_type: Some(reference_segment::ReferenceType::StructField(Box::new(
                    reference_segment::StructField {
                        field: index,
                        child: None,
                    },
                ))),
            },
        )),
        root_type: Some(field_reference::RootType::RootReference(
            field_reference::RootReference {},
        )),
    })
}

pub(crate) fn attach_sink(
    input: Rel,
    params: &TExecPlanFragmentParams,
    desc: &DescriptorTable,
    input_layout: &[i32],
) -> Result<Rel> {
    let fragment = params.fragment.as_ref().unwrap();
    let Some(sink) = fragment.output_sink.as_ref() else {
        return Ok(input);
    };
    if sink.type_ != TDataSinkType::DATA_STREAM_SINK {
        if sink.type_ == TDataSinkType::MULTI_CAST_DATA_STREAM_SINK {
            return Err(TranslateError::malformed(
                "multicast data stream sinks are unsupported",
            ));
        }
        return Ok(input);
    }
    let stream = sink
        .stream_sink
        .as_ref()
        .ok_or(TranslateError::MissingField {
            context: "DATA_STREAM_SINK",
            field: "stream_sink",
        })?;
    if stream.is_merge == Some(true)
        || stream.dest_dop.is_some_and(|dop| dop > 1)
        || stream
            .output_columns
            .as_ref()
            .is_some_and(|columns| !columns.is_empty())
        || stream.limit.is_some_and(|limit| limit >= 0)
    {
        return Err(TranslateError::malformed(
            "merge, parallel-driver, pruned-column, and limited stream sinks are unsupported",
        ));
    }
    let exec = exec(params.params.as_ref())?;
    let destinations = exec.destinations.as_deref().unwrap_or_default();
    if destinations.is_empty() {
        return Err(TranslateError::malformed(
            "stream sink requires destinations",
        ));
    }
    let count = i32::try_from(destinations.len())
        .map_err(|_| TranslateError::malformed("too many exchange destinations"))?;
    let metadata = ExchangeSink {
        query_id: Some(id(&exec.query_id)),
        sender_fragment_id: Some(id(&exec.fragment_instance_id)),
        sender_id: Some(nonnegative(
            exec.sender_id.ok_or(TranslateError::MissingField {
                context: "DATA_STREAM_SINK",
                field: "sender_id",
            })?,
            "sender id",
        )?),
    };
    let mut exchange = ExchangeRel {
        input: Some(Box::new(cast_output(input, params, desc, input_layout)?)),
        partition_count: count,
        ..Default::default()
    };
    let enhancement = exchange
        .advanced_extension
        .get_or_insert_default()
        .enhancement
        .get_or_insert_default();
    enhancement.type_url = SINK_TYPE_URL.into();
    enhancement.value = metadata.encode_to_vec().into();
    let exchange_id = nonnegative(stream.dest_node_id, "destination exchange node id")?;
    let mut receivers = std::collections::HashSet::new();
    for (index, destination) in destinations.iter().enumerate() {
        if destination
            .pipeline_driver_sequence
            .is_some_and(|sequence| sequence != 0)
        {
            return Err(TranslateError::malformed(
                "exchange pipeline driver sequence is unsupported",
            ));
        }
        if !receivers.insert((
            destination.fragment_instance_id.hi,
            destination.fragment_instance_id.lo,
        )) {
            return Err(TranslateError::malformed(
                "stream sink repeats a receiver fragment",
            ));
        }
        let address = destination
            .brpc_server
            .as_ref()
            .ok_or(TranslateError::MissingField {
                context: "exchange destination",
                field: "brpc_server",
            })?;
        if address.hostname.is_empty() || !(1..=65535).contains(&address.port) {
            return Err(TranslateError::malformed(
                "exchange destination has invalid BRPC address",
            ));
        }
        let metadata = ExchangeDestination {
            receiver_fragment_id: Some(id(&destination.fragment_instance_id)),
            exchange_id: Some(exchange_id),
            peer_id: format!("starrocks://{}:{}", address.hostname, address.port),
        };
        let mut target = exchange_rel::ExchangeTarget {
            partition_id: vec![index as i32],
            ..Default::default()
        };
        let any = Default::default();
        target.target_type = Some(exchange_rel::exchange_target::TargetType::Extended(any));
        if let Some(exchange_rel::exchange_target::TargetType::Extended(any)) =
            target.target_type.as_mut()
        {
            any.type_url = DESTINATION_TYPE_URL.into();
            any.value = metadata.encode_to_vec().into();
        }
        exchange.targets.push(target);
    }
    use exchange_rel::ExchangeKind;
    exchange.exchange_kind = Some(match stream.output_partition.type_ {
        TPartitionType::UNPARTITIONED if count == 1 => {
            ExchangeKind::SingleTarget(Box::new(exchange_rel::SingleBucketExpression {
                expression: Some(Box::new(substrait::proto::Expression {
                    rex_type: Some(expression::RexType::Literal(expression::Literal {
                        literal_type: Some(expression::literal::LiteralType::I32(0)),
                        ..Default::default()
                    })),
                })),
            }))
        }
        TPartitionType::UNPARTITIONED => ExchangeKind::Broadcast(exchange_rel::Broadcast {}),
        TPartitionType::HASH_PARTITIONED => {
            if stream
                .output_partition
                .bucket_properties
                .as_ref()
                .is_some_and(|properties| !properties.is_empty())
            {
                return Err(TranslateError::malformed(
                    "bucket-specific exchange hashing is unsupported",
                ));
            }
            let expressions = stream
                .output_partition
                .partition_exprs
                .as_deref()
                .unwrap_or_default();
            if expressions.is_empty() {
                return Err(TranslateError::malformed(
                    "hash exchange requires partition expressions",
                ));
            }
            let fields = expressions
                .iter()
                .map(|expr| {
                    let key = slot(expr).ok_or_else(|| {
                        TranslateError::malformed(
                            "hash exchange keys must be plain slot references",
                        )
                    })?;
                    let index = if let Some(outputs) = fragment
                        .output_exprs
                        .as_ref()
                        .filter(|outputs| !outputs.is_empty())
                    {
                        outputs
                            .iter()
                            .position(|output| slot(output) == Some(key))
                            .ok_or_else(|| {
                                TranslateError::malformed(
                                    "hash key is absent from fragment output expressions",
                                )
                            })?
                    } else {
                        desc.slot_global_index(key.0, key.1, input_layout)?
                    };
                    field(index)
                })
                .collect::<Result<Vec<_>>>()?;
            ExchangeKind::ScatterByFields(exchange_rel::ScatterFields { fields })
        }
        _ => {
            return Err(TranslateError::malformed(
                "unsupported StarRocks exchange partition type",
            ));
        }
    });
    Ok(Rel {
        rel_type: Some(rel::RelType::Exchange(Box::new(exchange))),
    })
}
