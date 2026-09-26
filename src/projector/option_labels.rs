//! Readable labels for one decision's options (hivemind-hk5z).
//!
//! An option label is meant to be a short human phrase. Older captures stored something else in
//! that slot: a slug (`name-a-upheld`), a slug with the answer's letter inside it
//! (`q1-a-drop-the-line`), or no label at all, only an id that is itself a slug of the label
//! (`option-channel-a-claude-code-plugin-first-<uuid>`). The ledger keeps what was recorded;
//! this turns it into words once, at projection, so every reader shows `Upheld` and `Standing`
//! and never `name-a-upheld`. Labels that are already words pass through untouched.

/// A label the projector found for one option: the text, and whether it was recovered from a
/// legacy option id (always slug-origin) rather than recorded as the option's label.
pub(super) struct FoundLabel {
    pub(super) text: String,
    pub(super) recovered_from_id: bool,
}

/// One entry per option, in `option_ids` order; `None` stays `None` (nothing to show but the id).
pub(super) fn readable_option_labels(found: &[Option<FoundLabel>]) -> Vec<Option<String>> {
    let words: Vec<Option<Vec<&str>>> = found
        .iter()
        .map(|label| {
            let label = label.as_ref()?;
            (label.recovered_from_id || is_slug_shaped(&label.text))
                .then(|| split_words(&label.text))
        })
        .collect();

    let letter_prefix = letter_code_prefix_len(&words);

    found
        .iter()
        .zip(&words)
        .map(|(label, words)| {
            let label = label.as_ref()?;
            let Some(words) = words else {
                return Some(label.text.clone());
            };
            let words = strip_uuid_fragments(words.get(letter_prefix..).unwrap_or_default());
            if words.is_empty() {
                return Some(label.text.clone());
            }
            Some(capitalize(&words.join(" ")))
        })
        .collect()
}

/// Lowercase letters and digits joined by `-` or `_`, no spaces, at least one joint and one
/// letter: `direct-cli`, `q1-a-drop-the-line`. A lone lowercase word (`sqlite`) is a fine label
/// and is not a slug.
fn is_slug_shaped(label: &str) -> bool {
    let mut has_joint = false;
    let mut has_letter = false;
    let mut previous_was_joint = true;
    for c in label.chars() {
        match c {
            '-' | '_' => {
                if previous_was_joint {
                    return false;
                }
                has_joint = true;
                previous_was_joint = true;
            }
            c if c.is_ascii_lowercase() => {
                has_letter = true;
                previous_was_joint = false;
            }
            c if c.is_ascii_digit() => previous_was_joint = false,
            _ => return false,
        }
    }
    has_joint && has_letter && !previous_was_joint
}

fn split_words(label: &str) -> Vec<&str> {
    label
        .split(['-', '_', ' '])
        .filter(|word| !word.is_empty())
        .collect()
}

/// How many leading words to drop from every option because they only say which answer it was:
/// a stem shared by all options (`name`, `q1`, `channel`) plus the answer's letter (`a`, `b`, `c`).
/// Fires only when every option is slug-origin and the letters are exactly `a`, `b`, `c`... one
/// per option, so `use-x-ray-tool` next to `use-y-axis-scale` is left alone. The letter names
/// the choice, and the choice is recorded once, as the chosen option, never inside a label.
fn letter_code_prefix_len(words: &[Option<Vec<&str>>]) -> usize {
    let Some(all_words) = words
        .iter()
        .map(|words| words.as_deref())
        .collect::<Option<Vec<&[&str]>>>()
    else {
        return 0;
    };
    let Some((first, rest)) = all_words.split_first() else {
        return 0;
    };
    if rest.is_empty() {
        return 0;
    }

    let stem_len = rest.iter().fold(first.len(), |len, other| {
        first
            .iter()
            .zip(other.iter())
            .take(len)
            .take_while(|(a, b)| a == b)
            .count()
    });

    let mut letters: Vec<char> = Vec::with_capacity(all_words.len());
    for option_words in &all_words {
        let Some(letter) = option_words
            .get(stem_len)
            .and_then(|word| single_letter(word))
        else {
            return 0;
        };
        if option_words.len() <= stem_len + 1 {
            return 0;
        }
        letters.push(letter);
    }
    letters.sort_unstable();
    let lettered_a_onwards = letters
        .iter()
        .zip('a'..)
        .all(|(letter, expected)| *letter == expected);
    if lettered_a_onwards {
        stem_len + 1
    } else {
        0
    }
}

fn single_letter(word: &str) -> Option<char> {
    let mut chars = word.chars();
    match (chars.next(), chars.next()) {
        (Some(letter), None) if letter.is_ascii_lowercase() => Some(letter),
        _ => None,
    }
}

/// Drops a trailing UUID cut off inside its last group (`18643473 3836 4f34 8006 d2ae`), the
/// leftover of an old id generator that capped ids at a fixed length. Only the exact shape
/// 8-4-4-4-(1 to 12) hex digits counts, and only when words remain in front of it.
fn strip_uuid_fragments<'a, 'b>(words: &'b [&'a str]) -> &'b [&'a str] {
    const GROUPS: usize = 5;
    let Some(start) = words.len().checked_sub(GROUPS) else {
        return words;
    };
    let (front, tail) = words.split_at(start);
    let is_uuid_prefix = tail.iter().enumerate().all(|(index, word)| {
        let len_ok = match index {
            0 => word.len() == 8,
            1..=3 => word.len() == 4,
            _ => (1..=12).contains(&word.len()),
        };
        len_ok && word.chars().all(|c| c.is_ascii_hexdigit())
    });
    if is_uuid_prefix && !front.is_empty() {
        front
    } else {
        words
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
