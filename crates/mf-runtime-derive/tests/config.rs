// Test the expansion implementation in a regular binary. Proc-macro unit binaries
// depend on dynamically loaded compiler libraries unavailable to standalone runners.
#[path = "../src/config.rs"]
mod config;
#[path = "support/config_expansion.rs"]
mod expansion;
