//! Immutable configuration construction without GPU access.

use std::path::Path;

use cxx::{UniquePtr, let_cxx_string};
use sirius_sys::config::bridge;

/// A failure while constructing or copying a configuration.
#[derive(Debug)]
pub enum ConfigError {
    /// A configuration file could not be opened or read.
    Io(String),
    /// The input could not be parsed as YAML.
    MalformedYaml(String),
    /// Settings are unknown, invalid, or conflicting.
    InvalidConfiguration(String),
    /// A native allocation or bridge operation failed.
    Native(cxx::Exception),
    /// The platform path cannot be represented by the native bridge.
    InvalidPath,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message)
            | Self::MalformedYaml(message)
            | Self::InvalidConfiguration(message) => f.write_str(message),
            Self::Native(error) => error.fmt(f),
            Self::InvalidPath => f.write_str("configuration path cannot be represented"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Native(error) => Some(error),
            _ => None,
        }
    }
}

impl From<cxx::Exception> for ConfigError {
    fn from(error: cxx::Exception) -> Self {
        Self::Native(error)
    }
}

fn configuration_error(code: bridge::ConfigErrorCode, message: String) -> ConfigError {
    match code {
        bridge::ConfigErrorCode::Io => ConfigError::Io(message),
        bridge::ConfigErrorCode::MalformedYaml => ConfigError::MalformedYaml(message),
        bridge::ConfigErrorCode::InvalidPath => ConfigError::InvalidPath,
        _ => ConfigError::InvalidConfiguration(message),
    }
}

/// An immutable configuration whose settings have been validated without accessing GPUs.
///
/// Owns its native snapshot independently of its builder or source file. Hardware
/// availability and capacity are checked when the engine is constructed.
/// No thread-safety guarantees are exposed yet: this type is neither `Send` nor `Sync`.
pub struct ContextConfig {
    pub(crate) inner: UniquePtr<bridge::ContextConfig>,
}

impl ContextConfig {
    /// Share the immutable settings through a new owned handle.
    ///
    /// ```no_run
    /// # use sirius::{ConfigError, ContextConfigBuilder};
    /// # fn example() -> Result<(), ConfigError> {
    /// let config = ContextConfigBuilder::new()?.build()?;
    /// let copy = config.try_clone()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_clone(&self) -> Result<Self, ConfigError> {
        Ok(Self {
            inner: bridge::config_copy(self.inner.as_ref().expect("owned configuration"))?,
        })
    }
}

/// Construct configurations from built-in defaults or a YAML file.
///
/// Parsing and validation are performed by the public C++ API. Loading and
/// building do not initialize CUDA, discover hardware, or allocate engine resources.
/// This type is neither `Send` nor `Sync`.
pub struct ContextConfigBuilder {
    inner: UniquePtr<bridge::ContextConfigBuilder>,
}

impl ContextConfigBuilder {
    /// Start with built-in defaults. Native allocation failures are returned as errors.
    ///
    /// ```no_run
    /// # use sirius::{ConfigError, ContextConfigBuilder};
    /// # fn example() -> Result<(), ConfigError> {
    /// let config = ContextConfigBuilder::new()?.build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new() -> Result<Self, ConfigError> {
        Ok(Self {
            inner: bridge::config_builder_defaults()?,
        })
    }

    /// Read and validate a YAML file, retaining its settings for later builds.
    ///
    /// Subsequent edits or deletion of the file do not affect this builder.
    /// Uses the [Sirius YAML schema](https://github.com/sirius-db/sirius/blob/main/docs/super-sirius/configuration.md).
    /// Unix path bytes are preserved, including non-UTF-8 names. NUL bytes are rejected.
    ///
    /// ```no_run
    /// # use sirius::{ConfigError, ContextConfigBuilder};
    /// # fn example() -> Result<(), ConfigError> {
    /// let config = ContextConfigBuilder::from_yaml("sirius.yaml")?.build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_yaml(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStrExt;
            path.as_os_str().as_bytes()
        };
        #[cfg(not(unix))]
        let bytes = path.to_str().ok_or(ConfigError::InvalidPath)?.as_bytes();
        let_cxx_string!(native_path = bytes);
        let result = bridge::config_builder_from_yaml(&native_path)?;
        if result.value.is_null() {
            return Err(configuration_error(result.code, result.message));
        }
        Ok(Self {
            inner: result.value,
        })
    }

    /// Produce an immutable configuration without rereading the file or accessing GPUs.
    ///
    /// ```no_run
    /// # use sirius::{ConfigError, ContextConfigBuilder};
    /// # fn example() -> Result<(), ConfigError> {
    /// let builder = ContextConfigBuilder::from_yaml("sirius.yaml")?;
    /// let config = builder.build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn build(&self) -> Result<ContextConfig, ConfigError> {
        let result = bridge::config_build(self.inner.as_ref().expect("owned builder"))?;
        if result.value.is_null() {
            return Err(configuration_error(result.code, result.message));
        }
        Ok(ContextConfig {
            inner: result.value,
        })
    }

    /// Share the loaded settings through an independently owned builder handle.
    ///
    /// ```no_run
    /// # use sirius::{ConfigError, ContextConfigBuilder};
    /// # fn example() -> Result<(), ConfigError> {
    /// let builder = ContextConfigBuilder::from_yaml("sirius.yaml")?;
    /// let copy = builder.try_clone()?;
    /// drop(builder);
    /// let config = copy.build()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_clone(&self) -> Result<Self, ConfigError> {
        Ok(Self {
            inner: bridge::config_builder_copy(self.inner.as_ref().expect("owned builder"))?,
        })
    }
}
