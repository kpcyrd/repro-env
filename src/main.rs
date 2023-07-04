use clap::Parser;
use env_logger::Env;
use repro_env::args::{Args, SubCommand};
use repro_env::container;
use repro_env::errors::*;
use repro_env::lockfile::Lockfile;
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

    match args.subcommand {
        SubCommand::Build(build) => {
            let path = build.file.as_deref().unwrap_or(Path::new("repro-env.lock"));

            let buf = fs::read_to_string(path)
                .await
                .with_context(|| anyhow!("Failed to read dependency lockfile: {path:?}"))?;

            let lockfile = Lockfile::deserialize(&buf)?;
            debug!("Loaded dependency lockfile from file: {lockfile:?}");

            let image = &lockfile.container.image;
            let cmd = build.cmd.iter().map(|s| s.as_str()).collect::<Vec<_>>();

            let pwd = env::current_dir()?;
            let pwd = pwd
                .into_os_string()
                .into_string()
                .map_err(|_| anyhow!("Failed to convert current path to utf-8"))?;

            container::run(image, &cmd, &[(&pwd, "/build")]).await?;

            Ok(())
        }
        SubCommand::Update(update) => {
            let manifest_path = Path::new("repro-env.toml");
            let lockfile_path = Path::new("repro-env.lock");

            let buf = fs::read_to_string(manifest_path).await.with_context(|| {
                anyhow!("Failed to read dependency manifest: {manifest_path:?}")
            })?;

            let manifest = Manifest::deserialize(&buf)?;
            debug!("Loaded manifest from file: {manifest:?}");

            let lockfile = resolver::resolve(&update, &manifest).await?;
            debug!("Resolved manifest into lockfile: {lockfile:?}");

            let buf = lockfile.serialize()?;
            fs::write(lockfile_path, buf).await?;

            Ok(())
        }
        SubCommand::Completions(completions) => completions.generate(),
    }
}
