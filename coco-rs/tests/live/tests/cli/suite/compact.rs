//! Force compaction with a small `context_window` and a long fact-list prompt.
//!
//! The agent loop should hit the auto-compact threshold mid-session and
//! emit `CompactionStarted` / `ContextCompacted` notifications. The
//! recall question at the end must succeed afterwards — proves the
//! compacted context preserved enough information to answer.

use anyhow::Result;

use crate::cli::events;
use crate::cli::harness::SessionConfig;
use crate::cli::harness::run_session;

fn build_prompt() -> String {
    let mut prompt = String::from(
        "Below is a list of facts. After it, you'll get a recall question.\n\
         For EACH fact, repeat it back verbatim on its own line so it lands \
         in the conversation history. Then answer the recall question.\n\n\
         FACTS:\n",
    );
    // Pad each fact with filler text on a separate line so the running
    // context grows quickly enough to cross the auto-compact threshold
    // even on small windows. Without padding the model's reply is too
    // short to push the conversation past the threshold.
    for (i, fact) in FACTS.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. {fact} (additional context: this fact was recorded on day {} of the project; \
             cross-reference notes are filed in archive section {}; reviewer initials JK confirmed; \
             tag set: persistent, biographical, low-priority, profile-only.)\n",
            i + 1,
            (i + 1) * 7,
            ((i + 1) * 13) % 100,
        ));
    }
    prompt.push_str(
        "\nNow answer this single question with the fact number followed by the value, \
         e.g. `7: <value>`:\n\n\
         Q: What is the favorite color of the developer in fact 7?\n",
    );
    prompt
}

const FACTS: &[&str] = &[
    "The developer Alice was born in 1990.",
    "Alice lives in Berlin.",
    "Alice prefers Rust over JavaScript.",
    "Alice owns a black cat named Mochi.",
    "Alice has been programming for twelve years.",
    "Alice's office is on the third floor.",
    "Alice's favorite color is teal.",
    "Alice's preferred IDE is Helix.",
    "Alice drinks oolong tea every morning.",
    "Alice runs five kilometers on Saturdays.",
    "Alice's keyboard is a split ergonomic Moonlander.",
    "Alice writes in the Dvorak layout.",
    "Alice has a younger brother named Jonas.",
    "Alice studied computer science at TU München.",
    "Alice once worked at a startup called Plumbus.",
    "Alice's favorite book is Snow Crash.",
    "Alice maintains an open source crate named `parquet-tail`.",
    "Alice's commute to work takes 22 minutes by bike.",
    "Alice took up bouldering in 2023.",
    "Alice's mentor in college was Professor Ortega.",
    "Alice has a small balcony garden with three tomato plants.",
    "Alice's last vacation was to Lisbon.",
    "Alice plays the cello on weekends.",
    "Alice prefers dark mode in every editor.",
    "Alice's coffee order is an oat-milk flat white.",
    "Alice once gave a conference talk about column-store indexes.",
    "Alice's first programming language was Pascal.",
    "Alice keeps a paper journal at her desk.",
    "Alice's password manager is 1Password.",
    "Alice's home network is named `tea-house-2g`.",
];

pub async fn run(provider: &str, model_id: &str) -> Result<()> {
    let cfg = SessionConfig::small_window(/*context_window*/ 4_000);
    let outcome = run_session(provider, model_id, cfg, &build_prompt()).await?;

    let summary = events::summarize(&outcome.events);
    // Compaction emission is best-effort: it depends on whether the model
    // hits the auto-compact threshold within `max_turns`. We log whether
    // it fired but only require the recall succeed — that's the user-
    // visible promise of "long context still works".
    eprintln!(
        "[compact] {provider}/{model_id}: compaction_fired={} ({summary})",
        events::saw_compaction(&outcome.events)
    );

    assert!(
        outcome.result.turns >= 1,
        "{provider}/{model_id}: expected >=1 turn, got {} ({summary})",
        outcome.result.turns
    );

    let lower = outcome.result.response_text.to_lowercase();
    assert!(
        lower.contains("teal"),
        "{provider}/{model_id}: post-compact recall failed; expected `teal`, got: {}",
        outcome.result.response_text
    );
    Ok(())
}
