use roxmltree::{Document, ParsingOptions};

use crate::{MAX_TREE_DEPTH, MAX_XML_NODES, Refusal, RefusalCode, parse};

pub(crate) fn document(bytes: &[u8]) -> Result<Document<'_>, Refusal> {
    parse::check_bytes(bytes)?;
    let text = std::str::from_utf8(bytes).map_err(|invalid| {
        parse::malformed(
            position(bytes, invalid.valid_up_to()),
            "The FCPXML file is not UTF-8.",
        )
    })?;
    preflight(text.as_bytes())?;
    Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: true,
            nodes_limit: MAX_XML_NODES,
        },
    )
    .map_err(|error| {
        if matches!(error, roxmltree::Error::NodesLimitReached) {
            parse::too_many_items()
        } else {
            let site = error.pos();
            parse::malformed(
                format!("{}:{}", site.row, site.col),
                "The FCPXML document is malformed.",
            )
        }
    })
}

// Scan XML markup before allocating the DOM. Quotes, comments and CDATA cannot
// start declarations or affect the nesting count.
fn preflight(bytes: &[u8]) -> Result<(), Refusal> {
    let mut index = 0;
    let mut depth = 0_usize;
    while let Some(open) = bytes[index..].iter().position(|byte| *byte == b'<') {
        let begin = index + open;
        let tail = &bytes[begin..];
        if let Some((opening, closing)) = [
            (b"<!--".as_slice(), b"-->".as_slice()),
            (b"<![CDATA[", b"]]>"),
            (b"<?", b"?>"),
        ]
        .into_iter()
        .find(|(opening, _)| tail.starts_with(opening))
        {
            let Some(end) = tail[opening.len()..]
                .windows(closing.len())
                .position(|window| window == closing)
            else {
                break;
            };
            index = begin + opening.len() + end + closing.len();
            continue;
        }
        if tail.starts_with(b"<!") {
            let bare = tail.strip_prefix(b"<!DOCTYPE").and_then(|rest| {
                let end = rest.iter().position(|byte| *byte == b'>')?;
                let declaration = rest[..end].trim_ascii();
                (declaration == b"fcpxml").then_some(b"<!DOCTYPE".len() + end + 1)
            });
            let Some(size) = bare else {
                return Err(Refusal::new(
                    RefusalCode::DtdNotAllowed,
                    "FCPXML admits only a bare DOCTYPE declaration.",
                )
                .at(position(bytes, begin)));
            };
            index = begin + size;
            continue;
        }
        let Some(end) = tag_end(tail) else {
            break;
        };
        if tail.starts_with(b"</") {
            depth = depth.saturating_sub(1);
        } else {
            if depth >= MAX_TREE_DEPTH {
                return Err(parse::too_deep().at(position(bytes, begin)));
            }
            if tail[end - 1] != b'/' {
                depth += 1;
            }
        }
        index = begin + end + 1;
    }
    Ok(())
}

fn tag_end(bytes: &[u8]) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in bytes.iter().copied().enumerate().skip(1) {
        match quote {
            Some(delimiter) if delimiter == byte => quote = None,
            None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
            None if byte == b'>' => return Some(index),
            _ => {}
        }
    }
    None
}

fn position(bytes: &[u8], index: usize) -> String {
    let prefix = &bytes[..index];
    let row = prefix.split(|byte| *byte == b'\n').count();
    let col = prefix
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(index + 1, |last| index - last);
    format!("{row}:{col}")
}
