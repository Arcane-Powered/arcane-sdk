//! Manual end-to-end check against a real Arcane desktop app.
//!
//! ```bash
//! cargo run --example client_init -- pk_your_portal_key
//! ```
//!
//! It renders a fake three-second loop calling `frame()`, prints the session
//! snapshot, then ends the session with `shutdown()`.
//!
//! Useful environment overrides (developer tooling — never set these in a game):
//!
//! - `ARCANE_DRM_ROOT`      point the SDK at a throwaway DRM directory
//! - `ARCANE_SDK_PORT`      talk to a stub or non-default loopback port
//! - `ARCANE_OFFLINE_ONLY`  never contact or launch Arcane desktop

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

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

            client.set_graphics("2560x1440", "high");

            for _ in 0..600 {
                client.frame();
                thread::sleep(Duration::from_millis(5));
            }

            let session = client.session();
            println!();
            println!("tracking    {}", session.tracking);
            println!("session_id  {:?}", session.session_id);
            println!("played_secs {}", session.played_seconds);
            println!("sampling    {}", session.fps_sampling);
            println!("samples     {}", session.samples_taken);
            println!("last_fps    {:?}", session.last_fps_avg);

            client.shutdown();
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
