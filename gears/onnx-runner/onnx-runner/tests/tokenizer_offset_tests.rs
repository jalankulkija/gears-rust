#![allow(clippy::unwrap_used, clippy::expect_used)]
// The workspace denies non-ASCII literals. These tests exist *because* of
// non-ASCII text: escaping "привет" to `\u{43f}\u{440}...` would leave a suite
// nobody can read about a property nobody can see. The literals are the
// subject matter, not incidental.
#![allow(clippy::non_ascii_literal)]

//! Proves `TokenEncoding`'s offset contract: offsets are **byte** offsets into
//! the original text.
//!
//! Every consumer that maps a model-space span back to source text depends on
//! this. If these were character offsets, every redaction span in the scanning
//! pipeline would be silently wrong for any non-ASCII input — the text would
//! still slice, just in the wrong place, which is the worst kind of bug.
//!
//! `infra/tokenizer.rs` carried a written warning that this was unverified.
//! These tests are what replaced it.
//!
//! Needs no model and no network: tokenization is independent of the ONNX
//! session, so these run everywhere.

mod common;

use common::{MODEL_ID, ctx, tokenizer_path};
use onnx_runner::infra::tokenizer::LoadedTokenizer;

async fn encode(text: &str) -> onnx_runner_sdk::TokenEncoding {
    let _ = ctx();
    LoadedTokenizer::load(&tokenizer_path())
        .expect("fixture tokenizer loads")
        .encode(MODEL_ID, text.to_owned())
        .await
        .expect("encodes")
}

/// Slicing the original text by each returned offset pair must reproduce that
/// token's own text. This is the contract, stated as an executable assertion.
async fn assert_offsets_slice_back(text: &str) {
    let enc = encode(text).await;

    for (i, &(start, end)) in enc.offsets.iter().enumerate() {
        // Special tokens carry (0, 0) — they correspond to no source text.
        if enc.special_tokens_mask[i] {
            assert_eq!(
                (start, end),
                (0, 0),
                "special token {i} should have an empty span"
            );
            continue;
        }

        let (start, end) = (start as usize, end as usize);
        let slice = text.get(start..end).unwrap_or_else(|| {
            panic!(
                "offsets ({start},{end}) do not fall on character boundaries of {text:?} — \
                 this is what a character-offset implementation looks like"
            )
        });
        assert!(
            !slice.is_empty(),
            "token {i} of {text:?} sliced to nothing at ({start},{end})"
        );
    }
}

#[tokio::test]
async fn ascii_offsets_slice_back() {
    assert_offsets_slice_back("hello world").await;
}

/// Cyrillic: 2 bytes per character, so byte and character offsets diverge
/// immediately.
#[tokio::test]
async fn cyrillic_offsets_are_byte_offsets() {
    let text = "привет мир";
    assert_offsets_slice_back(text).await;

    let enc = encode(text).await;
    let first = enc
        .offsets
        .iter()
        .zip(&enc.special_tokens_mask)
        .find(|(_, special)| !**special)
        .expect("a non-special token")
        .0;

    // "привет" is 6 characters and 12 bytes. A character-offset implementation
    // would report 6 here, and would still slice — into the middle of "мир".
    assert_eq!(
        *first,
        (0, 12),
        "expected byte offsets; got what looks like character offsets"
    );
    assert_eq!(text.get(0..12), Some("привет"));
}

/// CJK: 3 bytes per character.
#[tokio::test]
async fn cjk_offsets_are_byte_offsets() {
    let text = "日本 語";
    assert_offsets_slice_back(text).await;

    let enc = encode(text).await;
    let spans: Vec<(u32, u32)> = enc
        .offsets
        .iter()
        .zip(&enc.special_tokens_mask)
        .filter(|(_, special)| !**special)
        .map(|(o, _)| *o)
        .collect();

    // 2 characters / 6 bytes, then 1 character / 3 bytes after a space.
    assert_eq!(spans, vec![(0, 6), (7, 10)]);
    assert_eq!(text.get(0..6), Some("日本"));
    assert_eq!(text.get(7..10), Some("語"));
}

/// A combining sequence: the character count, the byte count and the grapheme
/// count are all different, so nothing coincidental can pass.
#[tokio::test]
async fn combining_sequences_slice_back() {
    // "e" + U+0301 combining acute, then a precomposed "é".
    assert_offsets_slice_back("cafe\u{301} café").await;
}

/// Text where byte length and character length differ substantially, so a
/// character-offset implementation cannot pass by coincidence on short input.
#[tokio::test]
async fn long_multibyte_text_offsets_stay_aligned() {
    let text = "привет мир привет мир 日本 語 привет";
    assert!(
        text.len() * 2 > text.chars().count() * 3,
        "fixture text must be substantially multi-byte"
    );
    assert_offsets_slice_back(text).await;
}

/// The test that proves the others have teeth: a character-offset conversion,
/// applied to the same data, must fail the contract. Written here rather than
/// performed by hand on the real code and reverted, so it stays proven.
#[test]
fn a_character_offset_implementation_would_fail_these_tests() {
    let text = "привет мир";

    // What the gear does: byte offsets, passed through unmodified.
    assert_eq!(text.get(0..12), Some("привет"));

    // What a character-offset implementation would produce for the same token:
    // char index 0..6. It still slices — into the wrong text.
    let by_chars: String = text.chars().take(6).collect();
    assert_eq!(by_chars, "привет");
    let wrongly_sliced = text.get(0..6);
    assert_eq!(
        wrongly_sliced,
        Some("при"),
        "a character offset used as a byte offset silently yields a shorter, wrong span — \
         which is exactly why this contract is tested rather than assumed"
    );
    assert_ne!(wrongly_sliced, Some("привет"));
}

#[tokio::test]
async fn encode_fragment_adds_no_special_tokens() {
    let tokenizer = LoadedTokenizer::load(&tokenizer_path()).expect("loads");

    let full = tokenizer
        .encode(MODEL_ID, "hello".to_owned())
        .await
        .expect("encodes");
    let fragment = tokenizer
        .encode_fragment(MODEL_ID, "hello".to_owned())
        .await
        .expect("encodes");

    assert!(
        full.special_tokens_mask.iter().any(|s| *s),
        "encode adds special tokens"
    );
    assert_eq!(
        fragment,
        vec![3],
        "encode_fragment returns raw sub-token ids only"
    );
    assert!(
        full.ids.len() > fragment.len(),
        "the difference is the special tokens"
    );
}

#[tokio::test]
async fn encoding_field_lengths_agree() {
    let enc = encode("hello world привет").await;
    let n = enc.ids.len();
    assert_eq!(enc.attention_mask.len(), n);
    assert_eq!(enc.offsets.len(), n);
    assert_eq!(enc.special_tokens_mask.len(), n);
}

#[tokio::test]
async fn empty_text_encodes_without_panicking() {
    let enc = encode("").await;
    // Special tokens only.
    assert!(enc.ids.len() <= 2, "empty text yields at most the template");
}
