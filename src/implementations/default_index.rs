//! Tantivy-based search index implementation

use async_std::sync::RwLock;
use async_trait::async_trait;
use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::*;
use tantivy::{doc, Index, IndexWriter, ReloadPolicy, Term};
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::traits::Index as IndexTrait;
use crate::types::*;

/// Tantivy-based search index
pub struct DefaultIndex {
    index: Arc<tantivy::Index>,
    schema: Schema,
    writer: Arc<RwLock<IndexWriter>>,
    id_field: Field,
    uid_field: Field,
    mailbox_field: Field,
    from_field: Field,
    to_field: Field,
    subject_field: Field,
    body_field: Field,
    flags_field: Field,
}

impl DefaultIndex {
    /// Create a new index at the given path
    pub fn new<P: AsRef<Path>>(index_path: P) -> Result<Self> {
        let mut schema_builder = Schema::builder();

        let id_field = schema_builder.add_text_field("id", STRING | STORED);
        let uid_field = schema_builder.add_u64_field("uid", INDEXED | STORED);
        let mailbox_field = schema_builder.add_text_field("mailbox", STRING | STORED);
        let from_field = schema_builder.add_text_field("from", TEXT | STORED);
        let to_field = schema_builder.add_text_field("to", TEXT | STORED);
        let subject_field = schema_builder.add_text_field("subject", TEXT | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let flags_field = schema_builder.add_text_field("flags", STRING);

        let schema = schema_builder.build();

        std::fs::create_dir_all(&index_path)?;
        let index = Index::create_in_dir(&index_path, schema.clone())?;

        let writer = index.writer(50_000_000)?;

        Ok(Self {
            index: Arc::new(index),
            schema,
            writer: Arc::new(RwLock::new(writer)),
            id_field,
            uid_field,
            mailbox_field,
            from_field,
            to_field,
            subject_field,
            body_field,
            flags_field,
        })
    }

    /// Create an in-memory index for testing
    pub fn in_memory() -> Result<Self> {
        let mut schema_builder = Schema::builder();

        let id_field = schema_builder.add_text_field("id", STRING | STORED);
        let uid_field = schema_builder.add_u64_field("uid", INDEXED | STORED);
        let mailbox_field = schema_builder.add_text_field("mailbox", STRING | STORED);
        let from_field = schema_builder.add_text_field("from", TEXT | STORED);
        let to_field = schema_builder.add_text_field("to", TEXT | STORED);
        let subject_field = schema_builder.add_text_field("subject", TEXT | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let flags_field = schema_builder.add_text_field("flags", STRING);

        let schema = schema_builder.build();
        let index = Index::create_in_ram(schema.clone());
        let writer = index.writer(50_000_000)?;

        Ok(Self {
            index: Arc::new(index),
            schema,
            writer: Arc::new(RwLock::new(writer)),
            id_field,
            uid_field,
            mailbox_field,
            from_field,
            to_field,
            subject_field,
            body_field,
            flags_field,
        })
    }

    fn parse_email_headers(&self, content: &[u8]) -> (String, String, String, String) {
        let content_str = String::from_utf8_lossy(content);
        let mut from = String::new();
        let mut to = String::new();
        let mut subject = String::new();
        let mut body = String::new();

        let mut in_body = false;
        for line in content_str.lines() {
            if in_body {
                body.push_str(line);
                body.push('\n');
            } else if line.is_empty() {
                in_body = true;
            } else if line.starts_with("From:") {
                from = line[5..].trim().to_string();
            } else if line.starts_with("To:") {
                to = line[3..].trim().to_string();
            } else if line.starts_with("Subject:") {
                subject = line[8..].trim().to_string();
            }
        }

        (from, to, subject, body)
    }

    fn build_search_query(&self, mailbox: &str, query: &SearchQuery) -> Box<dyn Query> {
        let mailbox_term = Term::from_field_text(self.mailbox_field, mailbox);
        let mailbox_query = TermQuery::new(mailbox_term, IndexRecordOption::Basic);

        match query {
            SearchQuery::All => Box::new(mailbox_query),
            SearchQuery::Text(text) => {
                let query_parser = QueryParser::for_index(
                    &self.index,
                    vec![self.from_field, self.to_field, self.subject_field, self.body_field],
                );
                if let Ok(parsed_query) = query_parser.parse_query(text) {
                    Box::new(BooleanQuery::new(vec![
                        (Occur::Must, Box::new(mailbox_query) as Box<dyn Query>),
                        (Occur::Must, parsed_query),
                    ]))
                } else {
                    Box::new(mailbox_query)
                }
            }
            SearchQuery::From(from) => {
                let query_parser = QueryParser::for_index(&self.index, vec![self.from_field]);
                if let Ok(parsed_query) = query_parser.parse_query(from) {
                    Box::new(BooleanQuery::new(vec![
                        (Occur::Must, Box::new(mailbox_query) as Box<dyn Query>),
                        (Occur::Must, parsed_query),
                    ]))
                } else {
                    Box::new(mailbox_query)
                }
            }
            SearchQuery::To(to) => {
                let query_parser = QueryParser::for_index(&self.index, vec![self.to_field]);
                if let Ok(parsed_query) = query_parser.parse_query(to) {
                    Box::new(BooleanQuery::new(vec![
                        (Occur::Must, Box::new(mailbox_query) as Box<dyn Query>),
                        (Occur::Must, parsed_query),
                    ]))
                } else {
                    Box::new(mailbox_query)
                }
            }
            SearchQuery::Subject(subject) => {
                let query_parser = QueryParser::for_index(&self.index, vec![self.subject_field]);
                if let Ok(parsed_query) = query_parser.parse_query(subject) {
                    Box::new(BooleanQuery::new(vec![
                        (Occur::Must, Box::new(mailbox_query) as Box<dyn Query>),
                        (Occur::Must, parsed_query),
                    ]))
                } else {
                    Box::new(mailbox_query)
                }
            }
            SearchQuery::Body(body) => {
                let query_parser = QueryParser::for_index(&self.index, vec![self.body_field]);
                if let Ok(parsed_query) = query_parser.parse_query(body) {
                    Box::new(BooleanQuery::new(vec![
                        (Occur::Must, Box::new(mailbox_query) as Box<dyn Query>),
                        (Occur::Must, parsed_query),
                    ]))
                } else {
                    Box::new(mailbox_query)
                }
            }
            SearchQuery::Seen => {
                let term = Term::from_field_text(self.flags_field, "Seen");
                let flags_query = TermQuery::new(term, IndexRecordOption::Basic);
                Box::new(BooleanQuery::new(vec![
                    (Occur::Must, Box::new(mailbox_query) as Box<dyn Query>),
                    (Occur::Must, Box::new(flags_query) as Box<dyn Query>),
                ]))
            }
            _ => Box::new(mailbox_query),
        }
    }
}

#[async_trait]
impl IndexTrait for DefaultIndex {
    async fn index_message(&self, message: &Message) -> Result<()> {
        let (from, to, subject, body) = self.parse_email_headers(&message.raw_content);

        let flags_str = message
            .flags
            .iter()
            .map(|f| f.to_imap_string())
            .collect::<Vec<_>>()
            .join(" ");

        let doc = doc!(
            self.id_field => message.id.0.to_string(),
            self.uid_field => message.uid as u64,
            self.mailbox_field => message.mailbox.clone(),
            self.from_field => from,
            self.to_field => to,
            self.subject_field => subject,
            self.body_field => body,
            self.flags_field => flags_str,
        );

        let mut writer = self.writer.write().await;
        writer.add_document(doc)?;
        writer.commit()?;

        Ok(())
    }

    async fn remove_message(&self, id: MessageId) -> Result<()> {
        let term = Term::from_field_text(self.id_field, &id.0.to_string());
        let mut writer = self.writer.write().await;
        writer.delete_term(term);
        writer.commit()?;
        Ok(())
    }

    async fn search(&self, mailbox: &str, query: &SearchQuery) -> Result<Vec<MessageId>> {
        let reader = self
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| Error::Index(format!("Failed to create reader: {}", e)))?;

        let searcher = reader.searcher();
        let search_query = self.build_search_query(mailbox, query);

        let top_docs = searcher
            .search(&*search_query, &TopDocs::with_limit(1000))
            .map_err(|e| Error::Index(format!("Search failed: {}", e)))?;

        let mut message_ids = Vec::new();
        for (_score, doc_address) in top_docs {
            if let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(doc_address) {
                if let Some(id_value) = doc.get_first(self.id_field) {
                    if let Some(id_str) = id_value.as_str() {
                        if let Ok(uuid) = uuid::Uuid::parse_str(id_str) {
                            message_ids.push(MessageId(uuid));
                        }
                    }
                }
            }
        }

        Ok(message_ids)
    }

    async fn update_message(&self, message: &Message) -> Result<()> {
        self.remove_message(message.id).await?;
        self.index_message(message).await?;
        Ok(())
    }

    async fn clear_mailbox(&self, mailbox: &str) -> Result<()> {
        let term = Term::from_field_text(self.mailbox_field, mailbox);
        let mut writer = self.writer.write().await;
        writer.delete_term(term);
        writer.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[async_std::test]
    async fn test_index_and_search_message() {
        let index = DefaultIndex::in_memory().unwrap();

        let message = Message {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            raw_content: b"From: sender@example.com\nTo: recipient@example.com\nSubject: Test\n\nHello World"
                .to_vec(),
        };

        index.index_message(&message).await.unwrap();

        let results = index
            .search("INBOX", &SearchQuery::Text("Hello".to_string()))
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], message.id);
    }

    #[async_std::test]
    async fn test_search_by_from() {
        let index = DefaultIndex::in_memory().unwrap();

        let message = Message {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            raw_content: b"From: alice@example.com\nTo: bob@example.com\nSubject: Test\n\nHello"
                .to_vec(),
        };

        index.index_message(&message).await.unwrap();

        let results = index
            .search("INBOX", &SearchQuery::From("alice".to_string()))
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], message.id);
    }

    #[async_std::test]
    async fn test_remove_message() {
        let index = DefaultIndex::in_memory().unwrap();

        let message = Message {
            id: MessageId::new(),
            uid: 1,
            mailbox: "INBOX".to_string(),
            flags: vec![],
            internal_date: Utc::now(),
            size: 100,
            raw_content: b"From: sender@example.com\nTo: recipient@example.com\nSubject: Test\n\nHello"
                .to_vec(),
        };

        index.index_message(&message).await.unwrap();
        index.remove_message(message.id).await.unwrap();

        let results = index
            .search("INBOX", &SearchQuery::All)
            .await
            .unwrap();

        assert_eq!(results.len(), 0);
    }
}
