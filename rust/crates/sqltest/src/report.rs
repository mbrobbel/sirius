use crate::{
    config::{Name, Target, axis_label},
    corpus::{Case, ConnectionId, Execution},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Match,
    Mismatch,
    Error,
    Timeout,
    Crash,
    ReferenceFailure,
    InfrastructureFailure,
    HarnessError,
    Blocked,
    UnmetRequirement,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseReport {
    pub id: String,
    #[serde(default)]
    pub suite: String,
    pub file: String,
    pub line: u32,
    pub tags: Vec<String>,
    pub profile: String,
    pub storage: String,
    #[serde(default)]
    pub axes: BTreeMap<Name, Name>,
    pub fingerprint: String,
    pub outcome: Outcome,
    #[serde(default)]
    pub execution: Execution,
    #[serde(default)]
    pub connection: ConnectionId,
    pub message: String,
    pub reference_rows: Option<usize>,
    pub expected_gap: Option<String>,
    pub unexpected_pass: bool,
    pub artifacts: String,
}

impl CaseReport {
    pub fn new(case: &Case, target: &Target, fingerprint: String, artifacts: String) -> Self {
        Self {
            id: case.id.clone(),
            suite: target.suite.to_string(),
            file: case.file.clone(),
            line: case.line,
            tags: case.tags.clone(),
            profile: target.profile.to_string(),
            storage: target.storage.to_string(),
            axes: target.axes.clone(),
            fingerprint,
            outcome: Outcome::Blocked,
            execution: case.execution,
            connection: case.connection.clone(),
            message: String::new(),
            reference_rows: None,
            expected_gap: None,
            unexpected_pass: false,
            artifacts,
        }
    }
    pub fn configuration(&self) -> String {
        if self.axes.is_empty() {
            format!("{} / {}", self.profile, self.storage)
        } else {
            axis_label(&self.axes)
        }
    }
    pub fn key(&self) -> String {
        if self.axes.is_empty() {
            return format!("{}:{}:{}", self.id, self.profile, self.storage);
        }
        format!(
            "{}:{}",
            self.id,
            serde_json::to_string(&self.axes).expect("axis names serialize as strings")
        )
    }
    pub fn failed(&self) -> bool {
        self.unexpected_pass || (self.outcome != Outcome::Match && self.expected_gap.is_none())
    }
}

#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Provenance {
    pub extension_sha256: Option<String>,
    pub runtime_sha256: String,
    pub runtime_revision: String,
    pub duckdb_version: String,
    pub runner_revision: String,
    pub runner_sha256: String,
    pub suite_revision: String,
    pub source_run: Option<String>,
    pub source_commit: Option<String>,
    pub device: String,
    pub cpu_only: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub comparison_version: u32,
    pub created_unix: u64,
    pub complete: bool,
    pub provenance: Provenance,
    pub execution_evidence: String,
    pub cases: Vec<CaseReport>,
}

impl Report {
    pub fn new(provenance: Provenance) -> Self {
        Self {
            schema_version: 1,
            comparison_version: 2,
            created_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            complete: false,
            provenance,
            execution_evidence:
                "GPU execution unverified; each case records whether CPU fallback is allowed".into(),
            cases: Vec::new(),
        }
    }

    pub fn failed(&self) -> bool {
        !self.complete || self.cases.iter().any(CaseReport::failed)
    }

    pub fn save(&self, directory: &Path, previous: Option<&Report>) -> Result<()> {
        fs::create_dir_all(directory)?;
        let temporary = directory.join("report.json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temporary, directory.join("report.json"))?;
        fs::write(directory.join("summary.md"), self.markdown(previous))?;
        let failures = self.cases.iter().filter(|c| c.failed()).count();
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"sirius-sqltest\" tests=\"{}\" failures=\"{failures}\">\n",
            self.cases.len()
        );
        for case in &self.cases {
            xml.push_str(&format!(
                "  <testcase name=\"{}\" classname=\"{}\">",
                escape(&case.id),
                escape(&case.configuration())
            ));
            if case.failed() {
                xml.push_str(&format!(
                    "<failure message=\"{:?}\">{}</failure>",
                    case.outcome,
                    escape(&case.message)
                ));
            } else if let Some(issue) = &case.expected_gap {
                xml.push_str(&format!(
                    "<skipped message=\"known gap: {}\"/>",
                    escape(issue)
                ));
            }
            xml.push_str("</testcase>\n");
        }
        xml.push_str("</testsuite>\n");
        fs::write(directory.join("junit.xml"), xml)?;
        Ok(())
    }

    pub fn markdown(&self, previous: Option<&Report>) -> String {
        let mut counts = BTreeMap::new();
        for case in &self.cases {
            *counts
                .entry(format!("{:?}", case.outcome))
                .or_insert(0usize) += 1;
        }
        let mut output = format!(
            "# Sirius SQL correctness\n\nComplete: **{}** · Cases: **{}** · Known gaps: **{}** · Unexpected passes: **{}** · Empty reference results: **{}**\n\n{}\n\nDuckDB: `{}` · Source: `{}` · Runner: `{}`\n\n| Outcome | Count |\n| --- | ---: |\n",
            self.complete,
            self.cases.len(),
            self.cases
                .iter()
                .filter(|c| c.expected_gap.is_some())
                .count(),
            self.cases.iter().filter(|c| c.unexpected_pass).count(),
            self.cases
                .iter()
                .filter(|c| c.reference_rows == Some(0))
                .count(),
            self.execution_evidence,
            self.provenance.duckdb_version,
            self.provenance.source_commit.as_deref().unwrap_or("local"),
            self.provenance.runner_revision
        );
        for (outcome, count) in counts {
            output.push_str(&format!("| {outcome} | {count} |\n"));
        }
        output.push_str("\n## Suites\n\n| Suite | Configuration | Execution policy | Matches | Total | Empty reference results |\n| --- | --- | --- | ---: | ---: | ---: |\n");
        let mut suites = BTreeMap::<_, (usize, usize, usize)>::new();
        for case in &self.cases {
            let suite = case.suite.as_str();
            let entry = suites
                .entry((suite, case.configuration(), case.execution))
                .or_default();
            entry.0 += usize::from(case.outcome == Outcome::Match);
            entry.1 += 1;
            entry.2 += usize::from(case.reference_rows == Some(0));
        }
        for ((suite, configuration, execution), (matches, total, empty)) in suites {
            output.push_str(&format!(
                "| {suite} | {configuration} | {} | {matches} | {total} | {empty} |\n",
                execution.label()
            ));
        }
        if let Some(previous) = previous {
            output.push_str(&self.history(previous));
        }
        output.push_str("\n## Cases requiring attention\n\n| Case | Configuration | Outcome | Detail |\n| --- | --- | --- | --- |\n");
        for case in self.cases.iter().filter(|c| c.failed()) {
            output.push_str(&format!(
                "| [{}]({}/result.json) | {} | {:?}{} | {} |\n",
                case.id,
                case.artifacts,
                case.configuration(),
                case.outcome,
                if case.unexpected_pass {
                    " (unexpected pass)"
                } else {
                    ""
                },
                case.message
                    .replace('|', "\\|")
                    .replace(['\n', '\r'], " ")
                    .chars()
                    .take(300)
                    .collect::<String>()
            ));
        }
        output.push_str("\n## Feature tags\n\nCounts describe this corpus, not SQL feature completeness.\n\n| Tag | Matches | Total |\n| --- | ---: | ---: |\n");
        let mut tags: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for case in &self.cases {
            for tag in &case.tags {
                let entry = tags.entry(tag).or_default();
                entry.0 += usize::from(case.outcome == Outcome::Match);
                entry.1 += 1;
            }
        }
        for (tag, (matches, total)) in tags {
            output.push_str(&format!("| {tag} | {matches} | {total} |\n"));
        }
        output
    }

    fn history(&self, previous: &Report) -> String {
        if !previous.complete
            || previous.schema_version != self.schema_version
            || previous.comparison_version != self.comparison_version
            || previous.provenance.runtime_sha256 != self.provenance.runtime_sha256
            || previous.provenance.cpu_only != self.provenance.cpu_only
            || previous.provenance.device != self.provenance.device
        {
            return "\nHistory: prior report is incomplete or uses a different runtime, comparator, or device.\n".into();
        }
        let before: BTreeMap<_, _> = previous.cases.iter().map(|c| (c.key(), c)).collect();
        let now: BTreeMap<_, _> = self.cases.iter().map(|c| (c.key(), c)).collect();
        let (mut regressions, mut improvements, mut changed, mut added) = (0, 0, 0, 0);
        for (key, case) in &now {
            match before.get(key) {
                None => added += 1,
                Some(old) if old.fingerprint != case.fingerprint => changed += 1,
                Some(old) => {
                    regressions += usize::from(
                        old.outcome == Outcome::Match && case.outcome != Outcome::Match,
                    );
                    improvements += usize::from(
                        old.outcome != Outcome::Match && case.outcome == Outcome::Match,
                    );
                }
            }
        }
        let removed = before.keys().filter(|k| !now.contains_key(*k)).count();
        format!(
            "\n## Changes from previous complete report\n\nRegressions: **{regressions}** · Newly passing: **{improvements}** · Changed cases: **{changed}** · Added: **{added}** · Removed: **{removed}**\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(outcome: Outcome) -> CaseReport {
        CaseReport {
            id: "suite/q01".into(),
            execution: Execution::NoFallback,
            connection: ConnectionId::Default,
            suite: "suite".into(),
            file: "q01.slt".into(),
            line: 1,
            tags: vec![],
            profile: "one-gpu".into(),
            storage: "native".into(),
            axes: [("gpu".parse().unwrap(), "one".parse().unwrap())].into(),
            fingerprint: "same".into(),
            outcome,
            message: "unsupported join".into(),
            reference_rows: Some(1),
            expected_gap: None,
            unexpected_pass: false,
            artifacts: "cases/q01".into(),
        }
    }

    #[test]
    fn baseline_keeps_raw_failures_and_detects_changes() {
        let baseline = Baseline {
            gaps: vec![Gap {
                id: "suite/q01".into(),
                axes: [("gpu".parse().unwrap(), "one".parse().unwrap())].into(),
                outcome: Outcome::Error,
                issue: "https://github.com/sirius-db/sirius/issues/123".into(),
                error: Some("unsupported join".into()),
            }],
        };
        let mut expected = case(Outcome::Error);
        baseline.apply(&mut expected);
        assert_eq!(expected.outcome, Outcome::Error);
        assert!(!expected.failed());
        let mut changed = case(Outcome::Error);
        changed.message = "unrelated failure".into();
        baseline.apply(&mut changed);
        assert!(changed.failed());
        let mut passed = case(Outcome::Match);
        baseline.apply(&mut passed);
        assert!(passed.unexpected_pass && passed.failed());
    }

    #[test]
    fn history_separates_changed_cases_from_regressions() {
        let mut before = Report::new(Provenance::default());
        before.complete = true;
        before.cases.push(case(Outcome::Match));
        let mut after = Report::new(Provenance::default());
        after.complete = true;
        after.cases.push(case(Outcome::Mismatch));
        assert!(after.history(&before).contains("Regressions: **1**"));
        after.cases[0].fingerprint = "changed".into();
        let history = after.history(&before);
        assert!(history.contains("Regressions: **0**") && history.contains("Changed cases: **1**"));
        before.complete = false;
        assert!(after.history(&before).contains("incomplete"));
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    #[serde(default)]
    pub gaps: Vec<Gap>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub id: String,
    pub axes: BTreeMap<Name, Name>,
    pub outcome: Outcome,
    pub issue: String,
    pub error: Option<String>,
}

impl Baseline {
    pub fn load(path: &Path) -> Result<Self> {
        let baseline: Self = toml::from_str(&fs::read_to_string(path)?)?;
        let mut keys = std::collections::HashSet::new();
        for gap in &baseline.gaps {
            ensure!(
                !gap.axes.is_empty(),
                "gaps must select an axis configuration"
            );
            ensure!(
                keys.insert((&gap.id, serde_json::to_string(&gap.axes)?)),
                "duplicate gap {}",
                gap.id
            );
            ensure!(
                matches!(
                    gap.outcome,
                    Outcome::Mismatch | Outcome::Error | Outcome::Timeout | Outcome::Crash
                ),
                "cannot baseline infrastructure/reference failures"
            );
            ensure!(
                gap.issue.starts_with("https://github.com/") && gap.issue.contains("/issues/"),
                "gap requires an issue URL"
            );
            ensure!(
                gap.outcome != Outcome::Error || gap.error.is_some(),
                "error gaps require a regex matcher"
            );
            if let Some(pattern) = &gap.error {
                regex::Regex::new(pattern)?;
            }
        }
        Ok(baseline)
    }
    pub fn apply(&self, case: &mut CaseReport) {
        if let Some(gap) = self
            .gaps
            .iter()
            .find(|g| g.id == case.id && g.axes == case.axes)
        {
            if case.outcome == Outcome::Match {
                case.unexpected_pass = true;
                case.message = format!("remove resolved gap: {}", gap.issue);
            } else if case.outcome == gap.outcome
                && gap
                    .error
                    .as_ref()
                    .is_none_or(|r| regex::Regex::new(r).unwrap().is_match(&case.message))
            {
                case.expected_gap = Some(gap.issue.clone());
            }
        }
    }
}
