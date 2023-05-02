#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeSet, VecDeque};
use std::iter::FromIterator;
use std::ops::ControlFlow;

use fxhash::FxHashSet;

use super::{DeadEndsCache, RankingRuleGraph, RankingRuleGraphTrait};
use crate::search::new::interner::{Interned, MappedInterner};
use crate::search::new::query_graph::QueryNode;
use crate::search::new::small_bitmap::SmallBitmap;
use crate::Result;

type VisitFn<'f, G> = &'f mut dyn FnMut(
    &[Interned<<G as RankingRuleGraphTrait>::Condition>],
    &mut RankingRuleGraph<G>,
    &mut DeadEndsCache<<G as RankingRuleGraphTrait>::Condition>,
) -> Result<ControlFlow<()>>;

struct VisitorContext<'a, G: RankingRuleGraphTrait> {
    graph: &'a mut RankingRuleGraph<G>,
    all_costs_from_node: &'a MappedInterner<QueryNode, Vec<u64>>,
    dead_ends_cache: &'a mut DeadEndsCache<G::Condition>,
}

struct VisitorState<G: RankingRuleGraphTrait> {
    remaining_cost: u64,

    path: Vec<Interned<G::Condition>>,

    visited_conditions: SmallBitmap<G::Condition>,
    visited_nodes: SmallBitmap<QueryNode>,

    forbidden_conditions: SmallBitmap<G::Condition>,
    forbidden_conditions_to_nodes: SmallBitmap<QueryNode>,
}

pub struct PathVisitor<'a, G: RankingRuleGraphTrait> {
    state: VisitorState<G>,
    ctx: VisitorContext<'a, G>,
}
impl<'a, G: RankingRuleGraphTrait> PathVisitor<'a, G> {
    pub fn new(
        cost: u64,
        graph: &'a mut RankingRuleGraph<G>,
        all_costs_from_node: &'a MappedInterner<QueryNode, Vec<u64>>,
        dead_ends_cache: &'a mut DeadEndsCache<G::Condition>,
    ) -> Self {
        Self {
            state: VisitorState {
                remaining_cost: cost,
                path: vec![],
                visited_conditions: SmallBitmap::for_interned_values_in(&graph.conditions_interner),
                visited_nodes: SmallBitmap::for_interned_values_in(&graph.query_graph.nodes),
                forbidden_conditions: SmallBitmap::for_interned_values_in(
                    &graph.conditions_interner,
                ),
                forbidden_conditions_to_nodes: SmallBitmap::for_interned_values_in(
                    &graph.query_graph.nodes,
                ),
            },
            ctx: VisitorContext { graph, all_costs_from_node, dead_ends_cache },
        }
    }

    pub fn visit_paths(mut self, visit: VisitFn<G>) -> Result<()> {
        let _ =
            self.state.visit_node(self.ctx.graph.query_graph.root_node, visit, &mut self.ctx)?;
        Ok(())
    }
}

impl<G: RankingRuleGraphTrait> VisitorState<G> {
    fn visit_node(
        &mut self,
        from_node: Interned<QueryNode>,
        visit: VisitFn<G>,
        ctx: &mut VisitorContext<G>,
    ) -> Result<ControlFlow<(), bool>> {
        let mut any_valid = false;

        let edges = ctx.graph.edges_of_node.get(from_node).clone();
        for edge_idx in edges.iter() {
            let Some(edge) = ctx.graph.edges_store.get(edge_idx).clone() else { continue };

            if self.remaining_cost < edge.cost as u64 {
                continue;
            }
            self.remaining_cost -= edge.cost as u64;
            let cf = match edge.condition {
                Some(condition) => self.visit_condition(
                    condition,
                    edge.dest_node,
                    &edge.nodes_to_skip,
                    visit,
                    ctx,
                )?,
                None => self.visit_no_condition(edge.dest_node, &edge.nodes_to_skip, visit, ctx)?,
            };
            self.remaining_cost += edge.cost as u64;

            let ControlFlow::Continue(next_any_valid) = cf else {
                return Ok(ControlFlow::Break(()));
            };
            any_valid |= next_any_valid;
            if next_any_valid {
                // backtrack as much as possible if a valid path was found and the dead_ends_cache
                // was updated such that the current prefix is now invalid
                self.forbidden_conditions = ctx
                    .dead_ends_cache
                    .forbidden_conditions_for_all_prefixes_up_to(self.path.iter().copied());
                if self.visited_conditions.intersects(&self.forbidden_conditions) {
                    return Ok(ControlFlow::Continue(true));
                }
            }
        }

        Ok(ControlFlow::Continue(any_valid))
    }

    fn visit_no_condition(
        &mut self,
        dest_node: Interned<QueryNode>,
        edge_new_nodes_to_skip: &SmallBitmap<QueryNode>,
        visit: VisitFn<G>,
        ctx: &mut VisitorContext<G>,
    ) -> Result<ControlFlow<(), bool>> {
        if !ctx
            .all_costs_from_node
            .get(dest_node)
            .iter()
            .any(|next_cost| *next_cost == self.remaining_cost)
        {
            return Ok(ControlFlow::Continue(false));
        }
        if dest_node == ctx.graph.query_graph.end_node {
            let control_flow = visit(&self.path, ctx.graph, ctx.dead_ends_cache)?;
            match control_flow {
                ControlFlow::Continue(_) => Ok(ControlFlow::Continue(true)),
                ControlFlow::Break(_) => Ok(ControlFlow::Break(())),
            }
        } else {
            let old_fbct = self.forbidden_conditions_to_nodes.clone();
            self.forbidden_conditions_to_nodes.union(edge_new_nodes_to_skip);
            let cf = self.visit_node(dest_node, visit, ctx)?;
            self.forbidden_conditions_to_nodes = old_fbct;
            Ok(cf)
        }
    }
    fn visit_condition(
        &mut self,
        condition: Interned<G::Condition>,
        dest_node: Interned<QueryNode>,
        edge_new_nodes_to_skip: &SmallBitmap<QueryNode>,
        visit: VisitFn<G>,
        ctx: &mut VisitorContext<G>,
    ) -> Result<ControlFlow<(), bool>> {
        assert!(dest_node != ctx.graph.query_graph.end_node);

        if self.forbidden_conditions.contains(condition)
            || self.forbidden_conditions_to_nodes.contains(dest_node)
            || edge_new_nodes_to_skip.intersects(&self.visited_nodes)
        {
            return Ok(ControlFlow::Continue(false));
        }

        // Checking that from the destination node, there is at least
        // one cost that we can visit that corresponds to our remaining budget.
        if !ctx
            .all_costs_from_node
            .get(dest_node)
            .iter()
            .any(|next_cost| *next_cost == self.remaining_cost)
        {
            return Ok(ControlFlow::Continue(false));
        }

        self.path.push(condition);
        self.visited_nodes.insert(dest_node);
        self.visited_conditions.insert(condition);

        let old_fc = self.forbidden_conditions.clone();
        if let Some(next_forbidden) =
            ctx.dead_ends_cache.forbidden_conditions_after_prefix(self.path.iter().copied())
        {
            self.forbidden_conditions.union(&next_forbidden);
        }
        let old_fctn = self.forbidden_conditions_to_nodes.clone();
        self.forbidden_conditions_to_nodes.union(edge_new_nodes_to_skip);

        let cf = self.visit_node(dest_node, visit, ctx)?;

        self.forbidden_conditions_to_nodes = old_fctn;
        self.forbidden_conditions = old_fc;

        self.visited_conditions.remove(condition);
        self.visited_nodes.remove(dest_node);
        self.path.pop();

        Ok(cf)
    }
}

impl<G: RankingRuleGraphTrait> RankingRuleGraph<G> {
    pub fn find_all_costs_to_end(&self) -> MappedInterner<QueryNode, Vec<u64>> {
        let mut costs_to_end = self.query_graph.nodes.map(|_| vec![]);
        let mut enqueued = SmallBitmap::new(self.query_graph.nodes.len());

        let mut node_stack = VecDeque::new();

        *costs_to_end.get_mut(self.query_graph.end_node) = vec![0];

        for prev_node in self.query_graph.nodes.get(self.query_graph.end_node).predecessors.iter() {
            node_stack.push_back(prev_node);
            enqueued.insert(prev_node);
        }

        while let Some(cur_node) = node_stack.pop_front() {
            let mut self_costs = Vec::<u64>::new();

            let cur_node_edges = &self.edges_of_node.get(cur_node);
            for edge_idx in cur_node_edges.iter() {
                let edge = self.edges_store.get(edge_idx).as_ref().unwrap();
                let succ_node = edge.dest_node;
                let succ_costs = costs_to_end.get(succ_node);
                for succ_cost in succ_costs {
                    self_costs.push(edge.cost as u64 + succ_cost);
                }
            }
            self_costs.sort_unstable();
            self_costs.dedup();

            *costs_to_end.get_mut(cur_node) = self_costs;
            for prev_node in self.query_graph.nodes.get(cur_node).predecessors.iter() {
                if !enqueued.contains(prev_node) {
                    node_stack.push_back(prev_node);
                    enqueued.insert(prev_node);
                }
            }
        }
        costs_to_end
    }

    pub fn update_all_costs_before_node(
        &self,
        node_with_removed_outgoing_conditions: Interned<QueryNode>,
        costs: &mut MappedInterner<QueryNode, Vec<u64>>,
    ) {
        let mut enqueued = SmallBitmap::new(self.query_graph.nodes.len());
        let mut node_stack = VecDeque::new();

        enqueued.insert(node_with_removed_outgoing_conditions);
        node_stack.push_back(node_with_removed_outgoing_conditions);

        'main_loop: while let Some(cur_node) = node_stack.pop_front() {
            let mut costs_to_remove = FxHashSet::default();
            for c in costs.get(cur_node) {
                costs_to_remove.insert(*c);
            }

            let cur_node_edges = &self.edges_of_node.get(cur_node);
            for edge_idx in cur_node_edges.iter() {
                let edge = self.edges_store.get(edge_idx).as_ref().unwrap();
                for cost in costs.get(edge.dest_node).iter() {
                    costs_to_remove.remove(&(*cost + edge.cost as u64));
                    if costs_to_remove.is_empty() {
                        continue 'main_loop;
                    }
                }
            }
            if costs_to_remove.is_empty() {
                continue 'main_loop;
            }
            let mut new_costs = BTreeSet::from_iter(costs.get(cur_node).iter().copied());
            for c in costs_to_remove {
                new_costs.remove(&c);
            }
            *costs.get_mut(cur_node) = new_costs.into_iter().collect();

            for prev_node in self.query_graph.nodes.get(cur_node).predecessors.iter() {
                if !enqueued.contains(prev_node) {
                    node_stack.push_back(prev_node);
                    enqueued.insert(prev_node);
                }
            }
        }
    }
}
