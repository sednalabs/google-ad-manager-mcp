//! Deterministic, non-secret fingerprints for bounded provider evidence.

pub(crate) fn stable_fingerprint(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::stable_fingerprint;

    #[test]
    fn stable_fingerprint_matches_fnv1a_vectors() {
        assert_eq!(stable_fingerprint(""), "cbf29ce484222325");
        assert_eq!(stable_fingerprint("a"), "af63dc4c8601ec8c");
    }
}
