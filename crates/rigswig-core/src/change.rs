//! Write mode, the planning half: a change, and what applying it would do. The core only plans (it has no way to
//! write); the binary applies a plan, and only when started with `--allow-writes`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A change a person asked for. Small on purpose: each kind is one key in one settings file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Change {
    /// Turn a plugin on or off at a scope (`enabledPlugins["<name>@<marketplace>"]`).
    Plugin { id: String, scope: Scope, on: bool },
    /// Approve or disable a project `.mcp.json` server at a scope (`enabledMcpjsonServers` / `disabledMcpjsonServers`).
    McpJson { name: String, scope: Scope, on: bool },
}

/// Where a change is written. Managed settings are never written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// This repo, this person (not committed).
    Local,
    /// This repo, everyone (committed with the repo).
    Project,
    /// Every repo, this person.
    User,
}

/// One line of a diff: `op` is ` ` (kept), `-` (removed) or `+` (added).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub op: char,
    pub text: String,
}

/// What applying a change would do.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub change: Change,
    /// The file, as shown to people.
    pub file: String,
    /// The file to write. Never sent to the page.
    #[serde(skip)]
    pub path: PathBuf,
    /// SHA-256 of the file now (absent when the file does not exist yet). Applying refuses if it changed since.
    pub before_hash: Option<String>,
    /// The new contents. Never sent to the page (the diff is).
    #[serde(skip)]
    pub after: String,
    pub creates_file: bool,
    pub diff: Vec<DiffLine>,
    pub tokens_before: u64,
    pub tokens_after: u64,
    /// Whether the change would actually take effect once applied (a higher layer may still decide otherwise).
    pub takes_effect: bool,
    /// Plain-language notes: when it applies, who else it affects, and why it might not take effect.
    pub notes: Vec<String>,
}

/// A line diff of two texts (longest common subsequence). Large inputs fall back to "all removed, all added".
pub fn diff_lines(before: &str, after: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let line = |op: char, t: &str| DiffLine { op, text: t.to_string() };
    if a.len() * b.len() > 4_000_000 {
        return a.iter().map(|t| line('-', t)).chain(b.iter().map(|t| line('+', t))).collect();
    }
    // lcs[i][j]: the LCS length of a[i..] and b[j..].
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push(line(' ', a[i]));
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            // On a tie, the removed line comes first, as diffs are read.
            out.push(line('-', a[i]));
            i += 1;
        } else {
            out.push(line('+', b[j]));
            j += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diffs_keep_common_lines() {
        let d = diff_lines("{\n  \"a\": 1\n}\n", "{\n  \"a\": 1,\n  \"b\": 2\n}\n");
        let ops: String = d.iter().map(|l| l.op).collect();
        assert_eq!(ops, " -++ ");
        assert_eq!(diff_lines("", "x\n").iter().map(|l| l.op).collect::<String>(), "+");
    }

    #[test]
    fn changes_parse_strictly() {
        let c: Change = serde_json::from_str(r#"{"kind":"plugin","id":"a@b","scope":"local","on":false}"#).unwrap();
        assert_eq!(c, Change::Plugin { id: "a@b".into(), scope: Scope::Local, on: false });
        assert!(
            serde_json::from_str::<Change>(r#"{"kind":"plugin","id":"a@b","scope":"managed","on":false}"#).is_err()
        );
        assert!(
            serde_json::from_str::<Change>(r#"{"kind":"plugin","id":"a@b","scope":"local","on":false,"x":1}"#).is_err()
        );
        assert!(serde_json::from_str::<Change>(r#"{"kind":"shell","cmd":"rm"}"#).is_err());
    }
}
