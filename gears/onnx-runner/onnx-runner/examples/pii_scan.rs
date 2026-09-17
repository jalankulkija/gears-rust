//! What a consumer of this gear looks like — a miniature `onnx-scanner`.
//!
//! ```sh
//! export ORT_DYLIB_PATH="$(bash scripts/fetch-onnxruntime.sh --path)"
//! bash gears/onnx-runner/scripts/fetch-models.sh piiranha-v1
//!
//! cargo run --release -p cf-gears-onnx-runner --example pii_scan
//! ```
//!
//! `onnx-runner` runs tensors and tokenizes text. It has no idea what PII is,
//! what a BIO tag is, or which model family it is holding — that is the whole
//! point of its design, and it means every consumer has to do the four things
//! below. This example does them for a BIO token classifier, end to end and in
//! the order they actually happen:
//!
//! 1. **Load and verify** ([`stage_1_load`]) — configure models, then confirm
//!    through `list_models` that the graph is the one expected. A consumer that
//!    skips this discovers its mistake as a shape error on live traffic.
//! 2. **Tokenize** ([`stage_2_tokenize`]) — `encode` returns ids plus the byte
//!    `offsets` that make step 4 possible at all.
//! 3. **Run** ([`stage_3_run`]) — build `NamedTensor`s whose names match the
//!    graph's contract, call `run`, get logits back.
//! 4. **Decode** ([`stage_4_decode`]) — argmax, merge BIO tags into entities,
//!    and map token indices back to byte ranges in the original text.
//!
//! Steps 2-4 are consumer code. Only step 3's `run` and step 2's `encode`
//! cross the gear boundary; the label map and the input-tensor assembly come
//! from the gear and the SDK respectively, so a consumer writes neither.
//!
//! **Not** shown, because they are policy rather than plumbing: batching
//! several texts into one call, redaction, confidence thresholds, and the
//! GLiNER-family prompt assembly that would use `encode_fragment`.

use std::path::PathBuf;
use std::sync::Arc;

use onnx_runner::config::{ModelConfig, OutputDtype};
use onnx_runner::domain::metrics::NoopMetrics;
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner::{OnnxRunnerClientV1, TensorData, TokenEncoding, token_classifier_inputs};
use onnx_runner_sdk::ModelState;
use toolkit_security::SecurityContext;

/// The models this example can drive. Any BIO token classifier works, as long
/// as it is pinned in `scripts/fetch-models.sh` and publishes an `id2label` map
/// in `config.json`.
///
/// `token_type_ids` is the only shape difference between them: some BERT
/// exports declare it and some do not, which is precisely why a consumer reads
/// the contract from `list_models` instead of assuming one.
const MODELS: &[(&str, &str)] = &[
    ("piiranha-v1", "model_int8.onnx"),
    ("minilm-l6-pii", "model.onnx"),
    ("multilang-pii-ner", "model_int8.onnx"),
];

const SAMPLE: &str = "Please email Sarah Chen at sarah.chen@example.com or call \
                      +1 415 555 0142 before Friday.";

/// Which entry of [`MODELS`] to use — `PII_SCAN_MODEL=minilm-l6-pii`.
fn selected_model() -> Result<(&'static str, &'static str), Box<dyn std::error::Error>> {
    let want = std::env::var("PII_SCAN_MODEL").unwrap_or_else(|_| MODELS[0].0.to_owned());
    MODELS
        .iter()
        .find(|(id, _)| *id == want)
        .copied()
        .ok_or_else(|| {
            let known: Vec<_> = MODELS.iter().map(|(id, _)| *id).collect();
            format!("unknown model '{want}'; this example knows {known:?}").into()
        })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `ort` 2.0.0-rc.12 *hangs* rather than erroring when it cannot load a
    // runtime, so check before doing anything that would look like a stall.
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        return Err(
            "ORT_DYLIB_PATH is unset. Without it `ort` hangs instead of \
                    failing; see libs/ort-bootstrap.\n  export ORT_DYLIB_PATH=\
                    \"$(bash scripts/fetch-onnxruntime.sh --path)\""
                .into(),
        );
    }

    let ctx = SecurityContext::builder()
        .subject_id(uuid::Uuid::new_v4())
        .subject_tenant_id(uuid::Uuid::new_v4())
        .build()?;

    let (model_id, model_file) = selected_model()?;
    let (client, inputs_wanted, labels) = stage_1_load(&ctx, model_id, model_file).await?;

    let encoding = stage_2_tokenize(&client, &ctx, model_id, SAMPLE).await?;
    let logits = stage_3_run(&client, &ctx, model_id, &encoding, &inputs_wanted).await?;
    let spans = stage_4_decode(&logits, &encoding, &labels)?;

    println!("\ntext: {SAMPLE}\n");
    if spans.is_empty() {
        println!("no PII found");
    }
    for span in &spans {
        println!(
            "  {:<18} bytes {:>3}..{:<3}  \"{}\"",
            span.label,
            span.start,
            span.end,
            &SAMPLE[span.start..span.end],
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 1. Load and verify
// ---------------------------------------------------------------------------

/// Configures the model, loads it, and **checks the contract before serving**.
///
/// In a real gear the `ModelConfig` comes from operator YAML and the client is
/// resolved through `ClientHub`; it is built by hand here so the example is
/// self-contained. What is not a shortcut is the `list_models` check at the
/// end — that is exactly what a consumer should do at startup.
async fn stage_1_load(
    ctx: &SecurityContext,
    model_id: &str,
    model_file: &str,
) -> Result<(OnnxRunnerLocalClient, Vec<String>, Vec<String>), Box<dyn std::error::Error>> {
    let model_dir = models_dir().join(model_id);
    if !model_dir.join(model_file).exists() {
        return Err(format!(
            "{model_id} is not present. Fetch it first:\n  bash \
             gears/onnx-runner/scripts/fetch-models.sh {model_id}"
        )
        .into());
    }

    // Read the graph's real input names rather than assuming them: these
    // models differ on `token_type_ids`. A production consumer would put this
    // list in configuration and treat a mismatch as a startup failure; here it
    // is discovered so one example can drive several model families.
    let declared = declared_inputs(&model_dir.join(model_file))?;

    let config = ModelConfig {
        model_id: model_id.to_owned(),
        model_dir,
        model_file: model_file.to_owned(),
        tokenizer_file: "tokenizer.json".to_owned(),

        // An operator pins the digest and it is verified on every load. Skipped
        // here only because this example is not the place to hardcode one —
        // `fetch-models.sh` has already verified this file against its pin.
        sha256: String::new(),
        skip_checksum: true,

        // The contract. Naming it in configuration rather than reading it off
        // the graph is what makes a swapped model a startup error instead of a
        // silently different answer.
        expected_inputs: declared.clone(),
        expected_outputs: vec!["logits".to_owned()],
        output_dtypes: [("logits".to_owned(), OutputDtype::F32)]
            .into_iter()
            .collect(),

        replicas: 1,
        intra_threads: 4,
        ..Default::default()
    };

    // `require_all_models: true` — fail startup rather than come up degraded.
    // A scanner that starts without its model is worse than one that does not
    // start: it answers "no PII found" to everything.
    let models = load_models(
        vec![config],
        true,
        Arc::new(NoopMetrics),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;

    let client = OnnxRunnerLocalClient::new(models);

    // The startup check. `list_models` reports *every configured* model,
    // including ones that failed to load, which is why an absent entry and an
    // unavailable one mean different things.
    let info = client
        .list_models(ctx)
        .await?
        .into_iter()
        .find(|m| m.model_id == model_id)
        .ok_or("model id not configured: a typo, not an outage")?;

    let loaded = info.loaded.as_ref().ok_or_else(|| match info.state {
        ModelState::Unavailable { reason } => {
            format!("{model_id} is configured but unavailable: {reason}")
        }
        _ => format!("{model_id} is not ready yet"),
    })?;

    println!("model    {model_id}  ({} replicas)", loaded.replicas);
    println!("inputs   {}", loaded.input_names.join(", "));
    println!("outputs  {}", loaded.output_names.join(", "));

    // The label map travels with the model, served from `list_models` like any
    // other metadata in the model directory. `None` is legitimate — a zero-shot
    // span model takes its labels per request — but this decoder cannot work
    // without one, so it is this consumer that says so.
    let labels = loaded
        .labels
        .clone()
        .ok_or_else(|| format!("{model_id} publishes no id2label map; a BIO decoder needs one"))?;
    println!("labels   {}", labels.len());

    // Belt and braces over the configured contract: the tensors this code
    // *builds* must exist on the graph this code is *talking to*.
    for required in ["input_ids", "attention_mask"] {
        if !loaded.input_names.iter().any(|n| n == required) {
            return Err(
                format!("{model_id} has no `{required}` input — wrong model family").into(),
            );
        }
    }

    Ok((client, loaded.input_names.clone(), labels))
}

// ---------------------------------------------------------------------------
// 2. Tokenize
// ---------------------------------------------------------------------------

/// Text to token ids, via the tokenizer that ships beside the model.
///
/// `encode` — not `encode_fragment` — because this is a whole document: it should
/// get the tokenizer's special tokens, and it needs the byte offsets that
/// [`stage_4_decode`] uses to get back to the original string. `encode_fragment`
/// exists for the opposite case, tokenizing a fragment to splice into a larger
/// sequence, which is what a zero-shot span model needs for its label prompt.
async fn stage_2_tokenize(
    client: &OnnxRunnerLocalClient,
    ctx: &SecurityContext,
    model_id: &str,
    text: &str,
) -> Result<TokenEncoding, Box<dyn std::error::Error>> {
    let encoding = client.encode(ctx, model_id, text).await?;
    println!("\ntokens   {}", encoding.ids.len());
    Ok(encoding)
}

// ---------------------------------------------------------------------------
// 3. Run
// ---------------------------------------------------------------------------

/// Builds the input tensors and runs the graph.
///
/// Shape is `[batch, sequence]` and this passes a batch of one. The dimension
/// is dynamic in every model here, so coalescing several texts into one call is
/// possible — it is a consumer's decision, and it is not free: the texts must
/// be padded to a common length and the results split apart again.
async fn stage_3_run(
    client: &OnnxRunnerLocalClient,
    ctx: &SecurityContext,
    model_id: &str,
    encoding: &TokenEncoding,
    inputs_wanted: &[String],
) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    // The SDK helper builds exactly the inputs the graph declares — including
    // `token_type_ids` when, and only when, it wants one. Assembling these by
    // hand is a dozen lines and the commonest way to get a shape wrong.
    let inputs = token_classifier_inputs(model_id, encoding, inputs_wanted)?;

    let outputs = client.run(ctx, model_id, inputs).await?;

    let logits = outputs
        .into_iter()
        .find(|t| t.name == "logits")
        .ok_or("model returned no `logits` tensor")?;

    match logits.data {
        TensorData::F32(values) => Ok(values),
        other => Err(format!("expected F32 logits, got {other:?}").into()),
    }
}

// ---------------------------------------------------------------------------
// 4. Decode
// ---------------------------------------------------------------------------

/// One detected entity, in **byte offsets into the original text**.
struct Span {
    label: String,
    start: usize,
    end: usize,
}

/// Logits to spans: argmax per token, merge adjacent tokens of the same entity,
/// then translate token positions into byte ranges.
///
/// The last step is the one that matters and the one the gear cannot do for
/// you. The model works in token space; a redactor needs byte ranges in the
/// caller's own payload, and `encoding.offsets` is the only bridge between
/// them. Sub-word tokens make this non-negotiable: `sarah.chen@example.com` is
/// several tokens whose text cannot be reassembled by concatenation.
fn stage_4_decode(
    logits: &[f32],
    encoding: &TokenEncoding,
    labels: &[String],
) -> Result<Vec<Span>, Box<dyn std::error::Error>> {
    let n_tokens = encoding.ids.len();
    let n_classes = logits
        .len()
        .checked_div(n_tokens)
        .filter(|c| *c > 0)
        .ok_or("logits do not divide evenly by token count")?;

    let mut spans: Vec<Span> = Vec::new();

    for (i, offset) in encoding.offsets.iter().enumerate().take(n_tokens) {
        // `[CLS]`, `[SEP]` and padding have no place in the source text; their
        // offsets are degenerate and a span over them would be meaningless.
        if encoding
            .special_tokens_mask
            .get(i)
            .copied()
            .unwrap_or(false)
        {
            continue;
        }

        let row = &logits[i * n_classes..(i + 1) * n_classes];
        let best = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(idx, _)| idx);

        let Some(tag) = labels.get(best) else {
            continue;
        };

        // `PII_SCAN_DEBUG=1` dumps the per-token decision. Worth having in an
        // example: when a consumer sees "no PII found", the first question is
        // whether the model predicted nothing or the decoder dropped it, and
        // these two are indistinguishable from the outside.
        if std::env::var_os("PII_SCAN_DEBUG").is_some() {
            eprintln!(
                "  [{i:>2}] {:>3}..{:<3} {:<16} {tag}",
                offset.0,
                offset.1,
                format!("\"{}\"", &SAMPLE[offset.0 as usize..offset.1 as usize]),
            );
        }

        if tag == "O" {
            continue;
        }

        // Strip the BIO prefix and merge by entity type rather than trusting
        // `B-`/`I-`: some published models tag every token `I-`, so a decoder
        // that relies on `B-` to open a span finds nothing at all.
        let entity = tag.split_once('-').map_or(tag.as_str(), |(_, rest)| rest);
        let (start, end) = (offset.0 as usize, offset.1 as usize);

        match spans.last_mut() {
            // Adjacent in byte space and the same type: one entity, not two.
            // Whitespace between tokens is bridged so "Sarah Chen" stays whole.
            Some(prev) if prev.label == entity && start.saturating_sub(prev.end) <= 1 => {
                prev.end = end;
            }
            _ => spans.push(Span {
                label: entity.to_owned(),
                start,
                end,
            }),
        }
    }

    Ok(spans)
}

// ---------------------------------------------------------------------------
// Support
// ---------------------------------------------------------------------------

fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../models")
}

/// The graph's declared input names, read straight off the file.
///
/// A production consumer would not do this — it would put the contract in
/// configuration so that a swapped model fails loudly. This example reads it
/// only so that one binary can drive several model families.
fn declared_inputs(path: &std::path::Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use ort::session::{Session, builder::GraphOptimizationLevel};
    let probe = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level1)?
        .commit_from_file(path)?;
    Ok(probe.inputs().iter().map(|i| i.name().to_owned()).collect())
}
