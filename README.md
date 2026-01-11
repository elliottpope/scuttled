# scuttled

An async IMAP server implementation in Rust using `tokio`.

## Features

- **Async/Await**: Built on `tokio` for efficient async I/O
- **Channel-Based Architecture**: Single-writer pattern using channels eliminates lock contention
- **Path-Based Storage**: Clean separation between metadata (Index) and content (MailStore)
- **Modular Design**: Clean trait-based abstractions for all major components
- **Graceful Shutdown**: All components support clean shutdown

## Architecture

### Core Principles

**Channel-Based Writer Loops**: All mutable state is managed through single-writer loops that receive commands via channels. This eliminates lock contention and guarantees write ordering.

**Separation of Concerns**:
- **Index** tracks all metadata and returns file paths
- **MailStore** is a simple key-value store (path -> content)
- No shared state between components

### Library Abstractions

The library provides trait abstractions for the following components:

- **MailStore**: Path-based interface for storing and retrieving email content
- **Index**: Manages message metadata, mailboxes, and search; returns paths for MailStore
- **Authenticator**: Interface for authenticating IMAP connections
- **UserStore**: Interface for managing user accounts and credentials
- **Queue**: Interface for queueing asynchronous tasks

### Concrete Implementations

The library includes production-ready implementations:

- **FilesystemMailStore**: Stores emails as files using channel-based writer loop
- **InMemoryIndex**: In-memory metadata tracking with full-text search
- **BasicAuthenticator**: Password-based authentication using UserStore
- **SQLiteUserStore**: User management with SQLite and bcrypt password hashing
- **ChannelQueue**: FIFO task queue with graceful shutdown

## Usage

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
scuttled = "0.1.0"
```

Use the default implementations:

```rust
use scuttled::{MailStore, Index, Authenticator, UserStore, Queue};
use scuttled::server::ImapServer;
use scuttled::authenticator::r#impl::BasicAuthenticator;
use scuttled::index::r#impl::InMemoryIndex;
use scuttled::mailstore::r#impl::FilesystemMailStore;
use scuttled::queue::r#impl::ChannelQueue;
use scuttled::userstore::r#impl::SQLiteUserStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mail_store = FilesystemMailStore::new("./data/mail").await?;
    let index = InMemoryIndex::new();
    let user_store1 = SQLiteUserStore::new("./data/users.db").await?;
    let user_store2 = SQLiteUserStore::new("./data/users.db").await?;
    let queue = ChannelQueue::new();

    // Create a default user and mailbox
    user_store1.create_user("test", "test").await?;
    index.create_mailbox("test", "INBOX").await?;

    let authenticator = BasicAuthenticator::new(user_store1);
    let server = ImapServer::new(mail_store, index, authenticator, user_store2, queue);

    server.listen("127.0.0.1:1143").await?;
    Ok(())
}
```

### As a Binary

Build and run the server:

```bash
cargo build --release
./target/release/scuttled
```

The server will:
- Listen on `127.0.0.1:1143`
- Create a default user `test:test`
- Store mail in `./data/mail/`
- Store user database in `./data/users.db`

Connect with any IMAP client:

```bash
telnet localhost 1143
```

## Protocol Support

Currently implements core IMAP4rev1 commands:

- **Connection Management**: CAPABILITY, NOOP, LOGOUT
- **Authentication**: LOGIN
- **Mailbox Management**: SELECT, EXAMINE, CREATE, DELETE, LIST, CLOSE
- **Message Management**: EXPUNGE (basic)

## Testing

Run the comprehensive test suite:

```bash
cargo test
```

Test coverage includes:
- **Unit tests**: 32 tests for each component
- **Integration tests**: Using external async-imap client for protocol compliance
- **Component tests**: Path-based operations, metadata tracking, authentication

All components have dedicated test suites that verify:
- Basic functionality
- Channel-based operations
- Graceful shutdown
- Concurrent access patterns

## Architecture Benefits

The channel-based architecture provides:

- **Better Performance**: No lock contention, guaranteed write ordering
- **Cleaner Separation**: Index handles metadata, MailStore handles content
- **Easier Testing**: Each component is independently testable
- **Graceful Shutdown**: All components can shut down cleanly
- **Modularity**: Easy to swap implementations

## Development

### Project Structure

```
scuttled/
├── src/
│   ├── lib.rs                    # Library entry point
│   ├── types.rs                  # Core types and data structures
│   ├── error.rs                  # Error types
│   ├── protocol.rs               # IMAP protocol parsing
│   ├── server.rs                 # IMAP server implementation
│   ├── mailstore/
│   │   ├── mod.rs                # MailStore trait
│   │   └── impl/
│   │       └── filesystem.rs     # Filesystem implementation
│   ├── index/
│   │   ├── mod.rs                # Index trait
│   │   └── impl/
│   │       └── inmemory.rs       # In-memory implementation
│   ├── queue/
│   │   ├── mod.rs                # Queue trait
│   │   └── impl/
│   │       └── channel.rs        # Channel-based implementation
│   ├── authenticator/
│   │   ├── mod.rs                # Authenticator trait
│   │   └── impl/
│   │       └── basic.rs          # Basic implementation
│   ├── userstore/
│   │   ├── mod.rs                # UserStore trait
│   │   └── impl/
│   │       └── sqlite.rs         # SQLite implementation
│   └── bin/
│       └── main.rs               # Binary entry point
└── tests/
    └── integration_tests.rs      # Integration tests with async-imap
```

### Building

```bash
cargo build
```

### Running

```bash
cargo run
```

Or with logging:

```bash
RUST_LOG=info cargo run
```

## Implementation Details

### Channel-Based Writer Loops

Each component with mutable state uses a channel-based writer loop:

```rust
enum Command {
    DoSomething(Data, oneshot::Sender<Result<Response>>),
    Shutdown(oneshot::Sender<()>),
}

async fn writer_loop(rx: Receiver<Command>) {
    let mut state = State::new();
    while let Ok(cmd) = rx.recv().await {
        match cmd {
            Command::DoSomething(data, reply) => {
                let result = state.process(data);
                let _ = reply.send(result);
            }
            Command::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}
```

This pattern ensures:
- Only one writer at a time (no locks needed)
- Guaranteed operation ordering
- Clean shutdown handling

### Path-Based Storage

The Index manages all metadata and returns paths to the MailStore:

```rust
// Index stores metadata and returns paths
let path = index.add_message("alice", "INBOX", message).await?;

// MailStore just stores/retrieves content at paths
mail_store.store(&path, content).await?;
let content = mail_store.retrieve(&path).await?;
```

Path format: `username/mailbox/message_id.eml`

## License

MIT OR Apache-2.0

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
