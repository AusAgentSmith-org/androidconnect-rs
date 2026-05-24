pub mod activity;
pub mod files;
pub mod messages;
pub mod mirror;
pub mod notifications;
pub mod overview;
pub mod pair;
pub mod phone;

pub use activity::{ActivityIcon, ActivityLog};
pub use messages::MessagesState;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Panel {
    Overview,
    Mirror,
    Notifications,
    Messages,
    Files,
    Phone,
}

impl Panel {
    pub fn label(self) -> &'static str {
        match self {
            Panel::Overview => "Overview",
            Panel::Mirror => "Mirror",
            Panel::Notifications => "Notifications",
            Panel::Messages => "Messages",
            Panel::Files => "Files",
            Panel::Phone => "Phone",
        }
    }

    /// Icon name (from the FluentGUI icon registry) for the sidebar.
    pub fn icon(self) -> &'static str {
        match self {
            Panel::Overview => "home",
            Panel::Mirror => "mirror",
            Panel::Notifications => "bell",
            Panel::Messages => "chat",
            Panel::Files => "folder",
            Panel::Phone => "phone",
        }
    }

    pub const ALL: [Panel; 6] = [
        Panel::Overview,
        Panel::Mirror,
        Panel::Notifications,
        Panel::Messages,
        Panel::Files,
        Panel::Phone,
    ];
}
