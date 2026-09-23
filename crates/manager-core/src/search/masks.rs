//! File-name masks, the way a search box takes them.
//!
//! `*.txt` is the whole idea, but the details are where the surprises are:
//! several masks at once, masks that *exclude*, and `*.*` meaning "everything"
//! rather than "everything with a dot in it" — which is what people who have
//! used file managers expect, and what Total Commander does.

/// A set of masks: what to look for, and what to leave out.
///
/// The text is what someone typed into the search box:
///
/// ```text
///     *.txt            one mask
///     *.txt;*.md       either of two
///     *.rs|test_*      every .rs file except the ones starting with test_
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masks {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl Masks {
    /// Reads what was typed. Anything unusable just means "match everything",
    /// because an empty search box should find things, not nothing.
    pub fn parse(text: &str) -> Masks {
        let (wanted, unwanted) = text.split_once('|').unwrap_or((text, ""));
        let mut include = split(wanted);
        // Saying only what to leave out means "everything else".
        if include.is_empty() {
            include.push("*".into());
        }
        Masks {
            include,
            exclude: split(unwanted),
        }
    }

    /// Whether a file called `name` is wanted.
    pub fn matches(&self, name: &str) -> bool {
        if self.exclude.iter().any(|mask| glob(mask, name)) {
            return false;
        }
        self.include.iter().any(|mask| glob(mask, name))
    }

    /// True when nothing is filtered out at all.
    pub fn is_everything(&self) -> bool {
        self.exclude.is_empty() && self.include.iter().any(|mask| mask.as_str() == "*")
    }
}

/// Matches one mask against one name, ignoring case.
///
/// `*` stands for any run of characters including none, `?` for exactly one.
/// Everything else is literal.
fn glob(mask: &str, name: &str) -> bool {
    let mask: Vec<char> = mask.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut at_mask, mut at_name) = (0, 0);
    // Where the last `*` was, and how much of the name it had swallowed, so a
    // dead end can be retried with the star taking one character more.
    let (mut star, mut swallowed) = (None, 0);

    while at_name < name.len() {
        if at_mask < mask.len() && (mask[at_mask] == '?' || same(mask[at_mask], name[at_name])) {
            at_mask += 1;
            at_name += 1;
        } else if at_mask < mask.len() && mask[at_mask] == '*' {
            star = Some(at_mask);
            swallowed = at_name;
            at_mask += 1;
        } else if let Some(position) = star {
            at_mask = position + 1;
            swallowed += 1;
            at_name = swallowed;
        } else {
            return false;
        }
    }

    // Trailing stars are allowed to match nothing at all.
    while at_mask < mask.len() && mask[at_mask] == '*' {
        at_mask += 1;
    }
    at_mask == mask.len()
}

/// Two characters, ignoring case, for every alphabet rather than just ASCII.
fn same(one: char, other: char) -> bool {
    one == other || one.to_lowercase().eq(other.to_lowercase())
}

/// Splits `a;b;c` into masks, dropping blanks and surrounding spaces.
fn split(text: &str) -> Vec<String> {
    text.split(';')
        .map(str::trim)
        .filter(|mask| !mask.is_empty())
        // `*.*` reads literally as "must contain a dot", but every file manager
        // has always taken it to mean everything, so that's what people expect.
        .map(|mask| if mask == "*.*" { "*" } else { mask }.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(text: &str) -> Masks {
        Masks::parse(text)
    }

    #[test]
    fn a_plain_name_matches_only_itself() {
        assert!(m("notes.txt").matches("notes.txt"));
        assert!(!m("notes.txt").matches("notes.md"));
        assert!(!m("notes.txt").matches("my notes.txt"));
    }

    #[test]
    fn a_star_stands_for_any_run_of_characters() {
        assert!(m("*.txt").matches("notes.txt"));
        assert!(m("*.txt").matches(".txt"));
        assert!(!m("*.txt").matches("notes.txt.bak"));
        assert!(!m("*.txt").matches("notes.tx"));
    }

    #[test]
    fn a_question_mark_stands_for_exactly_one() {
        assert!(m("log?.txt").matches("log1.txt"));
        assert!(!m("log?.txt").matches("log.txt"));
        assert!(!m("log?.txt").matches("log12.txt"));
    }

    #[test]
    fn stars_can_appear_anywhere_and_more_than_once() {
        assert!(m("a*b*c").matches("abc"));
        assert!(m("a*b*c").matches("axxbyyc"));
        assert!(!m("a*b*c").matches("acb"));
        assert!(m("*report*").matches("2024-report-final.pdf"));
    }

    #[test]
    fn case_does_not_matter() {
        // Half the archives in the world shout their extensions.
        assert!(m("*.txt").matches("NOTES.TXT"));
        assert!(m("*.TXT").matches("notes.txt"));
        assert!(m("ReadMe*").matches("readme.md"));
    }

    #[test]
    fn several_masks_can_be_given_at_once() {
        let masks = m("*.txt;*.md");
        assert!(masks.matches("notes.txt"));
        assert!(masks.matches("notes.md"));
        assert!(!masks.matches("notes.rs"));
    }

    #[test]
    fn spaces_around_masks_are_ignored() {
        let masks = m(" *.txt ; *.md ");
        assert!(masks.matches("notes.txt"));
        assert!(masks.matches("notes.md"));
    }

    #[test]
    fn a_bar_separates_off_what_to_leave_out() {
        let masks = m("*.rs|test_*");
        assert!(masks.matches("main.rs"));
        assert!(!masks.matches("test_main.rs"), "excluded by name");
        assert!(
            !masks.matches("main.txt"),
            "not included in the first place"
        );
    }

    #[test]
    fn several_things_can_be_left_out() {
        let masks = m("*|*.tmp;*.bak");
        assert!(masks.matches("notes.txt"));
        assert!(!masks.matches("notes.tmp"));
        assert!(!masks.matches("notes.bak"));
    }

    #[test]
    fn star_dot_star_means_everything() {
        // What people expect from years of file managers, even though read
        // literally it would miss every file without a dot in its name.
        let masks = m("*.*");
        assert!(masks.matches("notes.txt"));
        assert!(masks.matches("Makefile"), "no dot, still everything");
        assert!(masks.is_everything());
    }

    #[test]
    fn an_empty_box_finds_everything() {
        for text in ["", "   ", "*"] {
            let masks = m(text);
            assert!(masks.matches("anything at all"), "{text:?}");
            assert!(masks.is_everything(), "{text:?}");
        }
    }

    #[test]
    fn leaving_things_out_without_saying_what_to_include_still_includes_the_rest() {
        let masks = m("|*.tmp");
        assert!(masks.matches("notes.txt"));
        assert!(!masks.matches("notes.tmp"));
        assert!(!masks.is_everything());
    }

    #[test]
    fn a_mask_matches_whole_names_not_parts_of_them() {
        assert!(!m("report").matches("report.txt"));
        assert!(!m("txt").matches("notes.txt"));
    }

    #[test]
    fn names_with_awkward_characters_still_work() {
        assert!(m("*.txt").matches("\u{e9}t\u{e9} 2024.txt"));
        assert!(
            m("?.txt").matches("\u{4e16}.txt"),
            "one character, three bytes"
        );
    }
}
