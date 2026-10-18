use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub numerator: u32,
    pub denominator: u32,
}

impl Default for Rate {
    fn default() -> Self {
        Self {
            numerator: 24,
            denominator: 1,
        }
    }
}

impl Rate {
    pub const PRESETS: [&str; 16] = [
        "1", "3", "6", "12", "15", "16", "18", "23.976", "24", "25", "29.97", "30", "50", "59.94",
        "60", "120",
    ];

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (numerator, denominator) = if let Some((a, b)) = text.split_once('/') {
            (a.parse().ok()?, b.parse().ok()?)
        } else {
            match text {
                "23.976" | "23.98" => (24000, 1001),
                "29.97" => (30000, 1001),
                "59.94" => (60000, 1001),
                _ => {
                    if let Some((a, b)) = text.split_once('.') {
                        if b.len() > 6 || !b.bytes().all(|c| c.is_ascii_digit()) {
                            return None;
                        }
                        let denominator = 10u32.checked_pow(b.len() as u32)?;
                        let numerator = a
                            .parse::<u32>()
                            .ok()?
                            .checked_mul(denominator)?
                            .checked_add(b.parse::<u32>().ok()?)?;
                        (numerator, denominator)
                    } else {
                        (text.parse().ok()?, 1)
                    }
                }
            }
        };
        if numerator == 0 || denominator == 0 || numerator as f64 / denominator as f64 > 1000.0 {
            return None;
        }
        let mut a = numerator;
        let mut b = denominator;
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Some(Self {
            numerator: numerator / a,
            denominator: denominator / a,
        })
    }

    pub fn value(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }

    pub fn period(self) -> Duration {
        Duration::from_secs_f64(1.0 / self.value())
    }

    pub fn label(self) -> String {
        match (self.numerator, self.denominator) {
            (24000, 1001) => "23.976".into(),
            (30000, 1001) => "29.97".into(),
            (60000, 1001) => "59.94".into(),
            (_, 1) => self.numerator.to_string(),
            _ => format!("{}/{}", self.numerator, self.denominator),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Direction {
    Reverse,
    #[default]
    Stop,
    Forward,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    Once,
    #[default]
    Repeat,
    PingPong,
}

impl LoopMode {
    pub fn next(self) -> Self {
        match self {
            Self::Once => Self::Repeat,
            Self::Repeat => Self::PingPong,
            Self::PingPong => Self::Once,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Once => "Once",
            Self::Repeat => "Repeat",
            Self::PingPong => "Ping-pong",
        }
    }
}

pub struct Playback {
    pub frame: usize,
    pub count: usize,
    pub rate: Rate,
    pub direction: Direction,
    pub loop_mode: LoopMode,
    pub in_point: usize,
    pub out_point: usize,
    pub in_out: bool,
    previous: Direction,
    clock: Instant,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            frame: 0,
            count: 0,
            rate: Rate::default(),
            direction: Direction::Stop,
            loop_mode: LoopMode::Repeat,
            in_point: 0,
            out_point: 0,
            in_out: true,
            previous: Direction::Forward,
            clock: Instant::now(),
        }
    }
}

impl Playback {
    pub fn load(&mut self, count: usize, rate: Rate) {
        self.frame = 0;
        self.count = count;
        self.rate = rate;
        self.in_point = 0;
        self.out_point = count.saturating_sub(1);
        self.direction = Direction::Stop;
        self.set_direction(Direction::Forward);
    }

    pub fn range(&self, in_out: bool) -> (usize, usize) {
        if in_out && self.in_out {
            (
                self.in_point.min(self.out_point),
                self.out_point.max(self.in_point),
            )
        } else {
            (0, self.count.saturating_sub(1))
        }
    }

    pub fn set_direction(&mut self, direction: Direction) {
        if direction == self.direction {
            return;
        }
        self.previous = self.direction;
        self.direction = direction;
        self.restart_once();
        self.reset_clock();
    }

    pub fn set_loop(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
        self.restart_once();
        self.reset_clock();
    }

    fn restart_once(&mut self) {
        let direction = self.direction;
        let (start, end) = self.range(true);
        if self.loop_mode == LoopMode::Once {
            if direction == Direction::Forward && self.frame == end {
                self.frame = start;
            } else if direction == Direction::Reverse && self.frame == start {
                self.frame = end;
            }
        }
    }

    pub fn toggle(&mut self) {
        let next = if self.direction == Direction::Stop {
            self.previous
        } else {
            Direction::Stop
        };
        self.set_direction(next);
    }

    pub fn reset_clock(&mut self) {
        self.clock = Instant::now();
    }

    pub fn consume_tick(&mut self) {
        let now = Instant::now();
        let remainder = self.clock.elapsed().as_nanos() % self.rate.period().as_nanos();
        self.clock = now - Duration::from_nanos(remainder as u64);
    }

    pub fn wait(&self) -> Duration {
        let remaining = self.rate.period().saturating_sub(self.clock.elapsed());
        if remaining.is_zero() {
            Duration::from_millis(8)
        } else {
            remaining.max(Duration::from_millis(1))
        }
    }

    pub fn due(&self) -> bool {
        self.count > 1
            && self.direction != Direction::Stop
            && self.clock.elapsed() >= self.rate.period()
    }

    pub fn mapped(&self, frame: i64, in_out: bool) -> usize {
        let (start, end) = self.range(in_out);
        match self.loop_mode {
            LoopMode::Repeat => {
                start + (frame - start as i64).rem_euclid((end - start + 1) as i64) as usize
            }
            _ => frame.clamp(start as i64, end as i64) as usize,
        }
    }

    pub fn seek(&mut self, frame: i64, in_out: bool) {
        let next = self.mapped(frame, in_out);
        if next == self.frame {
            return;
        }
        self.frame = next;
        let (start, end) = self.range(in_out);
        match self.loop_mode {
            LoopMode::Once if frame <= start as i64 || frame >= end as i64 => {
                self.set_direction(Direction::Stop);
            }
            LoopMode::PingPong if self.direction != Direction::Stop => {
                if frame <= start as i64 {
                    self.set_direction(Direction::Forward);
                } else if frame >= end as i64 {
                    self.set_direction(Direction::Reverse);
                }
            }
            _ => {}
        }
    }

    pub fn next_frame(&self) -> i64 {
        self.frame as i64
            + if self.direction == Direction::Reverse {
                -1
            } else {
                1
            }
    }
}
