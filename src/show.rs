#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub at_ms: u32,
    pub slots: [u8; 512],
}

#[derive(Debug, Clone)]
pub struct Show {
    pub frames: Vec<Frame>,
}

impl Show {
    pub fn end_ms(&self) -> u32 {
        self.frames.last().map(|frame| frame.at_ms).unwrap_or(0)
    }

    pub fn active_channels(&self) -> Vec<u16> {
        let mut mask = [false; 512];
        for frame in &self.frames {
            for (index, slot) in frame.slots.iter().enumerate() {
                if *slot != 0 {
                    mask[index] = true;
                }
            }
        }
        mask.iter()
            .enumerate()
            .filter_map(|(index, on)| on.then_some(index as u16 + 1))
            .collect()
    }

    pub fn timestamps_monotonic(&self) -> bool {
        self.frames
            .windows(2)
            .all(|pair| pair[0].at_ms <= pair[1].at_ms)
    }

    pub fn sort_by_time(&mut self) {
        self.frames.sort_by_key(|frame| frame.at_ms);
    }
}
