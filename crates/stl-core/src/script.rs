//! A reader for the small subset of Paradox script that mod descriptors (`*.mod`) use: `key="string"`, `key=word`, `key={ "a" "b" }`,
//! comments (`#`). Nested blocks are skipped over; a descriptor has none that matter.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    List(Vec<String>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// The statements of a descriptor in file order (keys may repeat).
pub fn parse(text: &str) -> Vec<(String, Value)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut t = Tokens { chars: text.chars().collect(), pos: 0 };
    let mut out = Vec::new();
    while let Some(key) = t.word() {
        if t.peek_non_space() != Some('=') {
            continue; // a stray word
        }
        t.pos = t.skip_ws();
        t.pos += 1; // '='
        match t.peek_non_space() {
            Some('{') => {
                t.pos = t.skip_ws() + 1;
                let items = t.block_items();
                out.push((key, Value::List(items)));
            }
            Some(_) => {
                if let Some(v) = t.word() {
                    out.push((key, Value::Str(v)));
                }
            }
            None => break,
        }
    }
    out
}

struct Tokens {
    chars: Vec<char>,
    pos: usize,
}

impl Tokens {
    fn skip_ws(&self) -> usize {
        let mut p = self.pos;
        while p < self.chars.len() {
            let c = self.chars[p];
            if c == '#' {
                while p < self.chars.len() && self.chars[p] != '\n' {
                    p += 1;
                }
            } else if c.is_whitespace() {
                p += 1;
            } else {
                break;
            }
        }
        p
    }

    fn peek_non_space(&self) -> Option<char> {
        self.chars.get(self.skip_ws()).copied()
    }

    /// A quoted string or a bare word; None at the end of the input.
    fn word(&mut self) -> Option<String> {
        self.pos = self.skip_ws();
        let c = *self.chars.get(self.pos)?;
        if c == '"' {
            self.pos += 1;
            let mut s = String::new();
            while self.pos < self.chars.len() {
                let c = self.chars[self.pos];
                self.pos += 1;
                match c {
                    '"' => break,
                    '\\' if self.pos < self.chars.len() => {
                        s.push(self.chars[self.pos]);
                        self.pos += 1;
                    }
                    _ => s.push(c),
                }
            }
            Some(s)
        } else if matches!(c, '=' | '{' | '}') {
            self.pos += 1;
            Some(c.to_string())
        } else {
            let start = self.pos;
            while self.pos < self.chars.len() {
                let c = self.chars[self.pos];
                if c.is_whitespace() || matches!(c, '=' | '{' | '}' | '"' | '#') {
                    break;
                }
                self.pos += 1;
            }
            Some(self.chars[start..self.pos].iter().collect())
        }
    }

    /// The words up to the matching `}`; nested blocks are skipped.
    fn block_items(&mut self) -> Vec<String> {
        let mut items = Vec::new();
        let mut depth = 1;
        while let Some(w) = self.word() {
            match w.as_str() {
                "{" => depth += 1,
                "}" => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                "=" => {}
                _ if depth == 1 => items.push(w),
                _ => {}
            }
        }
        items
    }
}

/// First string value of `key`.
pub fn get<'a>(statements: &'a [(String, Value)], key: &str) -> Option<&'a str> {
    statements.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.as_str())
}

pub fn get_list<'a>(statements: &'a [(String, Value)], key: &str) -> Vec<&'a str> {
    statements
        .iter()
        .filter(|(k, _)| k == key)
        .flat_map(|(_, v)| match v {
            Value::List(l) => l.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            Value::Str(s) => vec![s.as_str()],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_descriptor() {
        let text = "\u{feff}version=\"2.20.0\"\ntags={\n\t\"Gameplay\"\n\t\"Fixes\"\n}\nname=\"A \\\"quoted\\\" mod\" # comment\nsupported_version=\"v4.*\"\npath=\"E:/steam/workshop/content/281990/727000451\"\nremote_file_id=\"727000451\"\n";
        let s = parse(text);
        assert_eq!(get(&s, "version"), Some("2.20.0"));
        assert_eq!(get(&s, "name"), Some("A \"quoted\" mod"));
        assert_eq!(get(&s, "supported_version"), Some("v4.*"));
        assert_eq!(get(&s, "path"), Some("E:/steam/workshop/content/281990/727000451"));
        assert_eq!(get_list(&s, "tags"), vec!["Gameplay", "Fixes"]);
        assert_eq!(get(&s, "missing"), None);
    }

    #[test]
    fn bare_values_and_nested_blocks() {
        let s = parse("a=yes\nb={ 1 2 { 3 4 } 5 }\nc=\"x\"");
        assert_eq!(get(&s, "a"), Some("yes"));
        assert_eq!(get_list(&s, "b"), vec!["1", "2", "5"]);
        assert_eq!(get(&s, "c"), Some("x"));
    }
}
