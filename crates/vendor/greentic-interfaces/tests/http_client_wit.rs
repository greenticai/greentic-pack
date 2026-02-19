use std::path::Path;

fn load(path: &str) -> String {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(base.join(path)).expect("reading wit fixture")
}

fn squish(s: &str) -> String {
    s.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

#[test]
fn http_client_v1_0_signature_is_two_args() {
    let wit = squish(&load("wit/greentic/http-client@1.0.0/package.wit"));
    assert!(
        wit.contains("send:func(req:request,ctx:option<tenant-ctx>)->result<response,host-error>;"),
        "http-client@1.0.0 send signature drifted"
    );
    assert!(
        !wit.contains("request-options"),
        "http-client@1.0.0 should not expose request-options"
    );
}

#[test]
fn http_client_v1_1_signature_is_three_args() {
    let wit = squish(&load("wit/greentic/http-client@1.1.0/package.wit"));
    assert!(
        wit.contains(
            "send:func(req:request,opts:option<request-options>,ctx:option<tenant-ctx>)->result<response,host-error>;"
        ),
        "http-client@1.1.0 send signature drifted"
    );
    assert!(
        wit.contains("recordrequest-options{timeout-ms:option<u32>,allow-insecure:option<bool>,follow-redirects:option<bool>,}"),
        "http-client@1.1.0 request-options changed unexpectedly"
    );
}
