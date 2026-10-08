//! Bridge to the public C++ engine context.

#[cxx::bridge(namespace = "sirius::rust_bridge")]
pub mod bridge {
    enum ContextErrorCode {
        InUse,
        Initialization,
    }

    struct ContextResult {
        value: UniquePtr<Context>,
        /// Only meaningful when value is null.
        code: ContextErrorCode,
        message: String,
    }

    unsafe extern "C++" {
        include!("context_bridge.hpp");

        #[namespace = "sirius"]
        type Context;
        #[namespace = "sirius"]
        type ContextConfig = crate::config::bridge::ContextConfig;

        fn context_create(config: &ContextConfig) -> Result<ContextResult>;
    }
}
