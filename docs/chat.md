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
