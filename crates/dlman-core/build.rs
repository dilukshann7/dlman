//! Bake verified per-file hashes into the executable after package preparation.
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let manifest = root.join("apps/desktop/src-tauri/resources/media-tools/manifest.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("tools/media-tools.lock.json").display()
    );
    // Source-only/CLI builds need no tools. A desktop release prepares this file first.
    let required = env::var_os("DLMAN_MEDIA_TOOLS_REQUIRED").is_some();
    println!("cargo:rerun-if-env-changed=DLMAN_MEDIA_TOOLS_REQUIRED");
    let contents = match fs::read_to_string(&manifest) {
        Ok(contents) => {
            let value: serde_json::Value =
                serde_json::from_str(&contents).expect("Invalid prepared media manifest");
            let lock_bytes = fs::read(root.join("tools/media-tools.lock.json"))
                .expect("Media tool lock missing");
            let lock: serde_json::Value =
                serde_json::from_slice(&lock_bytes).expect("Invalid media tool lock");
            let platform = value["platform"].as_str().expect("Missing media platform");
            let target = env::var("TARGET").unwrap();
            if lock["targets"][platform]["target"].as_str() != Some(target.as_str()) {
                assert!(!required, "Prepared YouTube tools do not match the release target; run fetch-media-tools.py");
                "{}".to_owned()
            } else {
                let expected = format!("{:x}", Sha256::digest(&lock_bytes));
                assert_eq!(
                    value["source_lock_sha256"].as_str(),
                    Some(expected.as_str()),
                    "Stale media tool manifest; run fetch-media-tools.py"
                );
                let archives: Vec<_> = lock["targets"][platform]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["sha256"].clone())
                    .collect();
                assert_eq!(
                    value["archives"],
                    serde_json::Value::Array(archives),
                    "Prepared tools differ from pinned archives"
                );
                assert_eq!(
                    value["files"].as_object().map(|files| files.len()),
                    Some(4),
                    "Prepare all four YouTube executables"
                );
                contents
            }
        }
        Err(_) => {
            assert!(
                !required,
                "YouTube resources missing; run tools/fetch-media-tools.py before packaging"
            );
            "{}".into()
        }
    };
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("youtube-tools.json"),
        contents,
    )
    .unwrap();
}
