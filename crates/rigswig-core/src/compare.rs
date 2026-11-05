//! Comparing two harnesses: two repos with one tool, or two tools on one repo.
//!
//! Items are matched by kind and name (ids carry the tool and layer, which differ between the two sides). Only
//! effective items take part: what each session actually gets.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::{Harness, Item, LoadWhen};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub tool: String,
    pub repo: String,
    pub chars: u64,
    pub tokens: u64,
}

/// One item as the comparison shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub kind: String,
    pub name: String,
    pub layer: String,
    /// Characters at startup (0 for items that load later or never).
    pub chars: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changed {
    pub kind: String,
    pub name: String,
    pub a: Entry,
    pub b: Entry,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingDiff {
    pub name: String,
    pub a: serde_json::Value,
    pub b: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindCount {
    pub only_a: usize,
    pub only_b: usize,
    pub same: usize,
    pub changed: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub a: Side,
    pub b: Side,
    /// Per kind, in a stable order.
    pub kinds: BTreeMap<String, KindCount>,
    /// Biggest first.
    pub only_a: Vec<Entry>,
    pub only_b: Vec<Entry>,
    /// Same kind and name, different content (by hash) or a different startup size.
    pub changed: Vec<Changed>,
    pub settings: Vec<SettingDiff>,
}

fn entry(i: &Item) -> Entry {
    Entry {
        kind: i.detail.kind().into(),
        name: i.name.clone(),
        layer: i.origin.layer.clone(),
        chars: if i.load.when == LoadWhen::Startup { i.load.chars.value } else { 0 },
    }
}

fn side(h: &Harness) -> Side {
    Side {
        tool: h.tool.id.clone(),
        repo: h.target.repo.clone(),
        chars: h.context.chars.value,
        tokens: h.context.tokens.value,
    }
}

/// The effective items of `h`, keyed by (kind, name). If a name occurs twice in one kind (skills a tool lists twice),
/// the occurrences are numbered so each is compared.
fn keyed(h: &Harness) -> BTreeMap<(String, String), &Item> {
    let mut out = BTreeMap::new();
    for i in h.items.iter().filter(|i| i.effective) {
        let mut key = (i.detail.kind().to_string(), i.name.clone());
        let mut n = 2;
        while out.contains_key(&key) {
            key.1 = format!("{} ({n})", i.name);
            n += 1;
        }
        out.insert(key, i);
    }
    out
}

pub fn compare(a: &Harness, b: &Harness) -> Comparison {
    let ka = keyed(a);
    let kb = keyed(b);
    let mut kinds: BTreeMap<String, KindCount> = BTreeMap::new();
    let mut only_a = Vec::new();
    let mut only_b = Vec::new();
    let mut changed = Vec::new();

    for (key, ia) in &ka {
        let c = kinds.entry(key.0.clone()).or_default();
        match kb.get(key) {
            None => {
                c.only_a += 1;
                only_a.push(entry(ia));
            }
            Some(ib) => {
                let (ea, eb) = (entry(ia), entry(ib));
                // Different content, or the same file loaded to a different size (one tool cuts it).
                let differ = ea.chars != eb.chars
                    || matches!((&ia.load.content_hash, &ib.load.content_hash), (Some(x), Some(y)) if x != y);
                if differ {
                    c.changed += 1;
                    changed.push(Changed { kind: key.0.clone(), name: key.1.clone(), a: ea, b: eb });
                } else {
                    c.same += 1;
                }
            }
        }
    }
    for (key, ib) in &kb {
        if !ka.contains_key(key) {
            kinds.entry(key.0.clone()).or_default().only_b += 1;
            only_b.push(entry(ib));
        }
    }
    let by_size =
        |x: &Entry, y: &Entry| y.chars.cmp(&x.chars).then_with(|| (&x.kind, &x.name).cmp(&(&y.kind, &y.name)));
    only_a.sort_by(by_size);
    only_b.sort_by(by_size);
    changed.sort_by(|x, y| {
        let dx = x.a.chars.abs_diff(x.b.chars);
        let dy = y.a.chars.abs_diff(y.b.chars);
        dy.cmp(&dx).then_with(|| (&x.kind, &x.name).cmp(&(&y.kind, &y.name)))
    });

    let mut names: Vec<&String> = a.settings.keys().chain(b.settings.keys()).collect();
    names.sort();
    names.dedup();
    let settings = names
        .into_iter()
        .filter_map(|n| {
            let va = a.settings.get(n).map_or(serde_json::Value::Null, |d| d.value.clone());
            let vb = b.settings.get(n).map_or(serde_json::Value::Null, |d| d.value.clone());
            (va != vb).then(|| SettingDiff { name: n.clone(), a: va, b: vb })
        })
        .collect();

    Comparison { a: side(a), b: side(b), kinds, only_a, only_b, changed, settings }
}
