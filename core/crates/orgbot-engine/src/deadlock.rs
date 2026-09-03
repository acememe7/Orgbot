//! Deadlock detection over the wait-for graph.
//!
//! A room of bots that can hand work to each other can hang in ways a single
//! agent cannot: A waits on a handoff to B, B is blocked on a task that is
//! waiting on A. Nothing errors — every bot is politely waiting — and the room
//! sits idle until a timeout it may not have.
//!
//! The engine therefore keeps an explicit graph of "task X cannot proceed
//! until task Y does" and looks for cycles every time an edge is added. The
//! graph is keyed by task rather than by bot because a bot can hold several
//! tasks, and it is a specific task that is stuck.
//!
//! Iteration order is deterministic (`BTreeMap`/`BTreeSet`) so that the same
//! room state always reports the same cycle. A detector that names a different
//! victim on each run makes deadlock bugs unreproducible.

use std::collections::{BTreeMap, BTreeSet};

use orgbot_proto::TaskId;

/// A directed graph of "waits for" edges between tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WaitGraph {
    edges: BTreeMap<TaskId, BTreeSet<TaskId>>,
}

impl WaitGraph {
    /// An empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `waiter` cannot proceed until `blocker` does.
    ///
    /// Self-edges are ignored: a task waiting on itself is a caller bug, and
    /// reporting it as a one-element deadlock cycle would be noise.
    pub fn add_wait(&mut self, waiter: TaskId, blocker: TaskId) {
        if waiter == blocker {
            return;
        }
        self.edges.entry(waiter).or_default().insert(blocker);
    }

    /// Remove every edge out of `waiter`, because it is no longer blocked.
    pub fn clear_waits(&mut self, waiter: TaskId) {
        self.edges.remove(&waiter);
    }

    /// Remove `task` entirely, as both waiter and blocker.
    pub fn remove_task(&mut self, task: TaskId) {
        self.edges.remove(&task);
        for blockers in self.edges.values_mut() {
            blockers.remove(&task);
        }
    }

    /// The tasks `waiter` is waiting on.
    pub fn blockers_of(&self, waiter: TaskId) -> Vec<TaskId> {
        self.edges
            .get(&waiter)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Whether the graph holds no edges.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// The number of waiting tasks.
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Find one cycle, if any exists.
    ///
    /// Returns the tasks in wait order: each element waits on the next, and the
    /// last waits on the first. Depth-first with a three-colour marking; the
    /// first back edge encountered yields the cycle.
    pub fn find_cycle(&self) -> Option<Vec<TaskId>> {
        let mut settled: BTreeSet<TaskId> = BTreeSet::new();
        let mut on_stack: BTreeSet<TaskId> = BTreeSet::new();
        let mut path: Vec<TaskId> = Vec::new();

        for &start in self.edges.keys() {
            if settled.contains(&start) {
                continue;
            }
            if let Some(cycle) = self.walk(start, &mut settled, &mut on_stack, &mut path) {
                return Some(cycle);
            }
        }
        None
    }

    /// Whether any cycle exists.
    pub fn has_cycle(&self) -> bool {
        self.find_cycle().is_some()
    }

    /// Depth-first search from `node`, returning a cycle when a back edge into
    /// the current path is found.
    ///
    /// Written iteratively rather than recursively: a long dependency chain in
    /// a big room would otherwise risk blowing the stack, and this runs inside
    /// the engine's command path.
    fn walk(
        &self,
        start: TaskId,
        settled: &mut BTreeSet<TaskId>,
        on_stack: &mut BTreeSet<TaskId>,
        path: &mut Vec<TaskId>,
    ) -> Option<Vec<TaskId>> {
        // Each frame is a node plus how many of its blockers we have visited.
        let mut frames: Vec<(TaskId, usize)> = vec![(start, 0)];
        path.clear();
        path.push(start);
        on_stack.insert(start);

        while let Some((node, cursor)) = frames.last_mut() {
            let node = *node;
            let blockers = self.blockers_of(node);

            if *cursor >= blockers.len() {
                // Exhausted this node's blockers; it cannot start a cycle.
                settled.insert(node);
                on_stack.remove(&node);
                path.pop();
                frames.pop();
                continue;
            }

            let next = blockers[*cursor];
            *cursor += 1;

            if on_stack.contains(&next) {
                // Back edge: everything from `next` to the top of the path is
                // the cycle.
                let start_of_cycle = path.iter().position(|&t| t == next)?;
                return Some(path[start_of_cycle..].to_vec());
            }
            if settled.contains(&next) {
                continue;
            }
            on_stack.insert(next);
            path.push(next);
            frames.push((next, 0));
        }
        None
    }

    /// Every distinct cycle reachable in the graph.
    ///
    /// Used by diagnostics rather than the hot path: after breaking one cycle
    /// the engine re-runs [`find_cycle`](Self::find_cycle) instead.
    pub fn find_all_cycles(&self) -> Vec<Vec<TaskId>> {
        let mut found: Vec<Vec<TaskId>> = Vec::new();
        let mut seen: BTreeSet<Vec<TaskId>> = BTreeSet::new();
        let mut scratch = self.clone();

        while let Some(cycle) = scratch.find_cycle() {
            let key = canonical(&cycle);
            if seen.insert(key) {
                found.push(cycle.clone());
            }
            // Break this cycle so the search moves on to the next one.
            let (waiter, blocker) = (cycle[0], cycle[1 % cycle.len()]);
            if let Some(blockers) = scratch.edges.get_mut(&waiter) {
                blockers.remove(&blocker);
                if blockers.is_empty() {
                    scratch.edges.remove(&waiter);
                }
            } else {
                break;
            }
        }
        found
    }
}

/// Rotate a cycle so it starts at its smallest id, giving cycles that differ
/// only by starting point the same key.
fn canonical(cycle: &[TaskId]) -> Vec<TaskId> {
    let Some(min_at) = (0..cycle.len()).min_by_key(|&i| cycle[i]) else {
        return Vec::new();
    };
    cycle[min_at..].iter().chain(&cycle[..min_at]).copied().collect()
}

/// Pick the task to release in order to break `cycle`.
///
/// The victim is the smallest id in the cycle. That is arbitrary on purpose:
/// any member breaks it, and a deterministic choice makes a deadlock in a test
/// resolve the same way every run.
pub fn victim(cycle: &[TaskId]) -> Option<TaskId> {
    cycle.iter().copied().min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tasks<const N: usize>() -> [TaskId; N] {
        std::array::from_fn(|_| TaskId::new())
    }

    #[test]
    fn an_empty_graph_has_no_cycle() {
        assert!(WaitGraph::new().find_cycle().is_none());
        assert!(WaitGraph::new().is_empty());
    }

    #[test]
    fn a_linear_chain_is_not_a_deadlock() {
        let [a, b, c] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(b, c);
        assert!(!graph.has_cycle(), "a -> b -> c terminates");
    }

    #[test]
    fn a_diamond_is_not_a_deadlock() {
        // Two tasks both waiting on a shared dependency is normal fan-in.
        let [a, b, c, d] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(a, c);
        graph.add_wait(b, d);
        graph.add_wait(c, d);
        assert!(!graph.has_cycle());
    }

    #[test]
    fn a_mutual_wait_is_detected() {
        let [a, b] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(b, a);
        let cycle = graph.find_cycle().expect("a and b wait on each other");
        assert_eq!(cycle.len(), 2);
        assert!(cycle.contains(&a) && cycle.contains(&b));
    }

    #[test]
    fn a_three_way_cycle_is_detected_in_wait_order() {
        let [a, b, c] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(b, c);
        graph.add_wait(c, a);
        let cycle = graph.find_cycle().expect("a -> b -> c -> a");
        assert_eq!(cycle.len(), 3);
        // Each element must actually wait on the next one round the ring.
        for i in 0..cycle.len() {
            let waiter = cycle[i];
            let next = cycle[(i + 1) % cycle.len()];
            assert!(
                graph.blockers_of(waiter).contains(&next),
                "reported cycle must follow real edges"
            );
        }
    }

    #[test]
    fn a_cycle_hanging_off_a_chain_is_still_found() {
        let [entry, a, b] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(entry, a);
        graph.add_wait(a, b);
        graph.add_wait(b, a);
        let cycle = graph.find_cycle().expect("a and b deadlock downstream");
        assert_eq!(cycle.len(), 2);
        assert!(!cycle.contains(&entry), "the entry task is not part of the cycle");
    }

    #[test]
    fn a_self_wait_is_ignored_rather_than_reported() {
        let [a] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, a);
        assert!(graph.is_empty());
        assert!(!graph.has_cycle());
    }

    #[test]
    fn clearing_waits_breaks_the_cycle() {
        let [a, b] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(b, a);
        assert!(graph.has_cycle());
        graph.clear_waits(a);
        assert!(!graph.has_cycle(), "unblocking a resolves the deadlock");
    }

    #[test]
    fn removing_a_task_drops_its_inbound_edges_too() {
        let [a, b, c] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(c, b);
        graph.remove_task(b);
        assert!(graph.blockers_of(a).is_empty());
        assert!(graph.blockers_of(c).is_empty());
    }

    #[test]
    fn detection_is_deterministic_across_runs() {
        let [a, b, c] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(b, c);
        graph.add_wait(c, a);
        let first = graph.find_cycle().unwrap();
        for _ in 0..16 {
            assert_eq!(graph.find_cycle().unwrap(), first);
        }
    }

    #[test]
    fn two_independent_cycles_are_both_reported() {
        let [a, b, c, d] = tasks();
        let mut graph = WaitGraph::new();
        graph.add_wait(a, b);
        graph.add_wait(b, a);
        graph.add_wait(c, d);
        graph.add_wait(d, c);
        let cycles = graph.find_all_cycles();
        assert_eq!(cycles.len(), 2, "got {cycles:?}");
    }

    #[test]
    fn the_victim_is_stable_for_a_given_cycle() {
        let [a, b, c] = tasks();
        let cycle = vec![a, b, c];
        let chosen = victim(&cycle).unwrap();
        assert_eq!(victim(&[c, a, b]), Some(chosen), "rotation must not change the victim");
        assert!(cycle.contains(&chosen));
    }

    #[test]
    fn victim_of_an_empty_cycle_is_none() {
        assert_eq!(victim(&[]), None);
    }

    /// A long chain must not overflow the stack; the walk is iterative.
    #[test]
    fn a_deep_chain_does_not_overflow() {
        let ids: Vec<TaskId> = (0..10_000).map(|_| TaskId::new()).collect();
        let mut graph = WaitGraph::new();
        for pair in ids.windows(2) {
            graph.add_wait(pair[0], pair[1]);
        }
        assert!(!graph.has_cycle());

        // Close the loop and confirm the whole chain comes back as the cycle.
        graph.add_wait(*ids.last().unwrap(), ids[0]);
        assert_eq!(graph.find_cycle().unwrap().len(), ids.len());
    }
}
