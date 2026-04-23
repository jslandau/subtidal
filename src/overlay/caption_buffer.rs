//! Caption text accumulator — pure logic, GTK-free.
//!
//! Fill-and-shift line model: words fill each line up to `max_chars_per_line`;
//! when all lines are full and new text arrives, the oldest line shifts off.
//! Lines individually expire after `expire_secs` of silence. Streaming RNNT
//! decoders can re-emit their tail, so overlap dedup on 4+ char matches strips
//! the duplicate prefix.

use std::time::Instant;

/// Represents one line of caption text with a timestamp for expiry.
pub struct CaptionLine {
    pub text: String,
    pub last_active: Instant,
}

/// Buffer that accumulates caption text in lines with fill-and-shift model.
pub struct CaptionBuffer {
    /// Ordered lines from oldest (top, shown first) to newest (bottom, shown last).
    pub lines: Vec<CaptionLine>,
    pub max_lines: usize,
    pub max_chars_per_line: usize,
    pub expire_secs: u64,
    /// Track the last few words to detect and skip repeated output from the RNNT decoder.
    pub last_tail: String,
}

impl CaptionBuffer {
    pub fn new(max_lines: usize, max_chars_per_line: usize, expire_secs: u64) -> Self {
        CaptionBuffer {
            lines: Vec::new(),
            max_lines,
            max_chars_per_line,
            expire_secs,
            last_tail: String::new(),
        }
    }

    /// Add a new caption fragment, deduplicating overlapping text from streaming RNNT.
    /// Preserves leading/trailing whitespace from the engine — these signal word
    /// boundaries (e.g. " ve" = new word, "ve" = continuation of previous word).
    pub fn push(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }

        // Deduplicate: if the new text starts with the end of what we already have,
        // skip the overlapping prefix. Streaming RNNT decoders sometimes re-emit
        // the tail of the previous output as the start of the next.
        let deduped = Self::remove_overlap(&self.last_tail, text.trim());
        if deduped.is_empty() {
            return;
        }

        // Preserve the leading space from the original engine output if present.
        // This signals a word boundary vs. a mid-word continuation.
        let fragment = if text.starts_with(char::is_whitespace) && !deduped.starts_with(char::is_whitespace) {
            format!(" {deduped}")
        } else {
            deduped.clone()
        };

        // Determine if this is a continuation fragment (no leading space and lines are not empty).
        let is_continuation = !fragment.starts_with(char::is_whitespace) && !self.lines.is_empty();

        if is_continuation {
            // Continuation: join with the last word on the current line.
            let idx = self.lines.len() - 1;
            let combined = format!("{}{}", self.lines[idx].text.clone(), fragment);

            if combined.len() <= self.max_chars_per_line {
                // Fits on current line: append directly.
                self.lines[idx].text = combined;
                self.lines[idx].last_active = Instant::now();
            } else {
                // Would overflow current line: move partial word to next line.
                if let Some(last_space_pos) = self.lines[idx].text.rfind(' ') {
                    // Split at last space: keep everything up to and including the space,
                    // move the partial word after the space.
                    let partial_word = self.lines[idx].text[last_space_pos + 1..].to_string();
                    self.lines[idx].text = self.lines[idx].text[..=last_space_pos].trim_end().to_string();

                    // Add new line with partial + continuation joined.
                    self.add_new_line(format!("{}{}", partial_word, fragment));
                } else {
                    // Entire line is one word with no space: start fresh on new line.
                    // Remove the old line before calling add_new_line to avoid stale index
                    // if add_new_line shifts (when buffer is at max_lines capacity).
                    let old_text = self.lines.remove(idx).text;
                    self.add_new_line(format!("{}{}", old_text, fragment));
                }
            }
        } else {
            // Not a continuation: split into words and fill lines normally.
            let words: Vec<&str> = fragment.split_whitespace().collect();
            for word in words {
                if word.is_empty() {
                    continue;
                }

                if self.lines.is_empty() {
                    // Start a new line with this word.
                    self.add_new_line(word.to_string());
                } else {
                    let idx = self.lines.len() - 1;

                    if self.lines[idx].text.is_empty() {
                        // Current line is empty: place word directly (no space prefix).
                        self.lines[idx].text = word.to_string();
                    } else if self.lines[idx].text.len() + 1 + word.len() <= self.max_chars_per_line {
                        // Room on current line: append with space.
                        self.lines[idx].text.push(' ');
                        self.lines[idx].text.push_str(word);
                    } else {
                        // Overflow: start new line (shifts if at max_lines).
                        self.add_new_line(word.to_string());
                    }
                }
            }
        }

        // Update last_active on the last line (most recent text).
        if !self.lines.is_empty() {
            let idx = self.lines.len() - 1;
            self.lines[idx].last_active = Instant::now();
        }

        // Rebuild tail for overlap detection.
        let display = self.all_text();
        let tail_start = display.len().saturating_sub(60);
        self.last_tail = display[tail_start..].to_string();
    }

    /// Add a new line, shifting off the oldest line if at max_lines capacity.
    fn add_new_line(&mut self, text: String) {
        if self.lines.len() >= self.max_lines {
            self.lines.remove(0); // Remove oldest (top) line.
        }
        self.lines.push(CaptionLine {
            text,
            last_active: Instant::now(),
        });
    }

    /// Join all line text with empty string. Each line's text is properly spaced already.
    fn all_text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("")
    }

    /// Remove overlapping prefix between existing tail and new text.
    /// Only triggers on overlaps of 4+ characters to avoid false positives
    /// from coincidental single-character matches.
    fn remove_overlap(tail: &str, new: &str) -> String {
        if tail.is_empty() {
            return new.to_string();
        }
        let tail_lower = tail.to_lowercase();
        let new_lower = new.to_lowercase();

        // Only consider overlaps of 4+ characters to avoid false positives.
        let max_check = tail_lower.len().min(new_lower.len());
        for overlap_len in (4..=max_check).rev() {
            let tail_suffix = &tail_lower[tail_lower.len() - overlap_len..];
            let new_prefix = &new_lower[..overlap_len];
            if tail_suffix == new_prefix {
                let remainder = new[overlap_len..].trim_start();
                if !remainder.is_empty() {
                    return remainder.to_string();
                }
            }
        }
        new.to_string()
    }

    /// Remove the oldest line if its last_active timestamp is older than expire_secs.
    /// Only removes one line per call (gradual drain). Returns true if a line was removed.
    pub fn expire(&mut self) -> bool {
        if self.lines.is_empty() {
            return false;
        }

        let cutoff = Instant::now() - std::time::Duration::from_secs(self.expire_secs);
        if self.lines[0].last_active <= cutoff {
            self.lines.remove(0);
            // Rebuild tail after removal.
            let display = self.all_text();
            let tail_start = display.len().saturating_sub(60);
            self.last_tail = display[tail_start..].to_string();
            true
        } else {
            false
        }
    }

    /// Join all lines with newline separators for display.
    pub fn display_text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    /// Update the buffer's configuration (max_chars_per_line and expire_secs).
    /// Called when appearance config changes via hot-reload.
    pub fn update_config(&mut self, max_lines: usize, max_chars_per_line: usize, expire_secs: u64) {
        self.max_lines = max_lines;
        self.max_chars_per_line = max_chars_per_line;
        self.expire_secs = expire_secs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC1.1: Text fills line 1 left-to-right, word by word, up to max_chars_per_line.
    #[test]
    fn ac1_1_fill_single_line() {
        let mut buf = CaptionBuffer::new(3, 20, 8);

        // Push words with leading spaces (word boundaries).
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());
        buf.push(" this".to_string());

        let display = buf.display_text();
        assert_eq!(display, "Hello world this", "Words should fill single line");
        assert!(!display.contains('\n'), "Should not have newline separator");
    }

    /// AC1.2: When line 1 is full, text continues on line 2 (up to max_lines).
    #[test]
    fn ac1_2_overflow_to_second_line() {
        let mut buf = CaptionBuffer::new(3, 15, 8);

        // Fill line 1 with "Hello world" (11 chars).
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());

        // Next word "this" (4 chars) won't fit (11 + 1 + 4 = 16 > 15).
        buf.push(" this".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 2, "Should have 2 lines");
        assert_eq!(lines[0], "Hello world");
        assert_eq!(lines[1], "this");
    }

    /// AC1.3: When all lines are full and new text arrives, line 1 is removed,
    /// all lines shift up, and new text fills the freed bottom line.
    #[test]
    fn ac1_3_shift_when_all_lines_full() {
        let mut buf = CaptionBuffer::new(2, 7, 8);

        // Fill line 1: " Hello" (5 chars, fits in 7).
        buf.push(" Hello".to_string());

        // Add word that goes to line 2: "Hello world" = 11 chars > 7, so "world" goes to line 2 (5 chars).
        buf.push(" world".to_string());

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines filled");
        assert_eq!(buf.lines[0].text, "Hello");
        assert_eq!(buf.lines[1].text, "world");

        // Add third word: "Hello world test" = " test" (4 chars) won't fit on line 2 (5+1+4=10 > 7),
        // so it goes to new line. Since we're at max_lines=2, oldest line (line 1: "Hello") shifts off.
        buf.push(" test".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 2, "Should still have max_lines=2 after shift");
        assert_eq!(lines[0], "world", "Line 1 should be old line 2");
        assert_eq!(lines[1], "test", "Line 2 should be new content");
    }

    /// AC1.4: Continuation fragments (no leading space) join the previous word
    /// on the same line without inserting a space.
    #[test]
    fn ac1_4_continuation_no_space() {
        let mut buf = CaptionBuffer::new(3, 20, 8);

        // Push " Hel" (word boundary).
        buf.push(" Hel".to_string());
        // Push "lo" (continuation, no leading space).
        buf.push("lo".to_string());

        let display = buf.display_text();
        assert_eq!(display, "Hello", "Continuation should join without space");
    }

    /// AC1.5: When a continuation fragment would cause the combined word to overflow
    /// the current line, the partial word moves to the next line and joins there.
    /// Tests the "with space" branch where we split at last space.
    #[test]
    fn ac1_5_partial_word_overflow() {
        let mut buf = CaptionBuffer::new(3, 10, 8);

        // Set up: Line 1: "Hello" (5), Line 2: "world" (5)
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());

        // Line 2 is now "world" (5 chars). Add another word " more" (5 chars).
        // "world more" = 10 chars, exactly fits.
        buf.push(" more".to_string());

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines before overflow");
        assert_eq!(buf.lines[1].text, "world more");

        // Current line 2: "world more" (10 chars). Push continuation "text" (4 chars).
        // Appending "text" to last word "more": "moretext" (8 chars).
        // Adding to current line: 10 + 8 = 18 > 10, overflow!
        // Last space in "world more" at position 5.
        // Split: keep "world", move "more" to new line.
        // New line 3: "more" + "text" = "moretext" (8 chars).
        buf.push("text".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 3, "Should have 3 lines after split");
        assert_eq!(lines[0], "Hello", "Line 1 should have 'Hello'");
        assert_eq!(lines[1], "world", "Line 2 should have 'world' (split off)");
        assert_eq!(lines[2], "moretext", "Line 3 should have 'more' + 'text' joined");
    }

    /// AC1.5 extended: "no space" branch at full max_lines capacity.
    /// When last line is a single word and continuation overflows with no space,
    /// the old line is removed and replaced with the joined word.
    /// This tests the critical bug fix where stale index could clear the wrong line.
    #[test]
    fn ac1_5_continuation_no_space_at_full_capacity() {
        let mut buf = CaptionBuffer::new(3, 7, 8); // max_lines=3, max_chars=7

        // Create three single-word lines to fill buffer to max_lines.
        buf.push(" one".to_string());   // Line 1: "one" (3 chars)
        buf.push(" two".to_string());   // Line 1: "one two" = 7, fits exactly
        buf.push(" three".to_string()); // "one two three" = 13 > 7, goes to line 2: "three" (5 chars)
        buf.push(" four".to_string());  // "three four" = 10 > 7, goes to line 3: "four" (4 chars)

        assert_eq!(buf.lines.len(), 3, "Buffer should be full at max_lines=3");
        assert_eq!(buf.lines[0].text, "one two");
        assert_eq!(buf.lines[1].text, "three");
        assert_eq!(buf.lines[2].text, "four");

        // Now buffer is full and all 3 lines exist. Push continuation on last line that overflows.
        // Current line 3: "four" (4 chars). Continuation "more" (4 chars).
        // Combined: "fourmore" (8 chars) > 7. No space in "four", so the whole line moves.
        // add_new_line will remove line 0 and add new line, resulting in:
        // ["three", "four", "fourmore"]
        buf.push("more".to_string());

        // Verify: no empty lines and correct content.
        assert_eq!(buf.lines.len(), 3, "Should still have max_lines=3");
        assert_eq!(buf.lines[0].text, "one two", "Line 1 unchanged");
        assert_eq!(buf.lines[1].text, "three", "Line 2 unchanged");
        assert_eq!(buf.lines[2].text, "fourmore", "Line 3 has joined word replacing old 'four'");

        let display = buf.display_text();
        assert!(display.contains("one two"), "Should contain 'one two'");
        assert!(display.contains("three"), "Should contain 'three'");
        assert!(display.contains("fourmore"), "Should contain 'fourmore'");
        assert_eq!(display.lines().count(), 3, "Display should have 3 lines");
    }

    /// AC1.5 extended: "with space" continuation overflow branch.
    /// When last line has multiple words and continuation overflows, the partial word
    /// after the last space moves to next line and joins the continuation.
    #[test]
    fn ac1_5_continuation_with_space_overflow() {
        let mut buf = CaptionBuffer::new(3, 20, 8);

        // Set up line 1: "Hello world" (11 chars, fits in 20)
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());
        assert_eq!(buf.lines[0].text, "Hello world");

        // Current line: "Hello world" (11 chars). Push continuation "ly" (2 chars).
        // Combined: "world" + "ly" = 7 chars, fits in 20. ✓
        buf.push("ly".to_string());
        assert_eq!(buf.lines[0].text, "Hello worldly");

        // Now make line nearly full and overflow. Reset for clearer setup.
        buf = CaptionBuffer::new(3, 18, 8);
        buf.push(" Hello".to_string());         // Line 1: "Hello" (5 chars)
        buf.push(" world".to_string());         // Line 1: "Hello world" (11 chars)

        // Current line: "Hello world" (11 chars). Push continuation "ly" (2 chars) that fits.
        buf.push("ly".to_string());             // Line 1: "Hello worldly" (13 chars)

        // Now push word that forces split. Current line: "Hello worldly" (13 chars).
        // Word " test" (5 chars): 13 + 1 + 5 = 19 > 18, doesn't fit.
        // Goes to line 2.
        buf.push(" test".to_string());          // Line 2: "test" (4 chars)

        // Current line 2: "test" (4 chars). Push continuation that overflows.
        // "test" + "something" = 13 chars > 18? No, 13 < 18, fits. Let's use longer continuation.
        // "test" + "ingsomething" = 16 chars, fits in 18. Hmm, still fits.
        // Let's be more aggressive: use continuation that definitely overflows.
        // "test" + "verylongcontinuation" = too long.
        buf.push("verylongcontinuation".to_string()); // "test" + "verylongcontinuation" = 24 > 18

        // This overflows. Line 2 is "test" (no space). Last space in "test"? None.
        // So the "no space" branch triggers, which just moves entire line to new line.
        // That's not the "with space" branch.

        // Let's retest more carefully to exercise "with space" branch:
        buf = CaptionBuffer::new(3, 18, 8);
        buf.push(" Hello".to_string());         // Line 1: "Hello" (5 chars)
        buf.push(" world".to_string());         // Line 1: "Hello world" (11 chars)
        buf.push(" more".to_string());          // Line 1: "Hello world more" (16 chars, fits)

        // Current line 1: "Hello world more" (16 chars, 2 chars left before max).
        // Push continuation "text" (4 chars).
        // "more" + "text" = 8 chars. 16 + 8 = 24 > 18. Overflow!
        // Last space in "Hello world more"? Yes, at position 11 (after "world").
        // Split: keep "Hello world " (12 chars), move "more" to next line.
        // New line: "moretext" (8 chars).
        buf.push("text".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 2, "Should have 2 lines after split");
        assert_eq!(lines[0], "Hello world", "First line should be trimmed to 'Hello world'");
        assert_eq!(lines[1], "moretext", "Second line should have partial word + continuation joined");
    }

    /// AC1.6: RNNT decoder overlap is deduplicated (4+ char matches).
    #[test]
    fn ac1_6_overlap_deduplication() {
        let mut buf = CaptionBuffer::new(3, 50, 8);

        buf.push(" The quick brown".to_string());
        // Simulating RNNT decoder re-emitting "brown fox" where "brown" already output.
        buf.push(" brown fox".to_string());

        let display = buf.display_text();
        assert_eq!(display, "The quick brown fox", "Overlap should be deduplicated");
        assert!(!display.contains("brownbrown"), "Should not duplicate 'brown'");
    }

    /// AC2.1: When no new text arrives for expire_secs, the oldest (top) line is removed
    /// and remaining lines shift up.
    #[test]
    fn ac2_1_oldest_line_expires() {
        let mut buf = CaptionBuffer::new(2, 7, 1); // expire_secs = 1, max_chars = 7

        buf.push(" line1".to_string()); // Creates line 1: "line1" (5 chars)
        buf.push(" line2".to_string()); // "line1 line2" = 11 chars > 7, so creates line 2: "line2" (5 chars)

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines");

        // Manually expire the oldest line by setting its timestamp to the past.
        let now = Instant::now();
        if !buf.lines.is_empty() {
            buf.lines[0].last_active = now - std::time::Duration::from_secs(2);
        }

        let expired = buf.expire();
        assert!(expired, "expire() should return true when a line is removed");

        let display = buf.display_text();
        assert_eq!(display, "line2", "Oldest line should be removed");
        assert_eq!(buf.lines.len(), 1, "Should have 1 line after expiry");
    }

    /// AC2.2: Expiry continues once per second until all lines are cleared during silence.
    #[test]
    fn ac2_2_expiry_gradual_drain() {
        let mut buf = CaptionBuffer::new(3, 5, 1); // max_chars = 5 to force separate lines

        buf.push(" one".to_string());   // Line 1: "one" (3 chars)
        buf.push(" two".to_string());   // Won't fit on line 1 (3+1+3=7 > 5), goes to line 2: "two" (3 chars)
        buf.push(" three".to_string()); // Won't fit on line 2 (3+1+5=9 > 5), goes to line 3: "three" (5 chars)

        assert_eq!(buf.lines.len(), 3, "Should have 3 separate lines");

        // Set all lines to expired state.
        let now = Instant::now();
        let expired_time = now - std::time::Duration::from_secs(2);
        for line in &mut buf.lines {
            line.last_active = expired_time;
        }

        // First expire call should remove one line.
        assert!(buf.expire(), "First expire should remove a line");
        assert_eq!(buf.lines.len(), 2, "Should have 2 lines after first expire");

        // Second expire call should remove another line.
        assert!(buf.expire(), "Second expire should remove another line");
        assert_eq!(buf.lines.len(), 1, "Should have 1 line after second expire");

        // Third expire call should remove the last line.
        assert!(buf.expire(), "Third expire should remove the last line");
        assert_eq!(buf.lines.len(), 0, "Should have 0 lines after third expire");

        // Fourth expire call should return false (no lines to expire).
        assert!(!buf.expire(), "expire() should return false when buffer is empty");
    }

    /// AC2.3: Active lines (receiving new text) do not expire — last_active resets on each push.
    #[test]
    fn ac2_3_active_lines_dont_expire() {
        let now = Instant::now();
        let mut buf = CaptionBuffer::new(2, 20, 1);

        // Manually construct two lines: one expired and one active.
        buf.lines.push(CaptionLine {
            text: "old_content".to_string(),
            last_active: now - std::time::Duration::from_secs(2),
        });
        buf.lines.push(CaptionLine {
            text: "recent_content".to_string(),
            last_active: Instant::now(),
        });

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines");

        // Expire should only remove the first (expired) line.
        assert!(buf.expire(), "Should remove the expired first line");
        assert_eq!(buf.lines.len(), 1, "Should have 1 line after expiry");
        assert_eq!(buf.lines[0].text, "recent_content");

        // The remaining line should have recent last_active and not expire on next call.
        assert!(!buf.expire(), "Active line should not expire");
    }

    /// AC3.2: CaptionBuffer configuration can be updated via update_config for hot-reload.
    /// Verifies that expire_secs and max_chars_per_line can be changed after creation.
    #[test]
    fn ac3_2_update_config_hot_reload() {
        let mut buf = CaptionBuffer::new(3, 20, 8);

        // Add initial text with the original max_chars_per_line (20).
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());

        // Verify initial values
        assert_eq!(buf.max_chars_per_line, 20, "Initial max_chars_per_line should be 20");
        assert_eq!(buf.expire_secs, 8, "Initial expire_secs should be 8");

        // Update config to smaller max_chars_per_line, different max_lines, and expire_secs
        buf.update_config(2, 10, 5);

        // Verify updated values
        assert_eq!(buf.max_lines, 2, "max_lines should be updated to 2");
        assert_eq!(buf.max_chars_per_line, 10, "max_chars_per_line should be updated to 10");
        assert_eq!(buf.expire_secs, 5, "expire_secs should be updated to 5");

        // Verify that existing content is preserved
        let display = buf.display_text();
        assert_eq!(display, "Hello world", "Existing content should be preserved");

        // Add more text with the new max_chars_per_line to verify it's applied
        buf.push(" this".to_string());

        // With max_chars_per_line=10, "Hello world this" won't fit on one line
        // "Hello world" is 11 chars, so with max_chars=10, "world" would go to line 2
        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert!(lines.len() >= 2, "New text should respect updated max_chars_per_line");
    }
}
