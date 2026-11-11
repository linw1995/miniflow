#![allow(dead_code)]

use mf_runtime::{WorkflowInputSchema, WorkflowManifest, WorkflowManifestVersion};

/// An executable fixture with the same embedded-manifest boundary as a built
/// runner. Its script is replaceable independently of the manifest-bearing host.
#[cfg(unix)]
pub fn write_runner(path: &std::path::Path, manifest: &WorkflowManifest, script: &str) {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};
    let script_path = path.with_extension("script");
    fs::write(&script_path, script).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o700)).unwrap();
    let source = tempfile::tempdir().unwrap();
    let bytes = manifest.to_bytes().unwrap();
    let code = format!(
        r#"
        use std::os::unix::process::CommandExt;
        #[used]
        #[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__mf_manifest"))]
        #[cfg_attr(target_os = "linux", unsafe(link_section = ".mf_manifest"))]
        static MANIFEST: [u8; {length}] = {bytes:?};
        fn main() {{
            std::hint::black_box(&MANIFEST);
            let error = std::process::Command::new({script_path:?})
                .args(std::env::args_os().skip(1)).exec();
            panic!("fixture script failed: {{error}}");
        }}
    "#,
        length = bytes.len()
    );
    let rust = source.path().join("runner.rs");
    fs::write(&rust, code).unwrap();
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&rust)
        .arg("-o")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn sample_manifest() -> WorkflowManifest {
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_11_06,
        graph: mf_runtime::CompiledGraph {
            definition: serde_json::from_value(serde_json::json!({
                "version": "2026-10-03", "dependencies": {}, "nodes": []
            }))
            .unwrap(),
            execution_order: vec![],
            relations: Default::default(),
            children: Default::default(),
        },
        input_schema: WorkflowInputSchema::default(),
    }
}

pub fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
pub fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
pub fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub fn elf(manifest: Option<&[u8]>, copies: usize) -> Vec<u8> {
    let names = b"\0.shstrtab\0.mf_manifest\0";
    let count = 2 + copies;
    let table_end = 64 + count * 64;
    let payload_offset = table_end + names.len();
    let payload = manifest.unwrap_or_default();
    let mut bytes = vec![0; payload_offset + payload.len()];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    put16(&mut bytes, 16, 2);
    put16(&mut bytes, 18, 62);
    put32(&mut bytes, 20, 1);
    put64(&mut bytes, 40, 64);
    put16(&mut bytes, 52, 64);
    put16(&mut bytes, 54, 56);
    put16(&mut bytes, 58, 64);
    put16(&mut bytes, 60, count as u16);
    put16(&mut bytes, 62, 1);
    put32(&mut bytes, 128, 1);
    put32(&mut bytes, 132, 3);
    put64(&mut bytes, 152, table_end as u64);
    put64(&mut bytes, 160, names.len() as u64);
    for index in 0..copies {
        let offset = 192 + index * 64;
        put32(&mut bytes, offset, 11);
        put32(&mut bytes, offset + 4, 1);
        put64(&mut bytes, offset + 24, payload_offset as u64);
        put64(&mut bytes, offset + 32, payload.len() as u64);
    }
    bytes[table_end..payload_offset].copy_from_slice(names);
    bytes[payload_offset..].copy_from_slice(payload);
    bytes
}

pub fn macho(manifest: Option<&[u8]>, copies: usize) -> Vec<u8> {
    let command_size = 72 + copies * 80;
    let payload_offset = 32 + command_size;
    let payload = manifest.unwrap_or_default();
    let mut bytes = vec![0; payload_offset + payload.len()];
    let length = bytes.len() as u64;
    put32(&mut bytes, 0, 0xfeedfacf);
    put32(&mut bytes, 4, 0x01000007);
    put32(&mut bytes, 8, 3);
    put32(&mut bytes, 12, 2);
    put32(&mut bytes, 16, 1);
    put32(&mut bytes, 20, command_size as u32);
    put32(&mut bytes, 32, 0x19);
    put32(&mut bytes, 36, command_size as u32);
    bytes[40..46].copy_from_slice(b"__DATA");
    put64(&mut bytes, 64, length);
    put64(&mut bytes, 80, length);
    put32(&mut bytes, 88, 3);
    put32(&mut bytes, 92, 3);
    put32(&mut bytes, 96, copies as u32);
    for index in 0..copies {
        let offset = 104 + index * 80;
        bytes[offset..offset + 13].copy_from_slice(b"__mf_manifest");
        bytes[offset + 16..offset + 22].copy_from_slice(b"__DATA");
        put64(&mut bytes, offset + 40, payload.len() as u64);
        put32(&mut bytes, offset + 48, payload_offset as u32);
    }
    bytes[payload_offset..].copy_from_slice(payload);
    bytes
}
