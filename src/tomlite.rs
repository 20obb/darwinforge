//! A deliberately small TOML reader.
//!
//! Only the subset `darwinforge.toml` needs is supported: comments, `[table]` and
//! `[table.sub]` headers, dotted keys, basic/literal strings, integers, floats,
//! booleans, inline arrays and arrays spanning several lines. Anything else is
//! a hard error carrying a line number, which upholds the "no silent
//! misconfiguration" promise. Hand-rolling it keeps the dependency count at zero.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<Value>),
    Table(Table),
}

pub type Table = Vec<(String, Value)>;

#[derive(Debug)]
pub struct TomlError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for TomlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Integer(_) => "integer",
            Value::Float(_) => "float",
            Value::Boolean(_) => "boolean",
            Value::Array(_) => "array",
            Value::Table(_) => "table",
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Value::Table(table) => Some(table),
            _ => None,
        }
    }
}

/// Convenience lookup: `table["app"]["name"]`.
pub fn get<'a>(table: &'a Table, key: &str) -> Option<&'a Value> {
    table.iter().find(|(name, _)| name == key).map(|(_, value)| value)
}

pub fn parse(input: &str) -> Result<Value, TomlError> {
    let mut root = Table::new();
    // Path of the table that bare `key = value` lines currently belong to.
    let mut current: Vec<String> = Vec::new();
    let mut lines = input.lines().enumerate().peekable();

    while let Some((index, raw_line)) = lines.next() {
        let line_number = index + 1;
        let line = strip_comment(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            if line.starts_with("[[") {
                return Err(TomlError {
                    line: line_number,
                    message: "arrays of tables (`[[...]]`) are not supported; \
                              use a list of strings instead"
                        .to_string(),
                });
            }
            let name = header.strip_suffix(']').ok_or_else(|| TomlError {
                line: line_number,
                message: "unterminated table header (missing `]`)".to_string(),
            })?;
            current = split_key_path(name.trim(), line_number)?;
            ensure_table(&mut root, &current, line_number)?;
            continue;
        }

        let (raw_key, raw_value) = line.split_once('=').ok_or_else(|| TomlError {
            line: line_number,
            message: format!("expected `key = value`, found `{line}`"),
        })?;
        let key_path = split_key_path(raw_key.trim(), line_number)?;
        let mut value_text = raw_value.trim().to_string();

        // Continue while an array is still open.
        while open_brackets(&value_text) > 0 {
            let (_, next) = lines.next().ok_or_else(|| TomlError {
                line: line_number,
                message: "unterminated array".to_string(),
            })?;
            value_text.push(' ');
            value_text.push_str(strip_comment(next).trim());
        }

        let value = parse_value(value_text.trim(), line_number)?;
        let mut full_path = current.clone();
        full_path.extend(key_path);
        let (last, parents) = full_path.split_last().expect("key path is never empty");
        let table = ensure_table(&mut root, parents, line_number)?;
        if table.iter().any(|(existing, _)| existing == last) {
            return Err(TomlError {
                line: line_number,
                message: format!("duplicate key `{last}`"),
            });
        }
        table.push((last.clone(), value));
    }

    Ok(Value::Table(root))
}

/// Remove a trailing `# comment`, respecting quotes.
fn strip_comment(line: &str) -> &str {
    let mut in_basic = false;
    let mut in_literal = false;
    for (index, byte) in line.as_bytes().iter().enumerate() {
        match byte {
            b'"' if !in_literal => in_basic = !in_basic,
            b'\'' if !in_basic => in_literal = !in_literal,
            b'#' if !in_basic && !in_literal => return &line[..index],
            _ => {}
        }
    }
    line
}

fn open_brackets(text: &str) -> i32 {
    let mut depth = 0;
    let mut quote: Option<u8> = None;
    for byte in text.bytes() {
        match quote {
            Some(active) => {
                // Text between a pair of quotes is literal and must not affect
                // bracket depth; the closing quote re-opens scanning.
                if byte == active {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth -= 1,
                _ => {}
            },
        }
    }
    depth
}

fn split_key_path(text: &str, line: usize) -> Result<Vec<String>, TomlError> {
    let mut parts = Vec::new();
    for part in text.split('.') {
        let part = unquote_key(part.trim(), line)?;
        if part.is_empty() {
            return Err(TomlError {
                line,
                message: format!("empty key segment in `{text}`"),
            });
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(TomlError { line, message: "empty key".to_string() });
    }
    Ok(parts)
}

fn unquote_key(part: &str, line: usize) -> Result<String, TomlError> {
    if part.len() >= 2 && part.starts_with('"') && part.ends_with('"') {
        return unescape(&part[1..part.len() - 1], line);
    }
    if part.len() >= 2 && part.starts_with('\'') && part.ends_with('\'') {
        return Ok(part[1..part.len() - 1].to_string());
    }
    if part.contains(['"', '\'']) {
        return Err(TomlError { line, message: format!("malformed key `{part}`") });
    }
    Ok(part.to_string())
}

fn ensure_table<'a>(
    root: &'a mut Table,
    path: &[String],
    line: usize,
) -> Result<&'a mut Table, TomlError> {
    let mut table = root;
    for segment in path {
        let position = table.iter().position(|(key, _)| key == segment);
        let position = match position {
            Some(position) => position,
            None => {
                table.push((segment.clone(), Value::Table(Table::new())));
                table.len() - 1
            }
        };
        table = match &mut table[position].1 {
            Value::Table(child) => child,
            other => {
                return Err(TomlError {
                    line,
                    message: format!(
                        "`{segment}` is already a {}, not a table",
                        other.type_name()
                    ),
                })
            }
        };
    }
    Ok(table)
}

/// Parse a single scalar or (possibly nested) array value.
fn parse_value(text: &str, line: usize) -> Result<Value, TomlError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(TomlError { line, message: "missing value".to_string() });
    }
    if let Some(rest) = text.strip_prefix('[') {
        let inner = rest.strip_suffix(']').ok_or_else(|| TomlError {
            line,
            message: "unterminated array (missing `]`)".to_string(),
        })?;
        let mut items = Vec::new();
        for element in split_array_elements(inner, line)? {
            let element = element.trim();
            if element.is_empty() {
                continue;
            }
            items.push(parse_value(element, line)?);
        }
        return Ok(Value::Array(items));
    }
    if text.starts_with('"') {
        if !text.ends_with('"') || text.len() < 2 {
            return Err(TomlError {
                line,
                message: "unterminated basic string (missing closing `\"`)".to_string(),
            });
        }
        return Ok(Value::String(unescape(&text[1..text.len() - 1], line)?));
    }
    if text.starts_with('\'') {
        if !text.ends_with('\'') || text.len() < 2 {
            return Err(TomlError {
                line,
                message: "unterminated literal string (missing closing `'`)`".to_string(),
            });
        }
        return Ok(Value::String(text[1..text.len() - 1].to_string()));
    }
    if text == "true" {
        return Ok(Value::Boolean(true));
    }
    if text == "false" {
        return Ok(Value::Boolean(false));
    }
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    if let Ok(value) = cleaned.parse::<i64>() {
        return Ok(Value::Integer(value));
    }
    if let Ok(value) = cleaned.parse::<f64>() {
        return Ok(Value::Float(value));
    }
    Err(TomlError { line, message: format!("cannot parse value `{text}`") })
}

/// Split array elements on top-level commas only.
fn split_array_elements(inner: &str, line: usize) -> Result<Vec<String>, TomlError> {
    let mut elements = Vec::new();
    let mut current = String::new();
    let mut depth = 0_i32;
    let mut quote: Option<char> = None;
    for character in inner.chars() {
        match quote {
            Some(active) => {
                current.push(character);
                if character == active {
                    quote = None;
                }
            }
            None => match character {
                '"' | '\'' => {
                    quote = Some(character);
                    current.push(character);
                }
                '[' | '{' => {
                    depth += 1;
                    current.push(character);
                }
                ']' | '}' => {
                    depth -= 1;
                    current.push(character);
                }
                ',' if depth == 0 => elements.push(std::mem::take(&mut current)),
                other => current.push(other),
            },
        }
    }
    if quote.is_some() || depth != 0 {
        return Err(TomlError {
            line,
            message: "unterminated string or array".to_string(),
        });
    }
    if !current.trim().is_empty() {
        elements.push(current);
    }
    Ok(elements)
}

fn unescape(text: &str, line: usize) -> Result<String, TomlError> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        let escape = chars.next().ok_or_else(|| TomlError {
            line,
            message: "string ends with a dangling backslash".to_string(),
        })?;
        match escape {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '0' => out.push('\0'),
            other => {
                return Err(TomlError {
                    line,
                    message: format!("unsupported escape sequence `\\{other}`"),
                })
            }
        }
    }
    Ok(out)
}