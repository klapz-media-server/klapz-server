//! Stacking: the parts of one copy split across files (`cd1`, `pt2`, `1 of 2`), by the legacy
//! rule the reference still follows (`docs/scanner/discovery.md` §4.4).

/// Files that make one copy, in part order, and the name they share when stacked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stack {
    pub parts: Vec<usize>,
    /// The common prefix with its part marker stripped (`Heat (1995) -`), for a stack of
    /// several files.
    pub root: Option<String>,
}

/// The characters a part number may be, from `1` to `n`.
const PART_MARKS: &str = "123456789abcdefghijklmn";

const PART_WORDS: [&str; 7] = ["cd", "dvd", "part", "pt", "disk", "disc", "scene"];

/// Groups `sorted` (indices into `files`, in name order) into stacks of adjacent names.
///
/// `x_of_y` admits `1 of 2` without a part word: films stack that way, but an episode's
/// `15 of 26` numbers it among a season.
pub(super) fn stacks(files: &[&str], sorted: &[usize], x_of_y: bool) -> Vec<Stack> {
    let mut stacks: Vec<Stack> = Vec::new();
    for (position, &index) in sorted.iter().enumerate() {
        let root = position
            .checked_sub(1)
            .and_then(|previous| stack_root(files[sorted[previous]], files[index], x_of_y));
        if let (Some(root), Some(last)) = (root, stacks.last_mut()) {
            if last.root.as_ref().is_none_or(|current| *current == root) {
                last.root = Some(root);
                last.parts.push(index);
                continue;
            }
        }
        stacks.push(Stack {
            parts: vec![index],
            root: None,
        });
    }
    stacks
}

/// The root two names stack under: they differ by one part mark, neither first nor last,
/// after a part word or before `of`.
fn stack_root(first: &str, second: &str, x_of_y: bool) -> Option<String> {
    let left: Vec<char> = first.chars().collect();
    let right: Vec<char> = second.chars().collect();
    if left.len() != right.len() {
        return None;
    }
    let mut differences = left
        .iter()
        .zip(&right)
        .enumerate()
        .filter(|(_, (a, b))| a != b);
    let (at, _) = differences.next()?;
    if differences.next().is_some()
        || at == 0
        || at + 1 == left.len()
        || !PART_MARKS.contains(left[at].to_ascii_lowercase())
    {
        return None;
    }

    let numbered = left[at + 1..]
        .iter()
        .collect::<String>()
        .trim()
        .to_lowercase()
        .starts_with("of");
    let mut root: String = left[..at].iter().collect();
    // `part 01` / `part 02`: the zero belongs to the number, not the root.
    if root.ends_with('0') {
        root.pop();
    }
    let trimmed = root.trim();
    let lower = trimmed.to_lowercase();
    // `Kill Bill Vol 1` and `Vol 2` are two films.
    if lower.ends_with("vol") || lower.ends_with("volume") {
        return None;
    }
    let word = PART_WORDS.iter().find(|word| {
        lower.strip_suffix(**word).is_some_and(|before| {
            // The legacy rule took any ending (`Concept A` / `Concept B` stacked on `pt`); the
            // native pattern wants the word on its own, as every measured stack has it.
            !before
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
        })
    });
    match word {
        Some(word) => trimmed
            .get(..trimmed.len() - word.len())
            .map(|root| root.trim().to_owned()),
        None if numbered && x_of_y => Some(trimmed.to_owned()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stacked(names: &[&str], x_of_y: bool) -> Vec<Vec<usize>> {
        let sorted: Vec<usize> = (0..names.len()).collect();
        stacks(names, &sorted, x_of_y)
            .into_iter()
            .map(|stack| stack.parts)
            .collect()
    }

    #[test]
    fn stacks_the_measured_part_forms() {
        for pair in [
            ["Heat (1995) - pt1.mkv", "Heat (1995) - pt2.mkv"],
            ["Stacked Film (2008) cd1.mkv", "Stacked Film (2008) cd2.mkv"],
            ["Twin (2001) - 1 of 2.mkv", "Twin (2001) - 2 of 2.mkv"],
            ["Film part01.mkv", "Film part02.mkv"],
        ] {
            assert_eq!(stacked(&pair, true), [vec![0, 1]], "{pair:?}");
        }
    }

    #[test]
    fn keeps_apart_what_the_reference_kept_apart() {
        for pair in [
            ["Movie.Name.2014.Part.1.mkv", "Movie.Name.2014.Part.2.mkv"],
            ["Kill Bill Vol 1 (2003).mkv", "Kill Bill Vol 2 (2004).mkv"],
            ["Letter A (1999).mkv", "Letter B (1999).mkv"],
            ["Concept A (1999).mkv", "Concept B (1999).mkv"],
            ["Show S01E01.mkv", "Show S01E02.mkv"],
        ] {
            assert_eq!(stacked(&pair, true), [vec![0], vec![1]], "{pair:?}");
        }
    }

    #[test]
    fn stacks_three_parts_under_one_root() {
        let names = ["F cd1.mkv", "F cd2.mkv", "F cd3.mkv"];
        let stacks = stacks(&names, &[0, 1, 2], true);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].parts, [0, 1, 2]);
        assert_eq!(stacks[0].root.as_deref(), Some("F"));
    }

    #[test]
    fn numbers_out_of_a_total_are_episodes_not_parts() {
        let names = ["Show 01 of 26.mkv", "Show 02 of 26.mkv"];
        assert_eq!(stacked(&names, false), [vec![0], vec![1]]);
    }
}
