use super::DecodedStateFrame;
use quadcortex::domain::{
    STATE_EVENT_HISTORY_MAXIMUM_BYTES, STATE_EVENT_MAXIMUM_LIMIT, STATE_EVENT_PAGE_MAXIMUM_BYTES,
};
use std::collections::VecDeque;
use std::io::{self, Write};

// Fixed UUID producer, cursor and JSON envelope fit comfortably in this reserve.
const PAGE_ENVELOPE_BYTES: usize = 1024;
const FRAME_BUDGET: usize = STATE_EVENT_PAGE_MAXIMUM_BYTES - PAGE_ENVELOPE_BYTES;

#[derive(Default)]
pub(super) struct StateEventJournal {
    frames: VecDeque<(DecodedStateFrame, usize)>,
    bytes: usize,
}

struct ByteCounter(usize);
impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > FRAME_BUDGET.saturating_sub(self.0) {
            return Err(io::Error::other("state frame exceeds page budget"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl StateEventJournal {
    /// Oversized frames are not retained or delivered to live subscribers. The
    /// caller still advances its tail, so cache recovery can acknowledge loss.
    #[cfg(test)]
    pub(super) fn push(&mut self, frame: DecodedStateFrame) -> bool {
        self.push_sized(frame).is_some()
    }

    /// [`Self::push`], returning the frame's encoded size when it was
    /// retained, so live subscriber queues can budget it in the same bytes.
    pub(super) fn push_sized(&mut self, frame: DecodedStateFrame) -> Option<usize> {
        let mut counter = ByteCounter(1); // Array comma, including the first item.
        if serde_json::to_writer(&mut counter, &frame).is_err() {
            return None;
        }
        let encoded = counter.0;
        self.bytes += counter.0;
        self.frames.push_back((frame, counter.0));
        while self.frames.len() > STATE_EVENT_MAXIMUM_LIMIT
            || self.bytes > STATE_EVENT_HISTORY_MAXIMUM_BYTES
        {
            self.bytes -= self
                .frames
                .pop_front()
                .expect("nonempty oversized journal")
                .1;
        }
        Some(encoded)
    }

    pub(super) fn page(&self, after: u64, limit: usize) -> Vec<DecodedStateFrame> {
        let mut bytes = 0;
        self.frames
            .iter()
            .filter(|(frame, _)| frame.sequence > after)
            .take(limit.min(STATE_EVENT_MAXIMUM_LIMIT))
            .take_while(|(_, size)| {
                bytes += size;
                bytes <= FRAME_BUDGET
            })
            .map(|(frame, _)| frame.clone())
            .collect()
    }

    pub(super) fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.frames.len()
    }
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
    #[cfg(test)]
    pub(super) fn front(&self) -> Option<&DecodedStateFrame> {
        self.frames.front().map(|(frame, _)| frame)
    }
    #[cfg(test)]
    pub(super) fn back(&self) -> Option<&DecodedStateFrame> {
        self.frames.back().map(|(frame, _)| frame)
    }
}

#[cfg(test)]
impl Extend<DecodedStateFrame> for StateEventJournal {
    fn extend<T: IntoIterator<Item = DecodedStateFrame>>(&mut self, iter: T) {
        for frame in iter {
            self.push(frame);
        }
    }
}

#[cfg(test)]
impl std::ops::Index<usize> for StateEventJournal {
    type Output = DecodedStateFrame;
    fn index(&self, index: usize) -> &Self::Output {
        &self.frames[index].0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(sequence: u64, text: String) -> DecodedStateFrame {
        DecodedStateFrame {
            stream_id: text,
            sequence,
            observed_at: 0,
            states: vec![],
            tempo_clock: None,
        }
    }

    #[test]
    fn byte_pages_progress_without_skipping_a_retained_frame() {
        let mut journal = StateEventJournal::default();
        for sequence in 1..=3 {
            assert!(journal.push(frame(sequence, "x".repeat(2_000_000))));
        }
        let first = journal.page(0, 4096);
        assert_eq!(
            first.iter().map(|frame| frame.sequence).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(serde_json::to_vec(&first).unwrap().len() < STATE_EVENT_PAGE_MAXIMUM_BYTES);
        assert_eq!(journal.page(2, 4096)[0].sequence, 3);
        assert_eq!(journal.page(0, 1).len(), 1);
    }

    #[test]
    fn history_counts_serialized_utf8_and_escaping_and_clear_reclaims_budget() {
        let mut journal = StateEventJournal::default();
        for sequence in 1..=4 {
            assert!(journal.push(frame(sequence, "\"界".repeat(600_000))));
        }
        assert_eq!(journal.len(), 2);
        assert_eq!(journal.front().unwrap().sequence, 3);
        assert!(journal.bytes <= STATE_EVENT_HISTORY_MAXIMUM_BYTES);
        journal.clear();
        assert_eq!(journal.bytes, 0);
        assert!(journal.push(frame(5, "stream".into())));
    }

    #[test]
    fn oversized_frame_does_not_evict_or_block_retained_history() {
        let mut journal = StateEventJournal::default();
        assert!(journal.push(frame(1, "stream".into())));
        assert!(!journal.push(frame(2, "x".repeat(STATE_EVENT_PAGE_MAXIMUM_BYTES))));
        assert!(journal.push(frame(3, "stream".into())));
        assert_eq!(
            journal
                .page(0, 4096)
                .iter()
                .map(|frame| frame.sequence)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
    }
}
