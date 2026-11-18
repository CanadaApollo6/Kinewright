use eframe::egui;

pub(crate) const ROW_CHARS: usize = 160;
pub(crate) const DETAIL_CHARS: usize = 1024;

fn display_escape(value: char) -> Option<std::char::EscapeUnicode> {
    (value.is_control()
        || matches!(
            value,
            '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{206f}'
        ))
    .then(|| value.escape_unicode())
}

fn append(output: &mut String, input: impl Iterator<Item = char>, limit: usize) -> bool {
    let mut remaining = limit;
    for value in input {
        let escape = display_escape(value);
        let width = escape.as_ref().map_or(1, ExactSizeIterator::len);
        if width > remaining {
            return false;
        }
        remaining -= width;
        if let Some(escape) = escape {
            output.extend(escape);
        } else {
            output.push(value);
        }
    }
    true
}

fn append_suffix(output: &mut String, input: impl DoubleEndedIterator<Item = char>, limit: usize) {
    let mut remaining = limit;
    let suffix: Vec<_> = input
        .rev()
        .take_while(|&value| {
            let width = display_escape(value).map_or(1, |escape| escape.len());
            if width > remaining {
                return false;
            }
            remaining -= width;
            true
        })
        .collect();
    append(output, suffix.into_iter().rev(), limit);
}

pub(crate) fn display_text(parts: &[&str], limit: usize) -> String {
    let chars = || parts.iter().flat_map(|part| part.chars());
    let mut output = String::new();
    if append(&mut output, chars(), limit) {
        return output;
    }
    let head = (limit - 1) / 2;
    let tail = limit - 1 - head;
    output.clear();
    append(&mut output, chars(), head);
    output.push('…');
    append_suffix(&mut output, chars(), tail);
    output
}

pub(crate) fn display_text_end(text: &str, limit: usize) -> String {
    let mut output = String::new();
    if !append(&mut output, text.chars(), limit) {
        output.clear();
        output.push('…');
        append_suffix(&mut output, text.chars(), limit - 1);
    }
    output
}

/// Bound the text while keeping the complete prefix, or decline to show it.
pub(crate) fn display_text_with_prefix(
    prefix: &[&str],
    tail: &[&str],
    limit: usize,
) -> Option<String> {
    let mut output = String::new();
    if !append(
        &mut output,
        prefix.iter().flat_map(|part| part.chars()),
        limit,
    ) {
        return None;
    }
    let remaining = limit - output.chars().count();
    if tail.iter().any(|part| !part.is_empty()) {
        if remaining == 0 {
            return None;
        }
        output.push_str(&display_text(tail, remaining));
    }
    Some(output)
}

/// Keep the original available to copy from a shortened label's context menu.
pub(crate) fn copyable_label(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    original: &str,
) -> egui::Response {
    let response = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
    response.context_menu(|ui| {
        if ui.button("Copy full text").clicked() {
            ui.ctx().copy_text(original.to_owned());
            ui.close();
        }
    });
    response
}
