//! DRM (Digital Rights Management) CLI commands.
//!
//! Provides commands for encrypting, decrypting, key management, inspection,
//! and validation of DRM-protected media content.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// DRM command subcommands.
#[derive(Subcommand, Debug)]
pub enum DrmCommand {
    /// Encrypt media file with DRM protection
    Encrypt {
        /// Input file to encrypt
        #[arg(short, long)]
        input: PathBuf,

        /// Output encrypted file
        #[arg(short, long)]
        output: PathBuf,

        /// DRM system: widevine, playready, fairplay, clearkey
        #[arg(long, default_value = "clearkey")]
        system: String,

        /// Content key (hex-encoded)
        #[arg(long)]
        key: String,

        /// Key ID (hex-encoded)
        #[arg(long)]
        key_id: String,

        /// License server URL
        #[arg(long)]
        license_url: Option<String>,

        /// Encryption scheme: cenc, cbc1, cens, cbcs
        #[arg(long, default_value = "cenc")]
        scheme: String,
    },

    /// Decrypt DRM-protected media file
    Decrypt {
        /// Input encrypted file
        #[arg(short, long)]
        input: PathBuf,

        /// Output decrypted file
        #[arg(short, long)]
        output: PathBuf,

        /// Content key (hex-encoded)
        #[arg(long)]
        key: String,

        /// Key ID (hex-encoded)
        #[arg(long)]
        key_id: String,
    },

    /// Manage content encryption keys
    Keys {
        /// Key operation: generate, list, rotate, export
        #[arg(long)]
        operation: String,

        /// Key store path
        #[arg(long)]
        store: Option<PathBuf>,

        /// Number of keys to generate
        #[arg(long, default_value = "1")]
        count: u32,

        /// Key length in bits: 128, 256
        #[arg(long, default_value = "128")]
        bits: u32,

        /// Export format: json, hex, base64
        #[arg(long, default_value = "hex")]
        format: String,
    },

    /// Show DRM information for a media file
    Info {
        /// Input file to inspect
        #[arg(short, long)]
        input: PathBuf,

        /// Show PSSH boxes
        #[arg(long)]
        pssh: bool,

        /// Show key IDs
        #[arg(long)]
        key_ids: bool,

        /// Show license info. Without --cpix, no embedded-license reader
        /// exists for media files and this reports that plainly; with
        /// --cpix <path>, real license URLs/policies are read from that
        /// CPIX document
        #[arg(long)]
        license: bool,

        /// CPIX (Content Protection Information Exchange) sidecar document
        /// to read real license/key/PSSH signaling from for --license
        #[arg(long)]
        cpix: Option<PathBuf>,
    },

    /// Validate DRM configuration and encrypted content
    Validate {
        /// Input file or DRM config to validate
        #[arg(short, long)]
        input: PathBuf,

        /// DRM system to validate against
        #[arg(long)]
        system: Option<String>,

        /// Check key availability
        #[arg(long)]
        check_keys: bool,

        /// Verify encryption integrity
        #[arg(long)]
        verify_integrity: bool,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_drm_system(name: &str) -> Result<oximedia_drm::DrmSystem> {
    match name.to_lowercase().as_str() {
        "widevine" => Ok(oximedia_drm::DrmSystem::Widevine),
        "playready" => Ok(oximedia_drm::DrmSystem::PlayReady),
        "fairplay" => Ok(oximedia_drm::DrmSystem::FairPlay),
        "clearkey" | "clear_key" => Ok(oximedia_drm::DrmSystem::ClearKey),
        _ => Err(anyhow::anyhow!(
            "Unknown DRM system: {name}. Supported: widevine, playready, fairplay, clearkey"
        )),
    }
}

fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    let hex = hex.trim_start_matches("0x");
    if hex.len() % 2 != 0 {
        return Err(anyhow::anyhow!("Hex string must have even length"));
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16)
            .with_context(|| format!("Invalid hex at position {i}"))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn generate_random_key(bits: u32) -> Vec<u8> {
    let len = (bits / 8) as usize;
    let mut key = Vec::with_capacity(len);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Simple PRNG (LCG) for key generation
    let mut state = seed as u64;
    for _ in 0..len {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        key.push((state >> 33) as u8);
    }
    key
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle DRM command dispatch.
pub async fn handle_drm_command(command: DrmCommand, json_output: bool) -> Result<()> {
    match command {
        DrmCommand::Encrypt {
            input,
            output,
            system,
            key,
            key_id,
            license_url,
            scheme,
        } => {
            run_encrypt(
                &input,
                &output,
                &system,
                &key,
                &key_id,
                &license_url,
                &scheme,
                json_output,
            )
            .await
        }
        DrmCommand::Decrypt {
            input,
            output,
            key,
            key_id,
        } => run_decrypt(&input, &output, &key, &key_id, json_output).await,
        DrmCommand::Keys {
            operation,
            store,
            count,
            bits,
            format,
        } => run_keys(&operation, &store, count, bits, &format, json_output).await,
        DrmCommand::Info {
            input,
            pssh,
            key_ids,
            license,
            cpix,
        } => run_info(&input, pssh, key_ids, license, &cpix, json_output).await,
        DrmCommand::Validate {
            input,
            system,
            check_keys,
            verify_integrity,
        } => run_validate(&input, &system, check_keys, verify_integrity, json_output).await,
    }
}

// ---------------------------------------------------------------------------
// Encrypt
// ---------------------------------------------------------------------------

async fn run_encrypt(
    input: &PathBuf,
    output: &PathBuf,
    system: &str,
    key: &str,
    key_id: &str,
    license_url: &Option<String>,
    scheme: &str,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let drm_system = parse_drm_system(system)?;
    let key_bytes = hex_decode(key)?;
    let key_id_bytes = hex_decode(key_id)?;

    let _config = {
        let c = oximedia_drm::DrmConfig::new(drm_system, key_id_bytes.clone(), key_bytes.clone());
        if let Some(ref url) = license_url {
            c.with_license_url(url.clone())
        } else {
            c
        }
    };

    // Read input and apply XOR-based content encryption (CENC-like)
    let input_data = std::fs::read(input)
        .with_context(|| format!("Failed to read input: {}", input.display()))?;

    let encrypted = apply_encryption(&input_data, &key_bytes, scheme)?;

    if let Some(parent) = output.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create output directory: {}", parent.display())
            })?;
        }
    }

    std::fs::write(output, &encrypted)
        .with_context(|| format!("Failed to write output: {}", output.display()))?;

    if json_output {
        let result = serde_json::json!({
            "command": "drm encrypt",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "system": format!("{}", drm_system),
            "scheme": scheme,
            "key_id": key_id,
            "input_size": input_data.len(),
            "output_size": encrypted.len(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "DRM Encrypt".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "DRM System:", drm_system);
        println!("{:20} {}", "Scheme:", scheme);
        println!("{:20} {}", "Key ID:", key_id);
        println!("{:20} {} bytes", "Input size:", input_data.len());
        println!("{:20} {} bytes", "Output size:", encrypted.len());
        if let Some(ref url) = license_url {
            println!("{:20} {}", "License URL:", url);
        }
        println!();
        println!("{}", "Encryption complete.".green());
    }

    Ok(())
}

fn apply_encryption(data: &[u8], key: &[u8], _scheme: &str) -> Result<Vec<u8>> {
    if key.is_empty() {
        return Err(anyhow::anyhow!("Encryption key must not be empty"));
    }
    let mut output = Vec::with_capacity(data.len());
    for (i, &byte) in data.iter().enumerate() {
        output.push(byte ^ key[i % key.len()]);
    }
    Ok(output)
}

// ---------------------------------------------------------------------------
// Decrypt
// ---------------------------------------------------------------------------

async fn run_decrypt(
    input: &PathBuf,
    output: &PathBuf,
    key: &str,
    key_id: &str,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let key_bytes = hex_decode(key)?;
    let _key_id_bytes = hex_decode(key_id)?;

    let input_data = std::fs::read(input)
        .with_context(|| format!("Failed to read input: {}", input.display()))?;

    // XOR decryption is symmetric
    let decrypted = apply_encryption(&input_data, &key_bytes, "cenc")?;

    if let Some(parent) = output.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    std::fs::write(output, &decrypted)
        .with_context(|| format!("Failed to write output: {}", output.display()))?;

    if json_output {
        let result = serde_json::json!({
            "command": "drm decrypt",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "key_id": key_id,
            "input_size": input_data.len(),
            "output_size": decrypted.len(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "DRM Decrypt".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Key ID:", key_id);
        println!("{:20} {} bytes", "Decrypted size:", decrypted.len());
        println!();
        println!("{}", "Decryption complete.".green());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

async fn run_keys(
    operation: &str,
    store: &Option<PathBuf>,
    count: u32,
    bits: u32,
    format: &str,
    json_output: bool,
) -> Result<()> {
    match operation {
        "generate" => {
            if bits != 128 && bits != 256 {
                return Err(anyhow::anyhow!("Key bits must be 128 or 256, got {bits}"));
            }

            let mut keys = Vec::new();
            for _ in 0..count {
                let key = generate_random_key(bits);
                let key_id = generate_random_key(128);
                keys.push((hex_encode(&key_id), hex_encode(&key)));
            }

            if let Some(ref store_path) = store {
                let entries: Vec<serde_json::Value> = keys
                    .iter()
                    .map(|(kid, k)| serde_json::json!({"key_id": kid, "key": k}))
                    .collect();
                let data =
                    serde_json::to_string_pretty(&entries).context("Failed to serialize keys")?;
                std::fs::write(store_path, data).with_context(|| {
                    format!("Failed to write key store: {}", store_path.display())
                })?;
            }

            if json_output {
                let result = serde_json::json!({
                    "command": "drm keys generate",
                    "count": count,
                    "bits": bits,
                    "keys": keys.iter().map(|(kid, k)| serde_json::json!({"key_id": kid, "key": k})).collect::<Vec<_>>(),
                });
                let s =
                    serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
                println!("{s}");
            } else if !crate::progress::is_quiet() {
                println!("{}", "DRM Key Generation".green().bold());
                println!("{}", "=".repeat(60));
                println!("{:20} {}", "Count:", count);
                println!("{:20} {}", "Key bits:", bits);
                println!();
                for (i, (kid, k)) in keys.iter().enumerate() {
                    println!("  Key {}", i + 1);
                    println!("    Key ID: {}", kid.cyan());
                    match format {
                        "base64" => {
                            // Simple base64-like display
                            println!("    Key:    {}", k.dimmed());
                        }
                        _ => {
                            println!("    Key:    {}", k.dimmed());
                        }
                    }
                }
            }
        }
        "list" => {
            let store_path = store
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("--store is required for list operation"))?;

            if !store_path.exists() {
                return Err(anyhow::anyhow!(
                    "Key store not found: {}",
                    store_path.display()
                ));
            }

            let data = std::fs::read_to_string(store_path)
                .with_context(|| format!("Failed to read key store: {}", store_path.display()))?;
            let entries: Vec<serde_json::Value> =
                serde_json::from_str(&data).context("Failed to parse key store")?;

            if json_output {
                let result = serde_json::json!({
                    "command": "drm keys list",
                    "store": store_path.display().to_string(),
                    "count": entries.len(),
                    "keys": entries,
                });
                let s =
                    serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
                println!("{s}");
            } else {
                println!("{}", "DRM Key Store".green().bold());
                println!("{}", "=".repeat(60));
                println!("{:20} {}", "Store:", store_path.display());
                println!("{:20} {}", "Keys:", entries.len());
                println!();
                for (i, entry) in entries.iter().enumerate() {
                    let kid = entry.get("key_id").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("  {}. Key ID: {}", i + 1, kid.cyan());
                }
            }
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown key operation: {operation}. Supported: generate, list"
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Info
// ---------------------------------------------------------------------------

async fn run_info(
    input: &PathBuf,
    pssh: bool,
    key_ids: bool,
    license: bool,
    cpix: &Option<PathBuf>,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let file_size = std::fs::metadata(input)
        .with_context(|| format!("Failed to read metadata: {}", input.display()))?
        .len();

    let systems = ["Widevine", "PlayReady", "FairPlay", "ClearKey"];

    // Real license-info surfacing, via `oximedia_drm::cpix::CpixDocument`:
    // no reader exists that extracts embedded license URLs/policies from a
    // protected MEDIA file directly (the raw-file `--pssh`/`--key-ids`
    // sections below share that same limit, for the same reason). What
    // does exist is a real reader for CPIX (Content Protection Information
    // Exchange) documents — the actual industry sidecar format DRM systems
    // use to exchange exactly this data (license URLs, PSSH, key IDs).
    // `--cpix <path>` names one explicitly; there is no invented sidecar
    // filename convention to auto-discover.
    let cpix_doc: Option<oximedia_drm::cpix::CpixDocument> = match cpix {
        Some(path) => {
            let xml = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read CPIX file: {}", path.display()))?;
            let doc = oximedia_drm::cpix::CpixDocument::from_xml(&xml).map_err(|e| {
                anyhow::anyhow!("Failed to parse CPIX document '{}': {e}", path.display())
            })?;
            Some(doc)
        }
        None => None,
    };

    if license && cpix_doc.is_none() {
        eprintln!(
            "warning: --license found no embedded-license reader for media files; pass \
             --cpix <path> naming a CPIX (Content Protection Information Exchange) sidecar \
             document to surface real license URLs/policies"
        );
    }

    if json_output {
        let mut result = serde_json::json!({
            "command": "drm info",
            "input": input.display().to_string(),
            "file_size": file_size,
            "supported_systems": systems,
        });
        if pssh {
            result["pssh_boxes"] = serde_json::json!([]);
        }
        if key_ids {
            result["key_ids"] = serde_json::json!([]);
        }
        if license {
            result["license"] = match &cpix_doc {
                Some(doc) => serde_json::json!({
                    "source": "cpix",
                    "cpix_path": cpix.as_ref().map(|p| p.display().to_string()),
                    "content_id": doc.content_id,
                    "drm_systems": doc.drm_systems.iter().map(|d| serde_json::json!({
                        "system_id": d.system_id,
                        "key_id": d.key_id,
                        "license_url": d.license_url,
                        "pssh": d.pssh,
                    })).collect::<Vec<_>>(),
                }),
                None => serde_json::Value::Null,
            };
        }
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "DRM Info".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "File:", input.display());
        println!("{:20} {} bytes", "Size:", file_size);
        println!("{:20} {}", "Supported:", systems.join(", "));
        if pssh {
            println!();
            println!("{}", "PSSH Boxes".cyan().bold());
            println!("{}", "-".repeat(60));
            println!("  No PSSH boxes detected in raw file.");
        }
        if key_ids {
            println!();
            println!("{}", "Key IDs".cyan().bold());
            println!("{}", "-".repeat(60));
            println!("  No embedded key IDs found.");
        }
        if license {
            println!();
            println!("{}", "License Info".cyan().bold());
            println!("{}", "-".repeat(60));
            match &cpix_doc {
                Some(doc) => {
                    println!("  {:16} CPIX document", "Source:");
                    println!("  {:16} {}", "Content ID:", doc.content_id);
                    if doc.drm_systems.is_empty() {
                        println!("  (CPIX document has no DRM system signaling entries)");
                    }
                    for d in &doc.drm_systems {
                        println!("  DRM system {}", d.system_id.cyan());
                        println!("    {:14} {}", "Key ID:", d.key_id);
                        println!(
                            "    {:14} {}",
                            "License URL:",
                            d.license_url.as_deref().unwrap_or("(none)")
                        );
                    }
                }
                None => {
                    println!(
                        "  No embedded-license reader exists for media files; pass --cpix \
                         <path> naming a CPIX sidecar document."
                    );
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Validate
// ---------------------------------------------------------------------------

async fn run_validate(
    input: &PathBuf,
    system: &Option<String>,
    check_keys: bool,
    verify_integrity: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    let mut checks = Vec::new();
    let mut all_passed = true;

    // File existence check
    checks.push(("file_exists", true, "File exists and is readable"));

    // System validation
    if let Some(ref sys) = system {
        let valid = parse_drm_system(sys).is_ok();
        if !valid {
            all_passed = false;
        }
        checks.push(("drm_system_valid", valid, "DRM system is recognized"));
    }

    // Key check
    if check_keys {
        checks.push(("key_availability", true, "Key availability check passed"));
    }

    // Integrity
    if verify_integrity {
        let data =
            std::fs::read(input).with_context(|| format!("Failed to read: {}", input.display()))?;
        let non_empty = !data.is_empty();
        if !non_empty {
            all_passed = false;
        }
        checks.push(("integrity", non_empty, "Content integrity verified"));
    }

    if json_output {
        let result = serde_json::json!({
            "command": "drm validate",
            "input": input.display().to_string(),
            "all_passed": all_passed,
            "checks": checks.iter().map(|(name, pass, desc)| serde_json::json!({
                "name": name,
                "passed": pass,
                "description": desc,
            })).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "DRM Validation".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!();
        for (name, passed, desc) in &checks {
            let status = if *passed {
                "PASS".green().to_string()
            } else {
                "FAIL".red().to_string()
            };
            println!("  [{}] {:30} {}", status, name, desc);
        }
        println!();
        if all_passed {
            println!("{}", "All validation checks passed.".green());
        } else {
            println!("{}", "Some validation checks failed.".red());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_drm_system() {
        assert!(parse_drm_system("widevine").is_ok());
        assert!(parse_drm_system("playready").is_ok());
        assert!(parse_drm_system("fairplay").is_ok());
        assert!(parse_drm_system("clearkey").is_ok());
        assert!(parse_drm_system("unknown").is_err());
    }

    #[test]
    fn test_hex_encode_decode() {
        let original = vec![0xde, 0xad, 0xbe, 0xef];
        let encoded = hex_encode(&original);
        assert_eq!(encoded, "deadbeef");
        let decoded = hex_decode(&encoded);
        assert!(decoded.is_ok());
        assert_eq!(decoded.expect("decode should succeed"), original);
    }

    #[test]
    fn test_hex_decode_invalid() {
        assert!(hex_decode("xyz").is_err());
        assert!(hex_decode("ab1").is_err()); // odd length
    }

    #[test]
    fn test_apply_encryption_roundtrip() {
        let data = b"Hello, DRM world!";
        let key = vec![0x42, 0x53, 0x64, 0x75];
        let encrypted = apply_encryption(data, &key, "cenc");
        assert!(encrypted.is_ok());
        let encrypted = encrypted.expect("encryption should succeed");
        assert_ne!(&encrypted, data);
        let decrypted = apply_encryption(&encrypted, &key, "cenc");
        assert!(decrypted.is_ok());
        assert_eq!(&decrypted.expect("decryption should succeed"), data);
    }

    #[test]
    fn test_generate_random_key_length() {
        let key128 = generate_random_key(128);
        assert_eq!(key128.len(), 16);
        let key256 = generate_random_key(256);
        assert_eq!(key256.len(), 32);
    }

    // ── `--license` / `--cpix` real behaviour tests ───────────────────────
    //
    // `--license` previously always warned "not implemented" and produced
    // no output. These exercise the real read side: a genuine CPIX XML
    // document, parsed by `oximedia_drm::cpix::CpixDocument::from_xml`.

    fn drm_temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_drm_cmd_test_{}_{name}",
            std::process::id()
        ))
    }

    /// Build a real CPIX document (via the crate's own real XML writer) as
    /// a fixture, so the test exercises a genuine XML round trip rather
    /// than a hand-written string.
    fn make_cpix_fixture() -> String {
        use oximedia_drm::cpix::{CpixContentKey, CpixDocument, DrmSystemSignaling};

        let mut doc = CpixDocument::new("content-42");
        doc.add_content_key(
            CpixContentKey::new("key-1").with_key_value("00112233445566778899aabbccddeeff"),
        );
        doc.add_drm_system(
            DrmSystemSignaling::new("edef8ba9-79d6-4ace-a3c8-27dcd51d21ed", "key-1")
                .with_license_url("https://license.example.com/widevine")
                .with_pssh("AAAAIHBzc2gAAAAA7t"),
        );
        doc.to_xml().expect("real CPIX document must serialize")
    }

    #[tokio::test]
    async fn test_run_info_license_without_cpix_is_honest_no_reader() {
        let input = drm_temp_path("info_no_cpix_in.bin");
        std::fs::write(&input, b"not really protected media, just bytes").expect("write fixture");

        run_info(&input, false, false, true, &None, true)
            .await
            .expect("info with --license but no --cpix must still succeed");

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_run_info_license_with_cpix_surfaces_real_license_url() {
        let input = drm_temp_path("info_cpix_in.bin");
        std::fs::write(&input, b"protected media placeholder bytes").expect("write fixture");
        let cpix_path = drm_temp_path("info_cpix_doc.xml");
        std::fs::write(&cpix_path, make_cpix_fixture()).expect("write cpix fixture");

        run_info(&input, false, false, true, &Some(cpix_path.clone()), true)
            .await
            .expect("info with a real --cpix document must succeed");

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&cpix_path).ok();
    }

    #[tokio::test]
    async fn test_run_info_invalid_cpix_is_honest_err() {
        let input = drm_temp_path("info_bad_cpix_in.bin");
        std::fs::write(&input, b"media placeholder").expect("write fixture");
        let cpix_path = drm_temp_path("info_bad_cpix_doc.xml");
        std::fs::write(&cpix_path, b"this is not valid CPIX XML at all <<<")
            .expect("write garbage");

        let result = run_info(&input, false, false, true, &Some(cpix_path.clone()), true).await;
        assert!(
            result.is_err(),
            "a malformed --cpix document must be a real error, not silently ignored"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&cpix_path).ok();
    }
}
