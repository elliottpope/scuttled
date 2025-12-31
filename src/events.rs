//! Global event system for inter-component communication
//!
//! This module provides a publish-subscribe event bus that allows components
//! to communicate asynchronously through events.

use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use chrono::{DateTime, Utc};
use futures::channel::oneshot;
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::types::MessageFlag;

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
    /// Publish an event
    Publish {
        event: Event,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Subscribe to specific event kinds
    Subscribe {
        kinds: Vec<EventKind>,
        subscriber: Sender<Event>,
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
    sender: Sender<Event>,
}

/// Global event bus for system-wide pub-sub
pub struct EventBus {
    command_tx: Sender<BusCommand>,
}

impl EventBus {
    /// Create a new EventBus
    pub fn new() -> Self {
        let (command_tx, command_rx) = bounded(1000);

        // Spawn the event bus loop
        task::spawn(event_bus_loop(command_rx));

        info!("EventBus initialized");

        Self { command_tx }
    }

    /// Publish an event to all interested subscribers
    pub async fn publish(&self, event: Event) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.command_tx
            .send(BusCommand::Publish { event, reply: tx })
            .await
            .map_err(|_| Error::Internal("Event bus stopped".to_string()))?;

        rx.await
            .map_err(|_| Error::Internal("Event bus dropped reply".to_string()))?
    }

    /// Subscribe to specific event kinds
    ///
    /// Returns a subscription ID and a receiver for events
    pub async fn subscribe(&self, kinds: Vec<EventKind>) -> Result<(SubscriptionId, Receiver<Event>)> {
        let (event_tx, event_rx) = bounded(100);
        let (reply_tx, reply_rx) = oneshot::channel();

        self.command_tx
            .send(BusCommand::Subscribe {
                kinds,
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
async fn event_bus_loop(command_rx: Receiver<BusCommand>) {
    let mut subscribers: HashMap<SubscriptionId, Subscriber> = HashMap::new();

    while let Ok(cmd) = command_rx.recv().await {
        match cmd {
            BusCommand::Publish { event, reply } => {
                let result = handle_publish(&event, &subscribers).await;
                let _ = reply.send(result);
            }
            BusCommand::Subscribe {
                kinds,
                subscriber,
                reply,
            } => {
                let id = SubscriptionId::new();
                subscribers.insert(id, Subscriber {
                    kinds,
                    sender: subscriber,
                });
                debug!("New subscription: {:?} for {:?}", id, subscribers.get(&id).unwrap().kinds);
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

/// Handle publishing an event to all interested subscribers
async fn handle_publish(event: &Event, subscribers: &HashMap<SubscriptionId, Subscriber>) -> Result<()> {
    let event_kind = event.kind();
    let mut delivered = 0;

    for (id, subscriber) in subscribers {
        // Check if this subscriber is interested in this event kind
        if subscriber.kinds.contains(&event_kind) {
            // Try to send the event
            match subscriber.sender.try_send(event.clone()) {
                Ok(_) => {
                    delivered += 1;
                }
                Err(e) => {
                    error!("Failed to deliver event to subscriber {:?}: {}", id, e);
                }
            }
        }
    }

    debug!("Published {:?} to {} subscribers", event_kind, delivered);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn test_event_bus_creation() {
        let bus = EventBus::new();
        bus.shutdown().await.unwrap();
    }

    #[async_std::test]
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
        let received = rx.recv().await.unwrap();
        match received {
            Event::MailboxCreated { username, mailbox } => {
                assert_eq!(username, "alice");
                assert_eq!(mailbox, "INBOX");
            }
            _ => panic!("Wrong event type received"),
        }

        bus.unsubscribe(sub_id).await.unwrap();
        bus.shutdown().await.unwrap();
    }

    #[async_std::test]
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
        let _ = rx1.recv().await.unwrap();
        let _ = rx2.recv().await.unwrap();

        bus.shutdown().await.unwrap();
    }

    #[async_std::test]
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
        let received = rx.recv().await.unwrap();
        match received {
            Event::MailboxCreated { .. } => {} // Expected
            _ => panic!("Received wrong event type"),
        }

        // Channel should be empty now (no message event received)
        assert!(rx.try_recv().is_err());

        bus.shutdown().await.unwrap();
    }
}
