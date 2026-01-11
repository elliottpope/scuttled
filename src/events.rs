//! Global event system for inter-component communication
//!
//! This module provides a publish-subscribe event bus that allows components
//! to communicate through events with both asynchronous (fire-and-forget) and
//! synchronous (wait-for-acknowledgment) delivery modes.
//!
//! # Event Bus Architecture
//!
//! The EventBus supports two subscription modes:
//!
//! ## Async Subscriptions (Default)
//!
//! Async subscribers receive events without blocking the publisher. This is the
//! default mode and is suitable for most use cases where the publisher doesn't
//! need to wait for event processing to complete.
//!
//! ```ignore
//! use scuttled::events::{EventBus, EventKind};
//!
//! let bus = EventBus::new();
//!
//! // Subscribe asynchronously
//! let (sub_id, mut rx) = bus.subscribe(vec![EventKind::MessageCreated]).await?;
//!
//! // Handle events in background
//! tokio::spawn(async move {
//!     while let Ok(delivery) = rx.recv().await {
//!         // Process event
//!         println!("Received: {:?}", delivery.event());
//!         // No acknowledgment needed for async
//!     }
//! });
//!
//! // Publish (returns immediately)
//! bus.publish(Event::MessageCreated { ... }).await?;
//! ```
//!
//! ## Sync Subscriptions
//!
//! Sync subscribers must acknowledge event processing before the publisher continues.
//! This ensures that critical state changes are fully propagated before the caller proceeds.
//!
//! ```ignore
//! use scuttled::events::{EventBus, EventKind, Event};
//!
//! let bus = EventBus::new();
//!
//! // Subscribe synchronously
//! let (sub_id, mut rx) = bus.subscribe_sync(vec![EventKind::MailboxCreated]).await?;
//!
//! // Handle events and acknowledge
//! tokio::spawn(async move {
//!     while let Ok(delivery) = rx.recv().await {
//!         // Process event
//!         update_index(delivery.event()).await;
//!         // MUST acknowledge for sync subscriptions
//!         delivery.acknowledge();
//!     }
//! });
//!
//! // Publish and wait for all sync subscribers
//! bus.publish_sync(Event::MailboxCreated {
//!     username: "alice".to_string(),
//!     mailbox: "INBOX".to_string(),
//! }).await?;
//! // At this point, all sync subscribers have processed the event
//! ```
//!
//! ## Mixed Subscriptions
//!
//! You can have both sync and async subscribers for the same event:
//!
//! - `publish()`: Delivers to all subscribers but doesn't wait for acknowledgment
//! - `publish_sync()`: Delivers to all subscribers and waits only for sync subscribers to acknowledge
//!
//! ## Event Delivery Modes
//!
//! Events are wrapped in `EventDelivery` enum:
//! - `EventDelivery::Async(event)`: Fire-and-forget delivery
//! - `EventDelivery::Sync { event, ack }`: Requires acknowledgment via oneshot channel
//!
//! ## Guidelines
//!
//! Use **async subscriptions** when:
//! - Event processing is independent of the publisher's flow
//! - Fire-and-forget semantics are acceptable
//! - You want to minimize latency in the publisher
//!
//! Use **sync subscriptions** when:
//! - State changes must be fully propagated before continuing
//! - You need consistency guarantees (e.g., Index must update before query)
//! - Critical operations require confirmation
//!
//! ## Performance Considerations
//!
//! - Async subscriptions have minimal overhead (channel send only)
//! - Sync subscriptions block the publisher until all subscribers acknowledge
//! - Mix subscription types carefully to balance latency and consistency

use tokio::sync::mpsc::{channel, Receiver, Sender};
use chrono::{DateTime, Utc};
use futures::channel::oneshot;
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::types::MessageFlag;

/// Subscription type - determines how events are delivered
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionType {
    /// Async subscription - events are delivered without waiting for acknowledgment
    /// Subscribers process events in the background
    Async,
    /// Sync subscription - event publisher waits for subscriber to acknowledge
    /// Use this when state changes must be fully propagated before continuing
    Sync,
}

/// Event delivery wrapper for subscribers
/// Sync subscribers receive events with an acknowledgment channel
#[derive(Debug)]
pub enum EventDelivery {
    /// Async event delivery - no acknowledgment needed
    Async(Event),
    /// Sync event delivery - subscriber must send acknowledgment when done
    Sync {
        event: Event,
        ack: oneshot::Sender<()>,
    },
}

impl EventDelivery {
    /// Get the inner event regardless of delivery type
    pub fn event(&self) -> &Event {
        match self {
            EventDelivery::Async(event) => event,
            EventDelivery::Sync { event, .. } => event,
        }
    }

    /// Acknowledge processing (only for sync events)
    /// For async events, this is a no-op
    pub fn acknowledge(self) {
        if let EventDelivery::Sync { ack, .. } = self {
            let _ = ack.send(());
        }
    }
}

/// Event types that can occur in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    /// A new mailbox was created
    MailboxCreated {
        username: String,
        mailbox: String,
    },
    /// A mailbox was deleted
    MailboxDeleted {
        username: String,
        mailbox: String,
    },
    /// A mailbox was renamed
    MailboxRenamed {
        username: String,
        old_name: String,
        new_name: String,
    },
    /// A new message was added to the mail store
    MessageCreated {
        username: String,
        mailbox: String,
        unique_id: String,
        /// Path to the message file (relative to mailstore root)
        /// Index can use this with MailStore to retrieve full content if needed
        path: String,
        /// Message flags from mailbox format
        flags: Vec<MessageFlag>,
        /// Whether this is a new/unseen message
        is_new: bool,
        /// Parsed email metadata (to avoid re-parsing)
        from: String,
        to: String,
        subject: String,
        body_preview: String,
        /// Message size in bytes
        size: usize,
        /// When the message was added
        internal_date: DateTime<Utc>,
    },
    /// A message was modified (flags changed, moved, etc.)
    MessageModified {
        username: String,
        mailbox: String,
        unique_id: String,
        flags: Vec<MessageFlag>,
    },
    /// A message was deleted
    MessageDeleted {
        username: String,
        mailbox: String,
        unique_id: String,
    },
}

impl Event {
    /// Get the event kind/category
    pub fn kind(&self) -> EventKind {
        match self {
            Event::MailboxCreated { .. } => EventKind::MailboxCreated,
            Event::MailboxDeleted { .. } => EventKind::MailboxDeleted,
            Event::MailboxRenamed { .. } => EventKind::MailboxRenamed,
            Event::MessageCreated { .. } => EventKind::MessageCreated,
            Event::MessageModified { .. } => EventKind::MessageModified,
            Event::MessageDeleted { .. } => EventKind::MessageDeleted,
        }
    }
}

/// Event kinds for subscription filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    MailboxCreated,
    MailboxDeleted,
    MailboxRenamed,
    MessageCreated,
    MessageModified,
    MessageDeleted,
}

/// Subscription handle that can be used to unsubscribe
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(Uuid);

impl SubscriptionId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Commands for the event bus
enum BusCommand {
    /// Publish an event (async - fire and forget)
    Publish {
        event: Event,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Publish an event and wait for all sync subscribers to acknowledge
    PublishSync {
        event: Event,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Subscribe to specific event kinds
    Subscribe {
        kinds: Vec<EventKind>,
        subscription_type: SubscriptionType,
        subscriber: Sender<EventDelivery>,
        reply: oneshot::Sender<Result<SubscriptionId>>,
    },
    /// Unsubscribe from events
    Unsubscribe {
        id: SubscriptionId,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Shutdown the event bus
    Shutdown(oneshot::Sender<()>),
}

/// Internal subscriber information
struct Subscriber {
    kinds: Vec<EventKind>,
    subscription_type: SubscriptionType,
    sender: Sender<EventDelivery>,
}

/// Global event bus for system-wide pub-sub
pub struct EventBus {
    command_tx: Sender<BusCommand>,
}

impl EventBus {
    /// Create a new EventBus
    pub fn new() -> Self {
        let (command_tx, command_rx) = channel(1000);

        // Spawn the event bus loop
        tokio::spawn(event_bus_loop(command_rx));

        info!("EventBus initialized");

        Self { command_tx }
    }

    /// Publish an event to all interested subscribers (async - fire and forget)
    ///
    /// This method returns immediately after dispatching the event.
    /// Async subscribers receive the event but don't block the publisher.
    /// Sync subscribers receive the event but the publisher doesn't wait for acknowledgment.
    pub async fn publish(&self, event: Event) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(BusCommand::Publish { event, reply: tx })
            .await
            .map_err(|_| Error::Internal("Event bus stopped".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Event bus dropped reply".to_string()))?
    }

    /// Publish an event and wait for all sync subscribers to acknowledge processing
    ///
    /// This method blocks until all sync subscribers have acknowledged the event.
    /// Use this when you need to ensure state changes are fully propagated before continuing.
    /// Async subscribers receive the event but don't block the publisher.
    ///
    /// # Example
    /// ```ignore
    /// // Create a mailbox and wait for Index to finish updating
    /// event_bus.publish_sync(Event::MailboxCreated {
    ///     username: "alice".to_string(),
    ///     mailbox: "INBOX".to_string(),
    /// }).await?;
    /// // Now we can safely query the Index for the new mailbox
    /// ```
    pub async fn publish_sync(&self, event: Event) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(BusCommand::PublishSync { event, reply: tx })
            .await
            .map_err(|_| Error::Internal("Event bus stopped".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Event bus dropped reply".to_string()))?
    }

    /// Subscribe to specific event kinds with async delivery (fire and forget)
    ///
    /// Returns a subscription ID and a receiver for events.
    /// Events are delivered without waiting for acknowledgment.
    pub async fn subscribe(&self, kinds: Vec<EventKind>) -> Result<(SubscriptionId, Receiver<EventDelivery>)> {
        self.subscribe_with_type(kinds, SubscriptionType::Async).await
    }

    /// Subscribe to specific event kinds with sync delivery
    ///
    /// Returns a subscription ID and a receiver for events.
    /// When events are published via `publish_sync()`, the publisher waits for
    /// this subscriber to acknowledge processing by calling `EventDelivery::acknowledge()`.
    pub async fn subscribe_sync(&self, kinds: Vec<EventKind>) -> Result<(SubscriptionId, Receiver<EventDelivery>)> {
        self.subscribe_with_type(kinds, SubscriptionType::Sync).await
    }

    /// Subscribe to specific event kinds with a specific subscription type
    async fn subscribe_with_type(
        &self,
        kinds: Vec<EventKind>,
        subscription_type: SubscriptionType,
    ) -> Result<(SubscriptionId, Receiver<EventDelivery>)> {
        let (event_tx, event_rx) = channel(100);
        let (reply_tx, reply_rx) = oneshot::channel();

        self.command_tx
            .send(BusCommand::Subscribe {
                kinds,
                subscription_type,
                subscriber: event_tx,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::Internal("Event bus stopped".to_string()))?;

        let id = reply_rx
            .await
            .map_err(|_| Error::Internal("Event bus dropped reply".to_string()))??;

        Ok((id, event_rx))
    }

    /// Unsubscribe from events
    pub async fn unsubscribe(&self, id: SubscriptionId) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(BusCommand::Unsubscribe { id, reply: tx })
            .await
            .map_err(|_| Error::Internal("Event bus stopped".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Event bus dropped reply".to_string()))?
    }

    /// Shutdown the event bus gracefully
    pub async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.command_tx.send(BusCommand::Shutdown(tx)).await;
        let _ = rx.await;
        Ok(())
    }
}

/// Main event bus loop
async fn event_bus_loop(mut command_rx: Receiver<BusCommand>) {
    let mut subscribers: HashMap<SubscriptionId, Subscriber> = HashMap::new();

    while let Some(cmd) = command_rx.recv().await {
        match cmd {
            BusCommand::Publish { event, reply } => {
                let result = handle_publish_async(&event, &subscribers).await;
                let _ = reply.send(result);
            }
            BusCommand::PublishSync { event, reply } => {
                let result = handle_publish_sync(&event, &subscribers).await;
                let _ = reply.send(result);
            }
            BusCommand::Subscribe {
                kinds,
                subscription_type,
                subscriber,
                reply,
            } => {
                let id = SubscriptionId::new();
                subscribers.insert(id, Subscriber {
                    kinds: kinds.clone(),
                    subscription_type,
                    sender: subscriber,
                });
                debug!(
                    "New {:?} subscription: {:?} for {:?}",
                    subscription_type, id, kinds
                );
                let _ = reply.send(Ok(id));
            }
            BusCommand::Unsubscribe { id, reply } => {
                subscribers.remove(&id);
                debug!("Unsubscribed: {:?}", id);
                let _ = reply.send(Ok(()));
            }
            BusCommand::Shutdown(reply) => {
                info!("Shutting down EventBus");
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// Handle publishing an event asynchronously (fire and forget)
/// All subscribers receive the event wrapped in EventDelivery::Async
async fn handle_publish_async(event: &Event, subscribers: &HashMap<SubscriptionId, Subscriber>) -> Result<()> {
    let event_kind = event.kind();
    let mut delivered = 0;

    for (id, subscriber) in subscribers {
        // Check if this subscriber is interested in this event kind
        if subscriber.kinds.contains(&event_kind) {
            // Wrap event in async delivery
            let delivery = EventDelivery::Async(event.clone());

            // Try to send the event
            match subscriber.sender.try_send(delivery) {
                Ok(_) => {
                    delivered += 1;
                }
                Err(e) => {
                    error!("Failed to deliver event to subscriber {:?}: {}", id, e);
                }
            }
        }
    }

    debug!("Published {:?} (async) to {} subscribers", event_kind, delivered);
    Ok(())
}

/// Handle publishing an event synchronously
/// Sync subscribers get EventDelivery::Sync with an ack channel
/// Async subscribers get EventDelivery::Async
/// Waits for all sync subscribers to acknowledge before returning
async fn handle_publish_sync(event: &Event, subscribers: &HashMap<SubscriptionId, Subscriber>) -> Result<()> {
    let event_kind = event.kind();
    let mut delivered = 0;
    let mut ack_receivers = Vec::new();

    for (id, subscriber) in subscribers {
        // Check if this subscriber is interested in this event kind
        if subscriber.kinds.contains(&event_kind) {
            let delivery = match subscriber.subscription_type {
                SubscriptionType::Async => {
                    // Async subscribers get fire-and-forget delivery
                    EventDelivery::Async(event.clone())
                }
                SubscriptionType::Sync => {
                    // Sync subscribers get delivery with acknowledgment channel
                    let (ack_tx, ack_rx) = oneshot::channel();
                    ack_receivers.push(ack_rx);
                    EventDelivery::Sync {
                        event: event.clone(),
                        ack: ack_tx,
                    }
                }
            };

            // Try to send the event
            match subscriber.sender.try_send(delivery) {
                Ok(_) => {
                    delivered += 1;
                }
                Err(e) => {
                    error!("Failed to deliver event to subscriber {:?}: {}", id, e);
                }
            }
        }
    }

    // Wait for all sync subscribers to acknowledge
    let sync_count = ack_receivers.len();
    for ack_rx in ack_receivers {
        // Wait for acknowledgment, but don't fail if the channel is dropped
        // (subscriber might have been unsubscribed)
        let _ = ack_rx.await;
    }

    debug!(
        "Published {:?} (sync) to {} subscribers ({} sync, {} async)",
        event_kind,
        delivered,
        sync_count,
        delivered - sync_count
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_bus_creation() {
        let bus = EventBus::new();
        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_subscribe_and_publish() {
        let bus = EventBus::new();

        // Subscribe to mailbox events
        let (sub_id, mut rx) = bus
            .subscribe(vec![EventKind::MailboxCreated])
            .await
            .unwrap();

        // Publish a mailbox created event
        let event = Event::MailboxCreated {
            username: "alice".to_string(),
            mailbox: "INBOX".to_string(),
        };
        bus.publish(event.clone()).await.unwrap();

        // Receive the event
        let delivery = rx.recv().await.unwrap();
        match delivery.event() {
            Event::MailboxCreated { username, mailbox } => {
                assert_eq!(username, "alice");
                assert_eq!(mailbox, "INBOX");
            }
            _ => panic!("Wrong event type received"),
        }

        bus.unsubscribe(sub_id).await.unwrap();
        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();

        // Two subscribers to the same event
        let (_id1, mut rx1) = bus
            .subscribe(vec![EventKind::MessageCreated])
            .await
            .unwrap();
        let (_id2, mut rx2) = bus
            .subscribe(vec![EventKind::MessageCreated])
            .await
            .unwrap();

        // Publish an event
        let event = Event::MessageCreated {
            username: "bob".to_string(),
            mailbox: "Sent".to_string(),
            unique_id: "test123".to_string(),
            path: "bob/Sent/test123.eml".to_string(),
            flags: vec![],
            is_new: true,
            from: "sender@example.com".to_string(),
            to: "bob@example.com".to_string(),
            subject: "Test Subject".to_string(),
            body_preview: "Test body".to_string(),
            size: 100,
            internal_date: Utc::now(),
        };
        bus.publish(event).await.unwrap();

        // Both should receive it
        let delivery1 = rx1.recv().await.unwrap();
        let delivery2 = rx2.recv().await.unwrap();

        // Verify both are async deliveries
        assert!(matches!(delivery1, EventDelivery::Async(_)));
        assert!(matches!(delivery2, EventDelivery::Async(_)));

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_event_filtering() {
        let bus = EventBus::new();

        // Subscribe only to mailbox events
        let (_id, mut rx) = bus
            .subscribe(vec![EventKind::MailboxCreated])
            .await
            .unwrap();

        // Publish a message event (should not be received)
        let message_event = Event::MessageCreated {
            username: "alice".to_string(),
            mailbox: "INBOX".to_string(),
            unique_id: "msg1".to_string(),
            path: "alice/INBOX/msg1.eml".to_string(),
            flags: vec![],
            is_new: true,
            from: "test@example.com".to_string(),
            to: "alice@example.com".to_string(),
            subject: "Test".to_string(),
            body_preview: "Test".to_string(),
            size: 50,
            internal_date: Utc::now(),
        };
        bus.publish(message_event).await.unwrap();

        // Publish a mailbox event (should be received)
        let mailbox_event = Event::MailboxCreated {
            username: "alice".to_string(),
            mailbox: "Drafts".to_string(),
        };
        bus.publish(mailbox_event).await.unwrap();

        // Should only receive the mailbox event
        let delivery = rx.recv().await.unwrap();
        match delivery.event() {
            Event::MailboxCreated { .. } => {} // Expected
            _ => panic!("Received wrong event type"),
        }

        // Channel should be empty now (no message event received)
        assert!(rx.try_recv().is_err());

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_sync_subscription() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let bus = EventBus::new();
        let verified = Arc::new(AtomicBool::new(false));
        let verified_clone = verified.clone();

        // Subscribe with sync delivery
        let (_sub_id, mut rx) = bus
            .subscribe_sync(vec![EventKind::MailboxCreated])
            .await
            .unwrap();

        // Spawn a task to handle the event
        tokio::spawn(async move {
            let delivery = rx.recv().await.unwrap();

            // Verify it's a sync delivery
            match &delivery {
                EventDelivery::Sync { event, .. } => {
                    match event {
                        Event::MailboxCreated { username, mailbox } => {
                            assert_eq!(username, "alice");
                            assert_eq!(mailbox, "INBOX");
                            verified_clone.store(true, Ordering::SeqCst);
                        }
                        _ => panic!("Wrong event type received"),
                    }
                }
                _ => panic!("Expected sync delivery"),
            }

            // Acknowledge the event
            delivery.acknowledge();
        });

        // Publish a mailbox created event (waits for acknowledgment)
        let event = Event::MailboxCreated {
            username: "alice".to_string(),
            mailbox: "INBOX".to_string(),
        };
        bus.publish_sync(event.clone()).await.unwrap();

        // After publish_sync returns, the event should be verified
        assert!(verified.load(Ordering::SeqCst));

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_publish_sync_waits_for_acknowledgment() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let bus = EventBus::new();
        let processed = Arc::new(AtomicBool::new(false));
        let processed_clone = processed.clone();

        // Subscribe with sync delivery
        let (_sub_id, mut rx) = bus
            .subscribe_sync(vec![EventKind::MailboxCreated])
            .await
            .unwrap();

        // Spawn a task to handle the event with a delay
        tokio::spawn(async move {
            let delivery = rx.recv().await.unwrap();
            // Simulate some processing time
            tokio::time::sleep(Duration::from_millis(100)).await;
            processed_clone.store(true, Ordering::SeqCst);
            delivery.acknowledge();
        });

        // Publish sync - should wait for acknowledgment
        let event = Event::MailboxCreated {
            username: "alice".to_string(),
            mailbox: "INBOX".to_string(),
        };
        bus.publish_sync(event).await.unwrap();

        // After publish_sync returns, the event should be processed
        assert!(processed.load(Ordering::SeqCst));

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mixed_sync_and_async_subscribers() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let bus = EventBus::new();
        let sync_count = Arc::new(AtomicU32::new(0));
        let async_count = Arc::new(AtomicU32::new(0));

        // Sync subscriber
        let (_id1, mut rx1) = bus
            .subscribe_sync(vec![EventKind::MailboxCreated])
            .await
            .unwrap();
        let sync_count_clone = sync_count.clone();
        tokio::spawn(async move {
            while let Some(delivery) = rx1.recv().await {
                sync_count_clone.fetch_add(1, Ordering::SeqCst);
                delivery.acknowledge();
            }
        });

        // Async subscriber
        let (_id2, mut rx2) = bus
            .subscribe(vec![EventKind::MailboxCreated])
            .await
            .unwrap();
        let async_count_clone = async_count.clone();
        tokio::spawn(async move {
            while let Some(delivery) = rx2.recv().await {
                async_count_clone.fetch_add(1, Ordering::SeqCst);
                delivery.acknowledge(); // No-op for async
            }
        });

        // Publish sync
        let event = Event::MailboxCreated {
            username: "alice".to_string(),
            mailbox: "INBOX".to_string(),
        };
        bus.publish_sync(event).await.unwrap();

        // Give async subscriber time to receive
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Both should have received the event
        assert_eq!(sync_count.load(Ordering::SeqCst), 1);
        assert_eq!(async_count.load(Ordering::SeqCst), 1);

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_async_publish_does_not_wait() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        let bus = EventBus::new();
        let processed = Arc::new(AtomicBool::new(false));
        let processed_clone = processed.clone();

        // Subscribe with sync delivery
        let (_sub_id, mut rx) = bus
            .subscribe_sync(vec![EventKind::MailboxCreated])
            .await
            .unwrap();

        // Spawn a task to handle the event with a delay
        tokio::spawn(async move {
            let delivery = rx.recv().await.unwrap();
            // Simulate some processing time
            tokio::time::sleep(Duration::from_millis(100)).await;
            processed_clone.store(true, Ordering::SeqCst);
            delivery.acknowledge();
        });

        // Publish async - should NOT wait for acknowledgment
        let event = Event::MailboxCreated {
            username: "alice".to_string(),
            mailbox: "INBOX".to_string(),
        };
        bus.publish(event).await.unwrap();

        // After async publish returns, the event should NOT be processed yet
        assert!(!processed.load(Ordering::SeqCst));

        // Wait for processing to complete
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(processed.load(Ordering::SeqCst));

        bus.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_sync_subscribers_all_acknowledged() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let bus = EventBus::new();
        let count = Arc::new(AtomicU32::new(0));

        // Create 3 sync subscribers
        for _ in 0..3 {
            let (_id, mut rx) = bus
                .subscribe_sync(vec![EventKind::MailboxCreated])
                .await
                .unwrap();
            let count_clone = count.clone();
            tokio::spawn(async move {
                while let Some(delivery) = rx.recv().await {
                    count_clone.fetch_add(1, Ordering::SeqCst);
                    delivery.acknowledge();
                }
            });
        }

        // Publish sync
        let event = Event::MailboxCreated {
            username: "alice".to_string(),
            mailbox: "INBOX".to_string(),
        };
        bus.publish_sync(event).await.unwrap();

        // All 3 subscribers should have processed and acknowledged
        assert_eq!(count.load(Ordering::SeqCst), 3);

        bus.shutdown().await.unwrap();
    }
}
