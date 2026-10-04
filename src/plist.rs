//! Apple property lists: a writer for `Info.plist` and a small XML reader.
//!
//! The reader exists so tests (and the pipeline's self-check) can verify what we
//! produced instead of trusting the writer; it understands the XML plist subset
//! Apple emits, which also lets `doctor` read `SDKSettings.plist`.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::tomlite::{Table, Value as TomlValue};

/// A plist value.
#[derive(Debug, Clone, PartialEq)]
pub enum Plist {
    String(String),
    Integer(i64),
    Real(f64),
    Boolean(bool),
    Array(Vec<Plist>),
    Dict(BTreeMap<String, Plist>),
    Data(Vec<u8>),
}

impl Plist {
    pub fn dict(entries: Vec<(&str, Plist)>) -> Plist {
        let mut map = BTreeMap::new();
        for (key, value) in entries {
            map.insert(key.to_string(), value);
        }
        Plist::Dict(map)
    }

    pub fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(map) => map.get(key),
            _ => None,
        }
    }

    /// Owned `(key, value)` pairs of a dict, or an empty list for other types.
    pub fn as_dict_entries(&self) -> Vec<(String, Plist)> {
        match self {
            Plist::Dict(map) => {
                map.iter().map(|(key, value)| (key.clone(), value.clone())).collect()
            }
            _ => Vec::new(),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Plist::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Plist]> {
        match self {
            Plist::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Plist::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// Convert a `[app.info_plist]` table from `darwinforge.toml` into a plist value.
    pub fn from_toml(table: &Table) -> Result<Plist> {
        let mut map = BTreeMap::new();
        for (key, value) in table {
            map.insert(key.clone(), Plist::from_toml_value(value)?);
        }
        Ok(Plist::Dict(map))
    }

    fn from_toml_value(value: &TomlValue) -> Result<Plist> {
        Ok(match value {
            TomlValue::String(value) => Plist::String(value.clone()),
            TomlValue::Integer(value) => Plist::Integer(*value),
            TomlValue::Float(value) => Plist::Real(*value),
            TomlValue::Boolean(value) => Plist::Boolean(*value),
            TomlValue::Array(items) => {
                let mut converted = Vec::with_capacity(items.len());
                for item in items {
                    converted.push(Plist::from_toml_value(item)?);
                }
                Plist::Array(converted)
            }
            TomlValue::Table(table) => Plist::from_toml(table)?,
        })
    }
}

/// Serialise a plist as XML (the format `Info.plist` uses).
pub fn to_xml(root: &Plist) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
    );
    out.push_str("<plist version=\"1.0\">\n");
    write_value(&mut out, root, 0);
    out.push_str("</plist>\n");
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

fn write_value(out: &mut String, value: &Plist, depth: usize) {
    match value {
        Plist::String(text) => {
            indent(out, depth);
            out.push_str("<string>");
            escape_into(out, text);
            out.push_str("</string>\n");
        }
        Plist::Integer(number) => {
            indent(out, depth);
            out.push_str(&format!("<integer>{number}</integer>\n"));
        }
        Plist::Real(number) => {
            indent(out, depth);
            out.push_str(&format!("<real>{number}</real>\n"));
        }
        Plist::Boolean(flag) => {
            indent(out, depth);
            out.push_str(if *flag { "<true/>\n" } else { "<false/>\n" });
        }
        Plist::Data(bytes) => {
            indent(out, depth);
            out.push_str("<data>");
            out.push_str(&base64_encode(bytes));
            out.push_str("</data>\n");
        }
        Plist::Array(items) => {
            if items.is_empty() {
                indent(out, depth);
                out.push_str("<array/>\n");
                return;
            }
            indent(out, depth);
            out.push_str("<array>\n");
            for item in items {
                write_value(out, item, depth + 1);
            }
            indent(out, depth);
            out.push_str("</array>\n");
        }
        Plist::Dict(map) => {
            if map.is_empty() {
                indent(out, depth);
                out.push_str("<dict/>\n");
                return;
            }
            indent(out, depth);
            out.push_str("<dict>\n");
            for (key, item) in map {
                indent(out, depth + 1);
                out.push_str("<key>");
                escape_into(out, key);
                out.push_str("</key>\n");
                write_value(out, item, depth + 1);
            }
            indent(out, depth);
            out.push_str("</dict>\n");
        }
    }
}

fn escape_into(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(character),
        }
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Parse an XML plist.
pub fn parse_xml(text: &str) -> Result<Plist> {
    let chars: Vec<char> = text.chars().collect();
    let mut cursor = 0usize;
    parse_value(&chars, &mut cursor)
}

fn fail<T>(message: &str, cursor: usize) -> Result<T> {
    Err(Error::format("plist", format!("{message} (at character {cursor})")))
}

fn skip_whitespace(chars: &[char], cursor: &mut usize) {
    while *cursor < chars.len() && chars[*cursor].is_whitespace() {
        *cursor += 1;
    }
}

/// What a `<...>` token turned out to be.
enum Token {
    /// An opening tag, possibly self-closing (`<key>`, `<dict/>`).
    Open { name: String, self_closing: bool },
    /// A closing tag (`</array>`).
    Close(String),
}

/// Consume one `<...>` token, skipping the XML declaration, comments and the
/// doctype.
fn parse_tag(chars: &[char], cursor: &mut usize) -> Result<Token> {
    skip_whitespace(chars, cursor);
    if *cursor >= chars.len() || chars[*cursor] != '<' {
        return fail("expected `<`", *cursor);
    }
    // Skip the XML declaration, comments and the doctype.
    if chars.get(*cursor + 1) == Some(&'?') || chars.get(*cursor + 1) == Some(&'!') {
        while *cursor < chars.len() && chars[*cursor] != '>' {
            *cursor += 1;
        }
        *cursor += 1;
        return parse_tag(chars, cursor);
    }
    *cursor += 1;
    if chars.get(*cursor) == Some(&'/') {
        // Closing tag: `</name>`.
        *cursor += 1;
        let start = *cursor;
        while *cursor < chars.len()
            && !chars[*cursor].is_whitespace()
            && chars[*cursor] != '>'
        {
            *cursor += 1;
        }
        let name: String = chars[start..*cursor].iter().collect();
        while *cursor < chars.len() && chars[*cursor] != '>' {
            *cursor += 1;
        }
        *cursor += 1;
        return Ok(Token::Close(name));
    }
    let start = *cursor;
    while *cursor < chars.len()
        && !chars[*cursor].is_whitespace()
        && chars[*cursor] != '>'
        && chars[*cursor] != '/'
    {
        *cursor += 1;
    }
    let name: String = chars[start..*cursor].iter().collect();
    let mut self_closing = false;
    while *cursor < chars.len() && chars[*cursor] != '>' {
        if chars[*cursor] == '/' {
            self_closing = true;
        }
        *cursor += 1;
    }
    *cursor += 1;
    Ok(Token::Open { name, self_closing })
}

fn parse_value(chars: &[char], cursor: &mut usize) -> Result<Plist> {
    match parse_tag(chars, cursor)? {
        Token::Close(name) => fail(&format!("unexpected </{name}> where a value was expected"), *cursor),
        Token::Open { name, self_closing } => parse_tagged_value(chars, cursor, &name, self_closing),
    }
}

/// Parse a value whose opening tag has already been consumed.
fn parse_tagged_value(
    chars: &[char],
    cursor: &mut usize,
    tag: &str,
    self_closing: bool,
) -> Result<Plist> {
    match tag {
        "plist" => parse_value(chars, cursor),
        "dict" if self_closing => Ok(Plist::Dict(BTreeMap::new())),
        "array" if self_closing => Ok(Plist::Array(Vec::new())),
        "true" => Ok(Plist::Boolean(true)),
        "false" => Ok(Plist::Boolean(false)),
        "dict" => parse_dict(chars, cursor),
        "array" => parse_array(chars, cursor),
        "string" => Ok(Plist::String(parse_text(chars, cursor, "string")?)),
        "integer" => {
            let text = parse_text(chars, cursor, "integer")?;
            let trimmed = text.trim().to_string();
            trimmed.parse().map(Plist::Integer).map_err(|_| {
                Error::format("plist", format!("<integer> content `{trimmed}` is not an integer"))
            })
        }
        "real" => {
            let text = parse_text(chars, cursor, "real")?;
            let trimmed = text.trim().to_string();
            trimmed
                .parse()
                .map(Plist::Real)
                .map_err(|_| Error::format("plist", "<real> content is not a number"))
        }
        "data" => Ok(Plist::Data(base64_decode(parse_text(chars, cursor, "data")?.trim())?)),
        other => fail(&format!("unsupported plist tag <{other}>"), *cursor),
    }
}

fn parse_text(chars: &[char], cursor: &mut usize, tag: &str) -> Result<String> {
    let closing = format!("</{tag}>");
    let start = *cursor;
    while *cursor < chars.len() {
        if chars[*cursor] == '<' {
            let end = (*cursor + closing.len()).min(chars.len());
            let candidate: String = chars[*cursor..end].iter().collect();
            if candidate == closing {
                let raw: String = chars[start..*cursor].iter().collect();
                *cursor = end;
                return Ok(unescape(&raw));
            }
        }
        *cursor += 1;
    }
    fail(&format!("unterminated <{tag}>"), *cursor)
}

fn parse_dict(chars: &[char], cursor: &mut usize) -> Result<Plist> {
    let mut map = BTreeMap::new();
    loop {
        match parse_tag(chars, cursor)? {
            Token::Open { name, self_closing } if name == "key" && !self_closing => {
                let key = parse_text(chars, cursor, "key")?;
                let value = parse_value(chars, cursor)?;
                map.insert(key, value);
            }
            Token::Open { name, self_closing } if name == "dict" && self_closing => {
                return Ok(Plist::Dict(map))
            }
            Token::Close(name) if name == "dict" => return Ok(Plist::Dict(map)),
            Token::Open { name, .. } => {
                return fail(&format!("unexpected <{name}> inside <dict>"), *cursor)
            }
            Token::Close(name) => return fail(&format!("unexpected </{name}> inside <dict>"), *cursor),
        }
    }
}

fn parse_array(chars: &[char], cursor: &mut usize) -> Result<Plist> {
    let mut items = Vec::new();
    loop {
        match parse_tag(chars, cursor)? {
            Token::Close(name) if name == "array" => return Ok(Plist::Array(items)),
            Token::Open { name, self_closing }
                if self_closing && name == "array" =>
            {
                return Ok(Plist::Array(items))
            }
            Token::Open { name, self_closing } => {
                match name.as_str() {
                    "dict" | "string" | "integer" | "real" | "true" | "false" | "array"
                    | "data" => {
                        items.push(parse_tagged_value(chars, cursor, &name, self_closing)?);
                    }
                    other => {
                        return fail(&format!("unexpected <{other}> inside <array>"), *cursor)
                    }
                }
            }
            Token::Close(name) => {
                return fail(&format!("unexpected </{name}> inside <array>"), *cursor)
            }
        }
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#13;", "\r")
        .replace("&amp;", "&")
}

fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for character in text.chars() {
        if character == '=' || character.is_whitespace() {
            continue;
        }
        let value = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            other => {
                return Err(Error::format(
                    "plist",
                    format!("invalid base64 character `{other}` in <data>"),
                ))
            }
        };
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Ok(out)
}