//! Validation shared by test and submit generated-check options.

use anyhow::bail;

pub(crate) fn validate_generated_check_options(
    no_test: bool,
    no_sample: bool,
    has_random: bool,
    has_cross: bool,
) -> anyhow::Result<()> {
    if no_test && has_random {
        bail!("`--random` and `--no-test` cannot be used together");
    }
    if no_test && has_cross {
        bail!("`--cross` and `--no-test` cannot be used together");
    }
    if no_sample && !has_random && !has_cross {
        bail!("`--no-sample` requires `--random` or `--cross`");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_test_rejects_generated_checks() {
        assert!(validate_generated_check_options(true, false, true, false).is_err());
        assert!(validate_generated_check_options(true, false, false, true).is_err());
    }

    #[test]
    fn no_sample_requires_generated_check() {
        assert!(validate_generated_check_options(false, true, false, false).is_err());
        assert!(validate_generated_check_options(false, true, true, false).is_ok());
        assert!(validate_generated_check_options(false, true, false, true).is_ok());
    }

    #[test]
    fn plain_no_test_remains_valid() {
        assert!(validate_generated_check_options(true, false, false, false).is_ok());
    }
}
