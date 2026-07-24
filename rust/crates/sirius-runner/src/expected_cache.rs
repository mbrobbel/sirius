//! Immutable filesystem cache for reference-query results.
//!
//! Entries are addressed by all inputs that can change a reference result. A
//! lookup is read-only and returns a miss token; publishing that token is an
//! explicit choice by the caller. Writers serialize through a per-entry lock,
//! build a sibling temporary directory, write `result.json` last, and atomically
//! rename the completed directory into place.

use std::{
    collections::BTreeMap,
    fmt,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

use crate::cancel;

pub const CACHE_FORMAT_VERSION: u32 = 1;
pub const CACHE_KEY_VERSION: u32 = 1;
pub const RESULT_FORMAT_VERSION: u32 = 1;

const RESULT_FILE: &str = "result.json";
const LOCKS_DIR: &str = ".locks";
const CORRUPT_ENTRY_MARKER: &str = ".corrupt-";
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOCK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const KEY_HASH_DOMAIN: &[u8] = b"sirius.expected-cache.key\0";
const RESULT_HASH_DOMAIN: &[u8] = b"sirius.expected-cache.result\0";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Identity from the immutable receipt of the exact dataset materialization.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DatasetReceiptId(String);

impl DatasetReceiptId {
    pub fn new(value: impl Into<String>) -> anyhow::Result<Self> {
        let value = value.into();
        validate_opaque_id(&value, "dataset receipt identity")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DatasetReceiptId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Identity of the executable or library used as the reference engine.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReferenceArtifactId(String);

impl ReferenceArtifactId {
    pub fn new(value: impl Into<String>) -> anyhow::Result<Self> {
        let value = value.into();
        validate_opaque_id(&value, "reference artifact identity")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReferenceArtifactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A SHA-256 digest encoded as lowercase hexadecimal in JSON.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn of_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let digest = Sha256::digest(bytes.as_ref());
        Self(digest.into())
    }

    pub fn from_hex(value: &str) -> anyhow::Result<Self> {
        ensure!(
            value.len() == 64,
            "SHA-256 digest must contain exactly 64 hexadecimal characters"
        );

        let mut bytes = [0_u8; 32];
        for (index, output) in bytes.iter_mut().enumerate() {
            let offset = index * 2;
            let high = decode_hex_digit(value.as_bytes()[offset])
                .with_context(|| format!("invalid SHA-256 digest `{value}`"))?;
            let low = decode_hex_digit(value.as_bytes()[offset + 1])
                .with_context(|| format!("invalid SHA-256 digest `{value}`"))?;
            *output = (high << 4) | low;
        }
        ensure!(
            value.bytes().all(|byte| !byte.is_ascii_uppercase()),
            "SHA-256 digest must use lowercase hexadecimal"
        );
        Ok(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Sha256Digest")
            .field(&self.to_hex())
            .finish()
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(de::Error::custom)
    }
}

/// Exact rendered-SQL identity. Whitespace and comments are intentionally
/// significant because the worker executes the rendered bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RenderedSqlHash(Sha256Digest);

impl RenderedSqlHash {
    pub fn of_sql(rendered_sql: impl AsRef<[u8]>) -> Self {
        Self(Sha256Digest::of_bytes(rendered_sql))
    }

    pub fn digest(self) -> Sha256Digest {
        self.0
    }
}

impl fmt::Display for RenderedSqlHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Typed reference-engine setting. Potentially lossy JSON numbers are strings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ReferenceSettingValue {
    Boolean(bool),
    Integer(String),
    UnsignedInteger(String),
    Float(String),
    Text(String),
}

impl ReferenceSettingValue {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Boolean(_) | Self::Text(_) => Ok(()),
            Self::Integer(value) => validate_integer(value, true, "reference integer"),
            Self::UnsignedInteger(value) => {
                validate_integer(value, false, "reference unsigned integer")
            }
            Self::Float(value) => validate_float(value, "reference float"),
        }
    }
}

/// Reference binary and the resolved settings that affect its output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceArtifact {
    pub artifact_id: ReferenceArtifactId,
    #[serde(default)]
    pub settings: BTreeMap<String, ReferenceSettingValue>,
}

impl ReferenceArtifact {
    pub fn new(
        artifact_id: ReferenceArtifactId,
        settings: BTreeMap<String, ReferenceSettingValue>,
    ) -> anyhow::Result<Self> {
        let artifact = Self {
            artifact_id,
            settings,
        };
        artifact.validate()?;
        Ok(artifact)
    }

    fn validate(&self) -> anyhow::Result<()> {
        validate_opaque_id(self.artifact_id.as_str(), "reference artifact identity")?;
        for (name, value) in &self.settings {
            validate_name(name, "reference setting name")?;
            value
                .validate()
                .with_context(|| format!("invalid reference setting `{name}`"))?;
        }
        Ok(())
    }
}

/// Every input that can alter an expected result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedCacheKey {
    pub key_version: u32,
    pub dataset_receipt_id: DatasetReceiptId,
    pub rendered_sql_hash: RenderedSqlHash,
    pub reference: ReferenceArtifact,
    pub validation_protocol_version: u32,
}

impl ExpectedCacheKey {
    pub fn new(
        dataset_receipt_id: DatasetReceiptId,
        rendered_sql: impl AsRef<[u8]>,
        reference: ReferenceArtifact,
        validation_protocol_version: u32,
    ) -> anyhow::Result<Self> {
        Self::from_rendered_sql_hash(
            dataset_receipt_id,
            RenderedSqlHash::of_sql(rendered_sql),
            reference,
            validation_protocol_version,
        )
    }

    pub fn from_rendered_sql_hash(
        dataset_receipt_id: DatasetReceiptId,
        rendered_sql_hash: RenderedSqlHash,
        reference: ReferenceArtifact,
        validation_protocol_version: u32,
    ) -> anyhow::Result<Self> {
        let key = Self {
            key_version: CACHE_KEY_VERSION,
            dataset_receipt_id,
            rendered_sql_hash,
            reference,
            validation_protocol_version,
        };
        key.validate()?;
        Ok(key)
    }

    pub fn canonical_id(&self) -> anyhow::Result<ExpectedCacheId> {
        self.validate()?;
        let canonical =
            serde_json::to_vec(self).context("serializing expected-result cache key")?;
        let mut hasher = Sha256::new();
        hasher.update(KEY_HASH_DOMAIN);
        hasher.update(canonical);
        let digest = Sha256Digest(hasher.finalize().into());
        Ok(ExpectedCacheId(format!(
            "expected-k{}-{}",
            self.key_version, digest
        )))
    }

    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.key_version == CACHE_KEY_VERSION,
            "unsupported expected-cache key version {}; supported version is {}",
            self.key_version,
            CACHE_KEY_VERSION
        );
        validate_opaque_id(self.dataset_receipt_id.as_str(), "dataset receipt identity")?;
        self.reference.validate()?;
        ensure!(
            self.validation_protocol_version > 0,
            "validation protocol version must be greater than zero"
        );
        Ok(())
    }
}

/// Filesystem-safe canonical identity of an expected-result entry.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExpectedCacheId(String);

impl ExpectedCacheId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExpectedCacheId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One result column in worker-compatible JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultColumn {
    pub name: String,
    pub logical_type: String,
}

impl ResultColumn {
    pub fn new(name: impl Into<String>, logical_type: impl Into<String>) -> anyhow::Result<Self> {
        let column = Self {
            name: name.into(),
            logical_type: logical_type.into(),
        };
        column.validate()?;
        Ok(column)
    }

    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.logical_type.is_empty(),
            "result-column logical type must not be empty"
        );
        ensure!(
            !self.logical_type.chars().any(char::is_control),
            "result-column logical type must not contain control characters"
        );
        ensure!(
            !self.name.chars().any(char::is_control),
            "result-column name must not contain control characters"
        );
        Ok(())
    }
}

/// Ordered schema of the cached query result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultSchema {
    pub columns: Vec<ResultColumn>,
}

impl ResultSchema {
    pub fn new(columns: Vec<ResultColumn>) -> anyhow::Result<Self> {
        let schema = Self { columns };
        schema.validate()?;
        Ok(schema)
    }

    fn validate(&self) -> anyhow::Result<()> {
        for (index, column) in self.columns.iter().enumerate() {
            column
                .validate()
                .with_context(|| format!("invalid result column at index {index}"))?;
        }
        Ok(())
    }
}

/// One map entry. An array preserves DuckDB map ordering and permits non-string
/// keys without relying on JSON object-key coercion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultMapEntry {
    pub key: ResultValue,
    pub value: ResultValue,
}

/// Lossless JSON representation of a DuckDB scalar or nested value.
///
/// Integers, decimals, floats, and blobs use canonical strings so values such
/// as `HUGEINT`, scaled decimals, `-0.0`, NaN, infinity, and arbitrary bytes do
/// not pass through lossy JSON number or Unicode conversions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ResultValue {
    Null,
    Boolean(bool),
    Integer(String),
    UnsignedInteger(String),
    Float(String),
    Decimal(String),
    Text(String),
    Blob(String),
    Date(String),
    Time(String),
    Timestamp(String),
    Interval(String),
    Uuid(String),
    Json(String),
    List(Vec<ResultValue>),
    Struct(BTreeMap<String, ResultValue>),
    Map(Vec<ResultMapEntry>),
}

impl ResultValue {
    fn validate(&self) -> anyhow::Result<()> {
        match self {
            Self::Null | Self::Boolean(_) | Self::Text(_) => Ok(()),
            Self::Integer(value) => validate_integer(value, true, "integer result value"),
            Self::UnsignedInteger(value) => {
                validate_integer(value, false, "unsigned integer result value")
            }
            Self::Float(value) => validate_float(value, "float result value"),
            Self::Decimal(value) => validate_decimal(value),
            Self::Blob(value) => validate_blob(value),
            Self::Date(value)
            | Self::Time(value)
            | Self::Timestamp(value)
            | Self::Interval(value)
            | Self::Uuid(value) => {
                ensure!(!value.is_empty(), "typed result value must not be empty");
                ensure!(
                    !value.chars().any(char::is_control),
                    "typed result value must not contain control characters"
                );
                Ok(())
            }
            Self::Json(value) => {
                serde_json::from_str::<serde_json::Value>(value)
                    .context("JSON result value is not valid JSON")?;
                Ok(())
            }
            Self::List(values) => {
                for (index, value) in values.iter().enumerate() {
                    value
                        .validate()
                        .with_context(|| format!("invalid list item at index {index}"))?;
                }
                Ok(())
            }
            Self::Struct(fields) => {
                for (name, value) in fields {
                    validate_name(name, "struct field name")?;
                    value
                        .validate()
                        .with_context(|| format!("invalid struct field `{name}`"))?;
                }
                Ok(())
            }
            Self::Map(entries) => {
                for (index, entry) in entries.iter().enumerate() {
                    entry
                        .key
                        .validate()
                        .with_context(|| format!("invalid map key at index {index}"))?;
                    entry
                        .value
                        .validate()
                        .with_context(|| format!("invalid map value at index {index}"))?;
                }
                Ok(())
            }
        }
    }
}

/// One ordered row. Its JSON representation is an array of tagged values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResultRow(pub Vec<ResultValue>);

/// Complete typed output and its exact canonical digest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedResult {
    pub format_version: u32,
    pub schema: ResultSchema,
    pub rows: Vec<ResultRow>,
    pub row_count: u64,
    pub digest: Sha256Digest,
}

impl ExpectedResult {
    pub fn new(schema: ResultSchema, rows: Vec<ResultRow>) -> anyhow::Result<Self> {
        let row_count = u64::try_from(rows.len()).context("result row count exceeds u64")?;
        validate_result_content(&schema, &rows, row_count)?;
        let digest = result_digest(&schema, &rows, row_count)?;
        Ok(Self {
            format_version: RESULT_FORMAT_VERSION,
            schema,
            rows,
            row_count,
            digest,
        })
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.format_version == RESULT_FORMAT_VERSION,
            "unsupported expected-result format version {}; supported version is {}",
            self.format_version,
            RESULT_FORMAT_VERSION
        );
        validate_result_content(&self.schema, &self.rows, self.row_count)?;
        let actual_digest = result_digest(&self.schema, &self.rows, self.row_count)?;
        ensure!(
            self.digest == actual_digest,
            "expected-result digest mismatch: stored {}, computed {}",
            self.digest,
            actual_digest
        );
        Ok(())
    }
}

#[derive(Serialize)]
struct ResultDigestInput<'a> {
    format_version: u32,
    schema: &'a ResultSchema,
    rows: &'a [ResultRow],
    row_count: u64,
}

fn result_digest(
    schema: &ResultSchema,
    rows: &[ResultRow],
    row_count: u64,
) -> anyhow::Result<Sha256Digest> {
    let canonical = serde_json::to_vec(&ResultDigestInput {
        format_version: RESULT_FORMAT_VERSION,
        schema,
        rows,
        row_count,
    })
    .context("serializing canonical expected-result content")?;
    let mut hasher = Sha256::new();
    hasher.update(RESULT_HASH_DOMAIN);
    hasher.update(canonical);
    Ok(Sha256Digest(hasher.finalize().into()))
}

fn validate_result_content(
    schema: &ResultSchema,
    rows: &[ResultRow],
    row_count: u64,
) -> anyhow::Result<()> {
    schema.validate()?;
    ensure!(
        row_count == rows.len() as u64,
        "result row_count is {row_count}, but {} rows are stored",
        rows.len()
    );
    for (row_index, row) in rows.iter().enumerate() {
        ensure!(
            row.0.len() == schema.columns.len(),
            "result row {row_index} has {} values, but the schema has {} columns",
            row.0.len(),
            schema.columns.len()
        );
        for (column_index, value) in row.0.iter().enumerate() {
            value.validate().with_context(|| {
                format!("invalid result value at row {row_index}, column {column_index}")
            })?;
        }
    }
    Ok(())
}

/// On-disk `result.json`. Its presence marks a complete cache entry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedExpectedResult {
    pub cache_format_version: u32,
    pub cache_id: ExpectedCacheId,
    pub key: ExpectedCacheKey,
    pub result: ExpectedResult,
}

impl CachedExpectedResult {
    fn new(key: ExpectedCacheKey, result: ExpectedResult) -> anyhow::Result<Self> {
        result.validate()?;
        let cache_id = key.canonical_id()?;
        Ok(Self {
            cache_format_version: CACHE_FORMAT_VERSION,
            cache_id,
            key,
            result,
        })
    }

    fn validate_for(
        &self,
        requested_key: &ExpectedCacheKey,
        requested_id: &ExpectedCacheId,
    ) -> anyhow::Result<()> {
        ensure!(
            self.cache_format_version == CACHE_FORMAT_VERSION,
            "unsupported expected-cache format version {}; supported version is {}",
            self.cache_format_version,
            CACHE_FORMAT_VERSION
        );
        self.key.validate()?;
        ensure!(
            &self.key == requested_key,
            "cache entry key does not exactly match the requested key"
        );
        let computed_id = self.key.canonical_id()?;
        ensure!(
            self.cache_id == computed_id,
            "cache entry records ID {}, but its key computes to {}",
            self.cache_id,
            computed_id
        );
        ensure!(
            &self.cache_id == requested_id,
            "cache entry ID {} does not match directory ID {}",
            self.cache_id,
            requested_id
        );
        self.result.validate()?;
        Ok(())
    }
}

/// Result of a read-only cache lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedCacheLookup {
    Hit(CachedExpectedResult),
    Miss(ExpectedCacheMiss),
}

/// Capability returned by a miss. Only explicitly selected miss tokens can be
/// published, which prevents a suite-wide resolver from filling unselected
/// queries as a side effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedCacheMiss {
    key: ExpectedCacheKey,
    cache_id: ExpectedCacheId,
}

impl ExpectedCacheMiss {
    pub fn key(&self) -> &ExpectedCacheKey {
        &self.key
    }

    pub fn cache_id(&self) -> &ExpectedCacheId {
        &self.cache_id
    }
}

/// Outcome after publishing a selected miss under its per-entry lock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublishOutcome {
    Published(CachedExpectedResult),
    AlreadyPresent(CachedExpectedResult),
}

pub enum ReservationOutcome {
    Reserved(ExpectedCacheReservation),
    AlreadyPresent(CachedExpectedResult),
}

enum ExistingEntry {
    Missing,
    Valid(Box<CachedExpectedResult>),
    Invalid(anyhow::Error),
}

pub struct ExpectedCacheReservation {
    cache: ExpectedCache,
    miss: ExpectedCacheMiss,
    _entry_lock: EntryLock,
}

impl ExpectedCacheReservation {
    pub fn publish(self, result: ExpectedResult) -> anyhow::Result<CachedExpectedResult> {
        self.publish_checked(result, || Ok(()))
    }

    /// Materializes the entry, verifies the source is still valid, then
    /// atomically publishes it while retaining the per-entry lock.
    pub fn publish_checked(
        self,
        result: ExpectedResult,
        verify_source: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<CachedExpectedResult> {
        self.cache.publish_locked(self.miss, result, verify_source)
    }
}

/// Immutable expected-result cache rooted at one directory.
#[derive(Clone, Debug)]
pub struct ExpectedCache {
    root: PathBuf,
}

impl ExpectedCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn lookup(&self, key: &ExpectedCacheKey) -> anyhow::Result<ExpectedCacheLookup> {
        let cache_id = key.canonical_id()?;
        match self.inspect_existing(key, &cache_id)? {
            ExistingEntry::Valid(entry) => Ok(ExpectedCacheLookup::Hit(*entry)),
            ExistingEntry::Missing | ExistingEntry::Invalid(_) => {
                Ok(ExpectedCacheLookup::Miss(ExpectedCacheMiss {
                    key: key.clone(),
                    cache_id,
                }))
            }
        }
    }

    /// Looks up exactly the caller-selected keys, preserving their order.
    pub fn lookup_selected<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a ExpectedCacheKey>,
    ) -> anyhow::Result<Vec<ExpectedCacheLookup>> {
        keys.into_iter().map(|key| self.lookup(key)).collect()
    }

    pub fn publish(
        &self,
        miss: ExpectedCacheMiss,
        result: ExpectedResult,
    ) -> anyhow::Result<PublishOutcome> {
        result.validate()?;
        match self.reserve(miss, |_| Ok(()))? {
            ReservationOutcome::AlreadyPresent(entry) => Ok(PublishOutcome::AlreadyPresent(entry)),
            ReservationOutcome::Reserved(reservation) => {
                Ok(PublishOutcome::Published(reservation.publish(result)?))
            }
        }
    }

    pub fn reserve(
        &self,
        miss: ExpectedCacheMiss,
        mut waiting: impl FnMut(Duration) -> anyhow::Result<()>,
    ) -> anyhow::Result<ReservationOutcome> {
        let computed_id = miss.key.canonical_id()?;
        ensure!(
            miss.cache_id == computed_id,
            "cache miss token ID does not match its key"
        );

        fs::create_dir_all(&self.root)
            .with_context(|| format!("creating expected-cache root {}", self.root.display()))?;
        let entry_lock = self.lock_entry(&miss.cache_id, &mut waiting)?;
        match self.inspect_existing(&miss.key, &miss.cache_id)? {
            ExistingEntry::Valid(entry) => {
                return Ok(ReservationOutcome::AlreadyPresent(*entry));
            }
            ExistingEntry::Missing => {}
            ExistingEntry::Invalid(error) => {
                self.quarantine_invalid_entry(&miss.cache_id)
                    .with_context(|| {
                        format!(
                            "repairing invalid expected-cache entry {}: {error:#}",
                            self.entry_path(&miss.cache_id).display()
                        )
                    })?;
            }
        }
        Ok(ReservationOutcome::Reserved(ExpectedCacheReservation {
            cache: self.clone(),
            miss,
            _entry_lock: entry_lock,
        }))
    }

    fn publish_locked(
        &self,
        miss: ExpectedCacheMiss,
        result: ExpectedResult,
        verify_source: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<CachedExpectedResult> {
        result.validate()?;
        let entry = CachedExpectedResult::new(miss.key, result)?;
        let mut temporary = TemporaryEntry::create(&self.root, &entry.cache_id)?;
        write_result_marker(temporary.path(), &entry)?;
        sync_directory(temporary.path()).with_context(|| {
            format!(
                "syncing temporary expected-cache entry {}",
                temporary.path().display()
            )
        })?;
        verify_source().context("verifying expected-result source before cache publication")?;

        let destination = self.entry_path(&entry.cache_id);
        fs::rename(temporary.path(), &destination).with_context(|| {
            format!(
                "publishing expected-cache entry {} to {}",
                temporary.path().display(),
                destination.display()
            )
        })?;
        temporary.disarm();
        sync_directory(&self.root)
            .with_context(|| format!("syncing expected-cache root {}", self.root.display()))?;

        Ok(entry)
    }

    fn inspect_existing(
        &self,
        key: &ExpectedCacheKey,
        cache_id: &ExpectedCacheId,
    ) -> anyhow::Result<ExistingEntry> {
        let entry_path = self.entry_path(cache_id);
        let metadata = match fs::symlink_metadata(&entry_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(ExistingEntry::Missing);
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspecting cache entry {}", entry_path.display()));
            }
        };
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Ok(ExistingEntry::Invalid(anyhow::anyhow!(
                "expected-cache entry {} is not a regular directory",
                entry_path.display()
            )));
        }

        let result_path = entry_path.join(RESULT_FILE);
        let result_metadata = match fs::symlink_metadata(&result_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(ExistingEntry::Invalid(anyhow::anyhow!(
                    "expected-cache entry {} is incomplete: {RESULT_FILE} is missing",
                    entry_path.display()
                )));
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspecting cache marker {}", result_path.display()));
            }
        };
        if !result_metadata.file_type().is_file() || result_metadata.file_type().is_symlink() {
            return Ok(ExistingEntry::Invalid(anyhow::anyhow!(
                "expected-cache marker {} is not a regular file",
                result_path.display()
            )));
        }

        let bytes = match fs::read(&result_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(ExistingEntry::Missing);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("reading expected-cache marker {}", result_path.display())
                });
            }
        };
        let entry: CachedExpectedResult = match serde_json::from_slice(&bytes) {
            Ok(entry) => entry,
            Err(error) => {
                return Ok(ExistingEntry::Invalid(anyhow::Error::new(error).context(
                    format!("parsing expected-cache marker {}", result_path.display()),
                )));
            }
        };
        if let Err(error) = entry.validate_for(key, cache_id) {
            return Ok(ExistingEntry::Invalid(error.context(format!(
                "validating expected-cache entry {}",
                entry_path.display()
            ))));
        }
        Ok(ExistingEntry::Valid(Box::new(entry)))
    }

    fn quarantine_invalid_entry(&self, cache_id: &ExpectedCacheId) -> anyhow::Result<()> {
        let entry_path = self.entry_path(cache_id);
        for _ in 0..100 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let quarantine_directory = self.root.join(format!(
                "{CORRUPT_ENTRY_MARKER}{}-{}-{sequence}",
                cache_id.as_str(),
                std::process::id()
            ));
            match fs::create_dir(&quarantine_directory) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "creating quarantine directory {}",
                            quarantine_directory.display()
                        )
                    });
                }
            }
            let quarantine_path = quarantine_directory.join("entry");
            match fs::rename(&entry_path, &quarantine_path) {
                Ok(()) => {
                    sync_directory(&self.root).with_context(|| {
                        format!(
                            "syncing expected-cache root after quarantining {}",
                            entry_path.display()
                        )
                    })?;
                    return Ok(());
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    let _ = fs::remove_dir(&quarantine_directory);
                    return Ok(());
                }
                Err(error) => {
                    let _ = fs::remove_dir(&quarantine_directory);
                    return Err(error).with_context(|| {
                        format!(
                            "quarantining invalid expected-cache entry {} as {}",
                            entry_path.display(),
                            quarantine_path.display()
                        )
                    });
                }
            }
        }
        bail!(
            "could not allocate a unique quarantine path for invalid expected-cache entry {}",
            entry_path.display()
        )
    }

    fn entry_path(&self, cache_id: &ExpectedCacheId) -> PathBuf {
        self.root.join(cache_id.as_str())
    }

    fn lock_entry(
        &self,
        cache_id: &ExpectedCacheId,
        waiting: &mut impl FnMut(Duration) -> anyhow::Result<()>,
    ) -> anyhow::Result<EntryLock> {
        let lock_directory = self.root.join(LOCKS_DIR);
        fs::create_dir_all(&lock_directory).with_context(|| {
            format!(
                "creating expected-cache lock directory {}",
                lock_directory.display()
            )
        })?;
        let lock_path = lock_directory.join(format!("{cache_id}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("opening expected-cache lock {}", lock_path.display()))?;
        let started = Instant::now();
        let mut last_heartbeat = None;
        loop {
            cancel::check()?;
            match file.try_lock() {
                Ok(()) => return Ok(EntryLock { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    let elapsed = started.elapsed();
                    if last_heartbeat
                        .is_none_or(|last: Instant| last.elapsed() >= LOCK_HEARTBEAT_INTERVAL)
                    {
                        waiting(elapsed)?;
                        last_heartbeat = Some(Instant::now());
                    }
                    thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("locking expected-cache entry {cache_id}"));
                }
            }
        }
    }
}

struct EntryLock {
    file: File,
}

impl Drop for EntryLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

struct TemporaryEntry {
    path: PathBuf,
    armed: bool,
}

impl TemporaryEntry {
    fn create(root: &Path, cache_id: &ExpectedCacheId) -> anyhow::Result<Self> {
        for _ in 0..100 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                ".{}.tmp-{}-{sequence}",
                cache_id.as_str(),
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, armed: true }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("creating temporary expected-cache entry {}", path.display())
                    });
                }
            }
        }
        bail!("could not allocate a unique temporary expected-cache directory")
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryEntry {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn write_result_marker(directory: &Path, entry: &CachedExpectedResult) -> anyhow::Result<()> {
    let partial_path = directory.join(format!("{RESULT_FILE}.partial"));
    let final_path = directory.join(RESULT_FILE);
    let mut bytes =
        serde_json::to_vec_pretty(entry).context("serializing expected-cache result marker")?;
    bytes.push(b'\n');

    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial_path)
        .with_context(|| {
            format!(
                "creating partial expected-cache marker {}",
                partial_path.display()
            )
        })?;
    file.write_all(&bytes)
        .with_context(|| format!("writing expected-cache marker {}", partial_path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing expected-cache marker {}", partial_path.display()))?;
    drop(file);

    fs::rename(&partial_path, &final_path)
        .with_context(|| format!("completing expected-cache marker {}", final_path.display()))?;
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

fn validate_opaque_id(value: &str, label: &str) -> anyhow::Result<()> {
    ensure!(!value.is_empty(), "{label} must not be empty");
    ensure!(
        !value.chars().any(char::is_control),
        "{label} must not contain control characters"
    );
    Ok(())
}

fn validate_name(value: &str, label: &str) -> anyhow::Result<()> {
    ensure!(!value.is_empty(), "{label} must not be empty");
    ensure!(
        !value.chars().any(char::is_control),
        "{label} must not contain control characters"
    );
    Ok(())
}

fn validate_integer(value: &str, signed: bool, label: &str) -> anyhow::Result<()> {
    ensure!(!value.is_empty(), "{label} must not be empty");
    let digits = if signed {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    ensure!(!digits.is_empty(), "{label} has no digits");
    ensure!(
        digits.bytes().all(|byte| byte.is_ascii_digit()),
        "{label} must contain only an optional minus sign and decimal digits"
    );
    ensure!(
        digits == "0" || !digits.starts_with('0'),
        "{label} must not contain leading zeroes"
    );
    ensure!(value != "-0", "{label} must represent zero as `0`");
    Ok(())
}

fn validate_float(value: &str, label: &str) -> anyhow::Result<()> {
    ensure!(!value.is_empty(), "{label} must not be empty");
    if matches!(value, "nan" | "inf" | "-inf") {
        return Ok(());
    }
    let parsed = value
        .parse::<f64>()
        .with_context(|| format!("{label} `{value}` is not a float"))?;
    ensure!(
        parsed.is_finite(),
        "{label} must encode special values as `nan`, `inf`, or `-inf`"
    );
    ensure!(
        value.trim() == value,
        "{label} must not contain surrounding whitespace"
    );
    Ok(())
}

fn validate_decimal(value: &str) -> anyhow::Result<()> {
    ensure!(!value.is_empty(), "decimal result value must not be empty");
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    ensure!(!unsigned.is_empty(), "decimal result value has no digits");
    let mut parts = unsigned.split('.');
    let integral = parts.next().expect("split always returns one element");
    let fractional = parts.next();
    ensure!(
        parts.next().is_none(),
        "decimal result value contains more than one decimal point"
    );
    ensure!(
        !integral.is_empty() && integral.bytes().all(|byte| byte.is_ascii_digit()),
        "decimal result value has an invalid integral part"
    );
    ensure!(
        integral == "0" || !integral.starts_with('0'),
        "decimal result value must not contain leading zeroes"
    );
    if let Some(fractional) = fractional {
        ensure!(
            !fractional.is_empty() && fractional.bytes().all(|byte| byte.is_ascii_digit()),
            "decimal result value has an invalid fractional part"
        );
    }
    ensure!(
        !value.starts_with("-0") || value.starts_with("-0."),
        "decimal result value must represent integral zero without a minus sign"
    );
    Ok(())
}

fn validate_blob(value: &str) -> anyhow::Result<()> {
    ensure!(
        value.len().is_multiple_of(2),
        "blob result value must contain an even number of hexadecimal characters"
    );
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "blob result value must use lowercase hexadecimal"
    );
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex_digit(byte: u8) -> anyhow::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("invalid hexadecimal character"),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        sync::{
            Arc, Barrier,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        },
        thread,
    };

    use anyhow::Context;
    use tempfile::TempDir;

    use super::*;

    fn reference(
        settings: impl IntoIterator<Item = (&'static str, ReferenceSettingValue)>,
    ) -> ReferenceArtifact {
        ReferenceArtifact::new(
            ReferenceArtifactId::new("sha256:reference-binary").unwrap(),
            settings
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
        )
        .unwrap()
    }

    fn key(sql: &str) -> ExpectedCacheKey {
        ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-receipt-v1:abc123").unwrap(),
            sql,
            reference([
                (
                    "threads",
                    ReferenceSettingValue::UnsignedInteger("8".into()),
                ),
                (
                    "preserve_insertion_order",
                    ReferenceSettingValue::Boolean(true),
                ),
            ]),
            1,
        )
        .unwrap()
    }

    fn result() -> ExpectedResult {
        let schema = ResultSchema::new(vec![
            ResultColumn::new("answer", "HUGEINT").unwrap(),
            ResultColumn::new("amount", "DECIMAL(20,4)").unwrap(),
            ResultColumn::new("measured", "DOUBLE").unwrap(),
            ResultColumn::new("created", "TIMESTAMP").unwrap(),
            ResultColumn::new("payload", "BLOB").unwrap(),
            ResultColumn::new("nested", "STRUCT(label VARCHAR, values INTEGER[])").unwrap(),
        ])
        .unwrap();

        ExpectedResult::new(
            schema,
            vec![ResultRow(vec![
                ResultValue::Integer("170141183460469231731687303715884105727".into()),
                ResultValue::Decimal("-12.3400".into()),
                ResultValue::Float("-0.0".into()),
                ResultValue::Timestamp("2026-07-24T12:34:56.123456+00:00".into()),
                ResultValue::Blob("00ff80".into()),
                ResultValue::Struct(BTreeMap::from([
                    ("label".into(), ResultValue::Text("hello".into())),
                    (
                        "values".into(),
                        ResultValue::List(vec![
                            ResultValue::Integer("1".into()),
                            ResultValue::Null,
                        ]),
                    ),
                ])),
            ])],
        )
        .unwrap()
    }

    fn miss(lookup: ExpectedCacheLookup) -> ExpectedCacheMiss {
        match lookup {
            ExpectedCacheLookup::Miss(miss) => miss,
            ExpectedCacheLookup::Hit(_) => panic!("expected cache miss"),
        }
    }

    fn quarantine_directories(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(CORRUPT_ENTRY_MARKER)
            })
            .map(|entry| entry.path())
            .collect()
    }

    #[test]
    fn sha256_digest_uses_lowercase_canonical_hex() {
        let digest = Sha256Digest::of_bytes(b"abc");
        assert_eq!(
            digest.to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(Sha256Digest::from_hex(&digest.to_hex()).unwrap(), digest);
        assert!(Sha256Digest::from_hex(&digest.to_hex().to_uppercase()).is_err());
        assert!(Sha256Digest::from_hex("00").is_err());
    }

    #[test]
    fn canonical_key_id_is_stable_and_setting_order_independent() {
        let settings_a = [
            (
                "threads",
                ReferenceSettingValue::UnsignedInteger("8".into()),
            ),
            (
                "preserve_insertion_order",
                ReferenceSettingValue::Boolean(true),
            ),
            ("mode", ReferenceSettingValue::Text("strict".into())),
        ];
        let settings_b = [
            ("mode", ReferenceSettingValue::Text("strict".into())),
            (
                "preserve_insertion_order",
                ReferenceSettingValue::Boolean(true),
            ),
            (
                "threads",
                ReferenceSettingValue::UnsignedInteger("8".into()),
            ),
        ];
        let a = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-a").unwrap(),
            "select 42",
            reference(settings_a),
            3,
        )
        .unwrap();
        let b = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-a").unwrap(),
            "select 42",
            reference(settings_b.clone()),
            3,
        )
        .unwrap();

        assert_eq!(a.canonical_id().unwrap(), b.canonical_id().unwrap());
        assert_eq!(
            a.canonical_id().unwrap().as_str(),
            "expected-k1-99c69c09e67f6a979a315effcfd1b96670392093336495057eb258c90577648d"
        );

        let changed_sql = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-a").unwrap(),
            "select 43",
            reference(settings_b.clone()),
            3,
        )
        .unwrap();
        let changed_dataset = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-b").unwrap(),
            "select 42",
            reference(settings_b.clone()),
            3,
        )
        .unwrap();
        let changed_reference = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-a").unwrap(),
            "select 42",
            ReferenceArtifact::new(
                ReferenceArtifactId::new("other-reference").unwrap(),
                BTreeMap::new(),
            )
            .unwrap(),
            3,
        )
        .unwrap();
        let changed_threads = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-a").unwrap(),
            "select 42",
            reference([
                ("mode", ReferenceSettingValue::Text("strict".into())),
                (
                    "preserve_insertion_order",
                    ReferenceSettingValue::Boolean(true),
                ),
                (
                    "threads",
                    ReferenceSettingValue::UnsignedInteger("16".into()),
                ),
            ]),
            3,
        )
        .unwrap();
        let changed_insertion_order = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-a").unwrap(),
            "select 42",
            reference([
                ("mode", ReferenceSettingValue::Text("strict".into())),
                (
                    "preserve_insertion_order",
                    ReferenceSettingValue::Boolean(false),
                ),
                (
                    "threads",
                    ReferenceSettingValue::UnsignedInteger("8".into()),
                ),
            ]),
            3,
        )
        .unwrap();
        let changed_protocol = ExpectedCacheKey::new(
            DatasetReceiptId::new("dataset-a").unwrap(),
            "select 42",
            reference(settings_b),
            4,
        )
        .unwrap();
        let original = a.canonical_id().unwrap();
        assert_ne!(changed_sql.canonical_id().unwrap(), original);
        assert_ne!(changed_dataset.canonical_id().unwrap(), original);
        assert_ne!(changed_reference.canonical_id().unwrap(), original);
        assert_ne!(changed_threads.canonical_id().unwrap(), original);
        assert_ne!(changed_insertion_order.canonical_id().unwrap(), original);
        assert_ne!(changed_protocol.canonical_id().unwrap(), original);
    }

    #[test]
    fn typed_result_round_trips_without_loss() {
        let mut expected = result();
        expected.rows.push(ResultRow(vec![
            ResultValue::UnsignedInteger("340282366920938463463374607431768211455".into()),
            ResultValue::Decimal("0.0000".into()),
            ResultValue::Float("nan".into()),
            ResultValue::Timestamp("infinity".into()),
            ResultValue::Blob(String::new()),
            ResultValue::Map(vec![ResultMapEntry {
                key: ResultValue::Date("2026-07-24".into()),
                value: ResultValue::Float("-inf".into()),
            }]),
        ]));
        expected.row_count = 2;
        expected.digest =
            result_digest(&expected.schema, &expected.rows, expected.row_count).unwrap();
        expected.validate().unwrap();

        let json = serde_json::to_string(&expected).unwrap();
        assert!(json.contains(r#""type":"decimal","value":"-12.3400""#));
        assert!(json.contains(r#""type":"float","value":"nan""#));
        assert!(json.contains(r#""type":"blob","value":"00ff80""#));
        let decoded: ExpectedResult = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, expected);
        decoded.validate().unwrap();
    }

    #[test]
    fn invalid_rows_and_tampered_digest_are_rejected() {
        let schema = ResultSchema::new(vec![ResultColumn::new("one", "INTEGER").unwrap()]).unwrap();
        assert!(
            ExpectedResult::new(
                schema.clone(),
                vec![ResultRow(vec![
                    ResultValue::Integer("1".into()),
                    ResultValue::Integer("2".into())
                ])]
            )
            .is_err()
        );
        assert!(
            ExpectedResult::new(
                schema,
                vec![ResultRow(vec![ResultValue::Integer("01".into())])]
            )
            .is_err()
        );

        let mut expected = result();
        expected.digest = Sha256Digest::of_bytes(b"tampered");
        assert!(expected.validate().is_err());
    }

    #[test]
    fn selected_miss_publishes_and_then_is_an_exact_hit() {
        let directory = TempDir::new().unwrap();
        let cache = ExpectedCache::new(directory.path());
        let selected = key("select 1");
        let unselected = key("select 2");

        let selected_lookups = cache.lookup_selected([&selected]).unwrap();
        assert_eq!(selected_lookups.len(), 1);
        let selected_miss = miss(selected_lookups.into_iter().next().unwrap());
        let cache_id = selected_miss.cache_id().clone();
        let published = cache.publish(selected_miss, result()).unwrap();
        assert!(matches!(published, PublishOutcome::Published(_)));

        let hit = cache.lookup(&selected).unwrap();
        let entry = match hit {
            ExpectedCacheLookup::Hit(entry) => entry,
            ExpectedCacheLookup::Miss(_) => panic!("published entry was not a hit"),
        };
        assert_eq!(entry.cache_id, cache_id);
        assert_eq!(entry.key, selected);
        assert_eq!(entry.result, result());
        assert!(
            directory
                .path()
                .join(cache_id.as_str())
                .join(RESULT_FILE)
                .is_file()
        );

        assert!(matches!(
            cache.lookup(&unselected).unwrap(),
            ExpectedCacheLookup::Miss(_)
        ));
        assert!(
            !directory
                .path()
                .join(unselected.canonical_id().unwrap().as_str())
                .exists()
        );

        let temporary_entries = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(temporary_entries, 0);
    }

    #[test]
    fn failed_source_check_prevents_publication_and_cleans_temporary_entry() {
        let directory = TempDir::new().unwrap();
        let cache = ExpectedCache::new(directory.path());
        let cache_key = key("select 1");
        let cache_id = cache_key.canonical_id().unwrap();
        let selected_miss = miss(cache.lookup(&cache_key).unwrap());
        let reservation = match cache.reserve(selected_miss, |_| Ok(())).unwrap() {
            ReservationOutcome::Reserved(reservation) => reservation,
            ReservationOutcome::AlreadyPresent(_) => panic!("expected a reservation"),
        };

        let error = reservation
            .publish_checked(result(), || Err(anyhow::anyhow!("dataset changed")))
            .unwrap_err();

        assert!(format!("{error:#}").contains("dataset changed"));
        assert!(!directory.path().join(cache_id.as_str()).exists());
        assert!(
            cache
                .lookup(&cache_key)
                .is_ok_and(|lookup| matches!(lookup, ExpectedCacheLookup::Miss(_)))
        );
        let temporary_entries = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(temporary_entries, 0);
    }

    #[test]
    fn read_only_lookup_treats_corruption_as_a_miss_without_mutating_it() {
        let directory = TempDir::new().unwrap();
        let cache = ExpectedCache::new(directory.path());
        let cache_key = key("select 1");
        let cache_id = cache_key.canonical_id().unwrap();
        let entry_path = directory.path().join(cache_id.as_str());
        fs::create_dir_all(&entry_path).unwrap();

        assert!(matches!(
            cache.lookup(&cache_key).unwrap(),
            ExpectedCacheLookup::Miss(_)
        ));
        assert!(entry_path.is_dir());
        assert!(quarantine_directories(directory.path()).is_empty());

        fs::remove_dir_all(&entry_path).unwrap();
        let selected_miss = miss(cache.lookup(&cache_key).unwrap());
        cache.publish(selected_miss, result()).unwrap();

        let result_path = entry_path.join(RESULT_FILE);
        let mut entry: CachedExpectedResult =
            serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
        entry.result.digest = Sha256Digest::of_bytes(b"wrong");
        let corrupt_bytes = serde_json::to_vec_pretty(&entry).unwrap();
        fs::write(&result_path, &corrupt_bytes).unwrap();

        assert!(matches!(
            cache.lookup(&cache_key).unwrap(),
            ExpectedCacheLookup::Miss(_)
        ));
        assert_eq!(fs::read(&result_path).unwrap(), corrupt_bytes);
        assert!(quarantine_directories(directory.path()).is_empty());
    }

    #[test]
    fn reserving_a_corrupt_entry_quarantines_and_regenerates_it() {
        let directory = TempDir::new().unwrap();
        let cache = ExpectedCache::new(directory.path());
        let cache_key = key("select corrupt");
        let cache_id = cache_key.canonical_id().unwrap();
        let entry_path = directory.path().join(cache_id.as_str());
        cache
            .publish(miss(cache.lookup(&cache_key).unwrap()), result())
            .unwrap();

        let result_path = entry_path.join(RESULT_FILE);
        let mut entry: CachedExpectedResult =
            serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
        entry.result.digest = Sha256Digest::of_bytes(b"wrong");
        fs::write(&result_path, serde_json::to_vec_pretty(&entry).unwrap()).unwrap();

        let reservation = match cache
            .reserve(miss(cache.lookup(&cache_key).unwrap()), |_| Ok(()))
            .unwrap()
        {
            ReservationOutcome::Reserved(reservation) => reservation,
            ReservationOutcome::AlreadyPresent(_) => panic!("corrupt entry was accepted"),
        };
        assert!(!entry_path.exists());
        let quarantined = quarantine_directories(directory.path());
        assert_eq!(quarantined.len(), 1);
        assert!(quarantined[0].join("entry").join(RESULT_FILE).is_file());

        reservation.publish(result()).unwrap();
        assert!(matches!(
            cache.lookup(&cache_key).unwrap(),
            ExpectedCacheLookup::Hit(_)
        ));
        assert!(entry_path.join(RESULT_FILE).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn repairing_an_entry_symlink_never_follows_its_target() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let cache = ExpectedCache::new(directory.path());
        let cache_key = key("select symlink");
        let cache_id = cache_key.canonical_id().unwrap();
        let entry_path = directory.path().join(cache_id.as_str());
        let target = directory.path().join("must-survive");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("sentinel"), b"untouched").unwrap();
        symlink(&target, &entry_path).unwrap();

        let reservation = match cache
            .reserve(miss(cache.lookup(&cache_key).unwrap()), |_| Ok(()))
            .unwrap()
        {
            ReservationOutcome::Reserved(reservation) => reservation,
            ReservationOutcome::AlreadyPresent(_) => panic!("symlink was accepted"),
        };
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"untouched");

        reservation.publish(result()).unwrap();
        assert!(entry_path.join(RESULT_FILE).is_file());
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"untouched");
    }

    #[test]
    fn mismatched_recorded_key_is_a_repairable_miss() {
        let directory = TempDir::new().unwrap();
        let cache = ExpectedCache::new(directory.path());
        let requested = key("select 1");
        let selected_miss = miss(cache.lookup(&requested).unwrap());
        let cache_id = selected_miss.cache_id().clone();
        cache.publish(selected_miss, result()).unwrap();

        let result_path = directory.path().join(cache_id.as_str()).join(RESULT_FILE);
        let mut entry: CachedExpectedResult =
            serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
        entry.key = key("select 2");
        fs::write(&result_path, serde_json::to_vec_pretty(&entry).unwrap()).unwrap();

        assert!(matches!(
            cache.lookup(&requested).unwrap(),
            ExpectedCacheLookup::Miss(_)
        ));
        assert!(result_path.is_file());
    }

    #[test]
    fn concurrent_publishers_converge_on_one_entry() {
        let directory = TempDir::new().unwrap();
        let cache = Arc::new(ExpectedCache::new(directory.path()));
        let cache_key = key("select 1");
        let first_miss = miss(cache.lookup(&cache_key).unwrap());
        let second_miss = miss(cache.lookup(&cache_key).unwrap());
        let barrier = Arc::new(Barrier::new(2));

        let handles = [first_miss, second_miss].map(|cache_miss| {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                cache
                    .publish(cache_miss, result())
                    .context("publishing from worker")
            })
        });

        let outcomes = handles.map(|handle| handle.join().unwrap().unwrap());
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, PublishOutcome::Published(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, PublishOutcome::AlreadyPresent(_)))
                .count(),
            1
        );
        assert!(matches!(
            cache.lookup(&cache_key).unwrap(),
            ExpectedCacheLookup::Hit(_)
        ));
    }

    #[test]
    fn reservations_prevent_concurrent_reference_generation() {
        let directory = TempDir::new().unwrap();
        let cache = Arc::new(ExpectedCache::new(directory.path()));
        let cache_key = key("select reserved");
        let first_miss = miss(cache.lookup(&cache_key).unwrap());
        let second_miss = miss(cache.lookup(&cache_key).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let generations = Arc::new(AtomicUsize::new(0));

        let handles = [first_miss, second_miss].map(|cache_miss| {
            let cache = Arc::clone(&cache);
            let barrier = Arc::clone(&barrier);
            let generations = Arc::clone(&generations);
            thread::spawn(move || {
                barrier.wait();
                match cache.reserve(cache_miss, |_| Ok(())).unwrap() {
                    ReservationOutcome::AlreadyPresent(entry) => entry,
                    ReservationOutcome::Reserved(reservation) => {
                        generations.fetch_add(1, AtomicOrdering::SeqCst);
                        thread::sleep(Duration::from_millis(50));
                        reservation.publish(result()).unwrap()
                    }
                }
            })
        });

        let entries = handles.map(|handle| handle.join().unwrap());
        assert_eq!(generations.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(entries[0], entries[1]);
    }

    #[test]
    fn unsupported_versions_and_unknown_json_fields_are_rejected() {
        let mut unsupported = key("select 1");
        unsupported.key_version += 1;
        assert!(unsupported.canonical_id().is_err());

        let mut json = serde_json::to_value(result()).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("surprise".into(), serde_json::Value::Bool(true));
        assert!(serde_json::from_value::<ExpectedResult>(json).is_err());
    }
}
