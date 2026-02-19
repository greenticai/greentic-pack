#![cfg(feature = "bindings-rust")]

use std::convert::TryFrom;

use greentic_interfaces::bindings;
use greentic_types as types;
use semver::Version;
use time::OffsetDateTime;

fn fixture_id<T>(value: &str) -> T
where
    T: TryFrom<String, Error = types::GreenticError>,
{
    T::try_from(value.to_owned())
        .unwrap_or_else(|err| panic!("invalid fixture identifier '{value}': {err}"))
}

fn sample_tenant_ctx() -> types::TenantCtx {
    types::TenantCtx {
        env: fixture_id("dev"),
        tenant: fixture_id("tenant"),
        tenant_id: fixture_id("tenant"),
        team: Some(fixture_id("team")),
        team_id: Some(fixture_id("team")),
        user: Some(fixture_id("user")),
        user_id: Some(fixture_id("user")),
        attributes: Default::default(),
        session_id: Some("sess-1".into()),
        flow_id: Some("flow-1".into()),
        node_id: Some("node-1".into()),
        provider_id: Some("provider-1".into()),
        trace_id: Some("trace".into()),
        i18n_id: Some("en-US".into()),
        correlation_id: Some("corr".into()),
        deadline: Some(types::InvocationDeadline::from_unix_millis(
            1_700_000_000_000,
        )),
        attempt: 1,
        idempotency_key: Some("idem".into()),
        impersonation: Some(types::Impersonation {
            actor_id: fixture_id("actor"),
            reason: Some("maintenance".into()),
        }),
    }
}

#[test]
fn tenant_ctx_roundtrip_external() {
    let ctx = sample_tenant_ctx();
    let wit = match bindings::greentic::interfaces_types::types::TenantCtx::try_from(ctx.clone()) {
        Ok(value) => value,
        Err(err) => panic!("wit conversion failed: {err}"),
    };
    let round = match types::TenantCtx::try_from(wit) {
        Ok(value) => value,
        Err(err) => panic!("rust conversion failed: {err}"),
    };
    assert_eq!(round.env.as_str(), ctx.env.as_str());
    assert_eq!(round.attempt, ctx.attempt);
    assert_eq!(
        round
            .impersonation
            .as_ref()
            .map(|imp| imp.actor_id.as_str()),
        ctx.impersonation.as_ref().map(|imp| imp.actor_id.as_str())
    );
    assert_eq!(round.session_id, ctx.session_id);
    assert_eq!(round.flow_id, ctx.flow_id);
    assert_eq!(round.node_id, ctx.node_id);
    assert_eq!(round.provider_id, ctx.provider_id);
}

#[test]
fn tenant_ctx_old_style_is_accepted() {
    let mut ctx = types::TenantCtx::new(fixture_id("dev"), fixture_id("tenant"));
    ctx.team = None;
    ctx.user = None;
    ctx.deadline = None;
    let wit = bindings::greentic::interfaces_types::types::TenantCtx::try_from(ctx.clone())
        .expect("old style to wit");
    let round = types::TenantCtx::try_from(wit).expect("old style from wit");
    assert!(round.session_id.is_none());
    assert!(round.flow_id.is_none());
    assert!(round.node_id.is_none());
    assert!(round.provider_id.is_none());
}

#[test]
fn outcome_roundtrip_external() {
    use bindings::greentic::interfaces_types::types::Outcome as WitOutcome;

    let outcome = types::Outcome::Pending {
        reason: "waiting".into(),
        expected_input: Some(vec!["input".into()]),
    };
    let wit: WitOutcome = outcome.clone().into();
    let round: types::Outcome<String> = wit.into();
    match round {
        types::Outcome::Pending {
            reason,
            expected_input,
        } => {
            assert_eq!(reason, "waiting");
            assert_eq!(
                expected_input.unwrap_or_default(),
                vec!["input".to_string()]
            );
        }
        _ => panic!("expected pending"),
    }
}

#[test]
fn span_context_roundtrip() {
    use bindings::greentic::interfaces_types::types::SpanContext as WitSpanContext;

    let span = types::SpanContext {
        tenant: fixture_id("tenant"),
        session_id: Some(types::SessionKey::from("session")),
        flow_id: "flow".into(),
        node_id: Some("node".into()),
        provider: "provider".into(),
        start: Some(OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()),
        end: None,
    };
    let wit = WitSpanContext::try_from(span.clone()).expect("to wit");
    let round = types::SpanContext::try_from(wit).expect("from wit");
    assert_eq!(round.provider, span.provider);
    assert_eq!(round.node_id, span.node_id);
}

#[test]
fn allow_list_roundtrip() {
    use bindings::greentic::interfaces_types::types::AllowList as WitAllowList;

    let list = types::AllowList {
        domains: vec!["example.com".into()],
        ports: vec![443],
        protocols: vec![
            types::Protocol::Https,
            types::Protocol::Custom("mqtt".into()),
        ],
    };
    let wit: WitAllowList = list.clone().into();
    let round: types::AllowList = wit.into();
    assert_eq!(round.domains, list.domains);
    assert_eq!(round.ports, list.ports);
    assert_eq!(round.protocols.len(), list.protocols.len());
}

#[test]
fn pack_ref_roundtrip() {
    use bindings::greentic::interfaces_types::types::PackRef as WitPackRef;

    let pack = types::PackRef {
        oci_url: "registry.example.com/pack".into(),
        version: Version::parse("1.2.3").expect("valid version"),
        digest: "sha256:deadbeef".into(),
        signatures: vec![types::Signature {
            key_id: "key1".into(),
            algorithm: types::SignatureAlgorithm::Ed25519,
            signature: vec![1, 2, 3],
        }],
    };
    let wit = WitPackRef::from(pack.clone());
    let round = types::PackRef::try_from(wit).expect("from wit");
    assert_eq!(round.oci_url, pack.oci_url);
    assert_eq!(round.version, pack.version);
    assert_eq!(round.digest, pack.digest);
    assert_eq!(round.signatures.len(), pack.signatures.len());
}
