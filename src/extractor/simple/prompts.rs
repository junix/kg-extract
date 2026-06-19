//! Prompt templates for the SimpleExtractor.
//!
//! These are the GraphRAG-style delimiter prompts (ported from
//! `graph/kg_extractor/simple.py`) plus the targeted *relation-gleaning*
//! follow-ups. Kept in their own module so the extraction logic in
//! [`super`] stays focused on control flow.

/// Main extraction prompt: identifies entities + relationships from one chunk
/// against the configured schema, emitting the `<|>` / `##` delimiter format.
pub const EXTRACTION_PROMPT: &str = r#"
--- GOAL ---
Given a text document and a schema definition, identify all entities, relationships, and attributes from the text according to the schema.

--- SCHEMA ---
Entity Types: {entity_types}
Relationship Types: {relationship_types}
Attribute Types: {attribute_types}

--- STEPS ---
1. Identify all entities. For each identified entity, extract the following information:
- entity_name: Name of the entity, capitalized, in language of 'Text'
- entity_type: One of the following types: {entity_types}
- entity_description: IMPORTANT - Provide a detailed and comprehensive description (at least 10-20 words) of the entity explaining what it is, its significance, and its key characteristics. DO NOT LEAVE THIS EMPTY.
- entity_attributes: Key attributes of the entity (if any from the attribute types: {attribute_types})
Format each entity as (entity<|>entity_name<|>entity_type<|>entity_description<|>entity_attributes)

2. From the entities identified in step 1, identify all pairs of (source_entity, target_entity) that are *clearly related* to each other.
For each pair of related entities, extract the following information:
- source_entity: name of the source entity, as identified in step 1
- target_entity: name of the target entity, as identified in step 1
- relationship_type: type of relationship from the schema (one of: {relationship_types})
- relationship_description: explanation as to why you think the source entity and the target entity are related to each other in language of 'Text'
- relationship_strength: a numeric score indicating strength of the relationship between the source entity and target entity
- direction: the triple must read left-to-right as a TRUE sentence: "source_entity relationship_type target_entity". EVERY type ending in _BY (FOUNDED_BY, DEVELOPED_BY, CREATED_BY, INVENTED_BY, PUBLISHED_BY, ...) is passive — the source is the thing acted on, the target is the doer ("Anthropic FOUNDED_BY Dario Amodei", never "Dario Amodei FOUNDED_BY Anthropic"); a person is never the source of a *_BY relationship to their own work. Active types (FOUNDED, DEVELOPED, USES) point from the doer to the thing. If the sentence is false as written, swap source and target or pick the opposite-voice type.
Format each relationship as (relationship<|>source_entity<|>target_entity<|>relationship_type<|>relationship_description<|>relationship_strength)

3. Return output as a single list of all the entities and relationships identified in steps 1 and 2. Use `##` as the list delimiter.
4. One entity or relation must use `<|>` as the tuple delimiter.

-- EXAMPLE OUTPUT --
```text
(entity<|>DeepSeek-R1-Distill-Qwen-14B<|>model<|>A 14-billion parameter model used in reinforcement learning experiments.<|>type: language model, parameters: 14B)##
(relationship<|>DeepSeek-R1-Distill-Qwen-14B<|>GRPO<|>uses<|>The model uses GRPO algorithm for optimization during training.<|>0.9)##
```

--- REAL DATA ---
Context about the text:
<text-context>
{context}
</text-context>
Text:
<chunk-to-analysis>
{chunk}
</chunk-to-analysis>
Output:
"#;

/// Plain entity-gleaning follow-up ("what did you miss?").
pub const CONTINUE_PROMPT: &str = "Seem MANY entities/relations were missed in the extraction. If there are no more entities/relations that need to be added, just answer NO. Otherwise, add them below using the same format:\n";

/// System prompt shared by every turn of a chunk's extraction session.
pub const SYSTEM_PROMPT: &str = "You are an elite assistant for extracting structured data from text.";

/// Targeted *relation* gleaning. Plain entity gleaning (`CONTINUE_PROMPT`) asks
/// "what did you miss?" and the model tends to answer with more *nouns*, leaving
/// freshly-named entities dangling. This prompt does the opposite: it names the
/// entities that ended up with **no** relationship and asks specifically *why* –
/// connect each to the graph, or explicitly leave it alone. Endpoints are
/// resolved against the already-extracted entities, so the round only adds edges
/// (optionally pulling in a new neighbour the model names to anchor one).
///
/// Sent as a follow-up *within the same session*, so the source text is already
/// in context (native multi-turn) or replayed in the history (`ReplaySession`) –
/// the chunk is intentionally not re-embedded here.
pub const RELATION_GLEANING_PROMPT: &str = r#"
Looking back at the text you just analysed: the entities below were extracted but
have NO relationship linking them to anything. For EACH one:
- if it is clearly related to another entity (one from this list, or any other
  entity in the text), emit that relationship;
- if it genuinely stands alone in this text, skip it — do NOT invent a link.

Orphan entities:
{orphans}

Relationship Types to prefer: {relationship_types}

Output ONLY relationships, `##`-separated, each in EXACTLY this format:
(relationship<|>source_entity<|>target_entity<|>relationship_type<|>why they are related<|>strength_0_to_1)
Each must read left-to-right as a true sentence ("source relationship_type target"); _BY types point from the thing acted on to the doer.

If none of these entities has any relationship in the text, answer just: NO
Output:
"#;
