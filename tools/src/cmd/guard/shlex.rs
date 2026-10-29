//! Python's `shlex.quote` and `shlex.split` (POSIX mode, no comments), which the
//! guard uses to rebuild and split command lines (git-push-guard.py L2129, L2133,
//! L3196, L3207, L3216).

/// `shlex.quote(s)`.
pub fn quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if s.chars().all(safe) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `" ".join(shlex.quote(w) for w in words)` (`shell_join`).
pub fn join<S: AsRef<str>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| quote(w.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why `shlex.split` raised `ValueError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitError(pub &'static str);

/// The states of `shlex.read_token`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Space,
    Word,
    Quote(char),
    Escape,
    Done,
}

const WHITESPACE: &str = " \t\r\n";

/// `shlex.split(s)`: `shlex(s, posix=True)` with `whitespace_split = True` and
/// `commenters = ''`, read token by token until EOF.
pub fn split(s: &str) -> Result<Vec<String>, SplitError> {
    let mut chars = s.chars();
    let mut state = State::Space;
    let mut tokens = Vec::new();
    loop {
        // read_token
        let mut token = String::new();
        let mut quoted = false;
        let mut escaped_state = State::Word;
        loop {
            let next = if state == State::Done {
                None
            } else {
                chars.next()
            };
            match state {
                State::Done => {
                    token.clear();
                    break;
                }
                State::Space => match next {
                    None => {
                        state = State::Done;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(c) => {
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some('\\') => {
                        escaped_state = State::Word;
                        state = State::Escape;
                    }
                    Some(c) if c == '\'' || c == '"' => state = State::Quote(c),
                    Some(c) => {
                        token = c.to_string();
                        state = State::Word;
                    }
                },
                State::Quote(q) => {
                    quoted = true;
                    match next {
                        None => return Err(SplitError("No closing quotation")),
                        Some(c) if c == q => state = State::Word,
                        Some('\\') if q == '"' => {
                            escaped_state = state;
                            state = State::Escape;
                        }
                        Some(c) => token.push(c),
                    }
                }
                State::Escape => {
                    let Some(c) = next else {
                        return Err(SplitError("No escaped character"));
                    };
                    if let State::Quote(q) = escaped_state {
                        if c != '\\' && c != q {
                            token.push('\\');
                        }
                    }
                    token.push(c);
                    state = escaped_state;
                }
                State::Word => match next {
                    None => {
                        state = State::Done;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(c) => {
                        state = State::Space;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some(c) if c == '\'' || c == '"' => state = State::Quote(c),
                    Some('\\') => {
                        escaped_state = State::Word;
                        state = State::Escape;
                    }
                    Some(c) => token.push(c),
                },
            }
        }
        if !quoted && token.is_empty() {
            return Ok(tokens);
        }
        tokens.push(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_matches_python() {
        // shlex.quote on CPython 3.13.
        for (s, want) in [
            ("", "''"),
            ("abc", "abc"),
            ("a b", "'a b'"),
            ("it's", "'it'\"'\"'s'"),
            ("x@y%z+1=2:3,4./-_", "x@y%z+1=2:3,4./-_"),
            ("é", "'é'"),
            ("$HOME", "'$HOME'"),
        ] {
            assert_eq!(quote(s), want, "quote({s:?})");
        }
        assert_eq!(join(&["git", "push", "a b"]), "git push 'a b'");
    }

    #[test]
    fn split_matches_python() {
        // shlex.split on CPython 3.13.
        let ok: [(&str, &[&str]); 12] = [
            ("a b  c", &["a", "b", "c"]),
            ("'a b' c", &["a b", "c"]),
            ("\"a\\\"b\" c", &["a\"b", "c"]),
            ("\"a\\xb\"", &["a\\xb"]),
            ("a\\ b", &["a b"]),
            ("''", &[""]),
            ("x'' y", &["x", "y"]),
            ("a#b #c", &["a#b", "#c"]),
            ("push origin\tmain\n", &["push", "origin", "main"]),
            ("", &[]),
            ("  ", &[]),
            ("a'b'\"c\"d", &["abcd"]),
        ];
        for (s, want) in ok {
            assert_eq!(
                split(s),
                Ok(want.iter().map(|w| w.to_string()).collect()),
                "split({s:?})"
            );
        }
        assert_eq!(split("'a"), Err(SplitError("No closing quotation")));
        assert_eq!(split("a\\"), Err(SplitError("No escaped character")));
    }
}
