use std::collections::VecDeque;
use std::time::SystemTime;

/// Icon category for an `ActivityEvent` — drives the small left glyph in the
/// Overview activity card.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Upload + Notification are reserved for future event sources.
pub enum ActivityIcon {
    Download,
    Upload,
    Send,
    Pair,
    Notification,
}

impl ActivityIcon {
    pub fn icon_name(self) -> &'static str {
        match self {
            ActivityIcon::Download => "download",
            ActivityIcon::Upload => "upload",
            ActivityIcon::Send => "send",
            ActivityIcon::Pair => "qr",
            ActivityIcon::Notification => "bell",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ActivityEvent {
    pub icon: ActivityIcon,
    pub text: String,
    pub timestamp: SystemTime,
}

/// Append-only ring buffer of recent user-visible events. Newest first.
#[derive(Debug, Default)]
pub struct ActivityLog {
    events: VecDeque<ActivityEvent>,
}

impl ActivityLog {
    const CAPACITY: usize = 50;

    pub fn push(&mut self, icon: ActivityIcon, text: impl Into<String>) {
        if self.events.len() >= Self::CAPACITY {
            self.events.pop_back();
        }
        self.events.push_front(ActivityEvent {
            icon,
            text: text.into(),
            timestamp: SystemTime::now(),
        });
    }

    pub fn iter(&self) -> impl Iterator<Item = &ActivityEvent> {
        self.events.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}
