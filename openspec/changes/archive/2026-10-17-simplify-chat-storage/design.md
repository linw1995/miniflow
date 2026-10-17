## Decisions

Use one reply representation: optional usages. A reported call with unknown counters still has a list entry; absent lists remain unknown and explicit empty lists report no calls. Keep legacy token columns for historical reads, but stop updating them on new replies. Migration converts only successful, identified single-model records with reported counters and no existing list, using the durable call ID as their stable legacy identity.

Remove the derived conversation counter from Session and SQLite. Development rollback reconstructs it from successful turns. Reply completion updates messages, call results, turn state, and session timestamps only.

Expose explicit title scheduling through the existing application-node pattern. In one short transaction, select
provisional sessions with successful turns and no prior title call, then queue their first successful turn. Reuse the
existing Changed result instead of adding a new scheduling result type or persistent flags. The host invokes scheduling
before title dispatch; automatic startup, polling, and post-completion dispatch close the commit-to-scheduling gap.
Failed or interrupted title attempts require explicit retry.

Retain title revisions, ownership locking, transactional result persistence, and the existing usage view. Do not introduce a new job framework, database abstraction, connection pool, or implicit usage collector.
