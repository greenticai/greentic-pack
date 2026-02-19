//! Conversion helpers between generated WIT bindings and `greentic-types`.

use std::collections::BTreeMap;
use std::convert::{TryFrom, TryInto};

use greentic_types as types;
use semver::Version;
use time::OffsetDateTime;

use crate::bindings;

type MapperResult<T> = Result<T, types::GreenticError>;

fn invalid_input(msg: impl Into<String>) -> types::GreenticError {
    types::GreenticError::new(types::ErrorCode::InvalidInput, msg)
}

fn i128_to_i64(value: i128) -> MapperResult<i64> {
    value
        .try_into()
        .map_err(|_| invalid_input("numeric overflow converting deadline"))
}

fn timestamp_ms_to_offset(ms: i64) -> MapperResult<OffsetDateTime> {
    let nanos = (ms as i128)
        .checked_mul(1_000_000)
        .ok_or_else(|| invalid_input("timestamp overflow"))?;
    OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .map_err(|err| invalid_input(format!("invalid timestamp: {err}")))
}

fn offset_to_timestamp_ms(dt: &OffsetDateTime) -> MapperResult<i64> {
    let nanos = dt.unix_timestamp_nanos();
    let ms = nanos
        .checked_div(1_000_000)
        .ok_or_else(|| invalid_input("timestamp division overflow"))?;
    ms.try_into()
        .map_err(|_| invalid_input("timestamp overflow converting to milliseconds"))
}

type WitTenantCtx = bindings::greentic::interfaces_types::types::TenantCtx;
type WitImpersonation = bindings::greentic::interfaces_types::types::Impersonation;
type WitSessionCursor = bindings::greentic::interfaces_types::types::SessionCursor;
type WitOutcome = bindings::greentic::interfaces_types::types::Outcome;
type WitOutcomePending = bindings::greentic::interfaces_types::types::OutcomePending;
type WitOutcomeError = bindings::greentic::interfaces_types::types::OutcomeError;
type WitErrorCode = bindings::greentic::interfaces_types::types::ErrorCode;
type WitAllowList = bindings::greentic::interfaces_types::types::AllowList;
type WitProtocol = bindings::greentic::interfaces_types::types::Protocol;
type WitNetworkPolicy = bindings::greentic::interfaces_types::types::NetworkPolicy;
type WitSpanContext = bindings::greentic::interfaces_types::types::SpanContext;
type WitPackRef = bindings::greentic::interfaces_types::types::PackRef;
type WitSignature = bindings::greentic::interfaces_types::types::Signature;
type WitSignatureAlgorithm = bindings::greentic::interfaces_types::types::SignatureAlgorithm;
type WitCommonFlowKind =
    bindings::greentic_common_types_0_1_0_common::exports::greentic::common_types::types::FlowKind;
type WitCommonTenantCtx =
    bindings::greentic_common_types_0_1_0_common::exports::greentic::common_types::types::TenantCtx;
type WitOutcomeStatus = bindings::greentic_common_types_0_1_0_common::exports::greentic::common_types::types::OutcomeStatus;
type WitComponentOutcome = bindings::greentic_common_types_0_1_0_common::exports::greentic::common_types::types::ComponentOutcome;
type WitPackKind = bindings::greentic_pack_export_v1_0_1_0_pack_host::exports::greentic::pack_export_v1::pack_api::PackKind;
type WitPackDescriptor =
    bindings::greentic_pack_export_v1_0_1_0_pack_host::exports::greentic::pack_export_v1::pack_api::PackDescriptor;
type WitFlowDescriptor =
    bindings::greentic_pack_export_v1_0_1_0_pack_host::exports::greentic::pack_export_v1::pack_api::FlowDescriptor;
type WitPackFlowKind =
    bindings::greentic_pack_export_v1_0_1_0_pack_host::greentic::common_types::types::FlowKind;

/// Normalized component outcome mirroring `greentic:common-types/component-outcome`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentOutcomeStatus {
    /// Component finished successfully.
    Done,
    /// Component needs more input.
    Pending,
    /// Component failed.
    Error,
}

/// Component outcome payload used by the v1 ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentOutcome {
    /// Status reported by the component.
    pub status: ComponentOutcomeStatus,
    /// Optional routing code.
    pub code: Option<String>,
    /// JSON payload returned by the component.
    pub payload: String,
    /// Optional metadata JSON blob.
    pub metadata: Option<String>,
}

/// Minimal pack descriptor mirroring the v1 pack-export ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackDescriptor {
    /// Logical pack identifier.
    pub pack_id: types::PackId,
    /// Pack version.
    pub version: Version,
    /// Pack kind classification.
    pub kind: types::PackKind,
    /// Declared publisher.
    pub publisher: String,
}

/// Minimal flow descriptor mirroring the v1 pack-export ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowDescriptor {
    /// Flow identifier.
    pub id: types::FlowId,
    /// Flow kind classification.
    pub kind: types::FlowKind,
    /// Flow tags.
    pub tags: Vec<String>,
    /// Flow entrypoints.
    pub entrypoints: Vec<String>,
}

/// Convert a WIT `TenantCtx` into the shared `greentic_types::TenantCtx`.
pub fn tenant_ctx_from_wit(ctx: WitTenantCtx) -> MapperResult<types::TenantCtx> {
    types::TenantCtx::try_from(ctx)
}

/// Convert a shared `greentic_types::TenantCtx` into the WIT `TenantCtx`.
pub fn tenant_ctx_to_wit(ctx: types::TenantCtx) -> MapperResult<WitTenantCtx> {
    WitTenantCtx::try_from(ctx)
}

impl TryFrom<WitImpersonation> for types::Impersonation {
    type Error = types::GreenticError;

    fn try_from(value: WitImpersonation) -> MapperResult<Self> {
        Ok(Self {
            actor_id: value.actor_id.try_into()?,
            reason: value.reason,
        })
    }
}

impl From<types::Impersonation> for WitImpersonation {
    fn from(value: types::Impersonation) -> Self {
        Self {
            actor_id: value.actor_id.into(),
            reason: value.reason,
        }
    }
}

impl TryFrom<WitTenantCtx> for types::TenantCtx {
    type Error = types::GreenticError;

    fn try_from(value: WitTenantCtx) -> MapperResult<Self> {
        let WitTenantCtx {
            env,
            tenant,
            tenant_id,
            team,
            team_id,
            user,
            user_id,
            trace_id,
            i18n_id,
            correlation_id,
            session_id,
            flow_id,
            node_id,
            provider_id,
            deadline_ms,
            attempt,
            idempotency_key,
            impersonation,
            attributes,
        } = value;

        let deadline =
            deadline_ms.map(|ms| types::InvocationDeadline::from_unix_millis(ms as i128));

        let env = env.try_into()?;
        let tenant = tenant.try_into()?;
        let tenant_id = tenant_id.try_into()?;
        let team = team.map(|item| item.try_into()).transpose()?;
        let team_id = team_id.map(|item| item.try_into()).transpose()?;
        let user = user.map(|item| item.try_into()).transpose()?;
        let user_id = user_id.map(|item| item.try_into()).transpose()?;
        let impersonation = impersonation
            .map(types::Impersonation::try_from)
            .transpose()?;
        let attributes: BTreeMap<String, String> = attributes.into_iter().collect();

        Ok(Self {
            env,
            tenant,
            tenant_id,
            team,
            team_id,
            user,
            user_id,
            session_id,
            flow_id,
            node_id,
            provider_id,
            trace_id,
            i18n_id,
            correlation_id,
            attributes,
            deadline,
            attempt,
            idempotency_key,
            impersonation,
        })
    }
}

impl TryFrom<types::TenantCtx> for WitTenantCtx {
    type Error = types::GreenticError;

    fn try_from(value: types::TenantCtx) -> MapperResult<Self> {
        let deadline_ms = match value.deadline {
            Some(deadline) => Some(i128_to_i64(deadline.unix_millis())?),
            None => None,
        };
        let attributes: Vec<(String, String)> = value.attributes.into_iter().collect();

        Ok(Self {
            env: value.env.into(),
            tenant: value.tenant.into(),
            tenant_id: value.tenant_id.into(),
            team: value.team.map(Into::into),
            team_id: value.team_id.map(Into::into),
            user: value.user.map(Into::into),
            user_id: value.user_id.map(Into::into),
            session_id: value.session_id,
            flow_id: value.flow_id,
            node_id: value.node_id,
            provider_id: value.provider_id,
            trace_id: value.trace_id,
            i18n_id: value.i18n_id.clone(),
            correlation_id: value.correlation_id,
            attributes,
            deadline_ms,
            attempt: value.attempt,
            idempotency_key: value.idempotency_key,
            impersonation: value.impersonation.map(Into::into),
        })
    }
}

impl From<WitSessionCursor> for types::SessionCursor {
    fn from(value: WitSessionCursor) -> Self {
        Self {
            node_pointer: value.node_pointer,
            wait_reason: value.wait_reason,
            outbox_marker: value.outbox_marker,
        }
    }
}

impl From<types::SessionCursor> for WitSessionCursor {
    fn from(value: types::SessionCursor) -> Self {
        Self {
            node_pointer: value.node_pointer,
            wait_reason: value.wait_reason,
            outbox_marker: value.outbox_marker,
        }
    }
}

impl From<WitErrorCode> for types::ErrorCode {
    fn from(value: WitErrorCode) -> Self {
        match value {
            WitErrorCode::Unknown => Self::Unknown,
            WitErrorCode::InvalidInput => Self::InvalidInput,
            WitErrorCode::NotFound => Self::NotFound,
            WitErrorCode::Conflict => Self::Conflict,
            WitErrorCode::Timeout => Self::Timeout,
            WitErrorCode::Unauthenticated => Self::Unauthenticated,
            WitErrorCode::PermissionDenied => Self::PermissionDenied,
            WitErrorCode::RateLimited => Self::RateLimited,
            WitErrorCode::Unavailable => Self::Unavailable,
            WitErrorCode::Internal => Self::Internal,
        }
    }
}

impl From<types::ErrorCode> for WitErrorCode {
    fn from(value: types::ErrorCode) -> Self {
        match value {
            types::ErrorCode::Unknown => Self::Unknown,
            types::ErrorCode::InvalidInput => Self::InvalidInput,
            types::ErrorCode::NotFound => Self::NotFound,
            types::ErrorCode::Conflict => Self::Conflict,
            types::ErrorCode::Timeout => Self::Timeout,
            types::ErrorCode::Unauthenticated => Self::Unauthenticated,
            types::ErrorCode::PermissionDenied => Self::PermissionDenied,
            types::ErrorCode::RateLimited => Self::RateLimited,
            types::ErrorCode::Unavailable => Self::Unavailable,
            types::ErrorCode::Internal => Self::Internal,
        }
    }
}

impl From<WitOutcome> for types::Outcome<String> {
    fn from(value: WitOutcome) -> Self {
        match value {
            WitOutcome::Done(val) => Self::Done(val),
            WitOutcome::Pending(payload) => Self::Pending {
                reason: payload.reason,
                expected_input: payload.expected_input,
            },
            WitOutcome::Error(payload) => Self::Error {
                code: payload.code.into(),
                message: payload.message,
            },
        }
    }
}

impl From<types::Outcome<String>> for WitOutcome {
    fn from(value: types::Outcome<String>) -> Self {
        match value {
            types::Outcome::Done(val) => Self::Done(val),
            types::Outcome::Pending {
                reason,
                expected_input,
            } => Self::Pending(WitOutcomePending {
                reason,
                expected_input,
            }),
            types::Outcome::Error { code, message } => Self::Error(WitOutcomeError {
                code: code.into(),
                message,
            }),
        }
    }
}

impl From<WitProtocol> for types::Protocol {
    fn from(value: WitProtocol) -> Self {
        match value {
            WitProtocol::Http => Self::Http,
            WitProtocol::Https => Self::Https,
            WitProtocol::Tcp => Self::Tcp,
            WitProtocol::Udp => Self::Udp,
            WitProtocol::Grpc => Self::Grpc,
            WitProtocol::Custom(v) => Self::Custom(v),
        }
    }
}

impl From<types::Protocol> for WitProtocol {
    fn from(value: types::Protocol) -> Self {
        match value {
            types::Protocol::Http => Self::Http,
            types::Protocol::Https => Self::Https,
            types::Protocol::Tcp => Self::Tcp,
            types::Protocol::Udp => Self::Udp,
            types::Protocol::Grpc => Self::Grpc,
            types::Protocol::Custom(v) => Self::Custom(v),
        }
    }
}

impl From<WitAllowList> for types::AllowList {
    fn from(value: WitAllowList) -> Self {
        Self {
            domains: value.domains,
            ports: value.ports,
            protocols: value.protocols.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<types::AllowList> for WitAllowList {
    fn from(value: types::AllowList) -> Self {
        Self {
            domains: value.domains,
            ports: value.ports,
            protocols: value.protocols.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<WitNetworkPolicy> for types::NetworkPolicy {
    fn from(value: WitNetworkPolicy) -> Self {
        Self {
            egress: value.egress.into(),
            deny_on_miss: value.deny_on_miss,
        }
    }
}

impl From<types::NetworkPolicy> for WitNetworkPolicy {
    fn from(value: types::NetworkPolicy) -> Self {
        Self {
            egress: value.egress.into(),
            deny_on_miss: value.deny_on_miss,
        }
    }
}

impl TryFrom<WitSpanContext> for types::SpanContext {
    type Error = types::GreenticError;

    fn try_from(value: WitSpanContext) -> MapperResult<Self> {
        let WitSpanContext {
            tenant,
            session_id,
            flow_id,
            node_id,
            provider,
            start_ms,
            end_ms,
        } = value;

        let start = start_ms.map(timestamp_ms_to_offset).transpose()?;
        let end = end_ms.map(timestamp_ms_to_offset).transpose()?;
        let tenant = tenant.try_into()?;

        Ok(Self {
            tenant,
            session_id: session_id.map(types::SessionKey::from),
            flow_id,
            node_id,
            provider,
            start,
            end,
        })
    }
}

impl TryFrom<types::SpanContext> for WitSpanContext {
    type Error = types::GreenticError;

    fn try_from(value: types::SpanContext) -> MapperResult<Self> {
        let start_ms = value
            .start
            .as_ref()
            .map(offset_to_timestamp_ms)
            .transpose()?;
        let end_ms = value.end.as_ref().map(offset_to_timestamp_ms).transpose()?;

        Ok(Self {
            tenant: value.tenant.into(),
            session_id: value.session_id.map(|key| key.to_string()),
            flow_id: value.flow_id,
            node_id: value.node_id,
            provider: value.provider,
            start_ms,
            end_ms,
        })
    }
}

impl From<WitSignatureAlgorithm> for types::SignatureAlgorithm {
    fn from(value: WitSignatureAlgorithm) -> Self {
        match value {
            WitSignatureAlgorithm::Ed25519 => Self::Ed25519,
            WitSignatureAlgorithm::Other(v) => Self::Other(v),
        }
    }
}

impl From<types::SignatureAlgorithm> for WitSignatureAlgorithm {
    fn from(value: types::SignatureAlgorithm) -> Self {
        match value {
            types::SignatureAlgorithm::Ed25519 => Self::Ed25519,
            types::SignatureAlgorithm::Other(v) => Self::Other(v),
        }
    }
}

impl From<WitSignature> for types::Signature {
    fn from(value: WitSignature) -> Self {
        Self {
            key_id: value.key_id,
            algorithm: value.algorithm.into(),
            signature: value.signature,
        }
    }
}

impl From<types::Signature> for WitSignature {
    fn from(value: types::Signature) -> Self {
        Self {
            key_id: value.key_id,
            algorithm: value.algorithm.into(),
            signature: value.signature,
        }
    }
}

impl TryFrom<WitPackRef> for types::PackRef {
    type Error = types::GreenticError;

    fn try_from(value: WitPackRef) -> MapperResult<Self> {
        let version = Version::parse(&value.version)
            .map_err(|err| invalid_input(format!("invalid version: {err}")))?;
        Ok(Self {
            oci_url: value.oci_url,
            version,
            digest: value.digest,
            signatures: value.signatures.into_iter().map(Into::into).collect(),
        })
    }
}

impl From<types::PackRef> for WitPackRef {
    fn from(value: types::PackRef) -> Self {
        Self {
            oci_url: value.oci_url,
            version: value.version.to_string(),
            digest: value.digest,
            signatures: value.signatures.into_iter().map(Into::into).collect(),
        }
    }
}

/// Convert the shared `FlowKind` into the WIT `flow-kind`.
pub fn flow_kind_to_wit(kind: types::FlowKind) -> WitCommonFlowKind {
    match kind {
        types::FlowKind::Messaging => WitCommonFlowKind::Messaging,
        types::FlowKind::Event => WitCommonFlowKind::Event,
        types::FlowKind::ComponentConfig => WitCommonFlowKind::ComponentConfig,
        types::FlowKind::Job => WitCommonFlowKind::Job,
        types::FlowKind::Http => WitCommonFlowKind::Http,
    }
}

/// Convert a WIT `flow-kind` into the shared `FlowKind`.
pub fn flow_kind_from_wit(kind: WitCommonFlowKind) -> types::FlowKind {
    match kind {
        WitCommonFlowKind::Messaging => types::FlowKind::Messaging,
        WitCommonFlowKind::Event => types::FlowKind::Event,
        WitCommonFlowKind::ComponentConfig => types::FlowKind::ComponentConfig,
        WitCommonFlowKind::Job => types::FlowKind::Job,
        WitCommonFlowKind::Http => types::FlowKind::Http,
    }
}

fn flow_kind_from_pack_wit(kind: WitPackFlowKind) -> types::FlowKind {
    match kind {
        WitPackFlowKind::Messaging => types::FlowKind::Messaging,
        WitPackFlowKind::Event => types::FlowKind::Event,
        WitPackFlowKind::ComponentConfig => types::FlowKind::ComponentConfig,
        WitPackFlowKind::Job => types::FlowKind::Job,
        WitPackFlowKind::Http => types::FlowKind::Http,
    }
}

fn flow_kind_to_pack_wit(kind: types::FlowKind) -> WitPackFlowKind {
    match kind {
        types::FlowKind::Messaging => WitPackFlowKind::Messaging,
        types::FlowKind::Event => WitPackFlowKind::Event,
        types::FlowKind::ComponentConfig => WitPackFlowKind::ComponentConfig,
        types::FlowKind::Job => WitPackFlowKind::Job,
        types::FlowKind::Http => WitPackFlowKind::Http,
    }
}

/// Convert the shared `PackKind` into the WIT `pack-kind`.
pub fn pack_kind_to_wit(kind: types::PackKind) -> WitPackKind {
    match kind {
        types::PackKind::Application => WitPackKind::Application,
        types::PackKind::Provider => WitPackKind::Provider,
        types::PackKind::Infrastructure => WitPackKind::Infrastructure,
        types::PackKind::Library => WitPackKind::Library,
    }
}

/// Convert a WIT `pack-kind` into the shared `PackKind`.
pub fn pack_kind_from_wit(kind: WitPackKind) -> types::PackKind {
    match kind {
        WitPackKind::Application => types::PackKind::Application,
        WitPackKind::Provider => types::PackKind::Provider,
        WitPackKind::Infrastructure => types::PackKind::Infrastructure,
        WitPackKind::Library => types::PackKind::Library,
    }
}

/// Convert a WIT `tenant-ctx` (v1 subset) into the shared `TenantCtx`.
pub fn tenant_ctx_from_common(ctx: WitCommonTenantCtx) -> MapperResult<types::TenantCtx> {
    let WitCommonTenantCtx {
        env,
        tenant_id,
        team_id,
        user_id,
        i18n_id,
        session_id,
        flow_id,
        node_id,
    } = ctx;
    let tenant_id: types::TenantId = tenant_id.try_into()?;
    let tenant = tenant_id.clone();
    let team = team_id
        .as_ref()
        .map(|id| id.as_str().try_into())
        .transpose()?;
    let team_id = team_id.map(|id| id.try_into()).transpose()?;
    let user = user_id
        .as_ref()
        .map(|id| id.as_str().try_into())
        .transpose()?;
    let user_id = user_id.map(|id| id.try_into()).transpose()?;

    Ok(types::TenantCtx {
        env: env.try_into()?,
        tenant,
        tenant_id,
        team,
        team_id,
        user,
        user_id,
        session_id,
        flow_id,
        node_id,
        provider_id: None,
        trace_id: None,
        i18n_id,
        correlation_id: None,
        attributes: BTreeMap::new(),
        deadline: None,
        attempt: 0,
        idempotency_key: None,
        impersonation: None,
    })
}

/// Convert a shared `TenantCtx` into the WIT `tenant-ctx` (v1 subset).
pub fn tenant_ctx_to_common(ctx: types::TenantCtx) -> MapperResult<WitCommonTenantCtx> {
    Ok(WitCommonTenantCtx {
        env: ctx.env.into(),
        tenant_id: ctx.tenant_id.into(),
        team_id: ctx.team_id.map(Into::into),
        user_id: ctx.user_id.map(Into::into),
        i18n_id: ctx.i18n_id,
        session_id: ctx.session_id,
        flow_id: ctx.flow_id,
        node_id: ctx.node_id,
    })
}

/// Convert a WIT `component-outcome` into the normalized struct.
pub fn component_outcome_from_wit(outcome: WitComponentOutcome) -> ComponentOutcome {
    let status = match outcome.status {
        WitOutcomeStatus::Done => ComponentOutcomeStatus::Done,
        WitOutcomeStatus::Pending => ComponentOutcomeStatus::Pending,
        WitOutcomeStatus::Error => ComponentOutcomeStatus::Error,
    };

    ComponentOutcome {
        status,
        code: outcome.code,
        payload: outcome.payload_json,
        metadata: outcome.metadata_json,
    }
}

/// Convert a normalized component outcome into the WIT `component-outcome`.
pub fn component_outcome_to_wit(outcome: ComponentOutcome) -> WitComponentOutcome {
    let status = match outcome.status {
        ComponentOutcomeStatus::Done => WitOutcomeStatus::Done,
        ComponentOutcomeStatus::Pending => WitOutcomeStatus::Pending,
        ComponentOutcomeStatus::Error => WitOutcomeStatus::Error,
    };

    WitComponentOutcome {
        status,
        code: outcome.code,
        payload_json: outcome.payload,
        metadata_json: outcome.metadata,
    }
}

/// Convert a WIT pack descriptor into the shared struct.
pub fn pack_descriptor_from_wit(desc: WitPackDescriptor) -> MapperResult<PackDescriptor> {
    Ok(PackDescriptor {
        pack_id: desc.pack_id.try_into()?,
        version: Version::parse(&desc.version)
            .map_err(|err| invalid_input(format!("invalid version: {err}")))?,
        kind: pack_kind_from_wit(desc.kind),
        publisher: desc.publisher,
    })
}

/// Convert a pack descriptor into the WIT shape.
pub fn pack_descriptor_to_wit(desc: PackDescriptor) -> WitPackDescriptor {
    WitPackDescriptor {
        pack_id: desc.pack_id.into(),
        version: desc.version.to_string(),
        kind: pack_kind_to_wit(desc.kind),
        publisher: desc.publisher,
    }
}

/// Convert a WIT flow descriptor into the shared struct.
pub fn flow_descriptor_from_wit(desc: WitFlowDescriptor) -> MapperResult<FlowDescriptor> {
    Ok(FlowDescriptor {
        id: desc.id.try_into()?,
        kind: flow_kind_from_pack_wit(desc.kind),
        tags: desc.tags,
        entrypoints: desc.entrypoints,
    })
}

/// Convert a flow descriptor into the WIT shape.
pub fn flow_descriptor_to_wit(desc: FlowDescriptor) -> WitFlowDescriptor {
    WitFlowDescriptor {
        id: desc.id.into(),
        kind: flow_kind_to_pack_wit(desc.kind),
        tags: desc.tags,
        entrypoints: desc.entrypoints,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryFrom;

    fn fixture_id<T>(value: &str) -> T
    where
        T: TryFrom<String, Error = types::GreenticError>,
    {
        T::try_from(value.to_owned())
            .unwrap_or_else(|err| panic!("invalid fixture identifier '{value}': {err}"))
    }

    fn sample_tenant_ctx() -> types::TenantCtx {
        types::TenantCtx {
            env: fixture_id("prod"),
            tenant: fixture_id("tenant-1"),
            tenant_id: fixture_id("tenant-1"),
            team: Some(fixture_id("team-42")),
            team_id: Some(fixture_id("team-42")),
            user: Some(fixture_id("user-7")),
            user_id: Some(fixture_id("user-7")),
            attributes: BTreeMap::new(),
            session_id: Some("sess-42".into()),
            flow_id: Some("flow-42".into()),
            node_id: Some("node-42".into()),
            provider_id: Some("provider-42".into()),
            trace_id: Some("trace".into()),
            i18n_id: Some("en-US".into()),
            correlation_id: Some("corr".into()),
            deadline: Some(types::InvocationDeadline::from_unix_millis(
                1_700_000_000_000,
            )),
            attempt: 2,
            idempotency_key: Some("idem".into()),
            impersonation: Some(types::Impersonation {
                actor_id: fixture_id("actor"),
                reason: Some("maintenance".into()),
            }),
        }
    }

    #[test]
    fn tenant_ctx_roundtrip() {
        let ctx = sample_tenant_ctx();
        let wit = match WitTenantCtx::try_from(ctx.clone()) {
            Ok(value) => value,
            Err(err) => panic!("failed to map to wit: {err}"),
        };
        let back = match types::TenantCtx::try_from(wit) {
            Ok(value) => value,
            Err(err) => panic!("failed to map from wit: {err}"),
        };
        assert_eq!(back.env.as_str(), ctx.env.as_str());
        assert_eq!(back.tenant.as_str(), ctx.tenant.as_str());
        assert!(back.impersonation.is_some());
        assert!(ctx.impersonation.is_some());
        assert_eq!(
            back.impersonation.as_ref().map(|imp| imp.actor_id.as_str()),
            ctx.impersonation.as_ref().map(|imp| imp.actor_id.as_str())
        );
        assert_eq!(back.session_id, ctx.session_id);
        assert_eq!(back.flow_id, ctx.flow_id);
        assert_eq!(back.node_id, ctx.node_id);
        assert_eq!(back.provider_id, ctx.provider_id);
    }

    #[test]
    fn outcome_roundtrip() {
        let pending = types::Outcome::Pending {
            reason: "waiting".into(),
            expected_input: Some(vec!["input".into()]),
        };
        let wit = WitOutcome::from(pending.clone());
        let back = types::Outcome::from(wit);
        match back {
            types::Outcome::Pending { reason, .. } => {
                assert_eq!(reason, "waiting");
            }
            _ => panic!("expected pending"),
        }
    }
}
