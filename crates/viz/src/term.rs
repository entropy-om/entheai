//! Terminal capability probe for the Kitty graphics protocol (used by Slice 2's
//! shader; exposed now so the TUI can label/gate viz features).

/// Pure decision from the two relevant env values — testable without touching
/// the real environment.
fn is_graphics_term(term_program: Option<&str>, term: Option<&str>) -> bool {
    let tp = term_program.unwrap_or("").to_ascii_lowercase();
    if tp.contains("ghostty") || tp.contains("wezterm") || tp.contains("kitty") {
        return true;
    }
    let t = term.unwrap_or("").to_ascii_lowercase();
    t.contains("kitty")
}

/// True when the current terminal supports the Kitty graphics protocol
/// (Ghostty / Kitty / WezTerm). Reads `$TERM_PROGRAM` and `$TERM`.
pub fn graphics_capable() -> bool {
    is_graphics_term(
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_kitty_graphics_terminals() {
        assert!(is_graphics_term(Some("ghostty"), None));
        assert!(is_graphics_term(Some("WezTerm"), None));
        assert!(is_graphics_term(None, Some("xterm-kitty")));
        assert!(!is_graphics_term(
            Some("Apple_Terminal"),
            Some("xterm-256color")
        ));
        assert!(!is_graphics_term(None, None));
    }
}
