use crate::problem::Problem;
use rand::{Rng, rng, seq::SliceRandom};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PracticeDeckSnapshot {
    pub queue: Vec<Problem>,
    pub current: Option<Problem>,
    pub stats: PracticeStats,
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

    pub fn from_snapshot(snapshot: PracticeDeckSnapshot) -> Self {
        Self {
            queue: snapshot.queue.into(),
            current: snapshot.current,
            stats: snapshot.stats,
        }
    }

    pub fn snapshot(&self) -> PracticeDeckSnapshot {
        PracticeDeckSnapshot {
            queue: self.queue.iter().cloned().collect(),
            current: self.current.clone(),
            stats: self.stats,
        }
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
            self.requeue(problem);
        }
        self.current = self.queue.pop_front();
    }

    pub fn requeue_current_without_advancing(&mut self) {
        if let Some(problem) = self.current.clone() {
            self.requeue(problem);
        }
    }

    pub fn submit_and_requeue(&mut self, answer: &str) -> SubmitResult {
        let Some(problem) = self.current.take() else {
            return SubmitResult::NoCurrentProblem;
        };

        if problem.is_correct(answer) {
            self.stats.correct += 1;
            self.requeue(problem);
            self.current = self.queue.pop_front();
            SubmitResult::Correct
        } else {
            self.stats.wrong += 1;
            let expected = problem.answer.clone();
            let explanation = problem.explanation.clone();
            self.requeue(problem);
            self.current = self.queue.pop_front();
            SubmitResult::Wrong {
                expected,
                explanation,
            }
        }
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
            self.requeue(problem);
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

    fn requeue(&mut self, problem: Problem) {
        if self.queue.is_empty() {
            self.queue.push_back(problem);
            return;
        }
        let start = (self.queue.len() / 3).max(1);
        let end = self.queue.len();
        let index = rng().random_range(start..=end);
        self.queue.insert(index, problem);
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

    #[test]
    fn requeue_current_without_advancing_keeps_current_problem_visible() {
        let mut deck = PracticeDeck::with_order(
            vec![problem("1", "a"), problem("2", "b"), problem("3", "c")],
            PracticeOrder::Sequential,
        );

        let current_id = deck.current().expect("current problem").id.clone();
        deck.requeue_current_without_advancing();

        assert_eq!(
            deck.current().expect("current problem after requeue").id,
            current_id
        );
        assert_eq!(deck.stats().remaining, 4);
    }

    #[test]
    fn submit_and_requeue_keeps_correct_problem_in_queue() {
        let mut deck = PracticeDeck::with_order(
            vec![problem("1", "a"), problem("2", "b")],
            PracticeOrder::Sequential,
        );

        assert_eq!(deck.submit_and_requeue("a"), SubmitResult::Correct);

        assert_eq!(deck.stats().correct, 1);
        assert_eq!(deck.stats().remaining, 2);
        assert!(!deck.is_finished());
    }

    #[test]
    fn deck_snapshot_restores_current_queue_and_stats() {
        let mut deck = PracticeDeck::with_order(
            vec![problem("1", "a"), problem("2", "b")],
            PracticeOrder::Sequential,
        );
        assert_eq!(
            deck.submit("x"),
            SubmitResult::Wrong {
                expected: "a".into(),
                explanation: "解析".into()
            }
        );

        let snapshot = deck.snapshot();
        let restored = PracticeDeck::from_snapshot(snapshot);

        assert_eq!(
            restored.current().expect("current").id,
            deck.current().expect("current").id
        );
        assert_eq!(restored.stats(), deck.stats());
    }
}
