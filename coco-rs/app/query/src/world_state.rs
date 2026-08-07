//! Diffing the model-visible world state.
//!
//! One place computes every "has the model been told this?" delta, against one
//! persisted baseline: [`coco_types::WorldStateSnapshot`], stored per agent
//! scope on `ToolAppState` and written into the session transcript beside the
//! history it describes.
//!
//! Why one place. These deltas used to be four independent baselines with four
//! diff functions, four compaction gates and four bookkeeping arms — and
//! because every baseline lived only in memory, none of them survived process
//! restart. A resumed session recomputed each delta against an empty baseline
//! and re-announced the whole inventory on top of a restored history that
//! already contained the announcements; the MCP-instructions delta re-emitted
//! every server's full instruction block. Folding them into one serializable
//! snapshot fixes that once rather than five times, and makes adding a section
//! a single-file change.
//!
//! ## Two-phase commit
//!
//! [`diff`] returns the reminders *and* a candidate snapshot, but does not
//! adopt it. A generator can be disabled by config, time out, or error, in
//! which case its reminder never reaches history and its baseline must not
//! advance — otherwise the announcement is lost for the rest of the session.
//! [`WorldStateDelta::adopt_fired`] advances exactly the sections whose
//! reminder actually fired.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;

use coco_system_reminder::AgentListingDeltaInfo;
use coco_system_reminder::DeferredToolsDeltaInfo;
use coco_system_reminder::McpInstructionsDeltaInfo;
use coco_system_reminder::McpServerSummary;
use coco_system_reminder::McpServersDeltaInfo;
use coco_system_reminder::ModelSwitchInfo;
use coco_types::AttachmentKind;
use coco_types::ContentDigest;
use coco_types::McpServerAnnouncementState;
use coco_types::WorldStateSnapshot;

/// Deferred-tool wire names that no longer exist. Announcing their *removal*
/// to a model resuming an old session is noise about tools it never used, so
/// they are filtered out of both sides of the diff rather than reported.
const RETIRED_DEFERRED_TOOL_NAMES: &[&str] = &[
    "Frame",
    "FrameRead",
    "TeamCreate",
    "TeamDelete",
    "SuggestBackgroundPR",
];

/// Maximum MCP servers listed in one `mcp_servers_delta` announcement.
const MAX_ANNOUNCED_MCP_SERVERS: usize = 8;

fn is_retired_deferred_tool_name(name: &str) -> bool {
    RETIRED_DEFERRED_TOOL_NAMES.contains(&name)
}

/// This turn's world, as the engine sees it, before comparing with what the
/// model was told.
pub(crate) struct WorldStateInput<'a> {
    /// Model id this turn runs as.
    pub model: &'a str,
    /// Tool wire-names reachable only through `ToolSearch` this turn.
    pub deferred_tools: &'a [String],
    /// Tool wire-names whose schemas are already in the request. A name that
    /// moved deferred → loaded is not a removal.
    pub loaded_tools: &'a [String],
    pub agent_types: &'a [String],
    pub mcp_servers: &'a [McpServerSummary],
    /// Server name → instructions text.
    pub mcp_instructions: &'a HashMap<String, String>,
}

/// What to tell the model, plus the baseline that describes the telling.
pub(crate) struct WorldStateDelta {
    pub deferred_tools: Option<DeferredToolsDeltaInfo>,
    pub agent_listing: Option<AgentListingDeltaInfo>,
    pub mcp_servers: Option<McpServersDeltaInfo>,
    pub mcp_instructions: Option<McpInstructionsDeltaInfo>,
    pub model_switch: Option<ModelSwitchInfo>,
    /// The snapshot that would be true once every reminder above has landed.
    /// Adopt through [`Self::adopt_fired`], never wholesale.
    candidate: WorldStateSnapshot,
}

impl WorldStateDelta {
    /// Advance `baseline` for each section whose announcement actually
    /// entered model-visible history.
    ///
    /// `fired` is the set of [`AttachmentKind`]s collected from the injected
    /// model-visible attachments — NOT from what the generators emitted. The
    /// distinction is the point: the baseline asserts "the model has been
    /// told", so the criterion must be the message that landed, not the
    /// intent upstream of the silence/visibility routing. A section that did
    /// not land keeps its previous value and retries next turn.
    ///
    /// The model section is the exception: its candidate is adopted whenever
    /// the model changed, fired or not. The alternative is worse — a suppressed
    /// switch notice would re-fire on every subsequent turn for the rest of the
    /// session, since the mismatch it reports never resolves on its own.
    pub(crate) fn adopt_fired(
        &self,
        baseline: &mut WorldStateSnapshot,
        fired: &HashSet<AttachmentKind>,
    ) {
        if fired.contains(&AttachmentKind::DeferredToolsDelta) {
            baseline.deferred_tools = self.candidate.deferred_tools.clone();
        }
        if fired.contains(&AttachmentKind::AgentListingDelta) {
            baseline.agent_types = self.candidate.agent_types.clone();
        }
        if fired.contains(&AttachmentKind::McpServersDelta) {
            baseline.mcp_servers = self.candidate.mcp_servers.clone();
        }
        if fired.contains(&AttachmentKind::McpInstructionsDelta) {
            baseline.mcp_instruction_digests = self.candidate.mcp_instruction_digests.clone();
        }
        baseline.model = self.candidate.model.clone();
    }
}

/// Compare this turn's world against what the model was last told.
pub(crate) fn diff(input: &WorldStateInput<'_>, previous: &WorldStateSnapshot) -> WorldStateDelta {
    let deferred_tools_current: BTreeSet<String> = input
        .deferred_tools
        .iter()
        .filter(|name| !is_retired_deferred_tool_name(name))
        .cloned()
        .collect();
    let agent_types_current: BTreeSet<String> = input.agent_types.iter().cloned().collect();
    let mcp_servers_current: BTreeMap<String, McpServerAnnouncementState> = input
        .mcp_servers
        .iter()
        .map(|server| {
            (
                server.name.clone(),
                McpServerAnnouncementState {
                    tool_count: server.tool_count,
                    description: server.description.clone(),
                },
            )
        })
        .collect();
    let mcp_digests_current: BTreeMap<String, ContentDigest> = input
        .mcp_instructions
        .iter()
        .map(|(name, text)| (name.clone(), ContentDigest::of(text)))
        .collect();

    // Explicit literal, no `..Default::default()`: `check-live-fields` requires
    // every snapshot field to be constructed, so a section added to the
    // snapshot but not wired here fails the build instead of staying `None`.
    let candidate = WorldStateSnapshot {
        model: Some(input.model.to_string()),
        deferred_tools: deferred_tools_current.clone(),
        agent_types: agent_types_current.clone(),
        mcp_servers: mcp_servers_current.clone(),
        mcp_instruction_digests: mcp_digests_current.clone(),
    };

    WorldStateDelta {
        deferred_tools: diff_deferred_tools(
            &deferred_tools_current,
            input.loaded_tools,
            &previous.deferred_tools,
        ),
        agent_listing: diff_agent_types(&agent_types_current, &previous.agent_types),
        mcp_servers: diff_mcp_servers(&mcp_servers_current, &previous.mcp_servers),
        mcp_instructions: diff_mcp_instructions(
            input.mcp_instructions,
            &mcp_digests_current,
            &previous.mcp_instruction_digests,
        ),
        model_switch: diff_model(input.model, previous.model.as_deref()),
        candidate,
    }
}

/// A tool the model can find via `ToolSearch` but cannot yet call directly.
///
/// `removed` is deliberately narrow: a name that left the *deferred* set but
/// is now loaded moved because the model discovered it, and its schema is in
/// the request — telling it the tool vanished would be false. Only names gone
/// from the registry entirely are reported.
fn diff_deferred_tools(
    current: &BTreeSet<String>,
    loaded: &[String],
    previous: &BTreeSet<String>,
) -> Option<DeferredToolsDeltaInfo> {
    let registry: HashSet<&str> = current
        .iter()
        .map(String::as_str)
        .chain(loaded.iter().map(String::as_str))
        .collect();

    let added_lines: Vec<String> = current
        .difference(previous)
        .map(|name| format!("- {name}"))
        .collect();
    let removed_names: Vec<String> = previous
        .iter()
        .filter(|name| !is_retired_deferred_tool_name(name))
        .filter(|name| !registry.contains(name.as_str()))
        .cloned()
        .collect();

    (!added_lines.is_empty() || !removed_names.is_empty()).then_some(DeferredToolsDeltaInfo {
        added_lines,
        removed_names,
    })
}

fn diff_agent_types(
    current: &BTreeSet<String>,
    previous: &BTreeSet<String>,
) -> Option<AgentListingDeltaInfo> {
    let added_lines: Vec<String> = current
        .difference(previous)
        .map(|name| format!("- {name}"))
        .collect();
    let removed_types: Vec<String> = previous.difference(current).cloned().collect();

    (!added_lines.is_empty() || !removed_types.is_empty()).then_some(AgentListingDeltaInfo {
        added_lines,
        removed_types,
        is_initial: previous.is_empty(),
        // Informational and always relevant: whenever new agent types appear,
        // remind the model that multi-agent dispatch should fan out in one
        // assistant message.
        show_concurrency_note: true,
    })
}

fn diff_mcp_servers(
    current: &BTreeMap<String, McpServerAnnouncementState>,
    previous: &BTreeMap<String, McpServerAnnouncementState>,
) -> Option<McpServersDeltaInfo> {
    if current == previous {
        return None;
    }
    let removed_names = previous
        .keys()
        .filter(|name| !current.contains_key(name.as_str()))
        .cloned()
        .collect();
    // The listing is rebuilt from the comparison map itself, so what is shown
    // can never disagree with what was recorded as told — and BTreeMap
    // iteration is already name-sorted, so the capped listing is stable.
    let mut servers: Vec<McpServerSummary> = current
        .iter()
        .map(|(name, state)| McpServerSummary {
            name: name.clone(),
            tool_count: state.tool_count,
            description: state.description.clone(),
        })
        .collect();
    let omitted = servers.len().saturating_sub(MAX_ANNOUNCED_MCP_SERVERS);
    servers.truncate(MAX_ANNOUNCED_MCP_SERVERS);
    Some(McpServersDeltaInfo {
        servers,
        removed_names,
        omitted,
    })
}

/// Instruction *bodies* are compared through their digests — the baseline is
/// persisted on every change and a handful of servers can carry several KB of
/// instructions each.
fn diff_mcp_instructions(
    instructions: &HashMap<String, String>,
    current: &BTreeMap<String, ContentDigest>,
    previous: &BTreeMap<String, ContentDigest>,
) -> Option<McpInstructionsDeltaInfo> {
    let added_blocks: Vec<String> = current
        .iter()
        .filter(|(name, digest)| previous.get(name.as_str()).is_none_or(|old| old != *digest))
        .filter_map(|(name, _)| {
            instructions
                .get(name)
                .map(|text| format!("## {name}\n\n{text}"))
        })
        .collect();
    let removed_names: Vec<String> = previous
        .keys()
        .filter(|name| !current.contains_key(name.as_str()))
        .cloned()
        .collect();

    // Output order is the BTreeMap walk: blocks are `## {name}`-prefixed and
    // names are the map keys, so the rendering is byte-stable without a sort.
    (!added_blocks.is_empty() || !removed_names.is_empty()).then_some(McpInstructionsDeltaInfo {
        added_blocks,
        removed_names,
    })
}

/// A first-ever turn has no previous model and reports nothing: the system
/// prompt is accurate at that point, and announcing a switch that never
/// happened would be its own falsehood.
fn diff_model(current: &str, previous: Option<&str>) -> Option<ModelSwitchInfo> {
    let previous = previous?;
    (previous != current).then(|| ModelSwitchInfo {
        previous: previous.to_string(),
        current: current.to_string(),
    })
}

/// Clear each section whose announcement did not survive compaction, so the
/// next turn re-announces exactly what was lost and nothing more.
///
/// This is the typed counterpart of scanning history for a fragment's rendered
/// text: `Message::Attachment` carries its `AttachmentKind`, so the question
/// "is this announcement still in history?" is answered by an enum comparison
/// rather than a string match — and per section rather than all-or-nothing.
pub(crate) fn retain_surviving(
    baseline: &mut WorldStateSnapshot,
    contains_kind: impl Fn(AttachmentKind) -> bool,
) {
    if !contains_kind(AttachmentKind::DeferredToolsDelta) {
        baseline.deferred_tools.clear();
    }
    if !contains_kind(AttachmentKind::AgentListingDelta) {
        baseline.agent_types.clear();
    }
    if !contains_kind(AttachmentKind::McpServersDelta) {
        baseline.mcp_servers.clear();
    }
    if !contains_kind(AttachmentKind::McpInstructionsDelta) {
        baseline.mcp_instruction_digests.clear();
    }
    // `model` is deliberately not cleared. It records which model the
    // conversation currently describes, not an announcement that has to
    // survive in history; re-announcing an unchanged model after every
    // compaction would be noise.
}

#[cfg(test)]
#[path = "world_state.test.rs"]
mod tests;
