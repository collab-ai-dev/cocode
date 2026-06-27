//! Run the multi-turn `tool_chain` scenario through both DeepSeek
//! protocols. Now that the thinking-block roundtrip bug is fixed in
//! `coco-inference::stream` + `coco-query::engine`, both protocols
//! should drive the agent loop through the same Bash → Write → Read
//! tool sequence successfully.

use anyhow::Result;

use super::tool_chain;

pub async fn run(model_id: &str) -> Result<()> {
    tool_chain::run("deepseek-openai", model_id).await?;
    tool_chain::run("deepseek-anthropic", model_id).await?;
    Ok(())
}
