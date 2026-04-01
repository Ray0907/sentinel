//! Semantic DOM tree diff.
//! Unlike agent-browser's text-based Myers diff, this operates on the tree structure.

// TODO: Implement tree-edit-distance based diff algorithm
// For v1, the timeline-based observation report serves as the primary diff mechanism.
// Each action captures all DOM mutations that occurred, which is more informative
// than a before/after tree diff.
