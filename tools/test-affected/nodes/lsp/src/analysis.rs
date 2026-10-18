use crate::{
    error::*,
    model::*,
    protocol::{Session, check},
};
use lsp_types::{
    self as lsp,
    notification::{Exit, Initialized},
    request::{
        CallHierarchyIncomingCalls, CallHierarchyPrepare, DocumentSymbolRequest, Initialize,
        References, Request, Shutdown,
    },
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
    // Keep rootUri for servers that predate workspace-folder support.
    #[allow(deprecated)]
    let initialized = session.request::<Initialize>(lsp::InitializeParams {
        process_id: Some(std::process::id()),
        root_uri: Some(root_uri),
        capabilities: lsp::ClientCapabilities {
            general: Some(lsp::GeneralClientCapabilities {
                position_encodings: Some(vec![
                    lsp::PositionEncodingKind::UTF8,
                    lsp::PositionEncodingKind::UTF16,
                    lsp::PositionEncodingKind::UTF32,
                ]),
                ..Default::default()
            }),
            text_document: Some(lsp::TextDocumentClientCapabilities {
                document_symbol: Some(lsp::DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            experimental: Some(
                json!({"serverStatusNotification": config.profile == Profile::RustAnalyzer}),
            ),
            ..Default::default()
        },
        initialization_options: Some(options.clone()),
        ..Default::default()
    })?;
    let capabilities = initialized.capabilities;
    let encoding = capabilities
        .position_encoding
        .as_ref()
        .map_or("utf-16", |value| value.as_str())
        .to_owned();
    ensure!(
        matches!(encoding.as_str(), "utf-8" | "utf-16" | "utf-32"),
        ProtocolSnafu {
            message: "unsupported position encoding"
        }
    );
    session.notify::<Initialized>(lsp::InitializedParams {})?;
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
        server_info: serde_json::to_value(initialized.server_info).context(JsonSnafu)?,
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
        if !matches!(
            capabilities.document_symbol_provider,
            Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_))
        ) {
            unsupported(gaps, snapshot, DocumentSymbolRequest::METHOD);
            continue;
        }
        let symbols = session.request::<DocumentSymbolRequest>(lsp::DocumentSymbolParams {
            text_document: lsp::TextDocumentIdentifier::new(uri.clone()),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })?;
        let mut definitions = Vec::new();
        flatten(symbols, &source, &encoding, &mut definitions)?;
        let mut evidence = DocumentEvidence {
            path: document.path.clone(),
            symbols: Vec::new(),
        };
        for (mut item, position) in definitions {
            if !document.ranges.iter().any(|r| intersects(r, &item.range)) {
                continue;
            }
            let text_document_position = lsp::TextDocumentPositionParams {
                text_document: lsp::TextDocumentIdentifier::new(uri.clone()),
                position,
            };
            if matches!(
                capabilities.references_provider,
                Some(lsp::OneOf::Left(true) | lsp::OneOf::Right(_))
            ) {
                item.references = values(session.request::<References>(lsp::ReferenceParams {
                    text_document_position: text_document_position.clone(),
                    context: lsp::ReferenceContext {
                        include_declaration: false,
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                })?)?;
            } else {
                unsupported(gaps, snapshot, References::METHOD);
            }
            if matches!(
                capabilities.call_hierarchy_provider,
                Some(
                    lsp::CallHierarchyServerCapability::Simple(true)
                        | lsp::CallHierarchyServerCapability::Options(_)
                )
            ) {
                let prepared =
                    session.request::<CallHierarchyPrepare>(lsp::CallHierarchyPrepareParams {
                        text_document_position_params: text_document_position,
                        work_done_progress_params: Default::default(),
                    })?;
                for call in prepared.into_iter().flatten() {
                    item.incoming_calls.extend(values(
                        session.request::<CallHierarchyIncomingCalls>(
                            lsp::CallHierarchyIncomingCallsParams {
                                item: call,
                                work_done_progress_params: Default::default(),
                                partial_result_params: Default::default(),
                            },
                        )?,
                    )?);
                }
            } else {
                unsupported(gaps, snapshot, CallHierarchyIncomingCalls::METHOD);
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
    session.request::<Shutdown>(())?;
    session.notify::<Exit>(())?;
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

fn values<T: serde::Serialize>(items: Option<Vec<T>>) -> Result<Vec<Value>, Error> {
    items
        .into_iter()
        .flatten()
        .map(|item| serde_json::to_value(item).context(JsonSnafu))
        .collect()
}

fn file_uri(path: &Path) -> Result<lsp::Uri, Error> {
    match url::Url::from_file_path(path) {
        Ok(uri) => serde_json::from_value(Value::String(uri.into())).context(JsonSnafu),
        Err(()) => InvalidSnafu {
            message: "cannot encode absolute file URI",
        }
        .fail(),
    }
}

fn flatten(
    symbols: Option<lsp::DocumentSymbolResponse>,
    source: &str,
    encoding: &str,
    output: &mut Vec<(SymbolEvidence, lsp::Position)>,
) -> Result<(), Error> {
    let mut definitions = Vec::new();
    match symbols {
        None => return Ok(()),
        Some(lsp::DocumentSymbolResponse::Flat(symbols)) => {
            definitions.extend(symbols.into_iter().map(|s| {
                (
                    s.name,
                    s.kind,
                    s.location.range,
                    s.location.range.start,
                    None,
                )
            }));
        }
        Some(lsp::DocumentSymbolResponse::Nested(symbols)) => {
            for symbol in symbols {
                definitions.push((
                    symbol.name,
                    symbol.kind,
                    symbol.range,
                    symbol.selection_range.start,
                    symbol.children,
                ));
            }
        }
    }
    for (name, kind, range, position, children) in definitions {
        let start = position_offset(source, range.start, encoding)?;
        let end = position_offset(source, range.end, encoding)?;
        ensure!(
            start <= end,
            ProtocolSnafu {
                message: "reversed symbol range"
            }
        );
        output.push((
            SymbolEvidence {
                name,
                kind: serde_json::from_value(serde_json::to_value(kind).context(JsonSnafu)?)
                    .context(JsonSnafu)?,
                range: SourceRange { start, end },
                references: Vec::new(),
                incoming_calls: Vec::new(),
            },
            position,
        ));
        flatten(
            children.map(lsp::DocumentSymbolResponse::Nested),
            source,
            encoding,
            output,
        )?;
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
    let position = serde_json::from_value(position.clone()).context(JsonSnafu)?;
    position_offset(source, position, encoding)
}

fn position_offset(source: &str, position: lsp::Position, encoding: &str) -> Result<usize, Error> {
    let lsp::Position { line, character } = position;
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
    let mut units = 0u32;
    for (offset, ch) in text.char_indices() {
        if units == character {
            return Ok(start + offset);
        }
        units += match encoding {
            "utf-8" => ch.len_utf8() as u32,
            "utf-16" => ch.len_utf16() as u32,
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
