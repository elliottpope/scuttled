//! Channel-based queue implementation

use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::RwLock;
use std::sync::Arc;
use async_trait::async_trait;
use futures::channel::oneshot;
use std::collections::VecDeque;
use uuid::Uuid;

use crate::error::Result;
use crate::queue::Queue;
use crate::types::*;

/// Write commands for the queue (only writes go through the channel)
enum WriteCommand {
    Enqueue(QueueTask, oneshot::Sender<Result<()>>),
    Dequeue(oneshot::Sender<Result<Option<QueueTask>>>),
    Remove(Uuid, oneshot::Sender<Result<bool>>),
    Clear(oneshot::Sender<Result<()>>),
    Shutdown(oneshot::Sender<()>),
}

/// Channel-based task queue
///
/// Uses a channel-based writer loop for write operations (enqueue, dequeue, remove, clear)
/// to ensure ordering, while read operations (peek, len, is_empty) access the state directly.
pub struct ChannelQueue {
    state: Arc<RwLock<VecDeque<QueueTask>>>,
    write_tx: Sender<WriteCommand>,
}

impl ChannelQueue {
    pub fn new() -> Self {
        let state = Arc::new(RwLock::new(VecDeque::new()));
        let (write_tx, write_rx) = channel(100);

        let state_clone = Arc::clone(&state);
        tokio::spawn(writer_loop(write_rx, state_clone));

        Self { state, write_tx }
    }
}

impl Default for ChannelQueue {
    fn default() -> Self {
        Self::new()
    }
}

async fn writer_loop(mut rx: Receiver<WriteCommand>, state: Arc<RwLock<VecDeque<QueueTask>>>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::Enqueue(task, reply) => {
                let mut queue = state.write().await;
                queue.push_back(task);
                let _ = reply.send(Ok(()));
            }
            WriteCommand::Dequeue(reply) => {
                let mut queue = state.write().await;
                let task = queue.pop_front();
                let _ = reply.send(Ok(task));
            }
            WriteCommand::Remove(id, reply) => {
                let mut queue = state.write().await;
                if let Some(index) = queue.iter().position(|task| task.id == id) {
                    queue.remove(index);
                    let _ = reply.send(Ok(true));
                } else {
                    let _ = reply.send(Ok(false));
                }
            }
            WriteCommand::Clear(reply) => {
                let mut queue = state.write().await;
                queue.clear();
                let _ = reply.send(Ok(()));
            }
            WriteCommand::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

#[async_trait]
impl Queue for ChannelQueue {
    async fn enqueue(&self, task: QueueTask) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx.send(WriteCommand::Enqueue(task, tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn dequeue(&self) -> Result<Option<QueueTask>> {
        let (tx, rx) = oneshot::channel();
        self.write_tx.send(WriteCommand::Dequeue(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn peek(&self) -> Result<Option<QueueTask>> {
        // Direct read - no channel needed
        let queue = self.state.read().await;
        Ok(queue.front().cloned())
    }

    async fn len(&self) -> Result<usize> {
        // Direct read - no channel needed
        let queue = self.state.read().await;
        Ok(queue.len())
    }

    async fn is_empty(&self) -> Result<bool> {
        // Direct read - no channel needed
        let queue = self.state.read().await;
        Ok(queue.is_empty())
    }

    async fn remove(&self, id: Uuid) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.write_tx.send(WriteCommand::Remove(id, tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn clear(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx.send(WriteCommand::Clear(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.write_tx.send(WriteCommand::Shutdown(tx)).await;
        let _ = rx.await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_enqueue_and_dequeue() {
        let queue = ChannelQueue::new();

        let task = QueueTask::new(TaskType::UidCompaction, serde_json::json!({"test": "data"}));
        queue.enqueue(task.clone()).await.unwrap();

        let dequeued = queue.dequeue().await.unwrap();
        assert!(dequeued.is_some());
        assert_eq!(dequeued.unwrap().id, task.id);
    }

    #[tokio::test]
    async fn test_peek() {
        let queue = ChannelQueue::new();

        let task = QueueTask::new(TaskType::IndexUpdate, serde_json::json!({"test": "data"}));
        queue.enqueue(task.clone()).await.unwrap();

        let peeked = queue.peek().await.unwrap();
        assert!(peeked.is_some());
        assert_eq!(peeked.unwrap().id, task.id);

        // Queue should still have the task
        assert_eq!(queue.len().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_len_and_is_empty() {
        let queue = ChannelQueue::new();

        assert!(queue.is_empty().await.unwrap());
        assert_eq!(queue.len().await.unwrap(), 0);

        let task = QueueTask::new(TaskType::UidCompaction, serde_json::json!({"test": "data"}));
        queue.enqueue(task).await.unwrap();

        assert!(!queue.is_empty().await.unwrap());
        assert_eq!(queue.len().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_fifo_order() {
        let queue = ChannelQueue::new();

        let task1 = QueueTask::new(TaskType::UidCompaction, serde_json::json!({"num": 1}));
        let task2 = QueueTask::new(TaskType::IndexUpdate, serde_json::json!({"num": 2}));

        queue.enqueue(task1.clone()).await.unwrap();
        queue.enqueue(task2.clone()).await.unwrap();

        let first = queue.dequeue().await.unwrap().unwrap();
        let second = queue.dequeue().await.unwrap().unwrap();

        assert_eq!(first.id, task1.id);
        assert_eq!(second.id, task2.id);
    }

    #[tokio::test]
    async fn test_remove() {
        let queue = ChannelQueue::new();

        let task1 = QueueTask::new(TaskType::UidCompaction, serde_json::json!({"num": 1}));
        let task2 = QueueTask::new(TaskType::IndexUpdate, serde_json::json!({"num": 2}));

        queue.enqueue(task1.clone()).await.unwrap();
        queue.enqueue(task2.clone()).await.unwrap();

        let removed = queue.remove(task1.id).await.unwrap();
        assert!(removed);

        assert_eq!(queue.len().await.unwrap(), 1);

        let remaining = queue.dequeue().await.unwrap().unwrap();
        assert_eq!(remaining.id, task2.id);
    }

    #[tokio::test]
    async fn test_clear() {
        let queue = ChannelQueue::new();

        let task1 = QueueTask::new(TaskType::UidCompaction, serde_json::json!({"num": 1}));
        let task2 = QueueTask::new(TaskType::IndexUpdate, serde_json::json!({"num": 2}));

        queue.enqueue(task1).await.unwrap();
        queue.enqueue(task2).await.unwrap();

        queue.clear().await.unwrap();

        assert!(queue.is_empty().await.unwrap());
        assert_eq!(queue.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_shutdown() {
        let queue = ChannelQueue::new();
        queue.shutdown().await.unwrap();
    }
}
