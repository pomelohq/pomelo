//! Reads this Mac's Claude Code transcripts. Opt-in: `cargo test -p agent_usage --test real_transcripts -- --ignored --nocapture`.

#[test]
#[ignore]
fn reads_the_last_week_of_this_macs_transcripts() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let since = agent_usage::unix_now() - 7 * 86_400;
    let started = std::time::Instant::now();
    let mut transcripts = agent_usage::Transcripts::default();
    transcripts.refresh(&home.join(".claude/projects"), since);
    let first = started.elapsed();
    let again = std::time::Instant::now();
    transcripts.refresh(&home.join(".claude/projects"), since);
    let turns = transcripts.turns(since);
    let tokens: u64 = turns.iter().map(|turn| turn.tokens()).sum();
    let cost: f64 = turns.iter().map(|turn| agent_usage::cost(turn)).sum();
    println!(
        "{} turns, {} tokens, ${cost:.2}; first read {first:?}, again {:?}",
        turns.len(),
        tokens,
        again.elapsed()
    );
    println!(
        "account: {:?}",
        agent_usage::read_account(&home).map(|account| account.plan)
    );
}

#[test]
#[ignore]
fn fetches_this_macs_limits() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    println!("limits: {:?}", agent_usage::fetch_limits(&home));
}
