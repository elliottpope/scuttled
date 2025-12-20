# Architecture Improvements

## Overview

This document outlines the planned architectural improvements to make Scuttled more performant and better aligned with Rust async best practices.

## Key Improvements

### 1. Module Restructuring

**Current Structure:**
```
src/
├── implementations/
│   ├── filesystem_mailstore.rs
│   ├── default_index.rs
│   └── ...
└── traits.rs
```

**New Structure:**
```
src/
├── mailstore/
│   ├── mod.rs          # MailStore trait
│   └── impl/
│       └── filesystem.rs
├── index/
│   ├── mod.rs          # Index trait
│   └── impl/
│       └── inmemory.rs
├── queue/
│   ├── mod.rs          # Queue trait
│   └── impl/
│       └── channel.rs
├── authenticator/
│   ├── mod.rs          # Authenticator trait
│   └── impl/
│       └── basic.rs
└── userstore/
    ├── mod.rs          # UserStore trait
    └── impl/
        └── sqlite.rs
```

### 2. Channel-Based Writer Loops

Replace `RwLock` with single-writer patterns using channels. This provides:
- Better performance (no lock contention)
- Guaranteed write ordering
- Easier to reason about concurrency

**Pattern:**
```rust
pub struct ChannelQueue {
    tx: Sender<Command>,
}

enum Command {
    Enqueue(QueueTask, oneshot::Sender<Result<()>>),
    Dequeue(oneshot::Sender<Result<Option<QueueTask>>>),
    Shutdown,
}

async fn writer_loop(mut rx: Receiver<Command>, mut queue: VecDeque<QueueTask>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::Enqueue(task, reply) => {
                queue.push_back(task);
                let _ = reply.send(Ok(()));
            }
            Command::Dequeue(reply) => {
                let task = queue.pop_front();
                let _ = reply.send(Ok(task));
            }
            Command::Shutdown => break,
        }
    }
}
```

### 3. Path-Based MailStore

**Current:** MailStore tracks messages by ID and performs lookups

**Improved:**
- Index handles all metadata and lookups
- Index returns file paths to MailStore
- MailStore is a simple key-value store (path -> content)

This separation of concerns makes the system more modular and easier to swap implementations.

**Example:**
```rust
// Index returns path
let path = index.get_message_path(message_id).await?;

// MailStore just retrieves by path
let content = mailstore.retrieve(&path).await?;
```

### 4. Graceful Shutdown

All components with writer loops have shutdown signals:

```rust
impl Queue {
    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Shutdown(tx)).await?;
        rx.await?;
        Ok(())
    }
}
```

### 5. External IMAP Client for Integration Tests

**Current:** Tests use internal methods directly

**Improved:** Use `async-imap` or similar crate to test as a real client would:

```rust
#[async_std::test]
async fn test_login_flow() {
    let server = start_test_server().await;

    let client = async_imap::Client::connect(("127.0.0.1", server.port())).await?;
    let session = client.login("test", "test").await?;

    // Real IMAP protocol testing
    let mailboxes = session.list(None, Some("*")).await?;
    assert_eq!(mailboxes.len(), 1);
}
```

## Migration Plan

1. ✅ Create new module structure
2. ⏳ Implement channel-based Queue
3. ⏳ Implement path-based MailStore with filesystem backend
4. ⏳ Implement channel-based Index (rename to InMemoryIndex)
5. ⏳ Update UserStore and Authenticator to new structure
6. ⏳ Update server.rs to work with new architecture
7. ⏳ Add external IMAP client to integration tests
8. ⏳ Update all unit tests
9. ⏳ Update documentation

## Benefits

- **Performance**: No lock contention, single-writer guarantees ordering
- **Testability**: External client tests ensure protocol compliance
- **Modularity**: Clear separation of concerns
- **Maintainability**: Each component is self-contained
- **Correctness**: Guaranteed write ordering prevents race conditions
