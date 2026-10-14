pub mod android;
pub mod base;
pub mod java_backend;
pub mod react_ts;
pub mod rust_crate;

use std::path::Path;
use std::sync::Arc;

pub use android::AndroidAdapter;
pub use base::BaseAdapter;
pub use java_backend::JavaBackendAdapter;
pub use react_ts::ReactTsAdapter;
pub use rust_crate::RustCrateAdapter;

#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolInfo {
    pub symbol_name: String,
    pub symbol_type: String, // "function", "class", "interface", "struct", "method"
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolSpan {
    pub symbol_name: String,
    pub symbol_type: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub body: String,
}

/// Dependency/vendored directories no stack wants searched or indexed: third-party sources checked
/// into the repo (NDK `external/`, `third_party/`, Go/PHP `vendor/`, ...) are large and not the user's code.
pub const COMMON_SKIP_DIRS: &[&str] = &[
    ".git", ".mysid", "node_modules", "external", "externals", "extern", "third_party", "third-party", "thirdparty", "3rdparty", "3rd-party",
    "3rd_party", "vendor", "contrib", "Pods",
];

/// Core StackAdapter trait following SOLID principles.
pub trait StackAdapter: Send + Sync {
    /// Human-friendly name of the stack profile
    fn name(&self) -> &'static str;

    /// Primary source directories (used for targeted searches / prioritization)
    fn source_dirs(&self) -> &[&'static str];

    /// Build, cache, and artifact directories to automatically skip
    fn skip_dirs(&self) -> &[&'static str];

    /// Workspace-relative directory suffixes (`/`-separated) skipped as a whole. For vendored code
    /// that can't be recognised by directory name alone (e.g. an NDK `include/` of third-party headers).
    fn skip_paths(&self) -> &[&'static str] {
        &[]
    }

    /// What walkers (search, index) call for every entry: name-based skips plus path-based skips.
    fn is_skipped_entry(&self, rel_path: &str, name: &str) -> bool {
        self.is_skipped_dir(name)
            || self.skip_paths().iter().any(|p| rel_path == *p || rel_path.ends_with(&format!("/{}", p)))
    }

    /// True for a directory name that must not be walked: stack-specific build/cache dirs plus
    /// the shared vendored-dependency names. Walkers (search, index) call this, not `skip_dirs`.
    fn is_skipped_dir(&self, name: &str) -> bool {
        self.skip_dirs().contains(&name) || COMMON_SKIP_DIRS.contains(&name)
    }

    /// Relevant source code file extensions
    fn file_extensions(&self) -> &[&'static str];

    /// Extract AST/regex symbols from a file's content
    fn extract_symbols(&self, content: &str, ext: &str) -> Vec<SymbolInfo>;

    /// Extract exact definition block for a given symbol name
    fn extract_definition(&self, content: &str, ext: &str, symbol: &str) -> Option<SymbolSpan>;
}

/// Detects workspace stack and returns the best matching adapter, or fallback BaseAdapter.
pub fn detect_adapter(workspace_root: &Path) -> Arc<dyn StackAdapter> {
    // 1. Android: check for AndroidManifest.xml, build.gradle/settings.gradle with Android markers, or app/src/main
    if workspace_root.join("app/src/main").exists()
        || workspace_root.join("AndroidManifest.xml").exists()
        || workspace_root.join("app/src/main/AndroidManifest.xml").exists()
    {
        return Arc::new(AndroidAdapter::new());
    }

    // 2. React / TypeScript / Node: check for package.json or tsconfig.json
    if workspace_root.join("package.json").exists() || workspace_root.join("tsconfig.json").exists() {
        return Arc::new(ReactTsAdapter::new());
    }

    // 3. Rust: check for Cargo.toml
    if workspace_root.join("Cargo.toml").exists() {
        return Arc::new(RustCrateAdapter::new());
    }

    // 4. Java / Kotlin backend: check for pom.xml or build.gradle without Android
    if workspace_root.join("pom.xml").exists()
        || (workspace_root.join("build.gradle").exists() && !workspace_root.join("app").exists())
    {
        return Arc::new(JavaBackendAdapter::new());
    }

    // 5. Fallback BaseAdapter for hybrid, polyglot, or unknown projects
    Arc::new(BaseAdapter::new())
}
