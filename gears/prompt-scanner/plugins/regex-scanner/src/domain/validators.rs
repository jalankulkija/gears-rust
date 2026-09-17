//! Structural/checksum validators shared by `domain::patterns::*`.
//!
//! Kept separate from the patterns that use them so each validator is
//! independently unit-testable against known-good/known-bad values, per
//! dlp-bench's own `deterministic` module split (see
//! `gears/onnx-runner/docs/DESIGN.md`).

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

/// Luhn checksum, used to validate candidate credit card numbers.
/// `digits` must contain only ASCII digits (callers pre-filter via regex).
#[must_use]
pub fn luhn_check(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for ch in digits.chars().rev() {
        let Some(mut d) = ch.to_digit(10) else {
            return false;
        };
        if double {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
        double = !double;
    }
    sum.is_multiple_of(10)
}

/// IBAN mod-97-10 checksum (ISO 7064). `iban` must be uppercase
/// alphanumeric with no spaces (callers normalize before calling).
#[must_use]
pub fn iban_checksum_valid(iban: &str) -> bool {
    if iban.len() < 15 || iban.len() > 34 {
        return false;
    }
    if !iban.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }

    // Move the first four characters (country code + check digits) to the
    // end, then convert letters to numbers (A=10, B=11, ...).
    let rearranged = format!("{}{}", &iban[4..], &iban[..4]);
    let mut numeric = String::with_capacity(rearranged.len() * 2);
    for ch in rearranged.chars() {
        if ch.is_ascii_digit() {
            numeric.push(ch);
        } else if ch.is_ascii_uppercase() {
            numeric.push_str(&(u32::from(ch as u8 - b'A') + 10).to_string());
        } else {
            return false;
        }
    }

    // mod-97 over a big numeric string, computed digit-by-digit to avoid
    // overflow.
    let mut remainder: u32 = 0;
    for ch in numeric.chars() {
        let digit = ch.to_digit(10).unwrap_or(0);
        remainder = (remainder * 10 + digit) % 97;
    }
    remainder == 1
}

#[must_use]
pub fn is_valid_ipv4(s: &str) -> bool {
    Ipv4Addr::from_str(s).is_ok()
}

#[must_use]
pub fn is_valid_ipv6(s: &str) -> bool {
    Ipv6Addr::from_str(s).is_ok()
}

/// Shannon entropy in bits/char, used as a gate for generic `key=value`
/// secret detection (a low-entropy value like `password=changeme` is not
/// worth flagging; a high-entropy one likely is).
#[must_use]
pub fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<char, u32> = HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    // Short scanned values only; a lossless usize->f64 conversion isn't
    // needed for an entropy heuristic.
    #[allow(clippy::cast_precision_loss)]
    let len = s.chars().count() as f64;
    counts
        .values()
        .map(|&count| {
            let p = f64::from(count) / len;
            -p * p.log2()
        })
        .sum()
}

/// Structural check for a JWT: three `.`-separated base64url segments where
/// the first (header) segment decodes to something JSON-shaped. Does not
/// validate the signature — this is a shape check, not cryptographic
/// verification.
#[must_use]
pub fn looks_like_jwt(candidate: &str) -> bool {
    let parts: Vec<&str> = candidate.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    if !parts.iter().all(|p| {
        p.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }) {
        return false;
    }
    decode_base64url(parts[0]).is_some_and(|bytes| bytes.first() == Some(&b'{'))
}

/// Minimal base64url decoder (no external dependency needed for this one
/// shape check) — unpadded alphabet per RFC 4648 §5.
fn decode_base64url(s: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }

    let bytes = s.as_bytes();
    // Decoded length estimate (3 output bytes per 4 base64 chars); exact
    // precision doesn't matter for a `Vec` capacity hint.
    #[allow(clippy::integer_division)]
    let capacity = bytes.len() * 3 / 4 + 3;
    let mut out = Vec::with_capacity(capacity);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for &b in bytes {
        let v = value(b)?;
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luhn_check_accepts_known_valid_card() {
        assert!(luhn_check("4111111111111111"));
    }

    #[test]
    fn luhn_check_rejects_tampered_card() {
        assert!(!luhn_check("4111111111111112"));
    }

    #[test]
    fn iban_checksum_valid_accepts_known_good_iban() {
        assert!(iban_checksum_valid("GB82WEST12345698765432"));
    }

    #[test]
    fn iban_checksum_valid_rejects_tampered_iban() {
        assert!(!iban_checksum_valid("GB82WEST12345698765431"));
    }

    #[test]
    fn ip_validators_accept_valid_addresses() {
        assert!(is_valid_ipv4("192.168.1.1"));
        assert!(!is_valid_ipv4("999.1.1.1"));
        assert!(is_valid_ipv6("2001:db8::1"));
        assert!(!is_valid_ipv6("not-an-ip"));
    }

    #[test]
    fn shannon_entropy_is_zero_for_repeated_char() {
        assert!((shannon_entropy("aaaaaaaa") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn shannon_entropy_is_higher_for_random_looking_string() {
        assert!(shannon_entropy("aB3$kL9!qZ") > shannon_entropy("aaaaaaaaaa"));
    }

    #[test]
    fn looks_like_jwt_accepts_structurally_valid_token() {
        // header {"alg":"HS256","typ":"JWT"} base64url-encoded, plus two
        // arbitrary non-empty segments.
        let header = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
        assert!(looks_like_jwt(&format!("{header}.payload.signature")));
    }

    #[test]
    fn looks_like_jwt_rejects_non_jwt_strings() {
        assert!(!looks_like_jwt("not.a.jwt.token"));
        assert!(!looks_like_jwt("just-one-segment"));
        assert!(!looks_like_jwt("a.b"));
    }
}
