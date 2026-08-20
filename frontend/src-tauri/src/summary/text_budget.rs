//! Fitting text into a model's context: the one place that converts between
//! tokens and characters, trims an oversized document, and splits a budget
//! across several documents.
//!
//! These were previously three separate reckonings — `rough_token_count`'s
//! 0.35 tokens/char in the summary path, a hardcoded 4 chars/token in the chat
//! path, and an inline head/tail cut inside `build_transcript_text`. They only
//! ever disagreed harmlessly because a single meeting never approached the
//! budget. Project-level context is the first caller that sits *at* the limit,
//! where a 40% overestimate is the difference between a full answer and a
//! silently truncated one.

/// Tokens per character, measured against this app's mixed English/Spanish
/// transcripts. The inverse (~2.857 chars/token) is deliberately more
/// conservative than the ~4 chars/token that holds for English-only ASCII:
/// Spanish tokenizes worse, and over-filling a context is a silent failure
/// (the provider truncates or 400s) while under-filling merely wastes room.
pub const TOKENS_PER_CHAR: f64 = 0.35;

/// Rough token count from a character count.
pub fn rough_token_count(s: &str) -> usize {
    (s.chars().count() as f64 * TOKENS_PER_CHAR).ceil() as usize
}

/// How many characters fit in `tokens` tokens.
pub fn chars_for_tokens(tokens: usize) -> usize {
    (tokens as f64 / TOKENS_PER_CHAR).floor() as usize
}

/// Trim `text` to `max_chars`, keeping the beginning and the end and eliding
/// the middle with a marker saying how much was dropped.
///
/// Keeping both ends matters for a transcript: an earlier head-only cut hid the
/// end of every long meeting, so "what did we decide at the end?" always failed.
/// Char-boundary safe. `usize::MAX` means "no limit" and returns the input
/// untouched.
pub fn elide_middle(text: &str, max_chars: usize) -> String {
    if max_chars == usize::MAX || text.chars().count() <= max_chars {
        return text.to_string();
    }
    // Degenerate budgets can't hold a marker, let alone two halves of content.
    if max_chars == 0 {
        return String::new();
    }

    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    let head_len = (max_chars * 3) / 5; // 60% opening
    let tail_len = max_chars - head_len; // 40% conclusion
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[total - tail_len..].iter().collect();
    let omitted = total - head_len - tail_len;
    format!(
        "{head}\n\n[... {omitted} characters from the middle of the transcript omitted for length ...]\n\n{tail}"
    )
}

/// Split `budget` characters across items of the given `lengths`.
///
/// Max-min fair (water-filling): every item is guaranteed at least an equal
/// share, and whatever the short items don't use is handed back to the long
/// ones. Returned in the caller's original order.
///
/// The two obvious alternatives are both wrong for a project: an equal cap
/// wastes most of the budget when most summaries are short, and no cap at all
/// lets one 40k-character summary starve the other nine meetings entirely.
pub fn allocate_fair_shares(lengths: &[usize], budget: usize) -> Vec<usize> {
    let n = lengths.len();
    if n == 0 {
        return Vec::new();
    }

    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| lengths[i]);

    let mut shares = vec![0usize; n];
    let mut remaining = budget;
    let mut left = n;

    // Shortest first: an item that fits inside its equal share takes only what
    // it needs, which raises the share available to everything after it.
    for &i in &order {
        let equal_share = remaining / left;
        let take = lengths[i].min(equal_share);
        shares[i] = take;
        remaining -= take;
        left -= 1;
    }
    shares
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elide_middle_keeps_both_ends_and_reports_the_gap() {
        let text: String = std::iter::repeat('a')
            .take(500)
            .chain(std::iter::repeat('z').take(500))
            .collect();
        let out = elide_middle(&text, 100);

        assert!(out.starts_with("aaa"), "keeps the opening");
        assert!(out.ends_with("zzz"), "keeps the conclusion");
        assert!(out.contains("900 characters from the middle"));
    }

    #[test]
    fn elide_middle_is_a_noop_when_it_fits_or_is_unbounded() {
        assert_eq!(elide_middle("short", 100), "short");
        assert_eq!(elide_middle("short", usize::MAX), "short");
        // Exactly at the limit is still untouched.
        assert_eq!(elide_middle("12345", 5), "12345");
    }

    /// The 60/40 split is over characters, not bytes — a multi-byte transcript
    /// must not panic or produce invalid UTF-8.
    #[test]
    fn elide_middle_is_char_boundary_safe() {
        let text: String = std::iter::repeat('á').take(400).collect();
        let out = elide_middle(&text, 50);
        assert!(out.starts_with('á'));
        assert!(out.ends_with('á'));
    }

    #[test]
    fn fair_shares_hand_surplus_from_short_items_to_long_ones() {
        // Equal shares would be 100 each; the two short items need only 30,
        // so the long one should receive the leftover rather than waste it.
        let shares = allocate_fair_shares(&[10, 20, 5_000], 300);
        assert_eq!(shares[0], 10);
        assert_eq!(shares[1], 20);
        assert_eq!(shares[2], 270);
        assert_eq!(shares.iter().sum::<usize>(), 300);
    }

    #[test]
    fn fair_shares_never_exceed_the_budget_or_the_need() {
        let shares = allocate_fair_shares(&[5_000, 5_000, 5_000], 300);
        assert_eq!(shares, vec![100, 100, 100]);

        // Everything fits: nobody is trimmed and the budget is not spent.
        let shares = allocate_fair_shares(&[10, 20, 30], 1_000);
        assert_eq!(shares, vec![10, 20, 30]);
    }

    #[test]
    fn fair_shares_handles_empty_and_zero_budget() {
        assert!(allocate_fair_shares(&[], 100).is_empty());
        assert_eq!(allocate_fair_shares(&[10, 20], 0), vec![0, 0]);
    }

    #[test]
    fn token_and_char_conversions_are_inverses() {
        assert_eq!(rough_token_count("abcd"), 2); // ceil(4 * 0.35)
        // ~2.857 chars/token, so a 1000-token budget is ~2857 characters.
        assert_eq!(chars_for_tokens(1_000), 2857);
    }
}
