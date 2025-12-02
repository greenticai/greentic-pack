#![cfg_attr(target_arch = "wasm32", no_std)]

#[cfg(target_arch = "wasm32")]
extern crate alloc;

mod data;

#[cfg(target_arch = "wasm32")]
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
#[cfg(not(target_arch = "wasm32"))]
use greentic_interfaces_host::bindings::exports::greentic::interfaces_pack::component_api::ProviderMeta;
#[cfg(not(target_arch = "wasm32"))]
const _: fn(ProviderMeta) = |_meta| {};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use std::vec::Vec;

#[derive(Debug, Clone, Serialize)]
pub struct FlowInfo {
    pub id: String,
    pub human_name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchemaDoc {
    pub flow_id: String,
    pub schema_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrepareResult {
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    pub status: String,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct A2AItem {
    pub title: String,
    pub flow_id: String,
}

pub trait PackExport {
    fn list_flows(&self) -> Vec<FlowInfo>;
    fn get_flow_schema(&self, flow_id: &str) -> Option<SchemaDoc>;
    fn prepare_flow(&self, flow_id: &str) -> PrepareResult;
    fn run_flow(&self, flow_id: &str, input: serde_json::Value) -> RunResult;
    fn a2a_search(&self, query: &str) -> Vec<A2AItem>;
}

/// Return the embedded pack manifest as CBOR bytes.
pub fn manifest_cbor() -> &'static [u8] {
    data::MANIFEST_CBOR
}

/// Decode the embedded pack manifest into a `serde_json::Value`.
pub fn manifest_value() -> Value {
    serde_cbor::from_slice(data::MANIFEST_CBOR)
        .expect("generated manifest bytes should always be valid CBOR")
}

/// Decode the embedded manifest into a strongly-typed `T`.
pub fn manifest_as<T>() -> T
where
    T: for<'de> Deserialize<'de>,
{
    serde_cbor::from_slice(data::MANIFEST_CBOR)
        .expect("generated manifest matches the requested type")
}

/// Access the embedded flow sources as `(id, raw_ygtc)` tuples.
pub fn flows() -> &'static [(&'static str, &'static str)] {
    data::FLOWS
}

/// Access the embedded templates as `(logical_path, bytes)` tuples.
pub fn templates() -> &'static [(&'static str, &'static [u8])] {
    data::TEMPLATES
}

/// Lookup a template payload by logical path.
pub fn template_by_path(path: &str) -> Option<&'static [u8]> {
    data::TEMPLATES
        .iter()
        .find(|(logical, _)| *logical == path)
        .map(|(_, bytes)| *bytes)
}

/// Component instance implementing the `greentic:pack-export` interface.
#[derive(Debug, Default)]
pub struct Component;

impl PackExport for Component {
    fn list_flows(&self) -> Vec<FlowInfo> {
        flows()
            .iter()
            .map(|(id, _)| FlowInfo {
                id: (*id).to_string(),
                human_name: None,
                description: None,
            })
            .collect()
    }

    fn get_flow_schema(&self, flow_id: &str) -> Option<SchemaDoc> {
        flows()
            .iter()
            .find(|(id, _)| *id == flow_id)
            .map(|(id, _)| SchemaDoc {
                flow_id: (*id).to_string(),
                schema_json: serde_json::json!({}),
            })
    }

    fn prepare_flow(&self, flow_id: &str) -> PrepareResult {
        if flows().iter().any(|(id, _)| *id == flow_id) {
            PrepareResult {
                status: "ok".into(),
                error: None,
            }
        } else {
            PrepareResult {
                status: "error".into(),
                error: Some(format!("unknown flow: {flow_id}")),
            }
        }
    }

    fn run_flow(&self, flow_id: &str, input: Value) -> RunResult {
        if let Some((_, source)) = flows().iter().find(|(id, _)| *id == flow_id) {
            RunResult {
                status: "ok".into(),
                output: Some(serde_json::json!({
                    "flow_id": flow_id,
                    "source": source,
                    "input_echo": input,
                })),
                error: None,
            }
        } else {
            RunResult {
                status: "error".into(),
                output: None,
                error: Some(format!("unknown flow: {flow_id}")),
            }
        }
    }

    fn a2a_search(&self, _query: &str) -> Vec<A2AItem> {
        Vec::new()
    }
}

/// Convenience helper for host environments that want an owned component.
pub fn component() -> Component {
    Component
}

// Export simple C ABI shims for the stub interface so a Wasm harness can
// exercise the component without native bindings.
#[unsafe(no_mangle)]
pub extern "C" fn greentic_pack_export__list_flows(json_buffer: *mut u8, len: usize) -> usize {
    let component = Component;
    let flows = component.list_flows();
    write_json_response(&flows, json_buffer, len)
}

#[unsafe(no_mangle)]
/// # Safety
///
/// The caller must ensure that `flow_id_ptr` points to `flow_id_len` bytes of
/// valid UTF-8 and that `json_buffer` points to a writable region of at least
/// `len` bytes when non-null.
pub unsafe extern "C" fn greentic_pack_export__prepare_flow(
    flow_id_ptr: *const u8,
    flow_id_len: usize,
    json_buffer: *mut u8,
    len: usize,
) -> usize {
    let component = Component;
    let flow_id = unsafe { slice_to_str(flow_id_ptr, flow_id_len) };
    let result = component.prepare_flow(flow_id);
    write_json_response(&result, json_buffer, len)
}

#[unsafe(no_mangle)]
/// # Safety
///
/// The caller must ensure that `flow_id_ptr` points to `flow_id_len` bytes of
/// valid UTF-8 and that `json_buffer` points to a writable region of at least
/// `len` bytes when non-null.
pub unsafe extern "C" fn greentic_pack_export__run_flow(
    flow_id_ptr: *const u8,
    flow_id_len: usize,
    json_buffer: *mut u8,
    len: usize,
) -> usize {
    let component = Component;
    let flow_id = unsafe { slice_to_str(flow_id_ptr, flow_id_len) };
    let result = component.run_flow(flow_id, serde_json::Value::Null);
    write_json_response(&result, json_buffer, len)
}

#[unsafe(no_mangle)]
pub extern "C" fn greentic_pack_export__a2a_search(json_buffer: *mut u8, len: usize) -> usize {
    let component = Component;
    let items = component.a2a_search("");
    write_json_response(&items, json_buffer, len)
}

fn write_json_response<T: serde::Serialize>(value: &T, buffer: *mut u8, len: usize) -> usize {
    let json = serde_json::to_vec(value).expect("serialisation succeeds");
    if buffer.is_null() || len == 0 {
        return json.len();
    }

    let copy_len = core::cmp::min(json.len(), len);
    unsafe {
        core::ptr::copy_nonoverlapping(json.as_ptr(), buffer, copy_len);
    }
    copy_len
}

unsafe fn slice_to_str<'a>(ptr: *const u8, len: usize) -> &'a str {
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    core::str::from_utf8(bytes).expect("flow id is valid utf-8")
}
