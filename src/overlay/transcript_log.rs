//! Transcript log: pure-data accumulator for durable speech-to-text records.
//!
//! `TranscriptLog` collects speech fragments with timestamps, automatically grouping them
//! into paragraphs based on a configurable silence threshold. Unlike `CaptionBuffer`
//! (which is a display-oriented buffer that filters for readability), `TranscriptLog`
//! preserves every fragment exactly as the engine emitted it—including whitespace-only
//! tokens that signal word boundaries in RNNT output. This makes it a faithful audit log
//! suitable for `.json` export and downstream processing.

use chrono::{DateTime, Local};
use serde::Serialize;
use std::time::Duration as StdDuration;

/// A single speech fragment with its timestamp and optional speaker ID.
#[derive(Debug, Clone, Serialize)]
pub struct Fragment {
    pub timestamp: DateTime<Local>,
    pub text: String,
    /// Speaker ID (0-based) when diarization is active, None otherwise.
    /// Included in JSON export as "speaker_id" (null when diarization was off).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<u32>,
    /// Sample offset (in the diarization engine's frame of reference) at which
    /// this fragment was emitted. Used to retroactively re-attribute fragments
    /// when Sortformer reveals a late speaker switch. `0` when diarization was
    /// off at emit time. Not serialised — implementation detail.
    #[serde(skip)]
    pub emit_sample: u64,
}

/// Result of a `push` indicating whether the fragment started a new paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendKind {
    NewParagraph,
    ContinueParagraph,
}

/// A paragraph: a contiguous run of fragments within the silence threshold,
/// timestamped at the first fragment of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    pub timestamp: DateTime<Local>,
    pub text: String,
}

/// Accumulator for speech fragments, grouping them into paragraphs on silence gaps.
pub struct TranscriptLog {
    fragments: Vec<Fragment>,
    paragraph_gap: StdDuration,
}

impl TranscriptLog {
    /// Create a new transcript log with a specified paragraph gap (silence threshold).
    ///
    /// Fragments arriving more than `paragraph_gap` apart belong to separate paragraphs.
    pub fn new(paragraph_gap: StdDuration) -> Self {
        TranscriptLog {
            fragments: Vec::new(),
            paragraph_gap,
        }
    }

    /// Push a fragment with the current time and no speaker.
    ///
    /// Thin wrapper around `push_with_speaker` that passes `None` for speaker_id.
    pub fn push(&mut self, text: String) -> AppendKind {
        self.push_with_speaker(text, None)
    }

    /// Push a fragment with speaker information at the current time.
    ///
    /// Thin wrapper around `push_at_with_speaker`.
    pub fn push_with_speaker(&mut self, text: String, speaker_id: Option<u32>) -> AppendKind {
        self.push_at_with_speaker(text, speaker_id, 0, Local::now())
    }

    /// Push a fragment with speaker information and emit-time sample offset.
    pub fn push_with_speaker_and_sample(
        &mut self,
        text: String,
        speaker_id: Option<u32>,
        emit_sample: u64,
    ) -> AppendKind {
        self.push_at_with_speaker(text, speaker_id, emit_sample, Local::now())
    }

    /// Push a fragment with an explicit timestamp, optional speaker ID, and emit sample.
    ///
    /// Determines whether this fragment starts a new paragraph or continues the previous one.
    /// A new paragraph is forced when:
    /// - This is the first fragment (empty vec)
    /// - The time gap since the last fragment exceeds `paragraph_gap`
    /// - The speaker changed from the previous fragment (diarization active)
    ///
    /// **Negative gaps** (clock backwards): Treated as `ContinueParagraph`.
    pub fn push_at_with_speaker(
        &mut self,
        text: String,
        speaker_id: Option<u32>,
        emit_sample: u64,
        ts: DateTime<Local>,
    ) -> AppendKind {
        let kind = if self.fragments.is_empty() {
            AppendKind::NewParagraph
        } else {
            let last = &self.fragments[self.fragments.len() - 1];
            // Speaker change always forces a new paragraph when diarization is active.
            if speaker_id.is_some() && last.speaker_id.is_some() && speaker_id != last.speaker_id {
                AppendKind::NewParagraph
            } else {
                let gap = ts
                    .signed_duration_since(last.timestamp)
                    .to_std()
                    .unwrap_or(StdDuration::ZERO);
                if gap > self.paragraph_gap {
                    AppendKind::NewParagraph
                } else {
                    AppendKind::ContinueParagraph
                }
            }
        };

        self.fragments.push(Fragment {
            timestamp: ts,
            text,
            speaker_id,
            emit_sample,
        });
        kind
    }

    /// Push a fragment with an explicit timestamp and no speaker information.
    ///
    /// Thin wrapper around `push_at_with_speaker` passing `None`.
    pub fn push_at(&mut self, text: String, ts: DateTime<Local>) -> AppendKind {
        self.push_at_with_speaker(text, None, 0, ts)
    }

    /// Retroactively re-attribute all fragments whose `emit_sample >= from_sample`
    /// to `new_speaker_id`. Returns the number of fragments rewritten.
    ///
    /// Called when Sortformer reveals (a few captions late) that a speaker switch
    /// actually began at `from_sample`. After this call, paragraph boundaries
    /// derived from `paragraphs()` automatically reflect the corrected speakers.
    pub fn relabel_since(&mut self, from_sample: u64, new_speaker_id: u32) -> usize {
        let mut n = 0;
        for frag in self.fragments.iter_mut().rev() {
            if frag.emit_sample < from_sample {
                break;
            }
            if frag.speaker_id != Some(new_speaker_id) {
                frag.speaker_id = Some(new_speaker_id);
                n += 1;
            }
        }
        n
    }

    /// Return a slice of all fragments in order.
    pub fn fragments(&self) -> &[Fragment] {
        &self.fragments
    }

    /// Derive paragraphs from the stored fragments.
    ///
    /// Walks fragments in order, splitting on the same gap rule as `push_at`.
    /// Each paragraph is timestamped at its first fragment and contains the verbatim
    /// concatenation of all fragment text in the run (no separators, preserving whitespace).
    pub fn paragraphs(&self) -> Vec<Paragraph> {
        if self.fragments.is_empty() {
            return Vec::new();
        }

        let mut result = Vec::new();
        let mut current_para_start_idx = 0;

        for i in 1..self.fragments.len() {
            let prev = &self.fragments[i - 1];
            let curr = &self.fragments[i];

            let gap = curr
                .timestamp
                .signed_duration_since(prev.timestamp)
                .to_std()
                .unwrap_or(StdDuration::ZERO);

            if gap > self.paragraph_gap {
                // End current paragraph and start a new one.
                let para_text = self.fragments[current_para_start_idx..i]
                    .iter()
                    .map(|f| f.text.as_str())
                    .collect::<String>();

                result.push(Paragraph {
                    timestamp: self.fragments[current_para_start_idx].timestamp,
                    text: para_text,
                });

                current_para_start_idx = i;
            }
        }

        // Add the final paragraph.
        let para_text = self.fragments[current_para_start_idx..]
            .iter()
            .map(|f| f.text.as_str())
            .collect::<String>();

        result.push(Paragraph {
            timestamp: self.fragments[current_para_start_idx].timestamp,
            text: para_text,
        });

        result
    }

    /// Serialize to JSON with session metadata.
    ///
    /// Returns a JSON object with:
    /// - `"session_start"`: RFC-3339 timestamp of session creation
    /// - `"engine"`: name of the STT engine (e.g., "nemotron")
    /// - `"fragments"`: array of `{timestamp, text}` objects for each fragment
    pub fn to_json(&self, engine_name: &str, session_start: DateTime<Local>) -> serde_json::Value {
        serde_json::json!({
            "session_start": session_start,
            "engine": engine_name,
            "fragments": serde_json::to_value(&self.fragments).unwrap(),
        })
    }

    /// Clear all fragments.
    ///
    /// After `clear()`, `fragments()` returns an empty slice and the next `push()`
    /// returns `NewParagraph`.
    pub fn clear(&mut self) {
        self.fragments.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64, nanos: u32) -> DateTime<Local> {
        Local.timestamp_opt(secs, nanos).unwrap()
    }

    /// AC1.2: Verify that fragments within the gap continue a paragraph,
    /// and fragments beyond the gap start a new paragraph.
    #[test]
    fn ac1_2_paragraph_break_after_gap() {
        let gap = StdDuration::from_millis(1500);
        let mut log = TranscriptLog::new(gap);

        // Fragment 1 at t=0
        let kind1 = log.push_at("Hello ".to_string(), ts(0, 0));
        assert_eq!(kind1, AppendKind::NewParagraph);

        // Fragment 2 at t=1.0s (gap = 1.0s ≤ 1.5s, continue)
        let kind2 = log.push_at("world".to_string(), ts(1, 0));
        assert_eq!(kind2, AppendKind::ContinueParagraph);

        // Fragment 3 at t=3.0s (gap = 2.0s > 1.5s, new paragraph)
        let kind3 = log.push_at("Hi".to_string(), ts(3, 0));
        assert_eq!(kind3, AppendKind::NewParagraph);
    }

    /// AC1.2b: Boundary test—exactly at the gap threshold.
    #[test]
    fn ac1_2b_paragraph_continue_under_gap() {
        let gap = StdDuration::from_millis(1500);
        let mut log = TranscriptLog::new(gap);

        log.push_at("Hello".to_string(), ts(0, 0));
        let kind = log.push_at("world".to_string(), ts(1, 500_000_000)); // 1.5s exactly

        // 1.5s = exactly at threshold; should be ≤, so ContinueParagraph
        assert_eq!(kind, AppendKind::ContinueParagraph);
    }

    /// AC1.3: Verify that whitespace is preserved in paragraphs.
    #[test]
    fn ac1_3_whitespace_preserved_in_paragraphs() {
        let gap = StdDuration::from_secs(2);
        let mut log = TranscriptLog::new(gap);

        // All within the gap, so one paragraph
        log.push_at("Hello".to_string(), ts(0, 0));
        log.push_at(" world".to_string(), ts(1, 0)); // leading space preserved
        log.push_at(",".to_string(), ts(2, 0));

        let paras = log.paragraphs();
        assert_eq!(paras.len(), 1);
        assert_eq!(
            paras[0].text, "Hello world,",
            "Whitespace should be preserved"
        );
    }

    /// AC1.4: Verify the shape of JSON output.
    #[test]
    fn ac1_4_to_json_shape() {
        let gap = StdDuration::from_secs(2);
        let mut log = TranscriptLog::new(gap);

        let t1 = ts(1000, 0);
        let t2 = ts(1001, 0);
        let session = ts(999, 0);

        log.push_at("Hello ".to_string(), t1);
        log.push_at("world".to_string(), t2);

        let json = log.to_json("nemotron", session);

        assert!(json.is_object());
        assert!(json["session_start"].is_string());
        assert_eq!(json["engine"], "nemotron");
        assert!(json["fragments"].is_array());
        assert_eq!(json["fragments"].as_array().unwrap().len(), 2);

        let frag0 = &json["fragments"][0];
        assert!(frag0["timestamp"].is_string());
        assert_eq!(frag0["text"], "Hello ");

        let frag1 = &json["fragments"][1];
        assert!(frag1["timestamp"].is_string());
        assert_eq!(frag1["text"], "world");
    }

    /// AC1.4b: Verify that JSON timestamps are RFC-3339 formatted.
    #[test]
    fn ac1_4b_to_json_timestamp_format_rfc3339() {
        let gap = StdDuration::from_secs(2);
        let mut log = TranscriptLog::new(gap);

        let t1 = ts(1000, 0);
        let session = ts(999, 0);

        log.push_at("test".to_string(), t1);

        let json = log.to_json("nemotron", session);
        let timestamp_str = json["fragments"][0]["timestamp"]
            .as_str()
            .expect("timestamp should be a string");

        // Parse with RFC-3339 to verify format
        let _parsed = chrono::DateTime::parse_from_rfc3339(timestamp_str)
            .expect("timestamp should parse as RFC-3339");
    }

    /// AC1.5: Verify that paragraphs() matches the expected structure.
    #[test]
    fn ac1_5_paragraphs_derivation_matches_txt() {
        let gap = StdDuration::from_millis(1500);
        let mut log = TranscriptLog::new(gap);

        // Paragraph 1: fragments at t=0 and t=1s (within 1.5s)
        log.push_at("Hello ".to_string(), ts(0, 0));
        log.push_at("world".to_string(), ts(1, 0));

        // Paragraph 2: fragment at t=3s (2s after last, > 1.5s gap)
        log.push_at("Goodbye".to_string(), ts(3, 0));

        let paras = log.paragraphs();
        assert_eq!(paras.len(), 2);

        assert_eq!(paras[0].timestamp, ts(0, 0));
        assert_eq!(paras[0].text, "Hello world");

        assert_eq!(paras[1].timestamp, ts(3, 0));
        assert_eq!(paras[1].text, "Goodbye");
    }

    /// AC1.6: Verify clear() empties the log.
    #[test]
    fn ac1_6_clear_empties_fragments() {
        let gap = StdDuration::from_secs(2);
        let mut log = TranscriptLog::new(gap);

        log.push_at("Hello".to_string(), ts(0, 0));
        log.push_at(" world".to_string(), ts(1, 0));
        log.push_at("!".to_string(), ts(2, 0));

        assert_eq!(log.fragments().len(), 3);

        log.clear();
        assert!(log.fragments().is_empty());

        let kind = log.push_at("New".to_string(), ts(10, 0));
        assert_eq!(kind, AppendKind::NewParagraph);
        assert_eq!(log.fragments().len(), 1);
    }

    /// AC1.extra: Verify that a fresh log returns empty paragraphs.
    #[test]
    fn ac1_extra_paragraphs_empty_when_no_fragments() {
        let log = TranscriptLog::new(StdDuration::from_secs(2));
        let paras = log.paragraphs();
        assert!(paras.is_empty());
    }

    /// transcript-window-mode.AC4.6: TranscriptLog accumulates across simulated
    /// mode-switch boundaries (purely a data-level test — the GTK side is exercised
    /// manually).
    #[test]
    fn ac4_6_transcript_log_accumulates_across_simulated_mode_switches() {
        let mut log = TranscriptLog::new(std::time::Duration::from_millis(1500));
        let t0 = ts(1_700_000_000, 0);
        log.push_at("hello".to_string(), t0);
        log.push_at(
            " world".to_string(),
            t0 + chrono::Duration::milliseconds(200),
        );
        // simulate user toggling mode (no effect on the log itself)
        log.push_at(
            "again".to_string(),
            t0 + chrono::Duration::milliseconds(2000),
        );
        assert_eq!(log.fragments().len(), 3);
        assert_eq!(
            log.paragraphs().len(),
            2,
            "second paragraph starts after >1.5s gap"
        );
    }

    /// relabel_since rewrites the speaker_id of trailing fragments whose
    /// emit_sample is at or after from_sample. Older fragments are
    /// untouched. Returns the number rewritten.
    #[test]
    fn relabel_since_rewrites_trailing_fragments() {
        let mut log = TranscriptLog::new(std::time::Duration::from_secs(2));
        let t0 = ts(1_700_000_000, 0);
        log.push_at_with_speaker("first".to_string(), Some(0), 16_000, t0);
        log.push_at_with_speaker(
            " second".to_string(),
            Some(0),
            32_000,
            t0 + chrono::Duration::milliseconds(100),
        );
        // Third fragment is misattributed to Speaker 1 but should be Speaker 2.
        log.push_at_with_speaker(
            " third".to_string(),
            Some(0),
            48_000,
            t0 + chrono::Duration::milliseconds(200),
        );

        // Sortformer reveals speaker switched at sample 40000.
        let n = log.relabel_since(40_000, 1);
        assert_eq!(n, 1, "only the third fragment should be relabeled");

        let frags = log.fragments();
        assert_eq!(frags[0].speaker_id, Some(0));
        assert_eq!(frags[1].speaker_id, Some(0));
        assert_eq!(frags[2].speaker_id, Some(1));
    }

    /// relabel_since is idempotent — calling it twice with the same args
    /// reports 0 rewrites on the second call.
    #[test]
    fn relabel_since_is_idempotent() {
        let mut log = TranscriptLog::new(std::time::Duration::from_secs(2));
        let t0 = ts(1_700_000_000, 0);
        log.push_at_with_speaker("only".to_string(), Some(0), 16_000, t0);
        assert_eq!(log.relabel_since(16_000, 1), 1);
        assert_eq!(log.relabel_since(16_000, 1), 0, "second call is a no-op");
    }
}
