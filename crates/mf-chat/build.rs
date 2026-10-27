#[path = "build/title.rs"]
mod title;

use std::{env, fs, path::Path, process::Command};

fn cargo_manifest_output(
    command: &str,
    manifest: &Path,
    arguments: &[&str],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let output = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .arg(command)
        .arg("--manifest-path")
        .arg(manifest)
        .args(arguments)
        .output()?;
    snafu::ensure!(
        output.status.success(),
        title::CargoManifestSnafu {
            command,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr)
        }
    );
    Ok(output.stdout)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=migrations");
    let manifest = Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("Cargo.toml");
    let package = serde_json::from_slice(&cargo_manifest_output("read-manifest", &manifest, &[])?)?;
    let workspace: serde_json::Value = serde_json::from_slice(&cargo_manifest_output(
        "locate-project",
        &manifest,
        &["--workspace"],
    )?)?;
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!(
        "cargo:rerun-if-changed={}",
        workspace["root"]
            .as_str()
            .expect("Cargo workspace manifest")
    );
    println!("cargo:rerun-if-changed=workflows/title.json");
    let mut definition = serde_json::from_slice(&fs::read("workflows/title.json")?)?;
    title::bind_dependencies(&mut definition, &package)?;
    fs::write(
        Path::new(&env::var("OUT_DIR")?).join("title.json"),
        serde_json::to_vec(&definition)?,
    )?;
    Ok(())
}
