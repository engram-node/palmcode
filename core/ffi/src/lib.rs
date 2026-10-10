//! UniFFI bindings for the pocket core.
//!
//! M0: smoke-test surface only (proves the Rust <-> Kotlin path end to end).
//! The real session/agent API lands in M1.

uniffi::setup_scaffolding!();

/// Core crate version, for UI diagnostics and crash-censored logs.
#[uniffi::export]
pub fn pocket_core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// M0 smoke test.
#[uniffi::export]
pub fn pocket_greet(who: String) -> String {
    format!(
        "pocket core {} online, hello {who}",
        env!("CARGO_PKG_VERSION")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_mentions_version() {
        let g = pocket_greet("test".to_string());
        assert!(g.contains(env!("CARGO_PKG_VERSION")));
        assert!(g.ends_with("hello test"));
    }
}
