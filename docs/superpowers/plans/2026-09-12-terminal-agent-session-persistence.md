# Terminal Agent Session Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist the OMP session bound to a local Agent Panel Terminal Thread and resume the exact session only when the user later selects that restored thread.

**Architecture:** `TerminalThreadMetadataStore` gains a generic `TerminalAgentSession` value and persists it with additive SQLite columns. A new Agent UI reporter installs an inert-outside-Zed OMP extension and accepts authenticated atomic-file reports from local live terminal threads. `AgentPanel` supplies one fresh capture capability per local spawn and, on restoration, asks the driver registry for the only safe session-specific startup command instead of replaying the global init command.

**Tech Stack:** Rust, GPUI entities/tasks, SQLite through `db`, `fs::Fs` atomic writes/watchers, OMP TypeScript extensions, existing terminal PTY startup handshake.

**Spec:** `docs/superpowers/specs/2026-09-12-terminal-agent-session-persistence-design.md`

## Global Constraints

- Modify `README.md` with the exact mandatory two-line review marker before modifying any Rust source or test file; do not remove it.
- Scope is local Agent Panel Terminal Threads only. SSH/remote terminals must neither receive capture variables nor resume OMP.
- Persist `agent_id`, `resume_target`, and session `working_directory`; never persist a shell command, command line, process argv, terminal text, or a "most recent" session heuristic.
- The initial known agent is only `omp`; accept only bounded ASCII OMP session IDs matching `[A-Za-z0-9_-]+`.
- An invalid or unknown persisted session must not execute any stored data. It falls back to the current global terminal init behavior.
- The OMP extension must be harmless outside a Zed-captured terminal and may not fail an OMP turn when reporting fails.
- Existing `agent.terminal_init_command` behavior remains unchanged for terminal threads without a safe, known agent session.
- Reuse the existing terminal startup handshake; one restore writes one command and reactivation writes none.
- Use focused tests and a manual two-session smoke test. Do not run the project-wide test suite or formatter mid-task.

---

## File Structure

| Path | Responsibility |
| --- | --- |
| `README.md` | Mandatory first-lines review marker required by repository rules before source/test edits. |
| `crates/agent_ui/src/terminal_thread_metadata_store.rs` | Generic durable terminal-agent session model, SQLite migration, cached metadata mutation, store tests. |
| `crates/agent_ui/src/terminal_agent_session.rs` | Agent driver registry, OMP extension materialization, token-authenticated event-file protocol, reporter lifecycle. |
| `crates/agent_ui/src/agent_ui.rs` | Registers the reporter global using the existing `Arc<dyn Fs>` received at crate initialization. |
| `crates/project/src/terminals.rs` | Narrow local-shell API for caller-supplied, authoritative per-spawn environment variables. |
| `crates/agent_ui/src/agent_panel.rs` | Registers/unregisters a local terminal capture, preserves event-derived metadata, chooses restore cwd/init command, tests terminal behavior. |

### Task 1: Persist generic terminal-agent metadata

**Files:**
- Modify: `README.md:1`
- Modify: `crates/agent_ui/src/terminal_thread_metadata_store.rs:47-56,486-646,648-803`
- Test: `crates/agent_ui/src/terminal_thread_metadata_store.rs:648-803`

**Interfaces:**
- Consumes: durable `TerminalId` from `agent_panel`, existing `TerminalThreadMetadataStore::save`, and the `sidebar_terminal_threads` row.
- Produces:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalAgentSession {
    pub agent_id: String,
    pub resume_target: String,
    pub working_directory: PathBuf,
}

pub struct TerminalThreadMetadata {
    pub terminal_id: TerminalId,
    pub title: SharedString,
    pub custom_title: Option<SharedString>,
    pub created_at: DateTime<Utc>,
    pub worktree_paths: WorktreePaths,
    pub remote_connection: Option<RemoteConnectionOptions>,
    pub working_directory: Option<PathBuf>,
    pub agent_session: Option<TerminalAgentSession>,
}

fn terminal_agent_session_from_columns(
    agent_id: Option<String>,
    resume_target: Option<String>,
    working_directory: Option<String>,
) -> Option<TerminalAgentSession>;

impl TerminalThreadMetadataStore {
    pub fn set_agent_session(
        &mut self,
        terminal_id: TerminalId,
        agent_session: Option<TerminalAgentSession>,
        cx: &mut Context<Self>,
    );
}
```

- Later consumers: the reporter calls `set_agent_session`; `AgentPanel` copies `agent_session` out of the cached row while saving routine terminal metadata.

- [ ] **Step 1: Add the required README review marker before any source edit**

Replace the beginning of `README.md` only if it does not already start with:

```markdown
> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.
```

- [ ] **Step 2: Write failing metadata-store tests**

Extend the existing `metadata` helper so generic fixtures construct `agent_session: None`. Add a GPUI test that stores, replaces, and clears a real association:

```rust
#[gpui::test]
async fn test_terminal_agent_session_is_saved_replaced_and_cleared(cx: &mut TestAppContext) {
    init_test(cx);
    let mut terminal_metadata = metadata(
        "OMP",
        WorktreePaths::from_folder_paths(&PathList::default()),
    );
    let terminal_id = terminal_metadata.terminal_id;
    terminal_metadata.agent_session = Some(TerminalAgentSession {
        agent_id: "omp".to_string(),
        resume_target: "session-one".to_string(),
        working_directory: PathBuf::from("/repo/one"),
    });

    cx.update(|cx| {
        TerminalThreadMetadataStore::global(cx).update(cx, |store, cx| {
            store.save(terminal_metadata, cx);
            store.set_agent_session(
                terminal_id,
                Some(TerminalAgentSession {
                    agent_id: "omp".to_string(),
                    resume_target: "session-two".to_string(),
                    working_directory: PathBuf::from("/repo/two"),
                }),
                cx,
            );
        });
    });

    cx.update(|cx| {
        let store = TerminalThreadMetadataStore::global(cx);
        assert_eq!(
            store.read(cx).entry(terminal_id).unwrap().agent_session,
            Some(TerminalAgentSession {
                agent_id: "omp".to_string(),
                resume_target: "session-two".to_string(),
                working_directory: PathBuf::from("/repo/two"),
            }),
        );
    });

    cx.update(|cx| {
        TerminalThreadMetadataStore::global(cx).update(cx, |store, cx| {
            store.set_agent_session(terminal_id, None, cx);
        });
    });
    cx.update(|cx| assert!(TerminalThreadMetadataStore::global(cx)
        .read(cx)
        .entry(terminal_id)
        .unwrap()
        .agent_session
        .is_none()));
}
```

Add the decoder tests next to the lifecycle test:

```rust
assert_eq!(
    terminal_agent_session_from_columns(
        Some("omp".to_string()),
        Some("session-one".to_string()),
        Some("/repo/one".to_string()),
    ),
    Some(TerminalAgentSession {
        agent_id: "omp".to_string(),
        resume_target: "session-one".to_string(),
        working_directory: PathBuf::from("/repo/one"),
    }),
);
assert_eq!(
    terminal_agent_session_from_columns(
        Some("omp".to_string()),
        None,
        Some("/repo/one".to_string()),
    ),
    None,
);
assert_eq!(terminal_agent_session_from_columns(None, None, None), None);
```

- [ ] **Step 3: Run the new test to verify it fails**

Run: `cargo test -p agent_ui terminal_agent_session_is_saved_replaced_and_cleared`

Expected: compile failure because `TerminalAgentSession`, `agent_session`, and `set_agent_session` do not exist.

- [ ] **Step 4: Add the generic model and additive migration**

Define `TerminalAgentSession` beside `TerminalThreadMetadata`. Add the exact nullable columns to the initial schema and append these three migrations in order:

```rust
sql!(ALTER TABLE sidebar_terminal_threads ADD COLUMN agent_id TEXT;),
sql!(ALTER TABLE sidebar_terminal_threads ADD COLUMN agent_resume_target TEXT;),
sql!(ALTER TABLE sidebar_terminal_threads ADD COLUMN agent_working_directory TEXT;),
```

Do not create an OMP-specific column or a serialized command field.

Update every `TerminalThreadMetadata` constructor in this module to provide the field explicitly. Route the select-list values through `terminal_agent_session_from_columns`, and extend the save upsert so complete values serialize as text while all-null/incomplete values deserialize as `None`.

- [ ] **Step 5: Add the cached mutation API**

Implement the mutation by cloning the cached row, changing exactly one field, then reusing the store's existing upsert queue:

```rust
pub fn set_agent_session(
    &mut self,
    terminal_id: TerminalId,
    agent_session: Option<TerminalAgentSession>,
    cx: &mut Context<Self>,
) {
    let Some(mut metadata) = self.entry(terminal_id).cloned() else {
        return;
    };
    if metadata.agent_session == agent_session {
        return;
    }
    metadata.agent_session = agent_session;
    self.save_internal(metadata);
    cx.notify();
}
```

- [ ] **Step 6: Run the focused store tests to verify they pass**

Run: `cargo test -p agent_ui terminal_thread_metadata_store`

Expected: PASS, including title/worktree indexing tests and the new save/replace/clear cases.

- [ ] **Step 7: Commit the independently testable store change**

```bash
git add README.md crates/agent_ui/src/terminal_thread_metadata_store.rs
git commit -m "feat: persist terminal agent session metadata"
```

### Task 2: Build the OMP session reporter and safe agent registry

**Files:**
- Create: `crates/agent_ui/src/terminal_agent_session.rs`
- Modify: `crates/agent_ui/src/agent_ui.rs:1-35,580-621`
- Modify: `crates/agent_ui/src/terminal_thread_metadata_store.rs`
- Test: `crates/agent_ui/src/terminal_agent_session.rs`

**Interfaces:**
- Consumes: `TerminalId`, `TerminalAgentSession`, `TerminalThreadMetadataStore`, `Arc<dyn Fs>`, `paths::data_dir()`, and OMP's `ctx.sessionManager` extension API.
- Produces:

```rust
pub(crate) struct TerminalAgentSessionCapture {
    pub(crate) environment: HashMap<String, String>,
    event_path: PathBuf,
    token: String,
}

#[derive(Deserialize)]
struct TerminalAgentSessionReport {
    version: u8,
    terminal_id: String,
    token: String,
    agent_id: String,
    resume_target: Option<String>,
    working_directory: Option<PathBuf>,
}

pub(crate) struct TerminalAgentSessionReporter;

impl TerminalAgentSessionReporter {
    pub(crate) fn prepare_omp_extension(&mut self, cx: &mut App) -> Task<anyhow::Result<()>>;
    pub(crate) fn register_capture(&mut self, terminal_id: TerminalId) -> TerminalAgentSessionCapture;
    pub(crate) fn unregister_capture(&mut self, terminal_id: TerminalId, cx: &mut App);
    fn apply_report(
        &mut self,
        event_path: &Path,
        report: TerminalAgentSessionReport,
        cx: &mut App,
    ) -> bool;
}

pub(crate) fn resume_command(session: &TerminalAgentSession) -> Option<String>;
```

- Later consumers: `AgentPanel` awaits extension preparation, passes `capture.environment` to Project, and calls `unregister_capture` on failure/close. `resume_command` is the only restore-command builder.

- [ ] **Step 1: Write failing reporter protocol tests**

Put tests in the new module using the same `TestAppContext` style as the metadata store. Add a helper that creates a registered capture and injects one parsed report. Test accepted and rejected paths explicitly:

```rust
let accepted = TerminalAgentSessionReport {
    version: 1,
    terminal_id: terminal_id.to_key_string(),
    token: capture.token.clone(),
    agent_id: "omp".to_string(),
    resume_target: Some("omp_123-ABC".to_string()),
    working_directory: Some(PathBuf::from("/repo/session")),
};
assert!(reporter.apply_report(&capture.event_path, accepted, cx));
assert_eq!(
    store.read(cx).entry(terminal_id).unwrap().agent_session.as_ref().unwrap().resume_target,
    "omp_123-ABC",
);
```

In separate assertions, use a stale token, another `TerminalId`, `agent_id: "unknown"`, `resume_target: Some("x'; rm -rf /")`, and a report with an ID but no cwd. Each returns `false` and leaves the prior session unchanged. A valid report with both optional session fields absent returns `true` and clears the old association.

- [ ] **Step 2: Run the reporter tests to verify they fail**

Run: `cargo test -p agent_ui terminal_agent_session`

Expected: compile failure because the module, capture type, and report handler do not exist.

- [ ] **Step 3: Define the wire model and the single-agent registry**

Implement a private deserializable report with these exact JSON fields:

```rust
#[derive(Deserialize)]
struct TerminalAgentSessionReport {
    version: u8,
    terminal_id: String,
    token: String,
    agent_id: String,
    resume_target: Option<String>,
    working_directory: Option<PathBuf>,
}
```

Reject a file larger than 16 KiB, any `version != 1`, nonmatching terminal/token/path, an unknown agent ID, control characters, an empty cwd, and OMP IDs not matching `^[A-Za-z0-9_-]{1,256}$`. The OMP registry entry returns only:

```rust
Some(format!("omp --resume '{}'", session.resume_target))
```

after re-validating its `agent_id` and target. It returns `None` for all unknown/invalid persisted sessions.

- [ ] **Step 4: Implement capture registration and watched atomic-file delivery**

Make the reporter global own `HashMap<TerminalId, RegisteredCapture>` and an event directory at:

```rust
paths::data_dir().join("terminal-agent-sessions")
```

`register_capture` creates a random token, derives a path named `<terminal-id>.json`, removes a leftover file, records the capture, and returns exactly:

```rust
HashMap::from([
    ("ZED_TERMINAL_THREAD_ID".to_string(), terminal_id.to_key_string()),
    ("ZED_AGENT_SESSION_EVENT_PATH".to_string(), event_path.display().to_string()),
    ("ZED_AGENT_SESSION_TOKEN".to_string(), token.clone()),
])
```

Start one `fs.watch(event_directory, Duration::from_millis(100))` task in `terminal_agent_session::init`. On an event, process only an exact registered final path: load it, deserialize once, validate it, call `TerminalThreadMetadataStore::set_agent_session`, and never log the token or target. `unregister_capture` removes the map entry and the final event file.

- [ ] **Step 5: Materialize the managed OMP extension**

`prepare_omp_extension` must create `util::paths::home_dir().join(".omp/agent/extensions")` only when a local Terminal Thread requests capture. Write `zed-terminal-agent-session.ts` only if it is absent or begins with:

```typescript
// @zed-managed-terminal-agent-session
```

If a same-named user-owned file exists, return an `anyhow` error; the caller will surface it and open an otherwise normal terminal.

Generate a self-contained OMP extension with this behavior:

```typescript
// @zed-managed-terminal-agent-session
const { existsSync, renameSync, writeFileSync } = require("node:fs")

export default function (pi: { on: Function }) {
  const eventPath = process.env.ZED_AGENT_SESSION_EVENT_PATH
  const token = process.env.ZED_AGENT_SESSION_TOKEN
  const terminalId = process.env.ZED_TERMINAL_THREAD_ID
  if (!eventPath || !token || !terminalId) return
  if (process.env.ZED_AGENT_SESSION_OWNER_PID) return
  process.env.ZED_AGENT_SESSION_OWNER_PID = String(process.pid)

  const atomicWrite = (payload: Record<string, unknown>) => {
    const temporaryPath = `${eventPath}.tmp-${process.pid}`
    writeFileSync(temporaryPath, JSON.stringify(payload), { mode: 0o600 })
    renameSync(temporaryPath, eventPath)
  }
  const report = (ctx: any) => {
    const sessionId = ctx.sessionManager?.getSessionId?.()
    const sessionFile = ctx.sessionManager?.getSessionFile?.()
    const persisted = typeof sessionId === "string" && sessionId &&
      typeof sessionFile === "string" && sessionFile && existsSync(sessionFile)
    atomicWrite({
      version: 1, terminal_id: terminalId, token, agent_id: "omp",
      ...(persisted ? { resume_target: sessionId, working_directory: ctx.cwd } : {}),
    })
  }
  for (const event of ["session_start", "session_switch", "agent_start", "agent_end"]) {
    pi.on(event, (_event: unknown, ctx: unknown) => { try { report(ctx) } catch {} })
  }
}
```

Do not await I/O or throw from an extension handler.

- [ ] **Step 6: Initialize the reporter global**

Add `mod terminal_agent_session;` to `agent_ui.rs`, then initialize it after `terminal_thread_metadata_store::init(cx)`:

```rust
terminal_agent_session::init(fs.clone(), cx);
```

Retain the watcher task in the global so dropping a local handle cannot cancel it.

- [ ] **Step 7: Run reporter tests to verify they pass**

Run: `cargo test -p agent_ui terminal_agent_session`

Expected: PASS for accepted report, all rejection cases, clearing an ephemeral session, and OMP command validation.

- [ ] **Step 8: Commit the reporter change**

```bash
git add crates/agent_ui/src/agent_ui.rs \
  crates/agent_ui/src/terminal_agent_session.rs \
  crates/agent_ui/src/terminal_thread_metadata_store.rs
git commit -m "feat: report OMP terminal sessions"
```

### Task 3: Give only local Agent Panel terminals a capture capability

**Files:**
- Modify: `crates/project/src/terminals.rs:284-430`
- Modify: `crates/agent_ui/src/agent_panel.rs:2062-2129,2194-2435`
- Test: `crates/agent_ui/src/agent_panel.rs:7651-7839`

**Interfaces:**
- Consumes: `TerminalAgentSessionReporter::prepare_omp_extension`, `register_capture`, `unregister_capture`, and `TerminalAgentSessionCapture.environment` from Task 2.
- Produces:

```rust
fn merge_terminal_environment(
    resolved_environment: HashMap<String, String>,
    settings_environment: HashMap<String, String>,
    additional_environment: HashMap<String, String>,
    is_via_remote: bool,
) -> HashMap<String, String>;

impl Project {
    pub fn create_terminal_shell_with_environment(
        &mut self,
        cwd: Option<PathBuf>,
        additional_environment: HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Entity<Terminal>>>;
}
```

- Later consumers: only `AgentPanel::spawn_terminal` uses this overload. Existing `create_terminal_shell` and `create_local_terminal` retain their old semantics.

- [ ] **Step 1: Write the failing Project API test**

In the existing `project::terminals` test module, test the extracted environment merge behavior:

```rust
#[test]
fn capture_environment_overrides_settings_for_local_terminals() {
    let environment = merge_terminal_environment(
        HashMap::from([("PATH".into(), "/bin".into())]),
        HashMap::from([("ZED_AGENT_SESSION_TOKEN".into(), "setting".into())]),
        HashMap::from([("ZED_AGENT_SESSION_TOKEN".into(), "capture".into())]),
        false,
    );
    assert_eq!(environment["ZED_AGENT_SESSION_TOKEN"], "capture");
}

#[test]
fn capture_environment_is_excluded_from_remote_terminals() {
    let environment = merge_terminal_environment(
        HashMap::default(),
        HashMap::default(),
        HashMap::from([("ZED_AGENT_SESSION_TOKEN".into(), "capture".into())]),
        true,
    );
    assert!(!environment.contains_key("ZED_AGENT_SESSION_TOKEN"));
}
```

Add this real-PTY Agent Panel test beside `test_spawn_terminal_runs_init_command_in_real_shell`:

```rust
#[cfg(unix)]
#[gpui::test]
async fn test_spawn_terminal_inherits_capture_environment(cx: &mut TestAppContext) {
    let (panel, mut cx) = setup_panel(cx).await;
    cx.executor().allow_parking();
    cx.update(|_, cx| {
        let mut settings = AgentSettings::get_global(cx).clone();
        settings.terminal_init_command =
            Some("printf 'capture=%s\\n' \"$ZED_TERMINAL_THREAD_ID\"".to_string());
        AgentSettings::override_global(settings, cx);

        let mut terminal_settings = TerminalSettings::get_global(cx).clone();
        terminal_settings.shell = task::Shell::Program("/bin/sh".to_string());
        TerminalSettings::override_global(terminal_settings, cx);
    });

    let terminal_id = TerminalId::new();
    panel.update_in(&mut cx, |panel, window, cx| {
        panel.spawn_terminal(
            terminal_id,
            None,
            None,
            None,
            None,
            true,
            true,
            true,
            AgentThreadSource::AgentPanel,
            window,
            cx,
        );
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let content = panel.read_with(&cx, |panel, cx| {
            panel
                .terminals
                .get(&terminal_id)
                .map(|terminal| terminal.view.read(cx).terminal().read(cx).get_content())
        });
        if content
            .as_deref()
            .is_some_and(|content| content.contains(&format!("capture={terminal_id}\n")))
        {
            break;
        }
        assert!(Instant::now() < deadline, "capture environment was not inherited");
        cx.executor().timer(Duration::from_millis(50)).await;
    }
}
```


- [ ] **Step 2: Run the new test to verify it fails**

Run: `cargo test -p agent_ui spawn_terminal`

Expected: compile failure because `create_terminal_shell_with_environment` and the injected environment path do not exist.

- [ ] **Step 3: Add the narrow Project shell API**

Make `create_terminal_shell` call `create_terminal_shell_internal(cwd, false, HashMap::default(), cx)`. Add `create_terminal_shell_with_environment` that calls the same internal function with its provided map. Thread the map into the async closure and use the tested helper:

```rust
let resolved_environment = env_task.await.unwrap_or_default();
let env = merge_terminal_environment(
    resolved_environment,
    settings.env,
    additional_environment,
    is_via_remote,
);
```

Keep `create_local_terminal` passing `HashMap::default()`. The helper discards `additional_environment` when `is_via_remote`, so a future caller cannot accidentally pass capture variables through `create_remote_shell`.

- [ ] **Step 4: Write the failing Agent Panel lifecycle test**

Add a display-only test that saves an OMP session to `TerminalThreadMetadataStore`, changes a terminal title, and verifies the normal metadata save preserves the session:

```rust
assert_eq!(
    TerminalThreadMetadataStore::global(cx)
        .read(cx)
        .entry(terminal_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap()
        .resume_target,
    "omp-session-preserved",
);
```

The focused Project merge tests cover remote exclusion. Do not add a second Agent Panel remote fixture: constructing one would test Project setup rather than the contract under change.

- [ ] **Step 5: Wire local spawn and cleanup**

Restructure `AgentPanel::spawn_terminal` so its async task first determines whether `self.project.read(cx).remote_connection_options(cx).is_none()`.

For local projects:

1. await `prepare_omp_extension`;
2. on success call `register_capture(terminal_id)`;
3. call `create_terminal_shell_with_environment(working_directory, capture.environment, cx)`;
4. if shell creation fails, call `unregister_capture` before the existing workspace error path.

For setup failure, show `workspace.show_error(error, cx)` and create the terminal through the existing `create_terminal_shell` path with no `ZED_*` variables. For remote projects, directly use the existing API and never register a capture.

In `close_terminal_internal`, call `unregister_capture(terminal_id, cx)` before deleting terminal metadata.

- [ ] **Step 6: Preserve reporter-owned metadata on ordinary saves**

Change `AgentPanel::terminal_metadata` to read the cached `agent_session` from `TerminalThreadMetadataStore::try_global(cx)` for its `terminal_id` and include it in the newly built `TerminalThreadMetadata`. If the store/global/entry is unavailable, use `None`; do not synthesize a session.

- [ ] **Step 7: Run local lifecycle tests to verify they pass**

Run: `cargo test -p project capture_environment`

Run: `cargo test -p agent_ui spawn_terminal`

Run: `cargo test -p agent_ui terminal_metadata`

Expected: PASS. The Project helper gives local capture values precedence and excludes them remotely; the local Agent Panel shell inherits its registered `TerminalId`; normal metadata persistence keeps the reporter's association.

- [ ] **Step 8: Commit the terminal capability change**

```bash
git add crates/project/src/terminals.rs crates/agent_ui/src/agent_panel.rs
git commit -m "feat: capture local terminal agent sessions"
```

### Task 4: Restore only the selected terminal's exact OMP session

**Files:**
- Modify: `crates/agent_ui/src/agent_panel.rs:2062-2191,2419-2505,6765-6855,7677-7750`
- Modify: `crates/agent_ui/src/terminal_agent_session.rs`
- Test: `crates/agent_ui/src/agent_panel.rs:7677-7750`

**Interfaces:**
- Consumes: `TerminalAgentSession`, `resume_command`, captured metadata from Task 1, and the existing `write_terminal_init_command` handshake.
- Produces:

```rust
fn terminal_init_command(
    run_init_command: bool,
    agent_session: Option<&TerminalAgentSession>,
    cx: &App,
) -> Option<String>;

fn terminal_restore_working_directory(
    &self,
    metadata: &TerminalThreadMetadata,
    workspace: Option<&Workspace>,
    cx: &App,
) -> Option<PathBuf>;
```

- Behavior contract: valid OMP session → exact OMP resume command and agent cwd; missing/unknown/invalid session → current global-init and cwd fallback behavior.

- [ ] **Step 1: Write failing exact-restore tests**

Replace the single-session body of `test_restored_terminal_runs_init_command_once` with an OMP-specific case and create a distinct persisted value that the test deliberately does not restore. Configure the global command to prove precedence:

```rust
settings.terminal_init_command = Some("printf 'global-init\\n'".to_string());

let metadata = TerminalThreadMetadata {
    terminal_id: TerminalId::new(),
    title: "OMP one".into(),
    custom_title: None,
    created_at: Utc::now(),
    worktree_paths: WorktreePaths::from_folder_paths(&PathList::new(&[PathBuf::from("/worktree")])),
    remote_connection: None,
    working_directory: Some(PathBuf::from("/last-shell-cwd")),
    agent_session: Some(TerminalAgentSession {
        agent_id: "omp".to_string(),
        resume_target: "omp-session-one".to_string(),
        working_directory: PathBuf::from("/agent-session-cwd"),
    }),
};
let second = TerminalThreadMetadata {
    terminal_id: TerminalId::new(),
    title: "OMP two".into(),
    custom_title: None,
    created_at: Utc::now(),
    worktree_paths: WorktreePaths::from_folder_paths(&PathList::new(&[PathBuf::from("/worktree")])),
    remote_connection: None,
    working_directory: Some(PathBuf::from("/second-last-shell-cwd")),
    agent_session: Some(TerminalAgentSession {
        agent_id: "omp".to_string(),
        resume_target: "omp-session-two".to_string(),
        working_directory: PathBuf::from("/second-agent-session-cwd"),
    }),
};
```

After `restore_test_terminal`, assert exactly:

```rust
assert_eq!(input_log, vec![b"omp --resume 'omp-session-one'\r".to_vec()]);
assert_eq!(panel.read_with(&cx, |panel, _| {
    panel.terminals.get(&metadata.terminal_id).unwrap().working_directory.as_deref()
}), Some(Path::new("/agent-session-cwd")));
```

Call restoration for the same terminal again and assert a fresh `take_input_log()` is empty. Do not call restore for the second metadata and assert `!panel.has_terminal(second.terminal_id)`.

Add two separate regression tests: `agent_session: None` yields `global-init\r` once; `agent_id: "unknown"` or target `"bad'; command"` also yields `global-init\r` once and never emits a resume string.

- [ ] **Step 2: Run restoration tests to verify they fail**

Run: `cargo test -p agent_ui restored_terminal`

Expected: compile failure because metadata lacks `agent_session`; once it compiles, failure because the current initializer still emits the global command and chooses `/last-shell-cwd`.

- [ ] **Step 3: Select the session cwd and a single safe startup command**

Thread `Option<TerminalAgentSession>` from `restore_terminal` into `spawn_terminal` and into the display-only test path. Implement restore cwd precedence:

```rust
if let Some(agent_session) = metadata.agent_session.as_ref() {
    return Some(agent_session.working_directory.clone());
}
if let Some(working_directory) = metadata.working_directory.clone() {
    return Some(working_directory);
}
```

Then retain the existing workspace and panel-default fallbacks.

Implement initializer precedence:

```rust
if let Some(command) = agent_session.and_then(resume_command) {
    return Some(command);
}
run_init_command
    .then(|| AgentSettings::get_global(cx).terminal_init_command.clone())
    .flatten()
    .filter(|command| !command.trim().is_empty())
```

Pass the chosen string to the existing `write_terminal_init_command` once. Do not add another PTY write, shell parser, or asynchronous resume path.

- [ ] **Step 4: Run restoration tests to verify they pass**

Run: `cargo test -p agent_ui restored_terminal`

Expected: PASS. The valid OMP case writes `omp --resume 'omp-session-one'` exactly once, selection does not spawn the other thread, reactivation writes nothing, and generic/invalid cases retain the global init behavior.

- [ ] **Step 5: Run the real-PTY regression test**

Run: `cargo test -p agent_ui test_spawn_terminal_runs_init_command_in_real_shell`

Expected: PASS. The pre-existing one-time shell handshake still delivers a command once and does not mark terminal keyboard input as user input.

- [ ] **Step 6: Commit the restoration change**

```bash
git add crates/agent_ui/src/agent_panel.rs crates/agent_ui/src/terminal_agent_session.rs
git commit -m "feat: resume persisted terminal agent sessions"
```

## Final verification and cleanup

**Files:**
- Verify: `crates/agent_ui/src/terminal_thread_metadata_store.rs`
- Verify: `crates/agent_ui/src/terminal_agent_session.rs`
- Verify: `crates/agent_ui/src/agent_panel.rs`
- Verify: `crates/project/src/terminals.rs`
- Manual artifact: Zed data directory's test event files; remove only disposable test files.

**Interfaces:**
- Consumes: all four completed tasks.
- Produces: evidence that two manually created OMP sessions resume independently and lazily.

- [ ] **Step 1: Run the focused automated verification set**

Run:

```bash
cargo test -p agent_ui terminal_thread_metadata_store
cargo test -p agent_ui terminal_agent_session
cargo test -p agent_ui restored_terminal
cargo test -p agent_ui test_spawn_terminal_runs_init_command_in_real_shell
cargo check -p agent_ui -p project
```

Expected: every command exits successfully. Fix any regression before continuing.

- [ ] **Step 2: Execute the manual two-session smoke test**

Launch the built Zed from this worktree against a disposable local project. In Agent Panel:

1. Create Terminal Thread A; type `omp`; create a persistent OMP session and note its displayed/session ID.
2. Create Terminal Thread B; type `omp`; create a distinct persistent OMP session.
3. Close Zed completely, then relaunch it.
4. Select only Thread A from the persisted terminal-thread UI.

Expected: only Thread A opens; its shell cwd is its reported OMP session cwd; terminal input shows one `omp --resume '<A-id>'`; no global terminal-init command is run. Select B separately and verify it receives only `omp --resume '<B-id>'`.

- [ ] **Step 3: Verify the negative manual path**

Create a third thread, invoke OMP without producing a persistent session (for example, exit before a session file materializes), close/relaunch Zed, and select it.

Expected: no OMP resume command is injected; the thread follows the existing global-init behavior.

- [ ] **Step 4: Remove only disposable event files and recheck the diff**

Delete only event files created by the smoke test under Zed's data directory. Leave the marked managed OMP extension in `~/.omp/agent/extensions`; it must remain installed and inert outside Zed-captured terminals.

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors; only intended source, test, README marker, and approved Superpowers documentation changes remain.

