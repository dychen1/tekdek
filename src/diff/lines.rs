//! Line diff of one function between two commits.
//!
//! Two lists of lines are the input. One unified list of context, added, and
//! removed lines is the output. The counts come from the same list, so a count
//! and the code view cannot disagree.

use super::models::{DiffLine, LineOp};

/// The largest LCS table. A bigger pair shows as all removed, then all added.
const MAX_CELLS: usize = 4_000_000;

/// Returns the unified diff of `old` and `new` with every context line.
#[must_use]
pub fn diff_lines(old: &[String], new: &[String]) -> Vec<DiffLine> {
    let line = |op, text: &String| DiffLine {
        op,
        text: text.clone(),
    };
    let (n, m) = (old.len(), new.len());
    if n.saturating_mul(m) > MAX_CELLS {
        // shortcut: no alignment above MAX_CELLS; use Myers if functions this long matter.
        return old
            .iter()
            .map(|text| line(LineOp::Removed, text))
            .chain(new.iter().map(|text| line(LineOp::Added, text)))
            .collect();
    }
    // lengths[i][j] is the LCS length of old[i..] and new[j..].
    let width = m + 1;
    let mut lengths = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lengths[i * width + j] = if old[i] == new[j] {
                lengths[(i + 1) * width + j + 1] + 1
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }
    let mut output = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            output.push(line(LineOp::Context, &old[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lengths[(i + 1) * width + j] >= lengths[i * width + j + 1]) {
            // Removed lines come before added lines, as in git.
            output.push(line(LineOp::Removed, &old[i]));
            i += 1;
        } else {
            output.push(line(LineOp::Added, &new[j]));
            j += 1;
        }
    }
    output
}

/// Returns the added and the removed line counts of one unified diff.
#[must_use]
pub fn counts(lines: &[DiffLine]) -> (u32, u32) {
    lines
        .iter()
        .fold((0, 0), |(added, removed), line| match line.op {
            LineOp::Added => (added + 1, removed),
            LineOp::Removed => (added, removed + 1),
            LineOp::Context => (added, removed),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|&line| line.to_owned()).collect()
    }

    fn ops(lines: &[DiffLine]) -> String {
        lines
            .iter()
            .map(|line| match line.op {
                LineOp::Context => ' ',
                LineOp::Added => '+',
                LineOp::Removed => '-',
            })
            .collect()
    }

    #[test]
    fn a_changed_line_shows_as_one_removed_and_one_added_line() {
        let lines = diff_lines(
            &text(&["fn a() {", "  1", "}"]),
            &text(&["fn a() {", "  2", "}"]),
        );
        assert_eq!(ops(&lines), " -+ ");
        assert_eq!(counts(&lines), (1, 1));
    }

    #[test]
    fn an_empty_side_makes_every_line_added_or_removed() {
        let body = text(&["fn a() {", "}"]);
        assert_eq!(counts(&diff_lines(&[], &body)), (2, 0));
        assert_eq!(counts(&diff_lines(&body, &[])), (0, 2));
        assert_eq!(counts(&diff_lines(&body, &body)), (0, 0));
    }

    #[test]
    fn inserted_lines_keep_the_surrounding_lines_as_context() {
        let lines = diff_lines(&text(&["a", "d"]), &text(&["a", "b", "c", "d"]));
        assert_eq!(ops(&lines), " ++ ");
    }
}
