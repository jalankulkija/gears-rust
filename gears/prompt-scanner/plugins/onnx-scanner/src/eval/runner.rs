//! Drives a [`PiiBackend`] over a corpus and prints a markdown report.
//!
//! Lives in the library rather than the binary so `domain` stays private: the
//! evaluation needs to construct backends, and widening the plugin's public
//! API for a measurement tool would be the wrong trade.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use onnx_runner::config::{ModelConfig, OutputDtype};
use onnx_runner::domain::metrics::NoopMetrics;
use onnx_runner::gear::{OnnxRunnerLocalClient, load_models};
use onnx_runner_sdk::OnnxRunnerClientV1;
use toolkit_security::SecurityContext;

use super::corpus::{self, Corpus};
use super::scoring::{MatchMode, NegativeReport, Report, TaxonomyMap, score_document};
use crate::domain::backend::PiiBackend;
use crate::domain::bio_backend::BioTokenClassifierBackend;
use crate::domain::gliner2::Gliner2Backend;

/// Categories that cannot legitimately occur in public-domain literature, so a
/// flag on one is unambiguously a false positive.
///
/// Person and place names are *deliberately absent*: a novel is full of them
/// and a detector firing there is working as designed. Conflating the two is
/// how a negative-control number becomes meaningless — see
/// [`NegativeReport::unambiguous_flags`].
const UNAMBIGUOUS_DEFAULT: &[&str] = &[
    "email",
    "phone_number",
    "credit_card",
    "iban",
    "ssn",
    "account_number",
    "api_key",
    "ip_address",
    "passport_number",
    "driver_license",
];

pub struct Job {
    pub model_id: String,
    pub model_dir: PathBuf,
    pub model_file: String,
    pub corpus: PathBuf,
    /// Treat the corpus as REDACT benchmark JSON rather than our JSONL.
    pub redact: bool,
    /// Gold type to canonical, applied when loading REDACT. Gold types with no
    /// entry are excluded from the comparison, not scored as misses.
    pub gold_taxonomy: TaxonomyMap,
    /// Model category to canonical category. Empty means identity, which is
    /// only correct when the corpus was annotated in the model's own space.
    pub taxonomy: TaxonomyMap,
    pub beta: f64,
    /// Which backend to drive. The zero-shot one takes its labels at request
    /// time, so the comparison has to say what to ask for.
    pub backend: BackendChoice,
}

/// Which `PiiBackend` implementation to evaluate.
#[derive(Debug, Clone)]
pub enum BackendChoice {
    /// A BIO token classifier. Labels come from the model's own `id2label`.
    Bio,
    /// `GLiNER2`, zero-shot. Labels are supplied per request, which makes the
    /// label list part of the measurement: the span scorer is
    /// `spans x labels`, so it is also a throughput knob. Whatever is used
    /// here must be the count the performance table is taken at.
    Gliner2 { labels: Vec<String>, threshold: f32 },
}

/// # Errors
///
/// Propagates model-load, corpus and inference failures. A single document
/// that fails inference aborts the run rather than being skipped: a silently
/// shorter corpus produces a plausible score for a different measurement.
pub async fn run(job: &Job) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        return Err("ORT_DYLIB_PATH is unset; `ort` hangs rather than failing without it".into());
    }

    let corpus = if job.redact {
        let (labelled, stats) = super::redact::load(&job.corpus, &job.gold_taxonomy)?;
        println!(
            "corpus   REDACT: {} records, {} gold spans kept, {} misannotated upstream",
            stats.records, stats.entities_kept, stats.entities_misannotated
        );
        if !stats.excluded_types.is_empty() {
            let total: usize = stats.excluded_types.values().sum();
            println!(
                "         {total} spans in {} type(s) excluded as out of scope",
                stats.excluded_types.len()
            );
        }
        Corpus {
            labelled,
            negative_controls: Vec::new(),
        }
    } else {
        corpus::load(&job.corpus)?
    };
    println!(
        "         {} labelled, {} negative controls",
        corpus.labelled.len(),
        corpus.negative_controls.len()
    );

    let (client, labels) = load_client(job).await?;
    let client: Arc<dyn OnnxRunnerClientV1> = Arc::new(client);
    let backend: Box<dyn PiiBackend> = match &job.backend {
        BackendChoice::Bio => Box::new(BioTokenClassifierBackend::new(
            Arc::clone(&client),
            job.model_id.clone(),
            labels.ok_or_else(|| {
                format!(
                    "{} publishes no id2label map; needed by the BIO backend",
                    job.model_id
                )
            })?,
        )),
        BackendChoice::Gliner2 { labels, threshold } => {
            println!("labels   {} supplied at request time", labels.len());
            Box::new(
                Gliner2Backend::load(
                    Arc::clone(&client),
                    job.model_id.clone(),
                    labels.clone(),
                    *threshold,
                )
                .await?,
            )
        }
    };

    let ctx = SecurityContext::builder()
        .subject_id(uuid::Uuid::new_v4())
        .subject_tenant_id(uuid::Uuid::new_v4())
        .build()?;

    report_inexpressible(&corpus, &job.taxonomy);

    // Both strictnesses from one pass over the corpus: inference is by far the
    // expensive part, and scoring the same predictions twice costs nothing.
    // Corpus-wide, so a category is scored everywhere or nowhere.
    let in_scope = corpus.gold_categories();
    let mut exact = Report::default();
    let mut overlap = Report::default();
    for document in &corpus.labelled {
        let predictions = backend.detect_entities(&ctx, &document.text).await?;

        // How much of the document the model actually saw. Tokenizers truncate
        // at the model's context window, and REDACT documents routinely exceed
        // 512 tokens — without this, a model with a short window is
        // indistinguishable from one that simply misses things.
        let encoding = client.encode(&ctx, &job.model_id, &document.text).await?;
        let reachable = encoding.offsets.iter().map(|(_, end)| *end).max();

        for (mode, report) in [
            (MatchMode::Exact, &mut exact),
            (MatchMode::Overlap, &mut overlap),
        ] {
            score_document(
                document,
                &predictions,
                &job.taxonomy,
                mode,
                reachable,
                &in_scope,
                report,
            );
        }
    }

    let mut negative = NegativeReport::default();
    for document in &corpus.negative_controls {
        let predictions = backend.detect_entities(&ctx, &document.text).await?;
        negative.observe(&document.text, &predictions);
    }

    print_report(job, &exact, &overlap, &negative);
    Ok(())
}

async fn load_client(
    job: &Job,
) -> Result<(OnnxRunnerLocalClient, Option<Vec<String>>), Box<dyn std::error::Error>> {
    let path = job.model_dir.join(&job.model_file);
    if !path.exists() {
        return Err(format!("model not found at {}", path.display()).into());
    }

    // Asked of onnx-runner rather than probed here: the answer depends on the
    // optimization level the session will be built at, and that is its business.
    let declared = onnx_runner::declared_input_names(&path)?;
    let outputs: Vec<String> = match job.backend {
        BackendChoice::Bio => vec!["logits".to_owned()],
        // GLiNER2 emits span probabilities plus a global entity-count gate.
        BackendChoice::Gliner2 { .. } => {
            vec!["span_probs".to_owned(), "count_logits".to_owned()]
        }
    };
    let config = ModelConfig {
        model_id: job.model_id.clone(),
        model_dir: job.model_dir.clone(),
        model_file: job.model_file.clone(),
        sha256: String::new(),
        skip_checksum: true,
        expected_inputs: declared,
        expected_outputs: outputs.clone(),
        output_dtypes: outputs
            .iter()
            .map(|name| (name.clone(), OutputDtype::F32))
            .collect(),
        replicas: 1,
        intra_threads: 4,
        ..Default::default()
    };

    let models = load_models(
        vec![config],
        true,
        Arc::new(NoopMetrics),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    let client = OnnxRunnerLocalClient::new(models);

    // The label set comes from the model rather than from evaluation config,
    // so it cannot drift from the graph it describes.
    let ctx = SecurityContext::builder()
        .subject_id(uuid::Uuid::new_v4())
        .subject_tenant_id(uuid::Uuid::new_v4())
        .build()?;
    // `None` is legitimate: a zero-shot model declares no fixed label set.
    let labels = client
        .list_models(&ctx)
        .await?
        .into_iter()
        .find(|m| m.model_id == job.model_id)
        .and_then(|m| m.loaded)
        .and_then(|l| l.labels);

    match &labels {
        Some(l) => println!("model    {} ({} labels)", job.model_id, l.len()),
        None => println!("model    {} (zero-shot)", job.model_id),
    }
    Ok((client, labels))
}

/// Names the in-scope categories this model's map can never produce.
///
/// A note rather than an error, and the distinction matters. A gold type the
/// *gold* map excludes is out of scope and is dropped at load. A category that
/// is in scope but which this model has no label for is a genuine recall
/// failure, correctly scored as misses — but it should be visible, or a model
/// that simply lacks a category looks like one that keeps missing it.
fn report_inexpressible(corpus: &Corpus, taxonomy: &TaxonomyMap) {
    if taxonomy.is_empty() {
        return;
    }
    let produced: BTreeSet<&String> = taxonomy.values().collect();
    let missing: Vec<String> = corpus
        .gold_categories()
        .into_iter()
        .filter(|c| !produced.contains(c))
        .collect();

    if !missing.is_empty() {
        println!(
            "note     this model has no label for {} in-scope categor(ies), \
             scored as misses: {}",
            missing.len(),
            missing.join(", ")
        );
    }
}

fn print_report(job: &Job, exact: &Report, overlap: &Report, negative: &NegativeReport) {
    println!("\n## {} — detection quality\n", job.model_id);

    if !exact.is_complete() {
        let listed: Vec<String> = exact
            .unmapped
            .iter()
            .map(|(category, count)| format!("{category} ({count})"))
            .collect();
        println!(
            "> **Incomplete taxonomy map — these numbers are not usable.** \
             Unmapped predicted categories: {}\n",
            listed.join(", ")
        );
    }

    if !overlap.out_of_scope.is_empty() {
        let total: usize = overlap.out_of_scope.values().sum();
        let listed: Vec<String> = overlap
            .out_of_scope
            .iter()
            .map(|(category, count)| format!("{category} ({count})"))
            .collect();
        println!(
            "> {total} predictions excluded as out of scope for this comparison, \
             not counted against the model: {}\n",
            listed.join(", ")
        );
    }

    if overlap.unreachable_gold > 0 {
        println!(
            "> **{} gold spans ({:.1}%) lie beyond this model's context window** and \
             were truncated away before inference. They are excluded from recall; \
             a deployment would still miss them.\n",
            overlap.unreachable_gold,
            overlap.truncation_share() * 100.0
        );
    }

    println!(
        "| Match | Precision | Recall | F{:.0} | TP | FP | FN |",
        job.beta
    );
    println!("|---|---|---|---|---|---|---|");
    for (name, report) in [("exact", exact), ("overlap", overlap)] {
        let c = report.overall;
        println!(
            "| {name} | {:.3} | {:.3} | {:.3} | {} | {} | {} |",
            c.precision(),
            c.recall(),
            c.f_beta(job.beta),
            c.true_positives,
            c.false_positives,
            c.false_negatives
        );
    }

    if !overlap.per_category.is_empty() {
        println!("\n### Per category (overlap)\n");
        println!("| Category | Recall | Precision | TP | FP | FN |");
        println!("|---|---|---|---|---|---|");
        for (category, c) in &overlap.per_category {
            println!(
                "| {category} | {:.3} | {:.3} | {} | {} | {} |",
                c.recall(),
                c.precision(),
                c.true_positives,
                c.false_positives,
                c.false_negatives
            );
        }
    }

    if !overlap.per_language.is_empty() {
        println!("\n### Per language (overlap)\n");
        println!("| Language | Recall | F{:.0} |", job.beta);
        println!("|---|---|---|");
        for (lang, c) in &overlap.per_language {
            println!("| {lang} | {:.3} | {:.3} |", c.recall(), c.f_beta(job.beta));
        }
    }

    print_negative(negative);
}

fn print_negative(negative: &NegativeReport) {
    if negative.documents == 0 {
        return;
    }
    let unambiguous: BTreeSet<String> = UNAMBIGUOUS_DEFAULT
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let clean = negative.unambiguous_flags(&unambiguous);

    println!(
        "\n### Negative controls — {} documents, {} characters\n",
        negative.documents, negative.chars
    );
    println!(
        "**{clean} unambiguous false positives** ({:.2} per 10k chars). \
         {} flags in total ({:.2} per 10k chars).\n",
        negative.rate_per_10k(clean),
        negative.total_flags(),
        negative.rate_per_10k(negative.total_flags())
    );
    println!(
        "> Only the unambiguous figure is an error rate. Public-domain prose is \
         full of person and place names, so a name detector firing on it is \
         behaving correctly; an email address or a national identifier in a \
         novel is not.\n"
    );

    println!("| Category | Flags | Per 10k chars | Unambiguous |");
    println!("|---|---|---|---|");
    let mut rows: Vec<(&String, &usize)> = negative.flags.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    for (category, count) in rows {
        println!(
            "| {category} | {count} | {:.2} | {} |",
            negative.rate_per_10k(*count),
            if unambiguous.contains(category) {
                "yes"
            } else {
                ""
            }
        );
    }
}

/// Parses `model=CANONICAL,model2=CANONICAL` into a [`TaxonomyMap`].
///
/// # Errors
///
/// Any entry without exactly one `=`.
pub fn parse_taxonomy(spec: &str) -> Result<TaxonomyMap, String> {
    parse_entries(spec.split(','))
}

/// Reads a taxonomy map from a file: one `model_label=CANONICAL` per line,
/// `#` comments and blank lines ignored.
///
/// Preferred over the inline form for anything real. These maps run to dozens
/// of entries, they are the part of the measurement most likely to be wrong,
/// and a file can be reviewed in a diff.
///
/// # Errors
///
/// If the file cannot be read, or a line is not `model_label=CANONICAL`.
pub fn load_taxonomy(path: &Path) -> Result<TaxonomyMap, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    parse_entries(raw.lines().map(str::trim).filter(|l| !l.starts_with('#')))
}

fn parse_entries<'a>(entries: impl Iterator<Item = &'a str>) -> Result<TaxonomyMap, String> {
    let mut map = BTreeMap::new();
    for entry in entries.filter(|e| !e.trim().is_empty()) {
        let (from, to) = entry
            .split_once('=')
            .ok_or_else(|| format!("taxonomy entry '{entry}' is not `model_label=CANONICAL`"))?;
        map.insert(from.trim().to_owned(), to.trim().to_owned());
    }
    Ok(map.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taxonomy_spec_round_trips() {
        let map = parse_taxonomy("email=EMAIL, first_name=PERSON").expect("parses");
        assert_eq!(map.get("email"), Some(&"EMAIL".to_owned()));
        assert_eq!(map.get("first_name"), Some(&"PERSON".to_owned()));
    }

    #[test]
    fn a_malformed_taxonomy_entry_is_rejected() {
        assert!(parse_taxonomy("email:EMAIL").is_err());
    }

    #[test]
    fn person_names_are_not_unambiguous_false_positives() {
        // The distinction the negative-control metric rests on.
        assert!(!UNAMBIGUOUS_DEFAULT.contains(&"first_name"));
        assert!(!UNAMBIGUOUS_DEFAULT.contains(&"last_name"));
        assert!(UNAMBIGUOUS_DEFAULT.contains(&"email"));
    }
}
