//! Per-subsystem source traits + materializer for cross-crate
//! reminder state.
//!
//! See `traits.rs` for the contract, `materialized.rs` for the
//! transient I/O types, `noop.rs` for default test-friendly impls.
//!
//! **Why this module exists**: reminder state is scattered across ~10 sibling
//! subsystems. The Rust design is a bundle of trait objects: each subsystem
//! (`hooks`, `services/lsp`, `tasks`, `skills`, `services/mcp`, swarm,
//! `bridge`, `memory`) implements its category-specific `*Source` trait in
//! its own crate. `QueryEngine` holds one `ReminderSources` field; CLI wires
//! it up at session start.
//!
//! **Layering**: the trait defs live here (`core/system-reminder`);
//! owning crates depend on this crate to `impl`. One-way edge, no cycles.
//! Matches the established `core/tool-runtime::*Handle` pattern.
//!
//! **Collection semantics**: [`ReminderSources::materialize`] fan-outs all
//! source calls via `tokio::join!`, wraps each in a per-source
//! `tokio::time::timeout` (defaulting to `SystemReminderConfig::timeout_ms`),
//! and degrades errors/timeouts to default values. Never poisons the turn.

pub mod materialized;
pub mod noop;
pub mod traits;

pub use materialized::MaterializeContext;
pub use materialized::MaterializedSources;
pub use noop::NoOpDiagnosticsSource;
pub use noop::NoOpHookEventsSource;
pub use noop::NoOpIdeBridgeSource;
pub use noop::NoOpMcpSource;
pub use noop::NoOpMemorySource;
pub use noop::NoOpSkillsSource;
pub use noop::NoOpSwarmSource;
pub use noop::NoOpTaskStatusSource;
pub use traits::DiagnosticsSource;
pub use traits::HookEventsSource;
pub use traits::IdeBridgeSource;
pub use traits::McpSource;
pub use traits::MemorySource;
pub use traits::ReminderSources;
pub use traits::SkillsSource;
pub use traits::SwarmSource;
pub use traits::TaskStatusSource;

use std::future::Future;
use std::time::Duration;

use tokio::time::timeout;

impl ReminderSources {
    /// Materialize every applicable source in parallel, respecting
    /// per-source timeouts + config gates. Returns a flat
    /// [`MaterializedSources`] struct that the engine spreads into
    /// `TurnReminderInput`.
    ///
    /// Contract:
    /// - If a source field is `None` → that field's output stays at
    ///   its `Default` (empty vec / `None`).
    /// - If a source's reminder is config-disabled → skipped (saves
    ///   the source call).
    /// - If a source times out or panics/errors → logged + default.
    ///   Never propagates.
    pub async fn materialize(&self, mctx: MaterializeContext<'_>) -> MaterializedSources {
        let MaterializeContext {
            config,
            agent_id,
            user_input,
            mentioned_paths,
            recent_tools,
            just_compacted,
            per_source_timeout,
            skill_overrides,
            skill_tool_loaded,
        } = mctx;
        let a = agent_id;
        let t = per_source_timeout;
        // Clone into an Arc so the per-source async tasks can move
        // their own handle without borrowing back into the
        // `MaterializeContext` lifetime.
        let so = std::sync::Arc::new(skill_overrides.clone());

        // Hook events — gated on ANY hook reminder being enabled (all
        // five share one source call and the generators filter
        // internally).
        let any_hook_enabled = config.attachments.hook_success
            || config.attachments.hook_blocking_error
            || config.attachments.hook_additional_context
            || config.attachments.hook_stopped_continuation
            || config.attachments.async_hook_response;

        let hook_events_fut = gate(self.hook_events.as_ref(), any_hook_enabled, t, |s| {
            let s = s.clone();
            async move { s.drain(a).await }
        });

        let diagnostics_fut = gate(
            self.diagnostics.as_ref(),
            config.attachments.diagnostics,
            t,
            |s| {
                let s = s.clone();
                async move { s.snapshot(a).await }
            },
        );

        let task_status_fut = gate(
            self.task_status.as_ref(),
            config.attachments.task_status,
            t,
            |s| {
                let s = s.clone();
                async move { s.collect(a, just_compacted).await }
            },
        );

        let skill_listing_fut = {
            let so = so.clone();
            gate(
                self.skills.as_ref(),
                config.attachments.skill_listing && skill_tool_loaded,
                t,
                move |s| {
                    let s = s.clone();
                    async move { s.listing(a, &so).await }
                },
            )
        };

        let invoked_skills_fut = gate(
            self.skills.as_ref(),
            config.attachments.invoked_skills,
            t,
            |s| {
                let s = s.clone();
                async move { s.invoked(a).await }
            },
        );
        let skill_discovery_fut = {
            let s = self
                .skills
                .as_ref()
                .filter(|_| config.attachments.skill_discovery)
                // Gate on the Skill tool being loaded, exactly like
                // `skill_listing` above. The discovery reminder tells the model
                // to "Invoke via `Skill(\"<name>\")`" — unactionable when the
                // Skill tool is filtered out of this turn's tool set (e.g. a
                // subagent with a restrictive tool filter). This is deliberately
                // STRICTER than literal TS: TS registers `SkillTool`
                // unconditionally and gates discovery only on the
                // `EXPERIMENTAL_SKILL_SEARCH` feature, with no Skill-tool check.
                // coco-rs adds the tool-loaded check to uphold the "never
                // advertise an unavailable Skill tool" invariant.
                .filter(|_| skill_tool_loaded)
                .filter(|_| user_input.is_some());
            let input_owned = user_input.unwrap_or("").to_string();
            let so = so.clone();
            async move {
                match s {
                    Some(s) => {
                        let s = s.clone();
                        match timeout(t, async move { s.skill_discovery(&input_owned, &so).await })
                            .await
                        {
                            Ok(v) => v,
                            Err(_) => {
                                tracing::warn!(
                                    timeout_ms = t.as_millis() as u64,
                                    "skills.skill_discovery timed out"
                                );
                                None
                            }
                        }
                    }
                    None => None,
                }
            }
        };

        let mcp_instructions_fut = gate(
            self.mcp.as_ref(),
            config.attachments.mcp_instructions_delta,
            t,
            |s| {
                let s = s.clone();
                async move { s.instructions(a).await }
            },
        );

        // MCP resources only resolves when the user actually submitted
        // text this turn (UserPrompt tier gate).
        let mcp_resources_fut = {
            let s = self
                .mcp
                .as_ref()
                .filter(|_| config.attachments.mcp_resources)
                .filter(|_| user_input.is_some());
            let input_owned = user_input.unwrap_or("").to_string();
            async move {
                match s {
                    Some(s) => {
                        let s = s.clone();
                        match timeout(t, async move { s.resolve_resources(a, &input_owned).await })
                            .await
                        {
                            Ok(v) => v,
                            Err(_) => {
                                tracing::warn!(
                                    timeout_ms = t.as_millis() as u64,
                                    "mcp.resolve_resources timed out"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => Vec::new(),
                }
            }
        };

        let teammate_mailbox_fut = gate(
            self.swarm.as_ref(),
            config.attachments.teammate_mailbox,
            t,
            |s| {
                let s = s.clone();
                async move { s.teammate_mailbox(a).await }
            },
        );

        let team_context_fut = gate(
            self.swarm.as_ref(),
            config.attachments.team_context,
            t,
            |s| {
                let s = s.clone();
                async move { s.team_context(a).await }
            },
        );

        let agent_pending_messages_fut = gate(
            self.swarm.as_ref(),
            config.attachments.agent_pending_messages,
            t,
            |s| {
                let s = s.clone();
                async move { s.agent_pending_messages(a).await }
            },
        );

        let ide_selection_fut = gate(
            self.ide.as_ref(),
            config.attachments.ide_selection,
            t,
            |s| {
                let s = s.clone();
                async move { s.selection(a).await }
            },
        );

        let ide_opened_file_fut = gate(
            self.ide.as_ref(),
            config.attachments.ide_opened_file,
            t,
            |s| {
                let s = s.clone();
                async move { s.opened_file(a).await }
            },
        );

        let nested_memories_fut = {
            let s = self
                .memory
                .as_ref()
                .filter(|_| config.attachments.nested_memory);
            let paths_owned: Vec<std::path::PathBuf> = mentioned_paths.to_vec();
            async move {
                match s {
                    Some(s) => {
                        let s = s.clone();
                        match timeout(t, async move { s.nested_memories(a, &paths_owned).await })
                            .await
                        {
                            Ok(v) => v,
                            Err(_) => {
                                tracing::warn!(
                                    timeout_ms = t.as_millis() as u64,
                                    "memory.nested_memories timed out"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => Vec::new(),
                }
            }
        };

        let relevant_memories_fut = {
            let s = self
                .memory
                .as_ref()
                .filter(|_| config.attachments.relevant_memories);
            let input_owned = user_input.unwrap_or("").to_string();
            let recent_tools_owned: Vec<String> = recent_tools.to_vec();
            async move {
                match s {
                    Some(s) => {
                        let s = s.clone();
                        match timeout(t, async move {
                            s.relevant_memories(a, &input_owned, &recent_tools_owned)
                                .await
                        })
                        .await
                        {
                            Ok(v) => v,
                            Err(_) => {
                                tracing::warn!(
                                    timeout_ms = t.as_millis() as u64,
                                    "memory.relevant_memories timed out"
                                );
                                Vec::new()
                            }
                        }
                    }
                    None => Vec::new(),
                }
            }
        };

        let (
            hook_events,
            diagnostics,
            task_statuses,
            skill_listing,
            invoked_skills,
            skill_discovery,
            mcp_instructions_current,
            mcp_resources,
            teammate_mailbox,
            team_context,
            agent_pending_messages,
            ide_selection,
            ide_opened_file,
            nested_memories,
            relevant_memories,
        ) = tokio::join!(
            hook_events_fut,
            diagnostics_fut,
            task_status_fut,
            skill_listing_fut,
            invoked_skills_fut,
            skill_discovery_fut,
            mcp_instructions_fut,
            mcp_resources_fut,
            teammate_mailbox_fut,
            team_context_fut,
            agent_pending_messages_fut,
            ide_selection_fut,
            ide_opened_file_fut,
            nested_memories_fut,
            relevant_memories_fut,
        );

        MaterializedSources {
            hook_events,
            diagnostics,
            task_statuses,
            skill_listing,
            skill_discovery,
            invoked_skills,
            mcp_instructions_current,
            mcp_resources,
            teammate_mailbox,
            team_context,
            agent_pending_messages,
            ide_selection,
            ide_opened_file,
            nested_memories,
            relevant_memories,
        }
    }
}

/// Helper: config-gate + timeout + error-to-default. Pulls a source
/// from an `Option<Arc<dyn T>>`, checks an `enabled` boolean, wraps
/// in `tokio::time::timeout`, and on miss returns `T::default()`.
async fn gate<S, F, Fut, O>(
    source: Option<&std::sync::Arc<S>>,
    enabled: bool,
    timeout_duration: Duration,
    f: F,
) -> O
where
    S: ?Sized,
    F: FnOnce(&std::sync::Arc<S>) -> Fut,
    Fut: Future<Output = O>,
    O: Default,
{
    let Some(s) = source.filter(|_| enabled) else {
        return O::default();
    };
    match timeout(timeout_duration, f(s)).await {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                timeout_ms = timeout_duration.as_millis() as u64,
                "reminder source timed out"
            );
            O::default()
        }
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
