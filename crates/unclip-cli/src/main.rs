//! unclip — outside-of-LLM possibility engine (CLI entry point).

#![forbid(unsafe_code)]

mod app;
mod cli;
mod commands;
mod db;
mod matching;
mod output;
mod sampling;
mod usage;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    app::run().await
}
