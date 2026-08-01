//! Repository trait and SeaORM-backed implementation for branches.

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use async_trait::async_trait;
use sea_orm::{
    sea_query::{LikeExpr, Query, SelectStatement},
    ActiveValue::{NotSet, Set},
    ColumnTrait, DatabaseConnection, DatabaseTransaction, DbBackend, EntityTrait, FromQueryResult,
    QueryFilter, QueryOrder, QuerySelect, Select, Statement, TransactionTrait,
};
use unclip_core::{
    parent_of, validate_branch_record, validate_reference, validate_sample_query, Branch,
    Reference, SampleQuery,
};
use unclip_entity::{
    branch_o2m_values, branch_o2o_values, branch_references, branches, usage_history,
};

use crate::mapper;
use crate::sqlite_limits::{sqlite_branch_id, ID_CHUNK, INSERT_ROW_CHUNK};
use crate::StoreError;

/// Fail broad queries before hydrating an unbounded archive into memory.
///
/// `find` returns fully assembled branches, including metadata and all child
/// rows. Ten thousand is intentionally conservative for a CLI process; larger
/// scans need a future streaming repository API rather than a larger `Vec`.
const MAX_FIND_RESULTS: u64 = 10_000;

/// Hard ceiling for commands that deliberately traverse a complete result set.
///
/// Pagination keeps individual SQL queries bounded, but returning a `Vec`
/// still retains the entire hydrated archive. Callers that need to exceed this
/// ceiling require a streaming API.
pub(crate) const MAX_BULK_RESULTS: usize = 100_000;

/// Page size used by bulk callers that intentionally consume every match.
const FIND_PAGE_SIZE: u64 = 1_000;

/// Backward-compatible name for the shared repository error contract.
pub type BranchRepositoryError = StoreError;

/// Typed result returned by the branch persistence boundary.
pub type BranchRepositoryResult<T> = Result<T, BranchRepositoryError>;

/// A distinct indexed value with how many branches carry it. Used to build
/// o2o/o2m catalogs (`unclip o2o`, `unclip o2m`).
#[derive(Debug, Clone, PartialEq, Eq, FromQueryResult)]
pub struct IndexedValue {
    pub name: String,
    pub value: String,
    pub count: i64,
}

/// Read half of the branch persistence boundary: navigation, filtered reads,
/// and catalog projections. Query-only commands depend on this alone, so a
/// handler's signature shows whether it can mutate the archive.
#[async_trait]
pub trait BranchReader: Sync {
    async fn get(&self, path: &str) -> BranchRepositoryResult<Option<Branch>>;

    async fn children(&self, path: &str) -> BranchRepositoryResult<Vec<Branch>>;
    async fn descendants(&self, path: &str) -> BranchRepositoryResult<Vec<Branch>>;
    async fn ancestors(&self, path: &str) -> BranchRepositoryResult<Vec<Branch>>;

    async fn find(&self, query: &SampleQuery) -> BranchRepositoryResult<Vec<Branch>>;
    /// Return one stable path-ordered page. `after_path` is an exclusive cursor.
    async fn find_page(
        &self,
        query: &SampleQuery,
        after_path: Option<&str>,
        limit: u64,
    ) -> BranchRepositoryResult<Vec<Branch>>;
    /// Consume every matching page. Bulk commands use this deliberately; code
    /// that samples candidates should prefer bounded [`Self::find`].
    async fn find_all(&self, query: SampleQuery) -> BranchRepositoryResult<Vec<Branch>> {
        let mut all = Vec::new();
        let mut after_path = None;
        loop {
            let page = self
                .find_page(&query, after_path.as_deref(), FIND_PAGE_SIZE)
                .await?;
            let done = page.len() < FIND_PAGE_SIZE as usize;
            after_path = page.last().map(|branch| branch.path.clone());
            if all.len() > MAX_BULK_RESULTS.saturating_sub(page.len()) {
                return Err(BranchRepositoryError::BulkQueryTooBroad {
                    limit: MAX_BULK_RESULTS,
                });
            }
            all.extend(page);
            if done {
                return Ok(all);
            }
        }
    }

    /// Distinct o2o `name=value` pairs with branch counts, optionally for a
    /// single name. Ordered by name then value.
    async fn o2o_catalog(&self, name: Option<&str>) -> BranchRepositoryResult<Vec<IndexedValue>>;
    /// Distinct o2m `name=value` pairs with branch counts, optionally for a
    /// single name. Ordered by name then value.
    async fn o2m_catalog(&self, name: Option<&str>) -> BranchRepositoryResult<Vec<IndexedValue>>;

    /// Branches carrying a specific o2o value.
    async fn branches_with_o2o(
        &self,
        name: &str,
        value: &str,
    ) -> BranchRepositoryResult<Vec<Branch>>;
    /// Branches carrying a specific o2m value.
    async fn branches_with_o2m(
        &self,
        name: &str,
        value: &str,
    ) -> BranchRepositoryResult<Vec<Branch>>;

    /// `(path, title)` for every branch that has a title.
    async fn titles(&self) -> BranchRepositoryResult<Vec<(String, String)>>;
}

/// Write half of the branch persistence boundary.
#[async_trait]
pub trait BranchWriter: Sync {
    async fn add(&self, branch: &Branch) -> BranchRepositoryResult<()>;
    async fn update(&self, branch: Branch) -> BranchRepositoryResult<()>;
    async fn delete(&self, path: &str) -> BranchRepositoryResult<()>;
    /// Delete a branch and every descendant atomically, returning how many
    /// branches were removed (0 when nothing matched the scope).
    async fn delete_subtree(&self, path: &str) -> BranchRepositoryResult<usize>;

    /// Attach a single reference to an existing branch.
    async fn attach_reference(
        &self,
        path: &str,
        reference: &Reference,
    ) -> BranchRepositoryResult<()>;

    /// Insert or replace many branches (upsert by path), returning the
    /// `(added, updated)` counts.
    ///
    /// Implementations must run the whole batch atomically.
    async fn upsert_many(&self, branches: Vec<Branch>) -> BranchRepositoryResult<(usize, usize)>;
}

/// The full persistence boundary for branches. Application logic depends on
/// these traits, not on SeaORM entities directly; blanket-implemented for any
/// type providing both halves.
pub trait BranchRepository: BranchReader + BranchWriter {}

impl<T: BranchReader + BranchWriter> BranchRepository for T {}

/// SeaORM implementation backed by SQLite.
pub struct SeaOrmBranchRepository {
    db: DatabaseConnection,
}

impl SeaOrmBranchRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Insert a branch's child rows (o2o/o2m/references) within a transaction.
    async fn insert_children(
        txn: &DatabaseTransaction,
        branch_id: i32,
        branch: &Branch,
    ) -> anyhow::Result<()> {
        let o2o = mapper::o2o_active_models(branch_id, branch);
        for chunk in o2o.chunks(INSERT_ROW_CHUNK) {
            branch_o2o_values::Entity::insert_many(chunk.iter().cloned())
                .exec(txn)
                .await?;
        }
        let o2m = mapper::o2m_active_models(branch_id, branch);
        for chunk in o2m.chunks(INSERT_ROW_CHUNK) {
            branch_o2m_values::Entity::insert_many(chunk.iter().cloned())
                .exec(txn)
                .await?;
        }
        let refs = mapper::reference_active_models(branch_id, branch);
        for chunk in refs.chunks(INSERT_ROW_CHUNK) {
            branch_references::Entity::insert_many(chunk.iter().cloned())
                .exec(txn)
                .await?;
        }
        Ok(())
    }

    /// Replace a branch's child rows wholesale (delete then re-insert) within a
    /// transaction.
    async fn replace_children(
        txn: &DatabaseTransaction,
        branch_id: i32,
        branch: &Branch,
    ) -> anyhow::Result<()> {
        branch_o2o_values::Entity::delete_many()
            .filter(branch_o2o_values::Column::BranchId.eq(branch_id))
            .exec(txn)
            .await?;
        branch_o2m_values::Entity::delete_many()
            .filter(branch_o2m_values::Column::BranchId.eq(branch_id))
            .exec(txn)
            .await?;
        branch_references::Entity::delete_many()
            .filter(branch_references::Column::BranchId.eq(branch_id))
            .exec(txn)
            .await?;
        Self::insert_children(txn, branch_id, branch).await
    }

    /// Load a branch's child rows and assemble the full domain value.
    ///
    /// Delegates to `hydrate_all` so single- and multi-branch loads share one
    /// grouping/assembly path.
    async fn hydrate(&self, model: branches::Model) -> anyhow::Result<Branch> {
        self.hydrate_all(vec![model])
            .await?
            .pop()
            .context("hydrate_all returned no branch for a single model")
    }

    /// Hydrate many branches in bounded batches, then group child rows per
    /// branch. This avoids both N+1 queries and SQLite's bound-variable limit.
    async fn hydrate_all(&self, models: Vec<branches::Model>) -> anyhow::Result<Vec<Branch>> {
        if models.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<i32> = models.iter().map(|m| m.id).collect();

        let mut o2o = Vec::new();
        let mut o2m = Vec::new();
        let mut refs = Vec::new();
        for chunk in ids.chunks(ID_CHUNK) {
            o2o.extend(
                branch_o2o_values::Entity::find()
                    .filter(branch_o2o_values::Column::BranchId.is_in(chunk.iter().copied()))
                    .all(&self.db)
                    .await?,
            );
            o2m.extend(
                branch_o2m_values::Entity::find()
                    .filter(branch_o2m_values::Column::BranchId.is_in(chunk.iter().copied()))
                    .all(&self.db)
                    .await?,
            );
            refs.extend(
                branch_references::Entity::find()
                    .filter(branch_references::Column::BranchId.is_in(chunk.iter().copied()))
                    .order_by_asc(branch_references::Column::Id)
                    .all(&self.db)
                    .await?,
            );
        }

        let mut o2o_by_id: HashMap<i32, Vec<branch_o2o_values::Model>> = HashMap::new();
        for row in o2o {
            o2o_by_id.entry(row.branch_id).or_default().push(row);
        }
        let mut o2m_by_id: HashMap<i32, Vec<branch_o2m_values::Model>> = HashMap::new();
        for row in o2m {
            o2m_by_id.entry(row.branch_id).or_default().push(row);
        }
        let mut refs_by_id: HashMap<i32, Vec<branch_references::Model>> = HashMap::new();
        for row in refs {
            refs_by_id.entry(row.branch_id).or_default().push(row);
        }

        let mut out = Vec::with_capacity(models.len());
        for model in models {
            let id = model.id;
            out.push(mapper::assemble_branch(
                model,
                o2o_by_id.remove(&id).unwrap_or_default(),
                o2m_by_id.remove(&id).unwrap_or_default(),
                refs_by_id.remove(&id).unwrap_or_default(),
            )?);
        }
        Ok(out)
    }

    async fn model_by_path(&self, path: &str) -> anyhow::Result<Option<branches::Model>> {
        Ok(branches::Entity::find()
            .filter(branches::Column::Path.eq(path))
            .one(&self.db)
            .await?)
    }

    async fn load_branches_by_ids(&self, ids: Vec<i32>) -> anyhow::Result<Vec<Branch>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut models = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(ID_CHUNK) {
            models.extend(
                branches::Entity::find()
                    .filter(branches::Column::Id.is_in(chunk.iter().copied()))
                    .all(&self.db)
                    .await?,
            );
        }
        self.hydrate_all(models).await
    }

    async fn index_catalog(
        &self,
        table: IndexTable,
        name: Option<&str>,
    ) -> anyhow::Result<Vec<IndexedValue>> {
        // The table name comes from a closed enum (never user input), so it is
        // safe to interpolate; `name` is always passed as a bound parameter.
        let (filter, values) = match name {
            Some(n) => ("WHERE name = ?", vec![n.into()]),
            None => ("", Vec::new()),
        };
        let sql = format!(
            "SELECT name, value, COUNT(*) AS count FROM {} {filter} \
             GROUP BY name, value ORDER BY name, value LIMIT {}",
            table.as_table(),
            MAX_BULK_RESULTS + 1
        );
        let rows = IndexedValue::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .all(&self.db)
        .await?;
        ensure_bulk_result_limit(rows.len())?;
        Ok(rows)
    }

    /// Build the SQL portion shared by bounded and paginated filtered reads.
    fn filtered_select(query: &SampleQuery) -> Select<branches::Entity> {
        let mut select = branches::Entity::find();
        if let Some(under) = &query.under {
            let under = under.trim_end_matches('/');
            select = select.filter(
                branches::Column::Path
                    .eq(under)
                    .or(branches::Column::Path.like(descendant_like(under))),
            );
        }
        for (name, value) in &query.require_o2o {
            select = select.filter(
                branches::Column::Id.in_subquery(o2o_subquery(name, std::slice::from_ref(value))),
            );
        }
        for (name, values) in &query.avoid_o2o {
            select =
                select.filter(branches::Column::Id.not_in_subquery(o2o_subquery(name, values)));
        }
        for (name, values) in &query.require_o2m {
            // o2m is a set: every requested value is a separate membership test.
            for value in values {
                select = select.filter(
                    branches::Column::Id
                        .in_subquery(o2m_subquery(name, std::slice::from_ref(value))),
                );
            }
        }
        for (name, values) in &query.avoid_o2m {
            select =
                select.filter(branches::Column::Id.not_in_subquery(o2m_subquery(name, values)));
        }
        select
    }
}

#[async_trait]
impl BranchReader for SeaOrmBranchRepository {
    async fn get(&self, path: &str) -> BranchRepositoryResult<Option<Branch>> {
        match self.model_by_path(path).await? {
            Some(model) => Ok(Some(self.hydrate(model).await?)),
            None => Ok(None),
        }
    }

    async fn children(&self, path: &str) -> BranchRepositoryResult<Vec<Branch>> {
        let models = branches::Entity::find()
            .filter(branches::Column::ParentPath.eq(path))
            .limit((MAX_BULK_RESULTS + 1) as u64)
            .all(&self.db)
            .await?;
        ensure_bulk_result_limit(models.len())?;
        self.hydrate_all(models).await.map_err(Into::into)
    }

    async fn descendants(&self, path: &str) -> BranchRepositoryResult<Vec<Branch>> {
        let models = branches::Entity::find()
            .filter(branches::Column::Path.like(descendant_like(path)))
            .limit((MAX_BULK_RESULTS + 1) as u64)
            .all(&self.db)
            .await?;
        ensure_bulk_result_limit(models.len())?;
        self.hydrate_all(models).await.map_err(Into::into)
    }

    async fn ancestors(&self, path: &str) -> BranchRepositoryResult<Vec<Branch>> {
        let mut paths = Vec::new();
        let mut current = path.to_string();
        while let Some(parent) = parent_of(&current) {
            paths.push(parent.clone());
            current = parent;
        }
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let models = branches::Entity::find()
            .filter(branches::Column::Path.is_in(paths))
            .all(&self.db)
            .await?;
        self.hydrate_all(models).await.map_err(Into::into)
    }

    async fn find(&self, query: &SampleQuery) -> BranchRepositoryResult<Vec<Branch>> {
        validate_sample_query(query)?;
        // Sampling is seeded, so candidate order is part of its reproducibility
        // contract. SQL row order is undefined without ORDER BY and may change
        // with SQLite versions, indexes, or query plans.
        let models = Self::filtered_select(query)
            .order_by_asc(branches::Column::Path)
            .limit(MAX_FIND_RESULTS + 1)
            .all(&self.db)
            .await?;
        if models.len() as u64 > MAX_FIND_RESULTS {
            return Err(BranchRepositoryError::QueryTooBroad {
                limit: MAX_FIND_RESULTS,
            });
        }
        self.hydrate_all(models).await.map_err(Into::into)
    }

    async fn find_page(
        &self,
        query: &SampleQuery,
        after_path: Option<&str>,
        limit: u64,
    ) -> BranchRepositoryResult<Vec<Branch>> {
        validate_sample_query(query)?;
        if limit == 0 {
            return Err(BranchRepositoryError::InvalidRequest {
                message: "page limit must be greater than zero".to_string(),
            });
        }
        if limit > MAX_FIND_RESULTS {
            return Err(BranchRepositoryError::InvalidRequest {
                message: format!("page limit must not exceed {MAX_FIND_RESULTS}"),
            });
        }
        let mut select = Self::filtered_select(query);
        if let Some(path) = after_path {
            select = select.filter(branches::Column::Path.gt(path));
        }
        let models = select
            .order_by_asc(branches::Column::Path)
            .limit(limit)
            .all(&self.db)
            .await?;
        self.hydrate_all(models).await.map_err(Into::into)
    }

    async fn o2o_catalog(&self, name: Option<&str>) -> BranchRepositoryResult<Vec<IndexedValue>> {
        self.index_catalog(IndexTable::O2o, name)
            .await
            .map_err(Into::into)
    }

    async fn o2m_catalog(&self, name: Option<&str>) -> BranchRepositoryResult<Vec<IndexedValue>> {
        self.index_catalog(IndexTable::O2m, name)
            .await
            .map_err(Into::into)
    }

    async fn branches_with_o2o(
        &self,
        name: &str,
        value: &str,
    ) -> BranchRepositoryResult<Vec<Branch>> {
        // Project only the branch ids rather than hydrating whole value rows.
        let ids = branch_o2o_values::Entity::find()
            .select_only()
            .column(branch_o2o_values::Column::BranchId)
            .filter(branch_o2o_values::Column::Name.eq(name))
            .filter(branch_o2o_values::Column::Value.eq(value))
            .limit((MAX_BULK_RESULTS + 1) as u64)
            .into_tuple::<i32>()
            .all(&self.db)
            .await?;
        ensure_bulk_result_limit(ids.len())?;
        self.load_branches_by_ids(ids).await.map_err(Into::into)
    }

    async fn branches_with_o2m(
        &self,
        name: &str,
        value: &str,
    ) -> BranchRepositoryResult<Vec<Branch>> {
        let ids = branch_o2m_values::Entity::find()
            .select_only()
            .column(branch_o2m_values::Column::BranchId)
            .filter(branch_o2m_values::Column::Name.eq(name))
            .filter(branch_o2m_values::Column::Value.eq(value))
            .limit((MAX_BULK_RESULTS + 1) as u64)
            .into_tuple::<i32>()
            .all(&self.db)
            .await?;
        ensure_bulk_result_limit(ids.len())?;
        self.load_branches_by_ids(ids).await.map_err(Into::into)
    }

    /// A projection — it loads only the two columns the matcher needs, avoiding
    /// the full o2o/o2m/reference hydration of `find`.
    async fn titles(&self) -> BranchRepositoryResult<Vec<(String, String)>> {
        #[derive(FromQueryResult)]
        struct TitleRow {
            path: String,
            title: Option<String>,
        }
        let rows = branches::Entity::find()
            .select_only()
            .column(branches::Column::Path)
            .column(branches::Column::Title)
            .filter(branches::Column::Title.is_not_null())
            .limit((MAX_BULK_RESULTS + 1) as u64)
            .into_model::<TitleRow>()
            .all(&self.db)
            .await?;
        ensure_bulk_result_limit(rows.len())?;
        Ok(rows
            .into_iter()
            .filter_map(|r| r.title.map(|t| (r.path, t)))
            .collect())
    }
}

#[async_trait]
impl BranchWriter for SeaOrmBranchRepository {
    async fn add(&self, branch: &Branch) -> BranchRepositoryResult<()> {
        validate_branch_record(branch)?;
        let now = crate::history::now();
        let txn = self.db.begin().await?;

        let am = mapper::branch_active_model(branch, &now, &now)?;
        // Map the unique-path violation to a typed error at the insert itself,
        // so two concurrent `add`s cannot race a check-then-insert window.
        let res = branches::Entity::insert(am)
            .exec(&txn)
            .await
            .map_err(|err| {
                if matches!(
                    err.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    BranchRepositoryError::AlreadyExists {
                        path: branch.path.clone(),
                    }
                } else {
                    BranchRepositoryError::Database(err)
                }
            })?;
        let branch_id = res.last_insert_id;

        Self::insert_children(&txn, branch_id, branch).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn update(&self, mut branch: Branch) -> BranchRepositoryResult<()> {
        validate_branch_record(&branch)?;
        let expected_id = sqlite_branch_id(
            branch
                .id
                .context("branch has no persistence id; reload it before updating")?,
        )?;
        let expected_revision = branch
            .revision
            .take()
            .context("branch has no persistence revision; reload it before updating")?;
        let existing = self.model_by_path(&branch.path).await?.ok_or_else(|| {
            BranchRepositoryError::NotFound {
                path: branch.path.clone(),
            }
        })?;
        if existing.id != expected_id {
            return Err(BranchRepositoryError::Conflict {
                path: branch.path.clone(),
            });
        }
        let branch_id = expected_id;
        let created_at = existing.created_at.clone();
        let next_revision = next_revision(&expected_revision)?;

        let txn = self.db.begin().await?;

        // Compare-and-swap the opaque revision before replacing child rows.
        // A concurrent editor that committed after this branch was loaded makes
        // the predicate match zero rows, preventing a silent lost update.
        branch.id = Some(branch_id as i64);
        let mut am = mapper::branch_active_model(&branch, &created_at, &next_revision)?;
        am.id = NotSet;
        branch.revision = Some(next_revision);
        let result = branches::Entity::update_many()
            .set(am)
            .filter(branches::Column::Id.eq(branch_id))
            .filter(branches::Column::UpdatedAt.eq(&expected_revision))
            .exec(&txn)
            .await?;
        if result.rows_affected != 1 {
            return Err(BranchRepositoryError::Conflict {
                path: branch.path.clone(),
            });
        }

        Self::replace_children(&txn, branch_id, &branch).await?;

        txn.commit().await?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> BranchRepositoryResult<()> {
        let Some(model) = self.model_by_path(path).await? else {
            return Ok(());
        };
        let branch_id = model.id;
        let txn = self.db.begin().await?;

        // Explicit child deletes so behavior does not depend on the
        // foreign_keys pragma being set on every pooled connection.
        branch_o2o_values::Entity::delete_many()
            .filter(branch_o2o_values::Column::BranchId.eq(branch_id))
            .exec(&txn)
            .await?;
        branch_o2m_values::Entity::delete_many()
            .filter(branch_o2m_values::Column::BranchId.eq(branch_id))
            .exec(&txn)
            .await?;
        branch_references::Entity::delete_many()
            .filter(branch_references::Column::BranchId.eq(branch_id))
            .exec(&txn)
            .await?;
        usage_history::Entity::delete_many()
            .filter(usage_history::Column::BranchId.eq(branch_id))
            .exec(&txn)
            .await?;
        branches::Entity::delete_by_id(branch_id).exec(&txn).await?;

        txn.commit().await?;
        Ok(())
    }

    /// Delete a branch and its whole subtree in one transaction.
    ///
    /// The id scan runs inside the same transaction as the deletes, so a
    /// branch added under the scope by a concurrent writer either survives
    /// (it committed first and is part of the scan) or fails to commit — it
    /// cannot be half-orphaned.
    async fn delete_subtree(&self, path: &str) -> BranchRepositoryResult<usize> {
        let txn = self.db.begin().await?;
        let ids: Vec<i32> = branches::Entity::find()
            .select_only()
            .column(branches::Column::Id)
            .filter(
                branches::Column::Path
                    .eq(path)
                    .or(branches::Column::Path.like(descendant_like(path))),
            )
            .limit((MAX_BULK_RESULTS + 1) as u64)
            .into_tuple::<i32>()
            .all(&txn)
            .await?;
        ensure_bulk_result_limit(ids.len())?;
        if ids.is_empty() {
            return Ok(0);
        }

        for chunk in ids.chunks(ID_CHUNK) {
            branch_o2o_values::Entity::delete_many()
                .filter(branch_o2o_values::Column::BranchId.is_in(chunk.iter().copied()))
                .exec(&txn)
                .await?;
            branch_o2m_values::Entity::delete_many()
                .filter(branch_o2m_values::Column::BranchId.is_in(chunk.iter().copied()))
                .exec(&txn)
                .await?;
            branch_references::Entity::delete_many()
                .filter(branch_references::Column::BranchId.is_in(chunk.iter().copied()))
                .exec(&txn)
                .await?;
            usage_history::Entity::delete_many()
                .filter(usage_history::Column::BranchId.is_in(chunk.iter().copied()))
                .exec(&txn)
                .await?;
            branches::Entity::delete_many()
                .filter(branches::Column::Id.is_in(chunk.iter().copied()))
                .exec(&txn)
                .await?;
        }

        txn.commit().await?;
        Ok(ids.len())
    }

    async fn attach_reference(
        &self,
        path: &str,
        reference: &Reference,
    ) -> BranchRepositoryResult<()> {
        validate_reference(reference)?;
        // Validate the resulting aggregate, not only the new reference. This
        // prevents repeated attachments from bypassing branch size/cardinality
        // limits that full add/update operations enforce.
        let mut aggregate =
            self.get(path)
                .await?
                .ok_or_else(|| BranchRepositoryError::NotFound {
                    path: path.to_string(),
                })?;
        let expected_revision = aggregate
            .revision
            .clone()
            .context("branch has no persistence revision; reload it before attaching")?;
        aggregate.references.push(reference.clone());
        validate_branch_record(&aggregate)?;

        let txn = self.db.begin().await?;
        let model = branches::Entity::find()
            .filter(branches::Column::Path.eq(path))
            .one(&txn)
            .await?
            .ok_or_else(|| BranchRepositoryError::NotFound {
                path: path.to_string(),
            })?;

        // Attaching a reference mutates the branch aggregate. Compare against
        // the revision whose complete aggregate was validated above, then
        // advance the same token used by full edits.
        if model.updated_at != expected_revision {
            return Err(BranchRepositoryError::Conflict {
                path: path.to_string(),
            });
        }
        let next_revision = next_revision(&expected_revision)?;
        let revision = branches::ActiveModel {
            updated_at: Set(next_revision),
            ..Default::default()
        };
        let result = branches::Entity::update_many()
            .set(revision)
            .filter(branches::Column::Id.eq(model.id))
            .filter(branches::Column::UpdatedAt.eq(&expected_revision))
            .exec(&txn)
            .await?;
        if result.rows_affected != 1 {
            return Err(BranchRepositoryError::Conflict {
                path: path.to_string(),
            });
        }

        let am = branch_references::ActiveModel {
            id: NotSet,
            branch_id: Set(model.id),
            r#type: Set(reference.kind.clone()),
            value: Set(reference.value.clone()),
            note: Set(reference.note.clone()),
        };
        branch_references::Entity::insert(am).exec(&txn).await?;
        txn.commit().await?;
        Ok(())
    }

    /// Atomic batch upsert: the whole set is applied in one transaction, so a
    /// failure on any branch rolls the entire import back.
    async fn upsert_many(&self, branches: Vec<Branch>) -> BranchRepositoryResult<(usize, usize)> {
        // Reject an ambiguous import before writing anything. Otherwise two
        // records for the same path would silently make the last one win and
        // misleadingly report one addition plus one update.
        let mut paths = HashSet::with_capacity(branches.len());
        for branch in &branches {
            validate_branch_record(branch)?;
            if !paths.insert(&branch.path) {
                return Err(BranchRepositoryError::InvalidRequest {
                    message: format!("duplicate branch path `{}` in import", branch.path),
                });
            }
        }

        let now = crate::history::now();
        let txn = self.db.begin().await?;
        let (mut added, mut updated) = (0usize, 0usize);

        // One chunked lookup for every incoming path instead of a SELECT per
        // branch; large imports would otherwise pay one round-trip per record.
        let all_paths: Vec<&str> = branches.iter().map(|b| b.path.as_str()).collect();
        let mut existing_by_path: HashMap<String, branches::Model> = HashMap::new();
        for chunk in all_paths.chunks(ID_CHUNK) {
            let models = branches::Entity::find()
                .filter(branches::Column::Path.is_in(chunk.iter().copied()))
                .all(&txn)
                .await?;
            existing_by_path.extend(models.into_iter().map(|m| (m.path.clone(), m)));
        }

        for mut branch in branches {
            match existing_by_path.remove(&branch.path) {
                Some(model) => {
                    let branch_id = model.id;
                    branch.id = Some(branch_id as i64);
                    // Imports replace the aggregate just like an edit, so they
                    // must also invalidate every previously loaded copy. Do
                    // not use the batch wall-clock timestamp directly: its
                    // millisecond precision can equal (or precede) the stored
                    // revision.
                    let revision = next_revision(&model.updated_at)?;
                    let am = mapper::branch_active_model(&branch, &model.created_at, &revision)?;
                    branches::Entity::update(am).exec(&txn).await?;
                    Self::replace_children(&txn, branch_id, &branch).await?;
                    updated += 1;
                }
                None => {
                    branch.id = None;
                    let am = mapper::branch_active_model(&branch, &now, &now)?;
                    let branch_id = branches::Entity::insert(am)
                        .exec(&txn)
                        .await?
                        .last_insert_id;
                    Self::insert_children(&txn, branch_id, &branch).await?;
                    added += 1;
                }
            }
        }

        txn.commit().await?;
        Ok((added, updated))
    }
}

pub(crate) fn ensure_bulk_result_limit(len: usize) -> BranchRepositoryResult<()> {
    if len > MAX_BULK_RESULTS {
        return Err(BranchRepositoryError::BulkQueryTooBroad {
            limit: MAX_BULK_RESULTS,
        });
    }
    Ok(())
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn bulk_result_limit_accepts_boundary_and_rejects_excess() {
        assert!(ensure_bulk_result_limit(MAX_BULK_RESULTS).is_ok());
        assert!(matches!(
            ensure_bulk_result_limit(MAX_BULK_RESULTS + 1),
            Err(BranchRepositoryError::BulkQueryTooBroad {
                limit: MAX_BULK_RESULTS
            })
        ));
    }
}

/// Produce a canonical timestamp that is strictly newer than `previous`.
///
/// The wall clock can have millisecond resolution or move backwards. Bumping
/// from the stored value in either case guarantees that a successful
/// compare-and-swap always changes the revision token.
fn next_revision(previous: &str) -> anyhow::Result<String> {
    use chrono::{DateTime, DurationRound, SecondsFormat, TimeDelta, Utc};

    let previous = DateTime::parse_from_rfc3339(previous)
        .context("branch has an invalid persistence revision")?
        .with_timezone(&Utc);
    // Truncate to the persisted millisecond precision so the comparison below
    // sees exactly the value that would be stored.
    let now = Utc::now()
        .duration_trunc(TimeDelta::milliseconds(1))
        .context("failed to create branch persistence revision")?;
    let next = if now > previous {
        now
    } else {
        previous
            .checked_add_signed(TimeDelta::milliseconds(1))
            .context("branch persistence revision overflow")?
    };
    Ok(next.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// The indexed child table a catalog query runs against. A closed enum so the
/// table name interpolated into `index_catalog`'s SQL can only ever be one of
/// these fixed identifiers.
#[derive(Debug, Clone, Copy)]
enum IndexTable {
    O2o,
    O2m,
}

impl IndexTable {
    fn as_table(self) -> &'static str {
        match self {
            IndexTable::O2o => "branch_o2o_values",
            IndexTable::O2m => "branch_o2m_values",
        }
    }
}

/// Build a `LIKE` pattern matching the strict descendants of `scope`.
///
/// Paths may legitimately contain `_` and `%`, which are SQL `LIKE`
/// metacharacters; without escaping, a scope like `/a_b` would also match
/// `/axb/...`. We escape `%`, `_`, and the `\` escape character itself and
/// emit an explicit `ESCAPE '\'` clause so only real descendants match.
fn descendant_like(scope: &str) -> LikeExpr {
    let scope = scope.trim_end_matches('/');
    let mut pattern = String::with_capacity(scope.len() + 2);
    for ch in scope.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push_str("/%");
    LikeExpr::new(pattern).escape('\\')
}

/// Subquery selecting branch ids that carry any of `values` under o2o `name`.
///
/// An empty `values` yields an empty result set, so `NOT IN (…)` keeps every
/// branch — matching "avoid nothing".
fn o2o_subquery(name: &str, values: &[String]) -> SelectStatement {
    Query::select()
        .column(branch_o2o_values::Column::BranchId)
        .from(branch_o2o_values::Entity)
        .and_where(branch_o2o_values::Column::Name.eq(name))
        .and_where(branch_o2o_values::Column::Value.is_in(values.to_vec()))
        .to_owned()
}

/// Subquery selecting branch ids that carry any of `values` under o2m `name`.
///
/// An empty `values` yields an empty result set, so `NOT IN (…)` keeps every
/// branch — matching "avoid nothing".
fn o2m_subquery(name: &str, values: &[String]) -> SelectStatement {
    Query::select()
        .column(branch_o2m_values::Column::BranchId)
        .from(branch_o2m_values::Entity)
        .and_where(branch_o2m_values::Column::Name.eq(name))
        .and_where(branch_o2m_values::Column::Value.is_in(values.to_vec()))
        .to_owned()
}
