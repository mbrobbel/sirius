use crate::config::{Fixture, FixturePath, Name};
use anyhow::{Context, Result, ensure};
use s3::{Bucket, BucketConfiguration, Region, creds::Credentials};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    num::{NonZeroU32, NonZeroUsize},
    path::Path,
    time::Duration,
};
use testcontainers_modules::{
    minio::MinIO,
    testcontainers::{Container, ImageExt, core::Mount, runners::SyncRunner},
};

const ACCESS_KEY: &str = "minioadmin";
const SECRET_KEY: &str = "minioadmin";
const REGION: &str = "us-east-1";
const IMAGE_NAME: &str = "quay.io/minio/minio";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Service {
    Minio(MinioService),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinioService {
    image_tag: ImageTag,
    bucket: BucketName,
    #[serde(default)]
    storage: ServiceStorage,
    #[serde(default)]
    objects: BTreeMap<ObjectKey, FixtureObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    copies: Vec<ObjectCopies>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ServiceStorage {
    Container {},
    Tmpfs { size_mib: NonZeroU32 },
}

impl Default for ServiceStorage {
    fn default() -> Self {
        Self::Container {}
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectCopies {
    source: FixtureObject,
    key_prefix: ObjectKey,
    key_suffix: String,
    count: NonZeroUsize,
}

impl MinioService {
    fn expanded_objects(&self) -> Result<BTreeMap<ObjectKey, &FixtureObject>> {
        let mut objects: BTreeMap<_, _> = self
            .objects
            .iter()
            .map(|(key, value)| (key.clone(), value))
            .collect();
        for copies in &self.copies {
            for index in 0..copies.count.get() {
                let key = ObjectKey::try_from(format!(
                    "{}{index}{}",
                    copies.key_prefix.0, copies.key_suffix
                ))?;
                ensure!(
                    !objects.contains_key(&key),
                    "duplicate object key {}",
                    key.0
                );
                objects.insert(key, &copies.source);
            }
        }
        Ok(objects)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureObject {
    fixture: Name,
    path: FixturePath,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct ImageTag(String);

impl TryFrom<String> for ImageTag {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            value.starts_with("RELEASE.")
                && value.len() <= 128
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)),
            "MinIO image_tag must be an explicit RELEASE tag"
        );
        Ok(Self(value))
    }
}
impl From<ImageTag> for String {
    fn from(value: ImageTag) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct BucketName(String);

impl TryFrom<String> for BucketName {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            (3..=63).contains(&value.len())
                && value
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !value.starts_with('-')
                && !value.ends_with('-'),
            "test bucket must contain 3–63 lowercase letters, digits or internal hyphens"
        );
        Ok(Self(value))
    }
}
impl From<BucketName> for String {
    fn from(value: BucketName) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct ObjectKey(String);

impl TryFrom<String> for ObjectKey {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            !value.is_empty()
                && value.len() <= 1024
                && !value.starts_with('/')
                && !value.chars().any(char::is_control)
                && !value.split('/').any(|part| matches!(part, "." | "..")),
            "object key must be a nonempty relative S3 key without control characters or dot segments"
        );
        Ok(Self(value))
    }
}
impl From<ObjectKey> for String {
    fn from(value: ObjectKey) -> Self {
        value.0
    }
}

impl Service {
    pub fn bucket(&self) -> &str {
        let Self::Minio(spec) = self;
        &spec.bucket.0
    }

    pub fn fixtures(&self) -> impl Iterator<Item = &Name> {
        let Self::Minio(spec) = self;
        spec.objects
            .values()
            .chain(spec.copies.iter().map(|copies| &copies.source))
            .map(|object| &object.fixture)
    }

    pub fn validate(&self, fixtures: &BTreeMap<Name, Fixture>) -> Result<()> {
        let Self::Minio(spec) = self;
        for (key, object) in spec.expanded_objects()? {
            let fixture = fixtures
                .get(&object.fixture)
                .with_context(|| format!("unknown service fixture {}", object.fixture))?;
            ensure!(
                fixture
                    .outputs()
                    .iter()
                    .any(|path| path == object.path.as_ref()),
                "object {} references an undeclared output of fixture {}",
                key.0,
                object.fixture
            );
        }
        Ok(())
    }

    pub fn stage(&self, fixtures: &Path, directory: &Path) -> Result<()> {
        let Self::Minio(spec) = self;
        for (key, object) in spec.expanded_objects()? {
            let destination = directory.join(".sqltest-objects").join(&key.0);
            ensure!(
                !destination.exists(),
                "object fixture destination already exists: {}",
                destination.display()
            );
            fs::create_dir_all(destination.parent().context("object key has no parent")?)?;
            fs::copy(
                fixtures.join(object.fixture.as_ref()).join(&object.path),
                &destination,
            )?;
        }
        Ok(())
    }

    pub fn start(&self, fixtures: &Path) -> Result<RunningService> {
        let Self::Minio(spec) = self;
        let request = MinIO::default()
            .with_name(IMAGE_NAME)
            .with_tag(&spec.image_tag.0)
            .with_env_var("MINIO_ROOT_USER", ACCESS_KEY)
            .with_env_var("MINIO_ROOT_PASSWORD", SECRET_KEY)
            .with_env_var("MINIO_REGION", REGION)
            .with_startup_timeout(Duration::from_secs(60));
        let request = match &spec.storage {
            ServiceStorage::Container {} => request,
            ServiceStorage::Tmpfs { size_mib } => request.with_mount(
                Mount::tmpfs_mount("/data")
                    .with_size_bytes(i64::from(size_mib.get()) * 1024 * 1024),
            ),
        };
        let container = request
            .start()
            .context("start managed MinIO; a reachable Docker daemon is required (Testcontainers honors DOCKER_HOST)")?;
        let endpoint = format!(
            "http://{}:{}",
            container.get_host()?,
            container.get_host_port_ipv4(9000)?
        );
        let created = Bucket::create_with_path_style(
            &spec.bucket.0,
            Region::Custom {
                region: REGION.into(),
                endpoint: endpoint.clone(),
            },
            Credentials::new(Some(ACCESS_KEY), Some(SECRET_KEY), None, None, None)?,
            BucketConfiguration::default(),
        )
        .context("create managed MinIO bucket")?;
        ensure!(
            (200..300).contains(&created.response_code),
            "create bucket returned {}",
            created.response_code
        );
        let mut bucket = created.bucket;
        bucket.set_request_timeout(Some(Duration::from_secs(30)));
        let mut objects = BTreeMap::new();
        for (key, object) in spec.expanded_objects()? {
            let path = fixtures.join(object.fixture.as_ref()).join(&object.path);
            let digest = crate::corpus::file_hash(&path)?;
            let status = bucket
                .put_object_stream(&mut fs::File::open(&path)?, &key.0)
                .with_context(|| format!("upload object {}", key.0))?;
            ensure!(
                (200..300).contains(&status),
                "upload {} returned {status}",
                key.0
            );
            objects.insert(key.0.clone(), digest);
        }
        Ok(RunningService {
            metadata: ServiceMetadata {
                container_id: container.id().into(),
                image: format!("{IMAGE_NAME}:{}", spec.image_tag.0),
                storage: spec.storage.clone(),
                endpoint,
                bucket: spec.bucket.0.clone(),
                objects,
            },
            container,
        })
    }
}

#[derive(Serialize)]
pub struct ServiceMetadata {
    container_id: String,
    image: String,
    storage: ServiceStorage,
    endpoint: String,
    bucket: String,
    objects: BTreeMap<String, String>,
}

pub struct RunningService {
    pub metadata: ServiceMetadata,
    container: Container<MinIO>,
}

impl RunningService {
    pub fn write_profile(&self, source: &Path, destination: &Path) -> Result<()> {
        write_profile(&self.metadata.endpoint, source, destination)
    }

    pub fn save_logs(&self, directory: &Path) -> Result<()> {
        fs::write(
            directory.join("minio.stdout.log"),
            self.container.stdout_to_vec()?,
        )?;
        fs::write(
            directory.join("minio.stderr.log"),
            self.container.stderr_to_vec()?,
        )?;
        Ok(())
    }
}

fn write_profile(endpoint: &str, source: &Path, destination: &Path) -> Result<()> {
    let mut profile: serde_yaml::Value = serde_yaml::from_slice(&fs::read(source)?)?;
    let mut store = &mut profile;
    for key in ["sirius", "executor", "scan_manager", "object_store"] {
        store = store
            .as_mapping_mut()
            .context("Sirius profile object_store ancestors must be YAML mappings")?
            .entry(key.into())
            .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    }
    let store = store
        .as_mapping_mut()
        .context("Sirius profile object_store must be a YAML mapping")?;
    for (key, value) in [
        ("endpoint", endpoint),
        ("region", REGION),
        ("access_key", ACCESS_KEY),
        ("secret_key", SECRET_KEY),
    ] {
        store.insert(key.into(), value.into());
    }
    store.remove(serde_yaml::Value::from("session_token"));
    fs::write(destination, serde_yaml::to_string(&profile)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Service {
        toml::from_str(
            r#"
kind = "minio"
image_tag = "RELEASE.2025-09-07T16-13-09Z-cpuv1"
bucket = "sirius-test"
storage = { kind = "tmpfs", size_mib = 2048 }
[objects]
"encoded/a%2Fb?x#y.parquet" = { fixture = "sample", path = "data.parquet" }
"literal/a/b.parquet" = { fixture = "sample", path = "other.parquet" }
"#,
        )
        .unwrap()
    }

    #[test]
    fn service_keys_preserve_literal_punctuation_and_validate_outputs() {
        let spec = spec();
        let fixtures = BTreeMap::from([(
            "sample".parse().unwrap(),
            toml::from_str(
                r#"
files = { "data.parquet" = "input", "other.parquet" = "input" }
"#,
            )
            .unwrap(),
        )]);
        spec.validate(&fixtures).unwrap();
        let serialized = toml::to_string(&spec).unwrap();
        assert!(serialized.contains("encoded/a%2Fb?x#y.parquet"));
        assert!(spec.validate(&BTreeMap::new()).is_err());
        let missing_output = BTreeMap::from([(
            "sample".parse().unwrap(),
            toml::from_str(
                r#"
files = { "missing.parquet" = "input" }
"#,
            )
            .unwrap(),
        )]);
        assert!(spec.validate(&missing_output).is_err());
        assert!(ObjectKey::try_from(String::from("../escape")).is_err());
        assert!(BucketName::try_from(String::from("INVALID")).is_err());
        assert!(ImageTag::try_from(String::from("latest")).is_err());
    }

    #[test]
    fn storage_requires_an_explicit_positive_tmpfs_limit() {
        let serialized = toml::to_string(&spec()).unwrap();
        assert!(toml::from_str::<Service>(&serialized).is_ok());
        for storage in [
            "kind = 'tmpfs'",
            "kind = 'tmpfs'\nsize_mib = 0",
            "kind = 'tmpfs'\nsize_mib = -1",
            "kind = 'tmpfs'\nsize_mib = 4294967296",
            "kind = 'container'\nsize_mib = 2048",
            "kind = 'unknown'",
        ] {
            assert!(
                toml::from_str::<ServiceStorage>(storage).is_err(),
                "{storage}"
            );
        }
    }

    #[test]
    fn counted_copies_preserve_keys_and_reject_collisions() {
        let text = r#"
kind = "minio"
image_tag = "RELEASE.2025-09-07T16-13-09Z-cpuv1"
bucket = "sirius-test"
[[copies]]
source = { fixture = "sample", path = "data.parquet" }
key_prefix = "glob-scale/part_"
key_suffix = ".parquet"
count = 1001
"#;
        let Service::Minio(spec) = toml::from_str(text).unwrap();
        let objects = spec.expanded_objects().unwrap();
        assert_eq!(objects.len(), 1001);
        for key in ["glob-scale/part_0.parquet", "glob-scale/part_1000.parquet"] {
            assert!(objects.contains_key(&ObjectKey::try_from(key.to_owned()).unwrap()));
        }
        assert!(toml::from_str::<Service>(&text.replace("count = 1001", "count = 0")).is_err());
        let Service::Minio(invalid) = toml::from_str(
            &text.replace("key_suffix = \".parquet\"", "key_suffix = \"/../escape\""),
        )
        .unwrap();
        assert!(invalid.expanded_objects().is_err());
        let Service::Minio(collision) = toml::from_str(&format!("{text}\n[objects]\n\"glob-scale/part_0.parquet\" = {{ fixture = \"sample\", path = \"data.parquet\" }}\n")).unwrap();
        assert!(collision.expanded_objects().is_err());
        let Service::Minio(duplicate) = toml::from_str(&format!(
            "{text}\n[[copies]]{}",
            text.split_once("[[copies]]").unwrap().1
        ))
        .unwrap();
        assert!(duplicate.expanded_objects().is_err());
    }

    #[test]
    fn profile_binding_preserves_settings_and_rejects_scalar_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input.yaml");
        let output = dir.path().join("output.yaml");
        fs::write(&source, "sirius:\n  executor:\n    scan_manager:\n      object_store:\n        tls_verify: false\n        session_token: stale\n  topology:\n    num_gpus: 2\n").unwrap();
        write_profile("http://127.0.0.1:1234", &source, &output).unwrap();
        let bound: serde_yaml::Value = serde_yaml::from_slice(&fs::read(&output).unwrap()).unwrap();
        let store = &bound["sirius"]["executor"]["scan_manager"]["object_store"];
        assert_eq!(store["endpoint"].as_str(), Some("http://127.0.0.1:1234"));
        assert_eq!(store["tls_verify"].as_bool(), Some(false));
        assert!(store["session_token"].is_null());
        assert_eq!(bound["sirius"]["topology"]["num_gpus"].as_i64(), Some(2));
        fs::write(&source, "sirius: wrong\n").unwrap();
        assert!(write_profile("http://localhost:1234", &source, &output).is_err());
    }

    #[test]
    #[ignore = "requires a Docker daemon and access to the pinned MinIO image"]
    fn managed_minio_uploads_exact_keys_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sample")).unwrap();
        fs::write(dir.path().join("sample/data.parquet"), b"encoded").unwrap();
        fs::write(dir.path().join("sample/other.parquet"), b"literal").unwrap();
        let mut recipe = spec();
        let Service::Minio(spec) = &mut recipe;
        spec.copies.push(ObjectCopies {
            source: FixtureObject {
                fixture: "sample".parse().unwrap(),
                path: Path::new("data.parquet").to_path_buf().try_into().unwrap(),
            },
            key_prefix: "copies/part_".to_owned().try_into().unwrap(),
            key_suffix: ".parquet".into(),
            count: NonZeroUsize::new(3).unwrap(),
        });
        let service = recipe.start(dir.path()).unwrap();
        assert_eq!(service.metadata.objects.len(), 5);
        let bucket = Bucket::new(
            &service.metadata.bucket,
            Region::Custom {
                region: REGION.into(),
                endpoint: service.metadata.endpoint.clone(),
            },
            Credentials::new(Some(ACCESS_KEY), Some(SECRET_KEY), None, None, None).unwrap(),
        )
        .unwrap()
        .with_path_style();
        assert_eq!(
            bucket
                .get_object("encoded/a%2Fb?x#y.parquet")
                .unwrap()
                .as_slice(),
            b"encoded"
        );
        assert_eq!(
            bucket.get_object("literal/a/b.parquet").unwrap().as_slice(),
            b"literal"
        );
        for index in 0..3 {
            assert_eq!(
                bucket
                    .get_object(format!("copies/part_{index}.parquet"))
                    .unwrap()
                    .as_slice(),
                b"encoded"
            );
        }
        assert!(bucket.get_object("copies/part_3.parquet").is_err());
        service.save_logs(dir.path()).unwrap();
        drop(service);
        assert!(bucket.get_object("literal/a/b.parquet").is_err());
    }
}
