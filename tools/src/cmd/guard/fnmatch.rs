//! `fnmatch.fnmatchcase` (CPython 3.13 `fnmatch._translate`), for push
//! destinations that are globs (`glob_hits_protected`) and sed operands that
//! may match a protected path (`sed_glob_protected`).
//!
//! Each `[...]` set is built as the class body Python's translate builds (empty
//! ranges removed, set operators and stray hyphens escaped) and read back the way
//! `re` reads a class, so the result is Python's for every pattern rather than
//! the `regex` crate's reading of the same text.

/// One element of a translated glob (`fnmatch._translate`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobPart {
    Star,
    Any,
    Never,
    Literal(char),
    Class {
        negate: bool,
        items: Vec<(char, char)>,
    },
}

/// `fnmatch.fnmatchcase(name, pattern)` (CPython 3.13): `*` any run, `?` any
/// character, `[...]` a set (`!` negates; an unclosed `[` is literal), every
/// other character itself.
pub fn fnmatch_case(name: &str, pattern: &str) -> bool {
    let parts = glob_parts(pattern);
    let chars: Vec<char> = name.chars().collect();
    glob_match(&parts, &chars)
}

/// Whether `parts` matches all of `s`, in O(parts x s) time: `ok[j]` is whether
/// the parts from the current one on match `s[j..]`.
fn glob_match(parts: &[GlobPart], s: &[char]) -> bool {
    let n = s.len();
    let mut ok = vec![false; n + 1];
    ok[n] = true;
    for part in parts.iter().rev() {
        let mut next = vec![false; n + 1];
        if *part == GlobPart::Star {
            let mut any = false;
            for j in (0..=n).rev() {
                any = any || ok[j];
                next[j] = any;
            }
        } else {
            for j in 0..n {
                next[j] = ok[j + 1] && part_matches(part, s[j]);
            }
        }
        ok = next;
    }
    ok[0]
}

fn part_matches(part: &GlobPart, c: char) -> bool {
    match part {
        GlobPart::Star | GlobPart::Any => true,
        GlobPart::Never => false,
        GlobPart::Literal(l) => *l == c,
        GlobPart::Class { negate, items } => {
            items.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negate
        }
    }
}

/// `fnmatch._translate`, keeping each set as the class body Python builds and
/// reading it back as `re` reads it ([`class_items`]).
fn glob_parts(pattern: &str) -> Vec<GlobPart> {
    let pat: Vec<char> = pattern.chars().collect();
    let n = pat.len();
    let mut res = Vec::new();
    let mut i = 0;
    while i < n {
        let c = pat[i];
        i += 1;
        match c {
            '*' => {
                if res.last() != Some(&GlobPart::Star) {
                    res.push(GlobPart::Star);
                }
            }
            '?' => res.push(GlobPart::Any),
            '[' => {
                let mut j = i;
                if j < n && pat[j] == '!' {
                    j += 1;
                }
                if j < n && pat[j] == ']' {
                    j += 1;
                }
                while j < n && pat[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    res.push(GlobPart::Literal('['));
                    continue;
                }
                let stuff = class_body(&pat, i, j);
                i = j + 1;
                if stuff.is_empty() {
                    res.push(GlobPart::Never);
                } else if stuff == ['!'] {
                    res.push(GlobPart::Any);
                } else {
                    res.push(class_items(&stuff));
                }
            }
            _ => res.push(GlobPart::Literal(c)),
        }
    }
    res
}

/// The class body `fnmatch._translate` builds for `pat[i..j]`, before its `!`
/// becomes `^`: backslashes escaped, hyphens that do not make a range escaped,
/// empty ranges removed, and `&`, `~` and `|` escaped.
fn class_body(pat: &[char], start: usize, j: usize) -> Vec<char> {
    let escape = |chunk: &[char], hyphens: bool| -> Vec<char> {
        let mut out = Vec::new();
        for &ch in chunk {
            if ch == '\\' || (hyphens && ch == '-') {
                out.push('\\');
            }
            out.push(ch);
        }
        out
    };
    let raw = &pat[start..j];
    let joined: Vec<char> = if !raw.contains(&'-') {
        escape(raw, false)
    } else {
        let mut i = start;
        let mut chunks: Vec<Vec<char>> = Vec::new();
        let mut k = if pat[i] == '!' { i + 2 } else { i + 1 };
        loop {
            let found = (k..j).find(|&p| pat[p] == '-');
            let Some(h) = found else { break };
            chunks.push(pat[i..h].to_vec());
            i = h + 1;
            k = h + 3;
        }
        let chunk = pat[i..j].to_vec();
        if !chunk.is_empty() {
            chunks.push(chunk);
        } else if let Some(last) = chunks.last_mut() {
            last.push('-');
        }
        let mut k = chunks.len().saturating_sub(1);
        while k > 0 {
            let prev_last = *chunks[k - 1].last().expect("a non-empty chunk");
            if prev_last > chunks[k][0] {
                let mut merged = chunks[k - 1][..chunks[k - 1].len() - 1].to_vec();
                merged.extend_from_slice(&chunks[k][1..]);
                chunks[k - 1] = merged;
                chunks.remove(k);
            }
            k -= 1;
        }
        let mut out = Vec::new();
        for (n, chunk) in chunks.iter().enumerate() {
            if n > 0 {
                out.push('-');
            }
            out.extend(escape(chunk, true));
        }
        out
    };
    let mut stuff = Vec::new();
    for ch in joined {
        if "&~|".contains(ch) {
            stuff.push('\\');
        }
        stuff.push(ch);
    }
    stuff
}

/// A class body read the way `re` reads `[...]`: a leading `!` (Python's `^`)
/// negates; `\x` is the character x; `a-b` is a range.
fn class_items(stuff: &[char]) -> GlobPart {
    let (negate, body) = match stuff.split_first() {
        Some(('!', rest)) => (true, rest),
        _ => (false, stuff),
    };
    let mut items = Vec::new();
    let mut p = 0;
    let item = |p: &mut usize| -> Option<(char, bool)> {
        let c = *body.get(*p)?;
        *p += 1;
        if c == '\\' {
            let e = *body.get(*p)?;
            *p += 1;
            return Some((e, true));
        }
        Some((c, false))
    };
    while let Some((lo, _)) = item(&mut p) {
        if body.get(p) == Some(&'-') {
            p += 1;
            match item(&mut p) {
                // A range whose end is past the body ends at Python's `]`: the
                // hyphen is literal.
                None => {
                    items.push((lo, lo));
                    items.push(('-', '-'));
                }
                // translate removes ranges whose start is past their end, so
                // every range here is ordered.
                Some((hi, _)) => items.push((lo, hi)),
            }
        } else {
            items.push((lo, lo));
        }
    }
    GlobPart::Class { negate, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnmatch_matches_python() {
        // fnmatch.fnmatchcase(name, pattern) on CPython 3.13.
        for (name, pattern, want) in [
            ("main", "*", true),
            ("main", "m*", true),
            ("main", "*n", true),
            ("main", "ma?n", true),
            ("main", "m[a-c]in", true),
            ("main", "m[!a]in", false),
            ("main", "m[!b]in", true),
            ("main", "m[c-a]in", false),
            ("main", "[z-a-m]ain", true),
            ("main", "[]m]ain", true),
            ("main", "[!]ain", false),
            ("main", "[!]main", false),
            ("main", "[m", false),
            ("[m", "[m", true),
            ("main", "[a-]ain", false),
            ("-ain", "[a-]ain", true),
            ("main", "[l-n]a*", true),
            ("main", "m[\\]ain", false),
            ("main", "*[!]*", false),
            ("main", "*[]*", false),
            ("main", "m[b-a-z]in", false),
            ("main", "**a**", true),
            ("MAIN", "M[A-Z]IN", true),
            ("main", "m[&~|a]in", true),
        ] {
            assert_eq!(
                fnmatch_case(name, pattern),
                want,
                "fnmatchcase({name:?}, {pattern:?})"
            );
        }
    }
}
