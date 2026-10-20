#![cfg(unix)]

use mf_runtime::{
    ExecutionContext, NodeExecutionError, NodeMetadata, NodeRegistration, NodeValue, PreparedNode,
    TypedNodeResult, TypedTaskNode,
};
use std::{
    fs,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

#[derive(NodeValue)]
struct Input {
    directory: String,
    block: bool,
}

#[derive(NodeValue)]
struct Output {
    finished: bool,
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.0.kill().unwrap();
        self.0.wait().unwrap();
    }
}

struct Wait;

impl TypedTaskNode for Wait {
    type Input = Input;
    type Output = Output;

    fn execute(
        &self,
        input: Input,
        context: &mut ExecutionContext,
    ) -> Result<TypedNodeResult<Output>, NodeExecutionError> {
        let directory = PathBuf::from(input.directory);
        let child = (!input.block).then(|| {
            OwnedChild(
                Command::new("sleep")
                    .arg("60")
                    .process_group(0)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        });
        if let Some(child) = &child {
            fs::write(directory.join("child"), child.0.id().to_string()).unwrap();
        }
        fs::write(directory.join("ready"), b"ready").unwrap();
        loop {
            if !input.block
                && let Some(error) = context.cancellation().failure()
            {
                drop(child);
                fs::write(directory.join("cleaned"), b"cleaned").unwrap();
                return Err(error.into());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

inventory::submit! {
    NodeRegistration {
        kind: "fixture.wait_for_interrupt",
        factory: mf_runtime::NodeFactory::Plain(|_| {
            PreparedNode::typed_task(Wait, NodeMetadata::default())
        }),
    }
}
