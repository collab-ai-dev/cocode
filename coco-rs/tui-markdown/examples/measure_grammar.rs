//! Measure the resident-memory cost of prewarming syntect grammars.
//!
//! Syntax highlighting's resident footprint comes from the *compiled*
//! fancy-regex DFA of each grammar that gets touched — NOT from how many
//! grammars are bundled (an untouched grammar stays a compact
//! `serialized_lazy_contexts` blob and never expands). This tool quantifies
//! that per-grammar cost so the `prewarm_highlighting` allow-list in
//! `src/highlight.rs` can be tuned with real numbers instead of guesses.
//!
//! It parses the SAME snippet the real prewarm uses, so the measured delta
//! equals the real prewarm cost of that grammar.
//!
//! ## Usage
//!
//! Run one grammar per process to isolate its cost (peak RSS is per-process):
//!
//! ```bash
//! cargo run --release --example measure_grammar -p coco-tui-markdown
//! #   ^ baseline: load the SyntaxSet, compile nothing
//!
//! cargo run --release --example measure_grammar -p coco-tui-markdown -- rust
//! #   ^ baseline + compile one grammar; the delta is that grammar's cost
//!
//! cargo run --release --example measure_grammar -p coco-tui-markdown -- diff bash json rust
//! #   ^ a whole candidate prewarm set (same process → shared contexts, so the
//! #     combined delta is lower than summing the isolated per-grammar numbers)
//! ```
//!
//! Accepted names are the fence tags / aliases `find()` below understands
//! (`rs`, `py`, `js`, `ts`, `sh`/`bash`, `yml`, `go`, `md`, plus any exact
//! grammar token like `diff`, `json`, `toml`, `yaml`).
//!
//! ## Caveat
//!
//! `ru_maxrss` is the process *peak*, and a short-lived run's peak ≈ its final
//! resident set, which is what we want. It is reported in bytes on macOS and in
//! kilobytes on Linux; `rss_bytes()` normalizes both to bytes. This is a
//! developer measurement tool, not shipped in the `coco` binary.

use std::time::Instant;

use syntect::parsing::ParseState;
use syntect::parsing::SyntaxReference;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Keep in sync with `prewarm_highlighting`'s `SNIPPET` in `src/highlight.rs`
/// so the measured cost matches what the real prewarm pays.
const SNIPPET: &str = "# t *m* `c` fn x() { let y: i32 = 1; } [l](u)\n";

/// Peak resident set size of this process, in bytes.
fn rss_bytes() -> i64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe {
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
    }
    let raw = usage.ru_maxrss as i64;
    // macOS reports bytes; Linux reports kilobytes.
    if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    }
}

/// Resolve a fence tag / alias to a grammar, mirroring `highlight::find_syntax`.
fn find<'s>(ss: &'s SyntaxSet, lang: &str) -> Option<&'s SyntaxReference> {
    let alias = match lang {
        "rs" => "Rust",
        "py" => "Python",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "sh" | "bash" => "Bourne Again Shell (bash)",
        "yml" => "YAML",
        "go" => "Go",
        "md" => "Markdown",
        _ => lang,
    };
    ss.find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_name(alias))
        .or_else(|| ss.find_syntax_by_token(alias))
}

fn main() {
    let langs: Vec<String> = std::env::args().skip(1).collect();

    let t0 = Instant::now();
    let ss = two_face::syntax::extra_newlines();
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    // RSS after the SyntaxSet is loaded but before any grammar is compiled — the
    // baseline the per-grammar delta is measured against.
    let base = rss_bytes();

    let t1 = Instant::now();
    let mut compiled = Vec::new();
    for lang in &langs {
        let Some(sx) = find(&ss, lang) else {
            eprintln!("  (skipping unknown grammar: {lang})");
            continue;
        };
        // parse_line drives syntect's lazy regex compilation — the same path the
        // real prewarm exercises.
        let mut state = ParseState::new(sx);
        for line in LinesWithEndings::from(SNIPPET) {
            let _ = state.parse_line(line, &ss);
        }
        compiled.push(sx.name.clone());
    }
    let compile_ms = t1.elapsed().as_secs_f64() * 1000.0;
    let after = rss_bytes();

    let mb = |b: i64| b as f64 / 1024.0 / 1024.0;
    println!(
        "grammars=[{}] | load_set={load_ms:.1}ms rss_after_load={:.1}MB | \
         compile={compile_ms:.1}ms resident_delta={:.2}MB ({} grammars)",
        compiled.join(", "),
        mb(base),
        mb(after - base),
        compiled.len(),
    );
}
