use crate::problem::{
    Problem, ProblemAnswerSource, ProblemImage, ProblemReviewConfidence, ProblemReviewStatus,
    ProblemReviewVerdict, ProblemState, ProblemType, normalize_problem,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{error::Error, path::PathBuf};

#[cfg(target_os = "android")]
static ANDROID_DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

#[cfg(target_os = "android")]
pub fn set_android_data_dir(path: PathBuf) {
    let _ = ANDROID_DATA_DIR.set(path);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub deck_id: i64,
    pub imported: usize,
    pub inserted: usize,
    pub updated: usize,
}

#[derive(Debug, Clone)]
pub struct DeckCard {
    pub id: i64,
    pub name: String,
    pub source_path: String,
    pub inserted: i64,
    pub updated: i64,
    pub problem_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct GroupCard {
    pub id: i64,
    pub name: String,
    pub deck_count: i64,
    pub problem_count: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct GroupDeckCard {
    pub id: i64,
    pub name: String,
    pub problem_count: i64,
}

#[derive(Debug, Clone)]
pub struct AnswerRecord {
    pub answered_at: String,
    pub problem_id: String,
    pub user_answer: String,
    pub correct_answer: String,
    pub is_correct: bool,
}

pub struct AppStore {
    conn: Connection,
}

impl AppStore {
    pub fn open_default() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let path = default_db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::open(path)
    }

    pub fn open(path: PathBuf) -> Result<Self, Box<dyn Error + Send + Sync>> {
        log::info!("Opening SQLite store: path={}", path.display());
        let conn = Connection::open(path)?;
        conn.execute_batch("pragma foreign_keys = on;")?;
        let store = Self { conn };
        store.migrate()?;
        log::info!("SQLite store opened and migrated");
        Ok(store)
    }

    pub fn import_problems(
        &mut self,
        problems: &[Problem],
        source_path: &str,
    ) -> Result<ImportSummary, Box<dyn Error + Send + Sync>> {
        log::info!(
            "Persisting problem import: source_path={}, problem_count={}",
            source_path,
            problems.len()
        );
        let tx = self.conn.transaction()?;
        let deck_name = problems
            .iter()
            .find_map(|problem| problem.deck_name.as_deref())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| deck_name_from_source(source_path));
        let deck_id = upsert_deck(&tx, &deck_name, source_path)?;
        let mut summary = ImportSummary {
            deck_id,
            imported: problems.len(),
            ..ImportSummary::default()
        };

        for problem in problems {
            let problem = normalize_problem(problem.clone());
            let exists: Option<String> = tx
                .query_row(
                    "select id from problems where id = ?1",
                    params![problem.id],
                    |row| row.get(0),
                )
                .optional()?;

            let tags = serde_json::to_string(&problem.tags)?;
            let images = serde_json::to_string(&problem.images)?;
            tx.execute(
                "insert into problems (
                    id, prompt, answer, explanation, tags, problem_type, images_json,
                    user_answer, answer_source, review_needed, review_status, review_verdict,
                    review_confidence, score_display, updated_at
                 )
                 values (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    datetime('now', 'localtime')
                 )
                 on conflict(id) do update set
                   prompt = excluded.prompt,
                   answer = excluded.answer,
                   explanation = excluded.explanation,
                   tags = excluded.tags,
                   problem_type = excluded.problem_type,
                   images_json = excluded.images_json,
                   user_answer = excluded.user_answer,
                   answer_source = excluded.answer_source,
                   review_needed = excluded.review_needed,
                   review_status = excluded.review_status,
                   review_verdict = excluded.review_verdict,
                   review_confidence = excluded.review_confidence,
                   score_display = excluded.score_display,
                   updated_at = datetime('now', 'localtime')",
                params![
                    problem.id,
                    problem.prompt,
                    problem.answer,
                    problem.explanation,
                    tags,
                    problem_type_to_str(problem.kind()),
                    images,
                    problem.state.user_answer,
                    answer_source_to_str(problem.state.answer_source),
                    i64::from(problem.state.review_needed),
                    review_status_to_str(problem.state.review_status),
                    review_verdict_to_str(problem.state.review_verdict),
                    review_confidence_to_str(problem.state.review_confidence),
                    problem.state.score_display,
                ],
            )?;

            if exists.is_some() {
                summary.updated += 1;
            } else {
                summary.inserted += 1;
            }

            tx.execute(
                "insert or ignore into deck_problems (deck_id, problem_id) values (?1, ?2)",
                params![deck_id, problem.id],
            )?;
        }

        let problem_count: i64 = tx.query_row(
            "select count(*) from deck_problems where deck_id = ?1",
            params![deck_id],
            |row| row.get(0),
        )?;

        tx.execute(
            "update decks
             set name = ?1,
                 source_path = ?2,
                 imported = ?3,
                 inserted = inserted + ?4,
                 updated = updated + ?5,
                 problem_count = ?6,
                 updated_at = datetime('now', 'localtime')
             where id = ?7",
            params![
                deck_name,
                source_path,
                summary.imported as i64,
                summary.inserted as i64,
                summary.updated as i64,
                problem_count,
                deck_id,
            ],
        )?;

        tx.execute(
            "insert into import_history (imported_at, source_path, imported, inserted, updated)
             values (datetime('now', 'localtime'), ?1, ?2, ?3, ?4)",
            params![
                source_path,
                summary.imported as i64,
                summary.inserted as i64,
                summary.updated as i64
            ],
        )?;
        tx.commit()?;
        log::info!(
            "Problem import persisted: deck_id={}, deck_name={}, imported={}, inserted={}, updated={}, deck_problem_count={}",
            summary.deck_id,
            deck_name,
            summary.imported,
            summary.inserted,
            summary.updated,
            problem_count
        );
        Ok(summary)
    }

    pub fn deck_cards(&self) -> Result<Vec<DeckCard>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select id, name, source_path, inserted, updated, problem_count, updated_at
             from decks
             order by updated_at desc, id desc",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DeckCard {
                id: row.get(0)?,
                name: row.get(1)?,
                source_path: row.get(2)?,
                inserted: row.get(3)?,
                updated: row.get(4)?,
                problem_count: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn group_cards(&self) -> Result<Vec<GroupCard>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select g.id,
                g.name,
                count(distinct gd.deck_id) as deck_count,
                count(distinct dp.problem_id) as problem_count,
                g.updated_at
             from deck_groups g
             left join group_decks gd on gd.group_id = g.id
             left join deck_problems dp on dp.deck_id = gd.deck_id
             group by g.id, g.name, g.updated_at
             order by g.updated_at desc, g.id desc",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(GroupCard {
                id: row.get(0)?,
                name: row.get(1)?,
                deck_count: row.get(2)?,
                problem_count: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn create_group(&self, name: &str) -> Result<i64, Box<dyn Error + Send + Sync>> {
        let name = name.trim();
        if name.is_empty() {
            return Err("题组名称不能为空".into());
        }
        self.conn.execute(
            "insert into deck_groups (name, updated_at)
             values (?1, datetime('now', 'localtime'))",
            params![name],
        )?;
        let group_id = self.conn.last_insert_rowid();
        log::info!("Group persisted: group_id={}, name={}", group_id, name);
        Ok(group_id)
    }

    pub fn add_deck_to_group(
        &self,
        group_id: i64,
        deck_id: i64,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn.execute(
            "insert or ignore into group_decks (group_id, deck_id) values (?1, ?2)",
            params![group_id, deck_id],
        )?;
        self.conn.execute(
            "update deck_groups set updated_at = datetime('now', 'localtime') where id = ?1",
            params![group_id],
        )?;
        log::info!(
            "Deck added to group: group_id={}, deck_id={}",
            group_id,
            deck_id
        );
        Ok(())
    }

    pub fn remove_deck_from_group(
        &self,
        group_id: i64,
        deck_id: i64,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn.execute(
            "delete from group_decks where group_id = ?1 and deck_id = ?2",
            params![group_id, deck_id],
        )?;
        self.conn.execute(
            "update deck_groups set updated_at = datetime('now', 'localtime') where id = ?1",
            params![group_id],
        )?;
        log::info!(
            "Deck removed from group: group_id={}, deck_id={}",
            group_id,
            deck_id
        );
        Ok(())
    }

    pub fn delete_deck(&mut self, deck_id: i64) -> Result<(), Box<dyn Error + Send + Sync>> {
        let tx = self.conn.transaction()?;
        let problem_ids = {
            let mut stmt = tx.prepare("select problem_id from deck_problems where deck_id = ?1")?;
            let rows = stmt.query_map(params![deck_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let candidate_problem_count = problem_ids.len();

        tx.execute("delete from decks where id = ?1", params![deck_id])?;

        for problem_id in problem_ids {
            let deleted_problem_count = tx.execute(
                "delete from problems
                 where id = ?1
                   and not exists (
                     select 1 from deck_problems where problem_id = ?1
                   )",
                params![problem_id],
            )?;
            if deleted_problem_count > 0 {
                tx.execute(
                    "delete from answer_history where problem_id = ?1",
                    params![problem_id],
                )?;
            }
        }

        tx.commit()?;
        log::info!(
            "Deck deleted: deck_id={}, candidate_problem_count={}",
            deck_id,
            candidate_problem_count
        );
        Ok(())
    }

    pub fn delete_group(&self, group_id: i64) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn
            .execute("delete from deck_groups where id = ?1", params![group_id])?;
        log::info!("Group deleted: group_id={}", group_id);
        Ok(())
    }

    pub fn load_all_problems(&self) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select id, prompt, answer, explanation, tags, problem_type, images_json,
                    user_answer, answer_source, review_needed, review_status, review_verdict,
                    review_confidence, score_display
             from problems order by created_at asc, id asc",
        )?;
        let rows = stmt.query_map([], problem_from_row)?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_deck_problems(
        &self,
        deck_id: i64,
    ) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select p.id, p.prompt, p.answer, p.explanation, p.tags, p.problem_type, p.images_json,
                    p.user_answer, p.answer_source, p.review_needed, p.review_status,
                    p.review_verdict, p.review_confidence, p.score_display
             from problems p
             join deck_problems dp on dp.problem_id = p.id
             where dp.deck_id = ?1
             order by dp.added_at asc, p.id asc",
        )?;
        let rows = stmt.query_map(params![deck_id], problem_from_row)?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_group_problems(
        &self,
        group_id: i64,
    ) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select distinct p.id, p.prompt, p.answer, p.explanation, p.tags, p.problem_type, p.images_json,
                    p.user_answer, p.answer_source, p.review_needed, p.review_status,
                    p.review_verdict, p.review_confidence, p.score_display
             from problems p
             join deck_problems dp on dp.problem_id = p.id
             join group_decks gd on gd.deck_id = dp.deck_id
             where gd.group_id = ?1
             order by p.created_at asc, p.id asc",
        )?;
        let rows = stmt.query_map(params![group_id], problem_from_row)?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn group_decks(
        &self,
        group_id: i64,
    ) -> Result<Vec<GroupDeckCard>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select d.id, d.name, d.problem_count
             from decks d
             join group_decks gd on gd.deck_id = d.id
             where gd.group_id = ?1
             order by gd.added_at asc, d.id asc",
        )?;
        let rows = stmt.query_map(params![group_id], |row| {
            Ok(GroupDeckCard {
                id: row.get(0)?,
                name: row.get(1)?,
                problem_count: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn problem_count(&self) -> Result<usize, Box<dyn Error + Send + Sync>> {
        let count: i64 = self
            .conn
            .query_row("select count(*) from problems", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn record_answer(
        &self,
        problem_id: &str,
        user_answer: &str,
        correct_answer: &str,
        is_correct: bool,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn.execute(
            "insert into answer_history (answered_at, problem_id, user_answer, correct_answer, is_correct)
             values (datetime('now', 'localtime'), ?1, ?2, ?3, ?4)",
            params![problem_id, user_answer, correct_answer, i64::from(is_correct)],
        )?;
        log::info!(
            "Answer record persisted: problem_id={}, is_correct={}, user_answer_chars={}, correct_answer_chars={}",
            problem_id,
            is_correct,
            user_answer.chars().count(),
            correct_answer.chars().count()
        );
        Ok(())
    }

    pub fn update_problem_tags(
        &mut self,
        problems: &[Problem],
    ) -> Result<usize, Box<dyn Error + Send + Sync>> {
        let tx = self.conn.transaction()?;
        let mut updated = 0usize;
        for problem in problems {
            let tags = serde_json::to_string(&problem.tags)?;
            updated += tx.execute(
                "update problems
                 set tags = ?2,
                     updated_at = datetime('now', 'localtime')
                 where id = ?1",
                params![problem.id, tags],
            )?;
        }
        tx.commit()?;
        log::info!("Problem tags updated: count={updated}");
        Ok(updated)
    }

    pub fn update_problem_explanations(
        &mut self,
        problems: &[Problem],
    ) -> Result<usize, Box<dyn Error + Send + Sync>> {
        let tx = self.conn.transaction()?;
        let mut updated = 0usize;
        for problem in problems {
            if !problem
                .explanation
                .trim_start()
                .starts_with("AI预生成解析：")
            {
                continue;
            }
            updated += tx.execute(
                "update problems
                 set explanation = ?2,
                     updated_at = datetime('now', 'localtime')
                 where id = ?1",
                params![problem.id, problem.explanation],
            )?;
        }
        tx.commit()?;
        log::info!("Problem explanations updated: count={updated}");
        Ok(updated)
    }

    pub fn update_problem_manual_answer(
        &mut self,
        problem_id: &str,
        answer: &str,
    ) -> Result<usize, Box<dyn Error + Send + Sync>> {
        let answer = answer.trim();
        if answer.is_empty() {
            return Err("人工修正答案不能为空".into());
        }

        let mut stmt = self.conn.prepare(
            "select id, prompt, answer, explanation, tags, problem_type, images_json,
                    user_answer, answer_source, review_needed, review_status, review_verdict,
                    review_confidence, score_display
             from problems
             where id = ?1",
        )?;
        let Some(mut problem) = stmt
            .query_row(params![problem_id], problem_from_row)
            .optional()?
        else {
            return Ok(0);
        };
        problem.answer = answer.to_owned();
        problem.state.user_answer.clear();
        problem.state.answer_source = ProblemAnswerSource::ManualReviewed;
        problem.state.review_needed = false;
        problem.state.review_status = ProblemReviewStatus::Accepted;
        problem.state.review_verdict = ProblemReviewVerdict::Unknown;
        problem.state.review_confidence = ProblemReviewConfidence::High;
        problem.state.score_display.clear();
        let problem = normalize_problem(problem);
        drop(stmt);

        let updated = self.conn.execute(
            "update problems
             set answer = ?2,
                 user_answer = '',
                 answer_source = ?3,
                 review_needed = 0,
                 review_status = ?4,
                 review_verdict = ?5,
                 review_confidence = ?6,
                 score_display = '',
                 updated_at = datetime('now', 'localtime')
             where id = ?1",
            params![
                problem_id,
                problem.answer,
                answer_source_to_str(ProblemAnswerSource::ManualReviewed),
                review_status_to_str(ProblemReviewStatus::Accepted),
                review_verdict_to_str(ProblemReviewVerdict::Unknown),
                review_confidence_to_str(ProblemReviewConfidence::High),
            ],
        )?;
        log::info!(
            "Manual answer updated: problem_id={}, answer_chars={}, updated={}",
            problem_id,
            answer.chars().count(),
            updated
        );
        Ok(updated)
    }

    pub fn update_ai_review_accepted_answers(
        &mut self,
        problems: &[Problem],
    ) -> Result<usize, Box<dyn Error + Send + Sync>> {
        let tx = self.conn.transaction()?;
        let mut updated = 0usize;
        for problem in problems {
            if problem.state.answer_source != ProblemAnswerSource::AiReviewed
                || problem.state.review_status != ProblemReviewStatus::Accepted
                || problem.answer.trim().is_empty()
            {
                continue;
            }
            updated += tx.execute(
                "update problems
                 set answer = ?2,
                     explanation = ?3,
                     user_answer = ?4,
                     answer_source = ?5,
                     review_needed = ?6,
                     review_status = ?7,
                     review_verdict = ?8,
                     review_confidence = ?9,
                     score_display = ?10,
                     updated_at = datetime('now', 'localtime')
                 where id = ?1",
                params![
                    problem.id,
                    problem.answer,
                    problem.explanation,
                    problem.state.user_answer,
                    answer_source_to_str(problem.state.answer_source),
                    i64::from(problem.state.review_needed),
                    review_status_to_str(problem.state.review_status),
                    review_verdict_to_str(problem.state.review_verdict),
                    review_confidence_to_str(problem.state.review_confidence),
                    problem.state.score_display,
                ],
            )?;
        }
        tx.commit()?;
        log::info!("AI-reviewed accepted answers updated: count={updated}");
        Ok(updated)
    }

    pub fn update_problems_after_ai_review(
        &mut self,
        problems: &[Problem],
    ) -> Result<usize, Box<dyn Error + Send + Sync>> {
        self.update_ai_review_accepted_answers(problems)
    }

    pub fn answer_history(
        &self,
        limit: usize,
    ) -> Result<Vec<AnswerRecord>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select answered_at, problem_id, user_answer, correct_answer, is_correct
             from answer_history
             order by id desc
             limit ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let is_correct: i64 = row.get(4)?;
            Ok(AnswerRecord {
                answered_at: row.get(0)?,
                problem_id: row.get(1)?,
                user_answer: row.get(2)?,
                correct_answer: row.get(3)?,
                is_correct: is_correct != 0,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn ensure_problem_images_column(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare("pragma table_info(problems)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let columns = rows.collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|name| name == "images_json") {
            self.conn.execute(
                "alter table problems add column images_json text not null default '[]'",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_problem_state_columns(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare("pragma table_info(problems)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let columns = rows.collect::<Result<Vec<_>, _>>()?;

        if !columns.iter().any(|name| name == "user_answer") {
            self.conn.execute(
                "alter table problems add column user_answer text not null default ''",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "answer_source") {
            self.conn.execute(
                "alter table problems add column answer_source text not null default 'standard'",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "review_needed") {
            self.conn.execute(
                "alter table problems add column review_needed integer not null default 0",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "review_status") {
            self.conn.execute(
                "alter table problems add column review_status text not null default 'none'",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "review_verdict") {
            self.conn.execute(
                "alter table problems add column review_verdict text not null default 'unknown'",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "review_confidence") {
            self.conn.execute(
                "alter table problems add column review_confidence text not null default 'unknown'",
                [],
            )?;
        }
        if !columns.iter().any(|name| name == "score_display") {
            self.conn.execute(
                "alter table problems add column score_display text not null default ''",
                [],
            )?;
        }
        Ok(())
    }

    fn backfill_problem_state_columns(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select id, prompt, answer, explanation, tags, problem_type, images_json,
                    user_answer, answer_source, review_needed, review_status, review_verdict,
                    review_confidence, score_display
             from problems",
        )?;
        let rows = stmt.query_map([], problem_from_row)?;
        let problems = rows.collect::<Result<Vec<_>, _>>()?;

        let tx = self.conn.unchecked_transaction()?;
        let mut updated = 0usize;
        for original in problems {
            let normalized = normalize_problem(original.clone());
            if normalized == original {
                continue;
            }
            let tags = serde_json::to_string(&normalized.tags)?;
            let images = serde_json::to_string(&normalized.images)?;
            updated += tx.execute(
                "update problems
                 set answer = ?2,
                     explanation = ?3,
                     tags = ?4,
                     problem_type = ?5,
                     images_json = ?6,
                     user_answer = ?7,
                     answer_source = ?8,
                     review_needed = ?9,
                     review_status = ?10,
                     review_verdict = ?11,
                     review_confidence = ?12,
                     score_display = ?13,
                     updated_at = datetime('now', 'localtime')
                 where id = ?1",
                params![
                    normalized.id,
                    normalized.answer,
                    normalized.explanation,
                    tags,
                    problem_type_to_str(normalized.kind()),
                    images,
                    normalized.state.user_answer,
                    answer_source_to_str(normalized.state.answer_source),
                    i64::from(normalized.state.review_needed),
                    review_status_to_str(normalized.state.review_status),
                    review_verdict_to_str(normalized.state.review_verdict),
                    review_confidence_to_str(normalized.state.review_confidence),
                    normalized.state.score_display,
                ],
            )?;
        }
        tx.commit()?;
        if updated > 0 {
            log::info!("Problem state backfilled from legacy tags/text: count={updated}");
        }
        Ok(())
    }

    pub fn deck_answer_history(
        &self,
        deck_id: i64,
        limit: usize,
    ) -> Result<Vec<AnswerRecord>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select ah.answered_at, ah.problem_id, ah.user_answer, ah.correct_answer, ah.is_correct
             from answer_history ah
             join deck_problems dp on dp.problem_id = ah.problem_id
             where dp.deck_id = ?1
             order by ah.id desc
             limit ?2",
        )?;
        let rows = stmt.query_map(params![deck_id, limit as i64], answer_record_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn group_answer_history(
        &self,
        group_id: i64,
        limit: usize,
    ) -> Result<Vec<AnswerRecord>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select distinct ah.id, ah.answered_at, ah.problem_id, ah.user_answer, ah.correct_answer, ah.is_correct
             from answer_history ah
             join deck_problems dp on dp.problem_id = ah.problem_id
             join group_decks gd on gd.deck_id = dp.deck_id
             where gd.group_id = ?1
             order by ah.id desc
             limit ?2",
        )?;
        let rows = stmt.query_map(params![group_id, limit as i64], |row| {
            let is_correct: i64 = row.get(5)?;
            Ok(AnswerRecord {
                answered_at: row.get(1)?,
                problem_id: row.get(2)?,
                user_answer: row.get(3)?,
                correct_answer: row.get(4)?,
                is_correct: is_correct != 0,
            })
        })?;
        self.ensure_problem_images_column()?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
        Ok(self
            .conn
            .query_row(
                "select value from app_settings where key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn.execute(
            "insert into app_settings (key, value, updated_at)
             values (?1, ?2, datetime('now', 'localtime'))
             on conflict(key) do update set
               value = excluded.value,
               updated_at = datetime('now', 'localtime')",
            params![key, value],
        )?;
        log::info!(
            "Setting persisted: key={}, value_chars={}",
            key,
            value.chars().count()
        );
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn
            .execute("delete from app_settings where key = ?1", params![key])?;
        log::info!("Setting deleted: key={key}");
        Ok(())
    }

    fn migrate(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn.execute_batch(
            "create table if not exists problems (
                id text primary key,
                prompt text not null,
                answer text not null,
                explanation text not null default '',
                tags text not null default '[]',
                problem_type text not null default 'text',
                images_json text not null default '[]',
                created_at text not null default (datetime('now', 'localtime')),
                updated_at text not null default (datetime('now', 'localtime'))
            );

            create table if not exists decks (
                id integer primary key autoincrement,
                name text not null,
                source_path text not null unique,
                imported integer not null default 0,
                inserted integer not null default 0,
                updated integer not null default 0,
                problem_count integer not null default 0,
                created_at text not null default (datetime('now', 'localtime')),
                updated_at text not null default (datetime('now', 'localtime'))
            );

            create table if not exists deck_problems (
                deck_id integer not null,
                problem_id text not null,
                added_at text not null default (datetime('now', 'localtime')),
                primary key (deck_id, problem_id),
                foreign key (deck_id) references decks(id) on delete cascade,
                foreign key (problem_id) references problems(id) on delete cascade
            );

            create table if not exists deck_groups (
                id integer primary key autoincrement,
                name text not null,
                created_at text not null default (datetime('now', 'localtime')),
                updated_at text not null default (datetime('now', 'localtime'))
            );

            create table if not exists group_decks (
                group_id integer not null,
                deck_id integer not null,
                added_at text not null default (datetime('now', 'localtime')),
                primary key (group_id, deck_id),
                foreign key (group_id) references deck_groups(id) on delete cascade,
                foreign key (deck_id) references decks(id) on delete cascade
            );

            create table if not exists import_history (
                id integer primary key autoincrement,
                imported_at text not null,
                source_path text not null,
                imported integer not null,
                inserted integer not null,
                updated integer not null
            );

            create table if not exists answer_history (
                id integer primary key autoincrement,
                answered_at text not null,
                problem_id text not null,
                user_answer text not null,
                correct_answer text not null,
                is_correct integer not null
            );

            create table if not exists app_settings (
                key text primary key,
                value text not null,
                updated_at text not null default (datetime('now', 'localtime'))
            );",
        )?;
        self.ensure_problem_images_column()?;
        self.ensure_problem_state_columns()?;
        self.backfill_problem_state_columns()?;
        self.seed_legacy_deck()?;
        Ok(())
    }

    fn seed_legacy_deck(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let deck_count: i64 = self
            .conn
            .query_row("select count(*) from decks", [], |row| row.get(0))?;
        let problem_count: i64 =
            self.conn
                .query_row("select count(*) from problems", [], |row| row.get(0))?;
        if deck_count > 0 || problem_count == 0 {
            return Ok(());
        }

        self.conn.execute(
            "insert into decks (name, source_path, imported, inserted, updated, problem_count)
             values ('历史全量题库', 'legacy://all-problems', ?1, ?1, 0, ?1)",
            params![problem_count],
        )?;
        let deck_id = self.conn.last_insert_rowid();
        self.conn.execute(
            "insert or ignore into deck_problems (deck_id, problem_id)
             select ?1, id from problems",
            params![deck_id],
        )?;
        log::info!(
            "Legacy deck seeded: deck_id={}, problem_count={}",
            deck_id,
            problem_count
        );
        Ok(())
    }
}

fn upsert_deck(
    tx: &rusqlite::Transaction<'_>,
    name: &str,
    source_path: &str,
) -> Result<i64, Box<dyn Error + Send + Sync>> {
    tx.execute(
        "insert into decks (name, source_path, updated_at)
         values (?1, ?2, datetime('now', 'localtime'))
         on conflict(source_path) do update set
           name = excluded.name,
           updated_at = datetime('now', 'localtime')",
        params![name, source_path],
    )?;
    Ok(tx.query_row(
        "select id from decks where source_path = ?1",
        params![source_path],
        |row| row.get(0),
    )?)
}

fn deck_name_from_source(source_path: &str) -> String {
    let path = PathBuf::from(source_path);
    path.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(|name| name.trim().to_owned())
        .unwrap_or_else(|| "未命名题库".into())
}

fn default_db_path() -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    #[cfg(target_os = "android")]
    {
        // Android: current_dir() can be "/" (read-only) under NativeActivity.
        // The entrypoint records AndroidApp::internal_data_path(), which points to
        // the app-private writable directory.
        let base = ANDROID_DATA_DIR
            .get()
            .cloned()
            .or_else(|| dirs::data_local_dir())
            .ok_or("无法确定 Android 本地数据目录")?;
        let db_dir = base.join("ShuaForge");
        std::fs::create_dir_all(&db_dir)?;
        Ok(db_dir.join("shuaforge.sqlite3"))
    }
    #[cfg(not(target_os = "android"))]
    {
        let base = dirs::data_local_dir()
            .or_else(|| std::env::current_dir().ok())
            .ok_or("无法确定本地数据目录")?;
        Ok(base.join("ShuaForge").join("shuaforge.sqlite3"))
    }
}

fn problem_type_to_str(problem_type: ProblemType) -> &'static str {
    match problem_type {
        ProblemType::SingleChoice => "single_choice",
        ProblemType::MultipleChoice => "multiple_choice",
        ProblemType::Text => "text",
    }
}

fn str_to_problem_type(value: &str) -> Option<ProblemType> {
    match value {
        "single_choice" => Some(ProblemType::SingleChoice),
        "multiple_choice" => Some(ProblemType::MultipleChoice),
        "text" => Some(ProblemType::Text),
        _ => None,
    }
}

fn answer_source_to_str(source: ProblemAnswerSource) -> &'static str {
    match source {
        ProblemAnswerSource::Standard => "standard",
        ProblemAnswerSource::UserTemporary => "user_temporary",
        ProblemAnswerSource::ScoreInferred => "score_inferred",
        ProblemAnswerSource::AiReviewed => "ai_reviewed",
        ProblemAnswerSource::ManualReviewed => "manual_reviewed",
    }
}

fn str_to_answer_source(value: &str) -> ProblemAnswerSource {
    match value {
        "user_temporary" => ProblemAnswerSource::UserTemporary,
        "score_inferred" => ProblemAnswerSource::ScoreInferred,
        "ai_reviewed" => ProblemAnswerSource::AiReviewed,
        "manual_reviewed" => ProblemAnswerSource::ManualReviewed,
        _ => ProblemAnswerSource::Standard,
    }
}

fn review_status_to_str(status: ProblemReviewStatus) -> &'static str {
    match status {
        ProblemReviewStatus::None => "none",
        ProblemReviewStatus::Pending => "pending",
        ProblemReviewStatus::Accepted => "accepted",
        ProblemReviewStatus::Unknown => "unknown",
        ProblemReviewStatus::Conflict => "conflict",
    }
}

fn str_to_review_status(value: &str) -> ProblemReviewStatus {
    match value {
        "pending" => ProblemReviewStatus::Pending,
        "accepted" => ProblemReviewStatus::Accepted,
        "unknown" => ProblemReviewStatus::Unknown,
        "conflict" => ProblemReviewStatus::Conflict,
        _ => ProblemReviewStatus::None,
    }
}

fn review_verdict_to_str(verdict: ProblemReviewVerdict) -> &'static str {
    match verdict {
        ProblemReviewVerdict::Correct => "correct",
        ProblemReviewVerdict::Wrong => "wrong",
        ProblemReviewVerdict::Unknown => "unknown",
    }
}

fn str_to_review_verdict(value: &str) -> ProblemReviewVerdict {
    match value {
        "correct" => ProblemReviewVerdict::Correct,
        "wrong" => ProblemReviewVerdict::Wrong,
        _ => ProblemReviewVerdict::Unknown,
    }
}

fn review_confidence_to_str(confidence: ProblemReviewConfidence) -> &'static str {
    match confidence {
        ProblemReviewConfidence::High => "high",
        ProblemReviewConfidence::Medium => "medium",
        ProblemReviewConfidence::Low => "low",
        ProblemReviewConfidence::Unknown => "unknown",
    }
}

fn str_to_review_confidence(value: &str) -> ProblemReviewConfidence {
    match value {
        "high" => ProblemReviewConfidence::High,
        "medium" => ProblemReviewConfidence::Medium,
        "low" => ProblemReviewConfidence::Low,
        _ => ProblemReviewConfidence::Unknown,
    }
}

fn problem_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Problem> {
    let tags_json: String = row.get(4)?;
    let problem_type_text: String = row.get(5)?;
    let images_json: String = row.get(6)?;
    let answer_source: String = row.get(8)?;
    let review_needed: i64 = row.get(9)?;
    let review_status: String = row.get(10)?;
    let review_verdict: String = row.get(11)?;
    let review_confidence: String = row.get(12)?;

    Ok(normalize_problem(Problem {
        id: row.get(0)?,
        prompt: row.get(1)?,
        answer: row.get(2)?,
        explanation: row.get(3)?,
        tags: serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default(),
        problem_type: str_to_problem_type(&problem_type_text),
        deck_name: None,
        deck_info: None,
        images: serde_json::from_str::<Vec<ProblemImage>>(&images_json).unwrap_or_default(),
        state: ProblemState {
            user_answer: row.get(7)?,
            answer_source: str_to_answer_source(&answer_source),
            review_needed: review_needed != 0,
            review_status: str_to_review_status(&review_status),
            review_verdict: str_to_review_verdict(&review_verdict),
            review_confidence: str_to_review_confidence(&review_confidence),
            score_display: row.get(13)?,
        },
    }))
}

fn answer_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AnswerRecord> {
    let is_correct: i64 = row.get(4)?;
    Ok(AnswerRecord {
        answered_at: row.get(0)?,
        problem_id: row.get(1)?,
        user_answer: row.get(2)?,
        correct_answer: row.get(3)?,
        is_correct: is_correct != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::problem::ProblemState;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn problem(id: &str) -> Problem {
        Problem {
            id: id.into(),
            prompt: format!("题目 {id}"),
            answer: "A".into(),
            explanation: String::new(),
            tags: vec![],
            problem_type: Some(ProblemType::SingleChoice),
            deck_name: None,
            deck_info: None,
            images: vec![],
            state: ProblemState::default(),
        }
    }

    fn temp_db_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("shuaforge-{name}-{nanos}.sqlite3"))
    }

    #[test]
    fn deleting_last_deck_removes_orphan_problems_and_prevents_legacy_seed() {
        let path = temp_db_path("delete-last-deck");
        {
            let mut store = AppStore::open(path.clone()).expect("open temp db");
            let summary = store
                .import_problems(&[problem("p1"), problem("p2")], "deck-a.csv")
                .expect("import deck");
            store
                .record_answer("p1", "B", "A", false)
                .expect("record answer");

            store.delete_deck(summary.deck_id).expect("delete deck");

            assert!(store.deck_cards().expect("deck cards").is_empty());
            assert_eq!(store.problem_count().expect("problem count"), 0);
            assert!(store.answer_history(10).expect("answer history").is_empty());
        }

        {
            let store = AppStore::open(path.clone()).expect("reopen temp db");
            assert!(
                store
                    .deck_cards()
                    .expect("deck cards after reopen")
                    .is_empty()
            );
            assert_eq!(
                store.problem_count().expect("problem count after reopen"),
                0
            );
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn deleting_one_deck_keeps_problems_used_by_other_decks() {
        let path = temp_db_path("delete-shared-deck");
        let mut store = AppStore::open(path.clone()).expect("open temp db");
        let first = store
            .import_problems(&[problem("shared")], "deck-a.csv")
            .expect("import first deck");
        let second = store
            .import_problems(&[problem("shared")], "deck-b.csv")
            .expect("import second deck");
        store
            .record_answer("shared", "A", "A", true)
            .expect("record shared answer");

        store.delete_deck(first.deck_id).expect("delete first deck");

        assert_eq!(store.problem_count().expect("problem count"), 1);
        assert_eq!(store.answer_history(10).expect("answer history").len(), 1);
        assert_eq!(
            store
                .load_deck_problems(second.deck_id)
                .expect("second deck problems")
                .len(),
            1
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn removing_deck_from_group_updates_group_problem_count() {
        let path = temp_db_path("remove-deck-from-group");
        let mut store = AppStore::open(path.clone()).expect("open temp db");
        let first = store
            .import_problems(&[problem("p1")], "deck-a.csv")
            .expect("import first deck");
        let second = store
            .import_problems(&[problem("p2")], "deck-b.csv")
            .expect("import second deck");

        let group_id = store.create_group("数学").expect("create group");
        store
            .add_deck_to_group(group_id, first.deck_id)
            .expect("add first deck");
        store
            .add_deck_to_group(group_id, second.deck_id)
            .expect("add second deck");

        let problems_before = store
            .load_group_problems(group_id)
            .expect("load group problems before");
        assert_eq!(problems_before.len(), 2);

        store
            .remove_deck_from_group(group_id, first.deck_id)
            .expect("remove first deck");

        let groups = store.group_cards().expect("group cards");
        let group = groups
            .iter()
            .find(|card| card.id == group_id)
            .expect("group exists");
        assert_eq!(group.deck_count, 1);
        assert_eq!(group.problem_count, 1);

        let problems_after = store
            .load_group_problems(group_id)
            .expect("load group problems after");
        assert_eq!(problems_after.len(), 1);
        assert_eq!(problems_after[0].id, "p2");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn problem_tags_can_be_updated_after_import() {
        let path = temp_db_path("update-problem-tags");
        let mut store = AppStore::open(path.clone()).expect("open temp db");
        let summary = store
            .import_problems(&[problem("p1")], "deck-a.csv")
            .expect("import deck");

        let mut problems = store
            .load_deck_problems(summary.deck_id)
            .expect("load deck problems");
        problems[0].tags.push("AI知识点:需求弹性".into());

        let updated = store
            .update_problem_tags(&problems)
            .expect("update problem tags");
        assert_eq!(updated, 1);

        let reloaded = store
            .load_deck_problems(summary.deck_id)
            .expect("reload deck problems");
        assert!(reloaded[0].tags.contains(&"AI知识点:需求弹性".into()));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn prefetched_explanations_are_written_back() {
        let path = temp_db_path("update-prefetched-explanations");
        let mut store = AppStore::open(path.clone()).expect("open temp db");
        let summary = store
            .import_problems(&[problem("p1"), problem("p2")], "deck-a.csv")
            .expect("import deck");

        let mut problems = store
            .load_deck_problems(summary.deck_id)
            .expect("load deck problems");
        problems[0].explanation = "AI预生成解析：\n需求曲线向右下方倾斜。".into();
        problems[1].explanation = "普通解析，不应由此接口覆盖。".into();

        let updated = store
            .update_problem_explanations(&problems)
            .expect("update prefetched explanations");
        assert_eq!(updated, 1);

        let reloaded = store
            .load_deck_problems(summary.deck_id)
            .expect("reload deck problems");
        assert!(reloaded[0].explanation.starts_with("AI预生成解析："));
        assert_eq!(reloaded[1].explanation, "");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn manual_answer_update_marks_problem_as_manually_reviewed() {
        let path = temp_db_path("manual-answer-update");
        let mut store = AppStore::open(path.clone()).expect("open temp db");
        let summary = store
            .import_problems(&[problem("p1")], "deck-a.csv")
            .expect("import deck");

        let mut problems = store
            .load_deck_problems(summary.deck_id)
            .expect("load deck problems");
        problems[0].state.user_answer = "B".into();
        problems[0].state.answer_source = ProblemAnswerSource::UserTemporary;
        problems[0].state.review_status = ProblemReviewStatus::Pending;
        problems[0].state.review_needed = true;
        store
            .import_problems(&problems, "deck-a.csv")
            .expect("persist pending state");

        let updated = store
            .update_problem_manual_answer("p1", "C")
            .expect("manual answer update");
        assert_eq!(updated, 1);

        let reloaded = store
            .load_deck_problems(summary.deck_id)
            .expect("reload deck problems");
        assert_eq!(reloaded[0].answer, "C");
        assert_eq!(reloaded[0].state.user_answer, "");
        assert_eq!(
            reloaded[0].state.answer_source,
            ProblemAnswerSource::ManualReviewed
        );
        assert_eq!(
            reloaded[0].state.review_status,
            ProblemReviewStatus::Accepted
        );
        assert_eq!(
            reloaded[0].state.review_confidence,
            ProblemReviewConfidence::High
        );
        assert!(!reloaded[0].state.review_needed);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn accepted_ai_review_answers_are_written_back_safely() {
        let path = temp_db_path("write-ai-review");
        let mut store = AppStore::open(path.clone()).expect("open temp db");
        let summary = store
            .import_problems(&[problem("p1"), problem("p2")], "deck-a.csv")
            .expect("import deck");

        let mut problems = store
            .load_deck_problems(summary.deck_id)
            .expect("load deck problems");
        problems[0].answer = "C".into();
        problems[0].explanation = "AI复核结果：错误".into();
        problems[0].state.answer_source = ProblemAnswerSource::AiReviewed;
        problems[0].state.review_status = ProblemReviewStatus::Accepted;
        problems[0].state.review_needed = false;
        problems[1].answer = "D".into();
        problems[1].state.answer_source = ProblemAnswerSource::UserTemporary;
        problems[1].state.review_status = ProblemReviewStatus::Unknown;
        problems[1].state.review_needed = true;

        let updated = store
            .update_ai_review_accepted_answers(&problems)
            .expect("write accepted ai review answers");
        assert_eq!(updated, 1);

        let reloaded = store
            .load_deck_problems(summary.deck_id)
            .expect("reload deck problems");
        assert_eq!(reloaded[0].answer, "C");
        assert_eq!(
            reloaded[0].state.answer_source,
            ProblemAnswerSource::AiReviewed
        );
        assert_eq!(
            reloaded[0].state.review_status,
            ProblemReviewStatus::Accepted
        );
        assert!(!reloaded[0].state.review_needed);
        assert_eq!(reloaded[1].answer, "A");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn app_settings_are_persisted() {
        let path = temp_db_path("settings");
        {
            let store = AppStore::open(path.clone()).expect("open temp db");
            store
                .set_setting("app_settings", r#"{"theme":"light"}"#)
                .expect("set setting");
        }

        {
            let store = AppStore::open(path.clone()).expect("reopen temp db");
            assert_eq!(
                store
                    .get_setting("app_settings")
                    .expect("get setting")
                    .as_deref(),
                Some(r#"{"theme":"light"}"#)
            );
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn app_settings_can_be_deleted() {
        let path = temp_db_path("delete-settings");
        let store = AppStore::open(path.clone()).expect("open temp db");

        store
            .set_setting("practice_session:test", r#"{"ok":true}"#)
            .expect("set setting");
        store
            .delete_setting("practice_session:test")
            .expect("delete setting");

        assert_eq!(
            store
                .get_setting("practice_session:test")
                .expect("get deleted setting"),
            None
        );

        let _ = std::fs::remove_file(path);
    }
}
