# spEARS Discovery

Discovery is the process of achieving genuine understanding of user needs
through Socratic questioning before any specification writing begins.

**Core principle**: Specs without traceable user journeys are hollow.
EARS format compliance means nothing if you cannot answer: “Who does what, and
why do they care?”

## When to Use Discovery

- New feature with no existing spec
- Existing spec that feels hollow or formulaic
- User has a vague idea ("I want to build X") without clear requirements
- Spec exists but user journeys and critical paths aren’t well understood

## Entry Protocol

Before asking any questions, understand context:

1. **Explore the project** to find specs and code related to the user’s topic.
   Focus on existing specs in `specs/`, adjacent specs that provide context, and
   any implementation that already exists.

2. **Review what exists**. Identify where understanding breaks down (usually:
   journeys and critical paths).
   Stay focused on the user’s topic -- don’t audit the entire project.

3. **If no relevant specs exist**: Start fresh from Motivation.

4. **If specs exist but feel hollow**: Identify which discovery levels are
   missing and start there.

## The Discovery Hierarchy

Probe in this order.
Each level builds on the previous:

```text
1. MOTIVATION      Why does this feature/project exist at all?
2. PERSONAS        Who are the users? What distinguishes them?
3. JOURNEYS        What do they do, step by step? What do they expect?
4. CRITICAL PATHS  Which journeys, if broken, kill the business?
5. EDGE CASES      What happens when things go wrong?
```

## Socratic Questioning Methodology

### Challenge Assumptions

“You said users need X -- but what if Y happens?
Who handles that?” "Why would a user trust this?
What have they seen before?"

### Demand Concrete Examples

“Walk me through a specific scenario where this matters.”
"Give me a real user -- name them, describe their day.
When do they hit this?"

### Probe the Why Chain (5 Whys)

“Why does the user care about that?”
"Why is that important to them?"
“Why would that hurt if it failed?”

### Identify Unknowns

“What don’t we know yet that we’d need to know?”
"Where are you guessing vs.
where do you have evidence?"

## The Questioning Loop

Repeat until the user confirms understanding:

1. **Ask a question** framed around the current hierarchy level.
   Include options when there are clear alternatives.
   Leave room for “Other” to capture unexpected answers.

2. **Synthesize the answer**. Connect new information to what you already know.
   Identify what’s still unclear.
   Note contradictions or gaps.

3. **Advance or probe deeper**. If the level is clear, move to the next.
   If gaps remain, probe deeper before advancing.

4. **Watch for the exit signal**. The user must explicitly confirm
   understanding: “you’ve got it”, “that’s exactly right”, “perfect, let’s write
   it”. Do NOT proceed to handoff based on your own assessment.

### Example Flow

```text
Q: "What problem are we solving? Who has this problem?"

User: "Users need to see their API usage stats."

Synthesize: Usage visibility is the surface need.
Q: "Why do they need to see them? What decision does this inform?"

User: "So they know if they're approaching limits."

Synthesize: The real need is avoiding surprise throttling.
Q: "What happens when they approach limits? Walk me through that moment."

User: "They get warned, then throttled if they exceed."

Synthesize: This is the CRITICAL path. Probing business impact.
Q: "If throttling surprises them, what's the business impact?"

User: "Definitely churn. Our power users would leave."

Synthesize: Critical journey = notice limit -> take action -> stay happy.
Q: "What actions can they take to avoid throttling?"

[...continues until user confirms "you've got it"...]
```

## When You’re Stuck

If questioning isn’t converging after ~20 rounds:

- “It seems like [X] is blocking us.
  Should we tackle that separately?”
- “This might be too big for one spec.
  Should we break out [Y] as its own feature?”
- “We’re uncertain about [Z]. Who could we ask?
  What data would help?”

Do NOT give up. Help the user find a path forward.

## Red Flags During Discovery

Stop and address immediately:

- **Vague motivation**: “It would be nice to have” -- probe for concrete pain
- **No personas**: “Users” generically -- demand specific examples
- **Feature shopping**: Jumping between unrelated capabilities -- focus
- **Implementation answers**: “We should use Redis” -- redirect to user need
- **Certainty theater**: Confident answers with no evidence -- probe for data

## Green Flags

You’re making progress when:

- User gives specific names/scenarios
- User identifies what failure would cost them
- User distinguishes “must have” from “nice to have”
- User acknowledges uncertainty honestly
- User connects features to business outcomes

## Handoff: Discovery to Spec Writing

When the user confirms understanding, crystallize into a structured handoff
document. This is an AI-to-AI transfer -- optimize for information density, not
human readability.

```text
# Spec Discovery Handoff

## Target
specs/[feature-name]/

## Scope
[new-full | update-requirements | update-design | targeted]

## Motivation
[Dense prose: why this exists, business context, what failure means]

## Personas
[For each persona: name, role, distinguishing characteristics, goals]

## User Journeys
[For each journey: trigger -> steps -> expected outcome]
[Mark which requirements likely map to each journey]

## Critical Paths
[Journeys where failure = business damage]
[Why they're critical, what failure looks like]

## Edge Cases
[Known failure modes, unclear scenarios, decisions deferred]

## Unknowns
[What we explicitly chose not to resolve, and why]

## Cross-Spec Impact
[Other specs that may need updates based on these discoveries]
```

After creating the handoff, proceed to write the spec documents following the
rules in `references/ears-format.md` and the guardrails in SKILL.md.

## Question Bank

### Motivation Level

- “What problem does this solve?
  For whom?”
- “What happens today without this?
  What’s the workaround?”
- “Why now? What changed that makes this urgent?”
- “What’s the cost of not building this?”

### Persona Level

- “Who uses this? Give me a specific person, real or composite.”
- “What distinguishes power users from casual users here?”
- “Who’s the most demanding user?
  What do they need?”
- “Who would complain loudest if this broke?”

### Journey Level

- “Walk me through a typical session with this feature.”
- “What triggers someone to use this?
  What were they doing before?”
- “What does success look like?
  How do they know it worked?”
- “What’s the happy path?
  What’s the frustrated path?”

### Critical Path Level

- “Which of these journeys, if broken, would be catastrophic?”
- “What’s the business impact of this failing?”
- “Where does user trust get established or destroyed?”
- “What’s the worst-case scenario we must prevent?”

### Edge Case Level

- “What happens when [X] fails?”
- “What if the user does [unexpected thing]?”
- “What’s the recovery path when things go wrong?”
- “Where are we making optimistic assumptions?”
