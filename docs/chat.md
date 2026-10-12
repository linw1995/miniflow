# Embedded chat core

The `mf-chat` library owns SQLite storage for multiple chat sessions. Its persisted operations are application-owned `mf_chat.*` nodes. It does not initialize a terminal or launch workflow runner processes.

## Database initialization

Open an explicit database path:

```rust
let database = mf_chat::ChatDatabase::open("state/chat.sqlite")?;
println!("{}", database.path().display());
```

The owner creates the parent directory, acquires an exclusive sibling lock, executes the initialization node, and keeps the lock until dropped. Another owning host receives a typed busy error. Startup applies embedded pending migrations and enables WAL, foreign keys, and a finite busy timeout. No external migration files or tools are needed.

Node preparation validates contracts without opening storage. Failures retain typed filesystem, connection, query, and migration sources through `ChatError`.

Persisted records distinguish sessions, ordered messages, conversation turns, and model calls. Title generation can therefore record model usage independently of chat messages. Schema rollback is an explicit development operation on disposable databases; startup only applies pending forward migrations.

## Sessions and history

Every host operation runs an application node through the existing workflow runtime:

```rust
let session = database.create_session("default", Some("Answer briefly".into()), None)?;
database.rename_session(&session.id, "Project questions")?;
database.archive_session(&session.id)?;
database.restore_session(&session.id)?;
let page = database.list_sessions(false, 50, None)?;
let messages = database.messages(&session.id, 50, None)?;
let context = database.history(&session.id, mf_chat::DEFAULT_HISTORY_BYTES)?;
```

Lists use bounded keyset pages. Pass `next_after` back as the next cursor. Session order is creation time followed by stable ID; message order uses a per-session sequence. Message results include each turn's state, so unsuccessful submissions remain inspectable. Archiving retains messages and call records and prevents new turns until restoration.

History includes only completed successful turns from the requested session. It keeps system instructions separate and excludes unsuccessful submissions and the current prompt. The default canonical history JSON bound is 1 MiB; exceeding it fails explicitly without deleting or trimming messages.

Registered kinds are `mf_chat.session.create`, `.list`, `.load`, `.rename`, `.archive`, `.restore`, `mf_chat.messages.list`, and `mf_chat.history.load`. All node inputs name an explicit database path; node preparation never reads storage.
