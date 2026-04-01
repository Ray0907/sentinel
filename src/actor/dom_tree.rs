//! Live DOM tree backed by slotmap for stable keys.
//! Incrementally updated via CDP DOM events.

use slotmap::{new_key_type, SlotMap};
use std::collections::HashMap;

use crate::cdp::types::DomNode;

new_key_type! {
    pub struct TreeKey;
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub node_id: i64,
    pub backend_node_id: i64,
    pub node_type: i32,
    pub node_name: String,
    pub node_value: String,
    pub attributes: HashMap<String, String>,
    pub parent: Option<TreeKey>,
    pub children: Vec<TreeKey>,
    pub shadow_roots: Vec<TreeKey>,
    pub pseudo_elements: Vec<TreeKey>,
    pub frame_id: Option<String>,
    pub child_node_count: Option<i32>,
}

/// Live DOM tree that is incrementally updated via CDP events.
pub struct LiveDomTree {
    nodes: SlotMap<TreeKey, TreeNode>,
    /// node_id → TreeKey mapping for O(1) lookup
    id_map: HashMap<i64, TreeKey>,
    /// backend_node_id → TreeKey for cross-domain correlation
    backend_map: HashMap<i64, TreeKey>,
    root: Option<TreeKey>,
}

impl Default for LiveDomTree {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveDomTree {
    pub fn new() -> Self {
        Self {
            nodes: SlotMap::with_key(),
            id_map: HashMap::new(),
            backend_map: HashMap::new(),
            root: None,
        }
    }

    /// Clear the entire tree (on documentUpdated).
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.id_map.clear();
        self.backend_map.clear();
        self.root = None;
    }

    /// Set the root document node from DOM.getDocument response.
    pub fn set_root(&mut self, node: DomNode) {
        self.clear();
        let key = self.insert_node_recursive(node, None);
        self.root = Some(key);
    }

    /// Set children for a node (from DOM.setChildNodes event).
    pub fn set_children(&mut self, parent_id: i64, children: Vec<DomNode>) {
        if let Some(&parent_key) = self.id_map.get(&parent_id) {
            // Remove old children
            if let Some(parent) = self.nodes.get(parent_key) {
                let old_children: Vec<TreeKey> = parent.children.clone();
                for child_key in old_children {
                    self.remove_subtree(child_key);
                }
            }

            // Insert new children
            let mut child_keys = Vec::with_capacity(children.len());
            for child in children {
                let key = self.insert_node_recursive(child, Some(parent_key));
                child_keys.push(key);
            }

            if let Some(parent) = self.nodes.get_mut(parent_key) {
                parent.children = child_keys;
            }
        }
    }

    /// Insert a child node (from DOM.childNodeInserted event).
    pub fn insert_child(&mut self, parent_node_id: i64, previous_node_id: i64, node: DomNode) {
        if let Some(&parent_key) = self.id_map.get(&parent_node_id) {
            let new_key = self.insert_node_recursive(node, Some(parent_key));

            if let Some(parent) = self.nodes.get_mut(parent_key) {
                if previous_node_id == 0 {
                    // Insert at the beginning
                    parent.children.insert(0, new_key);
                } else if let Some(&prev_key) = self.id_map.get(&previous_node_id) {
                    // Insert after the previous node
                    if let Some(pos) = parent.children.iter().position(|k| *k == prev_key) {
                        parent.children.insert(pos + 1, new_key);
                    } else {
                        parent.children.push(new_key);
                    }
                } else {
                    parent.children.push(new_key);
                }
            }
        }
    }

    /// Remove a child node (from DOM.childNodeRemoved event).
    pub fn remove_child(&mut self, parent_node_id: i64, node_id: i64) {
        if let Some(&node_key) = self.id_map.get(&node_id) {
            if let Some(&parent_key) = self.id_map.get(&parent_node_id) {
                if let Some(parent) = self.nodes.get_mut(parent_key) {
                    parent.children.retain(|k| *k != node_key);
                }
            }
            self.remove_subtree(node_key);
        }
    }

    /// Set an attribute on a node.
    pub fn set_attribute(&mut self, node_id: i64, name: &str, value: &str) {
        if let Some(&key) = self.id_map.get(&node_id) {
            if let Some(node) = self.nodes.get_mut(key) {
                node.attributes.insert(name.to_string(), value.to_string());
            }
        }
    }

    /// Remove an attribute from a node.
    pub fn remove_attribute(&mut self, node_id: i64, name: &str) {
        if let Some(&key) = self.id_map.get(&node_id) {
            if let Some(node) = self.nodes.get_mut(key) {
                node.attributes.remove(name);
            }
        }
    }

    /// Set character data for a text node.
    pub fn set_character_data(&mut self, node_id: i64, data: &str) {
        if let Some(&key) = self.id_map.get(&node_id) {
            if let Some(node) = self.nodes.get_mut(key) {
                node.node_value = data.to_string();
            }
        }
    }

    /// Update child count (from DOM.childNodeCountUpdated).
    pub fn update_child_count(&mut self, node_id: i64, count: i32) {
        if let Some(&key) = self.id_map.get(&node_id) {
            if let Some(node) = self.nodes.get_mut(key) {
                node.child_node_count = Some(count);
            }
        }
    }

    /// Add a shadow root to a host element.
    pub fn add_shadow_root(&mut self, host_id: i64, root: DomNode) {
        if let Some(&host_key) = self.id_map.get(&host_id) {
            let root_key = self.insert_node_recursive(root, Some(host_key));
            if let Some(host) = self.nodes.get_mut(host_key) {
                host.shadow_roots.push(root_key);
            }
        }
    }

    /// Remove a shadow root from a host element.
    pub fn remove_shadow_root(&mut self, host_id: i64, root_node_id: i64) {
        if let Some(&root_key) = self.id_map.get(&root_node_id) {
            if let Some(&host_key) = self.id_map.get(&host_id) {
                if let Some(host) = self.nodes.get_mut(host_key) {
                    host.shadow_roots.retain(|k| *k != root_key);
                }
            }
            self.remove_subtree(root_key);
        }
    }

    /// Add a pseudo element.
    pub fn add_pseudo_element(&mut self, parent_id: i64, pseudo: DomNode) {
        if let Some(&parent_key) = self.id_map.get(&parent_id) {
            let key = self.insert_node_recursive(pseudo, Some(parent_key));
            if let Some(parent) = self.nodes.get_mut(parent_key) {
                parent.pseudo_elements.push(key);
            }
        }
    }

    /// Remove a pseudo element.
    pub fn remove_pseudo_element(&mut self, parent_id: i64, pseudo_node_id: i64) {
        if let Some(&key) = self.id_map.get(&pseudo_node_id) {
            if let Some(&parent_key) = self.id_map.get(&parent_id) {
                if let Some(parent) = self.nodes.get_mut(parent_key) {
                    parent.pseudo_elements.retain(|k| *k != key);
                }
            }
            self.remove_subtree(key);
        }
    }

    /// Get the total node count.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get a node by its CDP node_id.
    pub fn get_by_node_id(&self, node_id: i64) -> Option<&TreeNode> {
        self.id_map.get(&node_id).and_then(|k| self.nodes.get(*k))
    }

    /// Render the tree as indented text (for debugging/compatibility).
    pub fn render(&self) -> String {
        let mut output = String::new();
        if let Some(root) = self.root {
            self.render_node(root, 0, &mut output);
        }
        output
    }

    // ── Internal helpers ──

    fn insert_node_recursive(&mut self, node: DomNode, parent: Option<TreeKey>) -> TreeKey {
        let attrs = parse_attributes(&node.attributes);

        let key = self.nodes.insert(TreeNode {
            node_id: node.node_id,
            backend_node_id: node.backend_node_id,
            node_type: node.node_type,
            node_name: node.node_name.clone(),
            node_value: node.node_value.clone(),
            attributes: attrs,
            parent,
            children: Vec::new(),
            shadow_roots: Vec::new(),
            pseudo_elements: Vec::new(),
            frame_id: node.frame_id.clone(),
            child_node_count: node.child_node_count,
        });

        // M9 fix: remove any existing node with the same ID to prevent orphaned slots
        if let Some(old_key) = self.id_map.get(&node.node_id).copied() {
            if old_key != key {
                self.remove_subtree(old_key);
            }
        }
        self.id_map.insert(node.node_id, key);
        self.backend_map.insert(node.backend_node_id, key);

        // Recursively insert children
        if let Some(children) = node.children {
            let child_keys: Vec<TreeKey> = children
                .into_iter()
                .map(|c| self.insert_node_recursive(c, Some(key)))
                .collect();
            if let Some(n) = self.nodes.get_mut(key) {
                n.children = child_keys;
            }
        }

        // Recursively insert shadow roots
        if let Some(shadow_roots) = node.shadow_roots {
            let sr_keys: Vec<TreeKey> = shadow_roots
                .into_iter()
                .map(|sr| self.insert_node_recursive(sr, Some(key)))
                .collect();
            if let Some(n) = self.nodes.get_mut(key) {
                n.shadow_roots = sr_keys;
            }
        }

        // Insert content document (for iframes)
        if let Some(content_doc) = node.content_document {
            let cd_key = self.insert_node_recursive(*content_doc, Some(key));
            if let Some(n) = self.nodes.get_mut(key) {
                n.children.push(cd_key);
            }
        }

        key
    }

    fn remove_subtree(&mut self, key: TreeKey) {
        // Collect all keys to remove (BFS)
        let mut to_remove = vec![key];
        let mut i = 0;
        while i < to_remove.len() {
            let k = to_remove[i];
            if let Some(node) = self.nodes.get(k) {
                to_remove.extend(node.children.iter().copied());
                to_remove.extend(node.shadow_roots.iter().copied());
                to_remove.extend(node.pseudo_elements.iter().copied());
            }
            i += 1;
        }

        // Remove all collected nodes
        for k in to_remove {
            if let Some(node) = self.nodes.remove(k) {
                self.id_map.remove(&node.node_id);
                self.backend_map.remove(&node.backend_node_id);
            }
        }
    }

    fn render_node(&self, key: TreeKey, depth: usize, output: &mut String) {
        if let Some(node) = self.nodes.get(key) {
            let indent = "  ".repeat(depth);
            match node.node_type {
                1 => {
                    // Element
                    output.push_str(&format!("{indent}<{}>\n", node.node_name.to_lowercase()));
                }
                3 => {
                    // Text
                    let text = node.node_value.trim();
                    if !text.is_empty() {
                        output.push_str(&format!("{indent}\"{text}\"\n"));
                    }
                }
                9 | 11 => {
                    // Document or DocumentFragment
                    output.push_str(&format!("{indent}#document\n"));
                }
                _ => {}
            }

            for &child in &node.children {
                self.render_node(child, depth + 1, output);
            }
            for &sr in &node.shadow_roots {
                self.render_node(sr, depth + 1, output);
            }
        }
    }
}

/// Parse CDP attribute array [name, value, name, value, ...] into HashMap.
fn parse_attributes(attrs: &Option<Vec<String>>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Some(attrs) = attrs {
        for pair in attrs.chunks(2) {
            if pair.len() == 2 {
                map.insert(pair[0].clone(), pair[1].clone());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a minimal DomNode for testing.
    fn make_node(node_id: i64, name: &str, node_type: i32) -> DomNode {
        DomNode {
            node_id,
            parent_id: None,
            backend_node_id: node_id * 100,
            node_type,
            node_name: name.to_string(),
            local_name: Some(name.to_lowercase()),
            node_value: String::new(),
            child_node_count: None,
            children: None,
            attributes: None,
            document_url: None,
            base_url: None,
            frame_id: None,
            content_document: None,
            shadow_roots: None,
            pseudo_type: None,
            pseudo_identifier: None,
            distributed_nodes: None,
        }
    }

    fn make_element(node_id: i64, name: &str) -> DomNode {
        make_node(node_id, name, 1) // node_type 1 = Element
    }

    fn make_text(node_id: i64, value: &str) -> DomNode {
        let mut n = make_node(node_id, "#text", 3); // node_type 3 = Text
        n.node_value = value.to_string();
        n
    }

    fn make_document(node_id: i64) -> DomNode {
        make_node(node_id, "#document", 9) // node_type 9 = Document
    }

    #[test]
    fn new_tree_is_empty() {
        let tree = LiveDomTree::new();
        assert_eq!(tree.node_count(), 0);
        assert_eq!(tree.render(), "");
    }

    #[test]
    fn set_root_populates_tree() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_element(2, "HTML")]);

        tree.set_root(root);
        assert_eq!(tree.node_count(), 2);
        assert!(tree.get_by_node_id(1).is_some());
        assert!(tree.get_by_node_id(2).is_some());
    }

    #[test]
    fn clear_resets_everything() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_element(2, "HTML"), make_element(3, "HEAD")]);
        tree.set_root(root);
        assert_eq!(tree.node_count(), 3);

        tree.clear();
        assert_eq!(tree.node_count(), 0);
        assert!(tree.get_by_node_id(1).is_none());
        assert_eq!(tree.render(), "");
    }

    #[test]
    fn insert_child_at_beginning() {
        let mut tree = LiveDomTree::new();
        let root = make_document(1);
        tree.set_root(root);
        assert_eq!(tree.node_count(), 1);

        // Insert child with previous_node_id=0 -> at beginning
        tree.insert_child(1, 0, make_element(10, "DIV"));
        assert_eq!(tree.node_count(), 2);

        let parent = tree.get_by_node_id(1).unwrap();
        assert_eq!(parent.children.len(), 1);
    }

    #[test]
    fn insert_child_after_existing() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_element(2, "A")]);
        tree.set_root(root);

        // Insert after node 2
        tree.insert_child(1, 2, make_element(3, "B"));
        assert_eq!(tree.node_count(), 3);

        let parent = tree.get_by_node_id(1).unwrap();
        assert_eq!(parent.children.len(), 2);
    }

    #[test]
    fn remove_child() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_element(2, "DIV"), make_element(3, "SPAN")]);
        tree.set_root(root);
        assert_eq!(tree.node_count(), 3);

        tree.remove_child(1, 2);
        assert_eq!(tree.node_count(), 2); // root + SPAN
        assert!(tree.get_by_node_id(2).is_none());

        let parent = tree.get_by_node_id(1).unwrap();
        assert_eq!(parent.children.len(), 1);
    }

    #[test]
    fn remove_child_with_subtree() {
        let mut tree = LiveDomTree::new();
        let mut div = make_element(2, "DIV");
        div.children = Some(vec![make_element(4, "P"), make_text(5, "hello")]);
        let mut root = make_document(1);
        root.children = Some(vec![div, make_element(3, "SPAN")]);
        tree.set_root(root);
        assert_eq!(tree.node_count(), 5);

        // Removing node 2 (DIV) should also remove its children (P, text)
        tree.remove_child(1, 2);
        assert_eq!(tree.node_count(), 2); // root + SPAN
        assert!(tree.get_by_node_id(4).is_none());
        assert!(tree.get_by_node_id(5).is_none());
    }

    #[test]
    fn set_attribute_and_remove_attribute() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_element(2, "DIV")]);
        tree.set_root(root);

        // Set attribute
        tree.set_attribute(2, "class", "container");
        let node = tree.get_by_node_id(2).unwrap();
        assert_eq!(node.attributes.get("class").unwrap(), "container");

        // Update attribute
        tree.set_attribute(2, "class", "wrapper");
        let node = tree.get_by_node_id(2).unwrap();
        assert_eq!(node.attributes.get("class").unwrap(), "wrapper");

        // Remove attribute
        tree.remove_attribute(2, "class");
        let node = tree.get_by_node_id(2).unwrap();
        assert!(node.attributes.get("class").is_none());
    }

    #[test]
    fn set_character_data() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_text(2, "hello")]);
        tree.set_root(root);

        tree.set_character_data(2, "world");
        let node = tree.get_by_node_id(2).unwrap();
        assert_eq!(node.node_value, "world");
    }

    #[test]
    fn node_count_is_accurate() {
        let mut tree = LiveDomTree::new();
        assert_eq!(tree.node_count(), 0);

        tree.set_root(make_document(1));
        assert_eq!(tree.node_count(), 1);

        tree.insert_child(1, 0, make_element(2, "HTML"));
        assert_eq!(tree.node_count(), 2);

        tree.insert_child(2, 0, make_element(3, "BODY"));
        assert_eq!(tree.node_count(), 3);

        tree.remove_child(2, 3);
        assert_eq!(tree.node_count(), 2);
    }

    #[test]
    fn update_child_count() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_element(2, "DIV")]);
        tree.set_root(root);

        tree.update_child_count(2, 5);
        let node = tree.get_by_node_id(2).unwrap();
        assert_eq!(node.child_node_count, Some(5));
    }

    #[test]
    fn render_produces_output() {
        let mut tree = LiveDomTree::new();
        let mut html = make_element(2, "HTML");
        let body = make_element(3, "BODY");
        html.children = Some(vec![body]);
        let mut root = make_document(1);
        root.children = Some(vec![html]);
        tree.set_root(root);

        let output = tree.render();
        assert!(output.contains("#document"));
        assert!(output.contains("<html>"));
        assert!(output.contains("<body>"));
    }

    #[test]
    fn set_children_replaces_existing() {
        let mut tree = LiveDomTree::new();
        let mut root = make_document(1);
        root.children = Some(vec![make_element(2, "OLD1"), make_element(3, "OLD2")]);
        tree.set_root(root);
        assert_eq!(tree.node_count(), 3);

        // Replace children
        tree.set_children(1, vec![make_element(4, "NEW1")]);
        assert_eq!(tree.node_count(), 2); // root + NEW1
        assert!(tree.get_by_node_id(2).is_none());
        assert!(tree.get_by_node_id(3).is_none());
        assert!(tree.get_by_node_id(4).is_some());
    }

    #[test]
    fn operations_on_nonexistent_nodes_are_safe() {
        let mut tree = LiveDomTree::new();
        tree.set_root(make_document(1));

        // These should not panic
        tree.insert_child(999, 0, make_element(10, "X"));
        tree.remove_child(999, 10);
        tree.set_attribute(999, "a", "b");
        tree.remove_attribute(999, "a");
        tree.set_character_data(999, "text");
        tree.update_child_count(999, 5);

        // Tree should be unchanged
        assert_eq!(tree.node_count(), 1);
    }

    #[test]
    fn parse_attributes_works() {
        let attrs = Some(vec![
            "class".to_string(),
            "foo".to_string(),
            "id".to_string(),
            "bar".to_string(),
        ]);
        let map = parse_attributes(&attrs);
        assert_eq!(map.get("class").unwrap(), "foo");
        assert_eq!(map.get("id").unwrap(), "bar");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn parse_attributes_none() {
        let map = parse_attributes(&None);
        assert!(map.is_empty());
    }

    #[test]
    fn parse_attributes_odd_length() {
        // Odd-length array: last element has no pair, should be skipped
        let attrs = Some(vec![
            "class".to_string(),
            "foo".to_string(),
            "orphan".to_string(),
        ]);
        let map = parse_attributes(&attrs);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("class").unwrap(), "foo");
    }
}
