//! Declared callable metadata.
//!
//! This module reads the exact source text of callable parameters and return
//! types, their type ranges, and documentation comments.

use syn::{Attribute, Expr, FnArg, ReturnType, Signature, spanned::Spanned};

use super::{ParsedDocument, source_range, source_text};
use crate::analysis::{fact::AnalysisError, source::SourceRange};

pub(super) fn callable_parameters(
    document: &ParsedDocument<'_>,
    signature: &Signature,
) -> Result<Vec<String>, AnalysisError> {
    let mut parameters = signature
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Receiver(_) => None,
            FnArg::Typed(argument) => Some(argument),
        })
        .map(|argument| source_text(document, argument.span()).map(|text| text.trim().to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(variadic) = &signature.variadic {
        parameters.push(source_text(document, variadic.span())?.trim().to_owned());
    }
    Ok(parameters)
}

pub(super) fn callable_return_type(
    document: &ParsedDocument<'_>,
    signature: &Signature,
) -> Result<Option<String>, AnalysisError> {
    match &signature.output {
        ReturnType::Default => Ok(None),
        ReturnType::Type(_, ty) => Ok(Some(source_text(document, ty.span())?.trim().to_owned())),
    }
}

/// Returns the type ranges of callable parameters.
pub(super) fn callable_type_inputs(
    document: &ParsedDocument<'_>,
    signature: &Signature,
) -> Result<Vec<SourceRange>, AnalysisError> {
    signature
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Receiver(_) => None,
            FnArg::Typed(argument) => Some(argument.ty.span()),
        })
        .map(|span| source_range(document, span))
        .collect()
}

/// Returns the type range of a callable return.
pub(super) fn callable_type_outputs(
    document: &ParsedDocument<'_>,
    signature: &Signature,
) -> Result<Vec<SourceRange>, AnalysisError> {
    match &signature.output {
        ReturnType::Default => Ok(Vec::new()),
        ReturnType::Type(_, ty) => Ok(vec![source_range(document, ty.span())?]),
    }
}

pub(super) fn documentation(attrs: &[Attribute]) -> Option<String> {
    let lines = attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .filter_map(|attribute| {
            let syn::Meta::NameValue(meta) = &attribute.meta else {
                return None;
            };
            let Expr::Lit(expression) = &meta.value else {
                return None;
            };
            let syn::Lit::Str(value) = &expression.lit else {
                return None;
            };
            Some(
                value
                    .value()
                    .strip_prefix(' ')
                    .unwrap_or(&value.value())
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let documentation = lines.join("\n");
    (!documentation.trim().is_empty()).then(|| documentation.trim().to_owned())
}
