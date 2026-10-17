use crate::{
    error::*,
    model::*,
    protocol::{Session, check},
};
use mf_runtime::StreamCancellation;
use serde_json::{Value, json};
use snafu::{ResultExt, ensure};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
    time::{Duration, Instant},
};

pub fn validate_config(config: &ServerConfig) -> Result<(), Error> {
    ensure!(
        !config.command.is_empty(),
        InvalidSnafu {
            message: "server command is empty"
        }
    );
    ensure!(
        (1..=600_000).contains(&config.deadline_ms),
        InvalidSnafu {
            message: "deadline_ms must be between 1 and 600000"
        }
    );
    ensure!(
        (4..=100_000).contains(&config.max_requests),
        InvalidSnafu {
            message: "max_requests must be between 4 and 100000"
        }
    );
    ensure!(
        (1024..=64 * 1024 * 1024).contains(&config.max_bytes),
        InvalidSnafu {
            message: "max_bytes must be between 1024 and 67108864"
        }
    );
    Ok(())
}

pub fn analyze(
    config: &ServerConfig,
    request: AnalysisRequest,
    cancellation: StreamCancellation,
) -> Result<AnalysisOutcome, Error> {
    validate_config(config)?;
    ensure!(
        serde_json::to_vec(&request).context(JsonSnafu)?.len() <= config.max_bytes,
        InvalidSnafu {
            message: "analysis request exceeds its byte budget"
        }
    );
    let start = Instant::now();
    let deadline = start + Duration::from_millis(config.deadline_ms);
    let mut identities = BTreeSet::new();
    for snapshot in &request.snapshots {
        ensure!(
            !snapshot.id.is_empty() && snapshot.root.is_absolute(),
            InvalidSnafu {
                message: "snapshots need an identity and absolute root"
            }
        );
        ensure!(
            identities.insert(snapshot.id.clone()),
            InvalidSnafu {
                message: "duplicate snapshot identity"
            }
        );
        for document in &snapshot.documents {
            ensure!(
                !document.path.is_empty()
                    && Path::new(&document.path)
                        .components()
                        .all(|c| matches!(c, Component::Normal(_))),
                InvalidSnafu {
                    message: "document paths must be relative and cannot escape the snapshot"
                }
            );
            ensure!(
                document.ranges.iter().all(|r| r.start <= r.end),
                InvalidSnafu {
                    message: "reversed source range"
                }
            );
        }
    }
    check(&cancellation, deadline)?;
    let mut outcome = AnalysisOutcome {
        request: request.clone(),
        server: config.clone(),
        status: Status::NotRequested,
        evidence: Vec::new(),
        gaps: Vec::new(),
        elapsed_ms: 0,
    };
    for snapshot in &request.snapshots {
        if !snapshot
            .documents
            .iter()
            .any(|d| config.accepts(d) && !d.ranges.is_empty())
        {
            continue;
        }
        outcome.status = Status::Analyzed;
        let result = analyze_snapshot(
            config,
            snapshot,
            &request.initialization_options,
            deadline,
            cancellation.clone(),
            &mut outcome.gaps,
        );
        match result {
            Ok(evidence) => outcome.evidence.push(evidence),
            Err(error) if error.fatal() => return Err(error),
            Err(error) => {
                outcome.gaps.push(Gap {
                    snapshot: snapshot.id.clone(),
                    operation: "analysis".into(),
                    kind: if matches!(error, Error::Limit { .. }) {
                        GapKind::Limit
                    } else {
                        GapKind::AnalyzerFailure
                    },
                    message: error.to_string(),
                });
                break;
            }
        }
        if serde_json::to_vec(&outcome).context(JsonSnafu)?.len() > config.max_bytes {
            outcome.evidence.pop();
            outcome.gaps.push(Gap {
                snapshot: snapshot.id.clone(),
                operation: "analysis".into(),
                kind: GapKind::Limit,
                message: "aggregate result too large".into(),
            });
            break;
        }
    }
    if !outcome.gaps.is_empty() {
        outcome.status = Status::Incomplete;
    }
    check_cancelled(&cancellation)?;
    outcome.elapsed_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    Ok(outcome)
}

fn check_cancelled(cancellation: &StreamCancellation) -> Result<(), Error> {
    if let Some(source) = cancellation.failure() {
        return Err(source).context(CancelledSnafu);
    }
    Ok(())
}

fn analyze_snapshot(
    config: &ServerConfig,
    snapshot: &Snapshot,
    options: &Value,
    deadline: Instant,
    cancellation: StreamCancellation,
    gaps: &mut Vec<Gap>,
) -> Result<SnapshotEvidence, Error> {
    let root = snapshot.root.canonicalize().context(IoSnafu {
        action: "resolve snapshot root",
    })?;
    let mut session = Session::start(config, &root, options.clone(), deadline, cancellation)?;
    let result = query_snapshot(&mut session, config, snapshot, &root, options, gaps);
    session.finish()?;
    result
}

fn query_snapshot(
    session: &mut Session,
    config: &ServerConfig,
    snapshot: &Snapshot,
    root: &Path,
    options: &Value,
    gaps: &mut Vec<Gap>,
) -> Result<SnapshotEvidence, Error> {
    let root_uri = file_uri(root)?;
    let initialized = session.request(
        "initialize",
        json!({
            "processId": std::process::id(), "rootUri":root_uri,
            "capabilities": {
                "general":{"positionEncodings":["utf-8","utf-16","utf-32"]},
                "textDocument":{"documentSymbol":{"hierarchicalDocumentSymbolSupport":true}},
                "experimental":{"serverStatusNotification":config.profile == Profile::RustAnalyzer}
            }, "initializationOptions":options,
        }),
    )?;
    session.capabilities = initialized["capabilities"].clone();
    ensure!(
        session.capabilities.is_object(),
        ProtocolSnafu {
            message: "server capabilities must be an object"
        }
    );
    let encoding = session.capabilities["positionEncoding"]
        .as_str()
        .unwrap_or("utf-16")
        .to_owned();
    ensure!(
        matches!(encoding.as_str(), "utf-8" | "utf-16" | "utf-32"),
        ProtocolSnafu {
            message: "unsupported position encoding"
        }
    );
    session.send(json!({"method":"initialized","params":{}}))?;
    if config.profile == Profile::RustAnalyzer {
        while !session
            .status
            .as_ref()
            .is_some_and(|s| s["quiescent"] == true)
        {
            session.receive()?;
        }
        ensure!(
            session.status.as_ref().is_some_and(|s| s["health"] == "ok"),
            ProtocolSnafu {
                message: "server workspace is not healthy"
            }
        );
    }
    let mut result = SnapshotEvidence {
        snapshot: snapshot.id.clone(),
        server_info: initialized["serverInfo"].clone(),
        position_encoding: encoding.clone(),
        progress_notifications: 0,
        documents: Vec::new(),
    };
    for document in snapshot
        .documents
        .iter()
        .filter(|d| config.accepts(d) && !d.ranges.is_empty())
    {
        let path = root.join(&document.path).canonicalize().context(IoSnafu {
            action: "resolve analysis document",
        })?;
        ensure!(
            path.starts_with(root),
            InvalidSnafu {
                message: "document symlink escapes snapshot"
            }
        );
        let metadata = std::fs::metadata(&path).context(IoSnafu {
            action: "inspect analysis document",
        })?;
        ensure!(
            metadata.is_file(),
            InvalidSnafu {
                message: "analysis documents must be regular files"
            }
        );
        ensure!(
            metadata.len() <= config.max_bytes as u64,
            LimitSnafu {
                message: "analysis document exceeds its byte budget"
            }
        );
        let source = std::fs::read_to_string(&path).context(IoSnafu {
            action: "read analysis document",
        })?;
        ensure!(
            document.ranges.iter().all(|r| r.end <= source.len()
                && source.is_char_boundary(r.start)
                && source.is_char_boundary(r.end)),
            InvalidSnafu {
                message: "source range is outside the document or splits UTF-8"
            }
        );
        let uri = file_uri(&path)?;
        if !session.supports("documentSymbolProvider") {
            unsupported(gaps, snapshot, "textDocument/documentSymbol");
            continue;
        }
        let symbols = session.request(
            "textDocument/documentSymbol",
            json!({"textDocument":{"uri":uri}}),
        )?;
        let mut definitions = Vec::new();
        flatten(&symbols, &source, &encoding, &mut definitions)?;
        let mut evidence = DocumentEvidence {
            path: document.path.clone(),
            symbols: Vec::new(),
        };
        for (symbol, range) in definitions {
            if !document.ranges.iter().any(|r| intersects(r, &range)) {
                continue;
            }
            let position = symbol
                .get("selectionRange")
                .or_else(|| symbol.get("range"))
                .or_else(|| symbol.pointer("/location/range"))
                .and_then(|r| r.get("start"))
                .cloned();
            let Some(position) = position else {
                return ProtocolSnafu {
                    message: "symbol has no position",
                }
                .fail();
            };
            let params = json!({"textDocument":{"uri":uri},"position":position,"context":{"includeDeclaration":false}});
            let mut item = SymbolEvidence {
                name: symbol["name"].as_str().unwrap_or("").into(),
                kind: symbol["kind"].as_u64().unwrap_or(0),
                range,
                references: Vec::new(),
                incoming_calls: Vec::new(),
                extensions: Default::default(),
            };
            if session.supports("referencesProvider") {
                item.references =
                    array(session.request("textDocument/references", params.clone())?)?;
            } else {
                unsupported(gaps, snapshot, "textDocument/references");
            }
            if session.supports("callHierarchyProvider") {
                let prepared =
                    array(session.request("textDocument/prepareCallHierarchy", params.clone())?)?;
                for call in prepared {
                    item.incoming_calls.extend(array(
                        session.request("callHierarchy/incomingCalls", json!({"item":call}))?,
                    )?);
                }
            } else {
                unsupported(gaps, snapshot, "callHierarchy/incomingCalls");
            }
            if config.profile == Profile::RustAnalyzer {
                item.extensions.insert(
                    "rust-analyzer/relatedTests".into(),
                    session.request("rust-analyzer/relatedTests", params)?,
                );
            }
            evidence.symbols.push(item);
            ensure!(
                serde_json::to_vec(&evidence).context(JsonSnafu)?.len() <= config.max_bytes,
                LimitSnafu {
                    message: "document evidence too large"
                }
            );
        }
        result.documents.push(evidence);
        ensure!(
            serde_json::to_vec(&result).context(JsonSnafu)?.len() <= config.max_bytes,
            LimitSnafu {
                message: "snapshot evidence too large"
            }
        );
    }
    result.progress_notifications = session.progress;
    session.request("shutdown", Value::Null)?;
    session.send(json!({"method":"exit","params":null}))?;
    Ok(result)
}

fn unsupported(gaps: &mut Vec<Gap>, snapshot: &Snapshot, operation: &str) {
    if !gaps
        .iter()
        .any(|g| g.snapshot == snapshot.id && g.operation == operation)
    {
        gaps.push(Gap {
            snapshot: snapshot.id.clone(),
            operation: operation.into(),
            kind: GapKind::Unsupported,
            message: "server did not advertise this capability".into(),
        });
    }
}

fn array(value: Value) -> Result<Vec<Value>, Error> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(values) => Ok(values),
        _ => ProtocolSnafu {
            message: "expected an array result",
        }
        .fail(),
    }
}

fn file_uri(path: &Path) -> Result<String, Error> {
    match url::Url::from_file_path(path) {
        Ok(uri) => Ok(uri.to_string()),
        Err(()) => InvalidSnafu {
            message: "cannot encode absolute file URI",
        }
        .fail(),
    }
}

fn flatten(
    value: &Value,
    source: &str,
    encoding: &str,
    output: &mut Vec<(Value, SourceRange)>,
) -> Result<(), Error> {
    if value.is_null() {
        return Ok(());
    }
    let Some(values) = value.as_array() else {
        return ProtocolSnafu {
            message: "expected document symbols",
        }
        .fail();
    };
    for symbol in values {
        let range = symbol
            .get("range")
            .or_else(|| symbol.pointer("/location/range"));
        let Some(range) = range else {
            return ProtocolSnafu {
                message: "symbol has no range",
            }
            .fail();
        };
        let start = byte_offset(source, &range["start"], encoding)?;
        let end = byte_offset(source, &range["end"], encoding)?;
        ensure!(
            start <= end,
            ProtocolSnafu {
                message: "reversed symbol range"
            }
        );
        output.push((symbol.clone(), SourceRange { start, end }));
        if let Some(children) = symbol.get("children") {
            flatten(children, source, encoding, output)?;
        }
    }
    Ok(())
}

fn intersects(left: &SourceRange, right: &SourceRange) -> bool {
    if left.start == left.end {
        right.start <= left.start && left.start < right.end
    } else {
        left.start < right.end && right.start < left.end
    }
}

pub fn byte_offset(source: &str, position: &Value, encoding: &str) -> Result<usize, Error> {
    ensure!(
        matches!(encoding, "utf-8" | "utf-16" | "utf-32"),
        ProtocolSnafu {
            message: "unsupported position encoding"
        }
    );
    let (Some(line), Some(character)) = (position["line"].as_u64(), position["character"].as_u64())
    else {
        return ProtocolSnafu {
            message: "invalid LSP position",
        }
        .fail();
    };
    let mut start = 0;
    for _ in 0..line {
        let Some(newline) = source[start..].find('\n') else {
            return ProtocolSnafu {
                message: "LSP line out of bounds",
            }
            .fail();
        };
        start += newline + 1;
    }
    let text = source[start..]
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');
    let mut units = 0u64;
    for (offset, ch) in text.char_indices() {
        if units == character {
            return Ok(start + offset);
        }
        units += match encoding {
            "utf-8" => ch.len_utf8() as u64,
            "utf-16" => ch.len_utf16() as u64,
            _ => 1,
        };
    }
    ensure!(
        units == character,
        ProtocolSnafu {
            message: "LSP column out of bounds or splits a character"
        }
    );
    Ok(start + text.len())
}
