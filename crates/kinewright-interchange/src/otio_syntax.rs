use std::borrow::Cow;

use crate::{MAX_ITEMS, MAX_MEDIA, MAX_TREE_DEPTH, Refusal, parse};

enum PathPart<'a> {
    Object(Option<Cow<'a, str>>),
    Array(usize),
}

struct Scope<'a> {
    part: PathPart<'a>,
    ignored: bool,
}

pub(crate) fn preflight(bytes: &[u8]) -> Result<(), Refusal> {
    scan(bytes, bytes.len(), true).map(|_| ())
}

pub(crate) fn pointer(bytes: &[u8], line: usize, column: usize) -> String {
    let line_start = bytes
        .split_inclusive(|byte| *byte == b'\n')
        .take(line.saturating_sub(1))
        .map(<[u8]>::len)
        .sum::<usize>();
    let offset = line_start.saturating_add(column).min(bytes.len());
    scan(bytes, offset, false).unwrap_or_else(|_| "/".into())
}

fn scan(bytes: &[u8], stop: usize, limits: bool) -> Result<String, Refusal> {
    let mut scopes = Vec::new();
    let mut i = 0;
    let mut items = 0;
    let mut media = 0;
    while i < stop {
        match bytes[i] {
            byte @ (b'{' | b'[') => {
                let ignored = scopes.last().is_some_and(|scope: &Scope<'_>| {
                    scope.ignored
                        || matches!(&scope.part, PathPart::Object(Some(key)) if key == "metadata")
                });
                scopes.push(Scope {
                    part: if byte == b'{' {
                        PathPart::Object(None)
                    } else {
                        PathPart::Array(0)
                    },
                    ignored,
                });
                if scopes.len() > MAX_TREE_DEPTH {
                    return Err(parse::too_deep());
                }
            }
            b'}' | b']' => {
                scopes.pop();
            }
            b',' => match scopes.last_mut().map(|scope| &mut scope.part) {
                Some(PathPart::Object(key)) => *key = None,
                Some(PathPart::Array(index)) => *index += 1,
                None => {}
            },
            b'"' => {
                let start = i;
                let Some(end) = string_end(bytes, i + 1) else {
                    break;
                };
                let token = &bytes[start..end];
                let is_key =
                    bytes[end..].iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b':');
                if let Some(scope) = scopes.last_mut().filter(|scope| !scope.ignored)
                    && let PathPart::Object(key) = &mut scope.part
                {
                    if is_key {
                        *key = string(token);
                    } else if limits
                        && key.as_deref() == Some("OTIO_SCHEMA")
                        && let Some(schema) = string(token)
                    {
                        let name = schema.split('.').next().unwrap_or_default();
                        if name.ends_with("Reference") {
                            media += 1;
                        } else if !matches!(
                            name,
                            "RationalTime"
                                | "TimeRange"
                                | "Track"
                                | "Sequence"
                                | "SerializableCollection"
                        ) {
                            items += 1;
                        }
                        if items > MAX_ITEMS || media > MAX_MEDIA {
                            return Err(parse::too_many_items());
                        }
                    }
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    if limits {
        return Ok(String::new());
    }
    let mut path = String::new();
    for scope in scopes {
        match scope.part {
            PathPart::Object(Some(key)) => {
                path.push('/');
                path.push_str(&key.replace('~', "~0").replace('/', "~1"));
            }
            PathPart::Array(index) => {
                use std::fmt::Write as _;
                let _ = write!(path, "/{index}");
            }
            PathPart::Object(None) => {}
        }
    }
    if path.is_empty() {
        path.push('/');
    }
    Ok(crate::shorten(&path, 4096).into())
}

fn string_end(bytes: &[u8], mut i: usize) -> Option<usize> {
    while let Some(byte) = bytes.get(i) {
        match byte {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

fn string(token: &[u8]) -> Option<Cow<'_, str>> {
    if token.contains(&b'\\') {
        serde_json::from_slice::<String>(token).ok().map(Cow::Owned)
    } else {
        std::str::from_utf8(&token[1..token.len() - 1])
            .ok()
            .map(Cow::Borrowed)
    }
}
