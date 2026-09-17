//! Pure JSON-walking + `data:` URI decoding logic for finding inline
//! base64-encoded attachments inside a request body.
//!
//! This module owns the inline-base64 `input_file.file_data` transport. The
//! other two upload transports belong to the attachment-scanning feature
//! rather than here: `POST /v1/files` multipart needs a multipart body
//! parser, and the `ChatGPT` web app's SAS-signed blob-storage `PUT` needs
//! cross-request `file_id` correlation through `on_response`, which
//! `docs/DESIGN.md`'s "Response handling" decision keeps non-blocking.
//! Extending coverage extends this module and `domain::service`, not the
//! core `forward-proxy` gear.
//!
//! Image attachments are intentionally skipped: no OCR backend exists in
//! this platform yet (see `gears/file-parser/docs/PRD.md`), so they pass
//! through unscanned rather than being reported as inspected.

use base64::Engine as _;
use serde_json::Value;

/// One inline attachment found in a request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineAttachment {
    pub filename: Option<String>,
    pub mime_type: String,
    pub data: Vec<u8>,
}

impl InlineAttachment {
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }
}

/// Parses a `data:<mime-type>;base64,<payload>` URI. Returns `None` for
/// anything else (including `image_url` data URIs, which this module
/// deliberately never calls this on — see module docs).
fn parse_data_uri(uri: &str) -> Option<(String, Vec<u8>)> {
    let rest = uri.strip_prefix("data:")?;
    let (mime, b64) = rest.split_once(";base64,")?;
    let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    Some((mime.to_owned(), data))
}

fn attachment_from_input_file_block(block: &Value) -> Option<InlineAttachment> {
    if block.get("type").and_then(Value::as_str) != Some("input_file") {
        return None;
    }
    let file_data = block.get("file_data").and_then(Value::as_str)?;
    let (mime_type, data) = parse_data_uri(file_data)?;
    let filename = block
        .get("filename")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(InlineAttachment {
        filename,
        mime_type,
        data,
    })
}

fn scan_blocks_for_attachments(blocks: &[Value]) -> Vec<InlineAttachment> {
    blocks
        .iter()
        .filter_map(attachment_from_input_file_block)
        .collect()
}

/// Extracts every inline base64 attachment from a request payload:
/// `messages[].content[]` blocks and top-level `input[]` blocks
/// (`OpenAI` Responses API shape) with `type: "input_file"`.
#[must_use]
pub fn extract_inline_attachments(payload: &Value) -> Vec<InlineAttachment> {
    let mut attachments = Vec::new();

    if let Some(Value::Array(messages)) = payload.get("messages") {
        for message in messages {
            if let Some(Value::Array(blocks)) = message.get("content") {
                attachments.extend(scan_blocks_for_attachments(blocks));
            }
        }
    }

    if let Some(Value::Array(blocks)) = payload.get("input") {
        attachments.extend(scan_blocks_for_attachments(blocks));
    }

    attachments
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;

    fn data_uri(mime: &str, content: &[u8]) -> String {
        format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(content)
        )
    }

    #[test]
    fn parses_valid_data_uri() {
        let uri = data_uri("application/pdf", b"pdf bytes");
        let (mime, data) = parse_data_uri(&uri).expect("should parse");
        assert_eq!(mime, "application/pdf");
        assert_eq!(data, b"pdf bytes");
    }

    #[test]
    fn rejects_non_data_uri() {
        assert!(parse_data_uri("https://example.com/file.pdf").is_none());
    }

    #[test]
    fn rejects_non_base64_data_uri() {
        assert!(parse_data_uri("data:text/plain,not-base64-encoded").is_none());
    }

    #[test]
    fn extracts_attachment_from_top_level_input() {
        let uri = data_uri("application/pdf", b"hello pdf");
        let payload = json!({
            "input": [
                {"type": "input_text", "text": "please review"},
                {"type": "input_file", "filename": "doc.pdf", "file_data": uri}
            ]
        });

        let attachments = extract_inline_attachments(&payload);

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename.as_deref(), Some("doc.pdf"));
        assert_eq!(attachments[0].mime_type, "application/pdf");
        assert_eq!(attachments[0].data, b"hello pdf");
    }

    #[test]
    fn extracts_attachment_from_message_content() {
        let uri = data_uri("text/plain", b"plain text content");
        let payload = json!({
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "input_file", "filename": "notes.txt", "file_data": uri}
                    ]
                }
            ]
        });

        let attachments = extract_inline_attachments(&payload);

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename.as_deref(), Some("notes.txt"));
    }

    #[test]
    fn is_image_detects_image_mime_types() {
        let image = InlineAttachment {
            filename: None,
            mime_type: "image/png".to_owned(),
            data: vec![],
        };
        let doc = InlineAttachment {
            filename: None,
            mime_type: "application/pdf".to_owned(),
            data: vec![],
        };
        assert!(image.is_image());
        assert!(!doc.is_image());
    }

    #[test]
    fn ignores_non_input_file_blocks() {
        let payload = json!({
            "input": [
                {"type": "input_image", "image_url": "data:image/png;base64,AAAA"}
            ]
        });
        assert!(extract_inline_attachments(&payload).is_empty());
    }

    #[test]
    fn empty_payload_yields_no_attachments() {
        assert!(extract_inline_attachments(&json!({})).is_empty());
    }
}
