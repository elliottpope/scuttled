//! In-memory queue implementation

use async_std::sync::RwLock;
use async_trait::async_trait;
use std::collections::VecDeque;
use uuid::Uuid;

use crate::error::Result;
use crate::traits::Queue;
use crate::types::*;

/// In-memory task queue
pub struct InMemoryQueue {
    queue: RwLock<VecDeque<QueueTask>>,
}

impl InMemoryQueue {
    pub fn new() -> Self {
        Self {
            queue: RwLock::new(VecDeque::new()),
        }
    }
}

impl Default for InMemoryQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Queue for InMemoryQueue {
    async fn enqueue(&self, task: QueueTask) -> Result<()> {
        let mut queue = self.queue.write().await;
        queue.push_back(task);
        Ok(())
    }

    async fn dequeue(&self) -> Result<Option<QueueTask>> {
        let mut queue = self.queue.write().await;
        Ok(queue.pop_front())
    }

    async fn peek(&self) -> Result<Option<QueueTask>> {
        let queue = self.queue.read().await;
        Ok(queue.front().cloned())
    }

    async fn len(&self) -> Result<usize> {
        let queue = self.queue.read().await;
        Ok(queue.len())
    }

    async fn is_empty(&self) -> Result<bool> {
        let queue = self.queue.read().await;
        Ok(queue.is_empty())
    }

    async fn remove(&self, id: Uuid) -> Result<bool> {
        let mut queue = self.queue.write().await;
        if let Some(index) = queue.iter().position(|task| task.id == id) {
            queue.remove(index);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn clear(&self) -> Result<()> {
        let mut queue = self.queue.write().await;
        queue.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    #[async_std::test]
    async fn test_enqueue_and_dequeue() {
        let queue = InMemoryQueue::new();

        let task = QueueTask::new(TaskType::UidCompaction, json!({}));
        queue.enqueue(task.clone()).await.unwrap();

        let dequeued = queue.dequeue().await.unwrap();
        assert!(dequeued.is_some());
        assert_eq!(dequeued.unwrap().id, task.id);
    }

    #[async_std::test]
    async fn test_peek() {
        let queue = InMemoryQueue::new();

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
        let queue = InMemoryQueue::new();

        assert!(queue.is_empty().await.unwrap());
        assert_eq!(queue.len().await.unwrap(), 0);

        let task = QueueTask::new(TaskType::UidCompaction, json!({}));
        queue.enqueue(task).await.unwrap();

        assert!(!queue.is_empty().await.unwrap());
        assert_eq!(queue.len().await.unwrap(), 1);
    }

    #[async_std::test]
    async fn test_remove() {
        let queue = InMemoryQueue::new();

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
        let queue = InMemoryQueue::new();

        queue.enqueue(QueueTask::new(TaskType::UidCompaction, json!({}))).await.unwrap();
        queue.enqueue(QueueTask::new(TaskType::IndexUpdate, json!({}))).await.unwrap();

        assert_eq!(queue.len().await.unwrap(), 2);

        queue.clear().await.unwrap();

        assert!(queue.is_empty().await.unwrap());
    }

    #[async_std::test]
    async fn test_fifo_order() {
        let queue = InMemoryQueue::new();

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
}
