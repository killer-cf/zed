use anyhow::{Context as _, Result};
use collections::HashMap;
use fs::{CreateOptions, Fs, RemoveOptions};
use futures::StreamExt;
use gpui::{App, AppContext as _, Global, Task, TaskExt as _, UpdateGlobal as _};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use util::paths::home_dir;

use crate::{
    TerminalId,
    terminal_thread_metadata_store::{
        TerminalAgentSession, TerminalThreadMetadataStore,
    },
};

const EVENT_DIRECTORY: &str = "terminal-agent-sessions";
const MAX_EVENT_FILE_SIZE: usize = 16 * 1024;
const OMP_AGENT_ID: &str = "omp";
const MANAGED_EXTENSION_MARKER: &str = "// @zed-managed-terminal-agent-session";
const OMP_EXTENSION_FILE_NAME: &str = "zed-terminal-agent-session.ts";

const OMP_EXTENSION: &str = r#"// @zed-managed-terminal-agent-session
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
"#;

pub(crate) struct TerminalAgentSessionCapture {
    pub(crate) environment: HashMap<String, String>,
    event_path: PathBuf,
    token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalAgentSessionReport {
    version: u8,
    terminal_id: String,
    token: String,
    agent_id: String,
    resume_target: Option<String>,
    working_directory: Option<PathBuf>,
}

struct RegisteredCapture {
    token: String,
    event_path: PathBuf,
}

pub(crate) struct TerminalAgentSessionReporter {
    fs: Arc<dyn Fs>,
    event_directory: PathBuf,
    captures: HashMap<TerminalId, RegisteredCapture>,
    _watcher_task: Task<()>,
}

impl Global for TerminalAgentSessionReporter {}

pub(crate) fn init(fs: Arc<dyn Fs>, cx: &mut App) {
    if cx.has_global::<TerminalAgentSessionReporter>() {
        return;
    }

    let event_directory = paths::data_dir().join(EVENT_DIRECTORY);
    let watcher_task = cx.spawn({
        let fs = fs.clone();
        let event_directory = event_directory.clone();
        async move |cx| {
            // The watch starts after creating the directory so native watchers can register it.
            // Failures are intentionally ignored: reporting must never affect an OMP turn.
            let _ = fs.create_dir(&event_directory).await;
            let (mut events, _watcher) = fs
                .watch(&event_directory, Duration::from_millis(100))
                .await;

            while let Some(events) = events.next().await {
                for event in events {
                    let event_path = event.path;
                    let registered = cx
                        .update(|cx| {
                            TerminalAgentSessionReporter::update_global(
                                cx,
                                |reporter, _cx| reporter.is_registered_event_path(&event_path),
                            )
                        })
                        .unwrap_or(false);
                    if !registered {
                        continue;
                    }

                    let Ok(Some(metadata)) = fs.metadata(&event_path).await else {
                        continue;
                    };
                    if metadata.len > MAX_EVENT_FILE_SIZE as u64 {
                        continue;
                    }
                    let Ok(bytes) = fs.load_bytes(&event_path).await else {
                        continue;
                    };
                    if bytes.len() > MAX_EVENT_FILE_SIZE {
                        continue;
                    }
                    let Ok(report) =
                        serde_json::from_slice::<TerminalAgentSessionReport>(&bytes)
                    else {
                        continue;
                    };

                    let _ = cx.update(|cx| {
                        TerminalAgentSessionReporter::update_global(cx, |reporter, cx| {
                            reporter.apply_report(&event_path, report, cx)
                        })
                    });
                }
            }
        }
    });

    cx.set_global(TerminalAgentSessionReporter {
        fs,
        event_directory,
        captures: HashMap::default(),
        _watcher_task: watcher_task,
    });
}

impl TerminalAgentSessionReporter {
    pub(crate) fn prepare_omp_extension(&mut self, cx: &mut App) -> Task<Result<()>> {
        if self.captures.is_empty() {
            return Task::ready(Ok(()));
        }

        let fs = self.fs.clone();
        let extension_directory = home_dir().join(".omp/agent/extensions");
        let extension_path = extension_directory.join(OMP_EXTENSION_FILE_NAME);
        cx.background_spawn(async move {
            fs.create_dir(&extension_directory)
                .await
                .context("creating OMP extension directory")?;

            if let Some(metadata) = fs
                .metadata(&extension_path)
                .await
                .context("checking OMP terminal session extension")?
            {
                if metadata.is_symlink || metadata.is_dir {
                    anyhow::bail!(
                        "OMP terminal session extension path is not a managed file: {}",
                        extension_path.display()
                    );
                }

                let contents = fs
                    .load_bytes(&extension_path)
                    .await
                    .context("reading OMP terminal session extension")?;
                if !contents.starts_with(MANAGED_EXTENSION_MARKER.as_bytes()) {
                    anyhow::bail!(
                        "OMP terminal session extension is owned by the user: {}",
                        extension_path.display()
                    );
                }
                return Ok(());
            }

            // Reserve the path without replacing a file that may have appeared since metadata
            // was read. Only the reserved file is populated with the managed extension.
            fs.create_file(&extension_path, CreateOptions::default())
                .await
                .context("creating OMP terminal session extension")?;
            fs.write(&extension_path, OMP_EXTENSION.as_bytes())
                .await
                .context("writing OMP terminal session extension")?;
            Ok(())
        })
    }

    pub(crate) fn register_capture(
        &mut self,
        terminal_id: TerminalId,
    ) -> TerminalAgentSessionCapture {
        let terminal_id_string = terminal_id.to_key_string();
        let token = uuid::Uuid::new_v4().to_string();
        let event_path = self
            .event_directory
            .join(format!("{terminal_id_string}.json"));

        // Registration is synchronous by design. Event files are local paths, so remove a
        // leftover final file before exposing the new token to the terminal process.
        let _ = std::fs::remove_file(&event_path);

        self.captures.insert(
            terminal_id,
            RegisteredCapture {
                token: token.clone(),
                event_path: event_path.clone(),
            },
        );

        TerminalAgentSessionCapture {
            environment: HashMap::from([
                (
                    "ZED_TERMINAL_THREAD_ID".to_string(),
                    terminal_id_string,
                ),
                (
                    "ZED_AGENT_SESSION_EVENT_PATH".to_string(),
                    event_path.display().to_string(),
                ),
                ("ZED_AGENT_SESSION_TOKEN".to_string(), token.clone()),
            ]),
            event_path,
            token,
        }
    }

    pub(crate) fn unregister_capture(&mut self, terminal_id: TerminalId, cx: &mut App) {
        let Some(capture) = self.captures.remove(&terminal_id) else {
            return;
        };
        let event_path = capture.event_path;
        let _ = std::fs::remove_file(&event_path);

        let fs = self.fs.clone();
        cx.background_spawn(async move {
            fs.remove_file(
                &event_path,
                RemoveOptions {
                    ignore_if_not_exists: true,
                    ..RemoveOptions::default()
                },
            )
            .await?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn is_registered_event_path(&self, event_path: &Path) -> bool {
        self.captures
            .values()
            .any(|capture| capture.event_path == event_path)
    }

    fn apply_report(
        &mut self,
        event_path: &Path,
        report: TerminalAgentSessionReport,
        cx: &mut App,
    ) -> bool {
        let Ok(terminal_id) = TerminalId::from_key_string(&report.terminal_id) else {
            return false;
        };
        if report.terminal_id != terminal_id.to_key_string() {
            return false;
        }

        let Some(capture) = self.captures.get(&terminal_id) else {
            return false;
        };
        if capture.event_path != event_path || capture.token != report.token {
            return false;
        }
        if !valid_report(&report) {
            return false;
        }

        let agent_session = match (report.resume_target, report.working_directory) {
            (Some(resume_target), Some(working_directory)) => Some(TerminalAgentSession {
                agent_id: OMP_AGENT_ID.to_string(),
                resume_target,
                working_directory,
            }),
            (None, None) => None,
            _ => return false,
        };

        if let Some(store) = TerminalThreadMetadataStore::try_global(cx) {
            store.update(cx, |store, cx| {
                store.set_agent_session(terminal_id, agent_session, cx);
            });
        }
        true
    }
}

pub(crate) fn resume_command(session: &TerminalAgentSession) -> Option<String> {
    if session.agent_id != OMP_AGENT_ID
        || !is_valid_omp_id(&session.resume_target)
        || session.working_directory.as_os_str().is_empty()
        || has_control_characters(&session.working_directory.to_string_lossy())
    {
        return None;
    }
    Some(format!("omp --resume '{}'", session.resume_target))
}

fn valid_report(report: &TerminalAgentSessionReport) -> bool {
    if report.version != 1
        || report.agent_id != OMP_AGENT_ID
        || has_control_characters(&report.terminal_id)
        || has_control_characters(&report.token)
        || has_control_characters(&report.agent_id)
    {
        return false;
    }

    if let Some(resume_target) = report.resume_target.as_deref()
        && (!is_valid_omp_id(resume_target) || has_control_characters(resume_target))
    {
        return false;
    }
    if let Some(working_directory) = report.working_directory.as_deref() {
        if working_directory.as_os_str().is_empty()
            || has_control_characters(&working_directory.to_string_lossy())
        {
            return false;
        }
    }
    true
}

fn is_valid_omp_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn has_control_characters(value: &str) -> bool {
    value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use fs::{FakeFs, Fs};
    use gpui::TestAppContext;
    use gpui::UpdateGlobal as _;
    use std::path::PathBuf;
    use workspace::PathList;

    use crate::terminal_thread_metadata_store::{
        TerminalAgentSession, TerminalThreadMetadata, TerminalThreadMetadataStore,
    };
    use crate::thread_metadata_store::WorktreePaths;
    use crate::TerminalId;

    fn init_test(cx: &mut TestAppContext) {
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            <dyn Fs>::set_global(fs.clone(), cx);
            TerminalThreadMetadataStore::init_global(cx);
            init(fs, cx);
        });
        cx.run_until_parked();
    }

    fn metadata(terminal_id: TerminalId) -> TerminalThreadMetadata {
        TerminalThreadMetadata {
            terminal_id,
            title: "OMP".into(),
            custom_title: None,
            created_at: Utc::now(),
            worktree_paths: WorktreePaths::from_folder_paths(&PathList::default()),
            remote_connection: None,
            working_directory: None,
            agent_session: None,
        }
    }

    fn register_capture(
        cx: &mut TestAppContext,
        terminal_id: TerminalId,
    ) -> TerminalAgentSessionCapture {
        cx.update(|cx| {
            TerminalAgentSessionReporter::update_global(cx, |reporter, _cx| {
                reporter.register_capture(terminal_id)
            })
        })
    }

    fn apply_report(
        cx: &mut TestAppContext,
        event_path: &std::path::Path,
        report: TerminalAgentSessionReport,
    ) -> bool {
        cx.update(|cx| {
            TerminalAgentSessionReporter::update_global(cx, |reporter, cx| {
                reporter.apply_report(event_path, report, cx)
            })
        })
    }

    #[gpui::test]
    async fn test_reporter_accepts_registered_reports_and_rejects_stale_or_invalid_reports(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        let terminal_id = TerminalId::new();
        let other_terminal_id = TerminalId::new();
        let capture = register_capture(cx, terminal_id);
        assert_eq!(
            capture.environment,
            HashMap::from([
                (
                    "ZED_TERMINAL_THREAD_ID".to_string(),
                    terminal_id.to_key_string(),
                ),
                (
                    "ZED_AGENT_SESSION_EVENT_PATH".to_string(),
                    capture.event_path.display().to_string(),
                ),
                (
                    "ZED_AGENT_SESSION_TOKEN".to_string(),
                    capture.token.clone(),
                ),
            ])
        );

        cx.update(|cx| {
            TerminalThreadMetadataStore::global(cx).update(cx, |store, cx| {
                store.save(
                    TerminalThreadMetadata {
                        agent_session: Some(TerminalAgentSession {
                            agent_id: "omp".to_string(),
                            resume_target: "old-session".to_string(),
                            working_directory: PathBuf::from("/repo/old"),
                        }),
                        ..metadata(terminal_id)
                    },
                    cx,
                );
            });
        });

        let accepted = TerminalAgentSessionReport {
            version: 1,
            terminal_id: terminal_id.to_key_string(),
            token: capture.token.clone(),
            agent_id: "omp".to_string(),
            resume_target: Some("omp_123-ABC".to_string()),
            working_directory: Some(PathBuf::from("/repo/session")),
        };
        assert!(apply_report(cx, &capture.event_path, accepted));
        cx.update(|cx| {
            let session = TerminalThreadMetadataStore::global(cx)
                .read(cx)
                .entry(terminal_id)
                .unwrap()
                .agent_session
                .as_ref()
                .unwrap()
                .clone();
            assert_eq!(session.resume_target, "omp_123-ABC");
            assert_eq!(session.working_directory, PathBuf::from("/repo/session"));
        });

        let previous = cx.update(|cx| {
            TerminalThreadMetadataStore::global(cx)
                .read(cx)
                .entry(terminal_id)
                .unwrap()
                .agent_session
                .clone()
                .unwrap()
        });
        let mut rejected = TerminalAgentSessionReport {
            version: 1,
            terminal_id: terminal_id.to_key_string(),
            token: "stale-token".to_string(),
            agent_id: "omp".to_string(),
            resume_target: Some("stale".to_string()),
            working_directory: Some(PathBuf::from("/repo/stale")),
        };
        assert!(!apply_report(cx, &capture.event_path, rejected));
        rejected.token = capture.token.clone();
        rejected.terminal_id = other_terminal_id.to_key_string();
        assert!(!apply_report(cx, &capture.event_path, rejected));
        rejected.terminal_id = terminal_id.to_key_string();
        rejected.agent_id = "unknown".to_string();
        assert!(!apply_report(cx, &capture.event_path, rejected));
        rejected.agent_id = "omp".to_string();
        rejected.resume_target = Some("x'; rm -rf /".to_string());
        assert!(!apply_report(cx, &capture.event_path, rejected));
        rejected.resume_target = Some("missing-cwd".to_string());
        rejected.working_directory = None;
        assert!(!apply_report(cx, &capture.event_path, rejected));
        cx.update(|cx| {
            assert_eq!(
                TerminalThreadMetadataStore::global(cx)
                    .read(cx)
                    .entry(terminal_id)
                    .unwrap()
                    .agent_session
                    .as_ref(),
                Some(&previous),
            );
        });

        let clears_session = TerminalAgentSessionReport {
            version: 1,
            terminal_id: terminal_id.to_key_string(),
            token: capture.token,
            agent_id: "omp".to_string(),
            resume_target: None,
            working_directory: None,
        };
        assert!(apply_report(cx, &capture.event_path, clears_session));
        cx.update(|cx| {
            assert!(
                TerminalThreadMetadataStore::global(cx)
                    .read(cx)
                    .entry(terminal_id)
                    .unwrap()
                    .agent_session
                    .is_none()
            );
        });
    }

    #[test]
    fn test_resume_command_revalidates_omp_agent_and_target() {
        let valid = TerminalAgentSession {
            agent_id: "omp".to_string(),
            resume_target: "omp_123-ABC".to_string(),
            working_directory: PathBuf::from("/repo"),
        };
        assert_eq!(
            resume_command(&valid).as_deref(),
            Some("omp --resume 'omp_123-ABC'")
        );

        for target in ["", "x'; rm -rf /", "omp\nagent"] {
            let mut session = valid.clone();
            session.resume_target = target.to_string();
            assert_eq!(resume_command(&session), None);
        }
        let mut session = valid;
        session.resume_target = "x".repeat(257);
        assert_eq!(resume_command(&session), None);
    }
}
