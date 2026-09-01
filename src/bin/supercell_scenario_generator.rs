//! Generate SuperCell runtime inputs from an ai-bm-sim scenario YAML.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let scenario = take_path(&mut arguments, "--scenario")?;
    let template = take_path(&mut arguments, "--template")?;
    let output_dir = take_path(&mut arguments, "--output-dir")?;
    if arguments.next().is_some() {
        bail!(
            "usage: supercell-scenario-generator --scenario YAML --template TOML --output-dir DIRECTORY"
        );
    }
    supercell::scenario_contract::generate(&scenario, &template, &output_dir)
}

fn take_path(arguments: &mut impl Iterator<Item = String>, expected: &str) -> Result<PathBuf> {
    let flag = arguments.next().context("missing generator arguments")?;
    if flag != expected {
        bail!("expected {expected}, received {flag}");
    }
    arguments
        .next()
        .map(PathBuf::from)
        .context("missing path after generator argument")
}
