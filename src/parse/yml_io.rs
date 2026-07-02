//! yml serialization helpers for the `random_test:` section.

use super::types::RandomTestSection;
use anyhow::Context as _;
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use std::fs;

/// Read the persisted `random_test:` section from a test-suite yml file.
pub(crate) fn read_random_test_section(
    yml_path: &Utf8Path,
) -> anyhow::Result<Option<RandomTestSection>> {
    #[derive(Deserialize)]
    struct Wrapper {
        random_test: Option<RandomTestSection>,
    }
    let content =
        fs::read_to_string(yml_path).with_context(|| format!("failed to read {yml_path}"))?;
    let wrapper: Wrapper = serde_yaml::from_str(&content)
        .with_context(|| format!("failed to deserialize {yml_path}"))?;
    Ok(wrapper.random_test)
}

/// Append a `random_test:` section to an existing yml test-suite file.
pub(crate) fn append_format_to_yml(
    yml_path: &Utf8Path,
    section: &RandomTestSection,
) -> anyhow::Result<()> {
    #[derive(Serialize)]
    struct Wrapper<'a> {
        random_test: &'a RandomTestSection,
    }
    let yaml = serde_yaml::to_string(&Wrapper {
        random_test: section,
    })
    .with_context(|| format!("serde_yaml serialise failed for {yml_path}"))?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let yaml = compactify_values_sequences(yaml);
    let raw = fs::read_to_string(yml_path).with_context(|| format!("failed to read {yml_path}"))?;
    // Idempotent: drop any previously appended `random_test:` section (a
    // top-level, non-indented key) so re-running retrieve/annotate replaces
    // it instead of appending a duplicate mapping key (which makes the file
    // fail to deserialize).
    let mut content: String = match raw.lines().position(|l| l.starts_with("random_test:")) {
        Some(idx) => {
            let mut s = raw.lines().take(idx).collect::<Vec<_>>().join("\n");
            s.push('\n');
            s
        }
        None => raw,
    };
    if !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.ends_with("\n\n") {
        content.push('\n');
    }
    content.push_str(&yaml);
    fs::write(yml_path, content).with_context(|| format!("failed to write {yml_path}"))?;
    Ok(())
}

/// Convert block-style sequences to flow style for specific keys.
///
/// serde_yaml 0.8 always emits block sequences; this post-processes the output
/// so that the listed keys render on a single line.
///
/// Two cases handled:
/// - **Flat sequences** (`values`, `scalars`, `vars`, `range`): items are scalar lines `- x`.
///   → `key: [a, b, c]`
/// - **Nested pair sequences** (`ordering`, `not_equal`): items are 2-element sub-arrays.
///   → `key: [[a, b], [c, d]]`
///
/// Top-level `vars:` (a YAML mapping, not a sequence) is unaffected because its
/// child lines start with a key name, not `- `, so detection falls through.
pub(crate) fn compactify_values_sequences(yaml: &str) -> String {
    const FLAT_KEYS: &[&str] = &["values:", "scalars:", "vars:", "range:"];
    const PAIR_KEYS: &[&str] = &["ordering:", "not_equal:"];

    let mut out = String::with_capacity(yaml.len());
    let mut lines = yaml.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        let key = trimmed.trim_end_matches(':');

        if FLAT_KEYS.contains(&trimmed) {
            let prefix = " ".repeat(indent + 2) + "- ";
            let mut items: Vec<String> = Vec::new();
            while let Some(&next) = lines.peek() {
                if next.starts_with(&prefix) {
                    items.push(
                        next.trim_start_matches(' ')
                            .trim_start_matches("- ")
                            .to_string(),
                    );
                    lines.next();
                } else {
                    break;
                }
            }
            if items.is_empty() {
                out.push_str(line);
            } else {
                out.push_str(&format!(
                    "{}{}: [{}]",
                    " ".repeat(indent),
                    key,
                    items.join(", ")
                ));
            }
        } else if PAIR_KEYS.contains(&trimmed) {
            // serde_yaml emits the outer `-` at indent + 2, inner `-` at indent + 4.
            let outer_prefix = " ".repeat(indent + 2) + "- ";
            let inner_prefix = " ".repeat(indent + 4) + "- ";
            let mut pairs: Vec<String> = Vec::new();
            while let Some(&next) = lines.peek() {
                if !next.starts_with(&outer_prefix) {
                    break;
                }
                // Consume the outer `- ` line. The first inner item is on the SAME line
                // (after `- `) or on the NEXT line at inner indent. serde_yaml 0.8 emits
                // it on the next line for `Vec<[T; 2]>`.
                let outer = lines.next().unwrap();
                let mut sub: Vec<String> = Vec::new();
                let after = outer.trim_start_matches(' ').trim_start_matches("- ");
                if !after.is_empty() {
                    // First inner item inlined (e.g. `- - a`)
                    sub.push(after.trim_start_matches("- ").to_string());
                }
                while let Some(&inner) = lines.peek() {
                    if inner.starts_with(&inner_prefix) {
                        sub.push(
                            inner
                                .trim_start_matches(' ')
                                .trim_start_matches("- ")
                                .to_string(),
                        );
                        lines.next();
                    } else {
                        break;
                    }
                }
                pairs.push(format!("[{}]", sub.join(", ")));
            }
            if pairs.is_empty() {
                out.push_str(line);
            } else {
                out.push_str(&format!(
                    "{}{}: [{}]",
                    " ".repeat(indent),
                    key,
                    pairs.join(", ")
                ));
            }
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{compactify_values_sequences, read_random_test_section};

    #[test]
    fn flat_sequences_get_flow_style() {
        let yaml = "values:\n  - a\n  - b\n  - c\n";
        let got = compactify_values_sequences(yaml);
        assert_eq!(got, "values: [a, b, c]\n");
    }

    #[test]
    fn range_2_lit() {
        let yaml = "range:\n  - 1\n  - 100\n";
        let got = compactify_values_sequences(yaml);
        assert_eq!(got, "range: [1, 100]\n");
    }

    #[test]
    fn ordering_nested_pairs() {
        // serde_yaml outputs nested pair sequences at indent_of_key + 2 for the outer `-`,
        // and indent_of_key + 4 for the inner `-`.
        let yaml = "ordering:\n  - - a\n    - b\n  - - c\n    - d\n";
        let got = compactify_values_sequences(yaml);
        assert_eq!(got, "ordering: [[a, b], [c, d]]\n");
    }

    #[test]
    fn reads_random_test_section_through_shared_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().join("a.yml")).unwrap();
        std::fs::write(
            &path,
            "type: Batch\nrandom_test:\n  vars: {}\n  format: []\n",
        )
        .unwrap();
        let got = read_random_test_section(&path).unwrap().expect("section");
        assert!(got.format.is_empty());
    }

    #[test]
    fn not_equal_nested_pairs() {
        let yaml = "not_equal:\n  - - a\n    - b\n";
        let got = compactify_values_sequences(yaml);
        assert_eq!(got, "not_equal: [[a, b]]\n");
    }

    #[test]
    fn nested_inside_indent() {
        // `ordering:` inside `random_test:` (indent 2)
        let yaml = "random_test:\n  ordering:\n    - - a\n      - b\nother: 1\n";
        let got = compactify_values_sequences(yaml);
        assert_eq!(got, "random_test:\n  ordering: [[a, b]]\nother: 1\n");
    }
}
