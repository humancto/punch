//! Integration tests for multi-agent coordination: swarm decomposition,
//! task assignment, load balancing, subtask completion, failure handling,
//! and capability-aware routing.

use punch_kernel::SwarmCoordinator;
use punch_types::{FighterId, SubtaskStatus, SwarmSubtask};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Swarm task decomposition
// ---------------------------------------------------------------------------

/// Decompose a multi-paragraph task into subtasks.
#[test]
fn test_decompose_paragraphs_into_subtasks() {
    let coord = SwarmCoordinator::new();
    let input = "First, analyze the codebase.\n\nSecond, identify bugs.\n\nThird, write fixes.";
    let subtasks = coord.decompose_task(input);

    assert_eq!(subtasks.len(), 3);
    assert!(subtasks[0].description.contains("analyze"));
    assert!(subtasks[1].description.contains("bugs"));
    assert!(subtasks[2].description.contains("fixes"));

    // All subtasks should start as Pending.
    for s in &subtasks {
        assert_eq!(s.status, SubtaskStatus::Pending);
        assert!(s.assigned_to.is_none());
    }
}

/// Single-line input produces a single subtask.
#[test]
fn test_decompose_atomic_task() {
    let coord = SwarmCoordinator::new();
    let subtasks = coord.decompose_task("run the test suite");
    assert_eq!(subtasks.len(), 1);
    assert_eq!(subtasks[0].description, "run the test suite");
}

/// Empty input still produces one subtask.
#[test]
fn test_decompose_empty_input() {
    let coord = SwarmCoordinator::new();
    let subtasks = coord.decompose_task("");
    assert_eq!(subtasks.len(), 1);
}

// ---------------------------------------------------------------------------
// Task creation and assignment
// ---------------------------------------------------------------------------

/// Create a task and assign subtasks to registered fighters.
#[tokio::test]
async fn test_create_task_and_assign() {
    let coord = SwarmCoordinator::new();
    let f1 = FighterId::new();
    let f2 = FighterId::new();
    coord.register_fighter(f1);
    coord.register_fighter(f2);

    let task_id = coord.create_task("step 1\nstep 2\nstep 3".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();

    assert_eq!(assignments.len(), 3);

    // All assignments should reference registered fighters.
    for (_, fighter) in &assignments {
        assert!(*fighter == f1 || *fighter == f2);
    }
}

/// Assignment with no available fighters returns empty.
#[tokio::test]
async fn test_assign_no_fighters_available() {
    let coord = SwarmCoordinator::new();
    let task_id = coord.create_task("lonely task".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();
    assert!(assignments.is_empty());
}

/// Assignment for a non-existent task returns an error.
#[tokio::test]
async fn test_assign_nonexistent_task_errors() {
    let coord = SwarmCoordinator::new();
    let result = coord.assign_subtasks(&Uuid::new_v4()).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Load balancing
// ---------------------------------------------------------------------------

/// Tasks are distributed evenly across fighters (load balancing).
#[tokio::test]
async fn test_load_balancing_even_distribution() {
    let coord = SwarmCoordinator::new();
    let f1 = FighterId::new();
    let f2 = FighterId::new();
    let f3 = FighterId::new();
    coord.register_fighter(f1);
    coord.register_fighter(f2);
    coord.register_fighter(f3);

    let task_id = coord.create_task("a\nb\nc\nd\ne\nf".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();
    assert_eq!(assignments.len(), 6);

    let mut counts = std::collections::HashMap::new();
    for (_, fighter) in &assignments {
        *counts.entry(*fighter).or_insert(0usize) += 1;
    }
    // Each fighter should get 2 tasks.
    for count in counts.values() {
        assert_eq!(*count, 2, "each fighter should get an equal share");
    }
}

/// Unhealthy fighters are skipped during assignment.
#[tokio::test]
async fn test_unhealthy_fighter_skipped() {
    let coord = SwarmCoordinator::new();
    let healthy = FighterId::new();
    let unhealthy = FighterId::new();
    coord.register_fighter(healthy);
    coord.register_fighter(unhealthy);
    coord.mark_unhealthy(&unhealthy);

    let task_id = coord.create_task("work".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();

    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].1, healthy);
}

// ---------------------------------------------------------------------------
// Subtask completion and failure
// ---------------------------------------------------------------------------

/// Complete all subtasks and verify progress reaches 100%.
#[tokio::test]
async fn test_complete_all_subtasks_progress_100() {
    let coord = SwarmCoordinator::new();
    let f = FighterId::new();
    coord.register_fighter(f);

    let task_id = coord.create_task("a\nb".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();

    for (subtask_id, _) in &assignments {
        coord
            .complete_subtask(&task_id, subtask_id, "done".to_string())
            .await
            .unwrap();
    }

    let progress = coord.get_progress(&task_id).await.unwrap();
    assert!((progress - 1.0).abs() < f64::EPSILON);

    let task = coord.get_task(&task_id).await.unwrap();
    assert!(task.aggregated_result.is_some());
}

/// Fail a subtask and verify status.
#[tokio::test]
async fn test_fail_subtask_updates_status() {
    let coord = SwarmCoordinator::new();
    let f = FighterId::new();
    coord.register_fighter(f);

    let task_id = coord.create_task("failing work".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();
    let (subtask_id, _) = assignments[0];

    coord
        .fail_subtask(&task_id, &subtask_id, "timeout".to_string())
        .await
        .unwrap();

    let task = coord.get_task(&task_id).await.unwrap();
    assert!(matches!(task.subtasks[0].status, SubtaskStatus::Failed(_)));
}

/// Reassign a failed subtask to a different fighter.
#[tokio::test]
async fn test_reassign_failed_subtask() {
    let coord = SwarmCoordinator::new();
    let f1 = FighterId::new();
    let f2 = FighterId::new();
    coord.register_fighter(f1);
    coord.register_fighter(f2);

    let task_id = coord.create_task("single task".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();
    let (subtask_id, original) = assignments[0];

    coord
        .fail_subtask(&task_id, &subtask_id, "crashed".to_string())
        .await
        .unwrap();

    let new_fighter = coord
        .reassign_failed_subtask(&task_id, &subtask_id)
        .await
        .unwrap();

    assert!(new_fighter.is_some());
    assert_ne!(new_fighter.unwrap(), original);
}

// ---------------------------------------------------------------------------
// Capability-aware routing
// ---------------------------------------------------------------------------

/// Register fighters with capabilities and verify routing by capability.
#[tokio::test]
async fn test_capability_aware_routing() {
    let coord = SwarmCoordinator::new();
    let coder = FighterId::new();
    let reviewer = FighterId::new();
    coord.register_fighter_with_capabilities(coder, vec!["code".to_string()]);
    coord.register_fighter_with_capabilities(reviewer, vec!["review".to_string()]);

    let subtasks = vec![SwarmSubtask {
        id: Uuid::new_v4(),
        description: "fix the code bug".to_string(),
        assigned_to: None,
        status: SubtaskStatus::Pending,
        result: None,
        depends_on: vec![],
    }];
    let task_id = coord.create_task_with_subtasks("code task".to_string(), subtasks);
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();

    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].1, coder, "code task should go to coder");
}

// ---------------------------------------------------------------------------
// Dependency-aware assignment
// ---------------------------------------------------------------------------

/// Subtasks with unmet dependencies are not assigned.
#[tokio::test]
async fn test_dependency_blocks_assignment() {
    let coord = SwarmCoordinator::new();
    let f = FighterId::new();
    coord.register_fighter(f);

    let dep_id = Uuid::new_v4();
    let subtasks = vec![
        SwarmSubtask {
            id: dep_id,
            description: "first".to_string(),
            assigned_to: None,
            status: SubtaskStatus::Pending,
            result: None,
            depends_on: vec![],
        },
        SwarmSubtask {
            id: Uuid::new_v4(),
            description: "second".to_string(),
            assigned_to: None,
            status: SubtaskStatus::Pending,
            result: None,
            depends_on: vec![dep_id],
        },
    ];
    let task_id = coord.create_task_with_subtasks("pipeline".to_string(), subtasks);

    // Only the first subtask should be assignable.
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0].0, dep_id);
}

// ---------------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------------

/// Verify detailed progress report after partial completion.
#[tokio::test]
async fn test_progress_report_partial() {
    let coord = SwarmCoordinator::new();
    let f = FighterId::new();
    coord.register_fighter(f);

    let task_id = coord.create_task("a\nb\nc".to_string());
    let assignments = coord.assign_subtasks(&task_id).await.unwrap();

    // Complete only the first subtask.
    coord
        .complete_subtask(&task_id, &assignments[0].0, "done".to_string())
        .await
        .unwrap();

    let report = coord.get_progress_report(&task_id).await.unwrap();
    assert_eq!(report.total_subtasks, 3);
    assert_eq!(report.completed, 1);
    assert_eq!(report.running, 2);
    assert_eq!(report.pending, 0);
    assert_eq!(report.failed, 0);
}

// ---------------------------------------------------------------------------
// Fighter health management
// ---------------------------------------------------------------------------

/// Mark fighter unhealthy then healthy again.
#[test]
fn test_mark_unhealthy_then_healthy() {
    let coord = SwarmCoordinator::new();
    let f = FighterId::new();
    coord.register_fighter(f);
    assert_eq!(coord.available_fighter_count(), 1);

    coord.mark_unhealthy(&f);
    assert_eq!(coord.available_fighter_count(), 0);

    coord.mark_healthy(&f);
    assert_eq!(coord.available_fighter_count(), 1);
}

/// Unregister a fighter removes it from the pool.
#[test]
fn test_unregister_fighter() {
    let coord = SwarmCoordinator::new();
    let f = FighterId::new();
    coord.register_fighter(f);
    assert_eq!(coord.available_fighter_count(), 1);

    coord.unregister_fighter(&f);
    assert_eq!(coord.available_fighter_count(), 0);
}
