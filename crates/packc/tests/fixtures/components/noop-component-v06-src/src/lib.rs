use greentic_interfaces_guest::component_v0_6::{
    component_descriptor, component_i18n, component_qa, component_runtime, component_schema,
};

struct NoopComponent;

impl component_descriptor::Guest for NoopComponent {
    fn get_component_info() -> Vec<u8> {
        Vec::new()
    }

    fn describe() -> Vec<u8> {
        include_bytes!("../describe.cbor").to_vec()
    }
}

impl component_schema::Guest for NoopComponent {
    fn input_schema() -> Vec<u8> {
        Vec::new()
    }

    fn output_schema() -> Vec<u8> {
        Vec::new()
    }

    fn config_schema() -> Vec<u8> {
        Vec::new()
    }
}

impl component_runtime::Guest for NoopComponent {
    fn run(_input: Vec<u8>, state: Vec<u8>) -> component_runtime::RunResult {
        component_runtime::RunResult {
            output: Vec::new(),
            new_state: state,
        }
    }
}

impl component_qa::Guest for NoopComponent {
    fn qa_spec(_mode: component_qa::QaMode) -> Vec<u8> {
        Vec::new()
    }

    fn apply_answers(
        _mode: component_qa::QaMode,
        current_config: Vec<u8>,
        _answers: Vec<u8>,
    ) -> Vec<u8> {
        current_config
    }
}

impl component_i18n::Guest for NoopComponent {
    fn i18n_keys() -> Vec<String> {
        Vec::new()
    }
}

greentic_interfaces_guest::export_component_v060!(NoopComponent);
