use crate::{
    WorkflowInputError, WorkflowInputSchema, WorkflowInterface,
    workflow_inputs::{from_unique_json, pointer},
};
use mf_telemetry::{ContractError, description::WorkflowDescription};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu, ensure};

pub const MANIFEST_MAGIC: &[u8; 8] = b"MFMANIF\0";
pub const MANIFEST_FRAMING_VERSION: u32 = 1;
pub const MANIFEST_HEADER_BYTES: usize = 20;
pub const MAX_MANIFEST_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_MANIFEST_PADDING_BYTES: usize = 4096;
pub const MAX_MANIFEST_SECTION_BYTES: usize =
    MANIFEST_HEADER_BYTES + MAX_MANIFEST_PAYLOAD_BYTES + MAX_MANIFEST_PADDING_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowManifestVersion {
    #[serde(rename = "2026-10-07")]
    V2026_10_07,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowManifest {
    pub version: WorkflowManifestVersion,
    pub description: WorkflowDescription,
    pub interface: WorkflowInterface,
}

#[derive(Debug, Snafu)]
pub enum WorkflowManifestError {
    #[snafu(display("compiled workflow interface drift at `{path}`: {component} differs"))]
    InterfaceDrift {
        path: String,
        component: &'static str,
    },
    #[snafu(display("invalid workflow manifest framing: {message}"))]
    Framing { message: &'static str },
    #[snafu(display("unsupported workflow manifest framing version {version}"))]
    UnsupportedFraming { version: u32 },
    #[snafu(display("workflow manifest exceeds the {limit}-byte limit"))]
    TooLarge { limit: usize },
    #[snafu(display("invalid workflow manifest JSON: {source}"))]
    Json { source: serde_json::Error },
    #[snafu(display("invalid workflow manifest graph: {source}"))]
    Graph { source: ContractError },
    #[snafu(display("invalid workflow manifest interface: {source}"))]
    Interface { source: WorkflowInputError },
}

impl WorkflowManifest {
    pub fn validate_prepared_schema(
        &self,
        actual: &WorkflowInputSchema,
    ) -> Result<(), WorkflowManifestError> {
        let expected = &self.interface.schema;
        for node in expected.inputs.keys().chain(actual.inputs.keys()) {
            let path = pointer("", node);
            let (Some(expected), Some(actual)) =
                (expected.inputs.get(node), actual.inputs.get(node))
            else {
                return InterfaceDriftSnafu {
                    path,
                    component: "initial node declaration",
                }
                .fail();
            };
            for port in expected.keys().chain(actual.keys()) {
                let path = pointer(&path, port);
                let (Some(expected), Some(actual)) = (expected.get(port), actual.get(port)) else {
                    return InterfaceDriftSnafu {
                        path,
                        component: "input port declaration",
                    }
                    .fail();
                };
                ensure!(
                    expected.value_type == actual.value_type,
                    InterfaceDriftSnafu {
                        path: path.clone(),
                        component: "input type"
                    }
                );
                ensure!(
                    expected.required == actual.required,
                    InterfaceDriftSnafu {
                        path,
                        component: "required flag"
                    }
                );
            }
        }
        for node in expected.stdin.keys().chain(actual.stdin.keys()) {
            ensure!(
                expected.stdin.get(node) == actual.stdin.get(node),
                InterfaceDriftSnafu {
                    path: pointer("", node),
                    component: "stdin ownership condition"
                }
            );
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), WorkflowManifestError> {
        self.description.validate().context(GraphSnafu)?;
        self.interface
            .validate_for_description(&self.description)
            .context(InterfaceSnafu)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WorkflowManifestError> {
        ensure!(
            bytes.len() <= MAX_MANIFEST_SECTION_BYTES,
            TooLargeSnafu {
                limit: MAX_MANIFEST_SECTION_BYTES
            }
        );
        ensure!(
            bytes.len() >= MANIFEST_HEADER_BYTES,
            FramingSnafu {
                message: "incomplete header"
            }
        );
        ensure!(
            &bytes[..8] == MANIFEST_MAGIC,
            FramingSnafu {
                message: "invalid magic"
            }
        );
        let version = u32::from_le_bytes(bytes[8..12].try_into().expect("checked header"));
        ensure!(
            version == MANIFEST_FRAMING_VERSION,
            UnsupportedFramingSnafu { version }
        );
        let length = u64::from_le_bytes(bytes[12..20].try_into().expect("checked header"));
        ensure!(
            length <= MAX_MANIFEST_PAYLOAD_BYTES as u64,
            TooLargeSnafu {
                limit: MAX_MANIFEST_PAYLOAD_BYTES
            }
        );
        let end = MANIFEST_HEADER_BYTES + length as usize;
        ensure!(
            end <= bytes.len(),
            FramingSnafu {
                message: "incomplete payload"
            }
        );
        let padding = &bytes[end..];
        ensure!(
            padding.len() <= MAX_MANIFEST_PADDING_BYTES && padding.iter().all(|byte| *byte == 0),
            FramingSnafu {
                message: "invalid alignment padding"
            }
        );
        let manifest: Self =
            from_unique_json(&bytes[MANIFEST_HEADER_BYTES..end]).context(JsonSnafu)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, WorkflowManifestError> {
        self.validate()?;
        let payload = serde_json::to_vec(self).context(JsonSnafu)?;
        ensure!(
            payload.len() <= MAX_MANIFEST_PAYLOAD_BYTES,
            TooLargeSnafu {
                limit: MAX_MANIFEST_PAYLOAD_BYTES
            }
        );
        let mut bytes = Vec::with_capacity(MANIFEST_HEADER_BYTES + payload.len());
        bytes.extend_from_slice(MANIFEST_MAGIC);
        bytes.extend_from_slice(&MANIFEST_FRAMING_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }
}
