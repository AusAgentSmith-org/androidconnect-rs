use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{
    FeatureStatus, MessageDirection, MessageEntry, MessageEvent, MessageSendRequest,
    MessageSendResponse, MessageSendResult, MessageThreadDetail, MessageThreadList,
    MessageThreadOpen, MessageThreadSummary, Payload,
};

#[derive(Debug, Default, Clone)]
pub struct MessagesState {
    pub threads: Vec<MessageThreadSummary>,
    pub thread_status: Option<FeatureStatus>,
    pub active_thread: Option<String>,
    pub thread_messages: HashMap<String, Vec<MessageEntry>>,
    pub thread_detail_status: HashMap<String, FeatureStatus>,
    pub pending_thread_open: HashMap<String, String>,
    pub pending_send: Option<PendingSend>,
    pub send_error: Option<String>,
    pub last_send_result: Option<MessageSendResult>,
}

#[derive(Debug, Clone)]
pub struct PendingSend {
    pub request_id: String,
    #[allow(dead_code)]
    pub thread_id: Option<String>,
}

impl MessagesState {
    pub fn apply_thread_list(&mut self, list: MessageThreadList) {
        self.threads = list.threads;
        self.thread_status = Some(list.status);
    }

    pub fn apply_event(&mut self, event: MessageEvent) {
        let thread_id = event.thread_id.clone();
        let entries = self.thread_messages.entry(thread_id.clone()).or_default();
        let entry = MessageEntry {
            message_id: format!("evt-{}-{}", thread_id, event.timestamp_unix_ms),
            thread_id: thread_id.clone(),
            sender: event.sender,
            body: event.body,
            timestamp_unix_ms: event.timestamp_unix_ms,
            direction: MessageDirection::Inbound,
            attachments: event.attachments,
        };
        if !entries
            .iter()
            .any(|e| e.timestamp_unix_ms == entry.timestamp_unix_ms && e.body == entry.body)
        {
            entries.push(entry);
            entries.sort_by_key(|e| e.timestamp_unix_ms);
        }
    }

    pub fn apply_thread_detail(&mut self, detail: MessageThreadDetail) {
        if self.pending_thread_open.get(&detail.thread_id) == Some(&detail.request_id) {
            self.pending_thread_open.remove(&detail.thread_id);
        }
        let mut messages = detail.messages;
        messages.sort_by_key(|m| m.timestamp_unix_ms);
        self.thread_messages
            .insert(detail.thread_id.clone(), messages);
        self.thread_detail_status
            .insert(detail.thread_id, detail.status);
    }

    pub fn apply_send_response(&mut self, resp: MessageSendResponse) {
        let matches_pending = self
            .pending_send
            .as_ref()
            .is_some_and(|p| p.request_id == resp.request_id);
        if matches_pending {
            self.pending_send = None;
        }
        self.last_send_result = Some(resp.result);
        match resp.result {
            MessageSendResult::Queued | MessageSendResult::Sent => {
                self.send_error = None;
            }
            MessageSendResult::Failed => {
                self.send_error = Some(
                    resp.message
                        .unwrap_or_else(|| "Failed to send message.".to_owned()),
                );
            }
            MessageSendResult::PermissionDenied => {
                self.send_error =
                    Some("SMS permission denied — grant Send SMS access in onboarding.".to_owned());
            }
        }
    }

    pub fn open_thread(&mut self, thread_id: String) -> Option<Payload> {
        self.active_thread = Some(thread_id.clone());
        // Clear unread count locally so the badge disappears immediately on open.
        if let Some(thread) = self.threads.iter_mut().find(|t| t.thread_id == thread_id) {
            thread.unread_count = 0;
        }
        if self.pending_thread_open.contains_key(&thread_id) {
            return None;
        }
        let request_id = format!("thread-open-{}", new_request_token());
        self.pending_thread_open
            .insert(thread_id.clone(), request_id.clone());
        Some(Payload::MessageThreadOpen(MessageThreadOpen {
            request_id,
            thread_id,
            limit: 100,
        }))
    }

    pub fn close_thread(&mut self) {
        self.active_thread = None;
    }

    /// Build a `MessageSendRequest` for the active thread. Returns None if there is no active
    /// thread or the body is empty.
    pub fn build_send(&mut self, body: String) -> Option<Payload> {
        let body = body.trim().to_owned();
        if body.is_empty() {
            return None;
        }
        let thread_id = self.active_thread.clone()?;
        let request_id = format!("msg-send-{}", new_request_token());
        self.pending_send = Some(PendingSend {
            request_id: request_id.clone(),
            thread_id: Some(thread_id.clone()),
        });
        self.send_error = None;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        // Optimistically append an outbound entry so the user sees their message right away.
        // It will be reconciled when the Android side echoes a MessageEvent or thread detail.
        let entries = self.thread_messages.entry(thread_id.clone()).or_default();
        entries.push(MessageEntry {
            message_id: format!("pending-{request_id}"),
            thread_id: thread_id.clone(),
            sender: "me".to_owned(),
            body: body.clone(),
            timestamp_unix_ms: now_ms,
            direction: MessageDirection::Outbound,
            attachments: Vec::new(),
        });
        Some(Payload::MessageSendRequest(MessageSendRequest {
            request_id,
            thread_id: Some(thread_id),
            recipients: Vec::new(),
            body,
            attachments: Vec::new(),
        }))
    }
}

fn new_request_token() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    format!("{micros}-{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_to_existing_thread_uses_thread_lookup_not_display_name() {
        let mut state = MessagesState {
            threads: vec![MessageThreadSummary {
                thread_id: "42".to_owned(),
                display_name: "Ada Lovelace".to_owned(),
                last_message: None,
                timestamp_unix_ms: None,
                unread_count: 0,
            }],
            active_thread: Some("42".to_owned()),
            ..MessagesState::default()
        };

        let Some(Payload::MessageSendRequest(request)) = state.build_send("hello".to_owned())
        else {
            panic!("expected message send request");
        };

        assert_eq!(request.thread_id.as_deref(), Some("42"));
        assert!(request.recipients.is_empty());
    }
}
