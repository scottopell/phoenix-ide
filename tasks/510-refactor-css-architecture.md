---
status: ready
priority: p3
created: 2025-02-08
---

# Refactor CSS Architecture

## Problem

`ui/src/index.css` is a 3,700+ line monolithic file with:
- No organization or clear sections
- Mix of component styles, utilities, and page-specific CSS
- Inconsistent naming (BEM-ish `.new-conv-input-left` mixed with IDs `#input-area`)
- Hard to find and maintain styles
- No code splitting benefit

## Proposed Solution

### Option A: CSS Modules
Move to component-scoped CSS modules:
```
components/
  InputArea/
    InputArea.tsx
    InputArea.module.css
  MessageBubble/
    MessageBubble.tsx  
    MessageBubble.module.css
```

Pros: True scoping, dead code elimination, colocation
Cons: More files, class name changes throughout

### Option B: Organized Single File
Keep single file but with clear sections:
```css
/* ==========================================================================
   1. Variables & Reset
   ========================================================================== */

/* ==========================================================================
   2. Layout (page structure)
   ========================================================================== */

/* ==========================================================================
   3. Components (reusable)
   ========================================================================== */

/* ==========================================================================
   4. Pages (page-specific overrides)
   ========================================================================== */

/* ==========================================================================
   5. Utilities (helpers)
   ========================================================================== */
```

Pros: Simpler migration, easier to grep
Cons: Still one big file, no true scoping

### Option C: Split by Category
```
styles/
  variables.css
  reset.css
  layout.css
  components.css
  pages.css
  utilities.css
index.css  # imports all
```

Pros: Logical grouping, easier to navigate
Cons: Import order matters, still global scope

## Recommendation

Start with **Option C** (split by category) as a first pass - lower risk, immediate improvement. Consider **Option A** (CSS Modules) for new components going forward.

## Additional Cleanup

- Standardize naming convention (pick BEM or something else, not both)
- Remove unused styles (audit with coverage tools)
- Consolidate duplicate patterns (many similar button styles)
- Add CSS custom properties for repeated values
