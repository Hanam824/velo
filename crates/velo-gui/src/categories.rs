//! Rows of the Categories pane.

use std::collections::HashSet;

use velo_core::{Category, Queue};

use crate::{CategoryIcon, CategoryItem};

/// The Categories tree: All Downloads, Unfinished and Finished, each with the categories under
/// it, then Queues with the main queue.
///
/// Keys are `all`, `unfinished` and `finished`, followed by `/<category name>` for their
/// children, plus `queues` and `queue/<id>`. Unfinished and Finished start collapsed, as in IDM.
#[derive(Debug, Clone)]
pub struct CategoryTree {
    /// Every row, collapsed or not, in display order.
    rows: Vec<CategoryItem>,
    /// Keys of the top-level rows whose children are hidden.
    collapsed: HashSet<String>,
}

impl CategoryTree {
    /// Builds the tree for `categories` and `queues`.
    pub fn new(categories: &[Category], queues: &[Queue]) -> Self {
        let mut rows = Vec::new();
        for (key, label, icon) in [
            ("all", "All Downloads", CategoryIcon::All),
            ("unfinished", "Unfinished", CategoryIcon::Unfinished),
            ("finished", "Finished", CategoryIcon::Finished),
        ] {
            rows.push(row(label, icon, 0, key.into(), !categories.is_empty()));
            for category in categories {
                let key = format!("{key}/{}", category.name);
                rows.push(row(&category.name, CategoryIcon::Folder, 1, key, false));
            }
        }
        rows.push(row(
            "Queues",
            CategoryIcon::Queues,
            0,
            "queues".into(),
            !queues.is_empty(),
        ));
        for queue in queues {
            let key = format!("queue/{}", queue.id.0);
            rows.push(row(&queue.name, CategoryIcon::Queue, 1, key, false));
        }
        Self {
            rows,
            collapsed: HashSet::from(["unfinished".into(), "finished".into()]),
        }
    }

    /// Rebuilds the rows for `categories` and `queues`, keeping which groups are collapsed.
    pub fn rebuild(&mut self, categories: &[Category], queues: &[Queue]) {
        let collapsed = std::mem::take(&mut self.collapsed);
        *self = Self::new(categories, queues);
        self.collapsed = collapsed;
    }

    /// Whether a row has `key`.
    pub fn contains(&self, key: &str) -> bool {
        self.rows.iter().any(|row| row.key == key)
    }

    /// Expands the row with `key` if it is collapsed, and collapses it otherwise.
    pub fn toggle(&mut self, key: &str) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_string());
        }
    }

    /// The rows to show: every top-level row, and the children of expanded ones.
    pub fn visible(&self) -> Vec<CategoryItem> {
        let mut visible = Vec::new();
        let mut hidden = false;
        for row in &self.rows {
            if row.depth == 0 {
                hidden = self.collapsed.contains(row.key.as_str());
                visible.push(CategoryItem {
                    expanded: !hidden,
                    ..row.clone()
                });
            } else if !hidden {
                visible.push(row.clone());
            }
        }
        visible
    }
}

/// One row of the tree.
fn row(
    label: &str,
    icon: CategoryIcon,
    depth: i32,
    key: String,
    has_children: bool,
) -> CategoryItem {
    CategoryItem {
        label: label.into(),
        icon,
        depth,
        key: key.into(),
        has_children,
        expanded: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(items: &[CategoryItem]) -> Vec<&str> {
        items.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn unfinished_and_finished_start_collapsed() {
        let tree = CategoryTree::new(&Category::defaults(), &[Queue::main()]);
        let visible = tree.visible();
        assert_eq!(
            keys(&visible),
            [
                "all",
                "all/Compressed",
                "all/Documents",
                "all/Music",
                "all/Programs",
                "all/Video",
                "unfinished",
                "finished",
                "queues",
                "queue/0",
            ]
        );
        assert!(visible[0].expanded && visible[0].has_children);
        assert!(!visible[6].expanded);
        assert_eq!(visible.last().unwrap().label, "Main download queue");
    }

    #[test]
    fn toggling_shows_and_hides_children() {
        let mut tree = CategoryTree::new(&Category::defaults(), &[Queue::main()]);
        tree.toggle("finished");
        tree.toggle("all");
        let visible = tree.visible();
        let keys = keys(&visible);
        assert!(keys.contains(&"finished/Music"));
        assert!(!keys.contains(&"all/Music"));
        assert!(!keys.contains(&"unfinished/Music"));
    }
}
