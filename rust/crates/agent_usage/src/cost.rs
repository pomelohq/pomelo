use crate::Turn;

/// Dollars per million tokens: (input, output, cache read, cache write).
fn prices(model: &str) -> (f64, f64, f64, f64) {
    let model = model.to_ascii_lowercase();
    if model.contains("opus") {
        (15.0, 75.0, 1.5, 18.75)
    } else if model.contains("haiku") {
        (1.0, 5.0, 0.1, 1.25)
    } else {
        (3.0, 15.0, 0.3, 3.75)
    }
}

/// What a turn would cost at API prices (an estimate: a subscription is not billed per token).
pub fn cost(turn: &Turn) -> f64 {
    let (input, output, cache_read, cache_write) = prices(&turn.model);
    (turn.input as f64 * input
        + turn.output as f64 * output
        + turn.cache_read as f64 * cache_read
        + turn.cache_write as f64 * cache_write)
        / 1_000_000.0
}
