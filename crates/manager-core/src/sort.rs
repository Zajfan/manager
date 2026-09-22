use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

use crate::Entry;

/// Which column to sort by (the columns of a Total Commander panel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Extension,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    pub order: SortOrder,
    /// Keep folders above files regardless of order, like every classic file manager.
    pub dirs_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec {
            key: SortKey::Name,
            order: SortOrder::Ascending,
            dirs_first: true,
        }
    }
}

/// Sorts a listing in place.
///
/// Folders have no meaningful size or extension, so when sorting by those
/// columns the folders are ordered by name instead. Ties always fall back to
/// natural name order, so the result is stable and predictable.
pub fn sort_entries(entries: &mut [Entry], spec: SortSpec) {
    entries.sort_by(|a, b| compare(a, b, spec));
}

fn compare(a: &Entry, b: &Entry, spec: SortSpec) -> Ordering {
    let (a_dir, b_dir) = (a.kind.is_dir_like(), b.kind.is_dir_like());
    if spec.dirs_first && a_dir != b_dir {
        // Not affected by `order`: folders stay on top even when sorting descending.
        return b_dir.cmp(&a_dir);
    }

    let key = if a_dir && b_dir {
        match spec.key {
            SortKey::Extension | SortKey::Size => SortKey::Name,
            other => other,
        }
    } else {
        spec.key
    };

    let by_name = || natural_cmp(&a.name, &b.name);
    let ord = match key {
        SortKey::Name => by_name(),
        SortKey::Extension => natural_cmp(a.extension().unwrap_or(""), b.extension().unwrap_or(""))
            .then_with(|| natural_cmp(a.stem(), b.stem())),
        SortKey::Size => a.size.cmp(&b.size).then_with(by_name),
        SortKey::Modified => a.modified.cmp(&b.modified).then_with(by_name),
    };

    match spec.order {
        SortOrder::Ascending => ord,
        SortOrder::Descending => ord.reverse(),
    }
}

/// "Natural" string comparison, the way humans expect file names to be sorted:
/// numbers compare by value (`file2` < `file10`) and letters ignore case (`apple` < `Banana`).
///
/// If two names are equal under those rules (`File` vs `file`, `01` vs `1`)
/// we fall back to a plain comparison so the order is always deterministic.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();

    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let ord = cmp_numbers(&take_digits(&mut ai), &take_digits(&mut bi));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(ca), Some(cb)) => {
                let ord = ca.to_lowercase().cmp(cb.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

fn take_digits(chars: &mut Peekable<Chars>) -> String {
    let mut digits = String::new();
    while let Some(c) = chars.next_if(char::is_ascii_digit) {
        digits.push(c);
    }
    digits
}

/// Compares two digit strings by numeric value without parsing them,
/// so arbitrarily long numbers (`photo_20240101123000`) can't overflow.
fn cmp_numbers(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut names: Vec<&str>) -> Vec<&str> {
        names.sort_by(|a, b| natural_cmp(a, b));
        names
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(
            sorted(vec!["file10", "file2", "file1", "file20"]),
            vec!["file1", "file2", "file10", "file20"]
        );
    }

    #[test]
    fn letters_ignore_case() {
        assert_eq!(
            sorted(vec!["banana", "Apple", "cherry"]),
            vec!["Apple", "banana", "cherry"]
        );
    }

    #[test]
    fn huge_numbers_do_not_overflow() {
        assert_eq!(
            natural_cmp("x99999999999999999999999", "x100000000000000000000000"),
            Ordering::Less
        );
    }

    #[test]
    fn equal_looking_names_still_have_a_fixed_order() {
        assert_ne!(natural_cmp("File", "file"), Ordering::Equal);
        assert_ne!(natural_cmp("01", "1"), Ordering::Equal);
    }

    #[test]
    fn prefix_sorts_first() {
        assert_eq!(natural_cmp("abc", "abcd"), Ordering::Less);
    }
}
