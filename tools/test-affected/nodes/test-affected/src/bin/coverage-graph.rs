//! Per-test coverage graph indexing for the default-branch coverage pipeline.
//!
//! `scripts/run-cov.sh` builds this command during full coverage-graph
//! collection; see `scripts/coverage_graph.py` for the producing pipeline.

use clap::{Parser, Subcommand};
use mfn_test_affected::graph::{CoverageManifest, Error, GraphWriter, SCHEMA_VERSION};
use serde_json::json;
use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

/// Index per-test coverage exports into a versioned SQLite graph.
#[derive(Debug, Parser)]
#[command(name = "coverage-graph", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Index a per-test coverage manifest into a SQLite graph.
    Index {
        /// Coverage manifest JSON (see graph::CoverageManifest).
        manifest: PathBuf,
        /// Output SQLite graph path; the commit must not already be indexed.
        output: PathBuf,
        /// Workspace root used to resolve lcov source paths.
        #[arg(long, value_name = "PATH", default_value = ".")]
        workspace_root: PathBuf,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(summary) => {
            // Collection-side reporting for the publishing pipeline.
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<serde_json::Value, Error> {
    match cli.command {
        Command::Index {
            manifest,
            output,
            workspace_root,
        } => {
            let manifest = CoverageManifest::read(&manifest)?;
            let mut writer = GraphWriter::create(&output)?;
            let indexed = writer.index(&manifest, &workspace_root)?;
            Ok(json!({
                "commit_sha": indexed.commit_sha,
                "tests": indexed.tests,
                "files": indexed.files,
                "ranges": indexed.ranges,
                "schema_version": SCHEMA_VERSION,
            }))
        }
    }
}
