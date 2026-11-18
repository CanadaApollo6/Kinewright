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

pub(crate) fn display_text(parts: &[&str], limit: usize) -> String {
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

    let mut remaining = tail;
    let suffix: Vec<_> = chars()
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
    append(&mut output, suffix.into_iter().rev(), tail);
    output
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
