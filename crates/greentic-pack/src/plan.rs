use std::collections::{BTreeSet, HashMap};

use greentic_types::TenantCtx;
use greentic_types::component::ComponentManifest;
use greentic_types::deployment::{
    ChannelPlan, DeploymentPlan, MessagingPlan, MessagingSubjectPlan, RunnerPlan, SecretPlan,
    TelemetryPlan,
};
use serde_json::{Map as JsonMap, Value as JsonValue, json};

use crate::builder::{FlowEntry, PackMeta};

/// Builds a provider-agnostic [`DeploymentPlan`] from the supplied pack metadata and component
/// manifests. The resulting plan is intentionally conservative and focuses on expressing the
/// minimal runtime contracts (flows, secrets, telemetry) so that deployers can extend it with
/// provider-specific data.
pub fn infer_base_deployment_plan(
    meta: &PackMeta,
    flows: &[FlowEntry],
    connectors: Option<&JsonValue>,
    components: &HashMap<String, ComponentManifest>,
    tenant: &TenantCtx,
    environment: &str,
) -> DeploymentPlan {
    let runners = vec![RunnerPlan {
        name: format!("{}-runner", meta.pack_id),
        replicas: 1,
        capabilities: json!({
            "flows": flows.iter().map(|flow| flow.id.clone()).collect::<Vec<_>>(),
        }),
    }];

    let messaging = infer_messaging_plan(connectors);
    let channels = infer_channel_plan(connectors);
    let secrets = infer_secret_plan(components);
    let telemetry = infer_telemetry_plan(components);

    DeploymentPlan {
        pack_id: meta.pack_id.clone(),
        pack_version: meta.version.clone(),
        tenant: tenant.tenant.as_str().to_string(),
        environment: environment.to_string(),
        runners,
        messaging,
        channels,
        secrets,
        oauth: Vec::new(),
        telemetry,
        extra: JsonValue::Object(JsonMap::new()),
    }
}

fn infer_messaging_plan(connectors: Option<&JsonValue>) -> Option<MessagingPlan> {
    let connectors = connectors?.as_object()?;
    let messaging_entry = connectors.get("messaging")?;
    let subjects = extract_subjects_from_connectors(messaging_entry);
    if subjects.is_empty() {
        None
    } else {
        Some(MessagingPlan {
            logical_cluster: "default".to_string(),
            subjects,
            extra: JsonValue::Object(JsonMap::new()),
        })
    }
}

fn extract_subjects_from_connectors(value: &JsonValue) -> Vec<MessagingSubjectPlan> {
    let mut subjects = Vec::new();
    if let Some(entries) = value.as_object() {
        for (key, entry) in entries {
            if entry.get("flow").is_some() {
                subjects.push(MessagingSubjectPlan {
                    name: key.clone(),
                    purpose: "messaging".to_string(),
                    durable: true,
                    extra: JsonValue::Object(JsonMap::new()),
                });
            } else if entry.is_object() {
                subjects.extend(extract_subjects_from_connectors(entry));
            }
        }
    }
    subjects
}

fn infer_channel_plan(connectors: Option<&JsonValue>) -> Vec<ChannelPlan> {
    let mut channels = Vec::new();
    if let Some(connectors) = connectors {
        collect_channels("", connectors, &mut channels);
    }
    channels
}

fn collect_channels(prefix: &str, value: &JsonValue, out: &mut Vec<ChannelPlan>) {
    match value {
        JsonValue::Object(map) => {
            if map.get("flow").and_then(|flow| flow.as_str()).is_some() {
                push_channel(prefix, map, out);
            } else {
                for (key, entry) in map {
                    let next_prefix = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    collect_channels(&next_prefix, entry, out);
                }
            }
        }
        JsonValue::Array(entries) => {
            for entry in entries {
                collect_channels(prefix, entry, out);
            }
        }
        _ => {}
    }
}

fn push_channel(prefix: &str, map: &JsonMap<String, JsonValue>, out: &mut Vec<ChannelPlan>) {
    if let Some(flow) = map.get("flow").and_then(|value| value.as_str()) {
        let name = map
            .get("name")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .unwrap_or_else(|| prefix.to_string());

        let mut config = map.clone();
        config.remove("flow");
        config.remove("name");

        out.push(ChannelPlan {
            name,
            flow_id: flow.to_string(),
            kind: prefix.to_string(),
            config: JsonValue::Object(config),
        });
    }
}

fn infer_secret_plan(components: &HashMap<String, ComponentManifest>) -> Vec<SecretPlan> {
    let mut seen = BTreeSet::new();
    let mut secrets = Vec::new();
    for component in components.values() {
        if let Some(secret_caps) = component.capabilities.host.secrets.as_ref() {
            for key in &secret_caps.required {
                if seen.insert(key.clone()) {
                    secrets.push(SecretPlan {
                        key: key.clone(),
                        required: true,
                        scope: "tenant".to_string(),
                    });
                }
            }
        }
    }
    secrets
}

fn infer_telemetry_plan(components: &HashMap<String, ComponentManifest>) -> Option<TelemetryPlan> {
    let requires_telemetry = components
        .values()
        .any(|component| component.capabilities.host.telemetry.is_some());

    if requires_telemetry {
        Some(TelemetryPlan {
            required: true,
            suggested_endpoint: None,
            extra: JsonValue::Object(JsonMap::new()),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use greentic_types::ResourceHints;
    use std::str::FromStr;

    use super::*;
    use crate::builder::PackMeta;
    use greentic_types::component::{
        ComponentCapabilities, ComponentProfiles, HostCapabilities, SecretsCapabilities,
        TelemetryCapabilities, WasiCapabilities,
    };
    use greentic_types::deployment::MessagingSubjectPlan;
    use greentic_types::{EnvId, TenantCtx, TenantId};
    use semver::Version;

    #[test]
    fn infers_plan_with_channels_and_secrets() {
        let mut meta = PackMeta {
            pack_version: crate::builder::PACK_VERSION,
            pack_id: "demo.pack".to_string(),
            version: Version::parse("1.2.3").unwrap(),
            name: "Demo".into(),
            kind: None,
            description: None,
            authors: Vec::new(),
            license: None,
            homepage: None,
            support: None,
            vendor: None,
            imports: Vec::new(),
            entry_flows: vec!["flow.main".into()],
            created_at_utc: "2025-01-01T00:00:00Z".into(),
            events: None,
            repo: None,
            messaging: None,
            interfaces: Vec::new(),
            annotations: JsonMap::new(),
            distribution: None,
            components: Vec::new(),
        };
        let connectors = json!({
            "messaging": {
                "teams": {
                    "primary": {
                        "flow": "flow.main",
                        "name": "teams-primary",
                        "team_id": "42"
                    }
                }
            }
        });
        meta.annotations
            .insert("connectors".into(), connectors.clone());

        let flows = vec![FlowEntry {
            id: "flow.main".into(),
            kind: "messaging".into(),
            entry: "start".into(),
            file_yaml: "flows/flow.main/flow.ygtc".into(),
            file_json: "flows/flow.main/flow.json".into(),
            hash_blake3: "abc".into(),
        }];

        let component_manifest = ComponentManifest {
            id: "component.a".parse().unwrap(),
            version: Version::parse("1.0.0").unwrap(),
            supports: vec![greentic_types::flow::FlowKind::Messaging],
            world: "greentic:test@1.0.0".into(),
            profiles: ComponentProfiles {
                default: Some("default".into()),
                supported: vec!["default".into()],
            },
            capabilities: ComponentCapabilities {
                wasi: WasiCapabilities::default(),
                host: HostCapabilities {
                    secrets: Some(SecretsCapabilities {
                        required: vec!["API_TOKEN".into()],
                    }),
                    telemetry: Some(TelemetryCapabilities {
                        scope: greentic_types::component::TelemetryScope::Tenant,
                    }),
                    ..Default::default()
                },
            },
            configurators: None,
            operations: Vec::new(),
            config_schema: None,
            resources: ResourceHints::default(),
        };

        let mut components = HashMap::new();
        components.insert(component_manifest.id.to_string(), component_manifest);

        let tenant = TenantCtx::new(
            EnvId::from_str("dev").unwrap(),
            TenantId::from_str("tenant-1").unwrap(),
        );

        let plan = infer_base_deployment_plan(
            &meta,
            &flows,
            meta.annotations.get("connectors"),
            &components,
            &tenant,
            "staging",
        );
        assert_eq!(plan.pack_id, "demo.pack");
        assert_eq!(plan.channels.len(), 1);
        assert_eq!(plan.secrets.len(), 1);
        assert!(plan.telemetry.is_some());
        if let Some(messaging) = plan.messaging.as_ref() {
            assert!(
                messaging
                    .subjects
                    .iter()
                    .any(|subject: &MessagingSubjectPlan| subject.name == "primary")
            );
        } else {
            panic!("messaging plan missing subjects");
        }
    }
}
