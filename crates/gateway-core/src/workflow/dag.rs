//! DAG (Directed Acyclic Graph) Executor
//!
//! Provides dependency-based execution order for workflow steps.

use crate::error::{Error, Result};
use std::collections::{HashMap, HashSet, VecDeque};

/// DAG node representing a workflow step
#[derive(Debug, Clone)]
pub struct DagNode {
    pub id: String,
    pub dependencies: Vec<String>,
}

/// DAG executor for determining execution order
pub struct DagExecutor {
    nodes: HashMap<String, DagNode>,
}

impl DagExecutor {
    /// Create a new DAG executor
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    /// Add a node to the DAG
    pub fn add_node(&mut self, id: String, dependencies: Vec<String>) {
        self.nodes.insert(id.clone(), DagNode { id, dependencies });
    }

    /// Clear all nodes
    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Validate the DAG (check for cycles)
    pub fn validate(&self) -> Result<()> {
        // Check for cycles using DFS
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        for node_id in self.nodes.keys() {
            if self.has_cycle(node_id, &mut visited, &mut rec_stack)? {
                return Err(Error::Workflow(format!(
                    "Cycle detected in DAG at node: {}",
                    node_id
                )));
            }
        }

        Ok(())
    }

    fn has_cycle(
        &self,
        node_id: &str,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
    ) -> Result<bool> {
        if rec_stack.contains(node_id) {
            return Ok(true);
        }
        if visited.contains(node_id) {
            return Ok(false);
        }

        visited.insert(node_id.to_string());
        rec_stack.insert(node_id.to_string());

        if let Some(node) = self.nodes.get(node_id) {
            for dep in &node.dependencies {
                if self.has_cycle(dep, visited, rec_stack)? {
                    return Ok(true);
                }
            }
        }

        rec_stack.remove(node_id);
        Ok(false)
    }

    /// Get topologically sorted execution order
    pub fn get_execution_order(&self) -> Result<Vec<String>> {
        self.validate()?;

        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();

        // Initialize in-degree for all nodes
        for node_id in self.nodes.keys() {
            in_degree.entry(node_id.clone()).or_insert(0);
        }

        // Calculate in-degrees
        for (node_id, node) in &self.nodes {
            for dep in &node.dependencies {
                reverse_deps
                    .entry(dep.clone())
                    .or_default()
                    .push(node_id.clone());
            }
            *in_degree.entry(node_id.clone()).or_insert(0) += node.dependencies.len();
        }

        // Kahn's algorithm for topological sort
        let mut queue: VecDeque<String> = VecDeque::new();
        for (node_id, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(node_id.clone());
            }
        }

        let mut result = Vec::new();
        while let Some(node_id) = queue.pop_front() {
            result.push(node_id.clone());

            if let Some(dependents) = reverse_deps.get(&node_id) {
                for dependent in dependents {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
        }

        if result.len() != self.nodes.len() {
            return Err(Error::Workflow(
                "DAG has unresolved dependencies".to_string(),
            ));
        }

        Ok(result)
    }

    /// Get nodes that can be executed in parallel at each level
    pub fn get_parallel_levels(&self) -> Result<Vec<Vec<String>>> {
        self.validate()?;

        let mut levels: Vec<Vec<String>> = Vec::new();
        let mut completed: HashSet<String> = HashSet::new();
        let remaining: HashSet<String> = self.nodes.keys().cloned().collect();

        while completed.len() < self.nodes.len() {
            let mut current_level = Vec::new();

            for node_id in remaining.difference(&completed) {
                let node = self.nodes.get(node_id).unwrap();
                let deps_satisfied = node.dependencies.iter().all(|dep| completed.contains(dep));

                if deps_satisfied {
                    current_level.push(node_id.clone());
                }
            }

            if current_level.is_empty() && completed.len() < self.nodes.len() {
                return Err(Error::Workflow(
                    "DAG has unresolvable dependencies".to_string(),
                ));
            }

            for node_id in &current_level {
                completed.insert(node_id.clone());
            }

            if !current_level.is_empty() {
                levels.push(current_level);
            }
        }

        Ok(levels)
    }

    /// Check if a specific node can be executed given completed nodes
    pub fn can_execute(&self, node_id: &str, completed: &HashSet<String>) -> bool {
        if let Some(node) = self.nodes.get(node_id) {
            node.dependencies.iter().all(|dep| completed.contains(dep))
        } else {
            false
        }
    }
}

impl Default for DagExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_dag() {
        let mut dag = DagExecutor::new();
        dag.add_node("A".to_string(), vec![]);
        dag.add_node("B".to_string(), vec!["A".to_string()]);
        dag.add_node("C".to_string(), vec!["A".to_string()]);
        dag.add_node("D".to_string(), vec!["B".to_string(), "C".to_string()]);

        let order = dag.get_execution_order().unwrap();
        assert_eq!(order.len(), 4);

        // A must come before B, C, D
        let a_pos = order.iter().position(|x| x == "A").unwrap();
        let b_pos = order.iter().position(|x| x == "B").unwrap();
        let c_pos = order.iter().position(|x| x == "C").unwrap();
        let d_pos = order.iter().position(|x| x == "D").unwrap();

        assert!(a_pos < b_pos);
        assert!(a_pos < c_pos);
        assert!(b_pos < d_pos);
        assert!(c_pos < d_pos);
    }

    #[test]
    fn test_parallel_levels() {
        let mut dag = DagExecutor::new();
        dag.add_node("A".to_string(), vec![]);
        dag.add_node("B".to_string(), vec![]);
        dag.add_node("C".to_string(), vec!["A".to_string(), "B".to_string()]);
        dag.add_node("D".to_string(), vec!["C".to_string()]);

        let levels = dag.get_parallel_levels().unwrap();

        // Level 0: A, B (can run in parallel)
        // Level 1: C
        // Level 2: D
        assert_eq!(levels.len(), 3);
        assert!(levels[0].contains(&"A".to_string()));
        assert!(levels[0].contains(&"B".to_string()));
        assert!(levels[1].contains(&"C".to_string()));
        assert!(levels[2].contains(&"D".to_string()));
    }

    #[test]
    fn test_cycle_detection() {
        let mut dag = DagExecutor::new();
        dag.add_node("A".to_string(), vec!["C".to_string()]);
        dag.add_node("B".to_string(), vec!["A".to_string()]);
        dag.add_node("C".to_string(), vec!["B".to_string()]);

        let result = dag.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_can_execute() {
        let mut dag = DagExecutor::new();
        dag.add_node("A".to_string(), vec![]);
        dag.add_node("B".to_string(), vec!["A".to_string()]);

        let mut completed = HashSet::new();

        // A can execute (no deps)
        assert!(dag.can_execute("A", &completed));

        // B cannot execute yet
        assert!(!dag.can_execute("B", &completed));

        // After A completes, B can execute
        completed.insert("A".to_string());
        assert!(dag.can_execute("B", &completed));
    }
}
