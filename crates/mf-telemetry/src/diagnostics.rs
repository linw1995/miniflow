use crate::wire::WireRecord;
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct NodeKey {
    workflow: String,
    run: String,
    node: String,
    path: Vec<(String, u64)>,
    invocation: Option<u64>,
}

impl NodeKey {
    fn from_wire(wire: &WireRecord) -> Self {
        Self {
            workflow: attribute(wire, "mf.workflow.id").to_owned(),
            run: attribute(wire, "mf.run.id").to_owned(),
            node: attribute(wire, "mf.node.id").to_owned(),
            path: wire.body["loop_path"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| {
                    (
                        entry["loop_id"].as_str().unwrap_or("unknown").to_owned(),
                        number(&entry["index"]),
                    )
                })
                .collect(),
            invocation: wire
                .body
                .get("stream")
                .map(|stream| number(&stream["invocation"])),
        }
    }
}

#[derive(Clone)]
struct RunningNode {
    wire: WireRecord,
    started: Instant,
    next_reminder: Instant,
}

#[derive(Default)]
struct State {
    active: BTreeMap<NodeKey, RunningNode>,
    closed: bool,
}

impl State {
    fn observe(&mut self, wire: &WireRecord, now: Instant, interval: Duration) {
        match wire.event_name.as_str() {
            "mf.node.started" => {
                self.active.insert(
                    NodeKey::from_wire(wire),
                    RunningNode {
                        wire: wire.clone(),
                        started: now,
                        next_reminder: now + interval,
                    },
                );
            }
            "mf.node.finished" | "mf.node.skipped" => {
                self.active.remove(&NodeKey::from_wire(wire));
            }
            _ => {}
        }
    }

    fn finish_run(&mut self, workflow: &str, run: &str) {
        self.active
            .retain(|key, _| key.workflow != workflow || key.run != run);
    }

    fn due(&mut self, now: Instant, interval: Duration) -> Vec<RunningNode> {
        self.active
            .values_mut()
            .filter_map(|node| {
                if node.next_reminder > now {
                    return None;
                }
                node.next_reminder = now + interval;
                Some(node.clone())
            })
            .collect()
    }
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct Diagnostics {
    shared: Arc<Shared>,
    interval: Duration,
    verbosity: u8,
}

impl Diagnostics {
    pub fn new(interval: Duration, verbosity: u8) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        if let Err(error) = thread::Builder::new()
            .name("mf-verbose".into())
            .spawn(move || reminders(&worker, interval, verbosity))
        {
            shared.state.lock().unwrap().closed = true;
            let _ = writeln!(
                std::io::stderr().lock(),
                "periodic verbose updates unavailable: {error}"
            );
        }
        Self {
            shared,
            interval,
            verbosity,
        }
    }

    pub fn emit(&self, wire: &WireRecord) {
        {
            let mut state = self.shared.state.lock().unwrap();
            if !state.closed {
                state.observe(wire, Instant::now(), self.interval);
            }
            self.shared.wake.notify_one();
        }
        let mut line = Vec::new();
        if write_diagnostic(&mut line, wire, self.verbosity).is_ok() {
            let _ = std::io::stderr().lock().write_all(&line);
        }
    }

    pub fn finish_run(&self, workflow: &str, run: &str) {
        self.shared.state.lock().unwrap().finish_run(workflow, run);
        self.shared.wake.notify_one();
    }
}

impl Drop for Diagnostics {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.closed = true;
        state.active.clear();
        self.shared.wake.notify_one();
    }
}

fn reminders(shared: &Shared, interval: Duration, verbosity: u8) {
    loop {
        let mut state = shared.state.lock().unwrap();
        loop {
            if state.closed {
                return;
            }
            let next = state.active.values().map(|node| node.next_reminder).min();
            match next {
                Some(next) if next <= Instant::now() => break,
                Some(next) => {
                    state = shared
                        .wake
                        .wait_timeout(state, next.saturating_duration_since(Instant::now()))
                        .unwrap()
                        .0
                }
                None => state = shared.wake.wait(state).unwrap(),
            }
        }
        drop(state);
        // Recheck under the output lock so reminders cannot follow a completed node's diagnostic line.
        let mut output = std::io::stderr().lock();
        let now = Instant::now();
        let mut state = shared.state.lock().unwrap();
        if state.closed {
            return;
        }
        let due = state.due(now, interval);
        drop(state);
        for node in due {
            let mut line = Vec::new();
            if write_running(&mut line, &node, now, verbosity).is_ok() {
                let _ = output.write_all(&line);
            }
        }
    }
}

fn attribute<'a>(wire: &'a WireRecord, name: &str) -> &'a str {
    wire.attributes
        .get(name)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
}

fn number(value: &serde_json::Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .unwrap_or(0)
}

fn write_running(
    writer: &mut impl Write,
    node: &RunningNode,
    now: Instant,
    verbosity: u8,
) -> std::io::Result<()> {
    let elapsed = now.saturating_duration_since(node.started);
    write!(
        writer,
        "[verbose] +{:.3}ms node {:?} ({}) still running for {:.3}s",
        number(&node.wire.body["elapsed_ns"]) as f64 / 1_000_000.0
            + elapsed.as_secs_f64() * 1_000.0,
        attribute(&node.wire, "mf.node.id"),
        attribute(&node.wire, "mf.node.kind").escape_debug(),
        elapsed.as_secs_f64()
    )?;
    write_context(writer, &node.wire.body)?;
    if verbosity >= 2 {
        write_details(writer, &node.wire, "start event")?;
    }
    writeln!(writer)
}
fn write_diagnostic(
    writer: &mut impl Write,
    wire: &WireRecord,
    verbosity: u8,
) -> std::io::Result<()> {
    if verbosity >= 3 {
        return writeln!(
            writer,
            "[verbose] {}",
            serde_json::to_string(wire).expect("lifecycle records serialize to JSON")
        );
    }
    let body = &wire.body;
    write!(
        writer,
        "[verbose] +{:.3}ms ",
        number(&body["elapsed_ns"]) as f64 / 1_000_000.0
    )?;
    let node = attribute(wire, "mf.node.id");
    match wire.event_name.as_str() {
        "mf.workflow.started" => write!(
            writer,
            "workflow started (nodes: {}, run: {})",
            number(&body["node_count"]),
            attribute(wire, "mf.run.id")
        )?,
        "mf.workflow.finished" => {
            write!(writer, "workflow {}", attribute(wire, "mf.outcome"))?;
            if let Some(counts) = body.get("counts") {
                write!(
                    writer,
                    " ({} frames, {} messages, {} outputs)",
                    number(&counts["completed_frames"]),
                    number(&counts["emitted_messages"]),
                    number(&counts["delivered_outputs"])
                )?;
            } else {
                write!(
                    writer,
                    " (visited: {})",
                    number(&body["visited_node_count"])
                )?;
            }
        }
        "mf.node.started" => write!(
            writer,
            "node {node:?} ({}) started",
            attribute(wire, "mf.node.kind").escape_debug()
        )?,
        "mf.node.finished" => write!(writer, "node {node:?} {}", attribute(wire, "mf.outcome"))?,
        "mf.node.skipped" => {
            write!(writer, "node {node:?} skipped")?;
            if let Some(causes) = body["causes"].as_array() {
                for cause in causes {
                    write!(
                        writer,
                        " | gated by {:?}.{:?}",
                        cause["source_node"].as_str().unwrap_or("unknown"),
                        cause["source_output"].as_str().unwrap_or("unknown")
                    )?;
                }
            }
        }
        "mf.loop.pass.started" => write!(writer, "loop pass started")?,
        "mf.loop.pass.finished" => write!(
            writer,
            "loop pass {}",
            body["outcome"].as_str().unwrap_or("unknown")
        )?,
        "mf.batch.buffered" => write!(
            writer,
            "node {node:?} buffered {} items",
            number(&body["item_count"])
        )?,
        "mf.batch.flushed" => write!(
            writer,
            "node {node:?} flushed {} items ({})",
            number(&body["item_count"]),
            body["reason"].as_str().unwrap_or("unknown").escape_debug()
        )?,
        _ => write!(writer, "{}", wire.event_name.escape_debug())?,
    }
    if let Some(duration) = body.get("duration_ns") {
        write!(writer, " in {:.3}ms", number(duration) as f64 / 1_000_000.0)?;
    }
    for (key, label) in [
        ("produced_ports", "outputs"),
        ("skipped_ports", "skipped outputs"),
    ] {
        if let Some(ports) = body[key].as_array().filter(|ports| !ports.is_empty()) {
            write!(writer, " | {label}: ")?;
            for (index, port) in ports.iter().enumerate() {
                if index != 0 {
                    write!(writer, ", ")?;
                }
                write!(writer, "{:?}", port.as_str().unwrap_or("unknown"))?;
            }
        }
    }
    write_context(writer, body)?;
    if let Some(count) = body.get("emission_count") {
        write!(writer, " | emitted {}", number(count))?;
    }
    if let Some(failure) = body.get("failure") {
        let phase = wire
            .attributes
            .get("mf.failure.phase")
            .or_else(|| failure.get("phase"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        write!(
            writer,
            " | error ({}): {:?}",
            phase.escape_debug(),
            failure["message"].as_str().unwrap_or("unknown")
        )?;
    }
    if verbosity >= 2 {
        write_details(writer, wire, "event")?;
    }
    writeln!(writer)
}

fn write_details(writer: &mut impl Write, wire: &WireRecord, label: &str) -> std::io::Result<()> {
    write!(
        writer,
        " | {label}: {} #{} | workflow: {}",
        wire.event_name.escape_debug(),
        number(&wire.attributes["mf.event.sequence"]),
        attribute(wire, "mf.workflow.id").escape_debug()
    )?;
    if let Some(position) = wire.body.get("position") {
        write!(writer, " | position: {}", number(position))?;
    }
    if wire.event_name != "mf.node.started"
        && let Some(kind) = wire
            .attributes
            .get("mf.node.kind")
            .and_then(serde_json::Value::as_str)
    {
        write!(writer, " | kind: {}", kind.escape_debug())?;
    }
    Ok(())
}

fn write_context(writer: &mut impl Write, body: &serde_json::Value) -> std::io::Result<()> {
    if let Some(path) = body
        .get("loop_path")
        .or_else(|| body.get("path"))
        .and_then(serde_json::Value::as_array)
        .filter(|path| !path.is_empty())
    {
        write!(writer, " | loop: ")?;
        for (index, entry) in path.iter().enumerate() {
            if index != 0 {
                write!(writer, "/")?;
            }
            write!(
                writer,
                "{:?}#{}",
                entry["loop_id"].as_str().unwrap_or("unknown"),
                number(&entry["index"]) + 1
            )?;
        }
    }
    if let Some(summary) = body.get("loop_summary") {
        write!(
            writer,
            " | {} passes ({})",
            number(&summary["pass_count"]),
            summary["reason"]
                .as_str()
                .unwrap_or("unknown")
                .escape_debug()
        )?;
    }
    if let Some(stream) = body.get("stream") {
        write!(
            writer,
            " | invocation {} ({})",
            number(&stream["invocation"]),
            stream["trigger"]
                .as_str()
                .unwrap_or("unknown")
                .escape_debug()
        )?;
        if let Some(message) = stream.get("message") {
            write!(
                writer,
                ", message {}:{}",
                number(&message["domain"]),
                number(&message["sequence"])
            )?;
        }
        if let Some(parent) = stream.get("parent") {
            write!(writer, ", parent {}", number(parent))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(source: &str) -> WireRecord {
        serde_json::from_str(source).unwrap()
    }

    #[test]
    fn reminders_repeat_at_intervals_and_stop_at_node_completion() {
        let started = fixture(include_str!("../tests/fixtures/node-started.json"));
        let now = Instant::now();
        let interval = Duration::from_secs(60);
        let mut state = State::default();
        state.observe(&started, now, interval);
        assert!(
            state
                .due(now + Duration::from_secs(59), interval)
                .is_empty()
        );
        let first = state.due(now + interval, interval);
        assert_eq!(first.len(), 1);
        let mut bytes = Vec::new();
        write_running(&mut bytes, &first[0], now + interval, 1).unwrap();
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .contains("still running for 60.000s")
        );
        assert!(
            state
                .due(now + Duration::from_secs(119), interval)
                .is_empty()
        );
        assert_eq!(state.due(now + interval * 2, interval).len(), 1);
        let finished = fixture(include_str!("../tests/fixtures/node-succeeded.json"));
        state.observe(&finished, now + interval * 2, interval);
        assert!(state.due(now + interval * 3, interval).is_empty());
    }

    #[test]
    fn reminders_isolate_stream_invocations_and_clear_terminated_runs() {
        let mut first = fixture(include_str!("../tests/fixtures/node-started.json"));
        first.body["loop_path"] = json!([{"loop_id":"outer", "index":0}]);
        first.body["stream"] =
            json!({"invocation":"1", "trigger":"message", "message":{"domain":0,"sequence":"1"}});
        let mut second = first.clone();
        second.body["stream"]["invocation"] = json!("2");
        let mut third = first.clone();
        third.body["loop_path"][0]["index"] = json!(1);
        let now = Instant::now();
        let interval = Duration::from_secs(60);
        let mut state = State::default();
        for wire in [&first, &second, &third] {
            state.observe(wire, now, interval);
        }
        first.event_name = "mf.node.finished".into();
        state.observe(&first, now, interval);
        let due = state.due(now + interval, interval);
        assert_eq!(due.len(), 2);
        let mut bytes = Vec::new();
        for node in due {
            write_running(&mut bytes, &node, now + interval, 1).unwrap();
        }
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.contains("invocation 2 (message)"));
        assert!(output.contains(r##"loop: "outer"#2"##));
        state.finish_run(
            attribute(&first, "mf.workflow.id"),
            attribute(&first, "mf.run.id"),
        );
        assert!(state.due(now + interval * 2, interval).is_empty());
        let observer = super::super::Observer::stderr(1);
        let shared = Arc::clone(&observer.0.diagnostics.as_ref().unwrap().shared);
        for finish_without_terminal_record in [false, true] {
            let mut run = observer
                .start(
                    crate::identity::WorkflowId::try_from(
                        attribute(&first, "mf.workflow.id").to_owned(),
                    )
                    .unwrap(),
                    crate::identity::RunId::new(),
                    vec![crate::event::NodeIdentity {
                        id: "slow".into(),
                        kind: "fixture.source".into(),
                        path: Vec::new(),
                    }],
                )
                .unwrap();
            let mut node = run.begin_node("slow").unwrap();
            run.node_started(&mut node);
            let pending = shared
                .state
                .lock()
                .unwrap()
                .due(Instant::now() + interval, interval);
            assert_eq!(pending.len(), 1);
            if finish_without_terminal_record {
                *run.sequence.lock().unwrap() =
                    crate::event::EventSequence::with_maximum(crate::Count::ZERO);
                run.finish(None);
            } else {
                drop(run);
            }
            let pending = shared
                .state
                .lock()
                .unwrap()
                .due(Instant::now() + interval * 3, interval);
            assert!(pending.is_empty());
        }
    }

    #[test]
    fn verbosity_adds_readable_metadata_and_round_trips_full_records() {
        let mut wire = fixture(include_str!("../tests/fixtures/node-succeeded.json"));
        wire.attributes["mf.node.id"] = json!("read\nnode");
        wire.trace_context = Some(crate::wire::TraceContext {
            trace_id: "1".repeat(32),
            span_id: "2".repeat(16),
            trace_flags: 1,
        });
        let mut bytes = Vec::new();
        write_diagnostic(&mut bytes, &wire, 2).unwrap();
        let detailed = String::from_utf8(bytes).unwrap();
        assert_eq!(detailed.lines().count(), 1);
        assert!(detailed.contains("node \"read\\nnode\" succeeded"));
        assert!(detailed.contains("event: mf.node.finished #3"));
        assert!(detailed.contains("position: 0"));
        assert!(detailed.contains("kind: fixture.source"));
        assert!(detailed.contains(attribute(&wire, "mf.workflow.id")));
        let mut bytes = Vec::new();
        write_diagnostic(&mut bytes, &wire, 3).unwrap();
        let full = String::from_utf8(bytes).unwrap();
        assert_eq!(full.lines().count(), 1);
        let decoded: WireRecord =
            serde_json::from_str(full.trim().strip_prefix("[verbose] ").unwrap()).unwrap();
        assert_eq!(decoded, wire);
        let now = Instant::now();
        let node = RunningNode {
            wire: fixture(include_str!("../tests/fixtures/node-started.json")),
            started: now,
            next_reminder: now + Duration::from_secs(60),
        };
        let mut bytes = Vec::new();
        write_running(&mut bytes, &node, now + Duration::from_secs(60), 3).unwrap();
        let reminder = String::from_utf8(bytes).unwrap();
        assert!(reminder.contains("still running for 60.000s"));
        assert!(reminder.contains("start event: mf.node.started #2"));
    }

    #[test]
    fn diagnostics_are_compact_and_keep_control_characters_on_one_line() {
        let mut succeeded = fixture(include_str!("../tests/fixtures/node-succeeded.json"));
        succeeded.body["elapsed_ns"] = json!(2_000_000);
        succeeded.body["duration_ns"] = json!(128_750);
        let mut failed = fixture(include_str!("../tests/fixtures/preparation-failed.json"));
        failed.attributes["mf.node.id"] = json!("read\nnode");
        failed.body["failure"]["message"] = json!("missing\nkey");
        let mut bytes = Vec::new();
        write_diagnostic(&mut bytes, &succeeded, 1).unwrap();
        write_diagnostic(&mut bytes, &failed, 1).unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.contains(
            "+2.000ms node \"load.order\" succeeded in 0.129ms | outputs: \"value.part\""
        ));
        assert!(
            output.contains(r#"node "read\nnode" failed | error (preparation): "missing\nkey""#)
        );
        assert_eq!(output.lines().count(), 2);
        assert!(!output.contains("mf.schema.version"));
        assert!(!output.contains("sha256:"));
        assert!(!output.contains("12345678-1234"));
    }

    #[test]
    fn diagnostics_preserve_loop_paths_and_unsigned_stream_identifiers() {
        let mut wire = fixture(include_str!("../tests/fixtures/node-succeeded.json"));
        wire.body["loop_path"] =
            json!([{"loop_id":"outer", "index":1}, {"loop_id":"inner", "index":0}]);
        wire.body["stream"] = json!({"invocation":u64::MAX.to_string(), "trigger":"message", "message":{"domain":2,"sequence":"9"}, "parent":"1"});
        wire.body["emission_count"] = json!(3);
        let mut bytes = Vec::new();
        write_diagnostic(&mut bytes, &wire, 1).unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert!(output.contains(r##"loop: "outer"#2/"inner"#1"##));
        assert!(
            output.contains("invocation 18446744073709551615 (message), message 2:9, parent 1")
        );
        assert!(output.contains("emitted 3"));
        assert_eq!(output.lines().count(), 1);
    }
}
