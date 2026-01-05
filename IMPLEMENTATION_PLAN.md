# Scuttled Architecture Implementation Plan

## Executive Summary

This plan outlines the refactoring of Scuttled from a trait-object-based architecture (Arc<dyn Trait>) to a concrete-type architecture (Copy/Clone types) with enhanced event-driven communication.

**Current State:** Well-architected async IMAP server with trait-based abstractions and channel-driven state management.

**Target State:** Event-driven architecture with Copy/Clone shared components, eliminating Arc/RwLock overhead and improving performance.

---

## Architecture Comparison: Current vs. Proposed

| Component | Current | Proposed | Gap Analysis |
|-----------|---------|----------|--------------|
| **Storage** | MailStore trait + FilesystemMailStore | Storage: path/key lookup, flag updates via rename/metadata | Need flag update support, rename from MailStore |
| **StorageWatcher** | FilesystemWatcher (coupled to Index) | StorageWatcher: emit events on write/update/delete | Decouple from Index, make purely event-driven |
| **Indexer** | Index trait + Indexer impl | Full text search, reconstructible from Storage | Ensure full reconstruction capability |
| **Searcher** | ❌ Does not exist | Read-only Indexer | **NEW: Create read-only search interface** |
| **Mailboxes** | Mailboxes trait + InMemoryMailboxes | Registry of mailboxes | Exists, convert to Copy/Clone |
| **Mailbox** | Mailbox struct in types.rs | Shared state (UID→path, validity, subscriptions) | **NEW: Make Copy/Clone shared component** |
| **Events** | EventBus with pub-sub | Core event bus with sync/async subscriptions | Add oneshot channel support for sync mutations |
| **CommandHandlers** | CommandHandler trait only | CommandHandlers registry + CommandHandler | **NEW: Create registry/dispatcher** |
| **Session** | Session struct with direct dispatch | Hands off TcpStream to handlers | Refactor for clean handler handoff |
| **Connection** | Connection enum (Plain/TLS) | Plain or TLS TCP stream | ✅ Already exists |
| **Server** | ImapServer struct | Starts loops, listeners, signals | Enhance signal handling |
| **UserStore** | UserStore trait + SQLiteUserStore | Add ACLs, shared mailbox permissions | Add ACL/permissions support |
| **Queue** | Queue trait + ChannelQueue | Async work queue | ✅ Already exists |
| **Authenticator** | Authenticator trait + BasicAuthenticator | Handles authentication | ✅ Already exists |

---

## Phase 1: Key Architectural Changes

### 1.1 Events Enhancement (1-2 days) ✅ **COMPLETED**

**Goal:** Support synchronous state changes with oneshot channels for consistency guarantees.

**Tasks:**
- [x] Add oneshot channel support to EventBus for synchronous event handling
- [x] Implement async vs. sync subscription registration
- [x] Add event acknowledgment mechanism (subscribers send completion signals)
- [x] Add `publish_sync()` method that waits for all sync subscribers
- [x] Update EventBus documentation with sync/async patterns

**Files:**
- `src/events.rs` - Add sync event support ✅
- Add tests for sync event handling ✅

**Success Criteria:**
- ✅ Publishers can wait for all synchronous subscribers to complete
- ✅ Async subscribers don't block synchronous operations
- ✅ No deadlocks in event handling

**Completion Notes:**
- Implemented `SubscriptionType` enum (Async/Sync)
- Created `EventDelivery` wrapper with acknowledgment support
- Added `subscribe_sync()` and `publish_sync()` methods
- All 55 tests passing including 5 new sync event tests
- Comprehensive documentation added to module
- Commit: f94c5b1

---

### 1.2 Copy/Clone Architecture for Shared Components (3-4 days) ✅ **COMPLETE**

**Goal:** Replace Arc<dyn Trait> with Copy/Clone concrete types for Storage, Searcher, and Mailbox.

**Status:** All three core components (Storage, Searcher, Mailbox) now use Copy/Clone patterns with channel-based coordination.

#### 1.2.1 Storage Redesign

**Current Pattern:**
```rust
Arc<dyn MailStore> // Cloning increments refcount
```

**Target Pattern:**
```rust
#[derive(Clone)]
struct Storage {
    tx: Sender<StorageCommand>, // Cheap to clone
}
```

**Tasks:**
- [x] Create new Storage module (kept mailstore for backward compat)
- [x] Implement flag updates via filesystem rename (Maildir cur/new convention)
- [x] Make Storage struct Clone (wraps channel sender)
- [x] Add all core methods: store, retrieve, delete, exists, update_flags
- [x] Create StoreMail trait for file operations abstraction
- [x] Create FilesystemStore implementation with channel-based writes
- [x] Make Storage generic over StoreMail + MailboxFormat
- [ ] Add streaming read support (return AsyncRead instead of Vec<u8>)
- [ ] Update all references from Arc<dyn MailStore> to Storage

**Files:**
- `src/storage.rs` ✅ Generic over StoreMail + MailboxFormat
- `src/storage/store_mail.rs` ✅ Low-level file operations trait
- `src/storage/filesystem_store.rs` ✅ Channel-based implementation
- `src/mailstore/` - Kept for backward compatibility
- `src/server.rs` - Update to use concrete Storage type (pending)
- `src/session.rs` - Update references (pending)

**Completion Notes:**
- Storage is now generic: `Storage<S: StoreMail, F: MailboxFormat>`
- StoreMail trait: write, move_file, remove, write_metadata, read, exists
- FilesystemStore: Clone-able with channel-based writer loop
- Clear separation: StoreMail (file I/O) vs MailboxFormat (filename conventions)
- All 5 storage tests passing including flag updates
- Maildir flag format fully implemented (D/F/R/S/T flags)
- Atomic file operations with fsync
- Commits: 53d8d49, 964053c

#### 1.2.2 Mailbox Redesign ✅ **COMPLETE**

**Tasks:**
- [x] Create new `Mailbox` struct as Copy/Clone shared handle
- [x] Implement internal channel-based state management
- [x] Add UID→path mapping storage
- [x] Add UID validity tracking
- [x] Add subscription management for unsolicited messages
- [x] Make Mailbox wrapping a sender channel (cheap clone)

**Files:**
- `src/mailbox.rs` ✅ Shared Mailbox handle
- `src/lib.rs` ✅ Added exports for Mailbox, MailboxState, MailboxNotification

**Completion Notes:**
- Mailbox is Clone (wraps `Sender<MailboxCommand>`)
- Channel-based state loop for atomic operations
- Bidirectional UID↔path mapping (HashMap<Uid, String> and HashMap<String, Uid>)
- UID validity and next UID tracking
- Subscription management for IDLE/unsolicited notifications
- Complete API: assign_uid(), get_path(), get_uid(), remove_message(), update_path(), get_state(), subscribe(), notify()
- 9 comprehensive tests covering all functionality
- All 76 tests passing (up from 67)

#### 1.2.3 Searcher Creation ✅ **COMPLETE**

**Tasks:**
- [x] Create read-only Searcher struct (completely separate from Indexer)
- [x] Create SearchBackend trait for read-only operations
- [x] Make Searcher Clone (wraps Arc<dyn SearchBackend>)
- [x] Implement full query API: search, list_mailboxes, message_count, mailbox_exists
- [ ] Add search optimization for common queries (future)
- [ ] Create SearchBackend implementation that wraps IndexBackend (future)

**Files:**
- `src/searcher.rs` ✅ Complete read/write separation from Indexer
- `src/index/indexer.rs` - No dependency (intentional separation)

**Completion Notes:**
- **Full read/write separation achieved** - Searcher has NO dependency on Indexer
- SearchBackend trait provides read-only query interface
- Searcher wraps Arc<dyn SearchBackend> for cheap cloning
- Complete API: search(), list_mailboxes(), message_count(), mailbox_exists()
- MockSearchBackend for testing
- 3 tests passing
- Architecture enforces read-only access at compile time
- Commits: 53d8d49, 964053c

**Success Criteria:**
- Storage, Mailbox, Searcher are all Clone
- No Arc or RwLock in public APIs of these types
- Performance improvement from reduced atomic operations

---

### 1.3 StorageWatcher Decoupling (2 days) ✅ **COMPLETE**

**Goal:** Fully decouple FilesystemWatcher from Index, make it purely event-driven.

**Tasks:**
- [x] Remove direct Index dependency from FilesystemWatcher
- [x] Emit events for: file write, rename (flag update), delete
- [x] Update event types to include full metadata (path, flags, etc.)
- [x] Make Indexer subscribe to storage events
- [x] Test event flow: StorageWatcher → EventBus → Indexer

**Files:**
- `src/mailstore/watcher/filesystem.rs` ✅ Event-driven watcher (location unchanged)
- `src/events.rs` ✅ Events: MessageCreated, MessageModified, MessageDeleted
- `src/index/indexer.rs` ✅ Subscribes to storage events

**Completion Notes:**
- **This phase was already complete from prior work!**
- FilesystemWatcher has ZERO direct Index dependency
- All communication flows through EventBus exclusively
- Events include full metadata: username, mailbox, unique_id, path, flags, from, to, subject, body_preview, size, internal_date
- Indexer subscribes to MessageCreated, MessageModified, MessageDeleted, and MailboxDeleted events
- Event loop processes filesystem changes and publishes to EventBus
- Email parsing extracts headers (from, to, subject) and body preview
- Test `test_event_bus_integration` verifies full event flow
- All 76 tests passing

**Success Criteria:**
- ✅ FilesystemWatcher has zero knowledge of Index
- ✅ All communication via EventBus
- ✅ Events contain sufficient data for Index to update itself

---

### 1.4 CommandHandlers Registry (2-3 days) 🚧 **IN PROGRESS**

**Goal:** Create centralized command handler registry for extensibility.

**Progress:** Registry and 8 core handlers complete. Remaining: Session/Server integration.

**Tasks:**
- [x] Create CommandHandlers struct with HashMap<String, Arc<dyn CommandHandler>>
- [x] Implement registration API: `register()`, `get()`, `handle()`, `list_commands()`
- [x] Add authentication and mailbox selection checking
- [x] Implement built-in handlers as separate modules:
  - [x] CapabilityHandler - Returns server capabilities
  - [x] NoopHandler - Keepalive command
  - [x] LogoutHandler - Graceful disconnect
  - [x] LoginHandler - Username/password authentication
  - [x] SelectHandler - Select mailbox for access
  - [x] CreateHandler - Create new mailbox
  - [x] DeleteHandler - Delete mailbox (protects INBOX)
  - [x] ListHandler - List mailboxes with pattern matching
- [ ] Move command dispatch logic from Session to CommandHandlers
- [ ] Update Session to use CommandHandlers registry
- [ ] Update Server to initialize CommandHandlers and register handlers

**Files:**
- `src/command_handlers.rs` ✅ Registry with 5 tests
- `src/handlers/` ✅ 8 handlers implemented
  - `mod.rs` ✅ Export all handlers
  - `capability.rs` ✅ (61 lines)
  - `noop.rs` ✅ (58 lines)
  - `logout.rs` ✅ (58 lines)
  - `login.rs` ✅ (123 lines)
  - `select.rs` ✅ (100 lines)
  - `create.rs` ✅ (82 lines)
  - `delete.rs` ✅ (91 lines)
  - `list.rs` ✅ (131 lines)
- `src/session.rs` - Needs update to use CommandHandlers
- `src/server.rs` - Needs handler registration

**Completion Notes:**
- CommandHandlers registry with HashMap-based dispatch
- Authentication checking: requires_auth() enforced at registry level
- Mailbox selection checking: requires_selected_mailbox() enforced
- Each handler is self-contained with comprehensive tests
- All 91 tests passing (up from 81)
- Commits: 34715ef, 530ae71, 2302dbe

**Success Criteria:**
- ✅ CommandHandlers infrastructure complete
- ✅ Core handlers implemented and tested
- ⏳ Session delegation (remaining)
- ⏳ Server initialization (remaining)

---

## Phase 2: Broader Refactoring

### 2.1 Session Refactoring (2 days)

**Goal:** Simplify Session to focus on connection lifecycle, tags, and sequence ID mapping.

**Tasks:**
- [ ] Remove command parsing logic (delegate to CommandHandlers)
- [ ] Keep: tag tracking, idle timeout, max connection time
- [ ] Keep: SequenceId bidirectional map
- [ ] Simplify state management (use Events for state changes)
- [ ] Add clean TcpStream handoff to handlers

**Files:**
- `src/session.rs` - Simplify
- `src/session_context.rs` - May merge into session.rs

---

### 2.2 Server Enhancement (2 days)

**Goal:** Improve signal handling and startup/shutdown orchestration.

**Tasks:**
- [ ] Add graceful shutdown for SIGTERM/SIGINT
- [ ] Coordinate EventBus startup
- [ ] Add health check endpoint (optional)
- [ ] Improve TLS configuration (support multiple cert sources)
- [ ] Add metrics collection hooks (optional)

**Files:**
- `src/server.rs`
- `src/bin/main.rs` - Update initialization

---

### 2.3 UserStore ACL Enhancement (2-3 days)

**Goal:** Add support for ACLs and shared mailbox permissions.

**Tasks:**
- [ ] Design ACL schema (user → mailbox → permissions)
- [ ] Add ACL tables to SQLite schema
- [ ] Implement permission checking in UserStore
- [ ] Add methods: `grant_permission()`, `revoke_permission()`, `check_permission()`
- [ ] Add shared mailbox metadata

**Files:**
- `src/userstore/mod.rs` - Add ACL methods to trait
- `src/userstore/impl/sqlite.rs` - Implement ACL storage
- Add migration support for schema changes

---

### 2.4 Indexer Reconstruction (2 days)

**Goal:** Ensure Indexer can be fully reconstructed from Storage.

**Tasks:**
- [ ] Implement `rebuild_from_storage(storage: &Storage)` method
- [ ] Walk all files in Storage
- [ ] Re-parse message metadata
- [ ] Rebuild UID mappings via Mailboxes
- [ ] Rebuild full-text search index (if Tantivy backend)
- [ ] Add progress reporting for large rebuilds

**Files:**
- `src/index/indexer.rs`
- Add integration test for rebuild

---

### 2.5 Flag Updates via Filesystem (2 days)

**Goal:** Support IMAP flag updates via Maildir-style renames.

**Maildir Convention:**
- `new/` directory: Unread messages
- `cur/` directory: Read messages with flags in filename
- Filename format: `unique-id:2,FLAGS` (FLAGS = DFPRST)

**Tasks:**
- [ ] Implement flag → filename encoding (e.g., `:2,S` for Seen)
- [ ] Implement rename operation in Storage
- [ ] Update StorageWatcher to detect renames and emit events
- [ ] Update Indexer to handle flag update events
- [ ] Add atomic rename support (ensure no race conditions)

**Files:**
- `src/storage/impl/filesystem.rs` - Add rename logic
- `src/storage/watcher/maildir.rs` - Already exists, enhance
- `src/types.rs` - Add flag encoding utilities

---

## Phase 3: Testing & Bug Fixes

### 3.1 Unit Testing (3-4 days)

**High-Priority Test Coverage:**

- [ ] Storage: Write, read, delete, flag updates, streaming
- [ ] StorageWatcher: Event emission on all operations
- [ ] EventBus: Sync/async subscriptions, oneshot channels
- [ ] Mailbox: UID assignment, path mapping, validity
- [ ] Searcher: Read-only operations, query optimization
- [ ] CommandHandlers: Registration, dispatch, handler lifecycle
- [ ] Session: Tag tracking, sequence ID mapping, timeouts

**Test Organization:**
```
tests/
├── unit/
│   ├── storage.rs
│   ├── events.rs
│   ├── mailbox.rs
│   ├── searcher.rs
│   └── command_handlers.rs
```

---

### 3.2 Integration Testing (3-4 days)

**Critical Integration Paths:**

- [ ] End-to-end IMAP session (LOGIN → SELECT → FETCH → LOGOUT)
- [ ] Storage → Watcher → EventBus → Indexer flow
- [ ] Concurrent sessions accessing same mailbox
- [ ] Flag updates propagating through system
- [ ] Mailbox creation/deletion with event propagation
- [ ] Authentication flow with UserStore
- [ ] TLS connection upgrade (STARTTLS)

**Test Organization:**
```
tests/
├── integration/
│   ├── imap_session.rs
│   ├── event_flow.rs
│   ├── concurrent_access.rs
│   └── tls.rs
```

---

### 3.3 Performance Testing (2 days)

**Benchmarks:**

- [ ] Message indexing throughput (messages/sec)
- [ ] Search query latency (p50, p95, p99)
- [ ] Concurrent session capacity
- [ ] Memory usage under load
- [ ] Storage I/O performance

**Tools:**
- Criterion.rs for benchmarking
- Memory profiling with valgrind/heaptrack

**Test Organization:**
```
benches/
├── indexing.rs
├── search.rs
├── concurrent_sessions.rs
└── storage_io.rs
```

---

### 3.4 Bug Fixes & Edge Cases (2-3 days)

**Known Areas to Investigate:**

- [ ] Race conditions in event handling
- [ ] UID validity handling on mailbox recreation
- [ ] Partial writes/reads in Storage
- [ ] Connection timeout edge cases
- [ ] TLS handshake failures
- [ ] Large message handling (>10MB)
- [ ] Special characters in mailbox names
- [ ] Concurrent flag updates on same message

**Process:**
1. Reproduce issue with failing test
2. Fix implementation
3. Verify test passes
4. Add regression test

---

## Phase 4: Documentation & Cleanup

### 4.1 Architecture Documentation (1-2 days)

**Tasks:**
- [ ] Update ARCHITECTURE.md with new design
- [ ] Document Copy/Clone pattern rationale
- [ ] Document event-driven communication patterns
- [ ] Add sequence diagrams for key flows
- [ ] Document extension points (custom commands, storage backends)

---

### 4.2 API Documentation (1 day)

**Tasks:**
- [ ] Add rustdoc comments to all public APIs
- [ ] Document trait methods with examples
- [ ] Document event types and their payloads
- [ ] Add module-level documentation
- [ ] Generate docs with `cargo doc --no-deps --open`

---

### 4.3 Code Cleanup (1 day)

**Tasks:**
- [ ] Remove deprecated code (old trait references)
- [ ] Run clippy and fix warnings: `cargo clippy --all-targets`
- [ ] Format code: `cargo fmt`
- [ ] Remove unused dependencies from Cargo.toml
- [ ] Update dependency versions

---

## Timeline Summary

| Phase | Estimated Duration | Priority |
|-------|-------------------|----------|
| **Phase 1: Key Architectural Changes** | 10-13 days | CRITICAL |
| 1.1 Events Enhancement | 1-2 days | HIGH |
| 1.2 Copy/Clone Architecture | 3-4 days | CRITICAL |
| 1.3 StorageWatcher Decoupling | 2 days | HIGH |
| 1.4 CommandHandlers Registry | 2-3 days | MEDIUM |
| **Phase 2: Broader Refactoring** | 10-11 days | HIGH |
| 2.1 Session Refactoring | 2 days | MEDIUM |
| 2.2 Server Enhancement | 2 days | LOW |
| 2.3 UserStore ACL Enhancement | 2-3 days | LOW |
| 2.4 Indexer Reconstruction | 2 days | MEDIUM |
| 2.5 Flag Updates via Filesystem | 2 days | HIGH |
| **Phase 3: Testing & Bug Fixes** | 10-13 days | CRITICAL |
| 3.1 Unit Testing | 3-4 days | CRITICAL |
| 3.2 Integration Testing | 3-4 days | CRITICAL |
| 3.3 Performance Testing | 2 days | MEDIUM |
| 3.4 Bug Fixes & Edge Cases | 2-3 days | HIGH |
| **Phase 4: Documentation & Cleanup** | 3-4 days | MEDIUM |
| 4.1 Architecture Documentation | 1-2 days | MEDIUM |
| 4.2 API Documentation | 1 day | MEDIUM |
| 4.3 Code Cleanup | 1 day | LOW |
| **TOTAL** | **33-41 days** | |

---

## Risk Assessment

### High-Risk Items

1. **Copy/Clone Architecture Migration (1.2)**
   - **Risk:** Breaking existing code, performance regressions
   - **Mitigation:** Incremental migration, comprehensive benchmarking, keep trait abstractions temporarily

2. **Event-Driven Synchronization (1.1)**
   - **Risk:** Deadlocks, race conditions in event handling
   - **Mitigation:** Thorough testing, async subscriber opt-out mechanism, timeout on sync events

3. **Flag Updates via Rename (2.5)**
   - **Risk:** Data loss during renames, inconsistent state
   - **Mitigation:** Atomic operations, fsync guarantees, rollback mechanism

### Medium-Risk Items

1. **CommandHandlers Registry (1.4)**
   - **Risk:** Breaking existing command handling logic
   - **Mitigation:** Incremental migration per command, integration tests

2. **Indexer Reconstruction (2.4)**
   - **Risk:** Long rebuild times, memory exhaustion
   - **Mitigation:** Streaming reconstruction, progress reporting, memory limits

---

## Success Metrics

### Performance Goals
- [ ] 10% reduction in memory usage (fewer Arc/RwLock allocations)
- [ ] 15% improvement in message indexing throughput
- [ ] Sub-100ms p95 search query latency
- [ ] Support 100+ concurrent sessions

### Code Quality Goals
- [ ] 80%+ test coverage on core components
- [ ] Zero clippy warnings
- [ ] All public APIs documented
- [ ] Clean architecture diagram

### Functional Goals
- [ ] All IMAP4rev1 commands working
- [ ] Flag updates persisted correctly
- [ ] EventBus handles sync/async subscriptions
- [ ] Indexer can rebuild from Storage
- [ ] ACLs enforce permissions

---

## Dependencies & Assumptions

### Dependencies
- Rust 1.70+ (async traits, GATs)
- async_std runtime
- Existing test infrastructure

### Assumptions
- Maildir format for flag storage acceptable
- Single-node deployment (no distributed coordination needed)
- SQLite sufficient for UserStore (no Postgres needed yet)
- EventBus doesn't need persistent event log

---

## Next Steps

1. **Review & Approval:** Stakeholder sign-off on plan
2. **Branch Creation:** Create feature branch for each phase
3. **Begin Phase 1.1:** Events enhancement (foundation for everything)
4. **Daily Standups:** Track progress, adjust timeline
5. **Weekly Demos:** Show working increments

---

## Open Questions

1. Should Storage support multiple backends (S3, etc.) or just filesystem?
2. Do we need distributed UID coordination for multi-node deployments?
3. Should EventBus support persistent event log for crash recovery?
4. What's the migration path for existing deployments (data format changes)?
5. Should we implement IMAP IDLE extension during this refactor?

---

**Document Version:** 1.0
**Last Updated:** 2026-01-04
**Author:** Claude (Architecture Review Agent)
