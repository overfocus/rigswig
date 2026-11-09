//! The data model's contract: an invented harness passes, every broken copy fails, the JSON keeps its shape, and the
//! checked-in schema and example match the code.

use std::collections::BTreeMap;

use rigswig_core::check::contract_errors;
use rigswig_core::model::*;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn layer(id: &str, kind: LayerKind, rank: u32) -> Layer {
    Layer {
        id: id.into(),
        kind,
        label: format!("{id} settings"),
        path: None,
        present: true,
        rank,
        mode: LayerMode::Default,
    }
}

fn item(id: &str, name: &str, layer: &str, chars: u64, detail: ItemDetail) -> Item {
    Item {
        id: id.into(),
        name: name.into(),
        origin: Origin { layer: layer.into(), package: None, file: None, line: None },
        effective: true,
        shadowed_by: None,
        load: Load {
            when: LoadWhen::Startup,
            chars: Measure { value: chars, confidence: Confidence::Calculated },
            content_hash: None,
        },
        detail,
    }
}

/// An invented harness that uses every kind of item, a shadowed agent, a package and a decided setting.
fn example() -> Harness {
    let mut shadowed = item(
        "example-tool:agent:user:reviewer",
        "reviewer",
        "user",
        90,
        ItemDetail::Agent(AgentAttrs { tools: ToolAccess::All, model: None }),
    );
    shadowed.effective = false;
    shadowed.shadowed_by = Some("example-tool:agent:project:reviewer".into());
    shadowed.load.when = LoadWhen::Never;

    let mut from_pkg = item(
        "example-tool:skill:plugin:docs@example-market:write-docs",
        "write-docs",
        "plugin:docs@example-market",
        120,
        ItemDetail::Skill(SkillAttrs { body_chars: Some(2400) }),
    );
    from_pkg.origin.package = Some("docs@example-market".into());

    let items = vec![
        item(
            "example-tool:instruction:project:AGENTS.md",
            "AGENTS.md",
            "project",
            4000,
            ItemDetail::Instruction(InstructionAttrs::default()),
        ),
        item(
            "example-tool:agent:project:reviewer",
            "reviewer",
            "project",
            100,
            ItemDetail::Agent(AgentAttrs {
                tools: ToolAccess::Only(vec!["Read".into(), "Grep".into()]),
                model: Some("small".into()),
            }),
        ),
        shadowed,
        from_pkg,
        item("example-tool:command:user:standup", "standup", "user", 60, ItemDetail::Command(CommandAttrs {})),
        item(
            "example-tool:hook:project:before-tool-0",
            "before-tool-0",
            "project",
            0,
            ItemDetail::Hook(HookAttrs {
                event: "before-tool".into(),
                native_event: "PreToolUse".into(),
                matcher: Some("Bash".into()),
                handler: HookHandler::Command,
                script: Some("guard.sh".into()),
            }),
        ),
        item(
            "example-tool:mcp:project:tickets",
            "tickets",
            "project",
            0,
            ItemDetail::Mcp(McpAttrs {
                transport: McpTransport::Http,
                target: Some("https://tickets.example.com".into()),
                state: McpState::Pending,
            }),
        ),
        item(
            "example-tool:permission:managed:deny-curl",
            "deny-curl",
            "managed",
            0,
            ItemDetail::Permission(PermissionAttrs {
                effect: PermissionEffect::Deny,
                tool: "Bash".into(),
                pattern: Some("curl:*".into()),
            }),
        ),
    ];
    let startup: u64 =
        items.iter().filter(|i| i.effective && i.load.when == LoadWhen::Startup).map(|i| i.load.chars.value).sum();

    let mut managed = layer("managed", LayerKind::Managed, 0);
    managed.mode = LayerMode::Enforce;
    let mut settings = BTreeMap::new();
    settings.insert(
        "model".into(),
        Decision {
            value: "large".into(),
            source: Some("project".into()),
            trail: vec![
                TrailStep { layer: "project".into(), value: "large".into(), note: None },
                TrailStep { layer: "user".into(), value: "small".into(), note: Some("overridden".into()) },
            ],
        },
    );

    Harness {
        schema_version: SCHEMA_VERSION.into(),
        tool: Tool {
            id: "example-tool".into(),
            adapter_version: "0.0.0".into(),
            tool_version_assumed: None,
            detected_by: vec!["AGENTS.md".into()],
            session: None,
        },
        target: Target { repo: "example-repo".into(), os: Os::Linux },
        layers: vec![
            managed,
            layer("project", LayerKind::Project, 1),
            layer("user", LayerKind::User, 2),
            layer("plugin:docs@example-market", LayerKind::Package, 3),
        ],
        packages: vec![Package {
            id: "docs@example-market".into(),
            kind: PackageKind::Plugin,
            source: "example-market".into(),
            enabled: true,
            decided_by: Some("user".into()),
            scopes: BTreeMap::from([("user".into(), true)]),
            contributes: vec!["example-tool:skill:plugin:docs@example-market:write-docs".into()],
        }],
        items,
        settings,
        context: ContextEstimate {
            chars: Measure { value: startup, confidence: Confidence::Calculated },
            tokens: estimate_tokens(startup),
            not_measured: vec![NotMeasured { what: "system prompt".into(), why: "not in any file".into() }],
        },
        coverage: Coverage { not_modelled: vec!["command-line flags".into()] },
        warnings: vec![Warning {
            code: "agent-all-tools".into(),
            message: "an agent has every tool".into(),
            item: None,
            file: None,
        }],
    }
}

#[test]
fn the_example_keeps_the_contract() {
    assert_eq!(contract_errors(&example()), Vec::<String>::new());
}

#[test]
fn json_round_trips_and_has_the_documented_shape() {
    let h = example();
    let json = serde_json::to_value(&h).unwrap();
    assert_eq!(serde_json::from_value::<Harness>(json.clone()).unwrap(), h);

    assert_eq!(json["schemaVersion"], SCHEMA_VERSION);
    let agent = &json["items"][1];
    assert_eq!(agent["kind"], "agent");
    assert_eq!(agent["attrs"]["tools"]["only"][0], "Read");
    assert_eq!(json["items"][2]["shadowedBy"], "example-tool:agent:project:reviewer");
    assert_eq!(json["items"][2]["load"]["when"], "never");
    assert_eq!(json["items"][0]["attrs"]["appliesTo"], serde_json::json!([]));
    assert_eq!(json["context"]["chars"]["confidence"], "calculated");
}

#[test]
fn unknown_fields_are_refused() {
    let mut json = serde_json::to_value(example()).unwrap();
    json["surprise"] = true.into();
    assert!(serde_json::from_value::<Harness>(json).is_err());
}

/// Each broken copy must fail the contract with a message that names the problem.
#[test]
fn broken_copies_fail() {
    type Break = fn(&mut Harness);
    let cases: &[(&str, Break)] = &[
        ("schemaVersion", |h| h.schema_version = "9.9.9".into()),
        ("notModelled is empty", |h| h.coverage.not_modelled.clear()),
        ("layer id \"user\" appears twice", |h| h.layers[3].id = "user".into()),
        ("rank 1 appears twice", |h| h.layers[2].rank = 1),
        ("appears twice", |h| h.items[1].id = h.items[0].id.clone()),
        ("is not example-tool:agent:project:<name>", |h| h.items[1].id = "example-tool:skill:project:reviewer".into()),
        ("unknown layer", |h| h.items[4].origin.layer = "nowhere".into()),
        ("unknown package", |h| h.items[3].origin.package = Some("ghost".into())),
        ("shadowed by unknown item", |h| h.items[2].shadowed_by = Some("example-tool:agent:project:ghost".into())),
        ("shadowed but marked effective", |h| h.items[2].effective = true),
        ("load.when must be never", |h| h.items[2].load.when = LoadWhen::Startup),
        ("decided by unknown layer", |h| h.packages[0].decided_by = Some("ghost".into())),
        ("contributes unknown item", |h| h.packages[0].contributes.push("example-tool:skill:user:ghost".into())),
        ("not in its trail", |h| h.settings.get_mut("model").unwrap().source = Some("managed".into())),
        ("outside their range", |h| h.context.tokens.value = h.context.tokens.high + 1),
        ("add up to", |h| h.context.chars.value += 1),
        ("tool.session is set but says nothing", |h| {
            h.tool.session = Some(Session { scope: SessionScope::Machine, evidence: Vec::new() })
        }),
    ];
    for (expect, breaks) in cases {
        let mut h = example();
        breaks(&mut h);
        let errs = contract_errors(&h);
        assert!(errs.iter().any(|e| e.contains(expect)), "a copy broken for {expect:?} passed or said: {errs:?}");
    }
}

#[test]
fn token_estimate_is_a_range_around_chars_over_four() {
    let t = estimate_tokens(1000);
    assert_eq!((t.low, t.value, t.high), (200, 250, 334));
    assert_eq!(t.method, "chars/4");
    let z = estimate_tokens(0);
    assert_eq!((z.low, z.value, z.high), (0, 0, 0));
}

/// `schema/harness.schema.json` and `schema/example.json` are generated; regenerate with
/// `RIGSWIG_BLESS=1 cargo test -p rigswig-core`.
#[test]
fn checked_in_schema_and_example_match_the_code() {
    let dir = std::path::Path::new(ROOT).join("../../schema");
    let files = [
        ("harness.schema.json", serde_json::to_string_pretty(&rigswig_core::json_schema()).unwrap() + "\n"),
        ("example.json", serde_json::to_string_pretty(&example()).unwrap() + "\n"),
    ];
    for (name, want) in files {
        let path = dir.join(name);
        if std::env::var_os("RIGSWIG_BLESS").is_some() {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(have == want, "schema/{name} is out of date: run RIGSWIG_BLESS=1 cargo test -p rigswig-core");
    }
}
