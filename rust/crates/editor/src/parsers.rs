//! Parsers are pooled, each holding a wasm store, so a grammar loaded from a package parses like a compiled-in
//! one on any thread.

use std::sync::{LazyLock, Mutex};

use tree_sitter::{wasmtime, Parser, WasmStore};

/// One engine for every store: compiled grammar code is shared between them.
static ENGINE: LazyLock<wasmtime::Engine> = LazyLock::new(wasmtime::Engine::default);

static PARSERS: Mutex<Vec<Parser>> = Mutex::new(Vec::new());

fn new_parser() -> Parser {
    let mut parser = Parser::new();
    match WasmStore::new(&ENGINE) {
        Ok(store) => {
            if let Err(error) = parser.set_wasm_store(store) {
                eprintln!("grammars: wasm store: {error}");
            }
        }
        Err(error) => eprintln!("grammars: wasm store: {}", error.message),
    }
    parser
}

/// Runs `work` with a parser from the pool, reset first: a cancelled parse leaves state that the next parse
/// would otherwise resume.
pub(crate) fn with_parser<R>(work: impl FnOnce(&mut Parser) -> R) -> R {
    let pooled = PARSERS.lock().ok().and_then(|mut parsers| parsers.pop());
    let mut parser = pooled.unwrap_or_else(new_parser);
    parser.reset();
    if let Err(error) = parser.set_included_ranges(&[]) {
        eprintln!("grammars: reset included ranges: {error}");
    }
    let result = work(&mut parser);
    if let Ok(mut parsers) = PARSERS.lock() {
        parsers.push(parser);
    }
    result
}

/// Compiles the grammar `name` from its wasm `bytes`. Slow (tens of ms), so callers run it off the UI thread.
pub(crate) fn load_wasm_grammar(name: &str, bytes: &[u8]) -> Result<tree_sitter::Language, String> {
    with_parser(|parser| {
        let mut store = match parser.take_wasm_store() {
            Some(store) => store,
            None => WasmStore::new(&ENGINE).map_err(|error| error.message)?,
        };
        let loaded = store
            .load_language(name, bytes)
            .map_err(|error| error.message);
        if let Err(error) = parser.set_wasm_store(store) {
            eprintln!("grammars: wasm store: {error}");
        }
        loaded
    })
}
