use std::{fs, path::Path, process::Command};

/// Keep the executable inspection boundary real, including fixtures that must never launch.
pub fn write(path: &Path, payload: Option<&[u8]>, script: &str) {
    let source = path.with_extension("rs");
    let (section, retain) = match payload {
        Some(bytes) => {
            let data = path.with_extension("manifest");
            fs::write(&data, bytes).unwrap();
            (
                format!(
                    r#"#[used]
#[cfg_attr(target_os = "linux", unsafe(link_section = ".mf_manifest"))]
#[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__mf_manifest"))]
static MANIFEST: [u8; include_bytes!({data:?}).len()] = *include_bytes!({data:?});"#
                ),
                "std::hint::black_box(&MANIFEST);",
            )
        }
        None => (String::new(), ""),
    };
    fs::write(&source, format!(r#"use std::os::unix::process::CommandExt;
{section}
fn main() {{
    {retain}
    let error = std::process::Command::new("sh").arg("-c").arg({script:?}).arg(std::env::current_exe().unwrap()).args(std::env::args_os().skip(1)).exec();
    panic!("fixture failed: {{error}}");
}}
"#)).unwrap();
    let output = Command::new("rustc")
        .arg(&source)
        .args(["--crate-name", "fixture_runner", "-o"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}
