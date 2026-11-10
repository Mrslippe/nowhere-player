//! 终端输出模型的序列化、兼容性与夹具一致性测试。

use super::*;

const DEMO_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../apps/web/src/shared/fixtures/terminal-demo.json"
));

fn sample_block() -> Block {
    Block {
        id: "blk_demo".to_string(),
        input: "scan ~/Music".to_string(),
        started_at: 1_762_512_000_000,
        duration_ms: Some(1_842),
        status: BlockStatus::Done,
        outputs: vec![
            Output::Text {
                spans: vec![
                    Span {
                        text: "扫描完成".to_string(),
                        bold: true,
                        dim: false,
                        italic: false,
                        color: Some(SpanColor::Accent),
                    },
                    Span {
                        text: " · 1,284 首".to_string(),
                        bold: false,
                        dim: true,
                        italic: false,
                        color: Some(SpanColor::Muted),
                    },
                ],
                tone: None,
            },
            Output::Table {
                columns: vec![
                    "#".to_string(),
                    "曲名".to_string(),
                    "艺人".to_string(),
                    "时长".to_string(),
                ],
                rows: vec![
                    vec![
                        "1".to_string(),
                        "晴天".to_string(),
                        "周杰伦".to_string(),
                        "4:29".to_string(),
                    ],
                    vec![
                        "2".to_string(),
                        "稻香".to_string(),
                        "周杰伦".to_string(),
                        "3:43".to_string(),
                    ],
                ],
                selectable: true,
            },
            Output::Progress {
                label: "扫描 ~/Music".to_string(),
                current: 1_284,
                total: 1_284,
                unit: "首".to_string(),
            },
            Output::Error {
                code: "E_LIB_PERMISSION".to_string(),
                message: "无法读取目录 ~/Music/Private".to_string(),
                hint: Some("检查文件权限后重试".to_string()),
                actions: Some(vec!["重试".to_string(), "跳过该目录".to_string()]),
            },
            Output::Hint {
                text: "did you mean 'play'?".to_string(),
                suggestion: Some("按 Tab 采纳".to_string()),
            },
            Output::Media {
                kind: MediaKind::Cover,
                r#ref: "np://library/album/1/cover.jpg".to_string(),
                size: 48,
            },
        ],
        footer: Some("6 项输出 · 输入 play 1 开始播放".to_string()),
        schema_version: SCHEMA_VERSION,
        actions: None,
    }
}

#[test]
fn block_serialization_roundtrips() {
    let block = sample_block();
    let json = serde_json::to_string(&block).expect("serialize block");

    // 契约要求 camelCase 字段名（ARCH-005 §2.1 / §5）。
    assert!(
        json.contains(r#""schemaVersion":1"#),
        "schemaVersion missing: {json}"
    );
    assert!(
        json.contains(r#""startedAt":1762512000000"#),
        "startedAt missing: {json}"
    );
    assert!(
        json.contains(r#""durationMs":1842"#),
        "durationMs missing: {json}"
    );
    assert!(
        json.contains(r#""type":"text""#),
        "type tag missing: {json}"
    );
    assert!(
        json.contains(r#""ref":"np://"#),
        "ref field missing: {json}"
    );

    let parsed: Block = serde_json::from_str(&json).expect("deserialize block");
    assert_eq!(parsed, block);
}

#[test]
fn unknown_fields_and_variants_degrade() {
    let json = r#"{
        "id": "blk_unknown",
        "input": "search 晴天",
        "startedAt": 1,
        "durationMs": null,
        "status": "paused",
        "outputs": [{
            "type": "text",
            "tone": "purple",
            "spans": [{
                "text": "hi",
                "bold": true,
                "dim": false,
                "italic": false,
                "color": "neon",
                "unknown_span_field": 1
            }],
            "unknown_output_field": true
        }],
        "footer": null,
        "schemaVersion": 1,
        "unknown_block_field": {"a": 1}
    }"#;

    let block: Block = serde_json::from_str(json).expect("unknown fields/variants must not fail");
    assert_eq!(block.status, BlockStatus::Pending);
    assert_eq!(block.outputs.len(), 1);
    match &block.outputs[0] {
        Output::Text { spans, tone } => {
            assert_eq!(*tone, None);
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].text, "hi");
            assert!(spans[0].bold);
            assert_eq!(spans[0].color, None);
        }
        other => panic!("expected text output, got {}", other.kind()),
    }

    let media: Output = serde_json::from_str(
        r#"{"type":"media","kind":"poster","ref":"np://library/album/1/cover.jpg","size":48}"#,
    )
    .expect("unknown media kind must not fail");
    assert!(matches!(
        media,
        Output::Media {
            kind: MediaKind::Cover,
            ..
        }
    ));
}

#[test]
fn unknown_output_type_degrades_to_unsupported_text() {
    let output: Output = serde_json::from_str(r#"{"type":"chart","series":[1,2,3]}"#)
        .expect("unknown output type must not fail");

    assert_eq!(output.kind(), "text");
    match output {
        Output::Text { spans, tone } => {
            assert_eq!(tone, None);
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].text, UNSUPPORTED_OUTPUT_TEXT);
            assert!(!spans[0].bold && !spans[0].dim && !spans[0].italic);
            assert_eq!(spans[0].color, None);
        }
        other => panic!("expected degraded text output, got {}", other.kind()),
    }
}

#[test]
fn demo_fixture_deserializes_and_covers_six_outputs() {
    let block: Block =
        serde_json::from_str(DEMO_FIXTURE).expect("terminal-demo.json must deserialize as Block");

    assert_eq!(block.schema_version, SCHEMA_VERSION);
    let kinds: Vec<&str> = block.outputs.iter().map(Output::kind).collect();
    assert_eq!(
        kinds,
        ["text", "table", "progress", "error", "hint", "media"]
    );

    match &block.outputs[1] {
        Output::Table {
            columns,
            rows,
            selectable,
        } => {
            assert_eq!(columns.len(), 4);
            assert_eq!(rows.len(), 5);
            assert!(rows.iter().all(|row| row.len() == columns.len()));
            assert!(*selectable);
        }
        other => panic!("expected table output, got {}", other.kind()),
    }

    match &block.outputs[3] {
        Output::Error { code, hint, .. } => {
            assert_eq!(code, "E_LIB_PERMISSION");
            assert!(hint.is_some());
        }
        other => panic!("expected error output, got {}", other.kind()),
    }

    match &block.outputs[5] {
        Output::Media { kind, r#ref, size } => {
            assert_eq!(*kind, MediaKind::Cover);
            assert!(r#ref.starts_with("np://"));
            assert_eq!(*size, 48);
        }
        other => panic!("expected media output, got {}", other.kind()),
    }
}

#[test]
fn demo_fixture_roundtrips_to_identical_wire_json() {
    let expected: serde_json::Value =
        serde_json::from_str(DEMO_FIXTURE).expect("terminal-demo.json must be valid JSON");
    let block: Block =
        serde_json::from_str(DEMO_FIXTURE).expect("terminal-demo.json must deserialize as Block");
    let actual: serde_json::Value =
        serde_json::to_value(&block).expect("block must serialize to JSON");

    // 字段名、枚举标签、null 形状必须与夹具（也就是 TS 镜像）逐字段一致。
    assert_eq!(actual, expected);
}

#[test]
fn block_actions_are_optional_and_roundtrip() {
    let mut block = sample_block();
    let old_json = serde_json::to_string(&block).expect("serialize old block");
    let old_value: serde_json::Value = serde_json::from_str(&old_json).expect("old block JSON");
    assert!(
        old_value.get("actions").is_none(),
        "None actions must be omitted at block level: {old_json}"
    );

    let old: Block = serde_json::from_str(&old_json).expect("old block deserialize");
    assert_eq!(old.actions, None);

    block.actions = Some(vec![
        BlockAction {
            label: "重试".to_string(),
            input: "search 晴天 --source netease".to_string(),
        },
        BlockAction {
            label: "只用本地搜索".to_string(),
            input: "search 晴天 --local".to_string(),
        },
    ]);
    let json = serde_json::to_string(&block).expect("serialize new block");
    assert!(json.contains(r#""actions":[{"label":"重试","input":"search 晴天 --source netease"}"#));
    let parsed: Block = serde_json::from_str(&json).expect("new block deserialize");
    assert_eq!(parsed, block);
}
