//! Manual end-to-end check against a real Arcane desktop app.
//!
//! ```bash
//! cargo run --example client_init -- pk_your_portal_key
//! ```
//!
//! Useful environment overrides (developer tooling — never set these in a game):
//!
//! - `ARCANE_DRM_ROOT`      point the SDK at a throwaway DRM directory
//! - `ARCANE_SDK_PORT`      talk to a stub or non-default loopback port
//! - `ARCANE_OFFLINE_ONLY`  never contact or launch Arcane desktop

use std::process::ExitCode;

use arcane_sdk::ArcaneClient;

fn main() -> ExitCode {
    let Some(public_key) = std::env::args().nth(1) else {
        eprintln!("usage: client_init <public_key>");
        return ExitCode::FAILURE;
    };

    match ArcaneClient::init(&public_key) {
        Ok(client) => {
            println!("ownership   {}", client.ownership());
            println!("owned       {}", client.is_owned());
            println!("public_key  {}", client.public_key());
            println!("game_id     {:?}", client.game_id());
            println!("user_id     {:?}", client.user_id());
            println!("device_hash {}", client.device_hash());
            println!("expires_at  {:?}", client.ticket_expires_at());
            println!("checked_at  {}", client.checked_at());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("init failed: {err}");
            eprintln!();
            eprintln!("code      {}", err.code());
            eprintln!("message   {}", err.message());
            eprintln!("hint      {}", err.hint().unwrap_or("—"));
            eprintln!("retryable {}", err.is_retryable());
            for (key, value) in err.context() {
                eprintln!("  {key} = {value}");
            }
            eprintln!();
            eprintln!("json      {}", err.to_json());
            ExitCode::FAILURE
        }
    }
}
