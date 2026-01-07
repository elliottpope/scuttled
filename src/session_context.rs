//! Session context for command handlers

use crate::index::Indexer;
use crate::types::*;
use crate::{Authenticator, MailStore, Mailboxes, Queue, UserStore};
use async_std::sync::{Arc, RwLock};

/// Session state
#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    NotAuthenticated,
    Authenticated {
        username: Username,
    },
    Selected {
        username: Username,
        mailbox: MailboxName,
    },
    Logout,
}

/// Context provided to command handlers
///
/// Contains references to all server stores and the current session state
pub struct SessionContext {
    pub mail_store: Arc<dyn MailStore>,
    pub index: Indexer,
    pub authenticator: Arc<dyn Authenticator>,
    pub user_store: Arc<dyn UserStore>,
    pub queue: Arc<dyn Queue>,
    pub mailboxes: Arc<dyn Mailboxes>,
    pub state: Arc<RwLock<SessionState>>,
    pub selected_mailbox: Arc<RwLock<Option<MailboxName>>>,
}

impl SessionContext {
    pub fn new<M, A, U, Q, B>(
        mail_store: Arc<M>,
        index: Indexer,
        authenticator: Arc<A>,
        user_store: Arc<U>,
        queue: Arc<Q>,
        mailboxes: Arc<B>,
    ) -> Self
    where
        M: MailStore + 'static,
        A: Authenticator + 'static,
        U: UserStore + 'static,
        Q: Queue + 'static,
        B: Mailboxes + 'static,
    {
        Self {
            mail_store,
            index,
            authenticator,
            user_store,
            queue,
            mailboxes,
            state: Arc::new(RwLock::new(SessionState::NotAuthenticated)),
            selected_mailbox: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the current session state
    pub async fn get_state(&self) -> SessionState {
        self.state.read().await.clone()
    }

    /// Set the session state
    pub async fn set_state(&self, state: SessionState) {
        *self.state.write().await = state;
    }

    /// Get the currently selected mailbox
    pub async fn get_selected_mailbox(&self) -> Option<MailboxName> {
        self.selected_mailbox.read().await.clone()
    }

    /// Set the currently selected mailbox
    pub async fn set_selected_mailbox(&self, mailbox: Option<MailboxName>) {
        *self.selected_mailbox.write().await = mailbox;
    }

    /// Get the authenticated username, if any
    pub async fn get_username(&self) -> Option<Username> {
        match self.get_state().await {
            SessionState::Authenticated { username } => Some(username),
            SessionState::Selected { username, .. } => Some(username),
            _ => None,
        }
    }

    /// Check if the session is authenticated
    pub async fn is_authenticated(&self) -> bool {
        !matches!(self.get_state().await, SessionState::NotAuthenticated)
    }

    /// Check if a mailbox is selected
    pub async fn has_selected_mailbox(&self) -> bool {
        self.get_selected_mailbox().await.is_some()
    }
}
