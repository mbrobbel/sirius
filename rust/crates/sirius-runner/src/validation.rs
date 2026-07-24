use std::{
    cmp::Ordering,
    collections::{BTreeMap, VecDeque},
};

use serde::{Deserialize, Serialize};

use crate::{
    assets::CompareStrategy,
    expected_cache::{ExpectedResult, ResultRow, ResultValue},
    model::QueryValidationPlan,
};

pub const VALIDATION_PROTOCOL_VERSION: u32 = 1;
pub const FLOAT_ABS_TOLERANCE: f64 = 1e-10;
pub const FLOAT_REL_TOLERANCE: f64 = 1e-9;
const MAX_EXAMPLES: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationOutcome {
    pub protocol_version: u32,
    pub matched: bool,
    pub policy: String,
    pub expected_digest: String,
    pub actual_digest: String,
    pub expected_rows: u64,
    pub actual_rows: u64,
    pub mismatch_count: u64,
    pub examples: Vec<MismatchExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MismatchExample {
    pub row: usize,
    pub expected: Option<ResultRow>,
    pub actual: Option<ResultRow>,
}

pub fn compare(
    expected: &ExpectedResult,
    actual: &ExpectedResult,
    policy: &QueryValidationPlan,
) -> ValidationOutcome {
    if policy.compare == CompareStrategy::Digest {
        let matched = expected.digest == actual.digest;
        return outcome(
            expected,
            actual,
            u64::from(!matched),
            Vec::new(),
            "exact typed-result digest; order=significant".to_string(),
        );
    }

    let (absolute_tolerance, relative_tolerance) = policy
        .float_tolerance
        .map_or((FLOAT_ABS_TOLERANCE, FLOAT_REL_TOLERANCE), |tolerance| {
            (tolerance, tolerance)
        });
    let description = format!(
        "{} typed rows; exact non-floats; abs_tol={absolute_tolerance}; rel_tol={relative_tolerance}",
        if policy.ordered_results {
            "ordered"
        } else {
            "unordered"
        }
    );
    if expected == actual {
        return outcome(expected, actual, 0, Vec::new(), description);
    }
    if expected.schema != actual.schema {
        return outcome(
            expected,
            actual,
            expected.row_count.max(actual.row_count),
            vec![MismatchExample {
                row: 0,
                expected: expected.rows.first().cloned(),
                actual: actual.rows.first().cloned(),
            }],
            description,
        );
    }

    if !policy.ordered_results {
        let (mismatch_count, examples) = compare_unordered_rows(
            &expected.rows,
            &actual.rows,
            absolute_tolerance,
            relative_tolerance,
        );
        return outcome(expected, actual, mismatch_count, examples, description);
    }

    let expected_rows = &expected.rows;
    let actual_rows = &actual.rows;
    let length = expected_rows.len().max(actual_rows.len());
    let mut mismatch_count = 0_u64;
    let mut examples = Vec::new();
    for index in 0..length {
        let expected_row = expected_rows.get(index);
        let actual_row = actual_rows.get(index);
        if !matches!(
            (expected_row, actual_row),
            (Some(expected), Some(actual))
                if rows_match(expected, actual, absolute_tolerance, relative_tolerance)
        ) {
            mismatch_count += 1;
            if examples.len() < MAX_EXAMPLES {
                examples.push(MismatchExample {
                    row: index,
                    expected: expected_row.cloned(),
                    actual: actual_row.cloned(),
                });
            }
        }
    }
    outcome(expected, actual, mismatch_count, examples, description)
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ValueStructure<'a> {
    Null,
    Boolean(bool),
    Integer(&'a str),
    UnsignedInteger(&'a str),
    Float,
    Decimal(&'a str),
    Text(&'a str),
    Blob(&'a str),
    Date(&'a str),
    Time(&'a str),
    Timestamp(&'a str),
    Interval(&'a str),
    Uuid(&'a str),
    Json(&'a str),
    List(Vec<ValueStructure<'a>>),
    Struct(Vec<(&'a str, ValueStructure<'a>)>),
    Map(Vec<(ValueStructure<'a>, ValueStructure<'a>)>),
}

#[derive(Debug, Default)]
struct RowGroup<'a> {
    expected: Vec<(usize, &'a ResultRow)>,
    actual: Vec<(usize, &'a ResultRow)>,
}

fn compare_unordered_rows<'a>(
    expected_rows: &'a [ResultRow],
    actual_rows: &'a [ResultRow],
    absolute_tolerance: f64,
    relative_tolerance: f64,
) -> (u64, Vec<MismatchExample>) {
    let mut groups: BTreeMap<Vec<ValueStructure<'a>>, RowGroup<'a>> = BTreeMap::new();
    for (index, row) in expected_rows.iter().enumerate() {
        groups
            .entry(row_structure(row))
            .or_default()
            .expected
            .push((index, row));
    }
    for (index, row) in actual_rows.iter().enumerate() {
        groups
            .entry(row_structure(row))
            .or_default()
            .actual
            .push((index, row));
    }

    let mut matched_count = 0;
    let mut unmatched_expected = Vec::new();
    let mut unmatched_actual = Vec::new();
    for group in groups.values_mut() {
        sort_indexed_rows(&mut group.expected);
        sort_indexed_rows(&mut group.actual);

        let pairwise_count = group.expected.len().min(group.actual.len());
        if group
            .expected
            .iter()
            .zip(&group.actual)
            .all(|((_, expected), (_, actual))| {
                rows_match(expected, actual, absolute_tolerance, relative_tolerance)
            })
        {
            matched_count += pairwise_count;
            unmatched_expected.extend(group.expected[pairwise_count..].iter().copied());
            unmatched_actual.extend(group.actual[pairwise_count..].iter().copied());
            continue;
        }

        let adjacency = group
            .expected
            .iter()
            .map(|(_, expected)| {
                group
                    .actual
                    .iter()
                    .enumerate()
                    .filter_map(|(actual_index, (_, actual))| {
                        rows_match(expected, actual, absolute_tolerance, relative_tolerance)
                            .then_some(actual_index)
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let (expected_matches, actual_matches) =
            maximum_bipartite_matching(&adjacency, group.actual.len());
        matched_count += expected_matches
            .iter()
            .filter(|item| item.is_some())
            .count();
        unmatched_expected.extend(
            expected_matches
                .iter()
                .enumerate()
                .filter_map(|(index, matched)| matched.is_none().then_some(group.expected[index])),
        );
        unmatched_actual.extend(
            actual_matches
                .iter()
                .enumerate()
                .filter_map(|(index, matched)| matched.is_none().then_some(group.actual[index])),
        );
    }

    unmatched_expected.sort_by_key(|(index, _)| *index);
    unmatched_actual.sort_by_key(|(index, _)| *index);
    let mismatch_count = expected_rows.len().max(actual_rows.len()) - matched_count;
    let examples = (0..mismatch_count.min(MAX_EXAMPLES))
        .map(|index| {
            let expected = unmatched_expected.get(index).copied();
            let actual = unmatched_actual.get(index).copied();
            MismatchExample {
                row: expected
                    .map(|(row, _)| row)
                    .or_else(|| actual.map(|(row, _)| row))
                    .unwrap_or(index),
                expected: expected.map(|(_, row)| row.clone()),
                actual: actual.map(|(_, row)| row.clone()),
            }
        })
        .collect();
    (mismatch_count as u64, examples)
}

fn row_structure(row: &ResultRow) -> Vec<ValueStructure<'_>> {
    row.0.iter().map(value_structure).collect()
}

fn value_structure(value: &ResultValue) -> ValueStructure<'_> {
    match value {
        ResultValue::Null => ValueStructure::Null,
        ResultValue::Boolean(value) => ValueStructure::Boolean(*value),
        ResultValue::Integer(value) => ValueStructure::Integer(value),
        ResultValue::UnsignedInteger(value) => ValueStructure::UnsignedInteger(value),
        ResultValue::Float(_) => ValueStructure::Float,
        ResultValue::Decimal(value) => ValueStructure::Decimal(value),
        ResultValue::Text(value) => ValueStructure::Text(value),
        ResultValue::Blob(value) => ValueStructure::Blob(value),
        ResultValue::Date(value) => ValueStructure::Date(value),
        ResultValue::Time(value) => ValueStructure::Time(value),
        ResultValue::Timestamp(value) => ValueStructure::Timestamp(value),
        ResultValue::Interval(value) => ValueStructure::Interval(value),
        ResultValue::Uuid(value) => ValueStructure::Uuid(value),
        ResultValue::Json(value) => ValueStructure::Json(value),
        ResultValue::List(values) => {
            ValueStructure::List(values.iter().map(value_structure).collect())
        }
        ResultValue::Struct(fields) => ValueStructure::Struct(
            fields
                .iter()
                .map(|(name, value)| (name.as_str(), value_structure(value)))
                .collect(),
        ),
        ResultValue::Map(entries) => ValueStructure::Map(
            entries
                .iter()
                .map(|entry| (value_structure(&entry.key), value_structure(&entry.value)))
                .collect(),
        ),
    }
}

fn sort_indexed_rows(rows: &mut [(usize, &ResultRow)]) {
    rows.sort_by(|(left_index, left), (right_index, right)| {
        compare_rows(left, right).then_with(|| left_index.cmp(right_index))
    });
}

fn maximum_bipartite_matching(
    adjacency: &[Vec<usize>],
    actual_count: usize,
) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
    let mut expected_matches = vec![None; adjacency.len()];
    let mut actual_matches = vec![None; actual_count];
    let mut distance = vec![usize::MAX; adjacency.len()];

    while matching_layers(adjacency, &expected_matches, &actual_matches, &mut distance) {
        for expected in 0..adjacency.len() {
            if expected_matches[expected].is_none() {
                try_augment(
                    expected,
                    adjacency,
                    &mut expected_matches,
                    &mut actual_matches,
                    &mut distance,
                );
            }
        }
    }

    (expected_matches, actual_matches)
}

fn matching_layers(
    adjacency: &[Vec<usize>],
    expected_matches: &[Option<usize>],
    actual_matches: &[Option<usize>],
    distance: &mut [usize],
) -> bool {
    let mut queue = VecDeque::new();
    for (expected, matched) in expected_matches.iter().enumerate() {
        if matched.is_none() {
            distance[expected] = 0;
            queue.push_back(expected);
        } else {
            distance[expected] = usize::MAX;
        }
    }

    let mut found_augmenting_path = false;
    while let Some(expected) = queue.pop_front() {
        for &actual in &adjacency[expected] {
            match actual_matches[actual] {
                Some(next_expected) if distance[next_expected] == usize::MAX => {
                    distance[next_expected] = distance[expected] + 1;
                    queue.push_back(next_expected);
                }
                None => found_augmenting_path = true,
                Some(_) => {}
            }
        }
    }
    found_augmenting_path
}

fn try_augment(
    expected: usize,
    adjacency: &[Vec<usize>],
    expected_matches: &mut [Option<usize>],
    actual_matches: &mut [Option<usize>],
    distance: &mut [usize],
) -> bool {
    for &actual in &adjacency[expected] {
        let can_augment = match actual_matches[actual] {
            None => true,
            Some(next_expected) if distance[next_expected] == distance[expected] + 1 => {
                try_augment(
                    next_expected,
                    adjacency,
                    expected_matches,
                    actual_matches,
                    distance,
                )
            }
            Some(_) => false,
        };
        if can_augment {
            expected_matches[expected] = Some(actual);
            actual_matches[actual] = Some(expected);
            return true;
        }
    }
    distance[expected] = usize::MAX;
    false
}

fn outcome(
    expected: &ExpectedResult,
    actual: &ExpectedResult,
    mismatch_count: u64,
    examples: Vec<MismatchExample>,
    policy: String,
) -> ValidationOutcome {
    ValidationOutcome {
        protocol_version: VALIDATION_PROTOCOL_VERSION,
        matched: mismatch_count == 0 && expected.schema == actual.schema,
        policy,
        expected_digest: expected.digest.to_string(),
        actual_digest: actual.digest.to_string(),
        expected_rows: expected.row_count,
        actual_rows: actual.row_count,
        mismatch_count,
        examples,
    }
}

fn rows_match(
    expected: &ResultRow,
    actual: &ResultRow,
    absolute_tolerance: f64,
    relative_tolerance: f64,
) -> bool {
    expected.0.len() == actual.0.len()
        && expected.0.iter().zip(&actual.0).all(|(expected, actual)| {
            values_match(expected, actual, absolute_tolerance, relative_tolerance)
        })
}

fn values_match(
    expected: &ResultValue,
    actual: &ResultValue,
    absolute_tolerance: f64,
    relative_tolerance: f64,
) -> bool {
    match (expected, actual) {
        (ResultValue::Float(expected), ResultValue::Float(actual)) => {
            floats_match(expected, actual, absolute_tolerance, relative_tolerance)
        }
        (ResultValue::List(expected), ResultValue::List(actual)) => {
            expected.len() == actual.len()
                && expected.iter().zip(actual).all(|(expected, actual)| {
                    values_match(expected, actual, absolute_tolerance, relative_tolerance)
                })
        }
        (ResultValue::Struct(expected), ResultValue::Struct(actual)) => {
            expected.len() == actual.len()
                && expected.iter().all(|(name, expected)| {
                    actual.get(name).is_some_and(|actual| {
                        values_match(expected, actual, absolute_tolerance, relative_tolerance)
                    })
                })
        }
        (ResultValue::Map(expected), ResultValue::Map(actual)) => {
            expected.len() == actual.len()
                && expected.iter().zip(actual).all(|(expected, actual)| {
                    values_match(
                        &expected.key,
                        &actual.key,
                        absolute_tolerance,
                        relative_tolerance,
                    ) && values_match(
                        &expected.value,
                        &actual.value,
                        absolute_tolerance,
                        relative_tolerance,
                    )
                })
        }
        _ => expected == actual,
    }
}

fn floats_match(
    expected: &str,
    actual: &str,
    absolute_tolerance: f64,
    relative_tolerance: f64,
) -> bool {
    if expected == actual {
        return true;
    }
    let (Ok(expected), Ok(actual)) = (expected.parse::<f64>(), actual.parse::<f64>()) else {
        return false;
    };
    if !expected.is_finite() || !actual.is_finite() {
        return false;
    }
    let difference = (expected - actual).abs();
    difference <= absolute_tolerance
        || difference <= relative_tolerance * expected.abs().max(actual.abs())
}

fn compare_rows(left: &ResultRow, right: &ResultRow) -> Ordering {
    for (left, right) in left.0.iter().zip(&right.0) {
        let ordering = compare_values(left, right);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.0.len().cmp(&right.0.len())
}

fn compare_values(left: &ResultValue, right: &ResultValue) -> Ordering {
    use ResultValue as Value;
    match (left, right) {
        (Value::Float(left), Value::Float(right)) => compare_floats(left, right),
        (Value::List(left), Value::List(right)) => compare_value_slices(left, right),
        _ => {
            let left = serde_json::to_string(left).unwrap_or_default();
            let right = serde_json::to_string(right).unwrap_or_default();
            left.cmp(&right)
        }
    }
}

fn compare_value_slices(left: &[ResultValue], right: &[ResultValue]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let ordering = compare_values(left, right);
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn compare_floats(left: &str, right: &str) -> Ordering {
    match (left.parse::<f64>(), right.parse::<f64>()) {
        (Ok(left), Ok(right)) => left.total_cmp(&right),
        _ => left.cmp(right),
    }
}

#[cfg(test)]
mod tests {
    use crate::assets::CompareStrategy;
    use crate::expected_cache::{ResultColumn, ResultSchema};

    use super::*;

    fn result(rows: Vec<ResultRow>) -> ExpectedResult {
        ExpectedResult::new(
            ResultSchema::new(vec![
                ResultColumn::new("id", "INTEGER").unwrap(),
                ResultColumn::new("metric", "DOUBLE").unwrap(),
            ])
            .unwrap(),
            rows,
        )
        .unwrap()
    }

    fn rows_policy() -> QueryValidationPlan {
        QueryValidationPlan {
            compare: CompareStrategy::Rows,
            float_tolerance: None,
            ordered_results: false,
        }
    }

    #[test]
    fn comparison_is_unordered_and_float_tolerant() {
        let first = ResultRow(vec![
            ResultValue::Integer("1".into()),
            ResultValue::Float("1.0".into()),
        ]);
        let second = ResultRow(vec![
            ResultValue::Integer("2".into()),
            ResultValue::Float("2.0".into()),
        ]);
        let expected = result(vec![first.clone(), second.clone()]);
        let actual = result(vec![
            ResultRow(vec![
                ResultValue::Integer("2".into()),
                ResultValue::Float("2.0000000001".into()),
            ]),
            first,
        ]);
        assert!(compare(&expected, &actual, &rows_policy()).matched);
    }

    #[test]
    fn comparison_reports_bounded_examples() {
        let expected = result(vec![ResultRow(vec![
            ResultValue::Integer("1".into()),
            ResultValue::Float("1".into()),
        ])]);
        let actual = result(vec![ResultRow(vec![
            ResultValue::Integer("9".into()),
            ResultValue::Float("9".into()),
        ])]);
        let outcome = compare(&expected, &actual, &rows_policy());
        assert!(!outcome.matched);
        assert_eq!(outcome.mismatch_count, 1);
        assert_eq!(outcome.examples.len(), 1);
    }

    #[test]
    fn manifest_tolerance_and_digest_policy_are_honored() {
        let expected = result(vec![ResultRow(vec![
            ResultValue::Integer("1".into()),
            ResultValue::Float("1".into()),
        ])]);
        let close = result(vec![ResultRow(vec![
            ResultValue::Integer("1".into()),
            ResultValue::Float("1.01".into()),
        ])]);
        assert!(
            compare(
                &expected,
                &close,
                &QueryValidationPlan {
                    compare: CompareStrategy::Rows,
                    float_tolerance: Some(0.02),
                    ordered_results: false,
                },
            )
            .matched
        );

        let digest = compare(
            &expected,
            &close,
            &QueryValidationPlan {
                compare: CompareStrategy::Digest,
                float_tolerance: None,
                ordered_results: true,
            },
        );
        assert!(!digest.matched);
        assert_eq!(
            digest.policy,
            "exact typed-result digest; order=significant"
        );
        assert!(digest.examples.is_empty());
    }

    #[test]
    fn ordered_policy_detects_a_row_order_regression() {
        let first = ResultRow(vec![
            ResultValue::Integer("1".into()),
            ResultValue::Float("1".into()),
        ]);
        let second = ResultRow(vec![
            ResultValue::Integer("2".into()),
            ResultValue::Float("2".into()),
        ]);
        let expected = result(vec![first.clone(), second.clone()]);
        let actual = result(vec![second, first]);
        let outcome = compare(
            &expected,
            &actual,
            &QueryValidationPlan {
                compare: CompareStrategy::Rows,
                float_tolerance: None,
                ordered_results: true,
            },
        );
        assert!(!outcome.matched);
        assert_eq!(outcome.mismatch_count, 2);
    }

    #[test]
    fn unordered_matching_handles_tolerance_valid_crossed_rows() {
        let schema = ResultSchema::new(vec![
            ResultColumn::new("first", "DOUBLE").unwrap(),
            ResultColumn::new("second", "DOUBLE").unwrap(),
        ])
        .unwrap();
        let expected = ExpectedResult::new(
            schema.clone(),
            vec![
                ResultRow(vec![
                    ResultValue::Float("0".into()),
                    ResultValue::Float("10".into()),
                ]),
                ResultRow(vec![
                    ResultValue::Float("0.05".into()),
                    ResultValue::Float("0".into()),
                ]),
            ],
        )
        .unwrap();
        let actual = ExpectedResult::new(
            schema,
            vec![
                ResultRow(vec![
                    ResultValue::Float("0.01".into()),
                    ResultValue::Float("0".into()),
                ]),
                ResultRow(vec![
                    ResultValue::Float("0.04".into()),
                    ResultValue::Float("10".into()),
                ]),
            ],
        )
        .unwrap();

        let outcome = compare(
            &expected,
            &actual,
            &QueryValidationPlan {
                compare: CompareStrategy::Rows,
                float_tolerance: Some(0.1),
                ordered_results: false,
            },
        );
        assert!(outcome.matched);
        assert_eq!(outcome.mismatch_count, 0);
        assert!(outcome.examples.is_empty());
    }
}
