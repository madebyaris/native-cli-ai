# Skills

Skills are discoverable instruction packs that extend nca's agent behavior. Each skill is a `SKILL.md` file that teaches the agent how to handle specific tasks, frameworks, or workflows.

## How Skills Work

Skills are **not code plugins** — they are structured instruction documents that the agent loads into its context when relevant. When a skill is invoked, the agent reads the skill's `SKILL.md` and follows its instructions.

## Skill Discovery

nca looks for skills in configured directories:

```toml
[harness]
skill_directories = [".nca/skills", ".claude/skills"]
```

Default search paths (relative to workspace):
1. `.nca/skills/` — nca-specific skills
2. `.claude/skills/` — compatible with Claude Code skills

### Skill Structure

Each skill is a directory containing a `SKILL.md` file:

```
.nca/skills/
├── rust-patterns/
│   └── SKILL.md
├── api-design/
│   └── SKILL.md
└── testing/
    └── SKILL.md
```

## Managing Skills

### List Skills

```bash
nca skills                    # List all discovered skills
nca skills list               # Same
nca skills list --json        # JSON output
```

Or in interactive mode:

```
/skills
```

### Install Skills

```bash
# Install from a remote source
nca skills add https://github.com/user/skill-repo

# Install specific skills from a source
nca skills add https://github.com/user/skill-repo -s rust-patterns -s testing

# Install globally (available in all workspaces)
nca skills add https://github.com/user/skill-repo --global
```

### Remove Skills

```bash
nca skills remove rust-patterns
nca skills remove rust-patterns --global
```

### Update Skills

```bash
nca skills update                # Update all skills
nca skills update rust-patterns  # Update a specific skill
```

## Using Skills

### Automatic Discovery

The agent's system prompt includes a list of available skills. The agent can choose to invoke relevant skills based on the task.

### Explicit Invocation

Ask the agent to use a skill:

```
Use the rust-patterns skill to review this code.
```

Or the agent can invoke skills programmatically via the `invoke_skill` tool.

### Slash Command

```
/skills                    # List available skills
```

## Writing Skills

### `SKILL.md` Format

Create a `SKILL.md` file in a skill directory:

```markdown
# Skill Name

Brief description of what this skill does.

## When to Use

Describe when this skill should be activated.

## Instructions

Step-by-step instructions for the agent to follow.

### Step 1: Analysis

Analyze the codebase for...

### Step 2: Implementation

Apply the following patterns...

## Examples

### Before
\`\`\`rust
// problematic code
\`\`\`

### After
\`\`\`rust
// improved code
\`\`\`
```

### Best Practices

1. **Be specific** — give clear, actionable instructions the agent can follow
2. **Include examples** — show before/after code when applicable
3. **Define scope** — explain when the skill should and shouldn't be used
4. **Keep it focused** — one skill per concern (don't combine testing and deployment)
5. **Use structured steps** — numbered steps help the agent track progress

## Skill Directories

### Workspace Skills

```
my-project/.nca/skills/my-skill/SKILL.md
```

Available only in this workspace. Good for project-specific conventions.

### Global Skills

```
~/.nca/skills/my-skill/SKILL.md
```

Available in all workspaces. Good for personal coding standards and reusable patterns.

### Claude-Compatible Skills

```
my-project/.claude/skills/my-skill/SKILL.md
```

nca discovers skills from `.claude/skills/` by default, making it compatible with Claude Code skill conventions.

## System Prompt Integration

When skills are available, nca adds a skills section to the system prompt listing all discovered skills by name. The agent can then use the `invoke_skill` tool to load any skill's full instructions on demand.
