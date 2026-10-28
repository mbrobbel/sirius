//! Substrait consumption through Sirius's public plan builder.
//!
//! Supports one local Parquet file, nullable i64 columns, and filters comparing
//! one input column with an i64 literal using standard comparison functions.
//! Unsupported relations, projections, extensions, and types return errors.
//! No DuckDB Substrait extension is called. Translation performs no file/GPU I/O.
//!
//! Returned plans are immutable and Send + Sync. Execution borrows an exclusive,
//! thread-confined Sirius context and returns completed CPU Arrow batches.
use prost::Message;
use sirius::{Comparison, Context, I64Field, Plan, PlanError, PlanResult, Relation};
use std::collections::HashMap;
use substrait::proto as p;

/// A malformed/unsupported Substrait plan, or failure from the public Sirius API.
#[derive(Debug)]
pub enum Error {
    /// Invalid protobuf encoding.
    Decode(prost::DecodeError),
    /// Unsupported or inconsistent plan semantics.
    Unsupported(String),
    /// The public builder or execution API failed.
    Sirius(PlanError),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(e) => e.fmt(f),
            Self::Unsupported(e) => f.write_str(e),
            Self::Sirius(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<PlanError> for Error {
    fn from(e: PlanError) -> Self {
        Self::Sirius(e)
    }
}
fn unsupported(message: &str) -> Error {
    Error::Unsupported(message.into())
}
fn require(ok: bool, message: &str) -> Result<(), Error> {
    if ok {
        Ok(())
    } else {
        Err(unsupported(message))
    }
}

/// Decode a binary Substrait plan into an owned, context-independent Sirius plan.
///
/// Borrows bytes only during the call; failure releases all construction handles.
/// Independent calls may run concurrently. Input size is limited to 16 MiB and
/// relation depth to 64. The protobuf decoder additionally bounds message nesting.
///
/// ```no_run
/// # fn translate(bytes: &[u8]) -> Result<sirius::Plan, sirius_substrait::Error> {
/// sirius_substrait::from_bytes(bytes)
/// # }
/// ```
pub fn from_bytes(bytes: &[u8]) -> Result<Plan, Error> {
    require(
        bytes.len() <= 16 * 1024 * 1024,
        "Substrait plan exceeds 16 MiB",
    )?;
    let plan = p::Plan::decode(bytes).map_err(Error::Decode)?;
    from_plan(&plan)
}

/// Translate an already decoded plan. Does not retain the protobuf or perform I/O.
/// Has the same thread/ownership contract as [`from_bytes`].
pub fn from_plan(plan: &p::Plan) -> Result<Plan, Error> {
    let version = plan
        .version
        .as_ref()
        .ok_or_else(|| unsupported("Plan version is required"))?;
    require(
        version.major_number == substrait::version::SUBSTRAIT_MAJOR_VERSION
            && version.minor_number <= substrait::version::SUBSTRAIT_MINOR_VERSION,
        "Unsupported Substrait version",
    )?;
    require(
        plan.relations.len() == 1,
        "Exactly one plan root is supported",
    )?;
    require(
        plan.advanced_extensions.is_none()
            && plan.parameter_bindings.is_empty()
            && plan.type_aliases.is_empty()
            && plan.expected_type_urls.is_empty(),
        "Plan extensions/parameters are unsupported",
    )?;
    let mut urns = HashMap::new();
    for urn in &plan.extension_urns {
        require(
            urns.insert(urn.extension_urn_anchor, urn.urn.as_str())
                .is_none(),
            "Duplicate extension URN anchor",
        )?;
    }
    let mut functions = HashMap::new();
    for extension in &plan.extensions {
        let Some(p::extensions::simple_extension_declaration::MappingType::ExtensionFunction(
            function,
        )) = &extension.mapping_type
        else {
            return Err(unsupported(
                "Only comparison function extensions are supported",
            ));
        };
        require(
            urns.get(&function.extension_urn_reference)
                == Some(&"extension:io.substrait:functions_comparison"),
            "Unsupported function namespace",
        )?;
        let (name, signature) = function
            .name
            .split_once(':')
            .unwrap_or((&function.name, ""));
        require(
            matches!(signature, "" | "i64_i64" | "any_any"),
            "Unsupported comparison signature",
        )?;
        let op = match name {
            "equal" => Comparison::Equal,
            "not_equal" => Comparison::NotEqual,
            "lt" => Comparison::Less,
            "lte" => Comparison::LessEqual,
            "gt" => Comparison::Greater,
            "gte" => Comparison::GreaterEqual,
            _ => return Err(unsupported("Unsupported comparison function")),
        };
        require(
            functions.insert(function.function_anchor, op).is_none(),
            "Duplicate function anchor",
        )?;
    }
    let Some(p::plan_rel::RelType::Root(root)) = &plan.relations[0].rel_type else {
        return Err(unsupported("Expected RelRoot"));
    };
    let (relation, fields) = build(
        root.input
            .as_ref()
            .ok_or_else(|| unsupported("Missing root input"))?,
        &functions,
        0,
    )?;
    require(
        root.names == fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
        "Root renaming is not supported; names must match the scan schema",
    )?;
    Ok(Plan::from_root(&relation)?)
}

/// Translate and execute through [`Context::execute_plan`]. Blocks until output is materialized.
/// The context remains usable after ordinary translation/execution failures.
pub fn execute(context: &mut Context, bytes: &[u8]) -> Result<PlanResult, Error> {
    Ok(context.execute_plan(&from_bytes(bytes)?)?)
}

fn common(common: Option<&p::RelCommon>) -> Result<(), Error> {
    if let Some(common) = common {
        require(
            common.advanced_extension.is_none() && common.rel_anchor.is_none(),
            "Relation extensions/anchors are unsupported",
        )?;
        require(
            matches!(
                common.emit_kind,
                None | Some(p::rel_common::EmitKind::Direct(_))
            ),
            "Emit remapping is unsupported",
        )?;
    }
    Ok(())
}
fn build(
    rel: &p::Rel,
    functions: &HashMap<u32, Comparison>,
    depth: usize,
) -> Result<(Relation, Vec<I64Field>), Error> {
    require(depth < 64, "Relation nesting exceeds 64")?;
    match &rel.rel_type {
        Some(p::rel::RelType::Read(read)) => {
            common(read.common.as_ref())?;
            require(
                read.projection.is_none() && read.advanced_extension.is_none(),
                "Read projection/extensions are unsupported",
            )?;
            let schema = read
                .base_schema
                .as_ref()
                .ok_or_else(|| unsupported("Read requires an explicit schema"))?;
            let structure = schema
                .r#struct
                .as_ref()
                .ok_or_else(|| unsupported("Missing schema struct"))?;
            require(
                structure.type_variation_reference == 0
                    && structure.nullability == p::r#type::Nullability::Required as i32
                    && structure.types.len() == schema.names.len(),
                "Unsupported schema variation or names",
            )?;
            let mut fields = Vec::new();
            for (name, ty) in schema.names.iter().zip(&structure.types) {
                let Some(p::r#type::Kind::I64(ty)) = &ty.kind else {
                    return Err(unsupported("Only nullable i64 columns are supported"));
                };
                require(
                    ty.type_variation_reference == 0
                        && ty.nullability == p::r#type::Nullability::Nullable as i32,
                    "Only nullable default-variation i64 columns are supported",
                )?;
                fields.push(I64Field::new(name, true));
            }
            let Some(p::read_rel::ReadType::LocalFiles(files)) = &read.read_type else {
                return Err(unsupported("Only local_files reads are supported"));
            };
            require(
                files.items.len() == 1 && files.advanced_extension.is_none(),
                "Exactly one local file without extensions is supported",
            )?;
            let file = &files.items[0];
            require(
                file.start == 0 && file.length == 0 && file.partition_index == 0,
                "Byte ranges and partitioned reads are unsupported",
            )?;
            use p::read_rel::local_files::file_or_files::{FileFormat, PathType};
            require(
                matches!(file.file_format, Some(FileFormat::Parquet(_))),
                "Only Parquet is supported",
            )?;
            let Some(PathType::UriFile(uri)) = &file.path_type else {
                return Err(unsupported("Expected uri_file"));
            };
            let url =
                url::Url::parse(uri).map_err(|_| unsupported("Expected an absolute file URI"))?;
            require(
                url.scheme() == "file" && url.query().is_none() && url.fragment().is_none(),
                "Expected a local file URI without query/fragment",
            )?;
            let path = url
                .to_file_path()
                .map_err(|_| unsupported("Invalid local file URI"))?;
            let mut relation = Relation::parquet(path, &fields)?;
            if let Some(predicate) = &read.filter {
                relation = filter(relation, predicate, functions)?;
            }
            // A best-effort filter is optional by definition; correctness depends only on filter.
            Ok((relation, fields))
        }
        Some(p::rel::RelType::Filter(node)) => {
            common(node.common.as_ref())?;
            require(
                node.advanced_extension.is_none(),
                "Filter extensions are unsupported",
            )?;
            let (relation, fields) = build(
                node.input
                    .as_ref()
                    .ok_or_else(|| unsupported("Missing filter input"))?,
                functions,
                depth + 1,
            )?;
            Ok((
                filter(
                    relation,
                    node.condition
                        .as_ref()
                        .ok_or_else(|| unsupported("Missing filter condition"))?,
                    functions,
                )?,
                fields,
            ))
        }
        _ => Err(unsupported("Only ReadRel and FilterRel are supported")),
    }
}
fn filter(
    input: Relation,
    expression: &p::Expression,
    functions: &HashMap<u32, Comparison>,
) -> Result<Relation, Error> {
    use p::expression::{
        RexType,
        field_reference::{ReferenceType, RootType},
        reference_segment::ReferenceType as Segment,
    };
    let Some(RexType::ScalarFunction(call)) = &expression.rex_type else {
        return Err(unsupported("Expected a comparison function"));
    };
    require(
        call.arguments.len() == 2 && call.options.is_empty(),
        "Comparison requires two value arguments and no options",
    )?;
    let Some(p::r#type::Kind::Bool(output)) =
        call.output_type.as_ref().and_then(|t| t.kind.as_ref())
    else {
        return Err(unsupported("Comparison must return boolean"));
    };
    require(
        output.type_variation_reference == 0
            && output.nullability == p::r#type::Nullability::Nullable as i32,
        "Comparison output must be nullable boolean",
    )?;
    let op = *functions
        .get(&call.function_reference)
        .ok_or_else(|| unsupported("Unknown comparison anchor"))?;
    let Some(p::function_argument::ArgType::Value(left)) = &call.arguments[0].arg_type else {
        return Err(unsupported("Expected column argument"));
    };
    let Some(RexType::Selection(field)) = &left.rex_type else {
        return Err(unsupported(
            "Left comparison argument must be an input column",
        ));
    };
    require(
        matches!(field.root_type, Some(RootType::RootReference(_))),
        "Only input-root field references are supported",
    )?;
    let Some(ReferenceType::DirectReference(segment)) = &field.reference_type else {
        return Err(unsupported("Expected direct field reference"));
    };
    let Some(Segment::StructField(field)) = &segment.reference_type else {
        return Err(unsupported("Expected struct field"));
    };
    require(
        field.field >= 0 && field.child.is_none(),
        "Nested or negative field references are unsupported",
    )?;
    let Some(p::function_argument::ArgType::Value(right)) = &call.arguments[1].arg_type else {
        return Err(unsupported("Expected literal argument"));
    };
    let Some(RexType::Literal(literal)) = &right.rex_type else {
        return Err(unsupported(
            "Right comparison argument must be an i64 literal",
        ));
    };
    require(
        literal.type_variation_reference == 0,
        "Literal type variations are unsupported",
    )?;
    let Some(p::expression::literal::LiteralType::I64(value)) = literal.literal_type else {
        return Err(unsupported("Expected i64 literal"));
    };
    let predicate = input.compare_i64(field.field as usize, op, value)?;
    Ok(input.filter(&predicate)?)
}
