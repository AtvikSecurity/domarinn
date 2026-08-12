//! The backend-neutral SQL executor: one API over rusqlite today and
//! Postgres tomorrow, so every query in `storage/` is written exactly once.
//!
//! Design rules this module enforces:
//!
//! * **SQL is written in SQLite's placeholder dialect (`?N`)** and translated
//!   to `$N` for Postgres at execution time by [`translate`]. Translation runs
//!   on the *final assembled* string, so the `format!`-based fragment builders
//!   (`visibility_predicate`, `cached_clause_sql`, the list-filter builders)
//!   compose exactly as they always have.
//! * **Parameters are [`Value`]s** — an owned, driver-free miniature of
//!   SQLite's type system (which is the intersection that matters: both
//!   engines round-trip i64/f64/text/bytes/NULL losslessly). The [`params!`]
//!   macro mirrors rusqlite's and borrows its arguments, so call sites keep
//!   reusing bindings after the call.
//! * **Rows come back through [`Row`]** and land in Rust via [`FromValue`],
//!   which owns the cross-engine coercions (SQLite has no BOOLEAN; Postgres
//!   is strict about integer widths) in one place instead of at 200 call
//!   sites.
//!
//! [`Conn`] and [`Tx`] are enums, not trait objects: the row-mapper closures
//! (`FnMut(&Row) -> anyhow::Result<T>`) make the query methods generic, which
//! rules out `dyn`. Helpers that serve both a connection and an open
//! transaction take `&mut impl Queryable` instead.
//!
//! Every method takes `&mut self` even though rusqlite only needs `&self`:
//! the Postgres client's query methods genuinely require `&mut`, and the
//! closure-based `Db::read`/`Db::write` API hands out exclusive access
//! anyway.

use std::borrow::Cow;

use anyhow::Context;

/// Which SQL engine a connection speaks. Exposed so the few genuinely
/// dialect-divergent fragments (FTS match/snippet/rank, the cache-entries id
/// column) can branch; everything else must stay dialect-blind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    Sqlite,
    Postgres,
}

/// An owned SQL parameter value.
///
/// The variant set is deliberately SQLite's storage classes and nothing more:
/// any richer type (timestamps, JSON, enums) is already encoded to one of
/// these by the schema's conventions (epoch-ms integers, opaque text, …).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl rusqlite::ToSql for Value {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef};
        Ok(match self {
            Value::Null => ToSqlOutput::Borrowed(ValueRef::Null),
            Value::Int(i) => ToSqlOutput::Borrowed(ValueRef::Integer(*i)),
            Value::Real(f) => ToSqlOutput::Borrowed(ValueRef::Real(*f)),
            Value::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
            Value::Blob(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

// ---------------------------------------------------------------------------
// IntoValue: what `params![]` accepts
// ---------------------------------------------------------------------------

/// Conversion into a [`Value`], implemented on *references* so the
/// [`params!`] macro borrows like rusqlite's did — a call site that binds
/// `key` twice keeps working unchanged.
pub(crate) trait IntoValue {
    fn into_value(&self) -> Value;
}

macro_rules! into_value_int {
    ($($t:ty),*) => {$(
        impl IntoValue for $t {
            fn into_value(&self) -> Value { Value::Int(*self as i64) }
        }
    )*};
}
into_value_int!(i64, i32, u32, u64, usize, bool);

impl IntoValue for f64 {
    fn into_value(&self) -> Value {
        Value::Real(*self)
    }
}

impl IntoValue for String {
    fn into_value(&self) -> Value {
        Value::Text(self.clone())
    }
}

impl IntoValue for str {
    fn into_value(&self) -> Value {
        Value::Text(self.to_owned())
    }
}

impl IntoValue for Vec<u8> {
    fn into_value(&self) -> Value {
        Value::Blob(self.clone())
    }
}

impl IntoValue for [u8] {
    fn into_value(&self) -> Value {
        Value::Blob(self.to_vec())
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(&self) -> Value {
        match self {
            Some(v) => v.into_value(),
            None => Value::Null,
        }
    }
}

impl<T: IntoValue + ?Sized> IntoValue for &T {
    fn into_value(&self) -> Value {
        (**self).into_value()
    }
}

impl IntoValue for Value {
    fn into_value(&self) -> Value {
        self.clone()
    }
}

macro_rules! into_value_from {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Value { IntoValue::into_value(&v) }
        }
    )*};
}
into_value_from!(i64, i32, u64, usize, bool, f64, String, &str, Vec<u8>);

impl<T> From<Option<T>> for Value
where
    Value: From<T>,
{
    fn from(v: Option<T>) -> Value {
        match v {
            Some(v) => Value::from(v),
            None => Value::Null,
        }
    }
}

/// Drop-in for `rusqlite::params!`: builds a `Vec<Value>`, borrowing each
/// argument.
macro_rules! params {
    () => { Vec::<$crate::storage::exec::Value>::new() };
    ($($v:expr),+ $(,)?) => {
        vec![$($crate::storage::exec::IntoValue::into_value(&$v)),+]
    };
}
pub(crate) use params;

// ---------------------------------------------------------------------------
// FromValue: what `Row::get` produces
// ---------------------------------------------------------------------------

/// Conversion out of an owned cell [`Value`], with the cross-engine
/// coercions: integers widen to `f64` where a float is asked for (SQLite
/// stores round floats as integers), and any non-zero integer is `true`.
pub(crate) trait FromValue: Sized {
    fn from_value(v: Value) -> anyhow::Result<Self>;
}

impl FromValue for i64 {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        match v {
            Value::Int(i) => Ok(i),
            other => anyhow::bail!("expected integer, got {other:?}"),
        }
    }
}

impl FromValue for f64 {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        match v {
            Value::Real(f) => Ok(f),
            Value::Int(i) => Ok(i as f64),
            other => anyhow::bail!("expected real, got {other:?}"),
        }
    }
}

impl FromValue for bool {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        Ok(i64::from_value(v)? != 0)
    }
}

impl FromValue for String {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        match v {
            Value::Text(s) => Ok(s),
            other => anyhow::bail!("expected text, got {other:?}"),
        }
    }
}

impl FromValue for Vec<u8> {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        match v {
            Value::Blob(b) => Ok(b),
            // SQLite happily hands back TEXT for a column written as BLOB by
            // an older build; bytes are bytes.
            Value::Text(s) => Ok(s.into_bytes()),
            other => anyhow::bail!("expected blob, got {other:?}"),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        match v {
            Value::Null => Ok(None),
            v => Ok(Some(T::from_value(v)?)),
        }
    }
}

impl FromValue for Value {
    fn from_value(v: Value) -> anyhow::Result<Self> {
        Ok(v)
    }
}

// ---------------------------------------------------------------------------
// Row
// ---------------------------------------------------------------------------

/// One result row, borrowed from whichever driver produced it.
pub(crate) enum Row<'a> {
    Sqlite(&'a rusqlite::Row<'a>),
    Pg(&'a tokio_postgres::Row),
}

impl Row<'_> {
    pub fn get<T: FromValue>(&self, idx: usize) -> anyhow::Result<T> {
        let value = match self {
            Row::Sqlite(row) => {
                use rusqlite::types::ValueRef;
                match row.get_ref(idx)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(i) => Value::Int(i),
                    ValueRef::Real(f) => Value::Real(f),
                    ValueRef::Text(t) => Value::Text(String::from_utf8_lossy(t).into_owned()),
                    ValueRef::Blob(b) => Value::Blob(b.to_vec()),
                }
            }
            // `FromSql for Value` narrows every wire type to the storage
            // classes above (bool → 0/1, float4 → f64, …).
            Row::Pg(row) => row.try_get::<usize, Value>(idx)?,
        };
        T::from_value(value).with_context(|| format!("column {idx}"))
    }
}

// ---------------------------------------------------------------------------
// Placeholder translation
// ---------------------------------------------------------------------------

/// Rewrite `?N` placeholders to `$N` for Postgres; identity for SQLite.
///
/// A character scan, not a regex: copy everything verbatim, except that
/// inside a `'…'` string literal (with `''` escapes) nothing is a
/// placeholder, and outside one a `?` followed by digits becomes `$` +
/// digits. A bare un-numbered `?` is a bug in the query (it would silently
/// bind by position on SQLite and be unmappable on Postgres), so it panics
/// in debug builds and is left untouched in release.
pub(crate) fn translate(dialect: Dialect, sql: &str) -> Cow<'_, str> {
    if dialect == Dialect::Sqlite || !sql.contains('?') {
        return Cow::Borrowed(sql);
    }
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\'' {
                // `''` is an escaped quote: stay inside the literal.
                if chars.peek() == Some(&'\'') {
                    out.push(chars.next().unwrap());
                } else {
                    in_string = false;
                }
            }
            continue;
        }
        match c {
            '\'' => {
                in_string = true;
                out.push(c);
            }
            '?' => {
                if chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                    out.push('$');
                    while chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                        out.push(chars.next().unwrap());
                    }
                } else {
                    debug_assert!(false, "bare `?` placeholder in SQL: {sql}");
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

// ---------------------------------------------------------------------------
// Queryable: the shared query API
// ---------------------------------------------------------------------------

/// The query surface shared by a plain connection and an open transaction.
/// Helpers that are called from both (`cacheindex::insert_fts`, the stats
/// readers) take `&mut impl Queryable`.
pub(crate) trait Queryable {
    fn dialect(&self) -> Dialect;

    fn execute(&mut self, sql: &str, params: &[Value]) -> anyhow::Result<u64>;

    /// One row or none. This replaces both rusqlite idioms: `.optional()?`
    /// (same semantics) and `.ok()` (which swallowed *every* error as
    /// "absent" — here a real failure propagates).
    fn query_row_opt<T>(
        &mut self,
        sql: &str,
        params: &[Value],
        f: impl FnOnce(&Row<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>>;

    /// All rows, materialized.
    fn query_map<T>(
        &mut self,
        sql: &str,
        params: &[Value],
        f: impl FnMut(&Row<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<Vec<T>>;

    /// One row, which must exist.
    fn query_row<T>(
        &mut self,
        sql: &str,
        params: &[Value],
        f: impl FnOnce(&Row<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.query_row_opt(sql, params, f)?
            .with_context(|| format!("query returned no rows: {sql}"))
    }
}

/// A live connection to whichever backend the [`super::Db`] opened.
///
/// The Postgres variant is `&mut` because the sync client's query methods
/// genuinely take `&mut self`; rusqlite's take `&self`. Both are exclusive
/// in practice — `Db` hands each closure sole ownership of its connection.
pub(crate) enum Conn<'c> {
    Sqlite(&'c rusqlite::Connection),
    Pg(&'c mut postgres::Client),
}

/// An open transaction. `BEGIN IMMEDIATE` on SQLite (writes must take the
/// file lock up front or risk a deferred-upgrade deadlock); a plain `BEGIN`
/// on Postgres, whose MVCC and row locks make READ COMMITTED correct for
/// every pattern in this crate.
pub(crate) enum Tx<'c> {
    Sqlite(rusqlite::Transaction<'c>),
    Pg(postgres::Transaction<'c>),
}

impl<'c> Conn<'c> {
    /// Open a write transaction (`BEGIN IMMEDIATE` on SQLite, plain `BEGIN`
    /// on Postgres).
    ///
    /// `new_unchecked` because [`Conn`] holds `&Connection`: the "unchecked"
    /// part is only rusqlite's compile-time guarantee that no *other* alias
    /// runs statements mid-transaction, and the `Db` writer mutex already
    /// guarantees this connection has exactly one user.
    pub fn immediate_tx(&mut self) -> anyhow::Result<Tx<'_>> {
        match self {
            Conn::Sqlite(conn) => Ok(Tx::Sqlite(rusqlite::Transaction::new_unchecked(
                conn,
                rusqlite::TransactionBehavior::Immediate,
            )?)),
            Conn::Pg(client) => Ok(Tx::Pg(client.transaction()?)),
        }
    }
}

impl Queryable for Conn<'_> {
    fn dialect(&self) -> Dialect {
        match self {
            Conn::Sqlite(_) => Dialect::Sqlite,
            Conn::Pg(_) => Dialect::Postgres,
        }
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> anyhow::Result<u64> {
        match self {
            Conn::Sqlite(conn) => sqlite_execute(conn, sql, params),
            Conn::Pg(client) => pg_execute(*client, sql, params),
        }
    }

    fn query_row_opt<T>(
        &mut self,
        sql: &str,
        params: &[Value],
        f: impl FnOnce(&Row<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        match self {
            Conn::Sqlite(conn) => sqlite_query_row_opt(conn, sql, params, f),
            Conn::Pg(client) => pg_query_row_opt(*client, sql, params, f),
        }
    }

    fn query_map<T>(
        &mut self,
        sql: &str,
        params: &[Value],
        f: impl FnMut(&Row<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<Vec<T>> {
        match self {
            Conn::Sqlite(conn) => sqlite_query_map(conn, sql, params, f),
            Conn::Pg(client) => pg_query_map(*client, sql, params, f),
        }
    }
}

impl Tx<'_> {
    pub fn commit(self) -> anyhow::Result<()> {
        match self {
            Tx::Sqlite(tx) => Ok(tx.commit()?),
            Tx::Pg(tx) => Ok(tx.commit()?),
        }
    }
}

impl Queryable for Tx<'_> {
    fn dialect(&self) -> Dialect {
        match self {
            Tx::Sqlite(_) => Dialect::Sqlite,
            Tx::Pg(_) => Dialect::Postgres,
        }
    }

    fn execute(&mut self, sql: &str, params: &[Value]) -> anyhow::Result<u64> {
        match self {
            Tx::Sqlite(tx) => sqlite_execute(tx, sql, params),
            Tx::Pg(tx) => pg_execute(tx, sql, params),
        }
    }

    fn query_row_opt<T>(
        &mut self,
        sql: &str,
        params: &[Value],
        f: impl FnOnce(&Row<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        match self {
            Tx::Sqlite(tx) => sqlite_query_row_opt(tx, sql, params, f),
            Tx::Pg(tx) => pg_query_row_opt(tx, sql, params, f),
        }
    }

    fn query_map<T>(
        &mut self,
        sql: &str,
        params: &[Value],
        f: impl FnMut(&Row<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<Vec<T>> {
        match self {
            Tx::Sqlite(tx) => sqlite_query_map(tx, sql, params, f),
            Tx::Pg(tx) => pg_query_map(tx, sql, params, f),
        }
    }
}

// ---------------------------------------------------------------------------
// rusqlite helpers
// ---------------------------------------------------------------------------

fn sqlite_execute(conn: &rusqlite::Connection, sql: &str, params: &[Value]) -> anyhow::Result<u64> {
    Ok(conn.execute(sql, rusqlite::params_from_iter(params.iter()))? as u64)
}

fn sqlite_query_row_opt<T>(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[Value],
    f: impl FnOnce(&Row<'_>) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
    match rows.next()? {
        Some(row) => Ok(Some(f(&Row::Sqlite(row))?)),
        None => Ok(None),
    }
}

fn sqlite_query_map<T>(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[Value],
    mut f: impl FnMut(&Row<'_>) -> anyhow::Result<T>,
) -> anyhow::Result<Vec<T>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(params.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(f(&Row::Sqlite(row))?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// postgres helpers
// ---------------------------------------------------------------------------

fn pg_params(params: &[Value]) -> Vec<&(dyn postgres::types::ToSql + Sync)> {
    params
        .iter()
        .map(|v| v as &(dyn postgres::types::ToSql + Sync))
        .collect()
}

fn pg_execute(
    client: &mut impl postgres::GenericClient,
    sql: &str,
    params: &[Value],
) -> anyhow::Result<u64> {
    let sql = translate(Dialect::Postgres, sql);
    Ok(client.execute(sql.as_ref(), &pg_params(params))?)
}

/// First row of however many, mirroring rusqlite's `query_row` (which never
/// errored on extra rows) — `Client::query_opt` would.
fn pg_query_row_opt<T>(
    client: &mut impl postgres::GenericClient,
    sql: &str,
    params: &[Value],
    f: impl FnOnce(&Row<'_>) -> anyhow::Result<T>,
) -> anyhow::Result<Option<T>> {
    let sql = translate(Dialect::Postgres, sql);
    let rows = client.query(sql.as_ref(), &pg_params(params))?;
    match rows.first() {
        Some(row) => Ok(Some(f(&Row::Pg(row))?)),
        None => Ok(None),
    }
}

fn pg_query_map<T>(
    client: &mut impl postgres::GenericClient,
    sql: &str,
    params: &[Value],
    mut f: impl FnMut(&Row<'_>) -> anyhow::Result<T>,
) -> anyhow::Result<Vec<T>> {
    let sql = translate(Dialect::Postgres, sql);
    let rows = client.query(sql.as_ref(), &pg_params(params))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(f(&Row::Pg(row))?);
    }
    Ok(out)
}

/// Bind a [`Value`] against whatever parameter type the server inferred.
///
/// This is the half of the dialect gap that placeholder translation can't
/// cover: Postgres types every parameter at prepare time, while the shared
/// SQL was written for SQLite's dynamic typing. Encoding by the *server's*
/// declared type (int8 vs int4 vs bool, text vs varchar) instead of by the
/// Rust type kills the "type inference tail" at one chokepoint.
impl postgres::types::ToSql for Value {
    fn to_sql(
        &self,
        ty: &postgres::types::Type,
        out: &mut bytes::BytesMut,
    ) -> Result<postgres::types::IsNull, Box<dyn std::error::Error + Sync + Send>> {
        use postgres::types::{IsNull, Type};
        match self {
            Value::Null => Ok(IsNull::Yes),
            Value::Int(i) => match *ty {
                Type::INT8 => i.to_sql(ty, out),
                Type::INT4 => i32::try_from(*i)?.to_sql(ty, out),
                Type::INT2 => i16::try_from(*i)?.to_sql(ty, out),
                Type::BOOL => (*i != 0).to_sql(ty, out),
                Type::FLOAT8 => (*i as f64).to_sql(ty, out),
                Type::OID => u32::try_from(*i)?.to_sql(ty, out),
                _ => Err(format!("cannot bind integer as {ty}").into()),
            },
            Value::Real(f) => match *ty {
                Type::FLOAT8 => f.to_sql(ty, out),
                Type::FLOAT4 => (*f as f32).to_sql(ty, out),
                _ => Err(format!("cannot bind real as {ty}").into()),
            },
            Value::Text(s) => s.to_sql(ty, out),
            Value::Blob(b) => b.as_slice().to_sql(ty, out),
        }
    }

    fn accepts(_ty: &postgres::types::Type) -> bool {
        // Per-value dispatch happens in `to_sql`; a static answer here would
        // have to be the union anyway.
        true
    }

    postgres::types::to_sql_checked!();
}

/// Decode any supported wire type into the [`Value`] storage classes.
///
/// `NUMERIC` is deliberately unsupported: shared SQL must `CAST` aggregates
/// to `BIGINT`/`DOUBLE PRECISION` instead (see the SUM sites), because a
/// silent lossy decode here would round money.
impl<'a> postgres::types::FromSql<'a> for Value {
    fn from_sql(
        ty: &postgres::types::Type,
        raw: &'a [u8],
    ) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        use postgres::types::Type;
        Ok(match *ty {
            Type::BOOL => Value::Int(bool::from_sql(ty, raw)? as i64),
            Type::INT2 => Value::Int(i16::from_sql(ty, raw)? as i64),
            Type::INT4 => Value::Int(i32::from_sql(ty, raw)? as i64),
            Type::INT8 => Value::Int(i64::from_sql(ty, raw)?),
            Type::OID => Value::Int(u32::from_sql(ty, raw)? as i64),
            Type::FLOAT4 => Value::Real(f32::from_sql(ty, raw)? as f64),
            Type::FLOAT8 => Value::Real(f64::from_sql(ty, raw)?),
            Type::TEXT | Type::VARCHAR | Type::NAME | Type::BPCHAR | Type::UNKNOWN => {
                Value::Text(String::from_sql(ty, raw)?)
            }
            Type::BYTEA => Value::Blob(<Vec<u8>>::from_sql(ty, raw)?),
            _ => return Err(format!("unsupported column type {ty}").into()),
        })
    }

    fn from_sql_null(
        _ty: &postgres::types::Type,
    ) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        Ok(Value::Null)
    }

    fn accepts(_ty: &postgres::types::Type) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_is_identity_for_sqlite() {
        assert_eq!(translate(Dialect::Sqlite, "SELECT ?1, ?2"), "SELECT ?1, ?2");
    }

    #[test]
    fn translate_rewrites_numbered_placeholders() {
        assert_eq!(
            translate(
                Dialect::Postgres,
                "SELECT a FROM t WHERE x = ?1 AND y IN (?2, ?13)"
            ),
            "SELECT a FROM t WHERE x = $1 AND y IN ($2, $13)"
        );
    }

    #[test]
    fn translate_skips_string_literals() {
        assert_eq!(
            translate(Dialect::Postgres, "SELECT '?1 literal', '''?2''', ?3"),
            "SELECT '?1 literal', '''?2''', $3"
        );
    }

    #[test]
    fn params_macro_borrows() {
        let key = String::from("k");
        let a = params![key, 7i64, Some(1.5f64), Option::<String>::None];
        assert_eq!(
            a,
            vec![
                Value::Text("k".into()),
                Value::Int(7),
                Value::Real(1.5),
                Value::Null
            ]
        );
        // `key` is still usable: the macro borrowed it.
        assert_eq!(key, "k");
    }

    #[test]
    fn from_value_coercions() {
        assert_eq!(f64::from_value(Value::Int(3)).unwrap(), 3.0);
        assert!(bool::from_value(Value::Int(1)).unwrap());
        assert!(!bool::from_value(Value::Int(0)).unwrap());
        assert_eq!(Option::<i64>::from_value(Value::Null).unwrap(), None);
    }

    #[test]
    fn immediate_tx_commits_through_conn() {
        let sqlite = rusqlite::Connection::open_in_memory().unwrap();
        sqlite.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();
        let mut conn = Conn::Sqlite(&sqlite);
        let mut tx = conn.immediate_tx().unwrap();
        tx.execute("INSERT INTO t (x) VALUES (?1)", &params![7i64])
            .unwrap();
        tx.commit().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM t", &params![], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
