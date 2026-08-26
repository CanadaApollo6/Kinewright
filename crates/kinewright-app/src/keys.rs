#[cfg(test)]
use std::collections::HashSet;

use eframe::egui;
use kinewright_core::{
    FrameRounding, TimeCode, map_frames_with_rounding, map_source_range_to_project,
};

use crate::app::KinewrightApp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum KeyAction {
    TogglePlayback,
    Split,
    Delete,
    RippleDelete,
    AddMarker,
    Undo,
    Redo,
    ShuttleBackwardFrame,
    Pause,
    PlayForward,
    StepBackward,
    StepForward,
    JumpStart,
    JumpEnd,
    SetIn,
    SetOut,
    Save,
    Export,
    /// CC6 §8.1: open the read-only Colour QC window.
    ColorQc,
    Help,
}

#[cfg(test)]
const ALL_ACTIONS: [KeyAction; 20] = [
    KeyAction::TogglePlayback,
    KeyAction::Split,
    KeyAction::Delete,
    KeyAction::RippleDelete,
    KeyAction::AddMarker,
    KeyAction::Undo,
    KeyAction::Redo,
    KeyAction::ShuttleBackwardFrame,
    KeyAction::Pause,
    KeyAction::PlayForward,
    KeyAction::StepBackward,
    KeyAction::StepForward,
    KeyAction::JumpStart,
    KeyAction::JumpEnd,
    KeyAction::SetIn,
    KeyAction::SetOut,
    KeyAction::Save,
    KeyAction::Export,
    KeyAction::ColorQc,
    KeyAction::Help,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct KeyBinding {
    pub(crate) key: egui::Key,
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) action: KeyAction,
    pub(crate) shortcut: &'static str,
    pub(crate) description: &'static str,
}

pub(crate) const KEYMAP: [KeyBinding; 20] = [
    KeyBinding {
        key: egui::Key::Space,
        ctrl: false,
        shift: false,
        action: KeyAction::TogglePlayback,
        shortcut: "Space",
        description: "Play / pause",
    },
    KeyBinding {
        key: egui::Key::S,
        ctrl: false,
        shift: false,
        action: KeyAction::Split,
        shortcut: "S",
        description: "Split selected or active clip",
    },
    KeyBinding {
        key: egui::Key::Delete,
        ctrl: false,
        shift: false,
        action: KeyAction::Delete,
        shortcut: "Del",
        description: "Delete selected clip",
    },
    KeyBinding {
        key: egui::Key::Delete,
        ctrl: false,
        shift: true,
        action: KeyAction::RippleDelete,
        shortcut: "Shift+Del",
        description: "Ripple delete selected linked clip group",
    },
    KeyBinding {
        key: egui::Key::M,
        ctrl: false,
        shift: false,
        action: KeyAction::AddMarker,
        shortcut: "M",
        description: "Add marker at playhead",
    },
    KeyBinding {
        key: egui::Key::Z,
        ctrl: true,
        shift: false,
        action: KeyAction::Undo,
        shortcut: "Ctrl+Z",
        description: "Undo",
    },
    KeyBinding {
        key: egui::Key::Y,
        ctrl: true,
        shift: false,
        action: KeyAction::Redo,
        shortcut: "Ctrl+Y",
        description: "Redo",
    },
    KeyBinding {
        key: egui::Key::J,
        ctrl: false,
        shift: false,
        action: KeyAction::ShuttleBackwardFrame,
        shortcut: "J",
        description: "Step one frame backward (reverse shuttle unavailable)",
    },
    KeyBinding {
        key: egui::Key::K,
        ctrl: false,
        shift: false,
        action: KeyAction::Pause,
        shortcut: "K",
        description: "Pause",
    },
    KeyBinding {
        key: egui::Key::L,
        ctrl: false,
        shift: false,
        action: KeyAction::PlayForward,
        shortcut: "L",
        description: "Play forward",
    },
    KeyBinding {
        key: egui::Key::ArrowLeft,
        ctrl: false,
        shift: false,
        action: KeyAction::StepBackward,
        shortcut: "Left",
        description: "Step one frame backward",
    },
    KeyBinding {
        key: egui::Key::ArrowRight,
        ctrl: false,
        shift: false,
        action: KeyAction::StepForward,
        shortcut: "Right",
        description: "Step one frame forward",
    },
    KeyBinding {
        key: egui::Key::Home,
        ctrl: false,
        shift: false,
        action: KeyAction::JumpStart,
        shortcut: "Home",
        description: "Jump to project start",
    },
    KeyBinding {
        key: egui::Key::End,
        ctrl: false,
        shift: false,
        action: KeyAction::JumpEnd,
        shortcut: "End",
        description: "Jump to project end",
    },
    KeyBinding {
        key: egui::Key::I,
        ctrl: false,
        shift: false,
        action: KeyAction::SetIn,
        shortcut: "I",
        description: "Trim selected clip in to playhead",
    },
    KeyBinding {
        key: egui::Key::O,
        ctrl: false,
        shift: false,
        action: KeyAction::SetOut,
        shortcut: "O",
        description: "Trim selected clip out to playhead",
    },
    KeyBinding {
        key: egui::Key::S,
        ctrl: true,
        shift: false,
        action: KeyAction::Save,
        shortcut: "Ctrl+S",
        description: "Save project",
    },
    KeyBinding {
        key: egui::Key::E,
        ctrl: true,
        shift: false,
        action: KeyAction::Export,
        shortcut: "Ctrl+E",
        description: "Open export dialog",
    },
    KeyBinding {
        // Free: no other binding uses C at all, and no binding in this map
        // is Ctrl+Shift. Deliberately not Ctrl+Q, which every desktop
        // environment already spends on Quit.
        key: egui::Key::C,
        ctrl: true,
        shift: true,
        action: KeyAction::ColorQc,
        shortcut: "Ctrl+Shift+C",
        description: "Open the Colour QC window (evidence only)",
    },
    KeyBinding {
        key: egui::Key::Questionmark,
        ctrl: false,
        shift: true,
        action: KeyAction::Help,
        shortcut: "?",
        description: "Show keyboard help",
    },
];

impl KinewrightApp {
    pub(crate) fn keyboard_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if self.focused().transcript_selection.is_some()
            && ctx.input(|input| {
                !input.modifiers.ctrl
                    && !input.modifiers.shift
                    && !input.modifiers.alt
                    && input.key_pressed(egui::Key::Backspace)
            })
        {
            self.delete_selected();
            return;
        }
        let action = ctx.input(|input| {
            KEYMAP
                .iter()
                .find(|binding| {
                    binding.ctrl == input.modifiers.ctrl
                        && binding.shift == input.modifiers.shift
                        && !input.modifiers.alt
                        && input.key_pressed(binding.key)
                })
                .map(|binding| binding.action)
        });
        if let Some(action) = action {
            self.perform_key_action(action);
        }
    }

    fn perform_key_action(&mut self, action: KeyAction) {
        match action {
            KeyAction::TogglePlayback => self.toggle_playback(),
            KeyAction::Split => self.split_at_playhead(),
            KeyAction::Delete => self.delete_selected(),
            KeyAction::RippleDelete => self.ripple_delete_selected(),
            KeyAction::AddMarker => self.add_marker_at_playhead(),
            KeyAction::Undo => self.undo(),
            KeyAction::Redo => self.redo(),
            KeyAction::ShuttleBackwardFrame | KeyAction::StepBackward => self.step_frames(-1),
            KeyAction::Pause => self.pause_playback(),
            KeyAction::PlayForward => self.play_forward(),
            KeyAction::StepForward => self.step_frames(1),
            KeyAction::JumpStart => self.pause_and_seek(TimeCode::ZERO),
            KeyAction::JumpEnd => {
                self.pause_and_seek(TimeCode(
                    self.focused().document.duration.0.saturating_sub(1).max(0),
                ));
            }
            KeyAction::SetIn => self.trim_selected_at_playhead(true),
            KeyAction::SetOut => self.trim_selected_at_playhead(false),
            KeyAction::Save => {
                self.save_project(false);
            }
            KeyAction::Export => self.open_export_dialog(),
            KeyAction::ColorQc => self.color_qc.open = true,
            KeyAction::Help => self.help_open = true,
        }
    }

    fn pause_playback(&mut self) {
        self.playback.pause();
    }

    fn play_forward(&mut self) {
        if !self.playing {
            self.toggle_playback();
        }
    }

    fn step_frames(&mut self, delta: i64) {
        self.pause_and_seek(TimeCode(self.focused().position.0.saturating_add(delta)));
    }

    fn pause_and_seek(&mut self, position: TimeCode) {
        self.playback.pause();
        self.seek_to(position);
    }

    fn trim_selected_at_playhead(&mut self, set_in: bool) {
        let Some(clip_id) = self.focused().selected_clip else {
            self.record_error(
                "Operations",
                "Select a clip before setting an in or out point",
            );
            return;
        };
        let Some(clip) = self.focused().document.clip(clip_id).cloned() else {
            self.record_error("Operations", format!("Clip {clip_id} no longer exists"));
            return;
        };
        let Some(asset) = self.focused().document.asset(clip.asset).cloned() else {
            self.record_error(
                "Operations",
                format!("Asset {} no longer exists", clip.asset),
            );
            return;
        };
        let Ok(project_duration) = map_source_range_to_project(
            clip.source_range.clone(),
            asset.fps,
            self.focused().document.fps,
        ) else {
            self.record_error("Operations", "Could not map the selected clip time base");
            return;
        };
        let project_end = clip.timeline_start.0.saturating_add(project_duration.0);
        let position = self.focused().position;
        if position < clip.timeline_start || position.0 > project_end {
            self.record_error(
                "Operations",
                "Move the playhead onto the selected clip first",
            );
            return;
        }
        let project_offset = TimeCode(position.0.saturating_sub(clip.timeline_start.0));
        let Ok(source_offset) = map_frames_with_rounding(
            project_offset,
            self.focused().document.fps,
            asset.fps,
            FrameRounding::Nearest,
        ) else {
            self.record_error("Operations", "Could not map the playhead to source frames");
            return;
        };
        let source_at = TimeCode(
            clip.source_range
                .start
                .0
                .saturating_add(source_offset.0)
                .clamp(clip.source_range.start.0, clip.source_range.end.0),
        );
        let new_source = if set_in {
            if source_at >= clip.source_range.end {
                self.record_error(
                    "Operations",
                    "The in point must be before the clip out point",
                );
                return;
            }
            source_at..clip.source_range.end
        } else {
            if source_at <= clip.source_range.start {
                self.record_error(
                    "Operations",
                    "The out point must be after the clip in point",
                );
                return;
            }
            clip.source_range.start..source_at
        };
        self.apply_linked_trim(clip_id, new_source);
    }

    pub(crate) fn show_help(&mut self, ctx: &egui::Context) {
        if !self.help_open {
            return;
        }
        egui::Window::new("Keyboard shortcuts")
            .open(&mut self.help_open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("keymap-help").striped(true).show(ui, |ui| {
                    for binding in KEYMAP {
                        ui.monospace(binding.shortcut);
                        ui.label(binding.description);
                        ui.end_row();
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_has_no_duplicates_and_every_action_is_reachable() {
        let bindings = KEYMAP
            .iter()
            .map(|binding| (binding.key, binding.ctrl, binding.shift))
            .collect::<HashSet<_>>();
        assert_eq!(bindings.len(), KEYMAP.len(), "duplicate key binding");

        // CC6 §8.1 added `ColorQc`, growing both arrays from 19 to 20. This is
        // the assertion that keeps them together: an action added to the enum
        // and to `ALL_ACTIONS` but given no binding fails here, as does a
        // binding for an action nobody declared. The array lengths are
        // compile-time constants and assert nothing on their own.
        let actions = KEYMAP
            .iter()
            .map(|binding| binding.action)
            .collect::<HashSet<_>>();
        assert_eq!(actions, HashSet::from(ALL_ACTIONS));
        assert!(
            actions.contains(&KeyAction::ColorQc),
            "the Colour QC window is reachable from the keyboard"
        );
    }
}
