use clap::Parser;
use env_logger::Env;
use repro_env::args::{Args, SubCommand};
use repro_env::build;
use repro_env::errors::*;
use repro_env::manifest::Manifest;
use repro_env::resolver;
use std::env;
use std::path::Path;
use tokio::fs;

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
        SubCommand::Update(update) => {
            let manifest_path = Path::new("repro-env.toml");
            let lockfile_path = Path::new("repro-env.lock");

            let buf = fs::read_to_string(manifest_path).await.with_context(|| {
                anyhow!("Failed to read dependency manifest: {manifest_path:?}")
            })?;

            let manifest = Manifest::deserialize(&buf)?;
            debug!("Loaded manifest from file: {manifest:?}");

            let lockfile = resolver::resolve(&update, &manifest).await?;
            trace!("Resolved manifest into lockfile: {lockfile:?}");

            let buf = lockfile.serialize()?;
            fs::write(lockfile_path, buf).await?;

            Ok(())
        }
        SubCommand::Completions(completions) => completions.generate(),
    }
}
