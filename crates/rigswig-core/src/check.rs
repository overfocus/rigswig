//! The contract every adapter's output must keep, beyond what the JSON Schema can say.

use std::collections::BTreeSet;

use crate::model::{Harness, LoadWhen, SCHEMA_VERSION};

/// Every way `h` breaks the contract, as readable sentences; empty when it keeps it.
pub fn contract_errors(h: &Harness) -> Vec<String> {
    let mut errs = Vec::new();

    if h.schema_version != SCHEMA_VERSION {
        errs.push(format!("schemaVersion is {:?}, expected {SCHEMA_VERSION:?}", h.schema_version));
    }
    if h.coverage.not_modelled.is_empty() {
        errs.push("coverage.notModelled is empty: every adapter must say what it does not model".into());
    }

    let mut layers = BTreeSet::new();
    let mut ranks = BTreeSet::new();
    for l in &h.layers {
        if !layers.insert(l.id.as_str()) {
            errs.push(format!("layer id {:?} appears twice", l.id));
        }
        if !ranks.insert(l.rank) {
            errs.push(format!("layer rank {} appears twice", l.rank));
        }
    }

    let mut items = BTreeSet::new();
    for i in &h.items {
        if !items.insert(i.id.as_str()) {
            errs.push(format!("item id {:?} appears twice", i.id));
        }
        let prefix = format!("{}:{}:{}:", h.tool.id, i.detail.kind(), i.origin.layer);
        if !i.id.starts_with(&prefix) || i.id.len() == prefix.len() {
            errs.push(format!("item id {:?} is not {prefix}<name>", i.id));
        }
    }

    let packages: BTreeSet<&str> = h.packages.iter().map(|p| p.id.as_str()).collect();
    for i in &h.items {
        if !layers.contains(i.origin.layer.as_str()) {
            errs.push(format!("item {:?} comes from unknown layer {:?}", i.id, i.origin.layer));
        }
        if let Some(p) = &i.origin.package {
            if !packages.contains(p.as_str()) {
                errs.push(format!("item {:?} names unknown package {p:?}", i.id));
            }
        }
        if let Some(s) = &i.shadowed_by {
            if !items.contains(s.as_str()) {
                errs.push(format!("item {:?} is shadowed by unknown item {s:?}", i.id));
            }
            if s == &i.id {
                errs.push(format!("item {:?} shadows itself", i.id));
            }
            if i.effective {
                errs.push(format!("item {:?} is shadowed but marked effective", i.id));
            }
        }
        if !i.effective && i.load.when != LoadWhen::Never {
            errs.push(format!("item {:?} is not effective, so its load.when must be never", i.id));
        }
    }

    for p in &h.packages {
        if let Some(d) = &p.decided_by {
            if !layers.contains(d.as_str()) {
                errs.push(format!("package {:?} decided by unknown layer {d:?}", p.id));
            }
        }
        for c in &p.contributes {
            if !items.contains(c.as_str()) {
                errs.push(format!("package {:?} contributes unknown item {c:?}", p.id));
            }
        }
    }

    for (name, d) in &h.settings {
        if let Some(s) = &d.source {
            if !layers.contains(s.as_str()) {
                errs.push(format!("setting {name:?} decided by unknown layer {s:?}"));
            }
            if !d.trail.iter().any(|t| &t.layer == s) {
                errs.push(format!("setting {name:?} is decided by {s:?}, which is not in its trail"));
            }
        }
        for t in &d.trail {
            if !layers.contains(t.layer.as_str()) {
                errs.push(format!("setting {name:?} trail names unknown layer {:?}", t.layer));
            }
        }
    }

    let t = &h.context.tokens;
    if !(t.low <= t.value && t.value <= t.high) {
        errs.push(format!("context tokens {} are outside their range {}..{}", t.value, t.low, t.high));
    }
    let startup: u64 =
        h.items.iter().filter(|i| i.effective && i.load.when == LoadWhen::Startup).map(|i| i.load.chars.value).sum();
    if h.context.chars.value != startup {
        errs.push(format!(
            "context.chars is {}, but the effective startup items add up to {startup}",
            h.context.chars.value
        ));
    }

    if let Some(s) = &h.tool.session {
        if s.evidence.is_empty() {
            errs.push("tool.session is set but says nothing: its evidence is empty".into());
        }
    }

    errs
}
