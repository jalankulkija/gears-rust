# regex-scanner

`prompt-scanner` scanner-backend plugin for deterministic detection: contact details,
financial identifiers and credential/secret shapes, matched by pattern and — where the
format allows it — confirmed by a checksum.

It runs before and alongside the ML backends rather than instead of them. A Luhn-valid
card number or an IBAN with a correct check digit is a fact, not a probability, and is
scored accordingly.

## Provenance

The confidence tiering in `src/domain/recognizer.rs` ("pattern < structural <
validated"), the validator split in `src/domain/validators.rs`, and the block-not-redact
posture for card numbers in `src/domain/patterns/financial.rs` follow **dlp-bench**, a
public Rust benchmark for local PII-detection models:

> <https://github.com/MikeFalcon77/dlp-bench> — MIT, Copyright (c) 2026 MikeY

Per-module doc comments name the specific upstream files. This notice is the attribution
MIT asks for; see that repository's `LICENSE` for the full terms.
