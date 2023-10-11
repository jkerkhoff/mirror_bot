use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{
    types::{FromSql, FromSqlError, ToSqlOutput},
    OptionalExtension, Row, ToSql,
};

use crate::{
    manifold::{LiteMarket, Managram, ManifoldMarket, TokenType},
    settings::Settings,
    types::{Question, QuestionSource},
};

pub fn open(config: &Settings) -> Result<rusqlite::Connection> {
    let db = rusqlite::Connection::open(&config.database.path)
        .with_context(|| "failed to connect to database")?;
    init_tables(&db)?;
    Ok(db)
}

pub fn init_tables(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "BEGIN;

        -- markets mirrored by the bot
        CREATE TABLE IF NOT EXISTS markets (
            id                      INTEGER PRIMARY KEY,
            clone_date              TEXT NOT NULL,
            manifold_contract_id    TEXT UNIQUE NOT NULL,
            manifold_url            TEXT NOT NULL,
            source                  TEXT NOT NULL,
            source_id               TEXT NOT NULL,
            source_url              TEXT NOT NULL,
            question                TEXT NOT NULL,
            resolved                INT NOT NULL CHECK( resolved IN (TRUE, FALSE) ) DEFAULT FALSE
        ) STRICT;
        CREATE UNIQUE INDEX IF NOT EXISTS markets_source_key ON markets (source, source_id);

        -- markets mirrored by others (avoid duplicating)
        CREATE TABLE IF NOT EXISTS third_party_markets (
            id                      INTEGER PRIMARY KEY,
            manifold_contract_id    TEXT UNIQUE NOT NULL,
            manifold_url            TEXT NOT NULL,
            source                  TEXT NOT NULL,
            source_id               TEXT NOT NULL,
            created_time            TEXT NOT NULL
        ) STRICT;

        -- managrams we have observed
        CREATE TABLE IF NOT EXISTS managrams (
            id                      INTEGER PRIMARY KEY,
            txn_id                  TEXT UNIQUE NOT NULL,
            group_id                TEXT NOT NULL,
            from_id                 TEXT NOT NULL,
            to_id                   TEXT NOT NULL,
            created_time            TEXT NOT NULL,
            token                   TEXT NOT NULL,
            amount                  REAL NOT NULL,   
            message                 TEXT NOT NULL,
            processed               INT NOT NULL CHECK( processed IN (TRUE, FALSE) ) DEFAULT FALSE
        ) STRICT;

        COMMIT;",
    )
    .with_context(|| "failed to initialize database tables")?;
    Ok(())
}

pub fn insert_managram(db: &rusqlite::Connection, managram: &Managram) -> Result<Managram> {
    let mut statement = db.prepare(
        "INSERT INTO MANAGRAMS (txn_id, group_id, from_id, to_id, created_time, token, amount, message)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING *"
    )?;
    Ok(statement.query_row(
        (
            &managram.id,
            &managram.group_id,
            &managram.from_id,
            &managram.to_id,
            &managram.created_time,
            &managram.token,
            &managram.amount,
            &managram.message,
        ),
        managram_row_helper,
    )?)
}

pub fn last_managram_timestamp(db: &rusqlite::Connection) -> Result<Option<DateTime<Utc>>> {
    Ok(db
        .query_row(
            "SELECT * FROM managrams ORDER BY datetime(created_time) DESC LIMIT 1",
            [],
            managram_row_helper,
        )
        .optional()?
        .map(|m| m.created_time))
}

pub fn get_unprocessed_managrams(db: &rusqlite::Connection) -> Result<Vec<Managram>> {
    let rows: rusqlite::Result<Vec<Managram>> = db
        .prepare("SELECT * FROM managrams WHERE processed = FALSE")?
        .query([])?
        .mapped(managram_row_helper)
        .collect();
    Ok(rows?)
}

pub fn set_managram_processed(db: &rusqlite::Connection, id: &str, processed: bool) -> Result<()> {
    let changed = db.execute(
        "UPDATE managrams SET processed = ?2 WHERE txn_id = ?1",
        (id, &processed),
    )?;
    if changed == 0 {
        return Err(anyhow!(
            "set_managram_processed query did not modify any rows"
        ));
    }
    Ok(())
}

pub fn insert_mirror(
    conn: &rusqlite::Connection,
    manifold_market: &LiteMarket,
    source_question: &Question,
    config: &Settings,
) -> Result<MirrorRow> {
    let mut statement = conn.prepare(
        "INSERT INTO markets (clone_date, manifold_contract_id, manifold_url, source, source_id, source_url, question)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING *",
    )?;
    Ok(statement.query_row(
        (
            Utc::now(),
            &manifold_market.id,
            manifold_market.url(config),
            &source_question.source,
            &source_question.source_id,
            &source_question.source_url,
            &source_question.question,
        ),
        MirrorRow::from_row,
    )?)
}

pub fn insert_third_party_mirror(
    conn: &rusqlite::Connection,
    manifold_market: &LiteMarket,
    source: &QuestionSource,
    source_id: &str,
    config: &Settings,
) -> Result<ThirdPartyMirrorRow> {
    let mut statement = conn.prepare(
        "INSERT INTO third_party_markets (manifold_contract_id, manifold_url, source, source_id, created_time)
        VALUES (?1, ?2, ?3, ?4, ?5) RETURNING *",
    )?;
    Ok(statement.query_row(
        (
            &manifold_market.id,
            &manifold_market.url(config),
            source,
            source_id,
            manifold_market.created_time,
        ),
        ThirdPartyMirrorRow::from_row,
    )?)
}

pub fn get_third_party_mirror_by_source_id(
    conn: &rusqlite::Connection,
    source: &QuestionSource,
    source_id: &str,
) -> Result<Option<ThirdPartyMirrorRow>> {
    Ok(conn
        .query_row(
            "SELECT * FROM third_party_markets WHERE source = ?1 AND source_id = ?2",
            (&source, &source_id),
            ThirdPartyMirrorRow::from_row,
        )
        .optional()?)
}

pub fn get_third_party_mirror_by_contract_id(
    conn: &rusqlite::Connection,
    contract_id: &str,
) -> Result<Option<ThirdPartyMirrorRow>> {
    Ok(conn
        .query_row(
            "SELECT * FROM third_party_markets WHERE manifold_contract_id = ?1",
            (&contract_id,),
            ThirdPartyMirrorRow::from_row,
        )
        .optional()?)
}

pub fn get_unresolved_mirrors(
    conn: &rusqlite::Connection,
    source: Option<QuestionSource>,
) -> Result<Vec<MirrorRow>> {
    let rows: rusqlite::Result<Vec<MirrorRow>> = if let Some(source) = source {
        conn.prepare("SELECT * FROM markets WHERE source = ?1 AND resolved = FALSE")?
            .query((&source,))?
            .mapped(MirrorRow::from_row)
            .collect()
    } else {
        conn.prepare("SELECT * FROM markets WHERE resolved = FALSE")?
            .query([])?
            .mapped(MirrorRow::from_row)
            .collect()
    };
    Ok(rows.with_context(|| "failed to fetch unresolved markets from db")?)
}

pub fn get_resolved_mirrors(
    conn: &rusqlite::Connection,
    source: Option<QuestionSource>,
) -> Result<Vec<MirrorRow>> {
    let rows: rusqlite::Result<Vec<MirrorRow>> = if let Some(source) = source {
        conn.prepare("SELECT * FROM markets WHERE source = ?1 AND resolved = TRUE")?
            .query((&source,))?
            .mapped(MirrorRow::from_row)
            .collect()
    } else {
        conn.prepare("SELECT * FROM markets WHERE resolved = TRUE")?
            .query([])?
            .mapped(MirrorRow::from_row)
            .collect()
    };
    Ok(rows.with_context(|| "failed to fetch unresolved markets from db")?)
}

pub fn get_mirrors(conn: &rusqlite::Connection) -> Result<Vec<MirrorRow>> {
    let rows: rusqlite::Result<Vec<MirrorRow>> = conn
        .prepare("SELECT * FROM markets")?
        .query([])?
        .mapped(MirrorRow::from_row)
        .collect();
    Ok(rows?)
}

pub fn get_third_party_mirrors(conn: &rusqlite::Connection) -> Result<Vec<ThirdPartyMirrorRow>> {
    let rows: rusqlite::Result<Vec<ThirdPartyMirrorRow>> = conn
        .prepare("SELECT * FROM third_party_markets")?
        .query([])?
        .mapped(ThirdPartyMirrorRow::from_row)
        .collect();
    Ok(rows?)
}

pub fn get_mirror_by_source_id(
    conn: &rusqlite::Connection,
    source: &QuestionSource,
    source_id: &str,
) -> Result<Option<MirrorRow>> {
    Ok(conn
        .query_row(
            "SELECT * FROM markets WHERE source = ?1 AND source_id = ?2",
            (&source, &source_id),
            MirrorRow::from_row,
        )
        .optional()?)
}

pub fn get_mirror_by_contract_id(
    conn: &rusqlite::Connection,
    contract_id: &str,
) -> Result<Option<MirrorRow>> {
    Ok(conn
        .query_row(
            "SELECT * FROM markets WHERE manifold_contract_id = ?1",
            (&contract_id,),
            MirrorRow::from_row,
        )
        .optional()?)
}

pub fn set_mirror_resolved(conn: &rusqlite::Connection, id: i64, resolved: bool) -> Result<()> {
    let changed = conn.execute(
        "UPDATE markets SET resolved = ?2 WHERE id = ?1",
        (id, &resolved),
    )?;
    if changed == 0 {
        return Err(anyhow!("set_market_resolved query did not modify any rows"));
    }
    Ok(())
}

pub fn get_any_mirror(
    db: &rusqlite::Connection,
    source: &QuestionSource,
    source_id: &str,
) -> Result<Option<AnyMirror>> {
    if let Some(mirror) = get_mirror_by_source_id(&db, source, source_id)? {
        return Ok(Some(AnyMirror::Mirror(mirror)));
    }
    if let Some(mirror) = get_third_party_mirror_by_source_id(&db, source, source_id)? {
        return Ok(Some(AnyMirror::ThirdPartyMirror(mirror)));
    }
    Ok(None)
}

#[derive(Debug)]
pub enum AnyMirror {
    Mirror(MirrorRow),
    ThirdPartyMirror(ThirdPartyMirrorRow),
}

impl AnyMirror {
    pub fn manifold_url(&self) -> &str {
        match self {
            AnyMirror::Mirror(m) => &m.manifold_url,
            AnyMirror::ThirdPartyMirror(m) => &m.manifold_url,
        }
    }
}

#[derive(Debug)]
pub struct MirrorRow {
    pub id: i64,
    pub clone_date: DateTime<Utc>,
    pub manifold_contract_id: String,
    pub manifold_url: String,
    pub source: QuestionSource,
    pub source_id: String,
    pub source_url: String,
    pub question: String,
    pub resolved: bool,
}

impl MirrorRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<MirrorRow> {
        Ok(MirrorRow {
            id: row.get("id")?,
            clone_date: row.get("clone_date")?,
            manifold_contract_id: row.get("manifold_contract_id")?,
            manifold_url: row.get("manifold_url")?,
            source: row.get("source")?,
            source_id: row.get("source_id")?,
            source_url: row.get("source_url")?,
            question: row.get("question")?,
            resolved: row.get("resolved")?,
        })
    }
}

#[derive(Debug)]
pub struct ThirdPartyMirrorRow {
    pub id: i64,
    pub manifold_contract_id: String,
    pub manifold_url: String,
    pub source: QuestionSource,
    pub source_id: String,
    pub created_time: DateTime<Utc>,
}

impl ThirdPartyMirrorRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<ThirdPartyMirrorRow> {
        Ok(ThirdPartyMirrorRow {
            id: row.get("id")?,
            manifold_contract_id: row.get("manifold_contract_id")?,
            manifold_url: row.get("manifold_url")?,
            source: row.get("source")?,
            source_id: row.get("source_id")?,
            created_time: row.get("created_time")?,
        })
    }
}

fn managram_row_helper(row: &Row<'_>) -> rusqlite::Result<Managram> {
    Ok(Managram {
        id: row.get("txn_id")?,
        group_id: row.get("group_id")?,
        from_id: row.get("from_id")?,
        to_id: row.get("to_id")?,
        created_time: row.get("created_time")?,
        token: row.get("token")?,
        amount: row.get("amount")?,
        message: row.get("message")?,
    })
}

impl ToSql for QuestionSource {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.to_string().to_uppercase()))
    }
}

impl FromSql for QuestionSource {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        Ok(match value.as_str()?.to_uppercase().as_str() {
            "KALSHI" => Self::Kalshi,
            "METACULUS" => Self::Metaculus,
            "POLYMARKET" => Self::Polymarket,
            _ => return Err(FromSqlError::InvalidType),
        })
    }
}

impl ToSql for TokenType {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.to_string().to_uppercase()))
    }
}

impl FromSql for TokenType {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        Ok(match value.as_str()?.to_uppercase().as_str() {
            "MANA" => Self::Mana,
            _ => return Err(FromSqlError::InvalidType),
        })
    }
}
