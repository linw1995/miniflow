//! Opt-in coding tools for trusted workflows on Linux and macOS.
#![cfg(any(target_os = "linux", target_os = "macos"))]

mod bash;
mod discovery;
mod error;
mod file;
mod find;
mod grep;
mod ls;
mod workspace;

pub use bash::BASH_KIND;
pub use error::CodingError;
pub use file::{EDIT_KIND, MKDIR_KIND, READ_KIND, WRITE_KIND};
pub use find::FIND_KIND;
pub use grep::GREP_KIND;
pub use ls::LS_KIND;

use mf_runtime::{ExecutionContext, Shared, ValueRef};

fn check_cancelled(context: &ExecutionContext) -> Result<(), CodingError> {
    match context.cancellation().failure() {
        Some(source) => Err(source.into()),
        None => Ok(()),
    }
}

fn text(value: &Shared<String>) -> &str {
    value.value().as_str().expect("typed string")
}

fn private_text(value: String) -> Shared<String> {
    Shared::new(ValueRef::from(value).into_secret()).expect("typed string")
}
