//! DOMSnapshot-based reconciliation for correcting incremental tree drift.
//! Triggered after navigation or burst mutations (>100 events in 500ms).

use anyhow::Result;
use serde::Deserialize;

use crate::cdp::client::CdpClient;
use crate::cdp::types::DomNode;

use super::dom_tree::LiveDomTree;

/// Snapshot data returned by DOMSnapshot.captureSnapshot.
/// We only use the `documents` and `strings` arrays that Chrome provides.
#[derive(Debug, Deserialize)]
struct CaptureSnapshotResponse {
    documents: Vec<SnapshotDocument>,
    strings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SnapshotDocument {
    #[serde(rename = "documentURL")]
    document_url: usize, // index into strings
    nodes: SnapshotNodes,
}

#[derive(Debug, Deserialize)]
struct SnapshotNodes {
    #[serde(rename = "parentIndex")]
    parent_index: Vec<i64>,
    #[serde(rename = "nodeType")]
    node_type: Vec<i32>,
    #[serde(rename = "nodeName")]
    node_name: Vec<i64>, // index into strings (can be -1)
    #[serde(rename = "nodeValue")]
    node_value: Vec<i64>, // index into strings (can be -1)
    #[serde(rename = "backendNodeId")]
    backend_node_id: Vec<i64>,
}

/// Reconcile the live DOM tree by capturing a full snapshot via CDP and
/// rebuilding the tree from it. Returns the node count of the rebuilt tree.
pub async fn reconcile_from_snapshot(cdp: &CdpClient, dom_tree: &mut LiveDomTree) -> Result<usize> {
    tracing::info!("Starting DOMSnapshot reconciliation");

    // Capture a full snapshot. computedStyles=[] means we skip style data
    // (we only need the DOM structure).
    let result = cdp
        .call(
            "DOMSnapshot.captureSnapshot",
            serde_json::json!({
                "computedStyles": []
            }),
        )
        .await?;

    let snapshot: CaptureSnapshotResponse = serde_json::from_value(result)?;

    if snapshot.documents.is_empty() {
        tracing::warn!("DOMSnapshot returned 0 documents, skipping reconciliation");
        return Ok(0);
    }

    // Build a DomNode tree from the first (main) document snapshot
    let doc = &snapshot.documents[0];
    let strings = &snapshot.strings;
    let nodes = &doc.nodes;
    let count = nodes.node_type.len();

    if count == 0 {
        tracing::warn!("DOMSnapshot returned 0 nodes, skipping reconciliation");
        return Ok(0);
    }

    // Build flat Vec<DomNode> first, then assemble the tree
    let mut flat_nodes: Vec<DomNode> = Vec::with_capacity(count);
    for i in 0..count {
        let name_idx = nodes.node_name[i];
        let value_idx = nodes.node_value[i];
        let name_idx = if name_idx >= 0 { name_idx as usize } else { usize::MAX };
        let value_idx = if value_idx >= 0 { value_idx as usize } else { usize::MAX };

        flat_nodes.push(DomNode {
            node_id: i as i64 + 1, // synthetic sequential IDs (1-based)
            parent_id: if nodes.parent_index[i] >= 0 {
                Some(nodes.parent_index[i] + 1)
            } else {
                None
            },
            backend_node_id: nodes.backend_node_id[i],
            node_type: nodes.node_type[i],
            node_name: lookup_string(strings, name_idx),
            local_name: None,
            node_value: lookup_string(strings, value_idx),
            child_node_count: None,
            children: None, // will be populated below
            attributes: None,
            document_url: None,
            base_url: None,
            frame_id: None,
            content_document: None,
            shadow_roots: None,
            pseudo_type: None,
            pseudo_identifier: None,
            distributed_nodes: None,
        });
    }

    // Build child lists: for each node, collect its children indices
    let mut children_map: Vec<Vec<usize>> = vec![Vec::new(); count];
    for i in 0..count {
        let parent_idx = nodes.parent_index[i];
        if parent_idx >= 0 && (parent_idx as usize) < count {
            children_map[parent_idx as usize].push(i);
        }
    }

    // Recursively build the tree starting from the root (index 0)
    let root = build_tree_node(&flat_nodes, &children_map, 0);

    // Replace the live DOM tree with the snapshot-derived tree
    dom_tree.set_root(root);

    let new_count = dom_tree.node_count();
    tracing::info!(nodes = new_count, "DOMSnapshot reconciliation complete");

    // After reconciliation, re-enable incremental DOM tracking by requesting
    // the document tree through DOM.getDocument. This ensures Chrome sends
    // subsequent incremental events with correct node IDs.
    if let Ok(result) = cdp
        .call(
            "DOM.getDocument",
            serde_json::json!({"depth": -1, "pierce": true}),
        )
        .await
    {
        if let Some(root_val) = result.get("root") {
            if let Ok(node) = serde_json::from_value::<DomNode>(root_val.clone()) {
                dom_tree.set_root(node);
                tracing::debug!(
                    nodes = dom_tree.node_count(),
                    "Re-synced tree via DOM.getDocument after snapshot reconciliation"
                );
            }
        }
    }

    Ok(dom_tree.node_count())
}

/// Recursively build a DomNode tree from flat snapshot data.
fn build_tree_node(flat_nodes: &[DomNode], children_map: &[Vec<usize>], index: usize) -> DomNode {
    let base = &flat_nodes[index];
    let child_indices = &children_map[index];

    let children = if child_indices.is_empty() {
        None
    } else {
        Some(
            child_indices
                .iter()
                .map(|&ci| build_tree_node(flat_nodes, children_map, ci))
                .collect(),
        )
    };

    DomNode {
        node_id: base.node_id,
        parent_id: base.parent_id,
        backend_node_id: base.backend_node_id,
        node_type: base.node_type,
        node_name: base.node_name.clone(),
        local_name: None,
        node_value: base.node_value.clone(),
        child_node_count: children.as_ref().map(|c: &Vec<DomNode>| c.len() as i32),
        children,
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

/// Safe string lookup: returns empty string for out-of-bounds or negative indices.
fn lookup_string(strings: &[String], idx: usize) -> String {
    strings.get(idx).cloned().unwrap_or_default()
}
