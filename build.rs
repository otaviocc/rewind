//! Bakes the syntax set into a binary pack at build time.

use std::error::Error;
use std::path::{Path, PathBuf};

use syntect::parsing::{SyntaxDefinition, SyntaxSet};

fn main() -> Result<(), Box<dyn Error>> {
    let syntaxes = Path::new("syntaxes");
    println!("cargo::rerun-if-changed={}", syntaxes.display());
    println!("cargo::rerun-if-changed=build.rs");

    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();

    let mut files: Vec<PathBuf> = std::fs::read_dir(syntaxes)
        .map_err(|error| format!("{}: {error}", syntaxes.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sublime-syntax"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("{}: nothing to bundle", syntaxes.display()).into());
    }

    for path in &files {
        let yaml = std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let definition =
            SyntaxDefinition::load_from_str(&yaml, true, None).map_err(|error| format!("{}: {error}", path.display()))?;
        builder.add(definition);
    }

    let directory = std::env::var_os("OUT_DIR").ok_or("OUT_DIR is unset")?;
    let out = PathBuf::from(directory).join("syntaxes.pack");
    syntect::dumps::dump_to_uncompressed_file(&builder.build(), &out).map_err(|error| format!("{}: {error}", out.display()))?;
    Ok(())
}
