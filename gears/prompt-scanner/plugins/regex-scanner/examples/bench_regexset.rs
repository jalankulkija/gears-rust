//! Why the pattern set is driven by a `RegexSet` prefilter.
//!
//! ```sh
//! cargo run --release -p cf-gears-regex-scanner --example bench_regexset
//! ```
//!
//! Compares two ways of running N patterns over a payload:
//!
//! - **sequential** — every pattern scans the whole text, so N patterns cost N
//!   passes. This is what the gear did when patterns were hardcoded.
//! - **prefilter + spans** — a `RegexSet` answers "which patterns occur?" in
//!   one pass, and only those are re-run to extract spans. This is what
//!   `PatternSet::scan` does.
//!
//! The interesting case is a payload with no sensitive material, which is the
//! common one for a scanning proxy: it collapses from N passes to one.

use std::error::Error;
use std::time::{Duration, Instant};

use regex::{Regex, RegexSet};
use regex_scanner::config::default_patterns;
use regex_scanner::domain::PatternSet;

/// Microseconds per iteration, as a plain number so no non-ASCII unit slips
/// into the output.
fn per_iter_us(total: Duration, iterations: u32) -> f64 {
    total.as_secs_f64() * 1e6 / f64::from(iterations)
}

fn main() -> Result<(), Box<dyn Error>> {
    let cfgs = default_patterns();
    let set = PatternSet::compile(&cfgs, 3.0)?;

    let mut individually = Vec::with_capacity(cfgs.len());
    for cfg in &cfgs {
        individually.push(Regex::new(&cfg.regex)?);
    }
    let bare_set = RegexSet::new(cfgs.iter().map(|c| &c.regex))?;

    let clean = "Thanks for the update. Could you summarise the attached quarterly report \
                 and highlight anything that looks unusual in the regional breakdown? \
                 I would like a short paragraph per region, plus one chart suggestion."
        .repeat(8);
    let hit = format!("{clean} my email is jane.doe@example.com by the way");

    let iterations: u32 = 2_000;

    for (label, text) in [("clean", &clean), ("one match", &hit)] {
        let mut sink = 0_usize;

        let started = Instant::now();
        for _ in 0..iterations {
            for re in &individually {
                sink += re.find_iter(text).count();
            }
        }
        let sequential = started.elapsed();

        let started = Instant::now();
        for _ in 0..iterations {
            sink += bare_set.matches(text).iter().count();
        }
        let prefilter_only = started.elapsed();

        let started = Instant::now();
        let mut findings = 0_usize;
        for _ in 0..iterations {
            findings += set.scan(text).len();
        }
        let production = started.elapsed();

        println!(
            "{label} ({} bytes, {} patterns)",
            text.len(),
            individually.len()
        );
        println!(
            "  sequential        {:>8.1} us/iter",
            per_iter_us(sequential, iterations)
        );
        println!(
            "  prefilter only    {:>8.1} us/iter",
            per_iter_us(prefilter_only, iterations)
        );
        println!(
            "  prefilter + spans {:>8.1} us/iter   speedup {:.1}x",
            per_iter_us(production, iterations),
            sequential.as_secs_f64() / production.as_secs_f64(),
        );
        println!("  findings total {findings}  (checksum {sink})");
    }

    Ok(())
}
