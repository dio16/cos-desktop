//! Development-only MP-PUB4 representation experiment summary.
//!
//! Prints the deterministic exploratory summary for the documented sizes. Add
//! `--extended` to also record the 100k curve on demand. This is not a product
//! command, a numeric gate or an adoption decision.

use cos_client::representation::{WorkloadConfig, run_experiment};

fn main() {
    let extended = std::env::args().any(|argument| argument == "--extended");
    let mut configs = WorkloadConfig::summary();
    if extended {
        configs.extend(WorkloadConfig::extended());
    }
    match run_experiment(&configs) {
        Ok(report) => {
            println!("{}", report.summary_json());
        }
        Err(error) => {
            eprintln!("representation experiment failed: {error}");
            std::process::exit(1);
        }
    }
}
