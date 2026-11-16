## 1. Shared messages and chatflow contract

Admission, result decoding, execution, and durable storage now use the transcript protocol together.
Strict native/JSON encoding, independent view validation, invocation-wide candidate invalidation, and
manifest-free save-only recovery share the same canonical result; no legacy-result fallback is added.

- [x] Extract provider-independent complete/pending-prefix transcript validation in `mfn-llm` with typed indexed errors.
- [x] Add strict JSON persistence support for the existing `ChatMessage` and `ToolCall` codecs; test runtime/JSON parity.
- [x] Bind admitted model context plus current user through `messages`; expose context revision through optional application context.
- [x] Normalize self-contained append/replace_context result objects into internal facts and MessageUpdate variants, without a Flow update/kind wrapper.
- [x] Keep status/usages/error/title inside each result; support append-only, replace-only, or two optional branch outputs with at least one advertised.
- [x] Reject zero/both/repeated/undeclared alternatives, null/nonobject payloads, missing facts/lists, and unknown payload fields; preserve empty lists.
- [x] Use the branch-output codec for canonical JSON, nested application results, typed custom backends, and runner output projection.
- [x] Centralize display-delta and next-context matching; append only in Append, replace exactly in ReplaceContext, including empty snapshots.
- [x] Update admission and bounded decoding with existing Flow syntax; record advertised alternatives for manifest-free recovery when contract-backed.
- [x] Check compatible result-object metadata where available; validate facts/message fields at capture because manifests lack payload schemas.

## 2. Durable transcripts and recovery

- [x] Add a forward migration for canonical audit payloads, terminal results, and revisioned model history, preserving historical data and accounting.
- [x] Seed effective history with system instructions and successful legacy exchanges exactly once; do not reinsert edited/deleted system messages.
- [x] Store admitted base revisions and explicit append/replace context operations; replacement checkpoints are complete and receive no audit suffix.
- [x] Remove fixed two-message reservation and per-turn/role uniqueness; append messages with checked sequence allocation.
- [x] Settle display messages/facts/title/state atomically with the variant-derived candidate context only on success.
- [x] Compare-and-swap the admitted context revision, reject stale publication without partial writes, and keep title revision independent.
- [x] Compare complete canonical updates, including variant and both replacement lists; check replay before CAS and never republish old context.
- [x] Generalize save-only retention/retry to either result status; retain complete failed transcripts when persistence fails.
- [x] Expose `model_context()` separately from bounded transcript `messages()` queries; failures publish no context revision.
- [x] Recover from the admitted revision rather than reconstructing inputs from current history or all successful audit messages.

## 3. Flows and execution backends

- [x] Keep built-in conversation orchestration separate from user answer/tool/compaction policy, shared persistence handlers, host resource management, and abort cleanup.
- [x] Make `mfn-llm.agent` consume messages, append terminal assistants, and export only generated assistant/tool additions.
- [x] Retain completed rounds and usages on captured failures and iteration limits, preserving ordered parallel tool results.
- [x] Export a single incremental result as append; assemble Agent status/messages/usages with an ordinary object only when needed, not a branch merge.
- [ ] Add a replacement example returning full continuation context and original display additions, including compaction of this turn's tool output.
- [x] Update application completion and registered runners to handle produced/skipped alternative ports without skipping settlement on an inactive branch.
- [x] Keep one complete terminal record and validate the whole invocation before settlement; later conflicting results must invalidate retained candidates.
- [x] Keep titles optional and separate; use host policy/resolved inputs, suppress returned title attempts, derive from display data, and remove root-title ownership assumptions.
- [x] Update executable runner registration, JSON/JSONL result capture, and manifest-free recovery; retain answer-stream delivery rules.
- [x] Migrate all chat examples, generated host fixtures, and backend tests; recompile affected executable fixtures.

## 4. Terminal presentation

- [x] Render multiple assistant messages, tool-call batches, and tool results with distinct message-scoped keys.
- [x] Match both variants for committed/unsaved display deltas only; never render replacement context as conversation history.
- [x] Preserve original audit presentation after context rewrites and distinguish pending, failed, and committed history update state.
- [x] Keep tool data inert and support pages that begin or end inside a tool exchange without mislabelling records.
- [x] Display usages/performance once per turn and verify no duplicate accounting across message pages or reloads.

## 5. Acceptance and documentation

- [ ] Cover invalid roles/fields, empty-content assistant calls, parallel result matching, duplicate/orphan IDs, and unresolved suffix rules.
- [ ] Cover append-only, replace-only, and direct two-branch output selection with no convergence node or new Flow syntax.
- [ ] Verify append and replace-context paths reach subsequent provider requests in embedded and compiled backends after reopening.
- [ ] Cover absent/both/undeclared alternatives, null versus empty payloads, skipped optional ports, duplicate terminal records, and recovery invalidation.
- [ ] Cover malformed replacement objects, empty snapshots, snapshots omitting current user/final answer, and no automatic snapshot append.
- [ ] Validate display and snapshot tool exchanges independently; cover repeated calls across both views without false duplicate-ID errors.
- [ ] Cover content/metadata edits, summaries, reordering, coherent call-ID changes, per-view collisions, and orphan rejection.
- [ ] Cover stale revisions, atomic rollback, failed/cancelled rewrite isolation, title-only renames, and old-result replay after later revisions.
- [ ] Cover transactional rollback, exact success/failure replay, save-only retries with no tool execution, reopen, and legacy migration.
- [ ] Cover cancellation, no terminal result, optional text previews, receiver failure, pagination, and retained full-transcript rendering.
- [ ] Document alternative outputs, internal enum normalization, no required author-side merge, UI/context separation, incompatible updates, and tool retry risks.
- [ ] Validate OpenSpec deltas; run `nix develop --command prek install` and `nix develop --command prek -a`.
- [ ] Compile the shared affected-test Flow, plan with worktree/test precision, and execute the selected tests without widening an empty selection.
