//! Unit tests for learning management.
//!
//! Tests cover:
//! - Add/list/delete learnings
//! - Task association
//! - CASCADE delete with tasks
//! - Source task tracking

use overseer::core::TaskService;
use overseer::db::{learning_repo, schema};
use overseer::error::OsError;
use overseer::types::CreateTaskInput;
use rusqlite::Connection;

fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    schema::init_schema(&conn).unwrap();
    conn
}

// ==================== Basic Operations ====================

#[test]
fn test_add_learning() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    let task = service
        .create(&CreateTaskInput {
            description: "Task".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let learning =
        learning_repo::add_learning(&conn, &task.id, "Always validate input", None).unwrap();

    assert_eq!(learning.task_id, task.id);
    assert_eq!(learning.content, "Always validate input");
    // When source not provided, origin defaults to task_id (self-origin)
    assert_eq!(learning.source_task_id, Some(task.id));
}

#[test]
fn test_add_learning_with_source() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    let source_task = service
        .create(&CreateTaskInput {
            description: "Source Task".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let task = service
        .create(&CreateTaskInput {
            description: "Task".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let learning = learning_repo::add_learning(
        &conn,
        &task.id,
        "Learned from source task",
        Some(&source_task.id),
    )
    .unwrap();

    assert_eq!(learning.source_task_id, Some(source_task.id));
}

#[test]
fn test_list_learnings() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    let task = service
        .create(&CreateTaskInput {
            description: "Task".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    learning_repo::add_learning(&conn, &task.id, "Learning 1", None).unwrap();
    learning_repo::add_learning(&conn, &task.id, "Learning 2", None).unwrap();
    learning_repo::add_learning(&conn, &task.id, "Learning 3", None).unwrap();

    let learnings = learning_repo::list_learnings(&conn, &task.id).unwrap();
    assert_eq!(learnings.len(), 3);
    assert_eq!(learnings[0].content, "Learning 1");
    assert_eq!(learnings[1].content, "Learning 2");
    assert_eq!(learnings[2].content, "Learning 3");
}

#[test]
fn test_list_learnings_empty() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    let task = service
        .create(&CreateTaskInput {
            description: "Task".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let learnings = learning_repo::list_learnings(&conn, &task.id).unwrap();
    assert_eq!(learnings.len(), 0);
}

#[test]
fn test_delete_learning() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    let task = service
        .create(&CreateTaskInput {
            description: "Task".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let learning = learning_repo::add_learning(&conn, &task.id, "Learning", None).unwrap();

    learning_repo::delete_learning(&conn, &learning.id).unwrap();

    let result = learning_repo::get_learning(&conn, &learning.id).unwrap();
    assert!(result.is_none());
}

// ==================== CASCADE Delete ====================

#[test]
fn test_cascade_delete_learnings_with_task() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    let task = service
        .create(&CreateTaskInput {
            description: "Task".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let learning1 = learning_repo::add_learning(&conn, &task.id, "Learning 1", None).unwrap();
    let learning2 = learning_repo::add_learning(&conn, &task.id, "Learning 2", None).unwrap();

    // Delete task - learnings should be CASCADE deleted
    service.delete(&task.id).unwrap();

    let result1 = learning_repo::get_learning(&conn, &learning1.id).unwrap();
    let result2 = learning_repo::get_learning(&conn, &learning2.id).unwrap();

    assert!(result1.is_none());
    assert!(result2.is_none());
}

#[test]
fn test_cascade_delete_learnings_with_parent_task() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    // Create parent → child
    let parent = service
        .create(&CreateTaskInput {
            description: "Parent".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let child = service
        .create(&CreateTaskInput {
            description: "Child".to_string(),
            context: None,
            parent_id: Some(parent.id.clone()),
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let parent_learning =
        learning_repo::add_learning(&conn, &parent.id, "Parent learning", None).unwrap();
    let child_learning =
        learning_repo::add_learning(&conn, &child.id, "Child learning", None).unwrap();

    // Delete parent - child and both learnings should be CASCADE deleted
    service.delete(&parent.id).unwrap();

    assert!(matches!(
        service.get(&child.id),
        Err(OsError::TaskNotFound(_))
    ));
    assert!(learning_repo::get_learning(&conn, &parent_learning.id)
        .unwrap()
        .is_none());
    assert!(learning_repo::get_learning(&conn, &child_learning.id)
        .unwrap()
        .is_none());
}

// ==================== Task Association ====================

#[test]
fn test_learnings_isolated_by_task() {
    let conn = setup_db();
    let service = TaskService::new(&conn);

    let task1 = service
        .create(&CreateTaskInput {
            description: "Task 1".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    let task2 = service
        .create(&CreateTaskInput {
            description: "Task 2".to_string(),
            context: None,
            parent_id: None,
            priority: None,
            blocked_by: vec![],
            ..Default::default()
        })
        .unwrap();

    learning_repo::add_learning(&conn, &task1.id, "Task 1 learning", None).unwrap();
    learning_repo::add_learning(&conn, &task2.id, "Task 2 learning", None).unwrap();

    let learnings1 = learning_repo::list_learnings(&conn, &task1.id).unwrap();
    let learnings2 = learning_repo::list_learnings(&conn, &task2.id).unwrap();

    assert_eq!(learnings1.len(), 1);
    assert_eq!(learnings1[0].content, "Task 1 learning");
    assert_eq!(learnings2.len(), 1);
    assert_eq!(learnings2[0].content, "Task 2 learning");
}
