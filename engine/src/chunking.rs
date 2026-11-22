// "[.!?]\s[A-Z]" or "\n\s[A-Z]"

use regex::Regex;

/// OpenAI's TTS input cap is 4096 chars.
pub const OPENAI_MAX: usize = 4000;
/// Speechify's input limit is lower; the vault's Speechify tool stays under it at 1800.
pub const SPEECHIFY_MAX: usize = 1800;

pub fn chunk_text(text: &str, max_budget: usize) -> Result<Vec<&str>, regex::Error> {
    const FIRST_BUDGET: usize = 300; // small opener = fast first audio

    let re = Regex::new(r"[.!?]\s+([A-ZØÅÆ])|\n\s*")?;

    let mut chunks: Vec<&str> = Vec::new();

    let mut chunk_start_idx: usize = 0;

    let mut budget = FIRST_BUDGET;

    let mut last_sentc_end: usize = 0;

    for capture in re.captures_iter(text) {
        // byte_idx is the byte that ends a sentence
        let sentence_end_idx = match capture.get(1) {
            Some(g1) => g1.start(),
            None => capture
                .get(0)
                .expect("Impossible state, because get(0) always will return something")
                .end(),
        };

        if sentence_end_idx - chunk_start_idx > budget && last_sentc_end > chunk_start_idx {
            let chunk = &text[chunk_start_idx..last_sentc_end];

            chunks.push(chunk);
            chunk_start_idx = last_sentc_end;
            budget = (budget * 2).min(max_budget);
        }
        last_sentc_end = sentence_end_idx;
    }
    if chunk_start_idx < text.len() {
        chunks.push(&text[chunk_start_idx..]);
    }

    // A single long sentence, or a long tail with no sentence break, can still
    // be over the API's input limit: cut those at a space, else at a char edge.
    Ok(chunks.into_iter().flat_map(|c| split_to_fit(c, max_budget)).collect())
}

fn split_to_fit(mut text: &str, max: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    while text.len() > max {
        let mut cut = max;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        if let Some(space) = text[..cut].rfind(char::is_whitespace) {
            if space > 0 {
                cut = space + 1;
            }
        }
        parts.push(&text[..cut]);
        text = &text[cut..];
    }
    if !text.is_empty() {
        parts.push(text);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_chunk_is_over_the_limit() {
        let text = "word ".repeat(2000);
        let chunks = chunk_text(&text, SPEECHIFY_MAX).unwrap();
        assert!(chunks.iter().all(|c| c.len() <= SPEECHIFY_MAX));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn multibyte_text_splits_on_char_edges() {
        let text = "æøå".repeat(1500);
        let chunks = chunk_text(&text, SPEECHIFY_MAX).unwrap();
        assert!(chunks.iter().all(|c| c.len() <= SPEECHIFY_MAX));
        assert_eq!(chunks.concat(), text);
    }
}
