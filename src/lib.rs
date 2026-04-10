/*!
# cuda-circuit

Circuit breaker patterns for agent resilience.

When an external service fails repeatedly, stop trying and recover
gracefully. Circuit breakers prevent cascade failures across the fleet.

- Circuit breaker (closed/open/half-open)
- Configurable thresholds and timeouts
- Health monitoring
- Bulkhead isolation per service
- Metrics (trips, resets, success rate)
*/

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitState { Closed, Open, HalfOpen }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CircuitBreaker {
    pub name: String,
    pub state: CircuitState,
    pub failure_threshold: u32,
    pub success_threshold: u32,
    pub half_open_max: u32,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub half_open_calls: u32,
    pub open_since_ms: u64,
    pub open_duration_ms: u64,
    pub total_trips: u64,
    pub total_calls: u64,
    pub total_failures: u64,
    pub total_successes: u64,
    pub last_failure_ms: Option<u64>,
}

impl CircuitBreaker {
    pub fn new(name: &str, failure_threshold: u32, open_duration_ms: u64) -> Self {
        CircuitBreaker { name: name.to_string(), state: CircuitState::Closed, failure_threshold, success_threshold: 3, half_open_max: 5, consecutive_failures: 0, consecutive_successes: 0, half_open_calls: 0, open_since_ms: None, open_duration_ms, total_trips: 0, total_calls: 0, total_failures: 0, total_successes: 0, last_failure_ms: None }
    }

    /// Can we make a call?
    pub fn allow(&mut self) -> bool {
        self.total_calls += 1;
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(since) = self.open_since_ms {
                    if now() - since > self.open_duration_ms {
                        self.state = CircuitState::HalfOpen;
                        self.half_open_calls = 0;
                        true
                    } else { false }
                } else { false }
            }
            CircuitState::HalfOpen => self.half_open_calls < self.half_open_max,
        }
    }

    pub fn record_success(&mut self) {
        self.total_successes += 1;
        self.consecutive_failures = 0;
        match self.state {
            CircuitState::HalfOpen => {
                self.consecutive_successes += 1;
                if self.consecutive_successes >= self.success_threshold {
                    self.state = CircuitState::Closed;
                }
            }
            _ => {}
        }
    }

    pub fn record_failure(&mut self) {
        self.total_failures += 1;
        self.consecutive_successes = 0;
        self.last_failure_ms = Some(now());
        match self.state {
            CircuitState::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= self.failure_threshold {
                    self.trip();
                }
            }
            CircuitState::HalfOpen => { self.trip(); }
            _ => {}
        }
    }

    fn trip(&mut self) {
        self.state = CircuitState::Open;
        self.open_since_ms = Some(now());
        self.total_trips += 1;
        self.consecutive_failures = 0;
    }

    pub fn force_close(&mut self) {
        self.state = CircuitState::Closed;
        self.open_since_ms = None;
        self.consecutive_failures = 0;
    }

    pub fn success_rate(&self) -> f64 {
        if self.total_calls == 0 { return 1.0; }
        self.total_successes as f64 / self.total_calls as f64
    }

    pub fn uptime_pct(&self) -> f64 {
        match self.state {
            CircuitState::Closed => 1.0,
            CircuitState::Open => 0.0,
            CircuitState::HalfOpen => 0.5,
        }
    }

    pub fn summary(&self) -> String {
        format!("Circuit[{}]: state={:?}, trips={}, success_rate={:.1}%, calls={}",
            self.name, self.state, self.total_trips, self.success_rate() * 100.0, self.total_calls)
    }
}

/// A bulkhead isolates services
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bulkhead {
    pub name: String,
    pub max_concurrent: usize,
    pub active: usize,
    pub total_accepted: u64,
    pub total_rejected: u64,
}

impl Bulkhead {
    pub fn new(name: &str, max: usize) -> Self { Bulkhead { name: name.to_string(), max_concurrent: max, active: 0, total_accepted: 0, total_rejected: 0 } }
    pub fn try_enter(&mut self) -> bool {
        if self.active >= self.max_concurrent { self.total_rejected += 1; return false; }
        self.active += 1; self.total_accepted += 1; true
    }
    pub fn exit(&mut self) { self.active = self.active.saturating_sub(1); }
    pub fn utilization(&self) -> f64 { if self.max_concurrent == 0 { return 1.0; } self.active as f64 / self.max_concurrent as f64 }
}

fn now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starts_closed() {
        let cb = CircuitBreaker::new("test", 3, 5000);
        assert_eq!(cb.state, CircuitState::Closed);
        assert!(cb.allow());
    }

    #[test]
    fn test_trips_after_threshold() {
        let mut cb = CircuitBreaker::new("test", 3, 5000);
        cb.allow(); cb.record_failure();
        cb.allow(); cb.record_failure();
        cb.allow(); cb.record_failure();
        assert_eq!(cb.state, CircuitState::Open);
        assert!(!cb.allow());
    }

    #[test]
    fn test_success_resets_failures() {
        let mut cb = CircuitBreaker::new("test", 3, 5000);
        cb.allow(); cb.record_failure();
        cb.allow(); cb.record_failure();
        cb.allow(); cb.record_success(); // resets counter
        cb.allow(); cb.record_failure(); // only 1 consecutive
        assert_eq!(cb.state, CircuitState::Closed);
    }

    #[test]
    fn test_success_rate() {
        let mut cb = CircuitBreaker::new("test", 5, 5000);
        for _ in 0..8 { cb.allow(); cb.record_success(); }
        for _ in 0..2 { cb.allow(); cb.record_failure(); }
        assert!((cb.success_rate() - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_force_close() {
        let mut cb = CircuitBreaker::new("test", 1, 5000);
        cb.allow(); cb.record_failure();
        assert_eq!(cb.state, CircuitState::Open);
        cb.force_close();
        assert_eq!(cb.state, CircuitState::Closed);
    }

    #[test]
    fn test_bulkhead() {
        let mut bh = Bulkhead::new("x", 2);
        assert!(bh.try_enter());
        assert!(bh.try_enter());
        assert!(!bh.try_enter());
        bh.exit();
        assert!(bh.try_enter());
    }

    #[test]
    fn test_bulkhead_utilization() {
        let mut bh = Bulkhead::new("x", 4);
        bh.try_enter(); bh.try_enter();
        assert!((bh.utilization() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_summary() {
        let cb = CircuitBreaker::new("x", 3, 5000);
        let s = cb.summary();
        assert!(s.contains("CircuitState::Closed"));
    }
}
