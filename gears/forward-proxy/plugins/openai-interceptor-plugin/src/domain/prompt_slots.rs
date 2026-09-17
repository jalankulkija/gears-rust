//! Pure JSON-walking logic for finding prompt text inside a `ChatGPT` web-app
//! or `OpenAI` API JSON request body. Returns JSON Pointer paths so a caller
//! can write a redacted replacement back into the same `serde_json::Value`
//! via `Value::pointer_mut`.
//!
//! Known limitation: JSON Pointer segments
//! aren't escaped (`~`/`/`), which is safe here because every key this
//! module addresses (`messages`, `content`, `parts`, `input`, `text`) is a
//! fixed literal with no special characters.

use serde_json::Value;

/// One prompt-text location inside a decoded JSON payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTextRef {
    /// JSON Pointer (RFC 6901) to the string value, usable with
    /// `Value::pointer_mut` to write a redacted replacement in place.
    pub pointer: String,
    pub text: String,
}

fn is_user_role(message: &Value) -> bool {
    let role = message.get("role").and_then(Value::as_str).or_else(|| {
        message
            .get("author")
            .and_then(|a| a.get("role"))
            .and_then(Value::as_str)
    });
    role == Some("user")
}

/// Finds every prompt-text slot under `container[key]`, covering the
/// various shapes `ChatGPT`'s web app and the `OpenAI` API use for message/
/// input content:
/// - a plain string: `{"content": "..."}`
/// - `ChatGPT` web app: `{"content": {"content_type": "text", "parts": ["..."]}}`
/// - `OpenAI` Responses API: `{"input": [{"type": "input_text", "text": "..."}]}`
fn container_text_slots(
    container: &Value,
    key: &str,
    container_pointer: &str,
) -> Vec<PromptTextRef> {
    let mut refs = Vec::new();
    let Some(value) = container.get(key) else {
        return refs;
    };
    let key_pointer = format!("{container_pointer}/{key}");

    match value {
        Value::String(text) => refs.push(PromptTextRef {
            pointer: key_pointer,
            text: text.clone(),
        }),
        Value::Object(_) => {
            if let Some(Value::Array(parts)) = value.get("parts") {
                for (i, part) in parts.iter().enumerate() {
                    if let Value::String(text) = part {
                        refs.push(PromptTextRef {
                            pointer: format!("{key_pointer}/parts/{i}"),
                            text: text.clone(),
                        });
                    }
                }
            }
        }
        Value::Array(blocks) => {
            for (i, block) in blocks.iter().enumerate() {
                if let Some(Value::String(text)) = block.get("text") {
                    refs.push(PromptTextRef {
                        pointer: format!("{key_pointer}/{i}/text"),
                        text: text.clone(),
                    });
                }
            }
        }
        _ => {}
    }

    refs
}

/// Extracts every prompt-text reference from a decoded request payload:
/// user-role message content (`messages[]`), and top-level `input` (`OpenAI`
/// Responses API).
#[must_use]
pub fn extract_prompt_refs(payload: &Value) -> Vec<PromptTextRef> {
    let mut refs = Vec::new();

    if let Some(Value::Array(messages)) = payload.get("messages") {
        for (i, message) in messages.iter().enumerate() {
            if message.is_object() && is_user_role(message) {
                refs.extend(container_text_slots(
                    message,
                    "content",
                    &format!("/messages/{i}"),
                ));
            }
        }
    }

    if payload.get("input").is_some() {
        refs.extend(container_text_slots(payload, "input", ""));
    }

    refs
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn extracts_plain_string_content_from_user_message() {
        let payload = json!({
            "messages": [
                {"role": "user", "content": "hello there"}
            ]
        });

        let refs = extract_prompt_refs(&payload);

        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "hello there");
        assert_eq!(refs[0].pointer, "/messages/0/content");
    }

    #[test]
    fn ignores_non_user_messages() {
        let payload = json!({
            "messages": [
                {"role": "assistant", "content": "a reply"},
                {"role": "system", "content": "a system prompt"}
            ]
        });

        assert!(extract_prompt_refs(&payload).is_empty());
    }

    #[test]
    fn extracts_chatgpt_web_app_parts_shape() {
        let payload = json!({
            "messages": [
                {
                    "role": "user",
                    "content": {"content_type": "text", "parts": ["part one", "part two"]}
                }
            ]
        });

        let refs = extract_prompt_refs(&payload);

        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].text, "part one");
        assert_eq!(refs[0].pointer, "/messages/0/content/parts/0");
        assert_eq!(refs[1].text, "part two");
        assert_eq!(refs[1].pointer, "/messages/0/content/parts/1");
    }

    #[test]
    fn extracts_top_level_input_responses_api_shape() {
        let payload = json!({
            "input": [
                {"type": "input_text", "text": "responses api prompt"}
            ]
        });

        let refs = extract_prompt_refs(&payload);

        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "responses api prompt");
        assert_eq!(refs[0].pointer, "/input/0/text");
    }

    #[test]
    fn handles_author_role_shape() {
        let payload = json!({
            "messages": [
                {"author": {"role": "user"}, "content": "author-shaped message"}
            ]
        });

        let refs = extract_prompt_refs(&payload);

        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].text, "author-shaped message");
    }

    #[test]
    fn pointer_can_be_used_to_write_redaction_back() {
        let mut payload = json!({
            "messages": [
                {"role": "user", "content": "sensitive text"}
            ]
        });

        let refs = extract_prompt_refs(&payload);
        assert_eq!(refs.len(), 1);

        if let Some(slot) = payload.pointer_mut(&refs[0].pointer) {
            *slot = Value::String("[REDACTED]".to_owned());
        }

        assert_eq!(payload["messages"][0]["content"], "[REDACTED]");
    }

    #[test]
    fn empty_payload_yields_no_refs() {
        assert!(extract_prompt_refs(&json!({})).is_empty());
    }
}
