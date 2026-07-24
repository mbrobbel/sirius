//! Execution of a translated fragment into Arrow result batches.
//!
//! The engine→CN result interchange is the Arrow C Data Interface. A [`StubExecutor`] stands in
//! for the GPU coordinator in no-engine tests so StarRocks dispatch and result-return plumbing can
//! be exercised without a build tree or GPU.

use std::sync::Arc;

use arrow_array::{ArrayRef, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use starrocks_plan_translator::TranslatedPlan;

use crate::exchange::FragmentExchange;

/// Output of executing one plan fragment: Arrow batches matching the fragment output schema.
#[derive(Clone, Debug)]
pub struct FragmentResult {
    /// Result batches in fragment output order. Empty for a fragment with no output columns.
    pub(crate) batches: Vec<RecordBatch>,
}

impl FragmentResult {
    /// Builds a result from its output batches (in fragment output order).
    pub fn new(batches: Vec<RecordBatch>) -> Self {
        Self { batches }
    }

    /// The result batches in fragment output order.
    pub fn batches(&self) -> &[RecordBatch] {
        &self.batches
    }
}

/// Runs a translated fragment and returns its result batches.
///
/// This remains a synchronous, fully-materializing RPC seam: `exec_plan_fragment` waits for the
/// engine coordinator, while exchange transport work inside that coordinator is asynchronous.
///
/// TODO(starrocks-execute): dispatch should eventually register a cancellable running fragment and
/// return after startup, with result batches streamed into `ResultStore` instead of materialized.
pub trait FragmentExecutor: std::fmt::Debug + Send + Sync {
    /// Executes `translated` with StarRocks exchange semantics owned by the compute node.
    fn execute(
        &self,
        translated: &TranslatedPlan,
        exchange: &FragmentExchange,
    ) -> Result<FragmentResult, String>;
}

/// Placeholder executor that fabricates one row so the result path works without a GPU.
#[derive(Clone, Copy, Debug, Default)]
pub struct StubExecutor;

impl FragmentExecutor for StubExecutor {
    fn execute(
        &self,
        translated: &TranslatedPlan,
        _exchange: &FragmentExchange,
    ) -> Result<FragmentResult, String> {
        // Emit one placeholder string row per output column so the FE→client path is exercised.
        let names = &translated.output_names;
        if names.is_empty() {
            return Ok(FragmentResult {
                batches: Vec::new(),
            });
        }
        let fields: Vec<Field> = names
            .iter()
            .map(|name| Field::new(name, DataType::Utf8, true))
            .collect();
        let columns: Vec<ArrayRef> = names
            .iter()
            .map(|_| Arc::new(StringArray::from(vec![Some("stub")])) as ArrayRef)
            .collect();
        let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)
            .map_err(|err| format!("failed to build stub result batch: {err}"))?;
        Ok(FragmentResult {
            batches: vec![batch],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_with_outputs(names: &[&str]) -> TranslatedPlan {
        TranslatedPlan {
            plan: Default::default(),
            output_names: names.iter().map(|name| name.to_string()).collect(),
            exchange_inputs: Vec::new(),
        }
    }

    #[test]
    fn stub_executor_emits_one_row_matching_output_names() {
        let result = StubExecutor
            .execute(
                &plan_with_outputs(&["id", "name"]),
                &FragmentExchange::default(),
            )
            .unwrap();
        assert_eq!(result.batches.len(), 1);
        let batch = &result.batches[0];
        assert_eq!(batch.num_columns(), 2);
        assert_eq!(batch.num_rows(), 1);
        assert_eq!(batch.schema().field(0).name(), "id");
        assert_eq!(batch.schema().field(1).name(), "name");
    }

    #[test]
    fn stub_executor_handles_empty_output() {
        let result = StubExecutor
            .execute(&plan_with_outputs(&[]), &FragmentExchange::default())
            .unwrap();
        assert!(result.batches.is_empty());
    }
}
