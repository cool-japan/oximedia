//! Task dependency modelling for `oximedia-workflow`.
//!
//! Provides [`DependencyKind`] classification, [`TaskDependency`] edges, and a
//! [`DependencyResolver`] that performs a topological sort to determine correct
//! execution order and parallelism opportunities.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

// ---------------------------------------------------------------------------
// Dependency kind
// ---------------------------------------------------------------------------

/// Nature of the relationship between a predecessor and a successor task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DependencyKind {
    /// Successor cannot start until predecessor finishes successfully.
    FinishToStart,
    /// Successor cannot start until predecessor starts.
    StartToStart,
    /// Successor cannot finish until predecessor finishes.
    FinishToFinish,
    /// Soft dependency: preferred but not enforced (hints for the scheduler).
    Preferred,
}

impl DependencyKind {
    /// Returns a short label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FinishToStart => "Finish-to-Start",
            Self::StartToStart => "Start-to-Start",
            Self::FinishToFinish => "Finish-to-Finish",
            Self::Preferred => "Preferred",
        }
    }

    /// Returns all variants.
    #[must_use]
    pub const fn all() -> &'static [DependencyKind] {
        &[
            Self::FinishToStart,
            Self::StartToStart,
            Self::FinishToFinish,
            Self::Preferred,
        ]
    }

    /// Returns `true` if this dependency kind is strictly enforced.
    #[must_use]
    pub const fn is_hard(self) -> bool {
        !matches!(self, Self::Preferred)
    }
}

impl std::fmt::Display for DependencyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ---------------------------------------------------------------------------
// Task dependency edge
// ---------------------------------------------------------------------------

/// A directed edge from one task to another in a dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskDependency {
    /// Task that must complete (or start) first.
    pub from: String,
    /// Task that depends on `from`.
    pub to: String,
    /// Nature of the dependency.
    pub kind: DependencyKind,
}

impl TaskDependency {
    /// Creates a new dependency edge.
    #[must_use]
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: DependencyKind) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
        }
    }

    /// Convenience constructor for the most common Finish-to-Start edge.
    #[must_use]
    pub fn finish_to_start(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self::new(from, to, DependencyKind::FinishToStart)
    }
}

// ---------------------------------------------------------------------------
// Dependency resolver
// ---------------------------------------------------------------------------

/// Resolves a set of [`TaskDependency`] edges into a topologically-sorted
/// execution order, detecting cycles.
#[derive(Debug, Clone, Default)]
pub struct DependencyResolver {
    edges: Vec<TaskDependency>,
}

impl DependencyResolver {
    /// Creates a new empty resolver.
    #[must_use]
    pub fn new() -> Self {
        Self { edges: Vec::new() }
    }

    /// Adds a dependency edge.
    pub fn add(&mut self, dep: TaskDependency) {
        self.edges.push(dep);
    }

    /// Returns the number of edges.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns all registered edges.
    #[must_use]
    pub fn edges(&self) -> &[TaskDependency] {
        &self.edges
    }

    /// Collects the set of all task IDs referenced in the edges.
    #[must_use]
    pub fn all_tasks(&self) -> HashSet<String> {
        let mut set = HashSet::new();
        for e in &self.edges {
            set.insert(e.from.clone());
            set.insert(e.to.clone());
        }
        set
    }

    /// Returns the direct predecessors (hard deps only) of a given task.
    #[must_use]
    pub fn predecessors(&self, task_id: &str) -> Vec<String> {
        self.edges
            .iter()
            .filter(|e| e.to == task_id && e.kind.is_hard())
            .map(|e| e.from.clone())
            .collect()
    }

    /// Returns the direct successors (hard deps only) of a given task.
    #[must_use]
    pub fn successors(&self, task_id: &str) -> Vec<String> {
        self.edges
            .iter()
            .filter(|e| e.from == task_id && e.kind.is_hard())
            .map(|e| e.to.clone())
            .collect()
    }

    /// Performs a Kahn-style topological sort on hard dependencies.
    ///
    /// Returns `Ok(order)` if the graph is a DAG, or `Err(())` if a cycle
    /// is detected.
    pub fn topological_sort(&self) -> std::result::Result<Vec<String>, ()> {
        // Build adjacency and in-degree maps (hard edges only).
        let tasks = self.all_tasks();
        let mut in_degree: HashMap<String, usize> = tasks.iter().map(|t| (t.clone(), 0)).collect();
        let mut adj: HashMap<String, Vec<String>> =
            tasks.iter().map(|t| (t.clone(), Vec::new())).collect();

        // `adj`/`in_degree` were seeded from `tasks = self.all_tasks()`,
        // which is defined as the union of every edge's `from`/`to` over
        // this same `self.edges` — so both lookups below always hit.  The
        // `if let` (rather than `.expect()`) means a future change that
        // breaks that invariant degrades to "edge ignored" instead of
        // panicking.
        for e in &self.edges {
            if e.kind.is_hard() {
                if let Some(list) = adj.get_mut(&e.from) {
                    list.push(e.to.clone());
                }
                if let Some(d) = in_degree.get_mut(&e.to) {
                    *d += 1;
                }
            }
        }

        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(t, _)| t.clone())
            .collect();
        // Sort for deterministic output in tests.
        let mut q_sorted: Vec<String> = queue.drain(..).collect();
        q_sorted.sort();
        for t in q_sorted {
            queue.push_back(t);
        }

        let mut order = Vec::new();
        while let Some(task) = queue.pop_front() {
            order.push(task.clone());
            let mut next_batch = Vec::new();
            if let Some(successors) = adj.get(&task) {
                for succ in successors {
                    if let Some(d) = in_degree.get_mut(succ) {
                        *d -= 1;
                        if *d == 0 {
                            next_batch.push(succ.clone());
                        }
                    }
                }
            }
            next_batch.sort();
            for t in next_batch {
                queue.push_back(t);
            }
        }

        if order.len() == tasks.len() {
            Ok(order)
        } else {
            Err(())
        }
    }

    /// Clears all edges.
    pub fn clear(&mut self) {
        self.edges.clear();
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- DependencyKind -----------------------------------------------------

    #[test]
    fn test_kind_label() {
        assert_eq!(DependencyKind::FinishToStart.label(), "Finish-to-Start");
        assert_eq!(DependencyKind::Preferred.label(), "Preferred");
    }

    #[test]
    fn test_kind_display() {
        assert_eq!(
            format!("{}", DependencyKind::StartToStart),
            "Start-to-Start"
        );
    }

    #[test]
    fn test_kind_all() {
        assert_eq!(DependencyKind::all().len(), 4);
    }

    #[test]
    fn test_kind_is_hard() {
        assert!(DependencyKind::FinishToStart.is_hard());
        assert!(!DependencyKind::Preferred.is_hard());
    }

    // -- TaskDependency -----------------------------------------------------

    #[test]
    fn test_dependency_new() {
        let d = TaskDependency::new("a", "b", DependencyKind::FinishToStart);
        assert_eq!(d.from, "a");
        assert_eq!(d.to, "b");
    }

    #[test]
    fn test_dependency_finish_to_start() {
        let d = TaskDependency::finish_to_start("x", "y");
        assert_eq!(d.kind, DependencyKind::FinishToStart);
    }

    // -- DependencyResolver -------------------------------------------------

    #[test]
    fn test_resolver_empty() {
        let r = DependencyResolver::new();
        assert_eq!(r.edge_count(), 0);
        assert!(r.all_tasks().is_empty());
    }

    #[test]
    fn test_resolver_add() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "b"));
        assert_eq!(r.edge_count(), 1);
        assert_eq!(r.all_tasks().len(), 2);
    }

    #[test]
    fn test_predecessors() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "c"));
        r.add(TaskDependency::finish_to_start("b", "c"));
        let preds = r.predecessors("c");
        assert_eq!(preds.len(), 2);
        assert!(preds.contains(&"a".to_string()));
        assert!(preds.contains(&"b".to_string()));
    }

    #[test]
    fn test_successors() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "b"));
        r.add(TaskDependency::finish_to_start("a", "c"));
        let succs = r.successors("a");
        assert_eq!(succs.len(), 2);
    }

    #[test]
    fn test_topological_sort_linear() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "b"));
        r.add(TaskDependency::finish_to_start("b", "c"));
        let order = r.topological_sort().expect("should succeed in test");
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_topological_sort_diamond() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "b"));
        r.add(TaskDependency::finish_to_start("a", "c"));
        r.add(TaskDependency::finish_to_start("b", "d"));
        r.add(TaskDependency::finish_to_start("c", "d"));
        let order = r.topological_sort().expect("should succeed in test");
        assert_eq!(order[0], "a");
        assert_eq!(order[3], "d");
    }

    #[test]
    fn test_topological_sort_cycle() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "b"));
        r.add(TaskDependency::finish_to_start("b", "a"));
        assert!(r.topological_sort().is_err());
    }

    #[test]
    fn test_preferred_edges_ignored_in_sort() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "b"));
        // Preferred edge b->a would cause a cycle if treated as hard.
        r.add(TaskDependency::new("b", "a", DependencyKind::Preferred));
        let order = r.topological_sort().expect("should succeed in test");
        assert_eq!(order, vec!["a", "b"]);
    }

    #[test]
    fn test_clear() {
        let mut r = DependencyResolver::new();
        r.add(TaskDependency::finish_to_start("a", "b"));
        r.clear();
        assert_eq!(r.edge_count(), 0);
    }
}
