//! Minimal reader for Valve's text KeyValues format (`libraryfolders.vdf`, `appmanifest_*.acf`).
//! Only what those two files need: quoted and bare tokens, nested blocks, `//` comments.

/// A value in a KeyValues file: either a string or a block of key and value pairs.
/// Blocks keep their order and may repeat keys, exactly like the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// First child with this key. Steam is not consistent about key case, so the
    /// comparison ignores it.
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        self.entries().iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }

    /// String value of a child, `None` if it is missing or a block.
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    /// Children of a block, empty for a string.
    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(entries) => entries,
            Vdf::Str(_) => &[],
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Text(String),
    Open,
    Close,
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() || c == '\u{feff}' => {
                chars.next();
            }
            '{' => {
                chars.next();
                tokens.push(Token::Open);
            }
            '}' => {
                chars.next();
                tokens.push(Token::Close);
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                loop {
                    match chars.next() {
                        None => return Err("unterminated string".into()),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => return Err("unterminated escape".into()),
                        },
                        Some(other) => s.push(other),
                    }
                }
                tokens.push(Token::Text(s));
            }
            '/' => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                } else {
                    // A bare token that happens to start with a slash.
                    let mut s = String::from('/');
                    read_bare(&mut chars, &mut s);
                    tokens.push(Token::Text(s));
                }
            }
            _ => {
                let mut s = String::new();
                read_bare(&mut chars, &mut s);
                tokens.push(Token::Text(s));
            }
        }
    }
    Ok(tokens)
}

fn read_bare(chars: &mut std::iter::Peekable<std::str::Chars>, out: &mut String) {
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
            break;
        }
        out.push(c);
        chars.next();
    }
}

/// Deeper nesting than this is not a Steam file but garbage, stop instead of recursing on.
const MAX_DEPTH: usize = 32;

fn parse_block(tokens: &[Token], pos: &mut usize, depth: usize, top: bool) -> Result<Vdf, String> {
    if depth > MAX_DEPTH {
        return Err("nested too deeply".into());
    }
    let mut entries = Vec::new();
    loop {
        let key = match tokens.get(*pos) {
            None if top => break,
            None => return Err("missing closing brace".into()),
            Some(Token::Close) if !top => {
                *pos += 1;
                break;
            }
            Some(Token::Close) => return Err("closing brace without a block".into()),
            Some(Token::Open) => return Err("block without a key".into()),
            Some(Token::Text(key)) => key.clone(),
        };
        *pos += 1;
        let value = match tokens.get(*pos) {
            Some(Token::Text(value)) => {
                *pos += 1;
                Vdf::Str(value.clone())
            }
            Some(Token::Open) => {
                *pos += 1;
                parse_block(tokens, pos, depth + 1, false)?
            }
            _ => return Err(format!("key '{key}' has no value")),
        };
        entries.push((key, value));
    }
    Ok(Vdf::Obj(entries))
}

/// Parses a whole file. The result is a block holding the top level pairs, for Steam
/// files that is a single pair such as `"AppState" { ... }`.
pub fn parse(text: &str) -> Result<Vdf, String> {
    let tokens = tokenize(text)?;
    let mut pos = 0;
    parse_block(&tokens, &mut pos, 0, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_blocks_and_escapes() {
        let v = parse("\"a\"\n{\n\t\"path\"\t\t\"C:\\\\Games\\\\X\"\n\t\"b\" { \"c\" \"1\" }\n}\n").unwrap();
        let a = v.get("a").unwrap();
        assert_eq!(a.str("path"), Some("C:\\Games\\X"));
        assert_eq!(a.get("b").unwrap().str("c"), Some("1"));
    }

    #[test]
    fn comments_bare_tokens_and_case() {
        let v = parse("// header\nRoot { Key value // trailing\n \"Other\" \"x y\" }").unwrap();
        let root = v.get("root").unwrap();
        assert_eq!(root.str("KEY"), Some("value"));
        assert_eq!(root.str("other"), Some("x y"));
    }

    #[test]
    fn broken_input_is_an_error() {
        assert!(parse("\"a\" {").is_err());
        assert!(parse("\"a\" \"unterminated").is_err());
        assert!(parse("}").is_err());
        assert!(parse("\"key\"").is_err());
    }
}
