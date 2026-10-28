//! Declared callable metadata.
//!
//! This module reads the exact source text of callable signatures, parameters,
//! and return annotations, and it cleans docstrings.

use rustpython_parser::{
    Mode, Tok,
    ast::{Arguments, Constant, Expr, Ranged, Stmt, text_size::TextRange},
    lexer,
};

use super::{CallableSyntax, invalid_source_range, source_text, token_error};
use crate::analysis::fact::AnalysisError;

/// Reads a callable signature from its source range.
///
/// The returned signature excludes the final header colon.
///
/// # Errors
///
/// Returns an error if tokenization fails or the signature range is invalid.
pub(super) fn source_signature(
    source: &str,
    callable: &CallableSyntax<'_>,
) -> Result<String, AnalysisError> {
    let definition = source_text(source, callable.range)?;
    let mut depth = 0_u32;

    for token in lexer::lex(definition, Mode::Module) {
        let (token, range) = token.map_err(|error| token_error(&error))?;

        match token {
            // Enter a bracketed expression.
            Tok::Lpar | Tok::Lsqb | Tok::Lbrace => depth += 1,
            // Leave a bracketed expression.
            Tok::Rpar | Tok::Rsqb | Tok::Rbrace => depth = depth.saturating_sub(1),
            // Stop at the outer header colon.
            Tok::Colon if depth == 0 => {
                return definition
                    .get(..range.start().to_usize())
                    .map(|signature| signature.trim_end().to_owned())
                    .ok_or_else(invalid_source_range);
            }
            // Ignore other signature tokens.
            _ => {}
        }
    }

    Err(AnalysisError {
        message: format!("cannot find the end of the {} signature", callable.name),
    })
}

/// Reads callable parameters from their source ranges.
///
/// The result keeps Python declaration order and excludes `self` and `cls`.
///
/// # Errors
///
/// Returns an error if a parameter range is invalid.
pub(super) fn source_parameters(
    source: &str,
    arguments: &Arguments,
) -> Result<Vec<String>, AnalysisError> {
    let mut parameters = Vec::new();

    for argument in arguments.posonlyargs.iter().chain(&arguments.args) {
        add_parameter(
            source,
            &mut parameters,
            argument.def.arg.as_str(),
            argument.range,
            "",
        )?;
    }

    if let Some(argument) = &arguments.vararg {
        add_parameter(
            source,
            &mut parameters,
            argument.arg.as_str(),
            argument.range,
            "*",
        )?;
    }

    for argument in &arguments.kwonlyargs {
        add_parameter(
            source,
            &mut parameters,
            argument.def.arg.as_str(),
            argument.range,
            "",
        )?;
    }

    if let Some(argument) = &arguments.kwarg {
        add_parameter(
            source,
            &mut parameters,
            argument.arg.as_str(),
            argument.range,
            "**",
        )?;
    }

    Ok(parameters)
}

/// Adds one source parameter unless it is `self` or `cls`.
///
/// # Errors
///
/// Returns an error if the parameter range is invalid.
pub(super) fn add_parameter(
    source: &str,
    parameters: &mut Vec<String>,
    name: &str,
    range: TextRange,
    prefix: &str,
) -> Result<(), AnalysisError> {
    if name != "self" && name != "cls" {
        let parameter = source_text(source, range)?.trim();
        parameters.push(format!("{prefix}{parameter}"));
    }

    Ok(())
}

/// Reads an optional return annotation from its source range.
///
/// # Errors
///
/// Returns an error if the annotation range is invalid.
pub(super) fn source_return_type(
    source: &str,
    returns: Option<&Expr>,
) -> Result<Option<String>, AnalysisError> {
    returns
        .map(|annotation| {
            source_text(source, annotation.range()).map(|text| text.trim().to_owned())
        })
        .transpose()
}

pub(super) fn source_docstring(body: &[Stmt]) -> Option<String> {
    let Stmt::Expr(statement) = body.first()? else {
        return None;
    };
    let Expr::Constant(expression) = statement.value.as_ref() else {
        return None;
    };
    let Constant::Str(documentation) = &expression.value else {
        return None;
    };

    clean_docstring(documentation)
}

/// Removes common indentation and surrounding empty lines from a docstring.
pub(super) fn clean_docstring(documentation: &str) -> Option<String> {
    let mut lines = documentation.lines().collect::<Vec<_>>();
    let indentation = lines
        .iter()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);

    if let Some(first) = lines.first_mut() {
        *first = first.trim();
    }
    for line in lines.iter_mut().skip(1) {
        *line = line.get(indentation..).unwrap_or_default().trim_end();
    }
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    let documentation = lines.join("\n");
    (!documentation.is_empty()).then_some(documentation)
}
