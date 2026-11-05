//! The harness-neutral data model: what a session of one tool, in one repo, gets.
//!
//! Every tool has the same kinds of thing (instruction files, agents, skills, commands, hooks, MCP servers,
//! permissions, settings such as the model), arriving through stacked layers (managed, org, user, project, local,
//! package, session), with a precedence rule deciding what wins. So the model is **layers + items + a decision trail**,
//! plus an honest account of what was not modelled or measured.
//!
//! The JSON form is camelCase and versioned by [`SCHEMA_VERSION`], separately from the crate version. Before 1.0.0
//! any minor version may break; from 1.0.0, minor versions only add.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The version of the JSON contract (`schema/harness.schema.json`), not of the crate.
pub const SCHEMA_VERSION: &str = "0.1.0";

/// A layer's id, unique within one [`Harness`] (e.g. `user`, `project`, `plugin:formatter@example-market`).
pub type LayerId = String;
/// An item's id: stable across runs, `<tool>:<kind>:<layer>:<name>`, so diffs and history can key on it.
pub type ItemId = String;

/// Everything one tool loads for a session in one repo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Harness {
    /// [`SCHEMA_VERSION`] at the time of writing.
    pub schema_version: String,
    pub tool: Tool,
    pub target: Target,
    /// Every layer the adapter knows of for this tool, present or not, highest precedence first.
    pub layers: Vec<Layer>,
    /// Plugins, extensions or packs that contribute items.
    #[serde(default)]
    pub packages: Vec<Package>,
    pub items: Vec<Item>,
    /// Resolved scalar settings (`model`, `approvalMode`, ...), each with the trail that decided it.
    #[serde(default)]
    pub settings: BTreeMap<String, Decision>,
    pub context: ContextEstimate,
    pub coverage: Coverage,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

/// Which tool this is, and which rules were applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tool {
    /// A short id: `claude-code`, `codex`, `gemini-cli`, ...
    pub id: String,
    /// The adapter's own version: which reading of the tool's rules produced this.
    pub adapter_version: String,
    /// The tool version whose documented behaviour the adapter models, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_version_assumed: Option<String>,
    /// Why the adapter thinks this tool is in use here (files found), as short labels.
    #[serde(default)]
    pub detected_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    /// The repo, as a label (by default its folder name); full paths are opt-in.
    pub repo: String,
    pub os: Os,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Macos,
    Linux,
    Windows,
    Other,
}

/// One source of configuration in the precedence stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layer {
    pub id: LayerId,
    pub kind: LayerKind,
    /// Human label, e.g. "project settings".
    pub label: String,
    /// Where it lives, as shown to the user (`~/...` or repo-relative); absent for layers with no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Whether anything was found there.
    pub present: bool,
    /// Precedence: lower wins. Unique within a harness.
    pub rank: u32,
    /// `enforce` layers cannot be overridden by lower ones; `default` layers can.
    pub mode: LayerMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    Managed,
    Org,
    User,
    Project,
    Local,
    Package,
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LayerMode {
    Enforce,
    Default,
}

/// A plugin, extension or pack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Package {
    pub id: String,
    pub kind: PackageKind,
    /// Where it came from, as a label (a marketplace name, `owner/repo`, a folder name); never a URL with a path.
    pub source: String,
    pub enabled: bool,
    /// The layer whose setting decided `enabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<LayerId>,
    /// What each layer that mentions it says (on or off).
    #[serde(default)]
    pub scopes: BTreeMap<LayerId, bool>,
    /// The items it contributes.
    #[serde(default)]
    pub contributes: Vec<ItemId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Plugin,
    Extension,
    Pack,
}

/// One thing the session may load: an instruction file, an agent, a skill, a command, a hook, an MCP server or a
/// permission rule.
// No `deny_unknown_fields` here: serde does not support it alongside `flatten`; the contract check covers the shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: ItemId,
    pub name: String,
    pub origin: Origin,
    /// False when the item is overridden (`shadowedBy`), disabled, or otherwise not loaded.
    pub effective: bool,
    /// The item that wins over this one (same kind and name at a higher-precedence layer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<ItemId>,
    pub load: Load,
    /// `kind` and its kind-specific `attrs`.
    #[serde(flatten)]
    pub detail: ItemDetail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Origin {
    pub layer: LayerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// The file, as shown to the user (`~/...` or repo-relative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// When an item reaches the model, and how big it is when it does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Load {
    pub when: LoadWhen,
    /// Characters that load at `when` (for a skill at startup: its name and description, not its body).
    pub chars: Measure,
    /// SHA-256 of the item's content, hex; lets two harnesses or two runs be compared without sending content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LoadWhen {
    /// Every session, before the first message.
    Startup,
    /// When a file matching the item's globs is touched.
    OnMatch,
    /// When the model or the user invokes it.
    OnInvoke,
    /// Never: nothing of it enters the model's context. Either it is not loaded (disabled, shadowed, blocked), or it
    /// works outside the context (a hook, a permission rule).
    Never,
}

/// A number and how much to trust it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Measure {
    pub value: u64,
    pub confidence: Confidence,
}

/// How a figure was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Reported by the tool itself.
    Native,
    /// Computed exactly from files (e.g. a character count).
    Calculated,
    /// Approximated (e.g. tokens from characters).
    Estimated,
    /// Could not be determined.
    Unknown,
}

/// The kind of an item and the attributes only that kind has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "attrs", rename_all = "lowercase")]
pub enum ItemDetail {
    Instruction(InstructionAttrs),
    Agent(AgentAttrs),
    Skill(SkillAttrs),
    Command(CommandAttrs),
    Hook(HookAttrs),
    Mcp(McpAttrs),
    Permission(PermissionAttrs),
}

impl ItemDetail {
    /// The kind as it appears in JSON and in item ids.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Instruction(_) => "instruction",
            Self::Agent(_) => "agent",
            Self::Skill(_) => "skill",
            Self::Command(_) => "command",
            Self::Hook(_) => "hook",
            Self::Mcp(_) => "mcp",
            Self::Permission(_) => "permission",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstructionAttrs {
    /// The instruction item that imports this one (`@file` imports, nested AGENTS.md).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_by: Option<ItemId>,
    /// Globs it is scoped to; empty means it applies everywhere.
    #[serde(default)]
    pub applies_to: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentAttrs {
    pub tools: ToolAccess,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Which tools an agent may use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ToolAccess {
    /// Every tool the session has (the agent names none).
    All,
    /// Only these.
    Only(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillAttrs {
    /// Characters of the body, which load only when the skill is invoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_chars: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandAttrs {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookAttrs {
    /// A tool-neutral event name (`before-tool`, `after-tool`, `session-start`, `prompt-submit`, `stop`, ...).
    pub event: String,
    /// The tool's own event name (`PreToolUse`, ...).
    pub native_event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,
    pub handler: HookHandler,
    /// The script's file name only, never the command line or inline code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HookHandler {
    Command,
    Http,
    Prompt,
    Agent,
    #[serde(rename = "mcp-tool")]
    McpTool,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpAttrs {
    pub transport: McpTransport,
    /// For a URL, the scheme and host only; for a command, its file name only. Never args, env, headers or paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub state: McpState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Http,
    Sse,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum McpState {
    Active,
    /// Declared by the repo but waiting for the user's approval.
    Pending,
    Disabled,
    /// Refused by policy.
    Denied,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionAttrs {
    pub effect: PermissionEffect,
    /// The tool the rule is about (`Bash`, `shell`, `mcp__server__tool`, ...).
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionEffect {
    Allow,
    Deny,
    Ask,
}

/// A resolved setting and how it was decided.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Decision {
    /// The value in effect; `null` when nothing sets it and the tool's default applies.
    pub value: serde_json::Value,
    /// The layer that decided it; absent when it is the tool's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<LayerId>,
    /// Every layer that had a say, highest precedence first, including the ones that lost.
    #[serde(default)]
    pub trail: Vec<TrailStep>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrailStep {
    pub layer: LayerId,
    pub value: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What loads before the first message, and how much of it could be measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextEstimate {
    /// Characters of every effective startup item. The primary unit: exact, and the same for every model.
    pub chars: Measure,
    /// Tokens derived from `chars`; always an estimate, with a range.
    pub tokens: TokenEstimate,
    /// Fixed costs the tool adds that no file shows (system prompt, built-in tool schemas, MCP tool lists).
    #[serde(default)]
    pub not_measured: Vec<NotMeasured>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenEstimate {
    /// How tokens were derived, e.g. `chars/4`.
    pub method: String,
    pub value: u64,
    pub low: u64,
    pub high: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotMeasured {
    pub what: String,
    pub why: String,
}

/// What the adapter knows it does not model. Required and never empty: no reading of a tool is complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Coverage {
    pub not_modelled: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Warning {
    /// A stable code, e.g. `stale-cache`, `agent-all-tools`.
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// Tokens from characters, as a labelled estimate: `chars / 4`, with a range of 3 to 5 characters per token.
/// Prose in Latin scripts sits near 4 for current tokenizers; markdown with code nearer 3, so `high` uses 3.
pub fn estimate_tokens(chars: u64) -> TokenEstimate {
    TokenEstimate {
        method: "chars/4".into(),
        value: chars.div_ceil(4),
        low: chars.div_ceil(5),
        high: chars.div_ceil(3),
    }
}
