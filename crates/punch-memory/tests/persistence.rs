//! Integration tests for memory persistence: bout messages, key-value memories,
//! memory recall ordering, consolidation, migration engine, and backup/restore.

use std::path::Path;
use std::sync::Arc;

use punch_memory::{
    BackupManager, ConsolidationConfig, MemoryConsolidator, MemorySubstrate, Migration,
    MigrationEngine,
};
use punch_types::{
    FighterId, FighterManifest, FighterStatus, Message, ModelConfig, Provider, Role, WeightClass,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_manifest() -> FighterManifest {
    FighterManifest {
        name: "PersistTest".into(),
        description: "persistence test fighter".into(),
        model: ModelConfig {
            provider: Provider::Ollama,
            model: "test-model".into(),
            api_key_env: None,
            base_url: Some("http://localhost:11434".into()),
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
        system_prompt: "test".into(),
        capabilities: Vec::new(),
        weight_class: WeightClass::Featherweight,
        tenant_id: None,
    }
}

async fn setup() -> (MemorySubstrate, FighterId) {
    let substrate = MemorySubstrate::in_memory().expect("in-memory substrate");
    let fid = FighterId::new();
    substrate
        .save_fighter(&fid, &test_manifest(), FighterStatus::Idle)
        .await
        .expect("save fighter");
    (substrate, fid)
}

// ---------------------------------------------------------------------------
// Bout message roundtrip tests
// ---------------------------------------------------------------------------

/// Save messages to a bout, load them, and verify content roundtrips.
#[tokio::test]
async fn test_bout_message_roundtrip() {
    let (substrate, fid) = setup().await;
    let bout_id = substrate.create_bout(&fid).await.unwrap();

    let user_msg = Message::new(Role::User, "What is Rust?");
    let assistant_msg = Message::new(Role::Assistant, "Rust is a systems programming language.");

    substrate.save_message(&bout_id, &user_msg).await.unwrap();
    substrate
        .save_message(&bout_id, &assistant_msg)
        .await
        .unwrap();

    let loaded = substrate.load_messages(&bout_id).await.unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].role, Role::User);
    assert_eq!(loaded[0].content, "What is Rust?");
    assert_eq!(loaded[1].role, Role::Assistant);
    assert_eq!(loaded[1].content, "Rust is a systems programming language.");
}

/// Multiple bouts for the same fighter stay isolated.
#[tokio::test]
async fn test_multiple_bouts_isolated() {
    let (substrate, fid) = setup().await;

    let bout1 = substrate.create_bout(&fid).await.unwrap();
    let bout2 = substrate.create_bout(&fid).await.unwrap();

    substrate
        .save_message(&bout1, &Message::new(Role::User, "bout1 msg"))
        .await
        .unwrap();
    substrate
        .save_message(&bout2, &Message::new(Role::User, "bout2 msg"))
        .await
        .unwrap();

    let msgs1 = substrate.load_messages(&bout1).await.unwrap();
    let msgs2 = substrate.load_messages(&bout2).await.unwrap();

    assert_eq!(msgs1.len(), 1);
    assert_eq!(msgs2.len(), 1);
    assert_eq!(msgs1[0].content, "bout1 msg");
    assert_eq!(msgs2[0].content, "bout2 msg");
}

/// Deleting a bout removes its messages too.
#[tokio::test]
async fn test_delete_bout_removes_messages() {
    let (substrate, fid) = setup().await;
    let bout_id = substrate.create_bout(&fid).await.unwrap();
    substrate
        .save_message(&bout_id, &Message::new(Role::User, "ephemeral"))
        .await
        .unwrap();

    substrate.delete_bout(&bout_id).await.unwrap();

    let bouts = substrate.list_bouts(&fid).await.unwrap();
    assert!(bouts.is_empty());
}

// ---------------------------------------------------------------------------
// Key-value memory tests
// ---------------------------------------------------------------------------

/// Store a memory, recall by key, and verify match.
#[tokio::test]
async fn test_store_recall_memory_by_key() {
    let (substrate, fid) = setup().await;

    substrate
        .store_memory(&fid, "favorite_language", "Rust", 0.95)
        .await
        .unwrap();

    let results = substrate
        .recall_memories(&fid, "favorite_language", 10)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "favorite_language");
    assert_eq!(results[0].value, "Rust");
    assert!((results[0].confidence - 0.95).abs() < f64::EPSILON);
}

/// Store multiple memories and verify recall orders by confidence.
#[tokio::test]
async fn test_recall_orders_by_confidence() {
    let (substrate, fid) = setup().await;

    substrate
        .store_memory(&fid, "fact_low", "trivia", 0.2)
        .await
        .unwrap();
    substrate
        .store_memory(&fid, "fact_high", "important", 0.9)
        .await
        .unwrap();
    substrate
        .store_memory(&fid, "fact_mid", "moderate", 0.5)
        .await
        .unwrap();

    let results = substrate.recall_memories(&fid, "fact", 10).await.unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].key, "fact_high");
    assert_eq!(results[1].key, "fact_mid");
    assert_eq!(results[2].key, "fact_low");
}

/// Overwriting a key updates the value and confidence.
#[tokio::test]
async fn test_memory_overwrite() {
    let (substrate, fid) = setup().await;

    substrate
        .store_memory(&fid, "pref", "old_value", 0.5)
        .await
        .unwrap();
    substrate
        .store_memory(&fid, "pref", "new_value", 0.9)
        .await
        .unwrap();

    let results = substrate.recall_memories(&fid, "pref", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].value, "new_value");
    assert!((results[0].confidence - 0.9).abs() < f64::EPSILON);
}

/// Decay memories and verify low-confidence entries are pruned.
#[tokio::test]
async fn test_decay_prunes_low_confidence() {
    let (substrate, fid) = setup().await;

    substrate
        .store_memory(&fid, "strong_mem", "important", 1.0)
        .await
        .unwrap();
    substrate
        .store_memory(&fid, "weak_mem", "trivial", 0.02)
        .await
        .unwrap();

    // Heavy decay: 0.02 * (1.0 - 0.9) = 0.002 < 0.01 threshold => pruned
    substrate.decay_memories(&fid, 0.9).await.unwrap();

    let weak = substrate
        .recall_memories(&fid, "weak_mem", 10)
        .await
        .unwrap();
    assert!(weak.is_empty(), "weak memory should be pruned after decay");

    let strong = substrate
        .recall_memories(&fid, "strong_mem", 10)
        .await
        .unwrap();
    assert_eq!(strong.len(), 1, "strong memory should survive decay");
}

/// Delete a specific memory by key.
#[tokio::test]
async fn test_delete_specific_memory() {
    let (substrate, fid) = setup().await;

    substrate
        .store_memory(&fid, "to_delete", "data", 0.9)
        .await
        .unwrap();
    substrate
        .store_memory(&fid, "to_keep", "data", 0.9)
        .await
        .unwrap();

    substrate.delete_memory(&fid, "to_delete").await.unwrap();

    let deleted = substrate
        .recall_memories(&fid, "to_delete", 10)
        .await
        .unwrap();
    assert!(deleted.is_empty());

    let kept = substrate
        .recall_memories(&fid, "to_keep", 10)
        .await
        .unwrap();
    assert_eq!(kept.len(), 1);
}

// ---------------------------------------------------------------------------
// Consolidation tests
// ---------------------------------------------------------------------------

/// Add many memories with low confidence, consolidate, verify count reduced.
#[tokio::test]
async fn test_consolidation_reduces_memory_count() {
    let (substrate, fid) = setup().await;

    // Store 30 memories: 10 with low confidence, 20 with high.
    for i in 0..10 {
        substrate
            .store_memory(
                &fid,
                &format!("low_{i}"),
                &format!("low_val_{i}"),
                0.1, // Below default min_confidence (0.3)
            )
            .await
            .unwrap();
    }
    for i in 0..20 {
        substrate
            .store_memory(
                &fid,
                &format!("high_{i}"),
                &format!("high_val_{i}"),
                0.8,
            )
            .await
            .unwrap();
    }

    let consolidator = MemoryConsolidator::new(ConsolidationConfig {
        max_memories_per_fighter: 100,
        consolidation_threshold: 10,
        min_confidence: 0.3,
        decay_rate: 0.0, // No decay for this test
        merge_similarity_threshold: 0.8,
        max_age_days: 90,
    });

    let result = consolidator.consolidate(&substrate, &fid).await.unwrap();

    assert_eq!(result.memories_before, 30);
    assert!(
        result.memories_after < result.memories_before,
        "consolidation should reduce count: before={}, after={}",
        result.memories_before,
        result.memories_after
    );
    assert!(
        result.pruned > 0,
        "should have pruned low-confidence memories"
    );
}

/// Consolidation preserves high-confidence memories.
#[tokio::test]
async fn test_consolidation_preserves_strong_memories() {
    let (substrate, fid) = setup().await;

    substrate
        .store_memory(&fid, "important_fact", "critical data", 0.99)
        .await
        .unwrap();
    substrate
        .store_memory(&fid, "garbage", "trash", 0.05)
        .await
        .unwrap();

    let consolidator = MemoryConsolidator::new(ConsolidationConfig {
        min_confidence: 0.3,
        decay_rate: 0.0,
        ..ConsolidationConfig::default()
    });

    consolidator.consolidate(&substrate, &fid).await.unwrap();

    let important = substrate
        .recall_memories(&fid, "important_fact", 10)
        .await
        .unwrap();
    assert_eq!(important.len(), 1, "important memory should survive");

    let garbage = substrate
        .recall_memories(&fid, "garbage", 10)
        .await
        .unwrap();
    assert!(garbage.is_empty(), "garbage memory should be pruned");
}

// ---------------------------------------------------------------------------
// Migration engine tests
// ---------------------------------------------------------------------------

/// Run all built-in migrations and verify schema is created.
#[tokio::test]
async fn test_builtin_migrations_create_schema() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    let arc = Arc::new(std::sync::Mutex::new(conn));
    let engine = MigrationEngine::new(Arc::clone(&arc)).unwrap();

    let builtins = MigrationEngine::builtin_migrations();
    let applied = engine.migrate_up(&builtins).unwrap();

    assert_eq!(applied.len(), 10, "should apply all 10 built-in migrations");
    assert_eq!(engine.current_version().unwrap(), 10);

    // Verify core tables exist.
    let c = arc.lock().unwrap();
    let tables: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert!(tables.contains(&"memories".to_string()));
    assert!(tables.contains(&"bouts".to_string()));
    assert!(tables.contains(&"messages".to_string()));
    assert!(tables.contains(&"fighters".to_string()));
    assert!(tables.contains(&"knowledge_entities".to_string()));
    assert!(tables.contains(&"knowledge_relations".to_string()));
}

/// Migrate up then down, verify rollback removes tables.
#[tokio::test]
async fn test_migration_up_then_down() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let arc = Arc::new(std::sync::Mutex::new(conn));
    let engine = MigrationEngine::new(Arc::clone(&arc)).unwrap();

    let migrations = vec![
        Migration {
            version: 1,
            name: "create_test_table".into(),
            up_sql: "CREATE TABLE test_tbl (id INTEGER PRIMARY KEY, val TEXT);".into(),
            down_sql: "DROP TABLE IF EXISTS test_tbl;".into(),
        },
        Migration {
            version: 2,
            name: "create_test_table2".into(),
            up_sql: "CREATE TABLE test_tbl2 (id INTEGER PRIMARY KEY);".into(),
            down_sql: "DROP TABLE IF EXISTS test_tbl2;".into(),
        },
    ];

    engine.migrate_up(&migrations).unwrap();
    assert_eq!(engine.current_version().unwrap(), 2);

    let rolled_back = engine.migrate_down(&migrations, 0).unwrap();
    assert_eq!(rolled_back, vec![2, 1]);
    assert_eq!(engine.current_version().unwrap(), 0);
}

/// Checksum verification detects tampered SQL.
#[tokio::test]
async fn test_migration_checksum_detects_tampering() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let arc = Arc::new(std::sync::Mutex::new(conn));
    let engine = MigrationEngine::new(Arc::clone(&arc)).unwrap();

    let migrations = vec![Migration {
        version: 1,
        name: "original".into(),
        up_sql: "CREATE TABLE orig (id INTEGER);".into(),
        down_sql: "DROP TABLE IF EXISTS orig;".into(),
    }];

    engine.migrate_up(&migrations).unwrap();

    // Tamper with the SQL content.
    let tampered = vec![Migration {
        version: 1,
        name: "original".into(),
        up_sql: "CREATE TABLE orig (id INTEGER, extra TEXT);".into(),
        down_sql: "DROP TABLE IF EXISTS orig;".into(),
    }];

    let result = engine.verify_checksums(&tampered);
    assert!(result.is_err(), "should detect tampered migration SQL");
}

// ---------------------------------------------------------------------------
// Backup / restore tests
// ---------------------------------------------------------------------------

/// Create a backup of a SQLite database and verify it exists and has data.
#[tokio::test]
async fn test_backup_create_and_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let backup_dir = dir.path().join("backups");

    // Create a minimal SQLite database.
    let conn = rusqlite::Connection::open(&db_path).expect("create db");
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE data (id INTEGER PRIMARY KEY, value TEXT);
         INSERT INTO data (value) VALUES ('test_data');",
    )
    .expect("init db");
    drop(conn);

    let mgr = BackupManager::new(db_path, backup_dir);

    // Initially empty.
    let list = mgr.list_backups().await.unwrap();
    assert!(list.is_empty());

    // Create backup.
    let info = mgr.create_backup().await.unwrap();
    assert!(info.path.exists());
    assert!(info.size_bytes > 0);
    assert!(info.id.starts_with("punch_backup_"));

    // Listed backup appears.
    let list = mgr.list_backups().await.unwrap();
    assert_eq!(list.len(), 1);
}

/// Backup cleanup removes old backups beyond the retention limit.
#[tokio::test]
async fn test_backup_cleanup_rotation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let backup_dir = dir.path().join("backups");

    let conn = rusqlite::Connection::open(&db_path).expect("create db");
    conn.execute_batch("CREATE TABLE t (id INTEGER);")
        .expect("init");
    drop(conn);

    // Create 4 backups manually.
    std::fs::create_dir_all(&backup_dir).unwrap();
    for i in 0..4 {
        let name = format!("punch_backup_20260101_12000{}.db", i);
        let path = backup_dir.join(&name);
        let c = rusqlite::Connection::open(&path).expect("create");
        c.execute_batch("CREATE TABLE t (id INTEGER);")
            .expect("init");
    }

    let mgr = BackupManager::new(db_path, backup_dir).with_max_backups(2);
    let removed = mgr.cleanup_old_backups().await.unwrap();
    assert_eq!(removed, 2);

    let remaining = mgr.list_backups().await.unwrap();
    assert_eq!(remaining.len(), 2);
}
