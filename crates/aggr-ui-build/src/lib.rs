//! Node-free reader compilation, bundling and immutable asset generations.
mod compiler;
mod vendor;
pub use vendor::verify_vendor;
mod build;
pub use build::{BuildResult, build};
mod bundle;
pub use bundle::{BundleOptions, BundleOutput, bundle};
pub use compiler::{Compiled, compile_component, compile_module, strip_typescript};

/// Vendored, version-locked JavaScript sources used by the native toolchain.
pub fn vendor_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor")
}
