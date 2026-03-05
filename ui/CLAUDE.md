# UI - OVERSEER TASK VIEWER

Local webapp for inspecting Overseer task store. Hono API + Vite SPA + React + TanStack Query + Tailwind v4.

## STRUCTURE

```
src/
├── api/              # Hono API server
│   ├── app.ts        # Routes, AppType export
│   ├── index.ts      # serve() entry
│   ├── cli.ts        # CLI bridge (spawns `os --json`)
│   └── routes/       # API route handlers
│
├── client/           # React SPA
│   ├── main.tsx      # React root, QueryClientProvider
│   ├── App.tsx       # 3-panel layout
│   ├── components/   # UI components
│   │   ├── views/    # View modes (graph, kanban, list)
│   │   └── ui/       # Reusable UI primitives
│   ├── lib/
│   │   ├── api.ts    # hc<AppType> client
│   │   ├── queries.ts # TanStack Query hooks
│   │   ├── utils.ts  # Status helpers, formatters
│   │   └── store.ts  # Zustand state
│   └── styles/
│       └── global.css # Tailwind v4 + theme tokens
│
└── types.ts          # Shared types (mirrors host/src/types.ts)
```

## COMMANDS

```bash
npm --prefix ui run dev              # Hono API + Vite HMR
npm --prefix ui run dev:api          # Hono API only
npm --prefix ui run dev:vite         # Vite only
npm --prefix ui run build            # Production build
npm --prefix ui run typecheck        # Type check
npm --prefix ui run test:ui          # agent-browser test suite
```

## KEY FILES

| Task | File |
|------|------|
| Add API route | `src/api/routes/tasks.ts` |
| Add React component | `src/client/components/` |
| Add query hook | `src/client/lib/queries.ts` |
| Modify theme | `src/client/styles/global.css` |
| CLI bridge | `src/api/cli.ts` |

## LARGE COMPONENTS

| Component | Lines | Purpose |
|-----------|-------|---------|
| `TaskGraph.tsx` | ~1000 | React Flow graph with hierarchy |
| `TaskDetail.tsx` | ~600 | Detail panel with context/learnings |
| `KanbanView.tsx` | ~550 | Kanban board view |
| `TaskList.tsx` | ~550 | Filterable task list |

## THEME

**Neo-Industrial / Technical Brutalism**. Dark mode only, OKLCH colors.

- Vibrant orange accents (`oklch(0.68 0.21 38)`)
- Condensed display typography (Big Shoulders Display)
- Hard edges (no rounded corners), thick borders
- Registration marks, chevron prefixes, highlight bars

Key tokens: `--color-accent`, `--color-bg-primary`, `--color-surface-primary`, `--color-text-primary`
Status: `--color-status-pending` (gray), `--color-status-active` (orange), `--color-status-blocked` (red), `--color-status-done` (teal)

## PATTERNS

### State Split
- **TanStack Query**: all server state (tasks, learnings). Polls every 5s.
- **Zustand**: ephemeral UI state only (view mode, selected task, panel height)
- `clearIfMissing(existingIds)`: auto-deselects deleted/filtered tasks after fetch
- Panel height persists to localStorage (`ui.layout.v1.detailPanelHeight`)
- Use exported hooks (`useViewMode`, `useSelectedTaskId`), not raw `useStore`

### React Query
- Separate `isLoading` (no cache) from `isRefetching` (background) for UX
- Derive "last updated" from `max(data.updatedAt)`, not `dataUpdatedAt`

### Domain
- Use `effectivelyBlocked`, NOT `blockedBy.length` - blocker edges persist after completion
- Shared status helpers in `lib/utils.ts`: `getStatusVariant`, `getStatusLabel`

### Scroll Containers
- Flex scroll pattern: outer (`flex-1 relative min-h-0`), inner (`absolute inset-0 overflow-y-auto`)

### Drag & Resize
- Direct DOM manipulation during drag, commit to store on release
- CSS transitions disabled during drag, re-enabled on release
- Pointer capture for smooth cross-element dragging

### URL State
- Custom events (`os:urlchange`) for programmatic URL changes
- `useSyncExternalStore` for concurrent rendering correctness
- Type guards (`isTaskId`) at parse boundary

## ENVIRONMENT

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `6969` | Hono API port |
| `OVERSEER_CLI_PATH` | `os` | CLI binary path |
| `OVERSEER_CLI_CWD` | `process.cwd()` | CLI working directory |

### Keyboard Scopes
- Scope-based system: `global`, `list`, `graph`, `kanban`, `detail`
- `claimScope(id, scope)` returns token with `activate()`/`release()`
- Scoped shortcuts win over global; two-pass dispatch
- Guard with `e.isComposing` (CJK IME)

### Changed Task Flash
- `use-changed-tasks.ts` detects changes by comparing `updatedAt` between polls
- 1-second CSS flash animation per changed task

### View Props
All views (Graph, Kanban, List) receive same props: `tasks`, `externalBlockers`, `selectedId`, `onSelect`, `nextUpTaskId`. `externalBlockers` = `Map<TaskId, Task>` for cross-milestone dependency rendering.

## NOTES

- Types in `src/types.ts` must mirror `host/src/types.ts`
- Vite proxies `/api/*` to Hono in dev mode
- Production: Hono serves `dist/` static files
- UI API does NOT expose `start` (intentional - workflow via agent/CLI only)
- `hc<AppType>` typed client exists but query hooks use raw `fetch` against `/api/tasks/*`
