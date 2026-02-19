use std::path::{Path, PathBuf};
use std::process::Command;

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView, p2};

mod harness_bindings {
    mod bindings {
        wasmtime::component::bindgen!({
            path: "../../guest-tests/oauth-broker-client-harness/wit",
            world: "test:oauth-broker-client-harness/harness@0.1.0",
        });
    }

    pub use bindings::*;
}

use harness_bindings::Harness;

struct BrokerHost;

impl BrokerHost {
    fn get_consent_url(
        &mut self,
        provider_id: String,
        subject: String,
        scopes: Vec<String>,
        redirect_path: String,
        extra_json: String,
    ) -> String {
        format!("consent:{provider_id}:{subject}:{redirect_path}:{extra_json}:{scopes:?}")
    }

    fn exchange_code(
        &mut self,
        provider_id: String,
        subject: String,
        code: String,
        redirect_path: String,
    ) -> String {
        format!("exchange:{provider_id}:{subject}:{code}:{redirect_path}")
    }

    fn get_token(&mut self, provider_id: String, subject: String, scopes: Vec<String>) -> String {
        format!("token:{provider_id}:{subject}:{scopes:?}")
    }
}

struct HostState {
    broker: BrokerHost,
    table: ResourceTable,
    wasi: WasiCtx,
}

impl HostState {
    fn new() -> anyhow::Result<Self> {
        let table = ResourceTable::new();
        let wasi = WasiCtxBuilder::new().inherit_stdio().inherit_env().build();
        Ok(Self {
            broker: BrokerHost,
            table,
            wasi,
        })
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl harness_bindings::test::oauth_broker_client_harness::broker_v1::Host for HostState {
    fn get_consent_url(
        &mut self,
        provider_id: String,
        subject: String,
        scopes: Vec<String>,
        redirect_path: String,
        extra_json: String,
    ) -> String {
        self.broker
            .get_consent_url(provider_id, subject, scopes, redirect_path, extra_json)
    }

    fn exchange_code(
        &mut self,
        provider_id: String,
        subject: String,
        code: String,
        redirect_path: String,
    ) -> String {
        self.broker
            .exchange_code(provider_id, subject, code, redirect_path)
    }

    fn get_token(&mut self, provider_id: String, subject: String, scopes: Vec<String>) -> String {
        self.broker.get_token(provider_id, subject, scopes)
    }
}

#[test]
fn broker_client_imports_roundtrip() -> anyhow::Result<()> {
    if !wasm_target_available() {
        eprintln!("skipping broker-client roundtrip: wasm32-wasip2 target missing");
        return Ok(());
    }

    let wasm_path = build_guest_component()?;
    let engine = new_component_engine()?;
    let component = Component::from_file(&engine, &wasm_path)?;
    let mut linker = Linker::new(&engine);
    harness_bindings::test::oauth_broker_client_harness::broker_v1::add_to_linker::<
        _,
        HasSelf<HostState>,
    >(&mut linker, |state| state)?;
    p2::add_to_linker_sync(&mut linker)?;

    let mut store = Store::new(&engine, HostState::new()?);
    let instance = linker.instantiate(&mut store, &component)?;
    let harness = Harness::new(&mut store, &instance)?;
    let results = harness
        .test_oauth_broker_client_harness_harness_api()
        .call_run(&mut store)?;

    assert_eq!(
        results,
        vec![
            "consent:provider:subject:/cb:{}:[\"openid\", \"profile\"]".to_string(),
            "token:provider:subject:[\"openid\", \"profile\"]".to_string(),
            "exchange:provider:subject:auth-code:/cb".to_string()
        ]
    );

    Ok(())
}

fn wasm_target_available() -> bool {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    let Ok(output) = output else {
        return false;
    };
    output.status.success() && String::from_utf8_lossy(&output.stdout).contains("wasm32-wasip2")
}

fn build_guest_component() -> anyhow::Result<PathBuf> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| anyhow::anyhow!("unable to locate workspace root"))?;

    let status = Command::new("cargo")
        .current_dir(workspace_root)
        .args([
            "build",
            "--package",
            "oauth-broker-client-harness",
            "--target",
            "wasm32-wasip2",
        ])
        .status()?;

    if !status.success() {
        anyhow::bail!("guest harness build failed");
    }

    let wasm_path =
        workspace_root.join("target/wasm32-wasip2/debug/oauth_broker_client_harness.wasm");
    if !wasm_path.exists() {
        anyhow::bail!("missing built harness artifact at {}", wasm_path.display());
    }

    Ok(wasm_path)
}

fn new_component_engine() -> anyhow::Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    Engine::new(&config)
}
