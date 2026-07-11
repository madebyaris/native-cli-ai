//! Slash-command registry shared by help, completion, and the command palette.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    Agent,
    Session,
    Provider,
    System,
}

impl CommandCategory {
    pub const ALL: [Self; 4] = [Self::Agent, Self::Session, Self::Provider, Self::System];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Session => "Session",
            Self::Provider => "Provider",
            Self::System => "System",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAvailability {
    Always,
    Tui,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub category: CommandCategory,
    pub shortcut: &'static str,
    pub availability: CommandAvailability,
    pub hidden: bool,
}

macro_rules! command {
    ($id:literal, $name:literal, [$($alias:literal),*], $description:literal, $category:ident, $shortcut:literal, $availability:ident) => {
        CommandSpec {
            id: $id,
            name: $name,
            aliases: &[$($alias),*],
            description: $description,
            category: CommandCategory::$category,
            shortcut: $shortcut,
            availability: CommandAvailability::$availability,
            hidden: false,
        }
    };
}

pub const COMMAND_SPECS: &[CommandSpec] = &[
    command!("help", "/help", [], "Show help", System, "ctrl+x h", Always),
    command!(
        "agent",
        "/agent",
        [],
        "Choose agent profile",
        Agent,
        "ctrl+x a",
        Always
    ),
    command!(
        "plan",
        "/plan",
        [],
        "Run a planning turn",
        Agent,
        "",
        Always
    ),
    command!(
        "review",
        "/review",
        [],
        "Run a code review turn",
        Agent,
        "",
        Always
    ),
    command!(
        "fix",
        "/fix",
        [],
        "Run a focused fix turn",
        Agent,
        "",
        Always
    ),
    command!(
        "test",
        "/test",
        [],
        "Run a validation turn",
        Agent,
        "",
        Always
    ),
    command!("skills", "/skills", [], "Browse skills", Agent, "", Always),
    command!(
        "memory",
        "/memory",
        [],
        "Show or save memory",
        Agent,
        "",
        Always
    ),
    command!(
        "compact",
        "/compact",
        [],
        "Compact session context",
        Session,
        "ctrl+x c",
        Always
    ),
    command!("mcp", "/mcp", [], "Show MCP servers", System, "", Always),
    command!(
        "agents",
        "/agents",
        [],
        "Show child agents",
        Agent,
        "",
        Always
    ),
    command!(
        "logs",
        "/logs",
        [],
        "Show session logs",
        Session,
        "",
        Always
    ),
    command!(
        "attach",
        "/attach",
        [],
        "Show attach details",
        Session,
        "",
        Always
    ),
    command!(
        "image",
        "/image",
        [],
        "Stage an image",
        Session,
        "ctrl+v",
        Tui
    ),
    command!(
        "editor",
        "/editor",
        [],
        "Open external editor",
        Session,
        "ctrl+x e",
        Always
    ),
    command!(
        "sessions",
        "/sessions",
        [],
        "Switch session",
        Session,
        "ctrl+x l",
        Always
    ),
    command!(
        "new",
        "/new",
        [],
        "Start a new session",
        Session,
        "ctrl+x n",
        Always
    ),
    command!(
        "export",
        "/export",
        [],
        "Export session",
        Session,
        "",
        Always
    ),
    command!(
        "thinking",
        "/thinking",
        [],
        "Toggle thinking display",
        Agent,
        "",
        Always
    ),
    command!(
        "stop",
        "/stop",
        [],
        "Stop current turn",
        Session,
        "esc",
        Always
    ),
    command!(
        "diff",
        "/diff",
        [],
        "Show recent changes",
        System,
        "",
        Always
    ),
    command!(
        "clear",
        "/clear",
        [],
        "Clear transcript",
        System,
        "ctrl+l",
        Always
    ),
    command!(
        "exit",
        "/exit",
        ["/quit", "/q"],
        "Exit",
        System,
        "ctrl+x q",
        Always
    ),
    command!(
        "auto-answer",
        "/auto-answer",
        [],
        "Use suggested answer",
        Agent,
        "",
        Always
    ),
    command!(
        "model",
        "/model",
        ["/models"],
        "Choose model",
        Provider,
        "ctrl+x m",
        Always
    ),
    command!(
        "connect",
        "/connect",
        ["/provider", "/apikey", "/custom"],
        "Connect provider",
        Provider,
        "",
        Always
    ),
    command!(
        "status",
        "/status",
        ["/stats", "/cost", "/doctor"],
        "Show status and health",
        System,
        "ctrl+x s",
        Always
    ),
    command!(
        "config",
        "/config",
        ["/settings", "/set-editor"],
        "Show or update config",
        System,
        "",
        Always
    ),
    command!(
        "permissions",
        "/permissions",
        ["/permission-bypass"],
        "Choose permissions",
        System,
        "",
        Always
    ),
];

pub fn visible_commands() -> impl Iterator<Item = &'static CommandSpec> {
    COMMAND_SPECS.iter().filter(|spec| !spec.hidden)
}

pub fn resolve_command(name: &str) -> Option<&'static CommandSpec> {
    COMMAND_SPECS
        .iter()
        .find(|spec| spec.name == name || spec.aliases.contains(&name))
}

pub fn help_lines() -> Vec<String> {
    let mut lines = Vec::new();
    for category in CommandCategory::ALL {
        lines.push(format!("{}:", category.label().to_ascii_uppercase()));
        for spec in visible_commands().filter(|spec| spec.category == category) {
            lines.push(format!("  {:<18} {}", spec.name, spec.description));
        }
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_has_no_duplicate_names_or_aliases() {
        let mut seen = HashSet::new();
        for spec in COMMAND_SPECS {
            assert!(seen.insert(spec.name), "duplicate command {}", spec.name);
            for alias in spec.aliases {
                assert!(seen.insert(*alias), "duplicate alias {alias}");
            }
        }
    }

    #[test]
    fn every_visible_command_resolves() {
        for spec in visible_commands() {
            assert_eq!(
                resolve_command(spec.name).map(|found| found.id),
                Some(spec.id)
            );
        }
    }

    #[test]
    fn help_is_derived_from_visible_registry() {
        let help = help_lines().join("\n");
        for spec in visible_commands() {
            assert!(help.contains(spec.name));
        }
        assert!(!help.contains("/undo"));
        assert!(!help.contains("/redo"));
    }
}
