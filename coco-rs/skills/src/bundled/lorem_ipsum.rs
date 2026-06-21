//! `/lorem-ipsum` — generate lorem-ipsum-style filler text for long-context testing. Mirrors claude-code's loremIpsum.ts.
//! Deferred: native token-accurate generator (loremIpsum.ts) not yet ported; model approximates.

pub const PROMPT: &str = r#"Generate filler text for long-context testing. Specify token count as argument (e.g., /lorem-ipsum 50000). Output approximately the requested number of tokens of lorem-ipsum-style filler text. Cap at 500,000 tokens for safety.

If no argument is given: default to 10,000 tokens.

If the argument is not a positive integer: respond with "Invalid token count. Please provide a positive number (e.g., /lorem-ipsum 10000)." and stop.
"#;
