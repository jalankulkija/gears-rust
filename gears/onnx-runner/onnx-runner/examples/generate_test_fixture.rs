//! Regenerates the committed ONNX test fixture.
//!
//! ```sh
//! cargo run -p cf-gears-onnx-runner --features fixture-gen \
//!   --example generate_test_fixture
//! ```
//!
//! The graph itself is defined in `src/fixture.rs`, which is also what the
//! benchmark uses to build its synthetic model ladder. This binary only writes
//! it out and reports the digest, so there is exactly one definition of what
//! the fixture *is*.
//!
//! See `tests/fixtures/README.md` for why the artifact is committed.

use std::path::PathBuf;

use onnx_runner::fixture;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");

    for (file, bytes, digest_const) in [
        ("tiny.onnx", fixture::tiny_graph(), "FIXTURE_SHA256"),
        ("other.onnx", fixture::other_graph(), "OTHER_FIXTURE_SHA256"),
    ] {
        let out = dir.join(file);
        std::fs::write(&out, &bytes)?;
        println!("wrote {} ({} bytes)", out.display(), bytes.len());
        println!("  sha256 = {}  -> {digest_const}", hex_sha256(&bytes));
    }

    println!("\nIf a digest changed, update the matching constant in tests/common/mod.rs.");
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
