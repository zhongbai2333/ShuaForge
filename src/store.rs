use crate::problem::{Problem, ProblemImage, ProblemType};
use rusqlite::{Connection, OptionalExtension, params};
use std::{error::Error, path::PathBuf};

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
        let conn = Connection::open(path)?;
        conn.execute_batch("pragma foreign_keys = on;")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn import_problems(
        &mut self,
        problems: &[Problem],
        source_path: &str,
    ) -> Result<ImportSummary, Box<dyn Error + Send + Sync>> {
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
                "insert into problems (id, prompt, answer, explanation, tags, problem_type, images_json, updated_at)
                 values (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now', 'localtime'))
                 on conflict(id) do update set
                   prompt = excluded.prompt,
                   answer = excluded.answer,
                   explanation = excluded.explanation,
                   tags = excluded.tags,
                   problem_type = excluded.problem_type,
                   images_json = excluded.images_json,
                   updated_at = datetime('now', 'localtime')",
                params![
                    problem.id,
                    problem.prompt,
                    problem.answer,
                    problem.explanation,
                    tags,
                    problem_type_to_str(problem.kind()),
                    images,
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
        Ok(self.conn.last_insert_rowid())
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
        Ok(())
    }

    pub fn delete_deck(&mut self, deck_id: i64) -> Result<(), Box<dyn Error + Send + Sync>> {
        let tx = self.conn.transaction()?;
        let problem_ids = {
            let mut stmt = tx.prepare("select problem_id from deck_problems where deck_id = ?1")?;
            let rows = stmt.query_map(params![deck_id], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        tx.execute("delete from decks where id = ?1", params![deck_id])?;

        for problem_id in problem_ids {
            tx.execute(
                "delete from problems
                 where id = ?1
                   and not exists (
                     select 1 from deck_problems where problem_id = ?1
                   )",
                params![problem_id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn delete_group(&self, group_id: i64) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.conn
            .execute("delete from deck_groups where id = ?1", params![group_id])?;
        Ok(())
    }

    pub fn load_all_problems(&self) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select id, prompt, answer, explanation, tags, problem_type, images_json from problems order by created_at asc, id asc",
        )?;
        let rows = stmt.query_map([], |row| {
            let tags_json: String = row.get(4)?;
            let tags = serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default();
            let problem_type_text: String = row.get(5)?;
            let images_json: String = row.get(6)?;
            Ok(Problem {
                id: row.get(0)?,
                prompt: row.get(1)?,
                answer: row.get(2)?,
                explanation: row.get(3)?,
                tags,
                problem_type: str_to_problem_type(&problem_type_text),
                deck_name: None,
                deck_info: None,
                images: serde_json::from_str::<Vec<ProblemImage>>(&images_json).unwrap_or_default(),
            })
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_deck_problems(
        &self,
        deck_id: i64,
    ) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select p.id, p.prompt, p.answer, p.explanation, p.tags, p.problem_type, p.images_json
             from problems p
             join deck_problems dp on dp.problem_id = p.id
             where dp.deck_id = ?1
             order by dp.added_at asc, p.id asc",
        )?;
        let rows = stmt.query_map(params![deck_id], |row| {
            let tags_json: String = row.get(4)?;
            let tags = serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default();
            let problem_type_text: String = row.get(5)?;
            let images_json: String = row.get(6)?;
            Ok(Problem {
                id: row.get(0)?,
                prompt: row.get(1)?,
                answer: row.get(2)?,
                explanation: row.get(3)?,
                tags,
                problem_type: str_to_problem_type(&problem_type_text),
                deck_name: None,
                deck_info: None,
                images: serde_json::from_str::<Vec<ProblemImage>>(&images_json).unwrap_or_default(),
            })
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn load_group_problems(
        &self,
        group_id: i64,
    ) -> Result<Vec<Problem>, Box<dyn Error + Send + Sync>> {
        let mut stmt = self.conn.prepare(
            "select distinct p.id, p.prompt, p.answer, p.explanation, p.tags, p.problem_type, p.images_json
             from problems p
             join deck_problems dp on dp.problem_id = p.id
             join group_decks gd on gd.deck_id = dp.deck_id
             where gd.group_id = ?1
             order by p.created_at asc, p.id asc",
        )?;
        let rows = stmt.query_map(params![group_id], |row| {
            let tags_json: String = row.get(4)?;
            let tags = serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default();
            let problem_type_text: String = row.get(5)?;
            let images_json: String = row.get(6)?;
            Ok(Problem {
                id: row.get(0)?,
                prompt: row.get(1)?,
                answer: row.get(2)?,
                explanation: row.get(3)?,
                tags,
                problem_type: str_to_problem_type(&problem_type_text),
                deck_name: None,
                deck_info: None,
                images: serde_json::from_str::<Vec<ProblemImage>>(&images_json).unwrap_or_default(),
            })
        })?;

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
        Ok(())
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
    let base = dirs::data_local_dir()
        .or_else(|| std::env::current_dir().ok())
        .ok_or("无法确定本地数据目录")?;
    Ok(base.join("ShuaForge").join("shuaforge.sqlite3"))
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

            store.delete_deck(summary.deck_id).expect("delete deck");

            assert!(store.deck_cards().expect("deck cards").is_empty());
            assert_eq!(store.problem_count().expect("problem count"), 0);
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

        store.delete_deck(first.deck_id).expect("delete first deck");

        assert_eq!(store.problem_count().expect("problem count"), 1);
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
}
