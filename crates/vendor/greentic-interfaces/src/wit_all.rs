#![cfg(not(target_arch = "wasm32"))]
//! Unified Wasmtime bindings for every WIT world shipped with this crate.
#![allow(clippy::all)]
#![allow(missing_docs)]
#![allow(unused_macros)]

macro_rules! declare_world {
    (
        mod $mod_name:ident,
        path = $path_literal:literal,
        world = $world_literal:literal
        $(, legacy = { $($legacy:item)* } )?
    ) => {
        pub mod $mod_name {
            mod bindings {
                wasmtime::component::bindgen!({
                    path: $path_literal,
                    world: $world_literal,
                });
            }

            #[allow(unused_imports)]
            pub use bindings::*;

            $(
                $($legacy)*
            )?
        }
    };
}

#[cfg(feature = "describe-v1")]
declare_world!(
    mod component_describe_v1,
    path = "target/wit-staging/greentic-component-1.0.0",
    world = "greentic:component/component@1.0.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:component@1.0.0";
    }
);

#[cfg(feature = "component-v1")]
declare_world!(
    mod component_v1,
    path = "target/wit-staging/greentic-component-v1-0.1.0",
    world = "greentic:component-v1/component-host@0.1.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:component-v1@0.1.0";
    }
);

#[cfg(feature = "component-v0-5")]
declare_world!(
    mod component_v0_5,
    path = "target/wit-staging/greentic-component-0.5.0",
    world = "greentic:component/component@0.5.0",
    legacy = {
        use anyhow::Result as AnyResult;
        use wasmtime::component::{Component as WasmtimeComponent, Linker};
        use wasmtime::StoreContextMut;

        pub use bindings::greentic::component::control::Host as ControlHost;

        /// Registers the Greentic control interface with the provided linker.
        pub fn add_control_to_linker<T>(
            linker: &mut Linker<T>,
            get_host: impl Fn(&mut T) -> &mut (dyn ControlHost + Send + Sync + 'static)
                + Send
                + Sync
                + Copy
                + 'static,
        ) -> wasmtime::Result<()>
        where
            T: Send + 'static,
        {
            let mut inst = linker.instance("greentic:component/control@0.5.0")?;

            inst.func_wrap(
                "should-cancel",
                move |mut caller: StoreContextMut<'_, T>, (): ()| {
                    let host = get_host(caller.data_mut());
                    let result = host.should_cancel();
                    Ok((result,))
                },
            )?;

            inst.func_wrap(
                "yield-now",
                move |mut caller: StoreContextMut<'_, T>, (): ()| {
                    let host = get_host(caller.data_mut());
                    host.yield_now();
                    Ok(())
                },
            )?;

            Ok(())
        }

        /// Back-compat shim for instantiating the component.
        pub struct Component;

        impl Component {
            /// Loads the component from raw bytes, mirroring the old helper.
            pub fn instantiate(
                engine: &wasmtime::Engine,
                component_wasm: &[u8],
            ) -> AnyResult<WasmtimeComponent> {
                Ok(WasmtimeComponent::from_binary(engine, component_wasm)?)
            }
        }

        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:component@0.5.0";
    }
);

#[cfg(feature = "component-v0-6")]
declare_world!(
    mod component_v0_6,
    path = "target/wit-staging/greentic-component-0.6.0",
    world = "greentic:component/component@0.6.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:component@0.6.0";
    }
);

#[cfg(feature = "component-v0-5")]
declare_world!(
    mod component_configurable_v0_5,
    path = "target/wit-staging/greentic-component-0.5.0",
    world = "greentic:component/component-configurable@0.5.0",
    legacy = {
        use anyhow::Result as AnyResult;
        use wasmtime::component::{Component as WasmtimeComponent, Linker};
        use wasmtime::StoreContextMut;

        pub use bindings::greentic::component::control::Host as ControlHost;

        /// Registers the Greentic control interface with the provided linker.
        pub fn add_control_to_linker<T>(
            linker: &mut Linker<T>,
            get_host: impl Fn(&mut T) -> &mut (dyn ControlHost + Send + Sync + 'static)
                + Send
                + Sync
                + Copy
                + 'static,
        ) -> wasmtime::Result<()>
        where
            T: Send + 'static,
        {
            let mut inst = linker.instance("greentic:component/control@0.5.0")?;

            inst.func_wrap(
                "should-cancel",
                move |mut caller: StoreContextMut<'_, T>, (): ()| {
                    let host = get_host(caller.data_mut());
                    let result = host.should_cancel();
                    Ok((result,))
                },
            )?;

            inst.func_wrap(
                "yield-now",
                move |mut caller: StoreContextMut<'_, T>, (): ()| {
                    let host = get_host(caller.data_mut());
                    host.yield_now();
                    Ok(())
                },
            )?;

            Ok(())
        }

        /// Back-compat shim for instantiating the component.
        pub struct Component;

        impl Component {
            /// Loads the component from raw bytes, mirroring the old helper.
            pub fn instantiate(
                engine: &wasmtime::Engine,
                component_wasm: &[u8],
            ) -> AnyResult<WasmtimeComponent> {
                Ok(WasmtimeComponent::from_binary(engine, component_wasm)?)
            }
        }

        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:component@0.5.0";
    }
);

#[cfg(feature = "common-types-v0-1")]
declare_world!(
    mod common_types_v0_1,
    path = "target/wit-staging/greentic-common-types-0.1.0",
    world = "greentic:common-types/common@0.1.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:common-types@0.1.0";
    }
);

#[cfg(feature = "component-v0-4")]
declare_world!(
    mod component_v0_4,
    path = "target/wit-staging/greentic-component-0.4.0",
    world = "greentic:component/component@0.4.0",
    legacy = {
        use anyhow::Result as AnyResult;
        use wasmtime::component::{Component as WasmtimeComponent, Linker};
        use wasmtime::StoreContextMut;

        pub use bindings::greentic::component::control::Host as ControlHost;

        /// Registers the Greentic control interface with the provided linker.
        pub fn add_control_to_linker<T>(
            linker: &mut Linker<T>,
            get_host: impl Fn(&mut T) -> &mut (dyn ControlHost + Send + Sync + 'static)
                + Send
                + Sync
                + Copy
                + 'static,
        ) -> wasmtime::Result<()>
        where
            T: Send + 'static,
        {
            let mut inst = linker.instance("greentic:component/control@0.4.0")?;

            inst.func_wrap(
                "should-cancel",
                move |mut caller: StoreContextMut<'_, T>, (): ()| {
                    let host = get_host(caller.data_mut());
                    let result = host.should_cancel();
                    Ok((result,))
                },
            )?;

            inst.func_wrap(
                "yield-now",
                move |mut caller: StoreContextMut<'_, T>, (): ()| {
                    let host = get_host(caller.data_mut());
                    host.yield_now();
                    Ok(())
                },
            )?;

            Ok(())
        }

        /// Back-compat shim for instantiating the component.
        pub struct Component;

        impl Component {
            /// Loads the component from raw bytes, mirroring the old helper.
            pub fn instantiate(
                engine: &wasmtime::Engine,
                component_wasm: &[u8],
            ) -> AnyResult<WasmtimeComponent> {
                Ok(WasmtimeComponent::from_binary(engine, component_wasm)?)
            }
        }

        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:component@0.4.0";
    }
);

#[cfg(feature = "pack-export-v0-4")]
declare_world!(
    mod pack_export_v0_4,
    path = "target/wit-staging/greentic-pack-export-0.4.0",
    world = "greentic:pack-export/pack-exports@0.4.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:pack-export@0.4.0";
    }
);

#[cfg(feature = "pack-export-v1")]
declare_world!(
    mod pack_export_v1,
    path = "target/wit-staging/greentic-pack-export-v1-0.1.0",
    world = "greentic:pack-export-v1/pack-host@0.1.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:pack-export-v1@0.1.0";
    }
);

#[cfg(feature = "pack-validate-v0-1")]
declare_world!(
    mod pack_validate_v0_1,
    path = "target/wit-staging/greentic-pack-validate-0.1.0",
    world = "greentic:pack-validate/pack-validator@0.1.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:pack-validate@0.1.0";
    }
);

#[cfg(feature = "provision-v0-1")]
declare_world!(
    mod provision_v0_1,
    path = "target/wit-staging/greentic-provision-0.1.0",
    world = "greentic:provision/provision-runner@0.1.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:provision@0.1.0";
    }
);

#[cfg(feature = "types-core-v0-4")]
declare_world!(
    mod types_core_v0_4,
    path = "target/wit-staging/greentic-types-core-0.4.0",
    world = "greentic:types-core/core@0.4.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:types-core@0.4.0";
    }
);

#[cfg(feature = "runner-host-v1")]
declare_world!(
    mod runner_host_v1,
    path = "target/wit-staging/greentic-host-1.0.0",
    world = "greentic:host/runner-host@1.0.0",
    legacy = {
        use std::vec::Vec;
        use wasmtime::component::Linker;
        use wasmtime::{Result, StoreContextMut};

        pub use bindings::greentic::host::{http_v1, kv_v1};

        /// Minimal trait hosts implement to satisfy the runner-host imports.
        pub trait RunnerHost {
            fn http_request(
                &mut self,
                method: String,
                url: String,
                headers: Vec<String>,
                body: Option<Vec<u8>>,
            ) -> Result<Result<Vec<u8>, String>>;

            fn kv_get(&mut self, ns: String, key: String) -> Result<Option<String>>;

            fn kv_put(&mut self, ns: String, key: String, val: String) -> Result<()>;
        }

        /// Registers the runner-host interfaces with the provided linker.
        pub fn add_to_linker<T>(
            linker: &mut Linker<T>,
            get_host: impl Fn(&mut T) -> &mut (dyn RunnerHost + Send + Sync + 'static)
                + Send
                + Sync
                + Copy
                + 'static,
        ) -> Result<()>
        where
            T: Send + 'static,
        {
            let mut http = linker.instance("greentic:host/http-v1@1.0.0")?;
            http.func_wrap(
                "request",
                move |mut caller: StoreContextMut<'_, T>,
                      (method, url, headers, body): (String, String, Vec<String>, Option<Vec<u8>>)| {
                    let host = get_host(caller.data_mut());
                    host.http_request(method, url, headers, body)
                        .map(|res| (res,))
                },
            )?;

            let mut kv = linker.instance("greentic:host/kv-v1@1.0.0")?;
            kv.func_wrap(
                "get",
                move |mut caller: StoreContextMut<'_, T>, (ns, key): (String, String)| {
                    let host = get_host(caller.data_mut());
                    host.kv_get(ns, key).map(|res| (res,))
                },
            )?;
            kv.func_wrap(
                "put",
                move |mut caller: StoreContextMut<'_, T>, (ns, key, val): (String, String, String)| {
                    let host = get_host(caller.data_mut());
                    host.kv_put(ns, key, val)
                },
            )?;

            Ok(())
        }

        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:host@1.0.0";
    }
);

#[cfg(feature = "pack-export-v0-2")]
declare_world!(
    mod pack_export_v0_2,
    path = "target/wit-staging/greentic-pack-export-0.2.0",
    world = "greentic:pack-export/pack-exports@0.2.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:pack-export@0.2.0";
    }
);

#[cfg(feature = "types-core-v0-2")]
declare_world!(
    mod types_core_v0_2,
    path = "target/wit-staging/greentic-types-core-0.2.0",
    world = "greentic:types-core/core@0.2.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:types-core@0.2.0";
    }
);

#[cfg(feature = "oauth-broker-v1")]
declare_world!(
    mod oauth_broker_v1,
    path = "target/wit-staging/greentic-oauth-broker-1.0.0",
    world = "greentic:oauth-broker/broker@1.0.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:oauth-broker@1.0.0";
    }
);

#[cfg(feature = "oauth-broker-v1")]
declare_world!(
    mod oauth_broker_client_v1,
    path = "target/wit-staging/greentic-oauth-broker-1.0.0",
    world = "greentic:oauth-broker/broker-client@1.0.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:oauth-broker@1.0.0";
    }
);

#[cfg(feature = "component-lifecycle-v1")]
declare_world!(
    mod component_lifecycle_v1,
    path = "target/wit-staging/greentic-lifecycle-1.0.0",
    world = "greentic:lifecycle/component-lifecycle@1.0.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:lifecycle@1.0.0";
    }
);

#[cfg(feature = "secrets-store-v1")]
declare_world!(
    mod secrets_store_v1,
    path = "target/wit-staging/greentic-secrets-store-1.0.0",
    world = "greentic:secrets-store/store@1.0.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:secrets-store@1.0.0";
    }
);

#[cfg(feature = "provider-core-v1")]
declare_world!(
    mod provider_schema_core_v1,
    path = "target/wit-staging/greentic-provider-schema-core-1.0.0",
    world = "greentic:provider-schema-core/schema-core@1.0.0",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "greentic:provider-schema-core@1.0.0";
    }
);

#[cfg(feature = "provider-common")]
declare_world!(
    mod provider_common,
    path = "target/wit-staging/provider-common-0.0.2",
    world = "provider:common/common@0.0.2"
);

#[cfg(feature = "state-store-v1")]
declare_world!(
    mod state_store_v1,
    path = "target/wit-staging/greentic-state-1.0.0",
    world = "greentic:state/store@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:state@1.0.0";
    }
);

#[cfg(feature = "http-client-v1")]
declare_world!(
    mod http_client_v1,
    path = "target/wit-staging/greentic-http-1.0.0",
    world = "greentic:http/client@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:http@1.0.0";
    }
);

#[cfg(feature = "http-client-v1-1")]
declare_world!(
    mod http_client_v1_1,
    path = "target/wit-staging/greentic-http-1.1.0",
    world = "greentic:http/client@1.1.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:http@1.1.0";
    }
);

#[cfg(feature = "telemetry-logger-v1")]
declare_world!(
    mod telemetry_logger_v1,
    path = "target/wit-staging/greentic-telemetry-1.0.0",
    world = "greentic:telemetry/logger@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:telemetry@1.0.0";
    }
);

#[cfg(feature = "source-v1")]
declare_world!(
    mod source_v1,
    path = "target/wit-staging/greentic-source-1.0.0",
    world = "greentic:source/source-sync@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:source@1.0.0";
    }
);

#[cfg(feature = "build-v1")]
declare_world!(
    mod build_v1,
    path = "target/wit-staging/greentic-build-1.0.0",
    world = "greentic:build/builder@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:build@1.0.0";
    }
);

#[cfg(feature = "scan-v1")]
declare_world!(
    mod scan_v1,
    path = "target/wit-staging/greentic-scan-1.0.0",
    world = "greentic:scan/scanner@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:scan@1.0.0";
    }
);

#[cfg(feature = "signing-v1")]
declare_world!(
    mod signing_v1,
    path = "target/wit-staging/greentic-signing-1.0.0",
    world = "greentic:signing/signer@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:signing@1.0.0";
    }
);

#[cfg(feature = "attestation-v1")]
declare_world!(
    mod attestation_v1,
    path = "target/wit-staging/greentic-attestation-1.0.0",
    world = "greentic:attestation/attester@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:attestation@1.0.0";
    }
);

#[cfg(feature = "policy-v1")]
declare_world!(
    mod policy_v1,
    path = "target/wit-staging/greentic-policy-1.0.0",
    world = "greentic:policy/policy-evaluator@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:policy@1.0.0";
    }
);

#[cfg(feature = "metadata-v1")]
declare_world!(
    mod metadata_v1,
    path = "target/wit-staging/greentic-metadata-1.0.0",
    world = "greentic:metadata/metadata-store@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:metadata@1.0.0";
    }
);

#[cfg(feature = "distribution-v1")]
declare_world!(
    mod distribution_v1,
    path = "target/wit-staging/greentic-distribution-1.0.0",
    world = "greentic:distribution/distribution@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:distribution@1.0.0";
    }
);

#[cfg(feature = "distributor-api")]
declare_world!(
    mod distributor_api_v1,
    path = "target/wit-staging/greentic-distributor-api-1.0.0",
    world = "greentic:distributor-api/distributor-api@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:distributor-api@1.0.0";
    }
);

#[cfg(feature = "distributor-api-v1-1")]
declare_world!(
    mod distributor_api_v1_1,
    path = "target/wit-staging/greentic-distributor-api-1.1.0",
    world = "greentic:distributor-api/distributor-api@1.1.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:distributor-api@1.1.0";
    }
);

#[cfg(feature = "oci-v1")]
declare_world!(
    mod oci_v1,
    path = "target/wit-staging/greentic-oci-1.0.0",
    world = "greentic:oci/oci-distribution@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:oci@1.0.0";
    }
);

#[cfg(feature = "repo-ui-actions-v1")]
declare_world!(
    mod repo_ui_actions_repo_ui_worker_v1,
    path = "target/wit-staging/greentic-repo-ui-actions-1.0.0",
    world = "greentic:repo-ui-actions/repo-ui-worker@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:repo-ui-actions@1.0.0";
    }
);

#[cfg(feature = "gui-fragment")]
declare_world!(
    mod gui_fragment_v1,
    path = "target/wit-staging/greentic-gui-1.0.0",
    world = "greentic:gui/gui-fragment@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:gui@1.0.0";
    }
);

#[cfg(feature = "worker-api")]
declare_world!(
    mod worker_v1,
    path = "target/wit-staging/greentic-worker-1.0.0",
    world = "greentic:worker/worker@1.0.0",
    legacy = {
        pub const PACKAGE_ID: &str = "greentic:worker@1.0.0";
    }
);

#[cfg(feature = "wasix-mcp-24-11-05")]
declare_world!(
    mod wasix_mcp_24_11_05,
    path = "wit/wasix-mcp@24.11.05.wit",
    world = "mcp-router",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "wasix:mcp@24.11.5";
    }
);

#[cfg(feature = "wasix-mcp-25-03-26")]
declare_world!(
    mod wasix_mcp_25_03_26,
    path = "wit/wasix-mcp@25.03.26.wit",
    world = "mcp-router",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "wasix:mcp@25.3.26";
    }
);

#[cfg(feature = "wasix-mcp-25-06-18")]
declare_world!(
    mod wasix_mcp_25_06_18,
    path = "wit/wasix-mcp@25.06.18.wit",
    world = "mcp-router",
    legacy = {
        /// Canonical package identifier.
        pub const PACKAGE_ID: &str = "wasix:mcp@25.6.18";
    }
);
