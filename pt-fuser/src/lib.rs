pub mod analysis;
pub mod merge;
pub mod perfetto;
pub mod trace;

/// Cargo package version, followed by the short Git commit when it is available.
pub const VERSION: &str = env!("PT_FUSER_VERSION");
