#[derive(Debug, Clone)]
pub struct RetryBudget {
    max: u8,
    used: u8,
}

impl RetryBudget {
    pub fn new(max: u8) -> Self {
        Self { max, used: 0 }
    }

    pub fn consume(&mut self) -> bool {
        if self.used < self.max {
            self.used += 1;
            true
        } else {
            false
        }
    }

    pub fn remaining(&self) -> u8 {
        self.max.saturating_sub(self.used)
    }

    pub fn exhausted(&self) -> bool {
        self.used >= self.max
    }

    pub fn attempt(&self) -> u8 {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_budget_default() {
        let budget = RetryBudget::new(3);
        assert_eq!(budget.remaining(), 3);
        assert!(!budget.exhausted());
    }

    #[test]
    fn test_retry_budget_consume() {
        let mut budget = RetryBudget::new(3);
        assert!(budget.consume());
        assert!(budget.consume());
        assert!(budget.consume());
        assert!(!budget.consume());
        assert!(budget.exhausted());
    }

    #[test]
    fn test_retry_budget_zero() {
        let budget = RetryBudget::new(0);
        assert!(budget.exhausted());
    }
}
