#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RequestCapacity {
    pub(super) context_window: usize,
    pub(super) input_tokens: usize,
    pub(super) output_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CapacityDecision {
    Ready,
    NeedsCompaction,
    Conflict,
}

impl RequestCapacity {
    pub(super) fn decide(self, mandatory_input_tokens: usize) -> CapacityDecision {
        if self.input_tokens.saturating_add(self.output_tokens) <= self.context_window {
            CapacityDecision::Ready
        } else if mandatory_input_tokens.saturating_add(self.output_tokens) > self.context_window {
            CapacityDecision::Conflict
        } else {
            CapacityDecision::NeedsCompaction
        }
    }
}

pub(super) const SUCCESSOR_WORKING_ROOM_TOKENS: usize = 8192;

#[derive(Debug, Clone, Copy)]
pub(super) struct SuccessorCapacity {
    pub(super) context_window: usize,
    pub(super) ordinary_overhead_tokens: usize,
    pub(super) output_tokens: usize,
}

impl SuccessorCapacity {
    pub(super) fn handoff_budget(self) -> Result<usize, String> {
        let required = self
            .ordinary_overhead_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(SUCCESSOR_WORKING_ROOM_TOKENS);
        if required >= self.context_window {
            return Err(format!(
                "Required instructions/tools (estimated {} tokens), output reservation ({}), and successor working room ({}) leave no handoff space in the {}-token context. Handoff editing cannot fix required-overhead pressure; select a model with more available input capacity or revise required instructions/tools.",
                self.ordinary_overhead_tokens, self.output_tokens, SUCCESSOR_WORKING_ROOM_TOKENS, self.context_window,
            ));
        }
        Ok(self.context_window - required)
    }

    pub(super) fn validate(self, handoff_tokens: usize) -> Result<(), String> {
        let target = self.handoff_budget()?;
        if handoff_tokens > target {
            return Err(format!(
                "Generated handoff is estimated at {handoff_tokens} tokens; target is at most {target} after instructions/tools ({}), output reservation ({}), and {} tokens of successor working room. The original transcript is preserved. Review a shorter handoff or explicitly select a model with more input capacity.",
                self.ordinary_overhead_tokens, self.output_tokens, SUCCESSOR_WORKING_ROOM_TOKENS,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_output_reservation_distinguishes_reducible_history_from_fixed_conflict() {
        let capacity = RequestCapacity {
            context_window: 272_000,
            input_tokens: 150_000,
            output_tokens: 128_000,
        };
        assert_eq!(capacity.decide(10_000), CapacityDecision::NeedsCompaction);
        assert_eq!(capacity.decide(145_000), CapacityDecision::Conflict);
        assert_eq!(
            RequestCapacity {
                input_tokens: 144_000,
                ..capacity
            }
            .decide(10_000),
            CapacityDecision::Ready
        );
    }

    #[test]
    fn handoff_that_barely_fits_does_not_leave_working_room() {
        let successor = SuccessorCapacity {
            context_window: 272_000,
            ordinary_overhead_tokens: 20_000,
            output_tokens: 128_000,
        };
        assert_eq!(successor.handoff_budget().unwrap(), 115_808);
        assert!(successor.validate(124_000).is_err());
        assert!(successor.validate(115_808).is_ok());
        assert!(SuccessorCapacity {
            ordinary_overhead_tokens: usize::MAX,
            ..successor
        }
        .handoff_budget()
        .is_err());
    }
}
