//! Declared callable metadata.
//!
//! This module reads the exact source text of callable signatures, parameters,
//! and type annotations, and it cleans documentation comments.

use oxc_ast::{
    Comment,
    ast::{FormalParameters, TSTypeAnnotation},
};
use oxc_span::{GetSpan, Span};

use super::{CallableSyntax, range};
use crate::analysis::source::SourceRange;

/// Reads the signature text of one callable.
///
/// The text runs from the header start to the body, without a final `=>` or
/// `;`. A variable binding keeps its keyword, such as `const`.
pub(super) fn signature(source: &str, callable: &CallableSyntax<'_, '_>) -> String {
    let end = callable.body.map_or(callable.scope.end, |body| body.start);
    let text = Span::new(callable.header_start.min(end), end)
        .source_text(source)
        .trim();
    let text = text
        .strip_suffix("=>")
        .or_else(|| text.strip_suffix(';'))
        .unwrap_or(text)
        .trim_end();
    match callable.keyword {
        Some(keyword) => format!("{keyword} {text}"),
        None => text.to_owned(),
    }
}

/// Reads each parameter from its name to the end of its type.
///
/// A default value and the `this` parameter are absent.
pub(super) fn parameters(source: &str, parameters: &FormalParameters<'_>) -> Vec<String> {
    let mut texts = parameters
        .items
        .iter()
        .map(|parameter| {
            parameter_text(
                source,
                parameter.pattern.span(),
                parameter.type_annotation.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    if let Some(rest) = &parameters.rest {
        texts.push(parameter_text(
            source,
            rest.rest.span,
            rest.type_annotation.as_deref(),
        ));
    }
    texts
}

fn parameter_text(
    source: &str,
    pattern: Span,
    annotation: Option<&TSTypeAnnotation<'_>>,
) -> String {
    let end = annotation.map_or(pattern.end, |annotation| annotation.span.end);
    Span::new(pattern.start, end)
        .source_text(source)
        .trim()
        .to_owned()
}

/// Returns the parameter type ranges and the return type range of one callable.
pub(super) fn annotations(
    parameters: &FormalParameters<'_>,
    return_type: Option<&TSTypeAnnotation<'_>>,
) -> (Vec<SourceRange>, Vec<SourceRange>) {
    let inputs = parameters
        .items
        .iter()
        .filter_map(|parameter| parameter.type_annotation.as_deref())
        .chain(
            parameters
                .rest
                .iter()
                .filter_map(|rest| rest.type_annotation.as_deref()),
        )
        .map(|annotation| range(annotation.type_annotation.span()))
        .collect();
    let outputs = return_type
        .map(|annotation| range(annotation.type_annotation.span()))
        .into_iter()
        .collect();
    (inputs, outputs)
}

/// Reads the type of one annotation, without its colon.
pub(super) fn type_text(source: &str, annotation: Option<&TSTypeAnnotation<'_>>) -> Option<String> {
    annotation.map(|annotation| {
        annotation
            .type_annotation
            .span()
            .source_text(source)
            .trim()
            .to_owned()
    })
}

/// Returns the cleaned `JSDoc` comment right before one declaration.
///
/// Only modifiers, such as `export`, can stand between the comment and the
/// declaration.
pub(super) fn documentation(source: &str, comments: &[Comment], start: u32) -> Option<String> {
    let before = comments.partition_point(|comment| comment.span.end <= start);
    let comment = comments.get(..before)?.last()?;
    if !comment.is_jsdoc() {
        return None;
    }
    let between = source.get(comment.span.end as usize..start as usize)?;
    if !between
        .split_whitespace()
        .all(|word| MODIFIERS.contains(&word))
    {
        return None;
    }
    clean_jsdoc(comment.content_span().source_text(source))
}

/// The keywords that can stand before a declaration.
const MODIFIERS: &[&str] = &[
    "abstract",
    "accessor",
    "async",
    "declare",
    "default",
    "export",
    "override",
    "private",
    "protected",
    "public",
    "readonly",
    "static",
];

/// Removes the comment stars and surrounding empty lines from a `JSDoc` text.
fn clean_jsdoc(content: &str) -> Option<String> {
    let content = content.strip_prefix('*').unwrap_or(content);
    let lines = content
        .lines()
        .map(|line| {
            let line = line.trim();
            let line = line.strip_prefix('*').unwrap_or(line);
            line.strip_prefix(' ').unwrap_or(line).trim_end()
        })
        .collect::<Vec<_>>();
    let first = lines.iter().position(|line| !line.is_empty())?;
    let last = lines.iter().rposition(|line| !line.is_empty())?;
    Some(lines.get(first..=last)?.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::clean_jsdoc;

    #[test]
    fn cleans_stars_and_keeps_inner_indentation() {
        assert_eq!(
            clean_jsdoc("*\n   * Adds one.\n   *\n   *   code\n   ").as_deref(),
            Some("Adds one.\n\n  code")
        );
        assert_eq!(clean_jsdoc("* One line. ").as_deref(), Some("One line."));
        assert_eq!(clean_jsdoc("*\n *\n "), None);
    }
}
