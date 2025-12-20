//! Channel-based queue implementation

use async_std::channel::{bounded, Receiver, Sender};
use async_std::task;
use async_trait::async_trait;
use futures::channel::oneshot;
use std::collections::VecDeque;
use uuid::Uuid;

use crate::error::Result;
use crate::queue::Queue;
use crate::types::*;

/// Commands for the queue writer loop
enum Command {
    Enqueue(QueueTask, oneshot::Sender<Result<()>>),
    Dequeue(oneshot::Sender<Result<Option<QueueTask>>>),
    Peek(oneshot::Sender<Result<Option<QueueTask>>>),
    Len(oneshot::Sender<Result<usize>>),
    IsEmpty(oneshot::Sender<Result<bool>>),
    Remove(Uuid, oneshot::Sender<Result<bool>>),
    Clear(oneshot::Sender<Result<()>>),
    Shutdown(oneshot::Sender<()>),
}

/// Channel-based task queue with single-writer pattern
pub struct ChannelQueue {
    tx: Sender<Command>,
}

impl ChannelQueue {
    pub fn new() -> Self {
        let (tx, rx) = bounded(100);

        task::spawn(writer_loop(rx));

        Self { tx }
    }
}

impl Default for ChannelQueue {
    fn default() -> Self {
        Self::new()
    }
}

async fn writer_loop(rx: Receiver<Command>) {
    let mut queue: VecDeque<QueueTask> = VecDeque::new();

    while let Ok(cmd) = rx.recv().await {
        match cmd {
            Command::Enqueue(task, reply) => {
                queue.push_back(task);
                let _ = reply.send(Ok(()));
            }
            Command::Dequeue(reply) => {
                let task = queue.pop_front();
                let _ = reply.send(Ok(task));
            }
            Command::Peek(reply) => {
                let task = queue.front().cloned();
                let _ = reply.send(Ok(task));
            }
            Command::Len(reply) => {
                let _ = reply.send(Ok(queue.len()));
            }
            Command::IsEmpty(reply) => {
                let _ = reply.send(Ok(queue.is_empty()));
            }
            Command::Remove(id, reply) => {
                if let Some(index) = queue.iter().position(|task| task.id == id) {
                    queue.remove(index);
                    let _ = reply.send(Ok(true));
                } else {
                    let _ = reply.send(Ok(false));
                }
            }
            Command::Clear(reply) => {
                queue.clear();
                let _ = reply.send(Ok(()));
            }
            Command::Shutdown(reply) => {
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
        self.tx.send(Command::Enqueue(task, tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn dequeue(&self) -> Result<Option<QueueTask>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Dequeue(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn peek(&self) -> Result<Option<QueueTask>> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Peek(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn len(&self) -> Result<usize> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Len(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn is_empty(&self) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::IsEmpty(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn remove(&self, id: Uuid) -> Result<bool> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Remove(id, tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn clear(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Clear(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?
    }

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Command::Shutdown(tx)).await
            .map_err(|_| crate::error::Error::QueueError("Channel closed".to_string()))?;
        rx.await
            .map_err(|_| crate::error::Error::QueueError("Reply channel closed".to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[async_std::test]
    async fn test_enqueue_and_dequeue() {
        let queue = ChannelQueue::new();

        let task = QueueTask::new(TaskType::UidCompaction, json!({}));
        queue.enqueue(task.clone()).await.unwrap();

        let dequeued = queue.dequeue().await.unwrap();
        assert!(dequeued.is_some());
        assert_eq!(dequeued.unwrap().id, task.id);
    }

    #[async_std::test]
    async fn test_peek() {
        let queue = ChannelQueue::new();

        let task = QueueTask::new(TaskType::UidCompaction, json!({}));
        queue.enqueue(task.clone()).await.unwrap();

        let peeked = queue.peek().await.unwrap();
        assert!(peeked.is_some());
        assert_eq!(peeked.unwrap().id, task.id);

        let len = queue.len().await.unwrap();
        assert_eq!(len, 1);
    }

    #[async_std::test]
    async fn test_len_and_is_empty() {
        let queue = ChannelQueue::new();

        assert!(queue.is_empty().await.unwrap());
        assert_eq!(queue.len().await.unwrap(), 0);

        let task = QueueTask::new(TaskType::UidCompaction, json!({}));
        queue.enqueue(task).await.unwrap();

        assert!(!queue.is_empty().await.unwrap());
        assert_eq!(queue.len().await.unwrap(), 1);
    }

    #[async_std::test]
    async fn test_remove() {
        let queue = ChannelQueue::new();

        let task1 = QueueTask::new(TaskType::UidCompaction, json!({}));
        let task2 = QueueTask::new(TaskType::IndexUpdate, json!({}));

        queue.enqueue(task1.clone()).await.unwrap();
        queue.enqueue(task2.clone()).await.unwrap();

        let removed = queue.remove(task1.id).await.unwrap();
        assert!(removed);

        assert_eq!(queue.len().await.unwrap(), 1);

        let remaining = queue.dequeue().await.unwrap().unwrap();
        assert_eq!(remaining.id, task2.id);
    }

    #[async_std::test]
    async fn test_clear() {
        let queue = ChannelQueue::new();

        queue.enqueue(QueueTask::new(TaskType::UidCompaction, json!({}))).await.unwrap();
        queue.enqueue(QueueTask::new(TaskType::IndexUpdate, json!({}))).await.unwrap();

        assert_eq!(queue.len().await.unwrap(), 2);

        queue.clear().await.unwrap();

        assert!(queue.is_empty().await.unwrap());
    }

    #[async_std::test]
    async fn test_fifo_order() {
        let queue = ChannelQueue::new();

        let task1 = QueueTask::new(TaskType::UidCompaction, json!({"order": 1}));
        let task2 = QueueTask::new(TaskType::IndexUpdate, json!({"order": 2}));
        let task3 = QueueTask::new(TaskType::Expunge, json!({"order": 3}));

        queue.enqueue(task1.clone()).await.unwrap();
        queue.enqueue(task2.clone()).await.unwrap();
        queue.enqueue(task3.clone()).await.unwrap();

        let dequeued1 = queue.dequeue().await.unwrap().unwrap();
        assert_eq!(dequeued1.id, task1.id);

        let dequeued2 = queue.dequeue().await.unwrap().unwrap();
        assert_eq!(dequeued2.id, task2.id);

        let dequeued3 = queue.dequeue().await.unwrap().unwrap();
        assert_eq!(dequeued3.id, task3.id);
    }

    #[async_std::test]
    async fn test_shutdown() {
        let queue = ChannelQueue::new();

        queue.enqueue(QueueTask::new(TaskType::UidCompaction, json!({}))).await.unwrap();

        queue.shutdown().await.unwrap();

        // After shutdown, operations should fail
        let result = queue.enqueue(QueueTask::new(TaskType::UidCompaction, json!({}))).await;
        assert!(result.is_err());
    }
}
