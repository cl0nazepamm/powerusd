//! Animation controller for glTF animation playback.

#![allow(dead_code)]

/// Controller for managing animation playback state.
#[derive(Debug, Clone)]
pub struct AnimationController {
    /// Whether the animation is currently playing.
    pub playing: bool,
    /// Current playback time in seconds.
    pub current_time: f32,
    /// Playback speed multiplier (1.0 = normal speed).
    pub speed: f32,
    /// Total duration of the current animation in seconds.
    pub duration: f32,
    /// Whether the animation should loop.
    pub looping: bool,
}

impl Default for AnimationController {
    fn default() -> Self {
        Self {
            playing: true,
            current_time: 0.0,
            speed: 1.0,
            duration: 0.0,
            looping: true,
        }
    }
}

impl AnimationController {
    /// Create a new animation controller with the specified duration.
    pub fn new(duration: f32) -> Self {
        Self {
            duration,
            ..Default::default()
        }
    }

    /// Update the animation time based on elapsed delta time.
    ///
    /// Returns the current animation time to pass to the model's animate method.
    pub fn update(&mut self, delta_time: f32) -> f32 {
        if self.playing && self.duration > 0.0 {
            self.current_time += delta_time * self.speed;

            if self.looping {
                // Wrap around when exceeding duration
                while self.current_time >= self.duration {
                    self.current_time -= self.duration;
                }
                while self.current_time < 0.0 {
                    self.current_time += self.duration;
                }
            } else {
                // Clamp to duration and stop at end
                if self.current_time >= self.duration {
                    self.current_time = self.duration;
                    self.playing = false;
                } else if self.current_time < 0.0 {
                    self.current_time = 0.0;
                    self.playing = false;
                }
            }
        }

        self.current_time
    }

    /// Start or resume playback.
    pub fn play(&mut self) {
        self.playing = true;
    }

    /// Pause playback.
    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Toggle between play and pause.
    pub fn toggle(&mut self) {
        self.playing = !self.playing;
    }

    /// Stop playback and reset to the beginning.
    pub fn stop(&mut self) {
        self.playing = false;
        self.current_time = 0.0;
    }

    /// Seek to a specific time in seconds.
    pub fn seek(&mut self, time: f32) {
        self.current_time = time.clamp(0.0, self.duration);
    }

    /// Seek to a normalized position (0.0 = start, 1.0 = end).
    pub fn seek_normalized(&mut self, position: f32) {
        self.current_time = (position.clamp(0.0, 1.0) * self.duration).min(self.duration);
    }

    /// Get the current position as a normalized value (0.0 to 1.0).
    pub fn normalized_position(&self) -> f32 {
        if self.duration > 0.0 {
            self.current_time / self.duration
        } else {
            0.0
        }
    }

    /// Set the animation duration and reset playback.
    pub fn set_duration(&mut self, duration: f32) {
        self.duration = duration;
        self.current_time = 0.0;
    }
}
