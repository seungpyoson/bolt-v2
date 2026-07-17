#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub(crate) text: String,
    start: usize,
    end: usize,
}

pub(crate) fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index..].starts_with(b"//") {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index..].starts_with(b"/*") {
            index = scan_nested_block_comment(bytes, index);
            continue;
        }
        if let Some(end) = scan_raw_string(bytes, index) {
            index = end;
            continue;
        }
        if let Some(end) = scan_quoted_literal(bytes, index) {
            index = end;
            continue;
        }
        if let Some(end) = scan_char_or_lifetime(bytes, index) {
            index = end;
            continue;
        }
        if bytes[index..].starts_with(b"r#")
            && bytes.get(index + 2).is_some_and(|byte| ident_start(*byte))
        {
            let start = index + 2;
            index = start + 1;
            while index < bytes.len() && ident_continue(bytes[index]) {
                index += 1;
            }
            tokens.push(Token {
                text: source[start..index].to_owned(),
                start,
                end: index,
            });
            continue;
        }
        if ident_start(bytes[index]) {
            let start = index;
            index += 1;
            while index < bytes.len() && ident_continue(bytes[index]) {
                index += 1;
            }
            tokens.push(Token {
                text: source[start..index].to_owned(),
                start,
                end: index,
            });
            continue;
        }
        if bytes[index..].starts_with(b"::") {
            tokens.push(Token {
                text: "::".to_owned(),
                start: index,
                end: index + 2,
            });
            index += 2;
            continue;
        }
        tokens.push(Token {
            text: (bytes[index] as char).to_string(),
            start: index,
            end: index + 1,
        });
        index += 1;
    }
    tokens
}

fn ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn scan_nested_block_comment(bytes: &[u8], mut index: usize) -> usize {
    let mut depth = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"/*") {
            depth += 1;
            index += 2;
        } else if bytes[index..].starts_with(b"*/") {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return index;
            }
        } else {
            index += 1;
        }
    }
    index
}

fn scan_raw_string(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'b') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;
    let mut hashes = 0;
    while bytes.get(cursor) == Some(&b'#') {
        hashes += 1;
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"'
            && (1..=hashes).all(|offset| bytes.get(cursor + offset) == Some(&b'#'))
        {
            return Some(cursor + hashes + 1);
        }
        cursor += 1;
    }
    Some(cursor)
}

fn scan_quoted_literal(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'b') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' {
            cursor += 2;
        } else if bytes[cursor] == b'"' {
            return Some(cursor + 1);
        } else {
            cursor += 1;
        }
    }
    Some(cursor)
}

fn scan_char_or_lifetime(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index;
    if bytes.get(cursor) == Some(&b'b') && bytes.get(cursor + 1) == Some(&b'\'') {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'\'') {
        return None;
    }
    if bytes.get(cursor + 1).is_some_and(|byte| ident_start(*byte))
        && bytes.get(cursor + 2) != Some(&b'\'')
    {
        cursor += 2;
        while cursor < bytes.len() && ident_continue(bytes[cursor]) {
            cursor += 1;
        }
        return Some(cursor);
    }
    cursor += 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' {
            cursor += 2;
        } else if bytes[cursor] == b'\'' {
            return Some(cursor + 1);
        } else {
            cursor += 1;
        }
    }
    Some(cursor)
}

pub(crate) fn texts(tokens: &[Token]) -> Vec<&str> {
    tokens.iter().map(|token| token.text.as_str()).collect()
}

pub(crate) fn count_sequence(tokens: &[Token], expected: &[&str]) -> usize {
    let actual = texts(tokens);
    actual
        .windows(expected.len())
        .filter(|window| *window == expected)
        .count()
}

fn sequence_position(tokens: &[Token], expected: &[&str]) -> Option<usize> {
    if expected.is_empty() {
        return None;
    }
    let actual = texts(tokens);
    let mut matches = actual
        .windows(expected.len())
        .enumerate()
        .filter_map(|(index, window)| (window == expected).then_some(index));
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

fn item_bounds(tokens: &[Token], signature: &[&str]) -> Option<(usize, usize, usize)> {
    let signature_start = sequence_position(tokens, signature)?;
    let mut open = None;
    for (index, token) in tokens
        .iter()
        .enumerate()
        .skip(signature_start + signature.len())
    {
        match token.text.as_str() {
            "{" => {
                open = Some(index);
                break;
            }
            ";" => return None,
            _ => {}
        }
    }
    let open = open?;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.text.as_str() {
            "{" => depth += 1,
            "}" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some((signature_start, open, index));
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn item_body_tokens<'a>(tokens: &'a [Token], signature: &[&str]) -> Option<&'a [Token]> {
    let (_, open, close) = item_bounds(tokens, signature)?;
    Some(&tokens[open + 1..close])
}

pub(crate) fn item_header<'a>(
    source: &'a str,
    tokens: &[Token],
    signature: &[&str],
) -> Option<&'a str> {
    let (signature_start, open, _) = item_bounds(tokens, signature)?;
    Some(&source[tokens[signature_start].start..tokens[open].end])
}
