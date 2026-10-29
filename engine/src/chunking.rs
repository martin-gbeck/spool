// "[.!?]\s[A-Z]" or "\n\s[A-Z]"

use regex::Regex;

pub fn chunk_text(text: &str) -> Result<Vec<&str>, regex::Error> {
    const FIRST_BUDGET: usize = 300; // small opener = fast first audio
    const MAX_BUDGET: usize = 4000; // OpenAI TTS input cap is 4096 chars

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
            budget = (budget * 2).min(MAX_BUDGET);
        }
        last_sentc_end = sentence_end_idx;
    }
    if chunk_start_idx < text.len() {
        chunks.push(&text[chunk_start_idx..]);
    }

    Ok(chunks)
}
