use std::collections::BTreeMap;
use std::path::Path;

use draft_core::pack_generator::PackGenerator;
use draft_core::set_pool::LimitedSetPool;

#[derive(Default)]
pub struct DraftPools {
    pools: BTreeMap<String, LimitedSetPool>,
}

impl DraftPools {
    pub fn from_path(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        // Read the whole file, then parse from the in-memory slice. serde_json's
        // `from_reader` over an unbuffered `File` issues a read syscall per token
        // and is pathologically slow on Windows, where per-syscall cost is high:
        // parsing this multi-megabyte pool file that way stalled the native-engine
        // server past the desktop shell's 20s health-check budget, so games fell
        // back to the in-browser engine. `from_slice` parses contiguous memory with
        // no per-read overhead (mirrors the `BufReader` already used in card_db.rs).
        let bytes = std::fs::read(path)?;
        let pools: BTreeMap<String, LimitedSetPool> = serde_json::from_slice(&bytes)?;
        let pools = pools
            .into_iter()
            .map(|(code, pool)| (code.to_lowercase(), pool))
            .collect();
        Ok(Self { pools })
    }

    pub fn len(&self) -> usize {
        self.pools.len()
    }

    /// The pool entry for `set_code`, matched case-insensitively.
    pub fn pool_for_set(&self, set_code: &str) -> Option<&LimitedSetPool> {
        self.pools.get(&set_code.to_lowercase())
    }

    /// A generator that opens one booster per entry of `set_codes`, in pack
    /// order. Names the first code with no pool data rather than silently
    /// dropping its packs; the same code may appear more than once, and its
    /// pool is still carried once.
    ///
    /// The whole sequence resolves here because a draft's boosters are a
    /// per-pack property (`DraftSource::Set`) — a multi-set pod that resolved
    /// only its first code would deal every later pack from the wrong set.
    pub fn generator_for_sequence(&self, set_codes: &[String]) -> Result<PackGenerator, String> {
        let mut pools: Vec<LimitedSetPool> = Vec::new();
        for code in set_codes {
            let pool = self
                .pool_for_set(code)
                .ok_or_else(|| format!("No draft pool data for set: {code}"))?;
            if !pools.iter().any(|held| held.code == pool.code) {
                pools.push(pool.clone());
            }
        }
        PackGenerator::for_sequence(pools, set_codes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn loads_pools_by_case_insensitive_set_code() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            r#"{{
                "TST": {{
                    "code": "TST",
                    "name": "Test Set",
                    "release_date": null,
                    "pack_variants": [],
                    "pack_variants_total_weight": 0,
                    "sheets": {{}},
                    "prints": [],
                    "basic_lands": []
                }}
            }}"#
        )
        .unwrap();

        let pools = DraftPools::from_path(file.path()).unwrap();

        assert_eq!(pools.len(), 1);
        assert!(pools.pool_for_set("TST").is_some());
        assert!(pools.pool_for_set("tst").is_some());
        assert!(pools.generator_for_sequence(&["TST".to_string()]).is_ok());
        assert!(pools
            .generator_for_sequence(&["missing".to_string()])
            .is_err());
    }

    /// A pod may draft the same set in several packs and different sets in
    /// others; both must resolve off one pool map, and a code with no data must
    /// name itself rather than yielding a short draft.
    #[test]
    fn resolves_a_repeated_and_mixed_pack_sequence() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{}", two_set_pool_json()).unwrap();

        let pools = DraftPools::from_path(file.path()).unwrap();

        assert!(pools
            .generator_for_sequence(&["AAA".to_string(), "BBB".to_string(), "AAA".to_string()])
            .is_ok());
        assert!(pools
            .generator_for_sequence(&["aaa".to_string(), "bbb".to_string()])
            .is_ok());
        // `PackGenerator` derives no `Debug`, so the error is read off the
        // `Err` arm rather than through `unwrap_err`.
        assert_eq!(
            pools
                .generator_for_sequence(&["AAA".to_string(), "ZZZ".to_string()])
                .err(),
            Some("No draft pool data for set: ZZZ".to_string())
        );
        assert!(pools.generator_for_sequence(&[]).is_err());
    }

    fn two_set_pool_json() -> String {
        let entry = |code: &str| {
            format!(
                r#""{code}": {{
                    "code": "{code}",
                    "name": "Set {code}",
                    "release_date": null,
                    "pack_variants": [],
                    "pack_variants_total_weight": 0,
                    "sheets": {{}},
                    "prints": [],
                    "basic_lands": []
                }}"#
            )
        };
        format!("{{{}, {}}}", entry("AAA"), entry("BBB"))
    }
}
