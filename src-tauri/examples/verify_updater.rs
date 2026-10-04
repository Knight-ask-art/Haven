//! Release-only verifier, not distributed in the application. Use the exact
//! minisign verification policy used by the official Tauri updater.
use base64::Engine;
use minisign_verify::{PublicKey, Signature};

fn verify() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("expected installer, signature file, and tauri config".into());
    }
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let key = config["plugins"]["updater"]["pubkey"]
        .as_str()
        .ok_or("missing update public key")?;
    let engine = base64::engine::general_purpose::STANDARD;
    let key = String::from_utf8(engine.decode(key.trim())?)?;
    let encoded_signature = std::fs::read_to_string(&args[1])?;
    let signature = String::from_utf8(engine.decode(encoded_signature.trim())?)?;
    let installer = std::fs::read(&args[0])?;
    PublicKey::decode(&key)?.verify(&installer, &Signature::decode(&signature)?, true)?;
    Ok(())
}

fn main() {
    if verify().is_err() {
        eprintln!("updater artifact verification failed; release must remain a draft");
        std::process::exit(1);
    }
    println!("updater artifact signature matches the installed application's public key");
}
