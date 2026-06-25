use crate::problem::Problem;
use rand::{rng, seq::SliceRandom};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PracticeStats {
    pub total: usize,
    pub remaining: usize,
    pub correct: usize,
    pub wrong: usize,
}

#[derive(Debug, Default)]
pub struct PracticeDeck {
    queue: VecDeque<Problem>,
    current: Option<Problem>,
    stats: PracticeStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PracticeOrder {
    Sequential,
    Shuffled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitResult {
    Correct,
    Wrong {
        expected: String,
        explanation: String,
    },
    NoCurrentProblem,
}

impl PracticeDeck {
    pub fn with_order(mut problems: Vec<Problem>, order: PracticeOrder) -> Self {
        if order == PracticeOrder::Shuffled {
            problems.shuffle(&mut rng());
        }
        let total = problems.len();

        let mut deck = Self {
            queue: problems.into(),
            current: None,
            stats: PracticeStats {
                total,
                remaining: total,
                ..PracticeStats::default()
            },
        };
        deck.next();
        deck
    }

    pub fn current(&self) -> Option<&Problem> {
        self.current.as_ref()
    }

    pub fn stats(&self) -> PracticeStats {
        PracticeStats {
            remaining: self.queue.len() + usize::from(self.current.is_some()),
            ..self.stats
        }
    }

    pub fn next(&mut self) -> Option<&Problem> {
        if self.current.is_none() {
            self.current = self.queue.pop_front();
        }
        self.current()
    }

    pub fn skip(&mut self) {
        if let Some(problem) = self.current.take() {
            self.queue.push_back(problem);
        }
        self.current = self.queue.pop_front();
    }

    pub fn submit(&mut self, answer: &str) -> SubmitResult {
        let Some(problem) = self.current.take() else {
            return SubmitResult::NoCurrentProblem;
        };

        if problem.is_correct(answer) {
            self.stats.correct += 1;
            self.current = self.queue.pop_front();
            SubmitResult::Correct
        } else {
            self.stats.wrong += 1;
            let expected = problem.answer.clone();
            let explanation = problem.explanation.clone();
            self.queue.push_back(problem);
            self.current = self.queue.pop_front();
            SubmitResult::Wrong {
                expected,
                explanation,
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.current.is_none() && self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{PracticeDeck, PracticeOrder, SubmitResult};
    use crate::problem::Problem;

    fn problem(id: &str, answer: &str) -> Problem {
        Problem {
            id: id.into(),
            prompt: format!("题目 {id}"),
            answer: answer.into(),
            explanation: "解析".into(),
            tags: vec![],
            problem_type: None,
            deck_name: None,
            deck_info: None,
            images: vec![],
        }
    }

    #[test]
    fn correct_answer_removes_problem() {
        let mut deck = PracticeDeck::with_order(vec![problem("1", "a")], PracticeOrder::Shuffled);
        assert_eq!(deck.submit("a"), SubmitResult::Correct);
        assert!(deck.is_finished());
        assert_eq!(deck.stats().correct, 1);
    }

    #[test]
    fn wrong_answer_requeues_problem() {
        let mut deck = PracticeDeck::with_order(vec![problem("1", "a")], PracticeOrder::Shuffled);
        let result = deck.submit("b");
        assert!(matches!(result, SubmitResult::Wrong { .. }));
        assert!(!deck.is_finished());
        assert_eq!(deck.stats().wrong, 1);
        assert_eq!(deck.stats().remaining, 1);
    }
}
