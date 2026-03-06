use clap::Subcommand;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::db::{learning_repo, task_repo, Learning};
use crate::error::Result;
use crate::id::TaskId;

#[derive(Subcommand, Clone)]
pub enum DataCommand {
    /// Export all tasks and learnings to JSON file
    Export {
        /// Output file path (default: overseer-export.json)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTask {
    pub id: TaskId,
    pub parent_id: Option<TaskId>,
    pub description: String,
    pub context: String,
    pub result: Option<String>,
    pub priority: i32,
    pub completed: bool,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub cancelled: bool,
    pub cancelled_at: Option<chrono::DateTime<chrono::Utc>>,
    pub archived: bool,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub commit_sha: Option<String>,
    pub base_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportData {
    pub version: String,
    pub exported_at: String,
    pub tasks: Vec<ExportTask>,
    pub learnings: Vec<Learning>,
    pub blockers: Vec<BlockerRelation>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockerRelation {
    pub task_id: TaskId,
    pub blocker_id: TaskId,
}

pub enum DataResult {
    Exported {
        path: String,
        tasks: usize,
        learnings: usize,
    },
}

pub fn handle(conn: &Connection, cmd: DataCommand) -> Result<DataResult> {
    match cmd {
        DataCommand::Export { output } => export_data(conn, output),
    }
}

pub(crate) fn export_data(conn: &Connection, output: Option<PathBuf>) -> Result<DataResult> {
    let output_path = output.unwrap_or_else(|| PathBuf::from("overseer-export.json"));

    use crate::types::ListTasksFilter;

    // Get all tasks including archived (archived: None = include all)
    let filter = ListTasksFilter {
        archived: None,
        ..Default::default()
    };
    let tasks = task_repo::list_tasks(conn, &filter)?;
    let export_tasks: Vec<ExportTask> = tasks
        .iter()
        .filter_map(|t| {
            task_repo::get_task(conn, &t.id)
                .ok()
                .flatten()
                .map(|full_task| ExportTask {
                    id: full_task.id,
                    parent_id: full_task.parent_id,
                    description: full_task.description,
                    context: full_task.context,
                    result: full_task.result,
                    priority: full_task.priority,
                    completed: full_task.completed,
                    completed_at: full_task.completed_at,
                    cancelled: full_task.cancelled,
                    cancelled_at: full_task.cancelled_at,
                    archived: full_task.archived,
                    archived_at: full_task.archived_at,
                    created_at: full_task.created_at,
                    updated_at: full_task.updated_at,
                    started_at: full_task.started_at,
                    commit_sha: full_task.commit_sha,
                    base_ref: full_task.base_ref,
                })
        })
        .collect();

    // Get all learnings
    let mut all_learnings = Vec::new();
    for task in &tasks {
        let learnings = learning_repo::list_learnings(conn, &task.id)?;
        all_learnings.extend(learnings);
    }

    // Get all blocker relations
    let mut blockers = Vec::new();
    for task in &export_tasks {
        if let Some(full_task) = task_repo::get_task(conn, &task.id)? {
            for blocker_id in &full_task.blocked_by {
                blockers.push(BlockerRelation {
                    task_id: task.id.clone(),
                    blocker_id: blocker_id.clone(),
                });
            }
        }
    }

    let export = ExportData {
        version: "1.1.0".to_string(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        tasks: export_tasks.clone(),
        learnings: all_learnings.clone(),
        blockers,
    };

    let json = serde_json::to_string_pretty(&export)?;
    fs::write(&output_path, json)?;

    Ok(DataResult::Exported {
        path: output_path.display().to_string(),
        tasks: export_tasks.len(),
        learnings: all_learnings.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::task_service::TaskService;
    use crate::db::{self, learning_repo};
    use tempfile::TempDir;

    fn setup_test_db() -> (rusqlite::Connection, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        let db_path = tmp_dir.path().join("test.db");
        let conn = db::open_db(&db_path).unwrap();
        (conn, tmp_dir)
    }

    #[test]
    fn test_export_empty_database() {
        let (conn, tmp_dir) = setup_test_db();
        let output_path = tmp_dir.path().join("export.json");

        let result = export_data(&conn, Some(output_path.clone()));
        assert!(result.is_ok());

        match result.unwrap() {
            DataResult::Exported {
                tasks, learnings, ..
            } => {
                assert_eq!(tasks, 0);
                assert_eq!(learnings, 0);
            }
        }

        // Verify file exists
        assert!(output_path.exists());

        // Verify content
        let content = fs::read_to_string(&output_path).unwrap();
        let export: ExportData = serde_json::from_str(&content).unwrap();
        assert_eq!(export.version, "1.1.0");
        assert_eq!(export.tasks.len(), 0);
        assert_eq!(export.learnings.len(), 0);
        assert_eq!(export.blockers.len(), 0);
    }

    #[test]
    fn test_export_with_tasks_and_learnings() {
        let (conn, tmp_dir) = setup_test_db();
        let task_service = TaskService::new(&conn);

        // Create test data
        let task1 = task_service
            .create(&crate::types::CreateTaskInput {
                description: "Task 1".to_string(),
                context: Some("Context 1".to_string()),
                parent_id: None,
                priority: Some(0),
                blocked_by: vec![],
            })
            .unwrap();

        let _task2 = task_service
            .create(&crate::types::CreateTaskInput {
                description: "Task 2".to_string(),
                context: Some("Context 2".to_string()),
                parent_id: Some(task1.id.clone()),
                priority: Some(1),
                blocked_by: vec![],
            })
            .unwrap();

        learning_repo::add_learning(&conn, &task1.id, "Learning 1", None).unwrap();

        // Export
        let export_path = tmp_dir.path().join("export.json");
        let export_result = export_data(&conn, Some(export_path.clone())).unwrap();

        match export_result {
            DataResult::Exported {
                tasks, learnings, ..
            } => {
                assert_eq!(tasks, 2);
                assert_eq!(learnings, 1);
            }
        }
    }

    #[test]
    fn test_export_with_blockers() {
        let (conn, tmp_dir) = setup_test_db();
        let task_service = TaskService::new(&conn);

        let task1 = task_service
            .create(&crate::types::CreateTaskInput {
                description: "Task 1".to_string(),
                context: None,
                parent_id: None,
                priority: None,
                blocked_by: vec![],
            })
            .unwrap();

        let task2 = task_service
            .create(&crate::types::CreateTaskInput {
                description: "Task 2".to_string(),
                context: None,
                parent_id: None,
                priority: None,
                blocked_by: vec![task1.id.clone()],
            })
            .unwrap();

        // Export
        let export_path = tmp_dir.path().join("export.json");
        export_data(&conn, Some(export_path.clone())).unwrap();

        // Verify blockers in export
        let content = fs::read_to_string(&export_path).unwrap();
        let export: ExportData = serde_json::from_str(&content).unwrap();
        assert_eq!(export.blockers.len(), 1);
        assert_eq!(export.blockers[0].task_id, task2.id);
        assert_eq!(export.blockers[0].blocker_id, task1.id);
    }
}
