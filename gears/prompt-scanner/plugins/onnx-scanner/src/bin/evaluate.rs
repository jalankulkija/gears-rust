//! Scores a PII backend against a labelled corpus.
//!
//! ```sh
//! export ORT_DYLIB_PATH="$(bash scripts/fetch-onnxruntime.sh --path)"
//!
//! cargo run --release -p cf-gears-onnx-scanner --features eval --bin evaluate -- \
//!   --model minilm-l6-pii \
//!   --corpus gears/prompt-scanner/plugins/onnx-scanner/eval-corpora/smoke.jsonl \
//!   --taxonomy 'email=EMAIL,first_name=PERSON,last_name=PERSON,phone_number=PHONE'
//! ```
//!
//! A thin shim: everything is in `onnx_scanner::eval`, so the plugin's
//! `domain` module stays private.

use std::path::PathBuf;

use onnx_scanner::eval::runner::{BackendChoice, Job, load_taxonomy, parse_taxonomy, run};
use onnx_scanner::eval::scoring::TaxonomyMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut model = "minilm-l6-pii".to_owned();
    let mut model_file: Option<String> = None;
    let mut corpus: Option<PathBuf> = None;
    let mut taxonomy = String::new();
    let mut taxonomy_file: Option<PathBuf> = None;
    let mut gold_taxonomy_file: Option<PathBuf> = None;
    let mut redact = false;
    let mut gliner2 = false;
    let mut gliner2_labels: Option<String> = None;
    let mut threshold = 0.5_f32;
    let mut beta = 2.0_f64;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--model" => model = value()?,
            "--model-file" => model_file = Some(value()?),
            "--corpus" => corpus = Some(PathBuf::from(value()?)),
            "--taxonomy" => taxonomy = value()?,
            "--taxonomy-file" => taxonomy_file = Some(PathBuf::from(value()?)),
            "--gold-taxonomy-file" => gold_taxonomy_file = Some(PathBuf::from(value()?)),
            "--redact" => redact = true,
            "--gliner2" => gliner2 = true,
            "--labels" => gliner2_labels = Some(value()?),
            "--threshold" => threshold = value()?.parse()?,
            "--beta" => beta = value()?.parse()?,
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            other => return Err(format!("unknown option {other}; try --help").into()),
        }
    }

    let corpus = corpus.ok_or("--corpus is required")?;
    let models_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../onnx-runner/models")
        .join(&model);

    // Default per model, because the export file name is repo-specific.
    let model_file = model_file.unwrap_or_else(|| {
        match model.as_str() {
            "piiranha-v1" | "multilang-pii-ner" => "model_int8.onnx",
            "kiji-pii" => "model_quantized.onnx",
            "gliner2-pii" => "model.quantized.onnx",
            _ => "model.onnx",
        }
        .to_owned()
    });

    run(&Job {
        model_id: model,
        model_dir: models_dir,
        model_file,
        corpus,
        taxonomy: match taxonomy_file {
            Some(path) => load_taxonomy(&path)?,
            None => parse_taxonomy(&taxonomy)?,
        },
        redact,
        gold_taxonomy: match gold_taxonomy_file {
            Some(path) => load_taxonomy(&path)?,
            None => TaxonomyMap::default(),
        },
        beta,
        backend: if gliner2 {
            BackendChoice::Gliner2 {
                labels: gliner2_labels
                    .as_deref()
                    .unwrap_or(DEFAULT_GLINER2_LABELS)
                    .split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect(),
                threshold,
            }
        } else {
            BackendChoice::Bio
        },
    })
    .await
}

/// Chosen to line up with the gold scope this comparison scores in, so the
/// zero-shot model is asked about the same categories the others are.
const DEFAULT_GLINER2_LABELS: &str = "person,email,phone_number,address,date_of_birth,\
national_id_number,passport_number,drivers_license_number,card_number,bank_account,\
ip_address,username,organization";

const HELP: &str = "\
evaluate - score a PII backend against a labelled corpus

  --model <id>          model directory under gears/onnx-runner/models (default minilm-l6-pii)
  --model-file <name>   ONNX export to load (defaults per model)
  --corpus <path>       JSONL corpus; see eval/corpus.rs for the format
  --taxonomy <spec>     model_label=CANONICAL,... mapping into the gold space
  --taxonomy-file <p>   the same, one per line, with # comments (preferred)
  --redact              read the corpus as REDACT benchmark JSON
  --gold-taxonomy-file  gold_type=CANONICAL map, coarsening REDACT's 51 types
  --gliner2             drive the zero-shot GLiNER2 backend instead of BIO
  --labels <a,b,...>    labels to condition GLiNER2 on (its cost scales with these)
  --threshold <f>       GLiNER2 decode threshold (default 0.5)
  --beta <n>            F-beta weight (default 2.0, recall-weighted)
";
