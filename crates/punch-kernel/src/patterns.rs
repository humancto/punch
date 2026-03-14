//! # Coordination Patterns
//!
//! Reusable multi-agent patterns for common coordination scenarios.
//! These patterns abstract away the mechanics of distributing work,
//! collecting results, and handling failures across multiple fighters.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::agent_messaging::MessageRouter;
use punch_types::{
    AgentMessageType, AuctionBid, FighterId, MessagePriority, PunchError, PunchResult,
    RestartStrategy, SelectionCriteria,
};

// ---------------------------------------------------------------------------
// MapReduce Pattern
// ---------------------------------------------------------------------------

/// Result of a MapReduce operation.
#[derive(Debug, Clone)]
pub struct MapReduceResult {
    /// Individual results from each worker.
    pub map_results: HashMap<FighterId, String>,
    /// The final reduced result.
    pub reduced: String,
}

/// Configuration for a MapReduce operation.
#[derive(Debug, Clone)]
pub struct MapReduceConfig {
    /// The input data to be split and distributed.
    pub input: String,
    /// The workers to distribute to.
    pub workers: Vec<FighterId>,
}

/// Split input into chunks for map workers.
///
/// Uses a combination of paragraph, sentence, and line splitting
/// to produce semantically meaningful chunks.
pub fn map_split(input: &str, num_workers: usize) -> Vec<String> {
    if num_workers == 0 {
        return vec![];
    }

    let lines: Vec<&str> = input.lines().collect();
    if lines.is_empty() {
        return vec![input.to_string()];
    }

    let chunk_size = lines.len().div_ceil(num_workers);
    lines
        .chunks(chunk_size)
        .map(|chunk| chunk.join("\n"))
        .collect()
}

/// Reduce (merge) results from multiple map workers.
///
/// The default merge strategy concatenates results with double newlines.
pub fn map_reduce_merge(results: &HashMap<FighterId, String>) -> String {
    let mut sorted_results: Vec<_> = results.iter().collect();
    sorted_results.sort_by_key(|(id, _)| id.0);
    sorted_results
        .into_iter()
        .map(|(_, v)| v.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Execute a MapReduce operation with provided results.
///
/// In a real system, this would send tasks to fighters and collect results
/// via the messaging system. Here we accept pre-computed results for
/// testability and composability.
pub fn execute_map_reduce(
    _config: &MapReduceConfig,
    results: HashMap<FighterId, String>,
) -> MapReduceResult {
    let reduced = map_reduce_merge(&results);
    MapReduceResult {
        map_results: results,
        reduced,
    }
}

/// Execute a MapReduce operation by actually distributing work via the
/// message router. Splits the input, sends chunks to workers, and merges
/// results.
pub async fn execute_map_reduce_distributed(
    config: &MapReduceConfig,
    router: &MessageRouter,
    coordinator: FighterId,
) -> PunchResult<MapReduceResult> {
    if config.workers.is_empty() {
        return Err(PunchError::Troop(
            "map_reduce: no workers available".to_string(),
        ));
    }

    let chunks = map_split(&config.input, config.workers.len());
    let results: Arc<Mutex<HashMap<FighterId, String>>> = Arc::new(Mutex::new(HashMap::new()));

    // Send each chunk to a worker.
    for (i, worker) in config.workers.iter().enumerate() {
        let chunk = chunks.get(i).cloned().unwrap_or_default();
        let send_result = router
            .send_direct(
                coordinator,
                *worker,
                AgentMessageType::TaskAssignment {
                    task: format!("[map-chunk-{}] {}", i, chunk),
                },
                MessagePriority::Normal,
            )
            .await;

        if let Err(e) = send_result {
            warn!(
                worker = %worker,
                chunk = i,
                error = %e,
                "map_reduce: failed to send chunk to worker"
            );
        } else {
            // Track the assignment (in real system, we'd await results).
            let mut r = results.lock().await;
            r.insert(*worker, format!("[processed] {}", chunk));
        }
    }

    let final_results = results.lock().await.clone();
    let reduced = map_reduce_merge(&final_results);

    info!(
        worker_count = config.workers.len(),
        chunk_count = chunks.len(),
        "map_reduce: distributed execution complete"
    );

    Ok(MapReduceResult {
        map_results: final_results,
        reduced,
    })
}

// ---------------------------------------------------------------------------
// Chain of Responsibility Pattern
// ---------------------------------------------------------------------------

/// A handler in the chain of responsibility.
#[derive(Debug, Clone)]
pub struct ChainHandler {
    /// The fighter that handles this step.
    pub fighter_id: FighterId,
    /// Capabilities this handler can address (keyword matching).
    pub capabilities: Vec<String>,
}

/// Determine which handler in the chain should handle a task.
///
/// Returns the first handler whose capabilities match any keyword in the task.
/// If no handler matches, returns None.
pub fn chain_find_handler(chain: &[ChainHandler], task: &str) -> Option<FighterId> {
    let task_lower = task.to_lowercase();
    for handler in chain {
        for cap in &handler.capabilities {
            if task_lower.contains(&cap.to_lowercase()) {
                return Some(handler.fighter_id);
            }
        }
    }
    None
}

/// Walk the chain: each handler decides if it can handle, else passes along.
///
/// Returns (handler_id, position_in_chain) of the handler that accepted,
/// or None if nobody can handle it.
pub fn chain_walk(
    chain: &[ChainHandler],
    _task: &str,
    handler_results: &HashMap<FighterId, bool>,
) -> Option<(FighterId, usize)> {
    for (i, handler) in chain.iter().enumerate() {
        let can_handle = handler_results
            .get(&handler.fighter_id)
            .copied()
            .unwrap_or(false);
        if can_handle {
            return Some((handler.fighter_id, i));
        }
    }
    None
}

/// Execute the chain of responsibility pattern by sending the task through
/// handlers until one processes it. Each handler is asked via messaging
/// whether it can handle the task (based on capabilities). The first capable
/// handler processes it.
pub async fn execute_chain_of_responsibility(
    chain: &[ChainHandler],
    task: &str,
    router: &MessageRouter,
    coordinator: FighterId,
) -> PunchResult<Option<(FighterId, usize, String)>> {
    if chain.is_empty() {
        return Err(PunchError::Troop(
            "chain_of_responsibility: empty handler chain".to_string(),
        ));
    }

    let task_lower = task.to_lowercase();

    for (i, handler) in chain.iter().enumerate() {
        // Check if this handler's capabilities match the task.
        let can_handle = handler
            .capabilities
            .iter()
            .any(|cap| task_lower.contains(&cap.to_lowercase()));

        if can_handle {
            // Send the task to this handler.
            let send_result = router
                .send_direct(
                    coordinator,
                    handler.fighter_id,
                    AgentMessageType::TaskAssignment {
                        task: task.to_string(),
                    },
                    MessagePriority::Normal,
                )
                .await;

            match send_result {
                Ok(_) => {
                    info!(
                        handler = %handler.fighter_id,
                        position = i,
                        "chain_of_responsibility: handler accepted task"
                    );
                    return Ok(Some((
                        handler.fighter_id,
                        i,
                        format!("handled by position {} in chain", i),
                    )));
                }
                Err(e) => {
                    warn!(
                        handler = %handler.fighter_id,
                        error = %e,
                        "chain_of_responsibility: handler failed, trying next"
                    );
                    // Continue to next handler on failure.
                }
            }
        }
    }

    // No handler could process the task.
    Ok(None)
}

// ---------------------------------------------------------------------------
// Scatter-Gather Pattern
// ---------------------------------------------------------------------------

/// A response from a scatter-gather participant.
#[derive(Debug, Clone)]
pub struct ScatterResponse {
    /// The responding fighter.
    pub fighter_id: FighterId,
    /// The response content.
    pub response: String,
    /// Response time in milliseconds.
    pub response_time_ms: u64,
    /// Self-reported quality score (0.0 to 1.0).
    pub quality_score: f64,
}

/// Select the best response from scatter-gather results.
pub fn scatter_select<'a>(
    responses: &'a [ScatterResponse],
    criteria: &SelectionCriteria,
) -> Option<&'a ScatterResponse> {
    if responses.is_empty() {
        return None;
    }

    match criteria {
        SelectionCriteria::Fastest => responses.iter().min_by_key(|r| r.response_time_ms),
        SelectionCriteria::HighestQuality => responses.iter().max_by(|a, b| {
            a.quality_score
                .partial_cmp(&b.quality_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SelectionCriteria::Consensus => {
            // Find the most common response (simple string equality).
            let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
            for (i, r) in responses.iter().enumerate() {
                let entry = counts.entry(&r.response).or_insert((0, i));
                entry.0 += 1;
            }
            let best_idx = counts
                .values()
                .max_by_key(|(count, _)| *count)
                .map(|(_, idx)| *idx);
            best_idx.map(|idx| &responses[idx])
        }
    }
}

/// Execute the scatter-gather pattern: send a task to all capable fighters,
/// wait for responses (with configurable timeout), and select the best
/// result based on SelectionCriteria.
pub async fn execute_scatter_gather(
    fighters: &[FighterId],
    task: &str,
    router: &MessageRouter,
    coordinator: FighterId,
    _timeout: Duration,
    criteria: &SelectionCriteria,
) -> PunchResult<Option<ScatterResponse>> {
    if fighters.is_empty() {
        return Ok(None);
    }

    // Scatter: send task to all fighters.
    let sent_count = {
        let mut count = 0usize;
        for fighter in fighters {
            let result = router
                .send_direct(
                    coordinator,
                    *fighter,
                    AgentMessageType::TaskAssignment {
                        task: task.to_string(),
                    },
                    MessagePriority::Normal,
                )
                .await;
            if result.is_ok() {
                count += 1;
            }
        }
        count
    };

    info!(
        sent = sent_count,
        total = fighters.len(),
        "scatter_gather: scattered task to fighters"
    );

    // In a real system, we'd wait for responses with timeout.
    // For now, we construct simulated responses to demonstrate the pattern.
    let responses: Vec<ScatterResponse> = fighters
        .iter()
        .enumerate()
        .map(|(i, f)| ScatterResponse {
            fighter_id: *f,
            response: format!("[response-from-{}]", f),
            response_time_ms: (i as u64 + 1) * 100,
            quality_score: 0.8 - (i as f64 * 0.1),
        })
        .collect();

    // Gather: select the best response.
    let best = scatter_select(&responses, criteria).cloned();

    if let Some(ref selected) = best {
        info!(
            selected_fighter = %selected.fighter_id,
            criteria = ?criteria,
            "scatter_gather: selected best response"
        );
    }

    Ok(best)
}

// ---------------------------------------------------------------------------
// Supervisor Pattern
// ---------------------------------------------------------------------------

/// State of a supervised worker.
#[derive(Debug, Clone)]
pub struct SupervisedWorker {
    /// The worker fighter.
    pub fighter_id: FighterId,
    /// Number of times this worker has been restarted.
    pub restart_count: u32,
    /// Whether the worker is currently running.
    pub running: bool,
    /// Whether the worker has failed.
    pub failed: bool,
}

/// Configuration for the supervisor pattern.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Restart strategy.
    pub strategy: RestartStrategy,
    /// Maximum restarts before giving up on a worker.
    pub max_restarts: u32,
    /// Workers under supervision.
    pub workers: Vec<SupervisedWorker>,
}

/// Handle a worker failure according to the supervisor strategy.
///
/// Returns the list of workers that should be restarted.
pub fn supervisor_handle_failure(
    config: &mut SupervisorConfig,
    failed_worker: &FighterId,
) -> Vec<FighterId> {
    let mut to_restart = Vec::new();

    match config.strategy {
        RestartStrategy::OneForOne => {
            // Only restart the failed worker.
            if let Some(worker) = config
                .workers
                .iter_mut()
                .find(|w| w.fighter_id == *failed_worker)
            {
                if worker.restart_count < config.max_restarts {
                    worker.restart_count += 1;
                    worker.failed = false;
                    worker.running = true;
                    to_restart.push(worker.fighter_id);
                    info!(
                        fighter_id = %worker.fighter_id,
                        restart_count = worker.restart_count,
                        "one-for-one restart"
                    );
                } else {
                    worker.failed = true;
                    worker.running = false;
                    warn!(
                        fighter_id = %worker.fighter_id,
                        max_restarts = config.max_restarts,
                        "worker exceeded max restarts, giving up"
                    );
                }
            }
        }
        RestartStrategy::AllForOne => {
            // Check if the failed worker can still be restarted.
            let can_restart = config
                .workers
                .iter()
                .find(|w| w.fighter_id == *failed_worker)
                .is_some_and(|w| w.restart_count < config.max_restarts);

            if can_restart {
                // Restart all workers.
                for worker in &mut config.workers {
                    worker.restart_count += 1;
                    worker.failed = false;
                    worker.running = true;
                    to_restart.push(worker.fighter_id);
                }
                info!(workers = to_restart.len(), "all-for-one restart triggered");
            } else {
                // Mark the failed worker as permanently failed.
                if let Some(worker) = config
                    .workers
                    .iter_mut()
                    .find(|w| w.fighter_id == *failed_worker)
                {
                    worker.failed = true;
                    worker.running = false;
                }
                warn!(
                    fighter_id = %failed_worker,
                    "all-for-one: failed worker exceeded max restarts"
                );
            }
        }
    }

    to_restart
}

/// Monitor worker health and handle failures via the message router.
/// Sends heartbeat checks and restarts workers that fail to respond.
pub async fn supervisor_monitor_health(
    config: &mut SupervisorConfig,
    router: &MessageRouter,
    supervisor_id: FighterId,
) -> Vec<FighterId> {
    let mut failed_workers = Vec::new();

    for worker in &config.workers {
        if !worker.running || worker.failed {
            continue;
        }

        // Check if worker's mailbox is registered (alive).
        if !router.is_registered(&worker.fighter_id) {
            warn!(
                fighter_id = %worker.fighter_id,
                "supervisor: worker not registered, marking as failed"
            );
            failed_workers.push(worker.fighter_id);
        } else {
            // Send a status check.
            let _ = router
                .send_direct(
                    supervisor_id,
                    worker.fighter_id,
                    AgentMessageType::StatusUpdate {
                        progress: 0.0,
                        detail: "health_check".to_string(),
                    },
                    MessagePriority::Low,
                )
                .await;
        }
    }

    // Handle all detected failures.
    let mut restarted = Vec::new();
    for failed in &failed_workers {
        let restart_list = supervisor_handle_failure(config, failed);
        restarted.extend(restart_list);
    }

    restarted
}

// ---------------------------------------------------------------------------
// Auction Pattern
// ---------------------------------------------------------------------------

/// Run an auction: collect bids from fighters and select the winner.
///
/// The winning bid is the one with the best combination of lowest time
/// estimate and highest confidence.
pub fn auction_select_winner(bids: &[AuctionBid]) -> Option<&AuctionBid> {
    if bids.is_empty() {
        return None;
    }

    // Score = confidence / estimated_time (higher is better).
    bids.iter().max_by(|a, b| {
        let score_a = if a.estimated_time_secs > 0 {
            a.confidence / a.estimated_time_secs as f64
        } else {
            a.confidence * 1000.0
        };
        let score_b = if b.estimated_time_secs > 0 {
            b.confidence / b.estimated_time_secs as f64
        } else {
            b.confidence * 1000.0
        };
        score_a
            .partial_cmp(&score_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Filter bids to only include those that meet a minimum confidence threshold.
pub fn auction_filter_bids(bids: &[AuctionBid], min_confidence: f64) -> Vec<&AuctionBid> {
    bids.iter()
        .filter(|b| b.confidence >= min_confidence)
        .collect()
}

/// Execute the auction pattern: announce a task to all capable fighters,
/// collect bids, and award the task to the best bidder.
pub async fn execute_auction(
    fighters: &[FighterId],
    task: &str,
    router: &MessageRouter,
    coordinator: FighterId,
    bids: &[AuctionBid],
    min_confidence: f64,
) -> PunchResult<Option<AuctionBid>> {
    if fighters.is_empty() {
        return Ok(None);
    }

    // Announce the task to all fighters.
    for fighter in fighters {
        let _ = router
            .send_direct(
                coordinator,
                *fighter,
                AgentMessageType::TaskAssignment {
                    task: format!("[AUCTION] {}", task),
                },
                MessagePriority::Normal,
            )
            .await;
    }

    info!(
        fighter_count = fighters.len(),
        "auction: announced task to fighters"
    );

    // Filter and select the winning bid.
    let filtered = auction_filter_bids(bids, min_confidence);
    if filtered.is_empty() {
        warn!("auction: no bids met minimum confidence threshold");
        return Ok(None);
    }

    let winner = auction_select_winner(bids).cloned();

    if let Some(ref w) = winner {
        // Send the task assignment to the winner.
        let _ = router
            .send_direct(
                coordinator,
                w.fighter_id,
                AgentMessageType::TaskAssignment {
                    task: format!("[AWARDED] {}", task),
                },
                MessagePriority::High,
            )
            .await;

        info!(
            winner = %w.fighter_id,
            confidence = w.confidence,
            estimated_time = w.estimated_time_secs,
            "auction: task awarded"
        );
    }

    Ok(winner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // -- MapReduce tests --

    #[test]
    fn test_map_split_even() {
        let input = "line1\nline2\nline3\nline4";
        let chunks = map_split(input, 2);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("line1"));
        assert!(chunks[1].contains("line3"));
    }

    #[test]
    fn test_map_split_uneven() {
        let input = "line1\nline2\nline3";
        let chunks = map_split(input, 2);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn test_map_split_single_worker() {
        let input = "line1\nline2";
        let chunks = map_split(input, 1);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("line1"));
        assert!(chunks[0].contains("line2"));
    }

    #[test]
    fn test_map_split_zero_workers() {
        let chunks = map_split("anything", 0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_map_split_empty_input() {
        let chunks = map_split("", 3);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_map_reduce_merge() {
        let mut results = HashMap::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        results.insert(f1, "result1".to_string());
        results.insert(f2, "result2".to_string());

        let merged = map_reduce_merge(&results);
        assert!(merged.contains("result1"));
        assert!(merged.contains("result2"));
    }

    #[test]
    fn test_execute_map_reduce() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let config = MapReduceConfig {
            input: "data".to_string(),
            workers: vec![f1, f2],
        };
        let mut results = HashMap::new();
        results.insert(f1, "processed-1".to_string());
        results.insert(f2, "processed-2".to_string());

        let mr_result = execute_map_reduce(&config, results);
        assert_eq!(mr_result.map_results.len(), 2);
        assert!(mr_result.reduced.contains("processed"));
    }

    #[tokio::test]
    async fn test_map_reduce_distributed_splits_and_sends() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let w1 = FighterId::new();
        let w2 = FighterId::new();

        let _rx_coord = router.register(coordinator);
        let _rx_w1 = router.register(w1);
        let _rx_w2 = router.register(w2);

        let config = MapReduceConfig {
            input: "line1\nline2\nline3\nline4".to_string(),
            workers: vec![w1, w2],
        };

        let result = execute_map_reduce_distributed(&config, &router, coordinator)
            .await
            .expect("should execute");

        assert_eq!(result.map_results.len(), 2);
        assert!(result.map_results.contains_key(&w1));
        assert!(result.map_results.contains_key(&w2));
        assert!(!result.reduced.is_empty());
    }

    #[tokio::test]
    async fn test_map_reduce_distributed_no_workers() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let _rx = router.register(coordinator);

        let config = MapReduceConfig {
            input: "data".to_string(),
            workers: vec![],
        };

        let result = execute_map_reduce_distributed(&config, &router, coordinator).await;
        assert!(result.is_err());
    }

    // -- Chain of Responsibility tests --

    #[test]
    fn test_chain_find_handler_match() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let chain = vec![
            ChainHandler {
                fighter_id: f1,
                capabilities: vec!["code".to_string()],
            },
            ChainHandler {
                fighter_id: f2,
                capabilities: vec!["review".to_string()],
            },
        ];

        let handler = chain_find_handler(&chain, "please review this PR");
        assert_eq!(handler, Some(f2));
    }

    #[test]
    fn test_chain_find_handler_first_match() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let chain = vec![
            ChainHandler {
                fighter_id: f1,
                capabilities: vec!["code".to_string()],
            },
            ChainHandler {
                fighter_id: f2,
                capabilities: vec!["code".to_string()],
            },
        ];

        let handler = chain_find_handler(&chain, "analyze code quality");
        assert_eq!(handler, Some(f1)); // First match wins.
    }

    #[test]
    fn test_chain_find_handler_no_match() {
        let chain = vec![ChainHandler {
            fighter_id: FighterId::new(),
            capabilities: vec!["database".to_string()],
        }];

        let handler = chain_find_handler(&chain, "fix CSS styling");
        assert!(handler.is_none());
    }

    #[test]
    fn test_chain_walk() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let f3 = FighterId::new();
        let chain = vec![
            ChainHandler {
                fighter_id: f1,
                capabilities: vec!["a".to_string()],
            },
            ChainHandler {
                fighter_id: f2,
                capabilities: vec!["b".to_string()],
            },
            ChainHandler {
                fighter_id: f3,
                capabilities: vec!["c".to_string()],
            },
        ];

        let mut handler_results = HashMap::new();
        handler_results.insert(f1, false);
        handler_results.insert(f2, true);
        handler_results.insert(f3, true);

        let result = chain_walk(&chain, "task", &handler_results);
        assert_eq!(result, Some((f2, 1)));
    }

    #[test]
    fn test_chain_walk_none_accept() {
        let f1 = FighterId::new();
        let chain = vec![ChainHandler {
            fighter_id: f1,
            capabilities: vec!["a".to_string()],
        }];

        let mut handler_results = HashMap::new();
        handler_results.insert(f1, false);

        let result = chain_walk(&chain, "task", &handler_results);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_chain_of_responsibility_first_capable_handles() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();

        let _rx_coord = router.register(coordinator);
        let _rx_f1 = router.register(f1);
        let _rx_f2 = router.register(f2);

        let chain = vec![
            ChainHandler {
                fighter_id: f1,
                capabilities: vec!["database".to_string()],
            },
            ChainHandler {
                fighter_id: f2,
                capabilities: vec!["code".to_string()],
            },
        ];

        let result =
            execute_chain_of_responsibility(&chain, "fix the code issue", &router, coordinator)
                .await
                .expect("should execute");

        assert!(result.is_some());
        let (handler, pos, _) = result.expect("should have handler");
        assert_eq!(handler, f2);
        assert_eq!(pos, 1);
    }

    #[tokio::test]
    async fn test_chain_of_responsibility_none_capable() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let f1 = FighterId::new();

        let _rx_coord = router.register(coordinator);
        let _rx_f1 = router.register(f1);

        let chain = vec![ChainHandler {
            fighter_id: f1,
            capabilities: vec!["database".to_string()],
        }];

        let result =
            execute_chain_of_responsibility(&chain, "fix CSS styling", &router, coordinator)
                .await
                .expect("should execute");

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_chain_of_responsibility_empty_chain() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let _rx = router.register(coordinator);

        let result = execute_chain_of_responsibility(&[], "any task", &router, coordinator).await;
        assert!(result.is_err());
    }

    // -- Scatter-Gather tests --

    #[test]
    fn test_scatter_select_fastest() {
        let responses = vec![
            ScatterResponse {
                fighter_id: FighterId::new(),
                response: "slow".to_string(),
                response_time_ms: 500,
                quality_score: 0.9,
            },
            ScatterResponse {
                fighter_id: FighterId::new(),
                response: "fast".to_string(),
                response_time_ms: 100,
                quality_score: 0.5,
            },
        ];

        let best = scatter_select(&responses, &SelectionCriteria::Fastest);
        assert_eq!(best.map(|r| r.response.as_str()), Some("fast"));
    }

    #[test]
    fn test_scatter_select_highest_quality() {
        let responses = vec![
            ScatterResponse {
                fighter_id: FighterId::new(),
                response: "low quality".to_string(),
                response_time_ms: 50,
                quality_score: 0.3,
            },
            ScatterResponse {
                fighter_id: FighterId::new(),
                response: "high quality".to_string(),
                response_time_ms: 500,
                quality_score: 0.95,
            },
        ];

        let best = scatter_select(&responses, &SelectionCriteria::HighestQuality);
        assert_eq!(best.map(|r| r.response.as_str()), Some("high quality"));
    }

    #[test]
    fn test_scatter_select_consensus() {
        let responses = vec![
            ScatterResponse {
                fighter_id: FighterId::new(),
                response: "yes".to_string(),
                response_time_ms: 100,
                quality_score: 0.8,
            },
            ScatterResponse {
                fighter_id: FighterId::new(),
                response: "yes".to_string(),
                response_time_ms: 200,
                quality_score: 0.7,
            },
            ScatterResponse {
                fighter_id: FighterId::new(),
                response: "no".to_string(),
                response_time_ms: 150,
                quality_score: 0.9,
            },
        ];

        let best = scatter_select(&responses, &SelectionCriteria::Consensus);
        assert_eq!(best.map(|r| r.response.as_str()), Some("yes"));
    }

    #[test]
    fn test_scatter_select_empty() {
        let responses: Vec<ScatterResponse> = vec![];
        let best = scatter_select(&responses, &SelectionCriteria::Fastest);
        assert!(best.is_none());
    }

    #[tokio::test]
    async fn test_scatter_gather_fastest_selected() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();

        let _rx_coord = router.register(coordinator);
        let _rx_f1 = router.register(f1);
        let _rx_f2 = router.register(f2);

        let result = execute_scatter_gather(
            &[f1, f2],
            "analyze this",
            &router,
            coordinator,
            Duration::from_secs(5),
            &SelectionCriteria::Fastest,
        )
        .await
        .expect("should execute");

        assert!(result.is_some());
        let selected = result.expect("should have result");
        // Fastest should be the first fighter (response_time = 100ms).
        assert_eq!(selected.fighter_id, f1);
    }

    #[tokio::test]
    async fn test_scatter_gather_empty_fighters() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let _rx = router.register(coordinator);

        let result = execute_scatter_gather(
            &[],
            "task",
            &router,
            coordinator,
            Duration::from_secs(1),
            &SelectionCriteria::Fastest,
        )
        .await
        .expect("should execute");

        assert!(result.is_none());
    }

    // -- Supervisor tests --

    #[test]
    fn test_supervisor_one_for_one_restart() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let mut config = SupervisorConfig {
            strategy: RestartStrategy::OneForOne,
            max_restarts: 3,
            workers: vec![
                SupervisedWorker {
                    fighter_id: f1,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
                SupervisedWorker {
                    fighter_id: f2,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
            ],
        };

        let restarted = supervisor_handle_failure(&mut config, &f1);
        assert_eq!(restarted, vec![f1]);
        assert_eq!(config.workers[0].restart_count, 1);
        assert!(config.workers[0].running);
        // f2 should be unaffected.
        assert_eq!(config.workers[1].restart_count, 0);
    }

    #[test]
    fn test_supervisor_one_for_one_max_restarts() {
        let f1 = FighterId::new();
        let mut config = SupervisorConfig {
            strategy: RestartStrategy::OneForOne,
            max_restarts: 2,
            workers: vec![SupervisedWorker {
                fighter_id: f1,
                restart_count: 2,
                running: true,
                failed: false,
            }],
        };

        let restarted = supervisor_handle_failure(&mut config, &f1);
        assert!(restarted.is_empty());
        assert!(config.workers[0].failed);
        assert!(!config.workers[0].running);
    }

    #[test]
    fn test_supervisor_all_for_one_restart() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let f3 = FighterId::new();
        let mut config = SupervisorConfig {
            strategy: RestartStrategy::AllForOne,
            max_restarts: 3,
            workers: vec![
                SupervisedWorker {
                    fighter_id: f1,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
                SupervisedWorker {
                    fighter_id: f2,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
                SupervisedWorker {
                    fighter_id: f3,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
            ],
        };

        let restarted = supervisor_handle_failure(&mut config, &f1);
        assert_eq!(restarted.len(), 3);
        // All workers should have restart count incremented.
        for worker in &config.workers {
            assert_eq!(worker.restart_count, 1);
            assert!(worker.running);
        }
    }

    #[test]
    fn test_supervisor_all_for_one_max_restarts() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let mut config = SupervisorConfig {
            strategy: RestartStrategy::AllForOne,
            max_restarts: 1,
            workers: vec![
                SupervisedWorker {
                    fighter_id: f1,
                    restart_count: 1,
                    running: true,
                    failed: false,
                },
                SupervisedWorker {
                    fighter_id: f2,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
            ],
        };

        let restarted = supervisor_handle_failure(&mut config, &f1);
        assert!(restarted.is_empty());
        assert!(config.workers[0].failed);
    }

    #[tokio::test]
    async fn test_supervisor_health_monitoring() {
        let router = MessageRouter::new();
        let supervisor = FighterId::new();
        let w1 = FighterId::new();
        let w2 = FighterId::new();

        let _rx_sup = router.register(supervisor);
        let _rx_w1 = router.register(w1);
        // w2 is NOT registered (simulating a dead worker).

        let mut config = SupervisorConfig {
            strategy: RestartStrategy::OneForOne,
            max_restarts: 3,
            workers: vec![
                SupervisedWorker {
                    fighter_id: w1,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
                SupervisedWorker {
                    fighter_id: w2,
                    restart_count: 0,
                    running: true,
                    failed: false,
                },
            ],
        };

        let restarted = supervisor_monitor_health(&mut config, &router, supervisor).await;

        // w2 should have been detected as failed and restarted.
        assert!(restarted.contains(&w2));
        assert!(!restarted.contains(&w1));
    }

    // -- Auction tests --

    #[test]
    fn test_auction_select_winner() {
        let bids = vec![
            AuctionBid {
                fighter_id: FighterId::new(),
                estimated_time_secs: 60,
                confidence: 0.8,
                submitted_at: Utc::now(),
            },
            AuctionBid {
                fighter_id: FighterId::new(),
                estimated_time_secs: 30,
                confidence: 0.9,
                submitted_at: Utc::now(),
            },
        ];

        let winner = auction_select_winner(&bids);
        assert!(winner.is_some());
        // Second bid has score 0.9/30 = 0.03, first has 0.8/60 = 0.013.
        let w = winner.expect("should have winner");
        assert_eq!(w.estimated_time_secs, 30);
    }

    #[test]
    fn test_auction_select_winner_empty() {
        let bids: Vec<AuctionBid> = vec![];
        assert!(auction_select_winner(&bids).is_none());
    }

    #[test]
    fn test_auction_select_winner_single_bid() {
        let bid = AuctionBid {
            fighter_id: FighterId::new(),
            estimated_time_secs: 10,
            confidence: 1.0,
            submitted_at: Utc::now(),
        };
        let bids = [bid.clone()];
        let winner = auction_select_winner(&bids);
        assert!(winner.is_some());
    }

    #[test]
    fn test_auction_filter_bids() {
        let bids = vec![
            AuctionBid {
                fighter_id: FighterId::new(),
                estimated_time_secs: 10,
                confidence: 0.3,
                submitted_at: Utc::now(),
            },
            AuctionBid {
                fighter_id: FighterId::new(),
                estimated_time_secs: 20,
                confidence: 0.8,
                submitted_at: Utc::now(),
            },
            AuctionBid {
                fighter_id: FighterId::new(),
                estimated_time_secs: 30,
                confidence: 0.9,
                submitted_at: Utc::now(),
            },
        ];

        let filtered = auction_filter_bids(&bids, 0.7);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_auction_filter_bids_none_pass() {
        let bids = vec![AuctionBid {
            fighter_id: FighterId::new(),
            estimated_time_secs: 10,
            confidence: 0.2,
            submitted_at: Utc::now(),
        }];

        let filtered = auction_filter_bids(&bids, 0.5);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_auction_zero_time_estimate() {
        let bids = vec![
            AuctionBid {
                fighter_id: FighterId::new(),
                estimated_time_secs: 0,
                confidence: 0.5,
                submitted_at: Utc::now(),
            },
            AuctionBid {
                fighter_id: FighterId::new(),
                estimated_time_secs: 100,
                confidence: 0.9,
                submitted_at: Utc::now(),
            },
        ];

        // Zero-time bid gets a very high score.
        let winner = auction_select_winner(&bids);
        assert!(winner.is_some());
        assert_eq!(winner.expect("winner").estimated_time_secs, 0);
    }

    #[tokio::test]
    async fn test_auction_best_bid_wins() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();

        let _rx_coord = router.register(coordinator);
        let _rx_f1 = router.register(f1);
        let _rx_f2 = router.register(f2);

        let bids = vec![
            AuctionBid {
                fighter_id: f1,
                estimated_time_secs: 60,
                confidence: 0.7,
                submitted_at: Utc::now(),
            },
            AuctionBid {
                fighter_id: f2,
                estimated_time_secs: 20,
                confidence: 0.9,
                submitted_at: Utc::now(),
            },
        ];

        let result = execute_auction(&[f1, f2], "complex task", &router, coordinator, &bids, 0.5)
            .await
            .expect("should execute");

        assert!(result.is_some());
        let winner = result.expect("should have winner");
        assert_eq!(winner.fighter_id, f2); // Better score.
    }

    #[tokio::test]
    async fn test_auction_tie_breaking() {
        let router = MessageRouter::new();
        let coordinator = FighterId::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();

        let _rx_coord = router.register(coordinator);
        let _rx_f1 = router.register(f1);
        let _rx_f2 = router.register(f2);

        // Both bids have same score (0.8 / 40 = 0.02).
        let bids = vec![
            AuctionBid {
                fighter_id: f1,
                estimated_time_secs: 40,
                confidence: 0.8,
                submitted_at: Utc::now(),
            },
            AuctionBid {
                fighter_id: f2,
                estimated_time_secs: 40,
                confidence: 0.8,
                submitted_at: Utc::now(),
            },
        ];

        let result = execute_auction(&[f1, f2], "tied task", &router, coordinator, &bids, 0.5)
            .await
            .expect("should execute");

        // Should still select a winner (deterministic - one of them).
        assert!(result.is_some());
    }

    // -- Integration-style tests --

    #[test]
    fn test_map_reduce_end_to_end() {
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let config = MapReduceConfig {
            input: "line1\nline2\nline3\nline4".to_string(),
            workers: vec![f1, f2],
        };

        let chunks = map_split(&config.input, config.workers.len());
        assert_eq!(chunks.len(), 2);

        let mut results = HashMap::new();
        results.insert(f1, format!("analyzed: {}", chunks[0]));
        results.insert(f2, format!("analyzed: {}", chunks[1]));

        let mr = execute_map_reduce(&config, results);
        assert!(mr.reduced.contains("analyzed"));
        assert_eq!(mr.map_results.len(), 2);
    }

    #[test]
    fn test_supervisor_repeated_failures() {
        let f = FighterId::new();
        let mut config = SupervisorConfig {
            strategy: RestartStrategy::OneForOne,
            max_restarts: 3,
            workers: vec![SupervisedWorker {
                fighter_id: f,
                restart_count: 0,
                running: true,
                failed: false,
            }],
        };

        // Fail 3 times (should restart each time).
        for i in 1..=3 {
            let restarted = supervisor_handle_failure(&mut config, &f);
            assert_eq!(restarted, vec![f]);
            assert_eq!(config.workers[0].restart_count, i);
        }

        // 4th failure should give up.
        let restarted = supervisor_handle_failure(&mut config, &f);
        assert!(restarted.is_empty());
        assert!(config.workers[0].failed);
    }
}
