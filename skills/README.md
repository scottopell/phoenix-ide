# Phoenix IDE Skills

This directory contains **Skills** - organized packages of instructions and resources that give LLM agents specialized capabilities for specific tasks.

## What are Skills?

Skills are an emerging standard from Anthropic for packaging procedural knowledge and organizational context for LLM agents. Think of them as "onboarding guides" that teach an agent how to perform specific tasks.

**Key resources:**
- [Equipping agents for the real world with Skills](https://www.anthropic.com/engineering/equipping-agents-for-the-real-world-with-agent-skills) - Anthropic engineering blog
- [Skills Cookbook](https://github.com/anthropics/anthropic-cookbook/tree/main/skills) - Examples and notebooks
- [Skills Documentation](https://docs.claude.com/en/docs/agents-and-tools/agent-skills/overview)

## Skill Structure

Each skill is a directory with this structure:

```
skill_name/
├── SKILL.md           # REQUIRED: Instructions with YAML frontmatter
├── *.md               # Optional: Additional documentation
├── scripts/           # Optional: Executable code
└── resources/         # Optional: Templates, data files
```

### SKILL.md Requirements

The `SKILL.md` file must include:

1. **YAML Frontmatter** with:
   - `name`: Lowercase alphanumeric with hyphens (max 64 chars)
   - `description`: Brief description (max 1024 chars)

2. **Instructions** in markdown format (recommended: <5,000 tokens)

Example:
```markdown
---
name: my-skill
description: Brief description of what this skill does
---

# My Skill

Instructions for how to use this skill...
```

## Design Principles

### Progressive Disclosure

Skills load in stages to optimize token usage:

| Stage | Content | When Loaded |
|-------|---------|-------------|
| Metadata | Name & description | Always visible |
| Instructions | All .md files | When relevant |
| Resources | Scripts & files | During execution |

### Best Practices

1. **Single Responsibility** - Each skill focuses on one area
2. **Clear Documentation** - SKILL.md is comprehensive yet concise
3. **Be Explicit** - Don't assume context; state what you want
4. **Use Examples** - Show concrete examples of desired outputs
5. **Use XML Tags** - Structure complex instructions with tags

## Our Approach (Conservative Start)

We're starting conservatively to learn how agents consume skills:

- Start with simple, focused skills
- Observe how they're used in practice
- Iterate based on what works
- Document learnings in each skill

## Skills Index

| Skill | Description |
|-------|-------------|
| [phoenix-development](phoenix-development/SKILL.md) | Dev workflow: `./dev.py up/down/restart/check`, testing, code conventions |
| [phoenix-deployment](phoenix-deployment/SKILL.md) | Production deployment across Lima VM, native systemd, and daemon modes |
| [phoenix-task-tracking](phoenix-task-tracking/SKILL.md) | Task file format, CLI commands, issue discovery protocol |

---

*This folder is part of the phoenix-ide project.*
