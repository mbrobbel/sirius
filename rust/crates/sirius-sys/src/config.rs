//! Bridge to the public C++ configuration API.

#[cxx::bridge(namespace = "sirius::rust_bridge")]
pub mod bridge {
    /// Configuration failure categories from the C++ API.
    enum ConfigErrorCode {
        Io,
        MalformedYaml,
        InvalidConfiguration,
        InvalidPath,
    }

    struct BuilderResult {
        value: UniquePtr<ContextConfigBuilder>,
        code: ConfigErrorCode,
        message: String,
    }

    struct ConfigResult {
        value: UniquePtr<ContextConfig>,
        code: ConfigErrorCode,
        message: String,
    }

    unsafe extern "C++" {
        include!("config_bridge.hpp");

        #[namespace = "sirius"]
        type ContextConfig;
        #[namespace = "sirius"]
        type ContextConfigBuilder;

        fn config_builder_defaults() -> Result<UniquePtr<ContextConfigBuilder>>;
        fn config_builder_from_yaml(path: &CxxString) -> Result<BuilderResult>;
        fn config_builder_copy(
            builder: &ContextConfigBuilder,
        ) -> Result<UniquePtr<ContextConfigBuilder>>;
        fn config_build(builder: &ContextConfigBuilder) -> Result<ConfigResult>;
        fn config_copy(config: &ContextConfig) -> Result<UniquePtr<ContextConfig>>;
    }
}
