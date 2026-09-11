//! The same terminal mascot for animated builds and append-only agent output.

pub(super) const WIDTH: usize = 18;
pub(super) const HEIGHT: usize = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ink {
    Box,
    Face,
    Tape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pose {
    Open,
    Folding,
    Closing,
    Taping(usize),
    Sealed,
}

impl Pose {
    /// Only Cargo's known progress moves the lid. Failure always leaves it open.
    pub(super) fn for_progress(done: usize, total: Option<usize>, ok: Option<bool>) -> Self {
        match ok {
            Some(true) => return Self::Sealed,
            Some(false) => return Self::Open,
            None => {}
        }
        let Some(total) = total.filter(|total| *total > 0) else {
            return Self::Open;
        };
        let percent = (done.min(total) as u128 * 100 / total as u128) as usize;
        match percent {
            0..60 => Self::Open,
            60..75 => Self::Folding,
            75..85 => Self::Closing,
            _ => Self::Taping(((percent - 85) / 2).min(7)),
        }
    }
}

pub(super) struct Mascot(pub [[(char, Ink); WIDTH]; HEIGHT]);

impl Mascot {
    pub(super) fn new(pose: Pose) -> Self {
        let mut art = Self([[(' ', Ink::Box); WIDTH]; HEIGHT]);
        let top = match pose {
            Pose::Open => [
                "      ┌─────────┐",
                "      │         │",
                "      ├─────────┤",
            ],
            Pose::Folding => ["", "     ┌─────────┐", "      ├─────────┤"],
            _ => ["", "", "      ┌─────────┐"],
        };
        for (y, line) in top.into_iter().enumerate() {
            art.put(0, y, line, Ink::Box);
        }
        art.put(
            0,
            3,
            if pose == Pose::Closing {
                "     ╱─────────╱│"
            } else {
                "     ╱         ╱│"
            },
            Ink::Box,
        );
        for (y, line) in [
            "    ╱─────────╱ │",
            "    │         │ │",
            "    │         │ ╱",
            "    │         │╱",
            "    └─────────┘",
        ]
        .into_iter()
        .enumerate()
        {
            art.put(0, y + 4, line, Ink::Box);
        }
        // Eye centers, mustache and bow tie share the front panel's center (x=9).
        art.put(6, 5, "•    (◉)", Ink::Face);
        art.put(7, 6, "◟▄▄▄◞", Ink::Face);
        art.put(8, 7, "◀◆▶", Ink::Face);
        art.put(15, 5, "≡", Ink::Tape);
        let tape = match pose {
            Pose::Taping(tape) => tape.min(7),
            Pose::Sealed => 7,
            _ => 0,
        };
        art.put(7, 3, &"═".repeat(tape), Ink::Tape);
        if matches!(pose, Pose::Taping(_)) {
            // A three-column, two-row gun; its cutter tracks the end of the tape.
            art.put(5 + tape, 1, "◉═╗", Ink::Face);
            art.put(5 + tape, 2, " ╲▼", Ink::Face);
        }
        art
    }

    fn put(&mut self, x: usize, y: usize, text: &str, ink: Ink) {
        for (cell, ch) in self.0[y][x..].iter_mut().zip(text.chars()) {
            *cell = (ch, ink);
        }
    }

    pub(super) fn plain() -> String {
        Self::new(Pose::Sealed).0[2..]
            .iter()
            .enumerate()
            .map(|(y, row)| {
                let line: String = row.iter().map(|(ch, _)| ch).collect();
                if y == 0 {
                    format!("{}  mr boxington", line.trim_end())
                } else {
                    line.trim_end().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_failed_builds_do_not_seal_and_overruns_are_bounded() {
        assert_eq!(Pose::for_progress(20, None, None), Pose::Open);
        assert_eq!(Pose::for_progress(0, Some(0), None), Pose::Open);
        assert_eq!(Pose::for_progress(100, Some(100), Some(false)), Pose::Open);
        assert_eq!(Pose::for_progress(0, None, Some(true)), Pose::Sealed);
        assert_eq!(
            Pose::for_progress(usize::MAX, Some(1), None),
            Pose::Taping(7)
        );
        for done in 0..=100 {
            let art = Mascot::new(Pose::for_progress(done, Some(100), None));
            assert_eq!(art.0[7][9], ('◆', Ink::Face));
            assert_eq!(art.0[5][6].0, '•');
            assert_eq!(art.0[5][12].0, '◉');
        }
        let plain = Mascot::plain();
        assert_eq!(plain.lines().count(), 7);
        assert!(!plain.contains(['\r', '\x1b']));
        assert!(!plain.contains('▼'));
    }
}
