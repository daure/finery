use std::time::Duration;

use tuicore::SpeedReader;

pub(crate) const MIN_SPEED_READER_WPM: u16 = 100;
pub(crate) const MAX_SPEED_READER_WPM: u16 = 1000;
pub(crate) const MAX_MARKDOWN_BLOCK_PAUSE_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpeedReaderSettings {
    pub(crate) wpm: u16,
    pub(crate) markdown_block_pause: Duration,
}

impl Default for SpeedReaderSettings {
    fn default() -> Self {
        Self {
            wpm: 600,
            markdown_block_pause: Duration::from_millis(250),
        }
    }
}

impl SpeedReaderSettings {
    pub(crate) fn apply(self, reader: SpeedReader) -> SpeedReader {
        reader
            .wpm(self.wpm)
            .markdown_block_pause(self.markdown_block_pause)
    }
}

pub(crate) fn parse_speed_reader_wpm(value: &str) -> Option<u16> {
    let wpm = value.parse().ok()?;
    (MIN_SPEED_READER_WPM..=MAX_SPEED_READER_WPM)
        .contains(&wpm)
        .then_some(wpm)
}

pub(crate) fn parse_markdown_block_pause(value: &str) -> Option<Duration> {
    let milliseconds = value.parse::<u64>().ok()?;
    (milliseconds <= MAX_MARKDOWN_BLOCK_PAUSE_MS).then_some(Duration::from_millis(milliseconds))
}
