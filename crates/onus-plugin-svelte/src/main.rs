//! `onus-plugin-svelte`: a language plugin for Svelte components.
//!
//! In a plugins file:
//!
//! ```yaml
//! plugins:
//!   - { name: svelte, kind: language, command: [onus-plugin-svelte], files: ["**/*.svelte"] }
//! ```
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::Read;

use onus_core::protocol::{PLUGIN_PROTOCOL_VERSION, PluginKind, PluginRequest, PluginResponse};

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn main() {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        fail("cannot read the request");
    }
    let request: PluginRequest =
        serde_json::from_str(&input).unwrap_or_else(|e| fail(&format!("invalid request: {e}")));
    if request.protocol != PLUGIN_PROTOCOL_VERSION {
        fail(&format!("unsupported protocol {}", request.protocol));
    }
    let mut response = PluginResponse {
        protocol: PLUGIN_PROTOCOL_VERSION,
        version: Some(format!("onus-plugin-svelte {}", env!("CARGO_PKG_VERSION"))),
        ..PluginResponse::default()
    };
    if request.kind == PluginKind::Language {
        let all = onus_map::walk::list_files(&request.workspace.root);
        response.map =
            onus_plugin_svelte::analyze(&request.workspace, &all).unwrap_or_else(|e| fail(&e));
    }
    match serde_json::to_string(&response) {
        Ok(json) => println!("{json}"),
        Err(e) => fail(&format!("cannot encode the response: {e}")),
    }
}
