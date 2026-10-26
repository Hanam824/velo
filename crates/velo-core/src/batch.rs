//! Batch downloads: one address pattern with a `*` that stands for a run of numbers or letters,
//! such as `https://a.test/photo*.jpg` from 1 to 20.

use crate::{Error, Result};

/// What the `*` in a batch pattern runs through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchRange {
    /// Numbers from `from` to `to`, padded with zeros to at least `width` digits.
    Numbers {
        /// The first number.
        from: u64,
        /// The last number.
        to: u64,
        /// The fewest digits; `1` pads nothing.
        width: usize,
    },
    /// Letters from `from` to `to`, both lowercase or both uppercase ASCII.
    Letters {
        /// The first letter.
        from: char,
        /// The last letter.
        to: char,
    },
}

/// The most addresses one batch makes.
pub const MAX_BATCH: u64 = 10_000;

/// The addresses `pattern` makes with its `*` replaced by each value of `range`, in order.
///
/// Fails if `pattern` doesn't have exactly one `*`, if the range runs backwards or mixes cases,
/// or if it would make more than [`MAX_BATCH`] addresses.
pub fn expand(pattern: &str, range: BatchRange) -> Result<Vec<String>> {
    let Some((before, after)) = pattern.split_once('*') else {
        return Err(Error::Invalid("the address needs a * to replace".into()));
    };
    if after.contains('*') {
        return Err(Error::Invalid("the address can have only one *".into()));
    }
    let values: Vec<String> = match range {
        BatchRange::Numbers { from, to, width } => {
            check_order(from <= to)?;
            check_count((to - from).saturating_add(1))?;
            (from..=to).map(|n| format!("{n:0width$}")).collect()
        }
        BatchRange::Letters { from, to } => {
            let same_case = (from.is_ascii_lowercase() && to.is_ascii_lowercase())
                || (from.is_ascii_uppercase() && to.is_ascii_uppercase());
            if !same_case {
                return Err(Error::Invalid("letters must be a to z, or A to Z".into()));
            }
            check_order(from <= to)?;
            (from..=to).map(String::from).collect()
        }
    };
    Ok(values
        .into_iter()
        .map(|value| format!("{before}{value}{after}"))
        .collect())
}

/// Fails unless the range runs forwards.
fn check_order(forwards: bool) -> Result<()> {
    if forwards {
        Ok(())
    } else {
        Err(Error::Invalid(
            "the range must not end before it starts".into(),
        ))
    }
}

/// Fails if a batch would make `count` addresses, too many.
fn check_count(count: u64) -> Result<()> {
    if count > MAX_BATCH {
        Err(Error::Invalid(format!(
            "a batch makes at most {MAX_BATCH} addresses, not {count}"
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_padded_to_the_width() {
        let urls = expand(
            "https://a.test/img*.jpg",
            BatchRange::Numbers {
                from: 8,
                to: 11,
                width: 3,
            },
        )
        .unwrap();
        assert_eq!(
            urls,
            [
                "https://a.test/img008.jpg",
                "https://a.test/img009.jpg",
                "https://a.test/img010.jpg",
                "https://a.test/img011.jpg"
            ]
        );
        let unpadded = BatchRange::Numbers {
            from: 9,
            to: 10,
            width: 1,
        };
        assert_eq!(expand("*", unpadded).unwrap(), ["9", "10"]);
    }

    #[test]
    fn letters_run_within_one_case() {
        let range = BatchRange::Letters { from: 'x', to: 'z' };
        assert_eq!(
            expand("a/*.zip", range).unwrap(),
            ["a/x.zip", "a/y.zip", "a/z.zip"]
        );
        let range = BatchRange::Letters { from: 'A', to: 'B' };
        assert_eq!(expand("*", range).unwrap(), ["A", "B"]);
        assert!(expand("*", BatchRange::Letters { from: 'a', to: 'C' }).is_err());
        assert!(expand("*", BatchRange::Letters { from: '1', to: '3' }).is_err());
    }

    #[test]
    fn bad_patterns_and_ranges_are_refused() {
        let numbers = |from, to| BatchRange::Numbers { from, to, width: 1 };
        assert!(expand("no-star", numbers(1, 2)).is_err());
        assert!(expand("**", numbers(1, 2)).is_err());
        assert!(expand("*", numbers(3, 2)).is_err());
        assert!(expand("*", BatchRange::Letters { from: 'c', to: 'a' }).is_err());
        assert!(expand("*", numbers(1, MAX_BATCH)).is_ok());
        assert!(expand("*", numbers(0, MAX_BATCH)).is_err());
        assert!(expand("*", numbers(0, u64::MAX)).is_err());
    }
}
