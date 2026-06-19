//! Prompt templates for the [`AgenticExtractor`](super::AgenticExtractor).
//!
//! The system prompt frames the slice-by-slice, single-session extraction; the
//! schema blocks (hints / strict / evolving) plug into its `{schema_section}`
//! slot per the active [`SchemaMode`](crate::types::SchemaMode); the slice and
//! gleaning prompts drive the per-turn and final-recovery turns.

pub const SYSTEM_PROMPT: &str = r#"You extract a knowledge graph (entities + relationships) from ONE document, slice by slice, across multiple turns.

The FULL document is saved at ./document.md in your working directory. When a slice is ambiguous — a pronoun, an abbreviation, a term defined elsewhere — use the Read or Grep tool on ./document.md to pull the surrounding context. Do not guess.

Across turns, REUSE the exact same entity_name for an entity you have already recorded (consistent coreference). Each turn, only add what is NEW.

Output a single `##`-separated list, each record in EXACTLY one of these forms:
(entity<|>entity_name<|>entity_type<|>a 10-20 word description<|>attributes)
(relationship<|>source_entity<|>target_entity<|>relationship_type<|>why they are related<|>strength_0_to_1)

DIRECTION RULE: a relationship must read left-to-right as a TRUE sentence: "source_entity relationship_type target_entity". EVERY type ending in _BY (FOUNDED_BY, DEVELOPED_BY, CREATED_BY, INVENTED_BY, PUBLISHED_BY, OWNED_BY, ...) is passive: the source is the thing acted on, the target is the doer — (relationship<|>Anthropic<|>Dario Amodei<|>FOUNDED_BY<|>...) is correct because "Anthropic [was] FOUNDED_BY Dario Amodei"; (relationship<|>Dario Amodei<|>Anthropic<|>FOUNDED_BY<|>...) is reversed and WRONG. A person is never the source of a *_BY relationship to their own work — for "X invented/created/wrote W" emit (W<|>X<|>INVENTED_BY) or the active (X<|>W<|>CREATED). Active types (FOUNDED, DEVELOPED, USES) point from the doer to the thing. Before emitting each relationship, read it aloud as a sentence; if it is false as written, swap source and target or pick the opposite-voice type.

Entity names capitalised, in the document's language. Output ONLY the records — no prose, no preamble.

{schema_section}"#;

/// Soft schema block (Open / Evolving / no-schema): types are hints only.
pub const SCHEMA_HINTS: &str = r#"Schema hints — Entity types: {entity_types}
Relationship types: {relationship_types}"#;

/// Hard schema block (Fixed): the model is told the closed type vocabulary up
/// front. Out-of-schema records are still validated and dropped on our side —
/// this just makes conformance the model's job, and the per-turn feedback
/// re-anchors it whenever it drifts.
pub const SCHEMA_STRICT: &str = r#"STRICT SCHEMA — you MUST use ONLY the types below. Any entity or relationship whose type is not in these lists is DISCARDED and wasted.
Entity types (use EXACTLY one of these as entity_type): {entity_types}
Relationship types (use EXACTLY one of these as relationship_type): {relationship_types}"#;

/// Open schema block (Evolving): the seed types are preferred, but the model
/// may coin a new type when none fits — those proposals are recorded (nothing
/// is dropped).
pub const SCHEMA_EVOLVING: &str = r#"SEED SCHEMA — PREFER these types, but you MAY introduce a NEW entity_type or relationship_type when the seed has no good fit. Nothing is discarded.
Entity types (seed): {entity_types}
Relationship types (seed): {relationship_types}"#;

/// Appended to the next slice's prompt after a turn dropped out-of-schema
/// records, so the model self-corrects mid-conversation.
pub const SCHEMA_FEEDBACK: &str = r#"NOTE: from your previous answer I discarded {dropped} record(s) because their types are NOT in the schema{dropped_types}. Stay strictly within — entity types: {entity_types}; relationship types: {relationship_types}. Do not emit any other type."#;

/// Sent to RE-DO a slice when *every* record it produced was out-of-schema
/// (the degenerate case): a single bounded retry with a sterner reminder.
pub const SCHEMA_REDO_PROMPT: &str = r#"EVERY record in your last answer used a type outside the schema, so all of it was discarded. Redo THIS slice using ONLY — entity types: {entity_types}; relationship types: {relationship_types}. Map each thing you found onto the closest allowed type; if something truly fits no allowed type, omit it. Output the records, or just NO if this slice has nothing that fits the schema."#;

pub const SLICE_PROMPT: &str = r#"Slice {i}/{n}:
<slice>
{slice}
</slice>
Extract entities and relationships from THIS slice, reusing names for entities you already recorded. Read/Grep ./document.md if you need context. Output the records, or just NO if this slice has none."#;

pub const RELATION_GLEANING_PROMPT: &str = r#"All slices are processed. The recorded entities below have NO relationship linking them to anything:
{orphans}

For EACH, look back over the WHOLE document (Read/Grep ./document.md if needed) and emit any relationship that connects it to another recorded entity; skip ones that genuinely stand alone — do NOT invent links.

Relationship types to prefer: {relationship_types}

Output ONLY relationships, `##`-separated, each as:
(relationship<|>source_entity<|>target_entity<|>relationship_type<|>why they are related<|>strength_0_to_1)
Each must read left-to-right as a true sentence ("source relationship_type target"); _BY types point from the thing acted on to the doer.
If none have any relationship, answer just: NO"#;
