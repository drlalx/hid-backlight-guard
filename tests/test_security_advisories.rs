//! Security advisory (OSV/CVE) checks for all crate dependencies.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Serialize)]
struct OsvQueryBatch {
    queries: Vec<OsvPackageQuery>,
}

#[derive(Serialize)]
struct OsvPackageQuery {
    package: OsvPackageInfo,
    version: String,
}

#[derive(Serialize)]
struct OsvPackageInfo {
    name: String,
    ecosystem: &'static str,
}

#[derive(Deserialize, Debug)]
struct OsvBatchResponse {
    #[serde(default)]
    results: Vec<OsvResultItem>,
}

#[derive(Deserialize, Debug)]
struct OsvResultItem {
    #[serde(default)]
    vulns: Vec<OsvVulnerability>,
}

#[derive(Deserialize, Debug)]
struct OsvVulnerability {
    id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    severity: Option<Vec<serde_json::Value>>,
}

impl OsvVulnerability {
    /// Determines whether this advisory represents an actionable security vulnerability
    /// rather than an informational/unmaintained notice.
    fn is_security_vulnerability(&self) -> bool {
        if let Some(summary) = &self.summary
            && summary.to_lowercase().contains("unmaintained")
        {
            return false;
        }

        if self.id.starts_with("GHSA-") || self.id.starts_with("CVE-") {
            return true;
        }

        if let Some(sev) = &self.severity
            && !sev.is_empty()
        {
            return true;
        }

        if let Some(aliases) = &self.aliases
            && aliases
                .iter()
                .any(|alias| alias.starts_with("CVE-") || alias.starts_with("GHSA-"))
        {
            return true;
        }

        true
    }
}

fn curl_get(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args(["-sSf", "--max-time", "15", url])
        .output()
        .map_err(|e| format!("Failed to invoke curl: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "curl GET failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("Invalid UTF-8 response: {e}"))
}

fn curl_post_json(url: &str, body: &str) -> Result<String, String> {
    let mut child = Command::new("curl")
        .args([
            "-sSf",
            "--max-time",
            "15",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn curl: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body.as_bytes())
            .map_err(|e| format!("Failed to write request body to curl: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to read curl output: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "curl POST failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("Invalid UTF-8 response: {e}"))
}

fn fetch_vuln_full(id: &str) -> Result<OsvVulnerability, String> {
    let url = format!("https://api.osv.dev/v1/vulns/{id}");
    let body = curl_get(&url)?;
    serde_json::from_str(&body).map_err(|e| format!("Failed to parse advisory: {e}"))
}

fn query_osv_vulnerabilities(
    packages: &[(&str, &str)],
) -> Result<Vec<(String, String, Vec<OsvVulnerability>)>, String> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }

    let queries: Vec<OsvPackageQuery> = packages
        .iter()
        .map(|(name, ver)| OsvPackageQuery {
            package: OsvPackageInfo {
                name: (*name).to_string(),
                ecosystem: "crates.io",
            },
            version: (*ver).to_string(),
        })
        .collect();

    let batch = OsvQueryBatch { queries };
    let json_body =
        serde_json::to_string(&batch).map_err(|e| format!("Failed to serialize OSV query: {e}"))?;

    let body = curl_post_json("https://api.osv.dev/v1/querybatch", &json_body)?;
    let batch_resp: OsvBatchResponse =
        serde_json::from_str(&body).map_err(|e| format!("Failed to parse OSV response: {e}"))?;

    let mut found = Vec::new();
    for (idx, result) in batch_resp.results.into_iter().enumerate() {
        if !result.vulns.is_empty() {
            let (name, ver) = packages[idx];
            let mut full_vulns = Vec::new();
            for stub in result.vulns {
                if let Ok(full) = fetch_vuln_full(&stub.id) {
                    full_vulns.push(full);
                } else {
                    full_vulns.push(stub);
                }
            }
            found.push((name.to_string(), ver.to_string(), full_vulns));
        }
    }

    Ok(found)
}

#[test]
fn test_osv_vulnerability_classification() {
    let cve = OsvVulnerability {
        id: "RUSTSEC-2021-0124".to_string(),
        summary: Some("Data race in oneshot channel".to_string()),
        aliases: Some(vec!["CVE-2021-45710".to_string()]),
        severity: None,
    };
    assert!(cve.is_security_vulnerability());

    let unmaintained = OsvVulnerability {
        id: "RUSTSEC-2024-0001".to_string(),
        summary: Some("Crate is unmaintained".to_string()),
        aliases: None,
        severity: None,
    };
    assert!(!unmaintained.is_security_vulnerability());
}

#[test]
fn test_no_security_vulnerabilities_in_dependencies() {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .output()
        .expect("cargo metadata execution failed");

    assert!(output.status.success(), "cargo metadata returned failure");

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse metadata JSON");

    let packages = json["packages"]
        .as_array()
        .expect("packages array in metadata");

    let mut dep_list = Vec::new();
    for pkg in packages {
        let name = pkg["name"].as_str().unwrap_or_default();
        let version = pkg["version"].as_str().unwrap_or_default();

        if name == "hid-backlight-guard" || name.is_empty() || version.is_empty() {
            continue;
        }

        dep_list.push((name, version));
    }

    dep_list.sort_unstable();
    dep_list.dedup();

    let mut vulnerabilities = Vec::new();
    let mut informational_notices = Vec::new();

    for chunk in dep_list.chunks(500) {
        match query_osv_vulnerabilities(chunk) {
            Ok(found) => {
                for (name, version, vulns) in found {
                    for v in vulns {
                        if v.is_security_vulnerability() {
                            let summary = v.summary.unwrap_or_else(|| "No summary".to_string());
                            let aliases = v.aliases.map(|a| a.join(", ")).unwrap_or_default();
                            vulnerabilities.push(format!(
                                "{name} v{version} - {summary} (Advisory: {}, Aliases: {aliases})",
                                v.id
                            ));
                        } else {
                            let summary = v.summary.unwrap_or_default();
                            informational_notices
                                .push(format!("{name} v{version} (Notice: {} - {summary})", v.id));
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("Warning: Could not contact OSV advisory API (offline mode): {err}");
                return;
            }
        }
    }

    if !informational_notices.is_empty() {
        println!("\nInformational dependency notices (non-vulnerabilities):");
        for notice in &informational_notices {
            println!("  ℹ️  {notice}");
        }
    }

    assert!(
        vulnerabilities.is_empty(),
        "\nCRITICAL: Found {} security vulnerabilities in project dependencies:\n- {}",
        vulnerabilities.len(),
        vulnerabilities.join("\n- ")
    );
}
