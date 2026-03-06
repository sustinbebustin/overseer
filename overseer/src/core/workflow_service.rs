use std::path::PathBuf;

use rusqlite::Connection;

use crate::core::TaskService;
use crate::db::task_repo;
use crate::error::{NotReadyReason, OsError, Result};
use crate::id::TaskId;
use crate::types::Task;
use crate::vcs::backend::{VcsBackend, VcsError, VcsType};

/// Coordinates task state transitions with VCS operations.
///
/// **Transaction semantics**: VCS-first, then DB.
/// - VCS operations run first and can fail the entire operation
/// - DB operations run after VCS succeeds
/// - This prevents partial state on VCS failure
///
/// VCS is mandatory for workflow operations (start/complete).
/// CRUD operations don't require VCS.
pub struct TaskWorkflowService<'a> {
    task_service: TaskService<'a>,
    workspace_root: PathBuf,
    conn: &'a Connection,
}

impl<'a> TaskWorkflowService<'a> {
    pub fn new(conn: &'a Connection, workspace_root: PathBuf) -> Self {
        Self {
            task_service: TaskService::new(conn),
            workspace_root,
            conn,
        }
    }

    /// Access the underlying TaskService (used primarily in tests)
    #[allow(dead_code)]
    pub fn task_service(&self) -> &TaskService<'a> {
        &self.task_service
    }

    /// Resolve VCS backend for a specific task based on its repo_path.
    /// Falls back to workspace_root when task has no repo_path.
    fn vcs_for_task(&self, task: &Task) -> Result<Box<dyn VcsBackend>> {
        let repo_dir = match &task.repo_path {
            Some(rel) => self.workspace_root.join(rel),
            None => self.workspace_root.clone(),
        };
        crate::vcs::get_backend(&repo_dir).map_err(|e| e.into())
    }

    pub fn start(&self, id: &TaskId) -> Result<Task> {
        let task = self.task_service.get(id)?;
        let bookmark = task
            .bookmark
            .clone()
            .unwrap_or_else(|| format!("task/{}", id));

        // Guard: cannot start non-active tasks (cancelled, completed, archived)
        // This check MUST come before the idempotent path to prevent starting
        // tasks that were started then cancelled (they still have bookmark/started_at)
        if !task.is_active_for_work() {
            return match task.lifecycle_state() {
                crate::types::LifecycleState::Completed => Err(OsError::CannotStartCompleted),
                crate::types::LifecycleState::Cancelled => Err(OsError::CannotStartCancelled),
                crate::types::LifecycleState::Archived => Err(OsError::CannotModifyArchived),
                // These are active states - is_active_for_work() would have returned true
                crate::types::LifecycleState::Pending
                | crate::types::LifecycleState::InProgress => {
                    unreachable!("is_active_for_work() returned false but state is active")
                }
            };
        }

        let vcs = self.vcs_for_task(&task)?;

        // Idempotent: already started with VCS state
        if task.started_at.is_some() && task.bookmark.is_some() {
            if task.base_ref.is_none() && vcs.vcs_type() == VcsType::Git {
                let inferred_base_ref = Self::current_git_branch_name_from(&*vcs)?;
                if inferred_base_ref == bookmark {
                    return Err(OsError::MissingBaseRef {
                        task_id: id.clone(),
                    });
                }
                task_repo::set_base_ref(self.conn, id, &inferred_base_ref)?;
            }

            // Just checkout the existing bookmark
            if let Some(ref bookmark) = task.bookmark {
                vcs.checkout(bookmark)?;
            }
            return self.task_service.get(id);
        }

        // Validate: must be the next ready task in its subtree
        self.validate_start_target(id, &task)?;

        let base_ref = match vcs.vcs_type() {
            VcsType::Git => Some(Self::current_git_branch_name_from(&*vcs)?),
            _ => None,
        };

        // 1. Ensure bookmark exists (idempotent)
        match vcs.create_bookmark(&bookmark, None) {
            Ok(()) | Err(VcsError::BookmarkExists(_)) => {}
            Err(e) => return Err(e.into()),
        }

        // 2. Checkout (can fail on DirtyWorkingCopy)
        vcs.checkout(&bookmark)?;

        // 3. Record start commit
        let sha = vcs.current_commit_id()?;

        // 4. DB updates (after VCS succeeds)
        task_repo::set_bookmark(self.conn, id, &bookmark)?;
        task_repo::set_start_commit(self.conn, id, &sha)?;
        if let Some(ref base_ref_value) = base_ref {
            task_repo::set_base_ref(self.conn, id, base_ref_value)?;
        }

        if task.started_at.is_none() {
            self.task_service.start(id)?;
        }

        // 5. Bubble started_at to ancestors (but not VCS state)
        self.bubble_start_to_ancestors(id)?;

        self.task_service.get(id)
    }

    /// Validate that a task can be started.
    /// Returns error if task is not the next ready task in its subtree.
    fn validate_start_target(&self, id: &TaskId, task: &Task) -> Result<()> {
        // Check if blocked first (more specific error)
        if self.task_service.is_effectively_blocked(task)? {
            let blockers: Vec<TaskId> = task
                .blocked_by
                .iter()
                .filter(|b| !task_repo::is_task_completed(self.conn, b).unwrap_or(false))
                .cloned()
                .collect();

            // Search globally for a ready task (not within blocked subtree)
            let next_ready = self.task_service.next_ready(None)?;

            return Err(OsError::NotNextReady {
                message: format!(
                    "Cannot start '{}' - blocked by {}. {}",
                    task.description,
                    blockers
                        .iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    next_ready
                        .as_ref()
                        .map(|nr| format!("Start '{}' instead.", nr))
                        .unwrap_or_else(|| "No ready tasks available.".to_string())
                ),
                requested: id.clone(),
                next_ready,
                reason: NotReadyReason::Blocked { blockers },
            });
        }

        // Check if this is the next ready task in its subtree
        let next_ready = self.task_service.next_ready(Some(id))?;

        match next_ready {
            Some(ref ready_id) if ready_id == id => Ok(()), // This is the next ready task
            Some(ref ready_id) => {
                // Has incomplete children - should start the ready child instead
                let ready_task = self.task_service.get(ready_id)?;
                Err(OsError::NotNextReady {
                    message: format!(
                        "Cannot start '{}' - has incomplete children. Start '{}' instead.",
                        task.description, ready_task.description
                    ),
                    requested: id.clone(),
                    next_ready: Some(ready_id.clone()),
                    reason: NotReadyReason::HasIncompleteChildren,
                })
            }
            None => {
                // No ready tasks - might be all complete or all blocked
                Err(OsError::NotNextReady {
                    message: format!(
                        "Cannot start '{}' - no ready tasks in subtree (all complete or blocked).",
                        task.description
                    ),
                    requested: id.clone(),
                    next_ready: None,
                    reason: NotReadyReason::NoReadyTasksInSubtree,
                })
            }
        }
    }

    /// Bubble started_at to all ancestors that don't have it set.
    /// Only sets started_at, not VCS bookmark/start_commit.
    fn bubble_start_to_ancestors(&self, id: &TaskId) -> Result<()> {
        let mut current_id = id.clone();

        loop {
            let current = task_repo::get_task(self.conn, &current_id)?
                .ok_or_else(|| OsError::TaskNotFound(current_id.clone()))?;

            let Some(parent_id) = current.parent_id else {
                break;
            };

            let parent = task_repo::get_task(self.conn, &parent_id)?
                .ok_or_else(|| OsError::TaskNotFound(parent_id.clone()))?;

            // Only set started_at if not already set
            if parent.started_at.is_none() {
                self.task_service.start(&parent_id)?;
            }

            current_id = parent_id;
        }

        Ok(())
    }

    fn current_git_branch_name_from(vcs: &dyn VcsBackend) -> Result<String> {
        let name = vcs
            .current_branch_name()?
            .ok_or(OsError::CannotStartDetachedHead)?;
        Ok(name)
    }

    fn enforce_git_integration_gate(vcs: &dyn VcsBackend, task: &Task, id: &TaskId) -> Result<()> {
        if vcs.vcs_type() != VcsType::Git {
            return Ok(());
        }

        if let Some(bookmark) = task.bookmark.clone() {
            let Some(base_ref) = task.base_ref.clone() else {
                return Err(OsError::MissingBaseRef {
                    task_id: id.clone(),
                });
            };
            let merged = vcs.merge_fast_forward(&bookmark, &base_ref)?;
            if !merged {
                return Err(OsError::TaskIntegrationRequired {
                    task_id: id.clone(),
                    source_ref: bookmark,
                    base_ref,
                });
            }
        }

        Ok(())
    }

    /// Start a task, following blockers to find startable work.
    ///
    /// If the requested task or any of its descendants are blocked,
    /// follows blockers until finding a startable task.
    /// Cascades down to deepest incomplete leaf.
    pub fn start_follow_blockers(&self, root: &TaskId) -> Result<Task> {
        let target = self.task_service.resolve_start_target(root)?;
        self.start(&target)
    }

    /// Convenience method for completing without learnings.
    #[allow(dead_code)] // Used in tests
    pub fn complete(&self, id: &TaskId, result: Option<&str>) -> Result<Task> {
        self.complete_with_learnings(id, result, &[])
    }

    /// Complete a task with optional learnings.
    /// Learnings are added to the task and bubbled to immediate parent.
    ///
    /// VCS-first ordering: commit changes before updating DB state.
    pub fn complete_with_learnings(
        &self,
        id: &TaskId,
        result: Option<&str>,
        learnings: &[String],
    ) -> Result<Task> {
        let task = self.task_service.get(id)?;

        // Lifecycle guard: reject inactive states
        if task.archived {
            return Err(OsError::CannotCompleteArchived);
        }
        if task.cancelled {
            return Err(OsError::CannotCompleteCancelled);
        }

        // Idempotent: already completed
        if task.completed {
            return Ok(task);
        }

        // Auto-detect milestone (depth 0)
        if task.depth == Some(0) {
            return self.complete_milestone_with_learnings(id, result, learnings);
        }

        let vcs = self.vcs_for_task(&task)?;

        // 1. VCS first - commit (NothingToCommit is OK)
        let msg = format!("Complete: {}\n\n{}", task.description, result.unwrap_or(""));
        match vcs.commit(&msg) {
            Ok(_) | Err(VcsError::NothingToCommit) => {}
            Err(e) => return Err(e.into()),
        }

        let commit_sha = vcs.current_commit_id().ok();

        Self::enforce_git_integration_gate(&*vcs, &task, id)?;

        // 2. DB updates (after VCS succeeds)
        let completed_task = self.task_service.complete_with_learnings_and_commit_sha(
            id,
            result,
            learnings,
            commit_sha.as_deref(),
        )?;

        // 3. Best-effort cleanup: checkout safe target then delete bookmark/branch
        // Unified stacking semantics: both jj and git get same behavior
        // Checkout first solves git's "cannot delete checked-out branch" error
        if let Some(ref bookmark) = task.bookmark {
            // Find checkout target: prefer current HEAD, fallback to start_commit
            let checkout_target = vcs
                .current_commit_id()
                .ok()
                .or_else(|| task.start_commit.clone());

            if let Some(ref target) = checkout_target {
                if let Err(e) = vcs.checkout(target) {
                    eprintln!(
                        "warn: failed to checkout {}: {} - skipping branch cleanup",
                        target, e
                    );
                } else if let Err(e) = vcs.delete_bookmark(bookmark) {
                    eprintln!("warn: failed to delete bookmark {}: {}", bookmark, e);
                } else {
                    // Clear bookmark field in DB after successful VCS deletion
                    let _ = task_repo::clear_bookmark(self.conn, id);
                }
            } else {
                eprintln!(
                    "warn: no checkout target available - skipping branch cleanup for {}",
                    bookmark
                );
            }
        }

        // Bubble up: auto-complete parents if all children done and unblocked
        self.bubble_up_completion(id)?;

        Ok(completed_task)
    }

    /// Auto-complete parent tasks if all siblings are done and parent is unblocked.
    /// Bubbles up recursively until hitting a blocked parent or pending children.
    fn bubble_up_completion(&self, completed_id: &TaskId) -> Result<()> {
        let mut current_id = completed_id.clone();

        loop {
            let current = task_repo::get_task(self.conn, &current_id)?
                .ok_or_else(|| crate::error::OsError::TaskNotFound(current_id.clone()))?;

            let Some(parent_id) = current.parent_id else {
                break;
            };

            // Check if parent has pending children
            if task_repo::has_pending_children(self.conn, &parent_id)? {
                break;
            }

            // Check if parent is blocked
            let parent = self.task_service.get(&parent_id)?;
            if self.task_service.is_effectively_blocked(&parent)? {
                break;
            }

            // Auto-complete parent (use service method to handle depth-0 special case)
            if parent.depth == Some(0) {
                self.complete_milestone(&parent_id, None)?;
            } else {
                self.complete(&parent_id, None)?;
            }

            current_id = parent_id;
        }

        Ok(())
    }

    pub fn complete_milestone(&self, id: &TaskId, result: Option<&str>) -> Result<Task> {
        self.complete_milestone_with_learnings(id, result, &[])
    }

    /// Complete a milestone with optional learnings.
    ///
    /// VCS-first ordering: commit changes before updating DB state.
    /// For milestones spanning multiple repos (repo_path: None), commit is attempted
    /// at workspace root - NothingToCommit or NotARepository are both OK.
    /// Descendant bookmark cleanup is grouped by repo_path with per-repo VCS resolution.
    pub fn complete_milestone_with_learnings(
        &self,
        id: &TaskId,
        result: Option<&str>,
        learnings: &[String],
    ) -> Result<Task> {
        let task = self.task_service.get(id)?;

        // Lifecycle guard: reject inactive states
        if task.archived {
            return Err(OsError::CannotCompleteArchived);
        }
        if task.cancelled {
            return Err(OsError::CannotCompleteCancelled);
        }

        // Idempotent: already completed
        if task.completed {
            return Ok(task);
        }

        // Not a milestone - delegate to regular complete
        if task.depth != Some(0) {
            return self.complete_with_learnings(id, result, learnings);
        }

        // Milestone: VCS first - commit (NothingToCommit is OK)
        // For milestones without repo_path, try workspace root; skip if no VCS
        let msg = format!(
            "Milestone: {}\n\n{}",
            task.description,
            result.unwrap_or("")
        );
        match self.vcs_for_task(&task) {
            Ok(vcs) => {
                match vcs.commit(&msg) {
                    Ok(_) | Err(VcsError::NothingToCommit) => {}
                    Err(e) => return Err(e.into()),
                }
                let commit_sha = vcs.current_commit_id().ok();
                Self::enforce_git_integration_gate(&*vcs, &task, id)?;

                // DB updates (after VCS succeeds)
                let completed_task = self.task_service.complete_with_learnings_and_commit_sha(
                    id,
                    result,
                    learnings,
                    commit_sha.as_deref(),
                )?;

                // Best-effort cleanup: delete ALL descendant bookmarks
                // Group descendants by repo_path and resolve VCS per group
                let descendants = task_repo::get_all_descendants(self.conn, id)?;

                // Collect (repo_path, bookmark, task_id) tuples grouped by repo_path
                let mut bookmarks_by_repo: std::collections::HashMap<
                    Option<String>,
                    Vec<(String, TaskId)>,
                > = std::collections::HashMap::new();
                for descendant in descendants.iter() {
                    if let Some(ref bookmark) = descendant.bookmark {
                        bookmarks_by_repo
                            .entry(descendant.repo_path.clone())
                            .or_default()
                            .push((bookmark.clone(), descendant.id.clone()));
                    }
                }
                // Include milestone's own bookmark
                if let Some(ref bookmark) = task.bookmark {
                    bookmarks_by_repo
                        .entry(task.repo_path.clone())
                        .or_default()
                        .push((bookmark.clone(), id.clone()));
                }

                // Delete bookmarks per repo
                for (repo_path, bookmarks) in &bookmarks_by_repo {
                    let repo_dir = match repo_path {
                        Some(rel) => self.workspace_root.join(rel),
                        None => self.workspace_root.clone(),
                    };
                    let vcs = match crate::vcs::get_backend(&repo_dir) {
                        Ok(vcs) => vcs,
                        Err(e) => {
                            eprintln!(
                                "warn: no VCS at {:?} - skipping bookmark cleanup: {}",
                                repo_dir, e
                            );
                            continue;
                        }
                    };

                    // Checkout a safe target first (needed for git "cannot delete checked-out branch")
                    let checkout_target = vcs
                        .current_commit_id()
                        .ok()
                        .or_else(|| task.start_commit.clone())
                        .or_else(|| descendants.iter().find_map(|d| d.start_commit.clone()));

                    if let Some(ref target) = checkout_target {
                        if let Err(e) = vcs.checkout(target) {
                            eprintln!(
                                "warn: failed to checkout {}: {} - skipping branch cleanup for {:?}",
                                target, e, repo_dir
                            );
                            continue;
                        }
                    }

                    for (bookmark, task_id) in bookmarks {
                        if let Err(e) = vcs.delete_bookmark(bookmark) {
                            eprintln!("warn: failed to delete bookmark {}: {}", bookmark, e);
                        } else {
                            let _ = task_repo::clear_bookmark(self.conn, task_id);
                        }
                    }
                }

                return Ok(completed_task);
            }
            Err(OsError::NotARepository) if task.repo_path.is_none() => {
                // Milestone spans repos, workspace root has no VCS - OK
            }
            Err(e) => return Err(e),
        }

        // DB updates (after VCS succeeds)
        let completed_task = self
            .task_service
            .complete_with_learnings_and_commit_sha(id, result, learnings, None)?;

        // Best-effort cleanup: delete ALL descendant bookmarks
        // Group descendants by repo_path and resolve VCS per group
        let descendants = task_repo::get_all_descendants(self.conn, id)?;

        // Collect (repo_path, bookmark, task_id) tuples grouped by repo_path
        let mut bookmarks_by_repo: std::collections::HashMap<
            Option<String>,
            Vec<(String, TaskId)>,
        > = std::collections::HashMap::new();
        for descendant in descendants.iter() {
            if let Some(ref bookmark) = descendant.bookmark {
                bookmarks_by_repo
                    .entry(descendant.repo_path.clone())
                    .or_default()
                    .push((bookmark.clone(), descendant.id.clone()));
            }
        }
        // Include milestone's own bookmark
        if let Some(ref bookmark) = task.bookmark {
            bookmarks_by_repo
                .entry(task.repo_path.clone())
                .or_default()
                .push((bookmark.clone(), id.clone()));
        }

        // Delete bookmarks per repo
        for (repo_path, bookmarks) in &bookmarks_by_repo {
            let repo_dir = match repo_path {
                Some(rel) => self.workspace_root.join(rel),
                None => self.workspace_root.clone(),
            };
            let vcs = match crate::vcs::get_backend(&repo_dir) {
                Ok(vcs) => vcs,
                Err(e) => {
                    eprintln!(
                        "warn: no VCS at {:?} - skipping bookmark cleanup: {}",
                        repo_dir, e
                    );
                    continue;
                }
            };

            // Checkout a safe target first (needed for git "cannot delete checked-out branch")
            let checkout_target = vcs
                .current_commit_id()
                .ok()
                .or_else(|| task.start_commit.clone())
                .or_else(|| descendants.iter().find_map(|d| d.start_commit.clone()));

            if let Some(ref target) = checkout_target {
                if let Err(e) = vcs.checkout(target) {
                    eprintln!(
                        "warn: failed to checkout {}: {} - skipping branch cleanup for {:?}",
                        target, e, repo_dir
                    );
                    continue;
                }
            }

            for (bookmark, task_id) in bookmarks {
                if let Err(e) = vcs.delete_bookmark(bookmark) {
                    eprintln!("warn: failed to delete bookmark {}: {}", bookmark, e);
                } else {
                    let _ = task_repo::clear_bookmark(self.conn, task_id);
                }
            }
        }

        Ok(completed_task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema::init_schema;
    use crate::testutil::{GitTestRepo, TestRepo};
    use crate::types::CreateTaskInput;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    /// Create a git test repo with an initial commit (needed for branch ops)
    fn setup_git_repo() -> GitTestRepo {
        let repo = GitTestRepo::new().unwrap();
        repo.write_file("README.md", "# test").unwrap();
        repo.commit("initial").unwrap();
        repo
    }

    #[test]
    fn test_start_records_vcs_state() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());

        let task = service
            .task_service()
            .create(&CreateTaskInput {
                description: "Test task".to_string(),
                context: None,
                parent_id: None,
                priority: None,
                blocked_by: vec![],
                ..Default::default()
            })
            .unwrap();

        let started = service.start(&task.id).unwrap();
        assert!(started.started_at.is_some());
        assert!(started.bookmark.is_some());
        assert!(started.start_commit.is_some());
        // Git repo defaults to "main" or "master" depending on config
        assert!(started.base_ref.is_some());
    }

    #[test]
    fn test_start_fails_on_detached_head() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let head = repo.head().unwrap();
        // Detach HEAD
        std::process::Command::new("git")
            .args(["checkout", "--detach", &head])
            .current_dir(repo.path())
            .output()
            .unwrap();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());

        let task = service
            .task_service()
            .create(&CreateTaskInput {
                description: "Test task".to_string(),
                ..Default::default()
            })
            .unwrap();

        let result = service.start(&task.id);
        assert!(matches!(result, Err(OsError::CannotStartDetachedHead)));
    }

    #[test]
    fn test_start_fails_on_unborn_repo() {
        let conn = setup_db();
        // Create git repo without any commits (unborn)
        let repo = GitTestRepo::new().unwrap();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());

        let task = service
            .task_service()
            .create(&CreateTaskInput {
                description: "Test task".to_string(),
                ..Default::default()
            })
            .unwrap();

        let result = service.start(&task.id);
        assert!(matches!(result, Err(OsError::CannotStartUnbornRepository)));
    }

    #[test]
    fn test_complete_returns_missing_base_ref_when_started_task_has_none() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let task_service = TaskService::new(&conn);

        let task = task_service
            .create(&CreateTaskInput {
                description: "Test task".to_string(),
                ..Default::default()
            })
            .unwrap();

        task_service.start(&task.id).unwrap();
        let bookmark = format!("task/{}", task.id);
        task_repo::set_bookmark(&conn, &task.id, &bookmark).unwrap();
        task_repo::set_start_commit(&conn, &task.id, &repo.head().unwrap()).unwrap();
        // Create the bookmark in git so complete can find it
        std::process::Command::new("git")
            .args(["branch", &bookmark])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let result = service.complete_with_learnings(&task.id, None, &[]);
        assert!(matches!(
            result,
            Err(OsError::MissingBaseRef { task_id }) if task_id == task.id
        ));
    }

    #[test]
    fn test_complete_updates_state() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());

        let task = service
            .task_service()
            .create(&CreateTaskInput {
                description: "Test task".to_string(),
                ..Default::default()
            })
            .unwrap();

        let completed = service.complete(&task.id, Some("Done")).unwrap();
        assert!(completed.completed);
        assert_eq!(completed.result, Some("Done".to_string()));
        assert!(completed.commit_sha.is_some());
    }

    #[test]
    fn test_workflow_complete_sets_commit_sha_from_workflow_vcs_backend() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());

        let task = service
            .task_service()
            .create(&CreateTaskInput {
                description: "Commit SHA provenance task".to_string(),
                ..Default::default()
            })
            .unwrap();

        let completed = service.complete(&task.id, None).unwrap();
        assert!(completed.completed);

        let expected_sha = crate::vcs::get_backend(repo.path())
            .unwrap()
            .current_commit_id()
            .unwrap();
        assert_eq!(completed.commit_sha, Some(expected_sha));
    }

    #[test]
    fn test_bubble_up_uses_workflow_invariants_for_parent_completion() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Build hierarchy: milestone -> parent_task -> child_task
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let parent_task = svc
            .create(&CreateTaskInput {
                description: "Parent task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let child_task = svc
            .create(&CreateTaskInput {
                description: "Child task".to_string(),
                parent_id: Some(parent_task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Seed parent as started git task with bookmark but missing base_ref.
        // Bubble-up must route through workflow completion and fail MissingBaseRef.
        svc.start(&parent_task.id).unwrap();
        let parent_bookmark = format!("task/{}", parent_task.id);
        task_repo::set_bookmark(&conn, &parent_task.id, &parent_bookmark).unwrap();
        task_repo::set_start_commit(&conn, &parent_task.id, &repo.head().unwrap()).unwrap();

        std::process::Command::new("git")
            .args(["branch", &parent_bookmark])
            .current_dir(repo.path())
            .output()
            .unwrap();

        let result = service.complete(&child_task.id, None);
        assert!(matches!(result, Err(OsError::MissingBaseRef { .. })));

        // Parent must remain incomplete since bubble-up completion failed closed.
        let parent_after = svc.get(&parent_task.id).unwrap();
        assert!(!parent_after.completed);
    }

    #[test]
    fn test_start_cascades_to_deepest_leaf() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask = svc
            .create(&CreateTaskInput {
                description: "Subtask".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let started = service.start_follow_blockers(&milestone.id).unwrap();
        assert_eq!(started.id, subtask.id);
        assert!(started.started_at.is_some());
    }

    #[test]
    fn test_start_follows_blockers_to_startable() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        let blocker_task = svc
            .create(&CreateTaskInput {
                description: "Blocker task".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let blocked_milestone = svc
            .create(&CreateTaskInput {
                description: "Blocked milestone".to_string(),
                priority: Some(0),
                blocked_by: vec![blocker_task.id.clone()],
                ..Default::default()
            })
            .unwrap();

        let _task = svc
            .create(&CreateTaskInput {
                description: "Task under blocked milestone".to_string(),
                parent_id: Some(blocked_milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let started = service
            .start_follow_blockers(&blocked_milestone.id)
            .unwrap();
        assert_eq!(started.id, blocker_task.id);
        assert!(started.started_at.is_some());
    }

    #[test]
    fn test_complete_bubbles_up_to_parent() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task -> subtask1, subtask2
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask1 = svc
            .create(&CreateTaskInput {
                description: "Subtask 1".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask2 = svc
            .create(&CreateTaskInput {
                description: "Subtask 2".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete first subtask - task should NOT be auto-completed
        service.complete(&subtask1.id, None).unwrap();
        let task_after = svc.get(&task.id).unwrap();
        assert!(!task_after.completed);

        // Complete second subtask - task SHOULD be auto-completed
        service.complete(&subtask2.id, None).unwrap();
        let task_after = svc.get(&task.id).unwrap();
        assert!(task_after.completed);
    }

    #[test]
    fn test_complete_bubbles_up_to_milestone() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task (single task, no siblings)
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete task - milestone should auto-complete
        service.complete(&task.id, None).unwrap();

        let milestone_after = svc.get(&milestone.id).unwrap();
        assert!(milestone_after.completed);
    }

    #[test]
    fn test_complete_stops_at_blocked_parent() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: blocker, milestone (blocked by blocker) -> task
        let blocker = svc
            .create(&CreateTaskInput {
                description: "Blocker".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let milestone = svc
            .create(&CreateTaskInput {
                description: "Blocked milestone".to_string(),
                priority: Some(0),
                blocked_by: vec![blocker.id.clone()],
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete task - milestone should NOT auto-complete (it's blocked)
        service.complete(&task.id, None).unwrap();

        let milestone_after = svc.get(&milestone.id).unwrap();
        assert!(!milestone_after.completed);
    }

    #[test]
    fn test_complete_stops_at_pending_siblings() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task1, task2
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task1 = svc
            .create(&CreateTaskInput {
                description: "Task 1".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let _task2 = svc
            .create(&CreateTaskInput {
                description: "Task 2".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete task1 - milestone should NOT auto-complete (task2 still pending)
        service.complete(&task1.id, None).unwrap();

        let milestone_after = svc.get(&milestone.id).unwrap();
        assert!(!milestone_after.completed);
    }

    #[test]
    fn test_complete_with_learnings_bubbles_to_parent() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task -> subtask1, subtask2 (sibling prevents auto-complete)
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask1 = svc
            .create(&CreateTaskInput {
                description: "Subtask 1".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Second subtask prevents task from auto-completing
        let _subtask2 = svc
            .create(&CreateTaskInput {
                description: "Subtask 2".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete subtask1 with learnings
        service
            .complete_with_learnings(
                &subtask1.id,
                Some("done"),
                &["Learning 1".to_string(), "Learning 2".to_string()],
            )
            .unwrap();

        // Learnings should be on subtask1
        let subtask_learnings =
            crate::db::learning_repo::list_learnings(&conn, &subtask1.id).unwrap();
        assert_eq!(subtask_learnings.len(), 2);
        assert_eq!(subtask_learnings[0].content, "Learning 1");
        assert_eq!(subtask_learnings[1].content, "Learning 2");
        // Origin should be subtask1 itself
        assert_eq!(
            subtask_learnings[0].source_task_id,
            Some(subtask1.id.clone())
        );

        // Learnings should have bubbled to task (parent)
        let task_learnings = crate::db::learning_repo::list_learnings(&conn, &task.id).unwrap();
        assert_eq!(task_learnings.len(), 2);
        assert_eq!(task_learnings[0].content, "Learning 1");
        // Origin preserved through bubble
        assert_eq!(task_learnings[0].source_task_id, Some(subtask1.id.clone()));

        // Task should NOT be auto-completed (subtask2 still pending)
        let task_after = svc.get(&task.id).unwrap();
        assert!(!task_after.completed);

        // Learnings should NOT be on milestone yet (task not completed)
        let milestone_learnings =
            crate::db::learning_repo::list_learnings(&conn, &milestone.id).unwrap();
        assert_eq!(milestone_learnings.len(), 0);
    }

    #[test]
    fn test_learnings_bubble_transitively_on_parent_complete() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task -> subtask
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask = svc
            .create(&CreateTaskInput {
                description: "Subtask".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete subtask with learning
        service
            .complete_with_learnings(&subtask.id, None, &["From subtask".to_string()])
            .unwrap();

        // Task auto-completes (only child done), which bubbles learnings to milestone
        let task_after = svc.get(&task.id).unwrap();
        assert!(task_after.completed);

        // Now milestone should have the learning (bubbled from task which had it from subtask)
        let milestone_learnings =
            crate::db::learning_repo::list_learnings(&conn, &milestone.id).unwrap();
        assert_eq!(milestone_learnings.len(), 1);
        assert_eq!(milestone_learnings[0].content, "From subtask");
        // Origin preserved: still points to subtask
        assert_eq!(
            milestone_learnings[0].source_task_id,
            Some(subtask.id.clone())
        );
    }

    #[test]
    fn test_sibling_sees_learnings_via_parent() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task_a (with subtasks), task_b (with subtasks)
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task_a = svc
            .create(&CreateTaskInput {
                description: "Task A".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask_a1 = svc
            .create(&CreateTaskInput {
                description: "Subtask A1".to_string(),
                parent_id: Some(task_a.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask_a2 = svc
            .create(&CreateTaskInput {
                description: "Subtask A2".to_string(),
                parent_id: Some(task_a.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete A1 with learning
        service
            .complete_with_learnings(&subtask_a1.id, None, &["A1 discovery".to_string()])
            .unwrap();

        // A2 should see A1's learning via parent (task_a)
        let task_a_learnings = crate::db::learning_repo::list_learnings(&conn, &task_a.id).unwrap();
        assert_eq!(task_a_learnings.len(), 1);
        assert_eq!(task_a_learnings[0].content, "A1 discovery");

        // Start A2 and get its inherited learnings
        let a2_with_context = svc.get(&subtask_a2.id).unwrap();
        // InheritedLearnings.parent should contain A1's learning
        assert!(a2_with_context.learnings.is_some());
        let inherited = a2_with_context.learnings.unwrap();
        assert_eq!(inherited.parent.len(), 1);
        assert_eq!(inherited.parent[0].content, "A1 discovery");
    }

    #[test]
    fn test_learnings_idempotent_on_retry() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Complete with learning
        service
            .complete_with_learnings(&task.id, None, &["Important note".to_string()])
            .unwrap();

        // Try to complete again (idempotent) - should not duplicate learnings
        service
            .complete_with_learnings(&task.id, None, &["Important note".to_string()])
            .unwrap();

        // Should still only have 1 learning on milestone (not duplicated)
        let milestone_learnings =
            crate::db::learning_repo::list_learnings(&conn, &milestone.id).unwrap();
        assert_eq!(milestone_learnings.len(), 1);
    }

    #[test]
    fn test_start_rejects_parent_with_incomplete_children() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task -> subtask
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask = svc
            .create(&CreateTaskInput {
                description: "Subtask".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Starting milestone directly should fail
        let result = service.start(&milestone.id);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            OsError::NotNextReady {
                requested,
                next_ready,
                ..
            } => {
                assert_eq!(requested, milestone.id);
                assert_eq!(next_ready, Some(subtask.id.clone()));
            }
            _ => panic!("Expected NotNextReady error, got {:?}", err),
        }

        // Starting task directly should also fail (has subtask)
        let result = service.start(&task.id);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            OsError::NotNextReady {
                requested,
                next_ready,
                ..
            } => {
                assert_eq!(requested, task.id);
                assert_eq!(next_ready, Some(subtask.id.clone()));
            }
            _ => panic!("Expected NotNextReady error, got {:?}", err),
        }

        // Starting subtask should succeed
        let result = service.start(&subtask.id);
        assert!(result.is_ok());
    }

    #[test]
    fn test_start_rejects_blocked_task() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: blocker, blocked_task (blocked by blocker)
        let blocker = svc
            .create(&CreateTaskInput {
                description: "Blocker".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let blocked_task = svc
            .create(&CreateTaskInput {
                description: "Blocked task".to_string(),
                priority: Some(0),
                blocked_by: vec![blocker.id.clone()],
                ..Default::default()
            })
            .unwrap();

        // Starting blocked_task should fail with blocked error
        let result = service.start(&blocked_task.id);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            OsError::NotNextReady { reason, .. } => {
                assert!(matches!(reason, NotReadyReason::Blocked { .. }));
            }
            _ => panic!(
                "Expected NotNextReady error with Blocked reason, got {:?}",
                err
            ),
        }
    }

    #[test]
    fn test_start_bubbles_started_at_to_ancestors() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create: milestone -> task -> subtask
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                parent_id: Some(milestone.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        let subtask = svc
            .create(&CreateTaskInput {
                description: "Subtask".to_string(),
                parent_id: Some(task.id.clone()),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Verify ancestors have no started_at initially
        assert!(svc.get(&milestone.id).unwrap().started_at.is_none());
        assert!(svc.get(&task.id).unwrap().started_at.is_none());
        assert!(svc.get(&subtask.id).unwrap().started_at.is_none());

        // Start the subtask
        let started = service.start(&subtask.id).unwrap();
        assert!(started.started_at.is_some());
        assert!(started.bookmark.is_some()); // VCS state only on leaf

        // Ancestors should now have started_at but NOT VCS state
        let task_after = svc.get(&task.id).unwrap();
        assert!(task_after.started_at.is_some());
        assert!(task_after.bookmark.is_none()); // No VCS bookmark on parent

        let milestone_after = svc.get(&milestone.id).unwrap();
        assert!(milestone_after.started_at.is_some());
        assert!(milestone_after.bookmark.is_none()); // No VCS bookmark on grandparent
    }

    #[test]
    fn test_start_is_idempotent() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Start task
        let first_start = service.start(&task.id).unwrap();
        assert!(first_start.started_at.is_some());
        let first_started_at = first_start.started_at;

        // Start again - should be idempotent
        let second_start = service.start(&task.id).unwrap();
        assert_eq!(second_start.started_at, first_started_at);
    }

    #[test]
    fn test_start_allows_leaf_without_children() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        // Create a milestone with no children (it's a leaf)
        let milestone = svc
            .create(&CreateTaskInput {
                description: "Leaf milestone".to_string(),
                priority: Some(0),
                ..Default::default()
            })
            .unwrap();

        // Should be startable since it's a leaf
        let result = service.start(&milestone.id);
        assert!(result.is_ok());
        assert!(result.unwrap().started_at.is_some());
    }

    #[test]
    fn test_complete_cancelled_task_fails() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                ..Default::default()
            })
            .unwrap();

        // Cancel the task
        svc.cancel(&task.id).unwrap();

        // Try to complete cancelled task
        let result = service.complete_with_learnings(&task.id, None, &[]);
        assert!(
            matches!(result, Err(OsError::CannotCompleteCancelled)),
            "Expected CannotCompleteCancelled error, got {:?}",
            result
        );
    }

    #[test]
    fn test_complete_archived_task_fails() {
        let conn = setup_db();
        let repo = setup_git_repo();
        let service = TaskWorkflowService::new(&conn, repo.path().to_path_buf());
        let svc = service.task_service();

        let task = svc
            .create(&CreateTaskInput {
                description: "Task".to_string(),
                ..Default::default()
            })
            .unwrap();

        // Complete then archive the task
        svc.complete(&task.id, None).unwrap();
        svc.archive(&task.id).unwrap();

        // Try to complete archived task (even though already completed, archived check comes first)
        let result = service.complete_with_learnings(&task.id, None, &[]);
        assert!(
            matches!(result, Err(OsError::CannotCompleteArchived)),
            "Expected CannotCompleteArchived error, got {:?}",
            result
        );
    }
}
