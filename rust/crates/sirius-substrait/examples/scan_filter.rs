//! Create a Parquet fixture, encode ordinary Substrait, and execute through the public API.
use arrow_array::{Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use prost::Message;
use std::{fs::File, sync::Arc};
use substrait::proto as p;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("orders.parquet");
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, true)]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![Arc::new(Int64Array::from(vec![
            Some(1),
            None,
            Some(20),
            Some(30),
        ]))],
    )?;
    let mut writer = ArrowWriter::try_new(File::create(&path)?, schema, None)?;
    writer.write(&batch)?;
    writer.close()?;

    let read = p::Rel {
        rel_type: Some(p::rel::RelType::Read(Box::new(p::ReadRel {
            base_schema: Some(p::NamedStruct {
                names: vec!["id".into()],
                r#struct: Some(p::r#type::Struct {
                    types: vec![p::Type {
                        kind: Some(p::r#type::Kind::I64(p::r#type::I64 {
                            nullability: p::r#type::Nullability::Nullable as i32,
                            ..Default::default()
                        })),
                    }],
                    nullability: p::r#type::Nullability::Required as i32,
                    ..Default::default()
                }),
            }),
            read_type: Some(p::read_rel::ReadType::LocalFiles(p::read_rel::LocalFiles {
                items: vec![p::read_rel::local_files::FileOrFiles {
                    path_type: Some(p::read_rel::local_files::file_or_files::PathType::UriFile(
                        url::Url::from_file_path(&path).unwrap().into(),
                    )),
                    file_format: Some(
                        p::read_rel::local_files::file_or_files::FileFormat::Parquet(
                            Default::default(),
                        ),
                    ),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }))),
    };
    let column = p::Expression {
        rex_type: Some(p::expression::RexType::Selection(Box::new(
            p::expression::FieldReference {
                reference_type: Some(
                    p::expression::field_reference::ReferenceType::DirectReference(
                        p::expression::ReferenceSegment {
                            reference_type: Some(
                                p::expression::reference_segment::ReferenceType::StructField(
                                    Box::new(p::expression::reference_segment::StructField {
                                        field: 0,
                                        child: None,
                                    }),
                                ),
                            ),
                        },
                    ),
                ),
                root_type: Some(p::expression::field_reference::RootType::RootReference(
                    Default::default(),
                )),
            },
        ))),
    };
    let literal = p::Expression {
        rex_type: Some(p::expression::RexType::Literal(p::expression::Literal {
            literal_type: Some(p::expression::literal::LiteralType::I64(10)),
            ..Default::default()
        })),
    };
    let filter = p::Rel {
        rel_type: Some(p::rel::RelType::Filter(Box::new(p::FilterRel {
            input: Some(Box::new(read)),
            condition: Some(Box::new(p::Expression {
                rex_type: Some(p::expression::RexType::ScalarFunction(
                    p::expression::ScalarFunction {
                        function_reference: 1,
                        arguments: vec![column, literal]
                            .into_iter()
                            .map(|value| p::FunctionArgument {
                                arg_type: Some(p::function_argument::ArgType::Value(value)),
                            })
                            .collect(),
                        output_type: Some(p::Type {
                            kind: Some(p::r#type::Kind::Bool(p::r#type::Boolean {
                                nullability: p::r#type::Nullability::Nullable as i32,
                                ..Default::default()
                            })),
                        }),
                        ..Default::default()
                    },
                )),
            })),
            ..Default::default()
        }))),
    };
    let plan = p::Plan {
        version: Some(p::Version {
            major_number: substrait::version::SUBSTRAIT_MAJOR_VERSION,
            minor_number: substrait::version::SUBSTRAIT_MINOR_VERSION,
            patch_number: substrait::version::SUBSTRAIT_PATCH_VERSION,
            producer: "sirius-substrait example".into(),
            ..Default::default()
        }),
        extension_urns: vec![p::extensions::SimpleExtensionUrn {
            extension_urn_anchor: 1,
            urn: "extension:io.substrait:functions_comparison".into(),
        }],
        extensions: vec![p::extensions::SimpleExtensionDeclaration {
            mapping_type: Some(
                p::extensions::simple_extension_declaration::MappingType::ExtensionFunction(
                    p::extensions::simple_extension_declaration::ExtensionFunction {
                        extension_urn_reference: 1,
                        function_anchor: 1,
                        name: "gt:i64_i64".into(),
                    },
                ),
            ),
        }],
        relations: vec![p::PlanRel {
            rel_type: Some(p::plan_rel::RelType::Root(p::RelRoot {
                input: Some(filter),
                names: vec!["id".into()],
            })),
        }],
        execution_behavior: Some(p::ExecutionBehavior {
            variable_eval_mode: p::execution_behavior::VariableEvaluationMode::PerPlan as i32,
        }),
        ..Default::default()
    };
    // Translation builds a Sirius plan before any context or GPU is initialized.
    let plan = sirius_substrait::from_bytes(&plan.encode_to_vec())?;
    let config = match std::env::args_os().nth(1) {
        Some(path) => sirius::ContextConfigBuilder::from_yaml(path)?,
        None => sirius::ContextConfigBuilder::new()?,
    }
    .build()?;
    let mut context = sirius::Context::new(&config)?;
    let output = context.execute_plan(&plan)?;
    for batch in output.batches {
        println!("{:?}", batch.column(0)); // Expected values: 20 and 30; order is unspecified.
    }
    Ok(())
}
