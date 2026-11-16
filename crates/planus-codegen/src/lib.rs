//! Library for doing code generation for types from [`planus_types`].
//!
//! This library is an internal implementation
//! detail of [planus-cli](https://docs.rs/planus-cli).
//!
//! Feel free to use it, however there are no stability guarantees.

use askama::Template;
use planus_types::intermediate::Declarations;

use crate::{
    analysis::run_analysis, backend_translation::run_backend, dot::DotBackend, rust::RustBackend,
};

mod analysis;
mod backend;
mod backend_translation;
mod dot;
mod rust;
mod templates;

/// Options for [`generate_rust_with_options`].
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RustOptions {
    /// Run rustfmt on the generated code.
    pub format: bool,
    /// Generate a `<Name>Key` type for every table and struct.
    ///
    /// A table key holds every field of its table as a hashable value: floats
    /// as their bit patterns, structs as their struct keys, and strings,
    /// vectors, tables and unions as the offsets they were written at. Equal
    /// keys write identical tables, so a map from key to offset can share
    /// repeated tables within one buffer.
    pub generate_keys: bool,
}

impl Default for RustOptions {
    fn default() -> Self {
        Self {
            format: true,
            generate_keys: false,
        }
    }
}

pub fn generate_rust(declarations: &Declarations, format: bool) -> eyre::Result<String> {
    generate_rust_with_options(
        declarations,
        &RustOptions {
            format,
            ..RustOptions::default()
        },
    )
}

pub fn generate_rust_with_options(
    declarations: &Declarations,
    options: &RustOptions,
) -> eyre::Result<String> {
    let default_analysis = run_analysis(declarations, &mut rust::analysis::DefaultAnalysis);
    let eq_analysis = run_analysis(declarations, &mut rust::analysis::EqAnalysis);
    let infallible_analysis = run_analysis(
        declarations,
        &mut rust::analysis::InfallibleConversionAnalysis,
    );
    let output = run_backend(
        &mut RustBackend {
            default_analysis,
            eq_analysis,
            infallible_analysis,
            generate_keys: options.generate_keys,
        },
        declarations,
    );
    let res = templates::rust::Namespace(&output).render().unwrap();
    if options.format {
        let res = rust::format_string(&res, Some(1_000_000))?;
        let res = rust::format_string(&res, None)?;
        Ok(res)
    } else {
        Ok(res)
    }
}

pub fn generate_dot(declarations: &Declarations) -> String {
    let output = run_backend(&mut DotBackend { color_seed: 0 }, declarations);

    templates::dot::Namespace(&output).render().unwrap()
}
