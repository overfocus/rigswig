#!/usr/bin/env bash
# Builds an invented setup for rigswig's screenshots: a home folder and a repo, "acme-shop", with Claude Code and
# Codex configured. Everything here is made up. Usage: docs/demo/make-demo.sh <empty folder>
# Then: HOME=<folder>/home USERPROFILE=<folder>/home rigswig ui --repo <folder>/acme-shop
set -euo pipefail
root="${1:?usage: make-demo.sh <empty folder>}"
mkdir -p "$root"
root="$(cd "$root" && pwd)"
home="$root/home"
repo="$root/acme-shop"
market="$home/marketplaces/acme-tools"
mkdir -p "$home/.claude/agents" "$home/.claude/skills" "$home/.claude/plugins" "$home/.codex" "$home/.agents/skills" \
  "$repo/.git" "$repo/.claude/agents" "$repo/.claude/rules" "$repo/.claude/commands" "$market/.claude-plugin"

w() { mkdir -p "$(dirname "$1")"; cat > "$1"; }

# ---- User level --------------------------------------------------------------------------------------------------
w "$home/.claude/CLAUDE.md" <<'EOF'
# How I work
Prefer small commits with a clear message. Run the tests before saying something works. Ask before deleting data.
EOF

agent() { # name, description, tools ("" for every tool)
  local tools=""; [ -n "$3" ] && tools="tools: $3"$'\n'
  local more=" Use it when a change touches this area, before a release, or when a reviewer asks for a second opinion; it reports what it checked, what it found, and what it could not check."
  printf -- "---\nname: %s\ndescription: %s\n%s---\nYou are the %s.\n" "$1" "$2$more" "$tools" "$1" > "$home/.claude/agents/$1.md"
}
agent code-reviewer "Reviews a diff for correctness bugs, missing tests and risky changes before merge." "Read, Grep, Glob"
agent test-writer "Writes focused unit and integration tests for new or changed code, matching the project's style." ""
agent security-auditor "Audits code and configuration for injection, secrets, unsafe defaults and dependency risk." "Read, Grep"
agent db-migrator "Plans and writes zero-downtime database migrations with a rollback for each step." ""
agent docs-writer "Writes and updates READMEs, API references and how-to guides from the code." ""
agent release-manager "Prepares release notes, version bumps and changelog entries from merged pull requests." "Read, Bash"
agent api-designer "Designs REST and GraphQL APIs: resources, errors, pagination, versioning and deprecation." ""
agent perf-profiler "Finds hot paths and slow queries from profiles and traces, and proposes measured fixes." ""
agent incident-responder "Leads an incident: timeline, impact, mitigation, and a blameless postmortem draft." ""
agent frontend-engineer "Builds accessible, responsive UI components with tests and stories." ""
agent data-analyst "Answers product questions from the warehouse with SQL and a short, sourced write-up." ""
agent infra-engineer "Writes and reviews infrastructure as code, CI pipelines and deployment configuration." ""
agent dependency-updater "Upgrades dependencies in small, tested steps and summarises breaking changes." ""
agent accessibility-checker "Checks pages against WCAG 2.2 AA and lists fixes in priority order." "Read, Grep"
agent copy-editor "Edits product copy for clarity, tone and consistency with the style guide." ""
agent pricing-analyst "Models pricing changes and their effect on revenue and churn from historical data." ""
agent support-triager "Triages support tickets: severity, owner, reproduction steps and a draft reply." ""
agent onboarding-guide "Explains an unfamiliar codebase: structure, key paths, and where to start." ""
agent schema-reviewer "Reviews JSON Schema and OpenAPI changes for compatibility with existing clients." ""
agent mobile-engineer "Builds and debugs the iOS and Android apps, including store submission checks." ""
agent observability-engineer "Adds metrics, logs and traces that answer real operational questions." ""
agent product-manager "Turns a vague request into a problem statement, success measures and a scoped plan." ""
agent ux-researcher "Plans usability tests, writes interview guides and synthesises findings." ""
agent legal-reviewer "Flags licence, privacy and terms issues in code and copy for a human to decide." "Read"

skill() { # dir, name, description
  w "$1/$2/SKILL.md" <<EOF
---
name: $2
description: $3
---
Steps for $2.
EOF
}
skill "$home/.claude/skills" commit-message "Write a commit message from the staged diff: what changed and why, in the project's format."
skill "$home/.claude/skills" pr-summary "Summarise a pull request for reviewers: the change, the risk, how it was tested."
skill "$home/.claude/skills" standup "Draft a standup update from yesterday's commits and open pull requests."

w "$home/.claude/settings.json" <<'EOF'
{
  "model": "opus",
  "enabledPlugins": {
    "deploy-kit@acme-tools": true,
    "design-system@acme-tools": true,
    "observability@acme-tools": true
  },
  "permissions": {
    "allow": ["Read", "Grep", "Bash(npm test:*)", "Bash(git status)"],
    "deny": ["Bash(rm -rf:*)", "Read(./.env)"]
  },
  "hooks": {
    "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "~/.claude/hooks/guard-shell.sh" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "~/.claude/hooks/notify.sh" }] }]
  }
}
EOF

w "$home/.claude.json" <<'EOF'
{
  "mcpServers": {
    "issue-tracker": { "type": "http", "url": "https://tracker.example.com/mcp" }
  }
}
EOF

# ---- A local marketplace with three plugins ------------------------------------------------------------------------
w "$home/.claude/plugins/known_marketplaces.json" <<EOF
{ "acme-tools": { "source": { "source": "directory", "path": "$market" } } }
EOF
w "$market/.claude-plugin/marketplace.json" <<'EOF'
{ "name": "acme-tools", "plugins": [
  { "name": "deploy-kit", "source": "./deploy-kit" },
  { "name": "design-system", "source": "./design-system" },
  { "name": "observability", "source": "./observability" }
] }
EOF
for s in preview-deploy rollback canary-release feature-flags env-check smoke-test; do
  skill "$market/deploy-kit/skills" "$s" "Deploy step: $s for the web app, with the checks it needs before and after, and how to undo it if something looks wrong."
done
printf -- "---\ndescription: Watches a deploy and reports errors and latency against the last release.\ntools: Read, Bash\n---\nWatch.\n" | w "$market/deploy-kit/agents/deploy-watcher.md"
w "$market/deploy-kit/hooks/hooks.json" <<'EOF'
{ "hooks": { "SessionStart": [{ "hooks": [{ "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/hooks/check-env.sh" }] }] } }
EOF
for s in tokens components layout motion; do
  skill "$market/design-system/skills" "$s" "Design system: use the $s guidelines and components, with examples of correct and incorrect use."
done
w "$market/observability/.mcp.json" <<'EOF'
{ "mcpServers": { "metrics": { "type": "http", "url": "https://metrics.example.com/mcp" } } }
EOF

# ---- The repo -------------------------------------------------------------------------------------------------------
w "$repo/CLAUDE.md" <<'EOF'
# acme-shop
The storefront and its admin. Read @AGENTS.md first; it is shared with other coding tools.
EOF
{
  echo "# acme-shop: guidelines for coding agents"
  for area in architecture checkout catalog search payments accounts admin api database caching queues email \
    testing deployment; do
    echo
    echo "## $area"
    for n in $(seq 1 13); do
      echo "- The $area rule $n: keep changes small and reviewed, cover them with tests, name things after the business terms in the glossary, and log enough to debug a failure in production without a reproduction."
    done
  done
} > "$repo/AGENTS.md"
printf -- "---\nname: code-reviewer\ndescription: Reviews acme-shop pull requests against the checkout and payments checklists.\ntools: Read, Grep, Glob\n---\nReview.\n" > "$repo/.claude/agents/code-reviewer.md"
printf -- "---\npaths:\n  - src/api/**\n---\nAPI handlers validate input with the shared schemas and return problem+json errors.\n" > "$repo/.claude/rules/api.md"
printf -- "---\ndescription: Summarise yesterday's merged pull requests for the team channel.\n---\nSummarise.\n" > "$repo/.claude/commands/standup.md"
w "$repo/.claude/settings.json" <<'EOF'
{
  "permissions": { "allow": ["Bash(npm run lint)", "Bash(npm run build)"], "ask": ["Bash(git push:*)"] },
  "enabledMcpjsonServers": ["docs-search"]
}
EOF
w "$repo/.mcp.json" <<'EOF'
{ "mcpServers": {
  "docs-search": { "command": "npx", "args": ["-y", "docs-search-mcp"] },
  "staging-db": { "command": "staging-db-mcp" }
} }
EOF

# ---- Codex on the same repo -----------------------------------------------------------------------------------------
w "$home/.codex/config.toml" <<EOF
model = "gpt-6"
approval_policy = "on-request"
sandbox_mode = "workspace-write"

[mcp_servers.issue-tracker]
url = "https://tracker.example.com/mcp"

[projects."$repo"]
trust_level = "trusted"
EOF
skill "$home/.agents/skills" commit-message "Write a commit message from the staged diff: what changed and why."

echo "demo setup in $root (repo: $repo)"
