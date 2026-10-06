use std::path::Path;

use prost::Message;
use sirius_exchange_proto::{
    DESTINATION_TYPE_URL, ExchangeDestination, ExchangeSink, ExchangeSource, Id128, SINK_TYPE_URL,
    SOURCE_TYPE_URL,
};
use substrait::proto::exchange_rel::{self, exchange_target};
use substrait::proto::expression::{self, field_reference, reference_segment};
use substrait::proto::extensions::{
    SimpleExtensionDeclaration, SimpleExtensionUrn, simple_extension_declaration,
};
use substrait::proto::{
    ExchangeRel, Expression, FetchRel, FilterRel, FunctionArgument, NamedStruct, ReadRel, Rel,
    Type, function_argument, read_rel, rel, r#type,
};

use super::{local_files_read, plan};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Gather,
    Hash,
    Broadcast,
}

pub enum KeyType {
    I32,
    I64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReceiverOperator {
    Identity,
    NotNull,
    Limit(u32),
    FilterFalse,
}

fn field(index: i32) -> expression::FieldReference {
    expression::FieldReference {
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
    }
}

pub fn id(value: u64) -> Id128 {
    Id128 {
        high: 0x736972697573,
        low: value,
    }
}

fn row_schema(key_type: KeyType) -> NamedStruct {
    NamedStruct {
        names: vec!["key".to_owned(), "payload".to_owned()],
        r#struct: Some(r#type::Struct {
            types: vec![
                Type {
                    kind: Some(match key_type {
                        KeyType::I32 => r#type::Kind::I32(r#type::I32 {
                            nullability: r#type::Nullability::Nullable as i32,
                            ..Default::default()
                        }),
                        KeyType::I64 => r#type::Kind::I64(r#type::I64 {
                            nullability: r#type::Nullability::Nullable as i32,
                            ..Default::default()
                        }),
                    }),
                },
                Type {
                    kind: Some(r#type::Kind::String(r#type::String {
                        nullability: r#type::Nullability::Nullable as i32,
                        ..Default::default()
                    })),
                },
            ],
            nullability: r#type::Nullability::Required as i32,
            ..Default::default()
        }),
    }
}

pub fn sender_plan(
    path: &Path,
    is_empty: bool,
    mode: Mode,
    query: u64,
    sender: u32,
    receiver_peers: &[String],
) -> Vec<u8> {
    let kind = match mode {
        Mode::Gather => {
            let mut bucket = exchange_rel::SingleBucketExpression::default();
            bucket.expression.get_or_insert_default().rex_type =
                Some(expression::RexType::Literal(expression::Literal {
                    literal_type: Some(expression::literal::LiteralType::I32(0)),
                    ..Default::default()
                }));
            exchange_rel::ExchangeKind::SingleTarget(bucket.into())
        }
        Mode::Hash => exchange_rel::ExchangeKind::ScatterByFields(exchange_rel::ScatterFields {
            fields: vec![field(0)],
        }),
        Mode::Broadcast => exchange_rel::ExchangeKind::Broadcast(exchange_rel::Broadcast {}),
    };
    let input = if is_empty {
        Rel {
            rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
                base_schema: Some(row_schema(KeyType::I64)),
                read_type: Some(read_rel::ReadType::VirtualTable(Default::default())),
                ..Default::default()
            }))),
        }
    } else {
        local_files_read(path)
    };
    let mut exchange = ExchangeRel {
        input: Some(Box::new(input)),
        partition_count: receiver_peers.len().try_into().unwrap(),
        exchange_kind: Some(kind),
        ..Default::default()
    };
    let metadata = ExchangeSink {
        query_id: Some(id(query)),
        sender_fragment_id: Some(id(u64::from(sender) + 100)),
        sender_id: Some(sender),
    };
    let detail = exchange
        .advanced_extension
        .get_or_insert_default()
        .enhancement
        .get_or_insert_default();
    detail.type_url = SINK_TYPE_URL.to_owned();
    detail.value = metadata.encode_to_vec();
    for (partition, peer) in receiver_peers.iter().enumerate() {
        let metadata = ExchangeDestination {
            receiver_fragment_id: Some(id(partition as u64 + 1)),
            exchange_id: Some(7),
            peer_id: peer.clone(),
        };
        let mut target = exchange_rel::ExchangeTarget {
            partition_id: vec![partition.try_into().unwrap()],
            target_type: Some(exchange_target::TargetType::Extended(Default::default())),
        };
        let Some(exchange_target::TargetType::Extended(detail)) = &mut target.target_type else {
            unreachable!()
        };
        detail.type_url = DESTINATION_TYPE_URL.to_owned();
        detail.value = metadata.encode_to_vec();
        exchange.targets.push(target);
    }
    plan(Rel {
        rel_type: Some(rel::RelType::Exchange(Box::new(exchange))),
    })
    .encode_to_vec()
}

pub fn receiver_plan(
    query: u64,
    receiver: u64,
    senders: u32,
    operation: ReceiverOperator,
    key_type: KeyType,
) -> Vec<u8> {
    let metadata = ExchangeSource {
        query_id: Some(id(query)),
        receiver_fragment_id: Some(id(receiver + 1)),
        exchange_id: Some(7),
        expected_sender_ids: (0..senders).collect(),
    };
    let mut extension = read_rel::ExtensionTable::default();
    let detail = extension.detail.get_or_insert_default();
    detail.type_url = SOURCE_TYPE_URL.to_owned();
    detail.value = metadata.encode_to_vec();
    let read = Rel {
        rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
            base_schema: Some(row_schema(key_type)),
            read_type: Some(read_rel::ReadType::ExtensionTable(extension)),
            ..Default::default()
        }))),
    };
    let mut output = plan(read.clone());
    if operation == ReceiverOperator::NotNull {
        let condition = Expression {
            rex_type: Some(expression::RexType::ScalarFunction(
                expression::ScalarFunction {
                    function_reference: 1,
                    arguments: vec![FunctionArgument {
                        arg_type: Some(function_argument::ArgType::Value(Expression {
                            rex_type: Some(expression::RexType::Selection(Box::new(field(0)))),
                        })),
                    }],
                    output_type: Some(Type {
                        kind: Some(r#type::Kind::Bool(r#type::Boolean {
                            nullability: r#type::Nullability::Required as i32,
                            ..Default::default()
                        })),
                    }),
                    ..Default::default()
                },
            )),
        };
        output = plan(Rel {
            rel_type: Some(rel::RelType::Filter(Box::new(FilterRel {
                input: Some(Box::new(read)),
                condition: Some(Box::new(condition)),
                ..Default::default()
            }))),
        });
        output.extension_urns.push(SimpleExtensionUrn {
            extension_urn_anchor: 1,
            urn: "extension:io.substrait:functions_comparison".to_owned(),
        });
        output.extensions.push(SimpleExtensionDeclaration {
            mapping_type: Some(
                simple_extension_declaration::MappingType::ExtensionFunction(
                    simple_extension_declaration::ExtensionFunction {
                        extension_urn_reference: 1,
                        function_anchor: 1,
                        name: "is_not_null".to_owned(),
                    },
                ),
            ),
        });
    } else if let ReceiverOperator::Limit(count) = operation {
        output = plan(Rel {
            rel_type: Some(rel::RelType::Fetch(Box::new(FetchRel {
                input: Some(Box::new(read)),
                count_expr: Some(Box::new(Expression {
                    rex_type: Some(expression::RexType::Literal(expression::Literal {
                        literal_type: Some(expression::literal::LiteralType::I64(i64::from(count))),
                        ..Default::default()
                    })),
                })),
                ..Default::default()
            }))),
        });
    } else if operation == ReceiverOperator::FilterFalse {
        output = plan(Rel {
            rel_type: Some(rel::RelType::Filter(Box::new(FilterRel {
                input: Some(Box::new(read)),
                condition: Some(Box::new(Expression {
                    rex_type: Some(expression::RexType::Literal(expression::Literal {
                        literal_type: Some(expression::literal::LiteralType::Boolean(false)),
                        ..Default::default()
                    })),
                })),
                ..Default::default()
            }))),
        });
    }
    output.encode_to_vec()
}
