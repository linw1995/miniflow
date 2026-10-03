#![allow(dead_code)]

use mf_runtime::{
    ChannelSender, PreparedStream, StreamError, StreamInstance, StreamOptions, StreamSummary,
};

pub struct ChannelRun {
    pub instance: StreamInstance,
    pub source: ChannelSender,
}

impl std::ops::Deref for ChannelRun {
    type Target = StreamInstance;
    fn deref(&self) -> &Self::Target {
        &self.instance
    }
}

impl ChannelRun {
    pub fn start(
        mut prepared: PreparedStream,
        source_node: &str,
        options: StreamOptions,
    ) -> Result<Self, StreamError> {
        let source = prepared.channel(source_node)?;
        let instance = prepared.start_with_options(options)?;
        Ok(Self { instance, source })
    }

    pub fn join(self) -> Result<StreamSummary, StreamError> {
        self.instance.join()
    }

    pub fn settled(&self) -> bool {
        let input = self.source.metrics();
        let summary = self.instance.summary();
        input.published == input.accepted
            && summary.completed_frames == summary.startup_frames + summary.emitted_messages
    }
}

pub fn source(item_type: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"id":"feed", "kind":"builtin.channel", "config":{"item_type":item_type}})
}
