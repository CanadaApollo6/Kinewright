//! Slash commands for the session composer (M24).
//!
//! Two kinds: `Local` commands run an app action instantly with no agent
//! turn, and `Prompt` commands expand into a full agent instruction. The
//! registry is the single source of truth for matching, help, and dispatch.

/// What a slash command does when run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlashAction {
    Import,
    RemoveFillers,
    AddCaptions,
    FreezeFrame,
    Record,
    Export,
    Undo,
    Redo,
    Help,
    /// Expand into this agent prompt and start a turn.
    Prompt(&'static str),
}

pub(crate) struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub action: SlashAction,
}

pub(crate) const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand {
        name: "cut-silences",
        description: "Agent: ripple-cut every cuttable silence, then verify none remain",
        action: SlashAction::Prompt(
            "Remove every cuttable silence on the timeline with ripple cuts. After applying \
             your plan, re-check the timeline and keep going until no cuttable silence \
             remains.",
        ),
    },
    SlashCommand {
        name: "tighten",
        description: "Agent: cut silences and fillers while keeping natural pacing",
        action: SlashAction::Prompt(
            "Tighten this edit: ripple-cut the silences and the filler words, keep natural \
             breathing room between sentences, and verify the whole timeline afterward - no \
             cuttable silence should remain and no words should be clipped.",
        ),
    },
    SlashCommand {
        name: "remove-fillers",
        description: "Instant: cut every filler word (um, uh...) in one undoable edit",
        action: SlashAction::RemoveFillers,
    },
    SlashCommand {
        name: "captions",
        description: "Instant: add burned-in captions from the transcript",
        action: SlashAction::AddCaptions,
    },
    SlashCommand {
        name: "freeze",
        description: "Instant: freeze the current frame for two seconds",
        action: SlashAction::FreezeFrame,
    },
    SlashCommand {
        name: "import",
        description: "Import media into this project (or just drop a file)",
        action: SlashAction::Import,
    },
    SlashCommand {
        name: "record",
        description: "Record the screen, a camera, or a voiceover",
        action: SlashAction::Record,
    },
    SlashCommand {
        name: "export",
        description: "Open the export dialog",
        action: SlashAction::Export,
    },
    SlashCommand {
        name: "undo",
        description: "Undo the last edit",
        action: SlashAction::Undo,
    },
    SlashCommand {
        name: "redo",
        description: "Redo the last undone edit",
        action: SlashAction::Redo,
    },
    SlashCommand {
        name: "help",
        description: "List the available slash commands",
        action: SlashAction::Help,
    },
];

/// Commands whose names start with the typed `/query`. Empty when the input
/// is not a slash invocation.
pub(crate) fn matching_commands(input: &str) -> Vec<&'static SlashCommand> {
    let Some(query) = input.trim_start().strip_prefix('/') else {
        return Vec::new();
    };
    let query = query.trim_end();
    if query.contains(char::is_whitespace) {
        return Vec::new();
    }
    SLASH_COMMANDS
        .iter()
        .filter(|command| command.name.starts_with(query))
        .collect()
}

pub(crate) fn help_text() -> String {
    use std::fmt::Write;

    let mut text = String::from("Available commands:");
    for command in SLASH_COMMANDS {
        let _ = write!(text, "\n/{} - {}", command.name, command.description);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_matches_nothing() {
        assert!(matching_commands("cut the silences").is_empty());
        assert!(matching_commands("").is_empty());
    }

    #[test]
    fn a_bare_slash_lists_everything_and_prefixes_filter() {
        assert_eq!(matching_commands("/").len(), SLASH_COMMANDS.len());
        let cut = matching_commands("/cut");
        assert_eq!(cut.len(), 1);
        assert_eq!(cut[0].name, "cut-silences");
        assert!(matching_commands("/zzz").is_empty());
    }

    #[test]
    fn a_completed_command_with_arguments_is_not_a_match_list() {
        assert!(matching_commands("/cut-silences please").is_empty());
    }

    #[test]
    fn command_names_are_unique() {
        let mut names: Vec<_> = SLASH_COMMANDS.iter().map(|command| command.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SLASH_COMMANDS.len());
    }
}
