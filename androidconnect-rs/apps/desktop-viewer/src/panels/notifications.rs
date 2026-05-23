use std::collections::{HashMap, HashSet};

use androidconnect_protocol::{NotificationPosted, NotificationRemoved};

#[derive(Debug, Default)]
pub struct NotificationsState {
    items: HashMap<String, NotificationPosted>,
    order: Vec<String>,
    suppressed_packages: HashSet<String>,
    pub hide_sensitive: bool,
}

impl NotificationsState {
    pub fn posted(&mut self, n: NotificationPosted) {
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
}
