//! Tantivy-based index implementation

use async_std::channel::{bounded, Receiver, Sender};
use async_std::path::PathBuf;
use async_std::task;
use async_trait::async_trait;
use futures::channel::oneshot;
use std::collections::HashMap;
use std::sync::Arc as StdArc;
use tantivy::{
    collector::TopDocs,
    doc,
    query::QueryParser,
    schema::*,
    Index as TantivyIndex, IndexReader,
    TantivyDocument,
};

use crate::error::{Error, Result};
use crate::index::{Index, IndexedMessage};
use crate::types::*;

/// Write commands for the index (only writes go through the channel)
enum WriteCommand {
    CreateMailbox(String, String, oneshot::Sender<Result<()>>),
    DeleteMailbox(String, String, oneshot::Sender<Result<()>>),
    RenameMailbox(String, String, String, oneshot::Sender<Result<()>>),
    AddMessage(String, String, IndexedMessage, oneshot::Sender<Result<String>>),
    UpdateFlags(MessageId, Vec<MessageFlag>, oneshot::Sender<Result<()>>),
    DeleteMessage(MessageId, oneshot::Sender<Result<()>>),
    GetNextUid(String, String, oneshot::Sender<Result<Uid>>),
    Shutdown(oneshot::Sender<()>),
}

/// Mailbox state (not stored in Tantivy)
struct MailboxState {
    info: Mailbox,
    uid_counter: Uid,
}

/// Tantivy-based index
///
/// Uses Tantivy for full-text search and metadata storage.
/// Write operations go through a channel, reads query Tantivy directly.
pub struct TantivyBackedIndex {
    schema: Schema,
    index: StdArc<TantivyIndex>,
    reader: IndexReader,
    write_tx: Sender<WriteCommand>,

    // Field handles for querying
    message_id_field: Field,
    uid_field: Field,
    username_field: Field,
    mailbox_field: Field,
    from_field: Field,
    to_field: Field,
    subject_field: Field,
    body_field: Field,
    flags_field: Field,
    path_field: Field,
}

impl TantivyBackedIndex {
    pub fn new<P: Into<PathBuf>>(index_path: P) -> Result<Self> {
        let index_path = index_path.into();

        // Build schema
        let mut schema_builder = Schema::builder();
        let message_id_field = schema_builder.add_text_field("message_id", STRING | STORED);
        let uid_field = schema_builder.add_u64_field("uid", INDEXED | STORED);
        let username_field = schema_builder.add_text_field("username", STRING | STORED);
        let mailbox_field = schema_builder.add_text_field("mailbox", STRING | STORED);
        let from_field = schema_builder.add_text_field("from", TEXT | STORED);
        let to_field = schema_builder.add_text_field("to", TEXT | STORED);
        let subject_field = schema_builder.add_text_field("subject", TEXT | STORED);
        let body_field = schema_builder.add_text_field("body", TEXT);
        let flags_field = schema_builder.add_text_field("flags", STRING | STORED);
        let path_field = schema_builder.add_text_field("path", STRING | STORED);

        let schema = schema_builder.build();

        // Create or open index
        let index_path_std = std::path::PathBuf::from(index_path.as_os_str());
        let index = if index_path_std.exists() {
            TantivyIndex::open_in_dir(&index_path_std)
                .map_err(|e| Error::Index(format!("Failed to open index: {}", e)))?
        } else {
            std::fs::create_dir_all(&index_path_std)
                .map_err(|e| Error::Index(format!("Failed to create index directory: {}", e)))?;
            TantivyIndex::create_in_dir(&index_path_std, schema.clone())
                .map_err(|e| Error::Index(format!("Failed to create index: {}", e)))?
        };

        let index = StdArc::new(index);

        // Create reader
        let reader = index
            .reader()
            .map_err(|e| Error::Index(format!("Failed to create reader: {}", e)))?;

        let (write_tx, write_rx) = bounded(100);

        // Spawn writer loop
        let index_clone = StdArc::clone(&index);
        task::spawn(writer_loop(write_rx, index_clone));

        Ok(Self {
            schema,
            index,
            reader,
            write_tx,
            message_id_field,
            uid_field,
            username_field,
            mailbox_field,
            from_field,
            to_field,
            subject_field,
            body_field,
            flags_field,
            path_field,
        })
    }

    fn make_message_path(username: &str, mailbox: &str, message_id: &MessageId) -> String {
        format!("{}/{}/{}.eml", username, mailbox, message_id.0)
    }
}

async fn writer_loop(rx: Receiver<WriteCommand>, index: StdArc<TantivyIndex>) {
    // Mailbox state stored separately (not in Tantivy)
    let mut mailboxes: HashMap<String, MailboxState> = HashMap::new();

    let mut writer = index
        .writer(50_000_000)
        .expect("Failed to create index writer");

    while let Ok(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::CreateMailbox(username, name, reply) => {
                let key = format!("{}:{}", username, name);
                if mailboxes.contains_key(&key) {
                    let _ = reply.send(Err(Error::AlreadyExists(format!("Mailbox {} already exists", name))));
                } else {
                    mailboxes.insert(
                        key,
                        MailboxState {
                            info: Mailbox {
                                name: name.clone(),
                                uid_validity: 1,
                                uid_next: 1,
                                flags: vec![],
                                permanent_flags: vec![
                                    MessageFlag::Seen,
                                    MessageFlag::Answered,
                                    MessageFlag::Flagged,
                                    MessageFlag::Deleted,
                                    MessageFlag::Draft,
                                ],
                                message_count: 0,
                                recent_count: 0,
                                unseen_count: 0,
                            },
                            uid_counter: 1,
                        },
                    );
                    let _ = reply.send(Ok(()));
                }
            }
            WriteCommand::DeleteMailbox(username, name, reply) => {
                let key = format!("{}:{}", username, name);
                if mailboxes.remove(&key).is_some() {
                    // TODO: Delete all documents for this mailbox from Tantivy
                    let _ = reply.send(Ok(()));
                } else {
                    let _ = reply.send(Err(Error::NotFound(format!("Mailbox {} not found", name))));
                }
            }
            WriteCommand::RenameMailbox(username, old_name, new_name, reply) => {
                let old_key = format!("{}:{}", username, old_name);
                let new_key = format!("{}:{}", username, new_name);

                if let Some(mut state) = mailboxes.remove(&old_key) {
                    state.info.name = new_name.clone();
                    mailboxes.insert(new_key, state);
                    // TODO: Update documents in Tantivy
                    let _ = reply.send(Ok(()));
                } else {
                    let _ = reply.send(Err(Error::NotFound(format!("Mailbox {} not found", old_name))));
                }
            }
            WriteCommand::AddMessage(username, mailbox, message, reply) => {
                let key = format!("{}:{}", username, mailbox);

                if !mailboxes.contains_key(&key) {
                    let _ = reply.send(Err(Error::NotFound(format!("Mailbox {} not found", mailbox))));
                    continue;
                }

                let path = TantivyBackedIndex::make_message_path(&username, &mailbox, &message.id);

                // Create Tantivy document
                let flags_str = message.flags.iter()
                    .map(|f| format!("{:?}", f))
                    .collect::<Vec<_>>()
                    .join(",");

                let schema = index.schema();
                let message_id_field = schema.get_field("message_id").unwrap();
                let uid_field = schema.get_field("uid").unwrap();
                let username_field = schema.get_field("username").unwrap();
                let mailbox_field = schema.get_field("mailbox").unwrap();
                let from_field = schema.get_field("from").unwrap();
                let to_field = schema.get_field("to").unwrap();
                let subject_field = schema.get_field("subject").unwrap();
                let body_field = schema.get_field("body").unwrap();
                let flags_field = schema.get_field("flags").unwrap();
                let path_field = schema.get_field("path").unwrap();

                let doc = doc!(
                    message_id_field => message.id.0.to_string(),
                    uid_field => message.uid as u64,
                    username_field => username.clone(),
                    mailbox_field => mailbox.clone(),
                    from_field => message.from.clone(),
                    to_field => message.to.clone(),
                    subject_field => message.subject.clone(),
                    body_field => message.body_preview.clone(),
                    flags_field => flags_str,
                    path_field => path.clone(),
                );

                if let Err(e) = writer.add_document(doc) {
                    let _ = reply.send(Err(Error::Index(format!("Failed to add document: {}", e))));
                    continue;
                }

                if let Err(e) = writer.commit() {
                    let _ = reply.send(Err(Error::Index(format!("Failed to commit: {}", e))));
                    continue;
                }

                // Update mailbox counts
                if let Some(state) = mailboxes.get_mut(&key) {
                    state.info.message_count += 1;
                    if !message.flags.contains(&MessageFlag::Seen) {
                        state.info.unseen_count += 1;
                    }
                }

                let _ = reply.send(Ok(path));
            }
            WriteCommand::UpdateFlags(_id, _flags, reply) => {
                // TODO: Implement flag updates in Tantivy
                let _ = reply.send(Ok(()));
            }
            WriteCommand::DeleteMessage(_id, reply) => {
                // TODO: Implement message deletion in Tantivy
                let _ = reply.send(Ok(()));
            }
            WriteCommand::GetNextUid(username, mailbox, reply) => {
                let key = format!("{}:{}", username, mailbox);
                if let Some(state) = mailboxes.get_mut(&key) {
                    let uid = state.uid_counter;
                    state.uid_counter += 1;
                    let _ = reply.send(Ok(uid));
                } else {
                    let _ = reply.send(Err(Error::NotFound(format!("Mailbox {} not found", mailbox))));
                }
            }
            WriteCommand::Shutdown(reply) => {
                let _ = reply.send(());
                break;
            }
        }
    }
}

#[async_trait]
impl Index for TantivyBackedIndex {
    async fn create_mailbox(&self, username: &str, name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::CreateMailbox(username.to_string(), name.to_string(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn delete_mailbox(&self, username: &str, name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::DeleteMailbox(username.to_string(), name.to_string(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn rename_mailbox(&self, username: &str, old_name: &str, new_name: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::RenameMailbox(
                username.to_string(),
                old_name.to_string(),
                new_name.to_string(),
                tx,
            ))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn list_mailboxes(&self, _username: &str) -> Result<Vec<Mailbox>> {
        // TODO: Implement mailbox listing from in-memory state or separate store
        Ok(vec![])
    }

    async fn get_mailbox(&self, _username: &str, _name: &str) -> Result<Option<Mailbox>> {
        // TODO: Implement mailbox retrieval from in-memory state or separate store
        Ok(None)
    }

    async fn add_message(&self, username: &str, mailbox: &str, message: IndexedMessage) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::AddMessage(username.to_string(), mailbox.to_string(), message, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn get_message_path(&self, id: MessageId) -> Result<Option<String>> {
        // Query Tantivy for the message
        let searcher = self.reader.searcher();
        let query = tantivy::query::TermQuery::new(
            tantivy::Term::from_field_text(self.message_id_field, &id.0.to_string()),
            tantivy::schema::IndexRecordOption::Basic,
        );

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| Error::Index(format!("Search failed: {}", e)))?;

        if let Some((_score, doc_address)) = top_docs.first() {
            let doc: TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| Error::Index(format!("Failed to retrieve document: {}", e)))?;

            if let Some(path_value) = doc.get_first(self.path_field) {
                if let Some(path) = path_value.as_str() {
                    return Ok(Some(path.to_string()));
                }
            }
        }

        Ok(None)
    }

    async fn get_message_path_by_uid(&self, username: &str, mailbox: &str, uid: Uid) -> Result<Option<String>> {
        // Build a query for username AND mailbox AND uid
        let searcher = self.reader.searcher();

        let username_term = tantivy::Term::from_field_text(self.username_field, username);
        let mailbox_term = tantivy::Term::from_field_text(self.mailbox_field, mailbox);
        let uid_term = tantivy::Term::from_field_u64(self.uid_field, uid as u64);

        let username_query = tantivy::query::TermQuery::new(username_term, tantivy::schema::IndexRecordOption::Basic);
        let mailbox_query = tantivy::query::TermQuery::new(mailbox_term, tantivy::schema::IndexRecordOption::Basic);
        let uid_query = tantivy::query::TermQuery::new(uid_term, tantivy::schema::IndexRecordOption::Basic);

        let query = tantivy::query::BooleanQuery::new(vec![
            (tantivy::query::Occur::Must, Box::new(username_query) as Box<dyn tantivy::query::Query>),
            (tantivy::query::Occur::Must, Box::new(mailbox_query) as Box<dyn tantivy::query::Query>),
            (tantivy::query::Occur::Must, Box::new(uid_query) as Box<dyn tantivy::query::Query>),
        ]);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(1))
            .map_err(|e| Error::Index(format!("Search failed: {}", e)))?;

        if let Some((_score, doc_address)) = top_docs.first() {
            let doc: TantivyDocument = searcher
                .doc(*doc_address)
                .map_err(|e| Error::Index(format!("Failed to retrieve document: {}", e)))?;

            if let Some(path_value) = doc.get_first(self.path_field) {
                if let Some(path) = path_value.as_str() {
                    return Ok(Some(path.to_string()));
                }
            }
        }

        Ok(None)
    }

    async fn list_message_paths(&self, username: &str, mailbox: &str) -> Result<Vec<String>> {
        let searcher = self.reader.searcher();

        let username_term = tantivy::Term::from_field_text(self.username_field, username);
        let mailbox_term = tantivy::Term::from_field_text(self.mailbox_field, mailbox);

        let username_query = tantivy::query::TermQuery::new(username_term, tantivy::schema::IndexRecordOption::Basic);
        let mailbox_query = tantivy::query::TermQuery::new(mailbox_term, tantivy::schema::IndexRecordOption::Basic);

        let query = tantivy::query::BooleanQuery::new(vec![
            (tantivy::query::Occur::Must, Box::new(username_query) as Box<dyn tantivy::query::Query>),
            (tantivy::query::Occur::Must, Box::new(mailbox_query) as Box<dyn tantivy::query::Query>),
        ]);

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(10000))
            .map_err(|e| Error::Index(format!("Search failed: {}", e)))?;

        let mut paths = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| Error::Index(format!("Failed to retrieve document: {}", e)))?;

            if let Some(path_value) = doc.get_first(self.path_field) {
                if let Some(path) = path_value.as_str() {
                    paths.push(path.to_string());
                }
            }
        }

        Ok(paths)
    }

    async fn get_message_metadata(&self, _id: MessageId) -> Result<Option<IndexedMessage>> {
        // TODO: Implement metadata retrieval from Tantivy
        Ok(None)
    }

    async fn update_flags(&self, id: MessageId, flags: Vec<MessageFlag>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::UpdateFlags(id, flags, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn delete_message(&self, id: MessageId) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::DeleteMessage(id, tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn search(&self, username: &str, mailbox: &str, query: &SearchQuery) -> Result<Vec<String>> {
        let searcher = self.reader.searcher();

        // Build query based on SearchQuery
        let search_query: Box<dyn tantivy::query::Query> = match query {
            SearchQuery::All => {
                // Match all documents in this mailbox
                let username_term = tantivy::Term::from_field_text(self.username_field, username);
                let mailbox_term = tantivy::Term::from_field_text(self.mailbox_field, mailbox);

                Box::new(tantivy::query::BooleanQuery::new(vec![
                    (tantivy::query::Occur::Must, Box::new(tantivy::query::TermQuery::new(username_term, tantivy::schema::IndexRecordOption::Basic)) as Box<dyn tantivy::query::Query>),
                    (tantivy::query::Occur::Must, Box::new(tantivy::query::TermQuery::new(mailbox_term, tantivy::schema::IndexRecordOption::Basic)) as Box<dyn tantivy::query::Query>),
                ]))
            }
            SearchQuery::From(text) => {
                let parser = QueryParser::for_index(&self.index, vec![self.from_field]);
                Box::new(parser.parse_query(text).map_err(|e| Error::Index(format!("Query parse error: {}", e)))?)
            }
            SearchQuery::Subject(text) => {
                let parser = QueryParser::for_index(&self.index, vec![self.subject_field]);
                Box::new(parser.parse_query(text).map_err(|e| Error::Index(format!("Query parse error: {}", e)))?)
            }
            SearchQuery::Text(text) => {
                let parser = QueryParser::for_index(&self.index, vec![self.from_field, self.to_field, self.subject_field, self.body_field]);
                Box::new(parser.parse_query(text).map_err(|e| Error::Index(format!("Query parse error: {}", e)))?)
            }
            _ => {
                // For other query types, return empty for now
                return Ok(vec![]);
            }
        };

        let top_docs = searcher
            .search(&*search_query, &TopDocs::with_limit(1000))
            .map_err(|e| Error::Index(format!("Search failed: {}", e)))?;

        let mut paths = Vec::new();
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| Error::Index(format!("Failed to retrieve document: {}", e)))?;

            // Verify this doc is in the right mailbox/username
            if let (Some(doc_username), Some(doc_mailbox)) = (
                doc.get_first(self.username_field).and_then(|v| v.as_str()),
                doc.get_first(self.mailbox_field).and_then(|v| v.as_str()),
            ) {
                if doc_username == username && doc_mailbox == mailbox {
                    if let Some(path_value) = doc.get_first(self.path_field) {
                        if let Some(path) = path_value.as_str() {
                            paths.push(path.to_string());
                        }
                    }
                }
            }
        }

        Ok(paths)
    }

    async fn get_next_uid(&self, username: &str, mailbox: &str) -> Result<Uid> {
        let (tx, rx) = oneshot::channel();
        self.write_tx
            .send(WriteCommand::GetNextUid(username.to_string(), mailbox.to_string(), tx))
            .await
            .map_err(|_| Error::Internal("Writer loop stopped".to_string()))?;
        rx.await
            .map_err(|_| Error::Internal("Writer loop dropped reply".to_string()))?
    }

    async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let _ = self.write_tx.send(WriteCommand::Shutdown(tx)).await;
        let _ = rx.await;
        Ok(())
    }
}
