use clap::Parser;
use env_logger::Env;
use repro_env::args::{Args, SubCommand};
use repro_env::build;
use repro_env::errors::*;
use repro_env::fetch;
use repro_env::update;
use std::env;
use std::io;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    env_logger::init_from_env(Env::default().default_filter_or(log_level));

    if let Some(path) = args.context {
        debug!("Changing current directory to {path:?}...");
        env::set_current_dir(&path)
            .with_context(|| anyhow!("Failed to switch to directory {path:?}"))?;
    }

    match args.subcommand {
        SubCommand::Build(build) => build::build(&build).await,
        SubCommand::Update(update) => update::update(&update).await,
        SubCommand::Fetch(fetch) => fetch::fetch(&fetch).await,
        SubCommand::Completions(completions) => completions.generate(io::stdout()),
    }
}
