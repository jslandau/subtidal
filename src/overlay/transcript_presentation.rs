//! Selectable transcript text and native-independent metadata/position mapping.
use super::transcript_log::{AppendKind, Fragment, TranscriptLog};
use std::{collections::HashMap, ops::Range};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataKind {
    Timestamp,
    Speaker,
}
#[derive(Clone, Debug)]
pub struct MetadataSpan {
    pub range: Range<usize>,
    pub kind: MetadataKind,
}
#[derive(Clone, Debug, Default)]
pub struct Presentation {
    pub text: String,
    /// Byte ranges; convert at the native boundary rather than mixing index units.
    pub metadata: Vec<MetadataSpan>,
    pub fragments: Vec<Range<usize>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FragmentPosition {
    pub fragment: usize,
    pub body_byte: usize,
    /// Metadata offsets remain selectable across rebuilds; absent metadata clamps to body.
    pub metadata: Option<MetadataKind>,
}

pub fn gtk_offset(text: &str, byte: usize) -> usize {
    text[..boundary(text, byte)].chars().count()
}
pub fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..boundary(text, byte)].encode_utf16().count()
}
/// Convert a native character index to a UTF-8 boundary, clamping past the end.
pub fn byte_from_gtk(text: &str, characters: usize) -> usize {
    text.char_indices()
        .nth(characters)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}
/// A native index inside a surrogate pair resolves to the start of that scalar.
pub fn byte_from_utf16(text: &str, units: usize) -> usize {
    let mut consumed = 0;
    for (byte, ch) in text.char_indices() {
        if consumed + ch.len_utf16() > units {
            return byte;
        }
        consumed += ch.len_utf16();
    }
    text.len()
}
fn boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}
impl Presentation {
    pub fn append(&mut self, fragment: &Fragment, kind: AppendKind, names: &HashMap<u32, String>) {
        if kind == AppendKind::NewParagraph {
            if !self.text.is_empty() {
                self.text.push('\n');
            }
            let start = self.text.len();
            self.text
                .push_str(&fragment.timestamp.format("[%H:%M:%S] ").to_string());
            self.metadata.push(MetadataSpan {
                range: start..self.text.len(),
                kind: MetadataKind::Timestamp,
            });
            if let Some(id) = fragment.speaker_id {
                let start = self.text.len();
                self.text.push_str(
                    &names
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| format!("Speaker {}", u64::from(id) + 1)),
                );
                self.text.push_str(": ");
                self.metadata.push(MetadataSpan {
                    range: start..self.text.len(),
                    kind: MetadataKind::Speaker,
                });
            }
        }
        let start = self.text.len();
        self.text.push_str(&fragment.text);
        self.fragments.push(start..self.text.len());
    }
    pub fn from_log(log: &TranscriptLog, names: &HashMap<u32, String>) -> Self {
        let mut result = Self::default();
        for (i, fragment) in log.fragments().iter().enumerate() {
            result.append(fragment, log.append_kind_at(i), names);
        }
        result
    }
    pub fn position(&self, byte: usize) -> Option<FragmentPosition> {
        if let Some(span) = self.metadata.iter().find(|span| span.range.contains(&byte)) {
            let fragment = self
                .fragments
                .iter()
                .position(|range| range.start >= span.range.end)?;
            return Some(FragmentPosition {
                fragment,
                body_byte: boundary(&self.text[span.range.clone()], byte - span.range.start),
                metadata: Some(span.kind),
            });
        }
        let index = self
            .fragments
            .iter()
            .rposition(|r| r.start <= byte)
            .unwrap_or(0);
        let range = self.fragments.get(index)?;
        Some(FragmentPosition {
            fragment: index,
            body_byte: boundary(&self.text[range.clone()], byte.saturating_sub(range.start)),
            metadata: None,
        })
    }
    pub fn resolve(&self, position: FragmentPosition) -> usize {
        if let (Some(kind), Some(body)) = (position.metadata, self.fragments.get(position.fragment))
        {
            let lower = position
                .fragment
                .checked_sub(1)
                .and_then(|index| self.fragments.get(index))
                .map(|range| range.end)
                .unwrap_or(0);
            if let Some(span) = self.metadata.iter().find(|span| {
                span.kind == kind && span.range.start >= lower && span.range.end <= body.start
            }) {
                return span.range.start
                    + boundary(&self.text[span.range.clone()], position.body_byte);
            }
            return body.start;
        }
        self.fragments
            .get(position.fragment)
            .map(|r| r.start + boundary(&self.text[r.clone()], position.body_byte))
            .unwrap_or(self.text.len())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;
    use std::time::Duration;
    #[test]
    fn transcript_presentation_unicode() {
        let text = "é🙂 中";
        assert_eq!(gtk_offset(text, text.len()), 4);
        assert_eq!(utf16_offset(text, text.len()), 5);
        assert_eq!(utf16_offset(text, 6), 3);
        assert_eq!(gtk_offset(text, 3), 1);
        assert_eq!(byte_from_utf16(text, 2), 2);
        assert_eq!(byte_from_utf16(text, 3), 6);
        assert_eq!(byte_from_gtk(text, 2), 6);
        for (byte, _) in text.char_indices() {
            assert_eq!(byte_from_gtk(text, gtk_offset(text, byte)), byte);
            assert_eq!(byte_from_utf16(text, utf16_offset(text, byte)), byte);
        }
    }
    #[test]
    fn transcript_presentation_speaker_changes() {
        let mut log = TranscriptLog::new(Duration::from_secs(7));
        let ts = Local::now();
        log.push_at_with_speaker("  hé🙂".into(), Some(0), 0, ts);
        log.push_at_with_speaker(" continuation".into(), Some(0), 1, ts);
        log.push_at_with_speaker(" other".into(), Some(1), 2, ts);
        let before = Presentation::from_log(&log, &HashMap::new());
        assert!(before.text.contains("Speaker 1:   hé🙂 continuation\n"));
        let pos = before.position(before.fragments[0].start + 2).unwrap();
        let after = Presentation::from_log(&log, &HashMap::from([(0, "長い名前".into())]));
        assert_eq!(
            &after.text[after.resolve(pos)..after.fragments[0].end],
            "hé🙂"
        );
        assert_eq!(before.metadata.len(), 4);
        let timestamp = before.metadata[0].range.start + 2;
        assert_eq!(
            after.resolve(before.position(timestamp).unwrap()),
            timestamp
        );
        let speaker = before.metadata[1].range.start;
        let mapped = after.resolve(before.position(speaker).unwrap());
        assert!(after.text[mapped..].starts_with("長い名前"));
    }
    #[test]
    fn positions_survive_relabel_paragraph_changes() {
        let mut log = TranscriptLog::new(Duration::from_secs(7));
        let ts = Local::now();
        log.push_at_with_speaker("first".into(), Some(0), 0, ts);
        log.push_at_with_speaker(" second🙂".into(), Some(0), 1, ts);
        let before = Presentation::from_log(&log, &HashMap::new());
        let byte = before.fragments[1].start + " second".len();
        let position = before.position(byte).unwrap();
        log.relabel_since(1, 1);
        let after = Presentation::from_log(&log, &HashMap::new());
        assert!(after.text.contains("\n"));
        assert_eq!(&after.text[after.resolve(position)..], "🙂");
        log.relabel_since(0, 1);
        let merged = Presentation::from_log(&log, &HashMap::new());
        assert!(!merged.text.contains("\n"));
        assert_eq!(&merged.text[merged.resolve(position)..], "🙂");
    }
    #[test]
    fn transcript_presentation_uses_configured_gap_and_negative_time() {
        let mut log = TranscriptLog::new(Duration::from_secs(7));
        let ts = Local::now();
        log.push_at("one ".into(), ts);
        log.push_at("two".into(), ts + chrono::Duration::seconds(3));
        log.push_at(" backwards".into(), ts);
        log.push_at(" end".into(), ts + chrono::Duration::seconds(8));
        let view = Presentation::from_log(&log, &HashMap::new());
        assert_eq!(view.metadata.len(), 2);
        assert!(view.text.contains("one two backwards\n"));
        assert_eq!(view.fragments.len(), 4);
    }
    #[test]
    fn transcript_export_unchanged() {
        let mut log = TranscriptLog::new(Duration::from_secs(2));
        let ts = Local::now();
        log.push_at_with_speaker(" é🙂 ".into(), Some(2), 123, ts);
        let before = log.to_json("nemotron", ts);
        let _ = Presentation::from_log(&log, &HashMap::new());
        assert_eq!(before, log.to_json("nemotron", ts));
        assert_eq!(before["fragments"][0]["text"], " é🙂 ");
        assert_eq!(before["fragments"][0]["speaker_id"], 2);
        assert!(before["fragments"][0].get("emit_sample").is_none());
        log.push_at_with_speaker(" next ".into(), Some(3), 124, ts);
        // TXT export intentionally groups by time gap, not visible speaker labels.
        assert_eq!(log.paragraphs().len(), 1);
        assert_eq!(log.paragraphs()[0].text, " é🙂  next ");
        let json = log.to_json("nemotron", ts);
        let _ = Presentation::from_log(&log, &HashMap::from([(3, "Custom".into())]));
        assert_eq!(json, log.to_json("nemotron", ts));
    }
}
