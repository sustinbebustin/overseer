# VCS MODULE

Native VCS backends: jj-lib (primary), gix (fallback). No subprocess spawning for read ops.

## FILES

| File | Purpose |
|------|---------|
| `mod.rs` | Public API: `get_backend()`, `detect()`, re-exports |
| `backend.rs` | `VcsBackend` trait, error types, data structs |
| `detection.rs` | `detect_vcs_type()`: walks up dirs, `.jj/` before `.git/` |
| `jj.rs` | jj-lib native, sync via pollster (~650 lines) |
| `git.rs` | gix for reads, git CLI for commits (~730 lines) |

## VcsBackend TRAIT

Common ops: `status()`, `log()`, `commit()`, `create_bookmark()`, `delete_bookmark()`, `checkout()`, `current_commit_id()`, `list_bookmarks()`

### jj.rs specifics
- `commit()`: Rewrite commit + `rebase_descendants()` + new working copy
- `resolve_to_commit_id()`: Bookmark/change ID resolution
- Workspace reloaded per operation (no stale state)

### git.rs specifics
- `status()`: gix status API with staged/worktree detection
- `commit()`: Falls back to git CLI (gix staging API unstable)

## UNIFIED STACKING SEMANTICS

Both backends implement identical workflow:
- **start**: Create bookmark/branch at HEAD, checkout
- **complete**: Commit -> checkout start_commit -> delete bookmark/branch

## ANTI-PATTERNS

- Never cache Workspace/ReadonlyRepo - reload each operation
- Never assume git CLI available in jj backend - jj-lib only
- Never skip `rebase_descendants()` after `rewrite_commit()` in jj
- Never check `.git/` first - jj repos can have both, jj takes precedence
- `jj-lib =0.37` pinned exactly - API breaks between minors
