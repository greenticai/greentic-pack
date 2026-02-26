#![forbid(unsafe_code)]

use anyhow::Result;
use greentic_interfaces_wasmtime::host_helpers::v1::state_store::{
    self as state_store_v1, StateStoreError, StateStoreHost,
};
use wasmtime::component::Linker;
use wasmtime_wasi::p2::add_to_linker_sync;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

pub struct DescribeHostState {
    table: ResourceTable,
    wasi: WasiCtx,
}

impl Default for DescribeHostState {
    fn default() -> Self {
        // Describe-only paths should be offline-safe: no inherited env/args,
        // no preopened directories, and no ambient stdio requirements.
        Self {
            table: ResourceTable::new(),
            wasi: WasiCtxBuilder::new().build(),
        }
    }
}

impl WasiView for DescribeHostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            table: &mut self.table,
            ctx: &mut self.wasi,
        }
    }
}

impl StateStoreHost for DescribeHostState {
    fn read(
        &mut self,
        _key: state_store_v1::StateKey,
        _ctx: Option<state_store_v1::TenantCtx>,
    ) -> std::result::Result<Vec<u8>, StateStoreError> {
        Ok(Vec::new())
    }

    fn write(
        &mut self,
        _key: state_store_v1::StateKey,
        _bytes: Vec<u8>,
        _ctx: Option<state_store_v1::TenantCtx>,
    ) -> std::result::Result<state_store_v1::OpAck, StateStoreError> {
        Ok(state_store_v1::OpAck::Ok)
    }

    fn delete(
        &mut self,
        _key: state_store_v1::StateKey,
        _ctx: Option<state_store_v1::TenantCtx>,
    ) -> std::result::Result<state_store_v1::OpAck, StateStoreError> {
        Ok(state_store_v1::OpAck::Ok)
    }
}

pub fn add_describe_host_imports(linker: &mut Linker<DescribeHostState>) -> Result<()> {
    add_to_linker_sync(linker)
        .map_err(|err| anyhow::anyhow!("register wasi preview2 describe host stubs: {err}"))?;
    state_store_v1::add_state_store_to_linker(linker, |host: &mut DescribeHostState| host)
        .map_err(|err| anyhow::anyhow!("register state-store@1.0.0 describe host stub: {err}"))?;
    Ok(())
}
