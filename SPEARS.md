# spEARS v2

Phoenix uses spEARS v2 for specification work.

For the complete authoring workflow, use the built-in `spears` skill. Its source
lives at `crates/phoenix-skills/src/builtin/spears/` and is packaged into the
Phoenix binary. Repository-specific migration guidance for retiring legacy v1
`design.md` files lives in `skills/spears-v2-migrate/`.

## Artifact model

spEARS v2 separates artifacts by their relationship with time:

```text
specs/
├── adrs/                 # shared project-level decision history
│   ├── _TEMPLATE.md
│   ├── README.md
│   └── NNN_<slug>.md
└── feature-name/
    ├── requirements.md   # timeless user need and REQ-* requirements
    ├── feature.allium    # optional precise behavior, when complexity warrants it
    └── executive.md      # status/current reality and verification coverage
```

- `requirements.md` and `.allium` are normative.
- ADRs are authoritative history for why decisions were made.
- `executive.md` is the status/current-reality exception.
- New specs do not create a required `design.md`.
- Existing legacy `design.md` files should be migrated into the v2 homes above
  and deleted when no unique v2-worthy content remains.

## Working in this repository

- For new specs, invoke/use the built-in `spears` skill.
- For legacy spec migration, use `skills/spears-v2-migrate/SKILL.md`.
- Before changing specifications, follow `specs/AUTHORING.md`.
- Validate the v2 shape with:

```bash
./dev.py check --lanes spec-shape
```
