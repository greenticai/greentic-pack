use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::p2::add_to_linker_sync;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod component_bindings {
    wasmtime::component::bindgen!({
        inline: r#"
        package greentic:component@0.4.0;

        interface node {
            type json = string;

            record tenant-ctx {
                tenant: string,
                team: option<string>,
                user: option<string>,
                trace-id: option<string>,
                correlation-id: option<string>,
                deadline-unix-ms: option<u64>,
                attempt: u32,
                idempotency-key: option<string>,
            }

            record exec-ctx {
                tenant: tenant-ctx,
                flow-id: string,
                node-id: option<string>,
            }

            record node-error {
                code: string,
                message: string,
                retryable: bool,
                backoff-ms: option<u64>,
                details: option<json>,
            }

            variant invoke-result {
                ok(json),
                err(node-error),
            }

            variant stream-event {
                data(json),
                progress(u8),
                done,
                error(string),
            }

            enum lifecycle-status { ok }

            get-manifest: func() -> json;
            on-start: func(ctx: exec-ctx) -> result<lifecycle-status, string>;
            on-stop: func(ctx: exec-ctx, reason: string) -> result<lifecycle-status, string>;
            invoke: func(ctx: exec-ctx, op: string, input: json) -> invoke-result;
            invoke-stream: func(ctx: exec-ctx, op: string, input: json) -> list<stream-event>;
        }

        world component-node {
            export node;
        }
        "#,
    });
}

use component_bindings::ComponentNode;
use component_bindings::exports::greentic::component::node::{ExecCtx, InvokeResult, TenantCtx};

#[derive(Default)]
struct Ctx {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            table: &mut self.table,
            ctx: &mut self.wasi,
        }
    }
}

fn compose_adapter_and_router(
    adapter: &std::path::Path,
    router: &std::path::Path,
    out: &std::path::Path,
) {
    let output = Command::new("wasm-tools")
        .args([
            "compose",
            adapter.to_str().unwrap(),
            "-d",
            router.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("invoke wasm-tools compose");
    assert!(
        output.status.success(),
        "composition should succeed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.exists(), "composed artifact should exist");
}

fn inspect_exports(component: &std::path::Path) -> String {
    let output = Command::new("wasm-tools")
        .args(["component", "wit", component.to_str().unwrap()])
        .output()
        .expect("invoke wasm-tools component wit");
    assert!(
        output.status.success(),
        "wasm-tools component wit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("wit output utf8")
}

#[test]
fn composed_component_exports_greentic_world_and_imports_router() {
    let temp = TempDir::new().unwrap();
    let out = temp.path().join("composed.component.wasm");
    let adapter = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("mcp_adapter_25_06_18.component.wasm");
    let router = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("router-echo-component.wasm");

    compose_adapter_and_router(&adapter, &router, &out);
    let wit = inspect_exports(&out);
    assert!(
        wit.contains("export greentic:component/node@0.4.0"),
        "composed component should export greentic component world:\n{}",
        wit
    );
    assert!(
        !wit.contains("import wasix:mcp/router@25.6.18"),
        "router dependency should be satisfied by composition:\n{}",
        wit
    );
}

#[test]
fn composed_component_invokes_router_echo() {
    let temp = TempDir::new().unwrap();
    let out = temp.path().join("composed.component.wasm");
    let adapter = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("mcp_adapter_25_06_18.component.wasm");
    let router = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("router-echo-component.wasm");

    compose_adapter_and_router(&adapter, &router, &out);

    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).expect("engine");
    let component = Component::from_file(&engine, &out).expect("load composed component");

    let mut linker: Linker<Ctx> = Linker::new(&engine);
    add_to_linker_sync(&mut linker).expect("add wasi to linker");

    let wasi = WasiCtxBuilder::new()
        .inherit_stdio()
        .inherit_stdout()
        .inherit_stderr()
        .build();
    let state = Ctx {
        table: ResourceTable::new(),
        wasi,
    };
    let mut store = Store::new(&engine, state);

    let bindings = ComponentNode::instantiate(&mut store, &component, &linker)
        .expect("instantiate composed component");
    let instance = bindings.greentic_component_node();

    let ctx = ExecCtx {
        tenant: TenantCtx {
            tenant: "tenant".into(),
            team: None,
            user: None,
            trace_id: None,
            correlation_id: None,
            deadline_unix_ms: None,
            attempt: 0,
            idempotency_key: None,
        },
        flow_id: "flow".into(),
        node_id: Some("node".into()),
    };

    let input = r#"{"text":"hello"}"#.to_string();
    let attempts = [
        ("echo", input.clone()),
        (
            "call-tool",
            r#"{"tool":"echo","arguments":{"text":"hello"}}"#.to_string(),
        ),
        (
            "call-tool",
            r#"{"tool_name":"echo","arguments":{"text":"hello"}}"#.to_string(),
        ),
    ];

    let mut success = false;
    for (op, payload) in attempts {
        let result = instance
            .call_invoke(&mut store, &ctx, op, &payload)
            .expect("invoke should succeed");
        if let InvokeResult::Ok(json) = result
            && json.contains("hello")
        {
            success = true;
            break;
        }
    }

    assert!(success, "invoke result should echo input via router");
}
