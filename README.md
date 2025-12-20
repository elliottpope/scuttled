# scuttled

An async IMAP server implementation in Rust using `async_std`.

## Features

- **Async/Await**: Built on `async_std` for efficient async I/O
- **Modular Design**: Clean trait-based abstractions for all major components
- **Full-Featured**: Includes implementations for mail storage, indexing, authentication, and task queuing

## Architecture

### Library Abstractions

The library provides trait abstractions for the following components:

- **MailStore**: Interface for storing and retrieving raw email messages
- **Index**: Interface for indexing and searching email contents
- **Authenticator**: Interface for authenticating IMAP connections
- **UserStore**: Interface for managing user accounts and credentials
- **Queue**: Interface for queueing asynchronous tasks

### Concrete Implementations

The binary includes production-ready implementations:

- **FilesystemMailStore**: Stores emails as files on the filesystem
- **DefaultIndex**: Full-text search using Tantivy
- **BasicAuthenticator**: Password-based authentication
- **SQLiteUserStore**: User management with SQLite and bcrypt password hashing
- **InMemoryQueue**: FIFO task queue for background operations

## Usage

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
scuttled = "0.1.0"
```

Implement your own storage backends:

```rust
use scuttled::{MailStore, Index, Authenticator, UserStore, Queue};
use scuttled::server::ImapServer;

// Use the default implementations
use scuttled::implementations::*;

#[async_std::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mail_store = FilesystemMailStore::new("./mail").await?;
    let index = DefaultIndex::new("./index")?;
    let user_store1 = SQLiteUserStore::new("./users.db").await?;
    let user_store2 = SQLiteUserStore::new("./users.db").await?;
    let authenticator = BasicAuthenticator::new(user_store1);
    let queue = InMemoryQueue::new();

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
- Store data in `./data/`

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
- Unit tests for each component
- Integration tests for full workflows
- Concurrent operation tests

## Development

### Project Structure

```
scuttled/
├── src/
│   ├── lib.rs              # Library entry point
│   ├── types.rs            # Core types and data structures
│   ├── traits.rs           # Trait abstractions
│   ├── error.rs            # Error types
│   ├── protocol.rs         # IMAP protocol parsing
│   ├── server.rs           # IMAP server implementation
│   ├── implementations/    # Concrete implementations
│   │   ├── filesystem_mailstore.rs
│   │   ├── default_index.rs
│   │   ├── basic_authenticator.rs
│   │   ├── sqlite_userstore.rs
│   │   └── inmemory_queue.rs
│   └── bin/
│       └── main.rs         # Binary entry point
└── tests/
    └── integration_tests.rs
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

## License

MIT OR Apache-2.0

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
