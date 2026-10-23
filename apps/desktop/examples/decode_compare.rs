//! Exploratory MP-IMG2 Phase 1 decoder-comparison harness; not a product decoder.
//!
//! Operator run (writes one bounded exploratory record, never a PASS/winner):
//! `cargo run --locked -p cos-desktop --example decode_compare -- \
//!  --cases DIR --candidate ffmpeg=C:\ffmpeg\bin\ffmpeg.exe \
//!  --candidate wic=pwsh:C:\probe\wic_probe.ps1 --out C:\tmp\img2-compare\compare.json`
//!
//! Only operator-nominated programs are launched (an executable, or `pwsh` running
//! one nominated script), a fixed bounded argv is passed without any shell, and
//! every candidate runs under a hard deadline with capped file capture. The module
//! carries no decoder dependency, no `unsafe`, no shell and no GPU object; see
//! `decode_compare/harness.rs` for the candidate protocol and record schema.
#[path = "decode_compare/harness.rs"]
mod harness;

fn main() {
    std::process::exit(harness::run_from_args(std::env::args_os().skip(1)));
}
