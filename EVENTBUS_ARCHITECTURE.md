# EventBus Architecture

## Overview

The EventBus provides a publish-subscribe system for inter-component communication. Components no longer have direct dependencies on each other - instead, they publish and subscribe to events.

## Key Principle

**FilesystemWatcher should NOT depend on Index. Instead, Index subscribes to events from FilesystemWatcher.**

## Event Flow

```
FilesystemWatcher (Publisher)
  |
  | publishes MessageCreated/Modified/Deleted events
  |
  v
EventBus
  |
  | routes to subscribers
  |
  v
Index (Subscriber)
```

## Updated Event Types

### MessageCreated
Contains metadata and a path reference (not the full content to avoid large copies):
- `username`, `mailbox`, `unique_id`
- `path` (relative path to message file - Index can use MailStore to retrieve content if needed)
- `flags`, `is_new`
- `from`, `to`, `subject`, `body_preview` (parsed metadata to avoid re-parsing)
- `size`, `internal_date`

**Why path instead of content?** Large emails with attachments could be megabytes. Passing full content through events would create unnecessary copies. The path allows Index to retrieve content from MailStore only when needed.

###  MessageModified
Contains flag update information:
- `username`, `mailbox`, `unique_id`
- `flags` (new flags)

### MessageDeleted
Contains deletion information:
- `username`, `mailbox`, `unique_id`

## Implementation Status

### ✅ Completed
- EventBus implementation with pub-sub
- Updated Event types with rich data
- Event filtering by EventKind

### 🚧 In Progress
- Removing Index dependency from FilesystemWatcher
- Updating FilesystemWatcher to publish rich events
- Creating Index event subscriber

## Next Steps

1. **Update FilesystemWatcher** to:
   - Remove Arc<dyn Index> from constructor
   - Remove Index method calls (add_message, update_flags, delete_message)
   - Publish MessageCreated with all parsed data
   - Publish MessageModified/Deleted with identifiers

2. **Update Index implementations** to:
   - Subscribe to EventKind::MessageCreated/Modified/Deleted
   - Handle events and update internal state
   - Track unique_id -> MessageId mapping internally

3. **Update tests** to:
   - Remove Index from FilesystemWatcher tests
   - Test event publishing
   - Create integration tests showing Index subscribing to events

## Example: Index Subscribing to Events

```rust
// In Index implementation startup/initialization
pub async fn start_event_subscription(
    &self,
    event_bus: Arc<EventBus>,
) -> Result<()> {
    let (_sub_id, mut rx) = event_bus
        .subscribe(vec![
            EventKind::MessageCreated,
            EventKind::MessageModified,
            EventKind::MessageDeleted,
        ])
        .await?;

    let index = self.clone();
    task::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match event {
                Event::MessageCreated {
                    username,
                    mailbox,
                    unique_id,
                    path,
                    flags,
                    from,
                    to,
                    subject,
                    body_preview,
                    size,
                    internal_date,
                    ..
                } => {
                    let message_id = MessageId::new();
                    let uid = index.get_next_uid(&username, &mailbox).await.unwrap();

                    let indexed_message = IndexedMessage {
                        id: message_id,
                        uid,
                        mailbox,
                        flags,
                        internal_date,
                        size,
                        from,
                        to,
                        subject,
                        body_preview,
                    };

                    // Add to index with path for future retrieval
                    index.add_message(&username, &mailbox, indexed_message).await.unwrap();

                    // Track unique_id -> message_id mapping for updates/deletes
                    index.track_message(&unique_id, message_id).await.unwrap();

                    // If needed, can retrieve full content from MailStore:
                    // let content = mail_store.retrieve(&path).await.unwrap();
                }
                Event::MessageModified {
                    username,
                    mailbox,
                    unique_id,
                    flags,
                } => {
                    // Look up MessageId by unique_id
                    // Update flags
                }
                Event::MessageDeleted {
                    username,
                    mailbox,
                    unique_id,
                } => {
                    // Look up MessageId by unique_id
                    // Delete message
                }
                _ => {}
            }
        }
    });

    Ok(())
}
```

## Benefits

1. **Separation of Concerns**: FilesystemWatcher only watches filesystem, doesn't know about Index
2. **Extensibility**: Easy to add new subscribers (analytics, notifications, etc.)
3. **Testability**: Can test FilesystemWatcher without Index
4. **Flexibility**: Index can be swapped out without changing FilesystemWatcher
5. **Observability**: All mail operations flow through EventBus

## Migration Path

For now, keep the old implementation working. The new event-based architecture will be:
1. More maintainable
2. Easier to test
3. More flexible for future features
