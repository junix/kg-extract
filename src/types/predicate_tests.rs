// Behavior-preservation guard: PredicateType now comes from kg-vocab.
use super::*;
use kg_vocab::TypeMatch;

#[test]
fn roundtrip_values() {
    assert_eq!(PredicateType::IsA.value(), "IS_A");
    assert_eq!(PredicateType::WorksFor.value(), "WORKS_FOR");
    assert_eq!(
        PredicateType::ReducesDimensionality.value(),
        "REDUCES_DIMENSIONALITY"
    );
}

#[test]
fn loose_match() {
    assert_eq!(PredicateType::from_loose("uses"), PredicateType::Uses);
    assert_eq!(
        PredicateType::from_loose("is used by"),
        PredicateType::IsUsedBy
    );
    assert_eq!(
        PredicateType::from_loose("no such thing"),
        PredicateType::RelatedTo
    );
}

#[test]
fn resolve_reports_match_kind() {
    assert_eq!(
        PredicateType::resolve("uses"),
        (PredicateType::Uses, TypeMatch::Exact)
    );
    assert_eq!(
        PredicateType::resolve("is developed by"),
        (PredicateType::DevelopedBy, TypeMatch::Aliased)
    );
    assert_eq!(
        PredicateType::resolve("no such thing"),
        (PredicateType::RelatedTo, TypeMatch::Fallback)
    );
    assert_eq!(
        PredicateType::resolve(""),
        (PredicateType::RelatedTo, TypeMatch::Fallback)
    );
}

#[test]
fn loose_empty_relation_is_related_to() {
    assert_eq!(PredicateType::from_loose(""), PredicateType::RelatedTo);
    assert_eq!(PredicateType::from_loose("   "), PredicateType::RelatedTo);
}

#[test]
fn display_label_titlecase() {
    assert_eq!(
        Predicate::new(PredicateType::DevelopedBy).display_label(),
        "Developed By"
    );
}

#[test]
fn variant_count() {
    assert_eq!(PredicateType::all().len(), 108);
    assert_eq!(default_predicates().len(), 108);
}

#[test]
fn kg_vocab_v4_parse_semantics() {
    // kg-vocab v2 (`kg.vocab.v2`) intentionally changed loose predicate
    // parsing, v3 (`kg.vocab.v3`) refined it again, and v4 (`kg.vocab.v4`)
    // widened the curated disambiguation table (4 → 67 entries) from a
    // tie-only pin to a general curated surface-form mapping — these
    // assertions pin the current upstream behaviour:
    assert_eq!(kg_vocab::VOCAB_VERSION, "kg.vocab.v4");
    // Inputs normalising to <3 chars fall back without fuzzy matching
    // (v1 aliased "in" to LOCATED_IN).
    assert_eq!(
        PredicateType::resolve("in"),
        (PredicateType::RelatedTo, TypeMatch::Fallback)
    );
    // Longest variant wins on substring matches (v1 resolved "used" to
    // USED_IN; both USED_IN and IS_USED_BY contain it at a `_` boundary).
    assert_eq!(PredicateType::from_loose("used"), PredicateType::IsUsedBy);
    // Substring matching requires a `_` word boundary: "overfit" is a
    // bare stem of OVERFITS (no boundary after "OVERFIT"), so it falls
    // back instead of aliasing.
    assert_eq!(
        PredicateType::resolve("overfit"),
        (PredicateType::RelatedTo, TypeMatch::Fallback)
    );
    // v3: the curated disambiguation table wins before the fuzzy scan.
    // "tested"/"validated" intentionally CHANGE v2's declaration-order
    // results (TESTED_BY/VALIDATED_BY) to the _ON variants (ML corpus:
    // "tested/validated on <benchmark>" dominates); "invented"/
    // "published" pin v2's results (INVENTED_BY/PUBLISHED_IN).
    assert_eq!(
        PredicateType::resolve("tested"),
        (PredicateType::TestedOn, TypeMatch::Aliased)
    );
    assert_eq!(
        PredicateType::resolve("validated"),
        (PredicateType::ValidatedOn, TypeMatch::Aliased)
    );
    assert_eq!(
        PredicateType::from_loose("invented"),
        PredicateType::InventedBy
    );
    assert_eq!(
        PredicateType::from_loose("published"),
        PredicateType::PublishedIn
    );
    // v3: an equal-length tie among the longest substring matches is NOT
    // broken by declaration order — it falls back to RELATED_TO unless
    // the disambiguation table pins it ("tested by tested on" matches
    // TESTED_BY and TESTED_ON at equal length, and "TESTED BY TESTED ON"
    // itself is not a curated key).
    assert_eq!(
        PredicateType::resolve("tested by tested on"),
        (PredicateType::RelatedTo, TypeMatch::Fallback)
    );
}

#[test]
fn kg_vocab_v3_inverse_and_groups() {
    // Curated inverse pairs from vocab.json; unpaired predicates invert
    // to themselves (total-function semantics).
    assert_eq!(PredicateType::Uses.inverse(), PredicateType::IsUsedBy);
    assert_eq!(PredicateType::IsUsedBy.inverse(), PredicateType::Uses);
    assert_eq!(PredicateType::RelatedTo.inverse(), PredicateType::RelatedTo);
    // Group tables ship for domain-scoped schema trimming; not yet
    // consumed here (see module-level TODO).
    assert!(!kg_vocab::PREDICATE_GROUPS.is_empty());
    assert!(!kg_vocab::ENTITY_GROUPS.is_empty());
}
