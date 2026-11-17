//! `.plexignore` (`docs/scanner/discovery.md` §3.3).

use crate::files::RelativePath;

/// The rules of a `.plexignore`.
#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    /// Globs on a file's name, lower-cased.
    names: Vec<Vec<char>>,
    /// Globs on a path below the `.plexignore`'s directory, one per component, lower-cased.
    paths: Vec<Vec<Vec<char>>>,
}

impl IgnoreRules {
    pub fn parse(text: &str) -> Self {
        let mut rules = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let pattern = line
                .strip_prefix('\\')
                .filter(|rest| rest.starts_with('#'))
                .unwrap_or(line);
            let lower = pattern.to_lowercase();
            if lower.contains('/') {
                // The legacy scanner dropped a rule with a leading `/`; it is read here as
                // relative to the `.plexignore`, which is what anyone writing it means.
                let components: Vec<Vec<char>> = lower
                    .split('/')
                    .filter(|component| !component.is_empty())
                    .map(|component| component.chars().collect())
                    .collect();
                if !components.is_empty() {
                    rules.paths.push(components);
                }
            } else {
                rules.names.push(lower.chars().collect());
            }
        }
        rules
    }

    /// Whether `path`, relative to the directory holding the `.plexignore`, is ignored.
    pub fn ignores(&self, path: &RelativePath) -> bool {
        let components: Vec<Vec<char>> = path
            .components()
            .map(|component| component.to_lowercase().chars().collect())
            .collect();
        let Some(name) = components.last() else {
            return false;
        };
        if self.names.iter().any(|rule| glob_matches(rule, name)) {
            return true;
        }
        // A rule ignores what it names and everything below it. `Dir/*` also ignores `Dir`
        // itself, which the reference logs as the directory ignored.
        self.paths.iter().any(|rule| {
            let directory = match rule.split_last() {
                Some((last, parent)) if last.iter().all(|c| *c == '*') && !parent.is_empty() => {
                    Some(parent)
                }
                _ => None,
            };
            (1..=components.len()).any(|depth| {
                let ancestor = &components[..depth];
                matches_components(rule, ancestor)
                    || directory.is_some_and(|directory| matches_components(directory, ancestor))
            })
        })
    }
}

fn matches_components(rule: &[Vec<char>], components: &[Vec<char>]) -> bool {
    rule.len() == components.len()
        && rule
            .iter()
            .zip(components)
            .all(|(pattern, component)| glob_matches(pattern, component))
}

/// `fnmatch`: `*`, `?`, `[seq]` and `[!seq]`; an unclosed `[` is itself.
fn glob_matches(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        let step = match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') => match class(pattern, p, text[t]) {
                Some((true, length)) => Some(length),
                Some((false, _)) => None,
                None => (text[t] == '[').then_some(1),
            },
            Some(c) => (*c == text[t]).then_some(1),
            None => None,
        };
        if let Some(length) = step {
            p += length;
            t += 1;
        } else if let Some((star_at, matched_to)) = star {
            p = star_at + 1;
            t = matched_to + 1;
            star = Some((star_at, matched_to + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// Whether the class opening at `start` admits `c`, and its length; `None` when it is not
/// closed.
fn class(pattern: &[char], start: usize, c: char) -> Option<(bool, usize)> {
    let mut index = start + 1;
    let negated = pattern.get(index) == Some(&'!');
    if negated {
        index += 1;
    }
    let first = index;
    let mut admitted = false;
    loop {
        let current = *pattern.get(index)?;
        if current == ']' && index > first {
            break;
        }
        if pattern.get(index + 1) == Some(&'-')
            && pattern.get(index + 2).is_some_and(|end| *end != ']')
        {
            let end = pattern[index + 2];
            admitted |= (current..=end).contains(&c);
            index += 3;
        } else {
            admitted |= current == c;
            index += 1;
        }
    }
    Some((admitted != negated, index + 1 - start))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn path(text: &str) -> RelativePath {
        RelativePath::parse(text).unwrap()
    }

    #[test]
    fn ignores_files_by_name_without_case() {
        let rules = IgnoreRules::parse("# comment\n\nIgnored*.mkv\n");
        assert!(rules.ignores(&path("Ignored Film (2000).mkv")));
        assert!(rules.ignores(&path("Sub/ignored.MKV")));
        assert!(!rules.ignores(&path("Kept (2000).mkv")));
    }

    #[test]
    fn ignores_a_directory_named_by_a_path_rule() {
        let rules = IgnoreRules::parse("IgnSub/*\n");
        assert!(rules.ignores(&path("IgnSub")));
        assert!(rules.ignores(&path("IgnSub/a.mkv")));
        assert!(rules.ignores(&path("IgnSub/deeper/b.mkv")));
        assert!(!rules.ignores(&path("Kept (2000).mkv")));
        assert!(!rules.ignores(&path("Other/IgnSub/a.mkv")));
    }

    #[test]
    fn reads_an_escaped_hash_as_a_name() {
        let rules = IgnoreRules::parse("\\#1*.mkv");
        assert!(rules.ignores(&path("#1 Film.mkv")));
    }

    #[test]
    fn matches_classes_and_single_characters() {
        let rules = IgnoreRules::parse("film[0-9]?.mkv\n[!a]*.avi");
        assert!(rules.ignores(&path("film1a.mkv")));
        assert!(!rules.ignores(&path("filmx1.mkv")));
        assert!(rules.ignores(&path("b.avi")));
        assert!(!rules.ignores(&path("a.avi")));
    }
}
