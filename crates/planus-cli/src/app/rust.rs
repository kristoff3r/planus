use std::{io::Write, path::PathBuf, process::ExitCode};

use clap::{Parser, ValueHint};
use color_eyre::Result;
use planus_codegen::{RustOptions, generate_rust_with_options};
use planus_translation::translate_files_with_options;

/// Generate rust code
#[derive(Parser)]
pub struct Command {
    #[clap(value_hint = ValueHint::FilePath, required = true)]
    files: Vec<PathBuf>,

    /// Output file
    #[clap(short = 'o')]
    #[clap(value_hint = ValueHint::AnyPath)]
    output_filename: PathBuf,

    /// Run rustfmt on the generated code
    #[clap(long, default_value_t = true, action = clap::ArgAction::Set)]
    format: bool,

    /// Generate a `<Name>Key` type for every table and struct: a hashable
    /// value holding every field, for sharing repeated tables within a buffer
    #[clap(long)]
    generate_keys: bool,
}

impl Command {
    pub fn run(self, options: super::AppOptions) -> Result<ExitCode> {
        let Some(declarations) =
            translate_files_with_options(&self.files, options.to_converter_options())
        else {
            return Ok(ExitCode::FAILURE);
        };

        let mut rust_options = RustOptions::default();
        rust_options.format = self.format;
        rust_options.generate_keys = self.generate_keys;
        let res = generate_rust_with_options(&declarations, &rust_options)?;
        let mut file = std::fs::File::create(&self.output_filename)?;
        file.write_all(res.as_bytes())?;
        file.flush()?;

        Ok(ExitCode::SUCCESS)
    }
}
