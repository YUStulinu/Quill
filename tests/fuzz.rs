//! A short differential-testing run as part of `cargo test` (the full run is `quill fuzz N`).
//! QUILL_FUZZ_COUNT raises the number of programs.

#[test]
fn random_programs_agree_everywhere() {
    let count: u64 = std::env::var("QUILL_FUZZ_COUNT").ok().and_then(|s| s.parse().ok()).unwrap_or(12);
    let work = std::env::temp_dir().join(format!("quill-fuzz-test-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let mut failures = Vec::new();
    for seed in 500..500 + count {
        let src = quill::fuzzgen::generate(seed);
        if let Some(why) = quill::fuzz_cmd::check_one(seed, &src, &work).unwrap() {
            failures.push(format!("seed {seed}: {why}\n{src}"));
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn generator_is_deterministic() {
    assert_eq!(quill::fuzzgen::generate(42), quill::fuzzgen::generate(42));
    assert_ne!(quill::fuzzgen::generate(42), quill::fuzzgen::generate(43));
}
