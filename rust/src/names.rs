//! Human-readable ticket identifiers, in the same spirit as OMP's generated
//! AdjectiveNoun task names (for example, `QuietHarbor` or `SwiftFalcon`).
//!
//! Names are generated from a pair of small word pools and checked against the
//! current store. They remain opaque to Lyra: the ID is only a stable filename
//! and CLI handle, not a description of the work.

use std::collections::HashSet;
use uuid::Uuid;

const ADJECTIVES: &[&str] = &[
    "amber",
    "ancient",
    "bold",
    "brave",
    "bright",
    "calm",
    "clever",
    "cobalt",
    "crimson",
    "curious",
    "distant",
    "eager",
    "fabled",
    "gentle",
    "golden",
    "hollow",
    "ivory",
    "lucid",
    "mellow",
    "nimble",
    "quiet",
    "rapid",
    "silent",
    "silver",
    "steady",
    "sunlit",
    "swift",
    "tender",
    "vivid",
    "wandering",
    "wild",
    "witty",
    "zealous",
    "candid",
    "dusk",
    "fleet",
    "frost",
    "harbor",
    "kind",
    "lively",
    "misty",
    "noble",
    "patient",
    "prime",
    "restless",
    "sable",
    "scarlet",
    "shining",
    "still",
    "tidal",
    "velvet",
    "warm",
    "wren",
    "young",
];

const NOUNS: &[&str] = &[
    "anchor", "beacon", "brook", "canyon", "cedar", "cinder", "cloud", "comet", "cove", "dawn",
    "delta", "ember", "falcon", "fern", "finch", "forest", "garden", "glade", "harbor", "heron",
    "horizon", "lantern", "lattice", "lark", "meadow", "moon", "moss", "orbit", "otter", "pebble",
    "pine", "prism", "quill", "river", "rook", "sable", "shore", "sparrow", "star", "stone",
    "tide", "valley", "willow", "wind", "wren", "zenith", "atlas", "cipher", "grove", "hollow",
    "lumen", "ridge", "signal", "summit", "thicket", "vector", "vista", "wake",
];

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Generate a unique `AdjectiveNoun` id for a store.
///
/// `existing` should contain every current ticket id. The function retries a
/// bounded number of UUID-derived pairs before adding a numeric suffix, so it
/// remains deterministic enough to test and cannot silently duplicate a name.
pub fn generate_ticket_id(existing: &HashSet<String>) -> String {
    for _ in 0..256 {
        let bytes = Uuid::new_v4();
        let bytes = bytes.as_bytes();
        let adjective = ADJECTIVES[usize::from(bytes[0]) % ADJECTIVES.len()];
        let noun = NOUNS[usize::from(bytes[1]) % NOUNS.len()];
        let id = format!("{}{}", capitalize(adjective), capitalize(noun));
        if !existing.contains(&id) {
            return id;
        }
    }
    for suffix in 2..=u32::MAX {
        let bytes = Uuid::new_v4();
        let bytes = bytes.as_bytes();
        let adjective = ADJECTIVES[usize::from(bytes[0]) % ADJECTIVES.len()];
        let noun = NOUNS[usize::from(bytes[1]) % NOUNS.len()];
        let id = format!("{}{}{}", capitalize(adjective), capitalize(noun), suffix);
        if !existing.contains(&id) {
            return id;
        }
    }
    // Practically unreachable: the loop above would need every numeric suffix.
    "LyraTicket".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_camel_case_and_avoid_existing_ids() {
        let mut existing = HashSet::new();
        for _ in 0..200 {
            let name = generate_ticket_id(&existing);
            assert!(!name.starts_with("T-"));
            assert!(name.chars().next().is_some_and(char::is_uppercase));
            assert!(name.chars().any(char::is_lowercase));
            assert!(!existing.contains(&name));
            existing.insert(name);
        }
    }
}
