//! Queue trait and implementations
//!
//! The Queue is responsible for managing asynchronous tasks that don't
//! require an active connection, such as UID compaction and cleanup.

use async_trait::async_trait;
use uuid::Uuid;
use crate::error::Result;
use crate::types::*;

pub mod r#impl;

/// Trait for queueing asynchronous tasks
#[async_trait]
pub trait Queue: Send + Sync {
    /// Enqueue a task
    async fn enqueue(&self, task: QueueTask) -> Result<()>;

    /// Dequeue the next task
    async fn dequeue(&self) -> Result<Option<QueueTask>>;

    /// Peek at the next task without removing it
    async fn peek(&self) -> Result<Option<QueueTask>>;

    /// Get the number of pending tasks
    async fn len(&self) -> Result<usize>;

    /// Check if the queue is empty
    async fn is_empty(&self) -> Result<bool>;

    /// Remove a specific task by ID
    async fn remove(&self, id: Uuid) -> Result<bool>;

    /// Clear all tasks
    async fn clear(&self) -> Result<()>;

    /// Shutdown the queue gracefully
    async fn shutdown(&self) -> Result<()>;
}
