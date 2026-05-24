use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use androidconnect_protocol::{NotificationPosted, NotificationRemoved};

#[derive(Debug, Default)]
pub struct NotificationsState {
    items: HashMap<String, NotificationPosted>,
    order: Vec<String>,
    suppressed_packages: HashSet<String>,
    pub hide_sensitive: bool,
    initial_load_started: Option<Instant>,
    initial_load_complete: bool,
}

impl NotificationsState {
    pub fn posted(&mut self, n: NotificationPosted) {
        self.initial_load_complete = true;
        if let Some(existing) = self.items.get_mut(&n.notification_id) {
            *existing = n;
        } else {
            self.order.push(n.notification_id.clone());
            self.items.insert(n.notification_id.clone(), n);
        }
    }

    pub fn removed(&mut self, r: NotificationRemoved) {
        self.items.remove(&r.notification_id);
        self.order.retain(|id| id != &r.notification_id);
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.initial_load_started = None;
        self.initial_load_complete = false;
    }

    pub fn toggle_suppress(&mut self, package: &str) {
        if !self.suppressed_packages.remove(package) {
            self.suppressed_packages.insert(package.to_owned());
        }
    }

    pub fn is_suppressed(&self, package: &str) -> bool {
        self.suppressed_packages.contains(package)
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn iter_items(&self) -> impl Iterator<Item = (&String, &NotificationPosted)> {
        self.order
            .iter()
            .filter_map(|id| self.items.get(id).map(|n| (id, n)))
    }

    pub fn show_initial_skeleton(&mut self) -> bool {
        if !self.items.is_empty() {
            self.initial_load_complete = true;
            return false;
        }
        if self.initial_load_complete {
            return false;
        }
        let started = *self.initial_load_started.get_or_insert_with(Instant::now);
        if started.elapsed() < Duration::from_secs(3) {
            true
        } else {
            self.initial_load_complete = true;
            false
        }
    }

    pub fn is_initial_loading(&self) -> bool {
        self.items.is_empty() && !self.initial_load_complete
    }
}
