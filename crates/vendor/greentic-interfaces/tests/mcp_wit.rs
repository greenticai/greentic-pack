use std::path::PathBuf;

use wit_parser::Resolve;

#[test]
fn mcp_packages_are_staged_and_parse() {
    let staged = PathBuf::from(env!("WIT_STAGING_DIR"));
    for pkg in [
        "wasix-mcp-24.11.5",
        "wasix-mcp-25.3.26",
        "wasix-mcp-25.6.18",
    ] {
        let path = staged.join(pkg).join("package.wit");
        assert!(
            path.exists(),
            "staged WIT missing for package {pkg}: {}",
            path.display()
        );

        let mut resolve = Resolve::new();
        resolve
            .push_path(&path)
            .unwrap_or_else(|err| panic!("failed to parse {pkg} ({path:?}): {err}"));
    }
}

#[test]
fn mcp_tool_and_result_shapes_compile() {
    use greentic_interfaces::bindings::wasix_mcp_24_11_5_mcp_router::exports::wasix::mcp::router as mcp24;
    let _tool24 = mcp24::Tool {
        name: "echo".into(),
        description: "test tool".into(),
        input_schema: mcp24::Value { json: "{}".into() },
        output_schema: None,
        output: Some("text/plain".into()),
        config: Some(vec![mcp24::ConfigDescriptor {
            name: "endpoint".into(),
            description: "service endpoint".into(),
            required: true,
        }]),
        secrets: Some(vec![mcp24::SecretDescriptor {
            name: "token".into(),
            description: "auth token".into(),
            required: true,
        }]),
    };

    use greentic_interfaces::bindings::wasix_mcp_25_3_26_mcp_router::exports::wasix::mcp::router as mcp25_03;
    let _result25 = mcp25_03::CallToolResult {
        content: vec![mcp25_03::Content::Audio(mcp25_03::AudioContent {
            data: "base64audio".into(),
            mime_type: "audio/wav".into(),
            annotations: None,
        })],
        progress: Some(vec![mcp25_03::ProgressNotification {
            progress: Some(0.5),
            message: Some("halfway".into()),
            annotations: None,
        }]),
        meta: Some(vec![mcp25_03::MetaEntry {
            key: "output".into(),
            value: "text/plain".into(),
        }]),
        is_error: Some(false),
    };

    use greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router as mcp25_06;
    let content = vec![
        mcp25_06::ContentBlock::Audio(mcp25_06::AudioContent {
            data: "base64audio".into(),
            mime_type: "audio/wav".into(),
            annotations: None,
        }),
        mcp25_06::ContentBlock::ResourceLink(mcp25_06::ResourceLinkContent {
            uri: "https://example.com/resource".into(),
            title: Some("example".into()),
            description: Some("linked resource".into()),
            mime_type: Some("text/plain".into()),
            annotations: None,
        }),
        mcp25_06::ContentBlock::EmbeddedResource(mcp25_06::EmbeddedResource {
            uri: "memory://embedded".into(),
            title: Some("embedded".into()),
            description: Some("inline resource".into()),
            mime_type: Some("text/plain".into()),
            data: "hello".into(),
            annotations: None,
        }),
    ];
    let _result = mcp25_06::ToolResult {
        content,
        structured_content: Some("{\"ok\":true}".into()),
        progress: None,
        meta: None,
        is_error: Some(false),
    };
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct ToolPayload {
    name: String,
    title: Option<String>,
    description: String,
    input_schema: String,
    output_schema: Option<String>,
    meta: Option<Vec<MetaEntryPayload>>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct MetaEntryPayload {
    key: String,
    value: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct ToolResultPayload {
    content: Vec<ContentBlockPayload>,
    structured_content: Option<String>,
    meta: Option<Vec<MetaEntryPayload>>,
    is_error: Option<bool>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case", content = "value")]
enum ContentBlockPayload {
    ResourceLink(ResourceLinkPayload),
}

#[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
struct ResourceLinkPayload {
    uri: String,
    title: Option<String>,
    description: Option<String>,
    mime_type: Option<String>,
}

impl From<&greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::Tool>
    for ToolPayload
{
    fn from(tool: &greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::Tool) -> Self {
        Self {
            name: tool.name.clone(),
            title: tool.title.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            output_schema: tool.output_schema.clone(),
            meta: tool
                .meta
                .as_ref()
                .map(|entries| entries.iter().map(MetaEntryPayload::from).collect()),
        }
    }
}

impl From<ToolPayload>
    for greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::Tool
{
    fn from(tool: ToolPayload) -> Self {
        greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::Tool {
            name: tool.name,
            title: tool.title,
            description: tool.description,
            input_schema: tool.input_schema,
            output_schema: tool.output_schema,
            annotations: None,
            meta: tool
                .meta
                .map(|entries| entries.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<&greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::MetaEntry>
    for MetaEntryPayload
{
    fn from(
        entry: &greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::MetaEntry,
    ) -> Self {
        Self {
            key: entry.key.clone(),
            value: entry.value.clone(),
        }
    }
}

impl From<MetaEntryPayload>
    for greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::MetaEntry
{
    fn from(entry: MetaEntryPayload) -> Self {
        greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::MetaEntry {
            key: entry.key,
            value: entry.value,
        }
    }
}

impl From<&greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::ToolResult>
    for ToolResultPayload
{
    fn from(
        result: &greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::ToolResult,
    ) -> Self {
        let mut content = Vec::new();
        for block in &result.content {
            match block {
                greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::ContentBlock::ResourceLink(link) => {
                    content.push(ContentBlockPayload::ResourceLink(ResourceLinkPayload {
                        uri: link.uri.clone(),
                        title: link.title.clone(),
                        description: link.description.clone(),
                        mime_type: link.mime_type.clone(),
                    }));
                }
                other => panic!("unexpected content variant in test fixture: {other:?}"),
            }
        }

        Self {
            content,
            structured_content: result.structured_content.clone(),
            meta: result
                .meta
                .as_ref()
                .map(|entries| entries.iter().map(MetaEntryPayload::from).collect()),
            is_error: result.is_error,
        }
    }
}

impl From<ToolResultPayload>
    for greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::ToolResult
{
    fn from(result: ToolResultPayload) -> Self {
        let content = result
            .content
            .into_iter()
            .map(|block| match block {
                ContentBlockPayload::ResourceLink(link) => {
                    greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::ContentBlock::ResourceLink(
                        greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::ResourceLinkContent {
                            uri: link.uri,
                            title: link.title,
                            description: link.description,
                            mime_type: link.mime_type,
                            annotations: None,
                        },
                    )
                }
            })
            .collect();

        greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router::ToolResult {
            content,
            structured_content: result.structured_content,
            progress: None,
            meta: result
                .meta
                .map(|entries| entries.into_iter().map(Into::into).collect()),
            is_error: result.is_error,
        }
    }
}

#[test]
fn mcp_25_06_18_tool_and_result_roundtrip_json() {
    use greentic_interfaces::bindings::wasix_mcp_25_6_18_mcp_router::exports::wasix::mcp::router as mcp25_06;

    let tool = mcp25_06::Tool {
        name: "summarize".into(),
        title: Some("Summarize Text".into()),
        description: "Summarize structured payloads".into(),
        input_schema: "{\"type\":\"object\"}".into(),
        output_schema: Some(
            "{\"type\":\"object\",\"properties\":{\"summary\":{\"type\":\"string\"}}}".into(),
        ),
        annotations: None,
        meta: Some(vec![mcp25_06::MetaEntry {
            key: "output-schema".into(),
            value: "application/json".into(),
        }]),
    };

    let tool_payload: ToolPayload = (&tool).into();
    let tool_json = serde_json::to_string(&tool_payload).expect("serialize tool payload");
    let parsed_tool: ToolPayload =
        serde_json::from_str(&tool_json).expect("round-trip tool payload json");
    let tool_round: mcp25_06::Tool = parsed_tool.into();

    assert_eq!(tool_round.name, tool.name);
    assert_eq!(tool_round.output_schema, tool.output_schema);
    assert_eq!(
        tool_round
            .meta
            .as_ref()
            .and_then(|m| m.first())
            .map(|m| &m.key),
        tool.meta.as_ref().and_then(|m| m.first()).map(|m| &m.key)
    );

    let result = mcp25_06::ToolResult {
        content: vec![mcp25_06::ContentBlock::ResourceLink(
            mcp25_06::ResourceLinkContent {
                uri: "https://example.com/report.json".into(),
                title: Some("report".into()),
                description: Some("Structured summary output".into()),
                mime_type: Some("application/json".into()),
                annotations: None,
            },
        )],
        structured_content: Some("{\"summary\":\"done\"}".into()),
        progress: None,
        meta: Some(vec![mcp25_06::MetaEntry {
            key: "trace-id".into(),
            value: "\"abc123\"".into(),
        }]),
        is_error: Some(false),
    };

    let result_payload: ToolResultPayload = (&result).into();
    let result_json = serde_json::to_string(&result_payload).expect("serialize result payload");
    let parsed_result: ToolResultPayload =
        serde_json::from_str(&result_json).expect("round-trip result payload json");
    let result_round: mcp25_06::ToolResult = parsed_result.into();

    assert_eq!(result_round.structured_content, result.structured_content);
    assert_eq!(
        result_round
            .meta
            .as_ref()
            .and_then(|m| m.first())
            .map(|m| &m.key),
        result.meta.as_ref().and_then(|m| m.first()).map(|m| &m.key)
    );
    match result_round.content.first().expect("result content exists") {
        mcp25_06::ContentBlock::ResourceLink(link) => {
            assert_eq!(link.uri, "https://example.com/report.json");
            assert_eq!(link.mime_type.as_deref(), Some("application/json"));
        }
        other => panic!("expected resource-link content, got {other:?}"),
    }
}
