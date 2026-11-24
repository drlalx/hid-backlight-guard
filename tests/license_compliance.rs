//! License compatibility checks for all crate dependencies.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::Command;

/// Approved permissive licenses for third-party dependencies.
/// Keep in sync with `deny.toml`.
const ALLOWED_LICENSES: &[&str] = &[
    "0BSD",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "CDLA-Permissive-2.0",
    "ISC",
    "MIT",
    "OFL-1.1",
    "Ubuntu-font-1.0",
    "Unicode-3.0",
    "Unlicense",
    "Zlib",
];

fn is_license_allowed(lic: &str) -> bool {
    let trimmed = lic.trim();
    ALLOWED_LICENSES
        .iter()
        .any(|&allowed| allowed.eq_ignore_ascii_case(trimmed))
}

#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    License(&'a str),
    Or,
    And,
    OpenParen,
    CloseParen,
}

fn tokenize_spdx(input: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = input.char_indices().peekable();

    while let Some(&(i, ch)) = chars.peek() {
        match ch {
            ' ' | '\t' | '\r' | '\n' => {
                chars.next();
            }
            '(' => {
                tokens.push(Token::OpenParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::CloseParen);
                chars.next();
            }
            '/' => {
                tokens.push(Token::Or);
                chars.next();
            }
            _ => {
                let start = i;
                let mut end = i;
                while let Some(&(j, c)) = chars.peek() {
                    if c == '(' || c == ')' || c == '/' || c.is_whitespace() {
                        break;
                    }
                    end = j + c.len_utf8();
                    chars.next();
                }
                let word = &input[start..end];
                if word.eq_ignore_ascii_case("OR") {
                    tokens.push(Token::Or);
                } else if word.eq_ignore_ascii_case("AND") {
                    tokens.push(Token::And);
                } else if word.eq_ignore_ascii_case("WITH") {
                    if let Some(Token::License(prev)) = tokens.pop() {
                        while let Some(&(_, c)) = chars.peek() {
                            if !c.is_whitespace() {
                                break;
                            }
                            chars.next();
                        }
                        let mut exc_end = end;
                        while let Some(&(j, c)) = chars.peek() {
                            if c == '(' || c == ')' || c == '/' || c.is_whitespace() {
                                break;
                            }
                            exc_end = j + c.len_utf8();
                            chars.next();
                        }
                        let prev_offset = prev.as_ptr() as usize - input.as_ptr() as usize;
                        tokens.push(Token::License(input[prev_offset..exc_end].trim()));
                    } else {
                        tokens.push(Token::License(word));
                    }
                } else {
                    tokens.push(Token::License(word));
                }
            }
        }
    }

    tokens
}

fn eval_spdx_tokens(tokens: &[Token<'_>]) -> bool {
    if tokens.is_empty() {
        return false;
    }

    let mut depth = 0usize;
    for (idx, token) in tokens.iter().enumerate() {
        match token {
            Token::OpenParen => depth += 1,
            Token::CloseParen => depth = depth.saturating_sub(1),
            Token::Or if depth == 0 => {
                return eval_spdx_tokens(&tokens[..idx]) || eval_spdx_tokens(&tokens[idx + 1..]);
            }
            _ => {}
        }
    }

    depth = 0;
    for (idx, token) in tokens.iter().enumerate() {
        match token {
            Token::OpenParen => depth += 1,
            Token::CloseParen => depth = depth.saturating_sub(1),
            Token::And if depth == 0 => {
                return eval_spdx_tokens(&tokens[..idx]) && eval_spdx_tokens(&tokens[idx + 1..]);
            }
            _ => {}
        }
    }

    if tokens.first() == Some(&Token::OpenParen) && tokens.last() == Some(&Token::CloseParen) {
        let mut inner_depth = 0usize;
        let mut balanced = true;
        for token in &tokens[1..tokens.len() - 1] {
            match token {
                Token::OpenParen => inner_depth += 1,
                Token::CloseParen => {
                    if inner_depth == 0 {
                        balanced = false;
                        break;
                    }
                    inner_depth -= 1;
                }
                _ => {}
            }
        }
        if balanced && inner_depth == 0 {
            return eval_spdx_tokens(&tokens[1..tokens.len() - 1]);
        }
    }

    if tokens.len() == 1
        && let Token::License(lic) = tokens[0]
    {
        return is_license_allowed(lic);
    }

    false
}

fn is_license_compatible(expression: &str) -> bool {
    let tokens = tokenize_spdx(expression);
    eval_spdx_tokens(&tokens)
}

#[test]
fn only_approved_licenses_are_used() {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .output()
        .expect("cargo metadata must execute successfully");

    assert!(output.status.success(), "cargo metadata failed");

    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Failed to parse cargo metadata JSON");

    let packages = json["packages"]
        .as_array()
        .expect("Expected packages array in metadata");

    let mut violations = Vec::new();

    for pkg in packages {
        let name = pkg["name"].as_str().unwrap_or("unknown");
        let version = pkg["version"].as_str().unwrap_or("0.0.0");

        if name == "hid-backlight-guard" {
            continue;
        }

        match pkg["license"].as_str() {
            Some(lic) => {
                if !is_license_compatible(lic) {
                    violations.push(format!(
                        "{name} v{version} has incompatible license: '{lic}'"
                    ));
                }
            }
            None => {
                violations.push(format!("{name} v{version} has NO license specified"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Found dependencies with unapproved licenses:\n- {}",
        violations.join("\n- ")
    );
}

#[test]
fn test_license_compatibility_logic() {
    assert!(is_license_compatible("MIT"));
    assert!(is_license_compatible("Apache-2.0"));
    assert!(is_license_compatible("Apache-2.0 WITH LLVM-exception"));
    assert!(is_license_compatible("MIT OR Apache-2.0"));
    assert!(is_license_compatible("Apache-2.0 / MIT"));
    assert!(is_license_compatible("Apache-2.0 OR GPL-2.0-only"));
    assert!(is_license_compatible("(MIT OR Apache-2.0) AND Unicode-3.0"));
    assert!(!is_license_compatible("MIT AND GPL-3.0-only"));
    assert!(!is_license_compatible("GPL-3.0-only"));
    assert!(!is_license_compatible("AGPL-3.0-only"));
    assert!(!is_license_compatible("Proprietary"));
    assert!(!is_license_compatible(""));
}
