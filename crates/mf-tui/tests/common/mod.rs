#![allow(dead_code)]

use mf_runtime::{
    WorkflowInputSchema, WorkflowInterface, WorkflowInterfaceVersion, WorkflowManifest,
    WorkflowManifestVersion,
};
use mf_telemetry::{
    description::{WorkflowDescription, WorkflowDescriptionVersion},
    identity::WorkflowId,
};

pub fn sample_manifest() -> WorkflowManifest {
    let workflow_id = WorkflowId::try_from(format!("sha256:{}", "a".repeat(64))).unwrap();
    WorkflowManifest {
        version: WorkflowManifestVersion::V2026_10_07,
        description: WorkflowDescription {
            version: WorkflowDescriptionVersion::V2026_09_27,
            workflow_id: workflow_id.clone(),
            execution: None,
            nodes: vec![],
            data_edges: vec![],
            control_edges: vec![],
            execution_order: vec![],
            loop_bodies: vec![],
        },
        interface: WorkflowInterface {
            version: WorkflowInterfaceVersion::V2026_10_03,
            workflow_id,
            schema: WorkflowInputSchema::default(),
        },
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
